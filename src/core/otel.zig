/// Minimal OpenTelemetry client (OTLP/HTTP+JSON).
///
/// Scope. v0 supports HTTP/JSON only — gRPC requires protobuf and is left as
/// a follow-up. Trace and log records are buffered in memory; the caller
/// drains them with explicit `flush()` calls (typically once per RPC turn).
/// We stay single-threaded on purpose: the RPC server is single-threaded, so
/// adding a background flusher would only buy us hidden complexity (and Zig
/// 0.16's Mutex API ties locking to `std.Io`, which doesn't fit a long-lived
/// background thread cleanly).
///
/// Disabled mode. When `Config.endpoint` is empty (the default), `init`
/// returns a `*Otel` whose `enabled = false`. Every method becomes a no-op
/// and no allocations happen, so the harness pays nothing for observability
/// when the user has not opted in.
///
/// Memory. `SpanRecord` and `LogRecord` instances are heap-allocated
/// individually. `flush()` POSTs them in one batch per signal and frees the
/// records — even on POST failure, the records are dropped (the goal is
/// "audit a running session", not "lossless ingest"). When the queue
/// exceeds `max_queue_size`, oldest items are dropped silently and a
/// counter is bumped.
const std = @import("std");
const http = @import("http_client.zig");

pub const Header = struct {
    name: []const u8,
    value: []const u8,
};

/// Severity follows the OTLP severity number convention.
pub const Severity = enum(u8) {
    trace = 1,
    debug = 5,
    info = 9,
    warn = 13,
    err = 17,
    fatal = 21,

    pub fn text(self: Severity) []const u8 {
        return switch (self) {
            .trace => "TRACE",
            .debug => "DEBUG",
            .info => "INFO",
            .warn => "WARN",
            .err => "ERROR",
            .fatal => "FATAL",
        };
    }
};

pub const SpanStatusCode = enum(u8) {
    unset = 0,
    ok = 1,
    err = 2,
};

pub const SpanKind = enum(u8) {
    internal = 1,
    server = 2,
    client = 3,
    producer = 4,
    consumer = 5,
};

pub const SpanContext = struct {
    trace_id: [16]u8,
    span_id: [8]u8,
};

/// Compile-time config. `endpoint` empty disables OTel entirely.
pub const Config = struct {
    /// OTLP base URL (e.g., http://localhost:4318). Per-signal endpoints fall
    /// back to "<endpoint>/v1/traces" and "<endpoint>/v1/logs". Empty = off.
    endpoint: []const u8 = "",
    /// Override for traces signal (full path); takes precedence over `endpoint`.
    traces_endpoint: []const u8 = "",
    /// Override for logs signal (full path); takes precedence over `endpoint`.
    logs_endpoint: []const u8 = "",
    /// resource.service.name attribute on every emitted record.
    service_name: []const u8 = "phoenix",
    /// Extra HTTP headers (auth tokens, tenant IDs, etc.).
    headers: []const Header = &.{},
    /// Cap on queued items before oldest are dropped.
    max_queue_size: usize = 1024,
    /// Per-request timeout for the OTLP POST.
    request_timeout_ms: u64 = 5000,
    /// Reserved for a future async path. Currently unused.
    flush_interval_ms: u64 = 1000,
};

pub const AttrKind = enum { str, int, boolean, dbl };

pub const Attr = struct {
    key: []const u8,
    value: union(AttrKind) {
        str: []const u8,
        int: i64,
        boolean: bool,
        dbl: f64,
    },

    pub fn string(key: []const u8, value: []const u8) Attr {
        return .{ .key = key, .value = .{ .str = value } };
    }
    pub fn integer(key: []const u8, value: i64) Attr {
        return .{ .key = key, .value = .{ .int = value } };
    }
    pub fn boolean(key: []const u8, value: bool) Attr {
        return .{ .key = key, .value = .{ .boolean = value } };
    }
    pub fn double(key: []const u8, value: f64) Attr {
        return .{ .key = key, .value = .{ .dbl = value } };
    }
};

const StoredAttr = struct {
    key: []u8,
    value: union(AttrKind) {
        str: []u8,
        int: i64,
        boolean: bool,
        dbl: f64,
    },
};

pub const SpanRecord = struct {
    trace_id: [16]u8,
    span_id: [8]u8,
    parent_span_id: ?[8]u8,
    name: []u8,
    kind: SpanKind,
    start_unix_nano: u64,
    end_unix_nano: u64,
    status_code: SpanStatusCode,
    status_message: ?[]u8,
    attrs: std.ArrayList(StoredAttr),
};

pub const LogRecord = struct {
    time_unix_nano: u64,
    severity: Severity,
    body: []u8,
    attrs: std.ArrayList(StoredAttr),
    trace_id: ?[16]u8,
    span_id: ?[8]u8,
};

/// Span handle returned by `Otel.startSpan`. Methods are no-ops when
/// `record == null` (disabled mode or alloc failure during start).
pub const Span = struct {
    otel: ?*Otel = null,
    record: ?*SpanRecord = null,

    pub fn context(self: Span) ?SpanContext {
        if (self.record) |r| return .{ .trace_id = r.trace_id, .span_id = r.span_id };
        return null;
    }

    pub fn setAttr(self: Span, attr: Attr) void {
        const o = self.otel orelse return;
        const r = self.record orelse return;
        o.appendAttr(&r.attrs, attr) catch {};
    }

    pub fn setAttrs(self: Span, attrs: []const Attr) void {
        for (attrs) |a| self.setAttr(a);
    }

    pub fn setStatusOk(self: Span) void {
        if (self.record) |r| r.status_code = .ok;
    }

    pub fn setStatusError(self: Span, msg: []const u8) void {
        const o = self.otel orelse return;
        const r = self.record orelse return;
        r.status_code = .err;
        if (r.status_message) |old| o.allocator.free(old);
        r.status_message = o.allocator.dupe(u8, msg) catch null;
    }

    pub fn end(self: *Span) void {
        if (self.otel) |o| if (self.record) |r| {
            r.end_unix_nano = nowUnixNano();
            o.submitSpan(r);
        };
        self.otel = null;
        self.record = null;
    }
};

/// Statistics surfaced via stderr at shutdown so silent loss is visible.
pub const Stats = struct {
    spans_submitted: u64 = 0,
    logs_submitted: u64 = 0,
    spans_dropped: u64 = 0,
    logs_dropped: u64 = 0,
    flushes_ok: u64 = 0,
    flushes_failed: u64 = 0,
};

pub const Otel = struct {
    allocator: std.mem.Allocator,
    io: std.Io,
    enabled: bool,
    /// True iff `cfg` holds heap-allocated dups (only when enabled).
    /// Disabled instances skip OwnedConfig.deinit to avoid double-free.
    cfg_owned: bool,
    cfg: OwnedConfig,

    spans: std.ArrayList(*SpanRecord) = .empty,
    logs: std.ArrayList(*LogRecord) = .empty,
    stats: Stats = .{},
    rng: std.Random.DefaultPrng,

    /// Construct an `Otel` instance. NEVER fails. If the user did not
    /// configure an endpoint, returns a disabled instance whose every
    /// method is a no-op. If the user did configure one but allocation
    /// fails (genuine OOM), logs a warning and still returns a disabled
    /// instance — the agent loop must never break because OTel can't
    /// allocate.
    pub fn init(allocator: std.mem.Allocator, io: std.Io, cfg: Config) Otel {
        const want_enabled = cfg.endpoint.len > 0 or cfg.traces_endpoint.len > 0 or cfg.logs_endpoint.len > 0;
        if (!want_enabled) {
            return .{
                .allocator = allocator,
                .io = io,
                .enabled = false,
                .cfg_owned = false,
                .cfg = OwnedConfig.disabled(),
                .rng = std.Random.DefaultPrng.init(nowUnixNano()),
            };
        }

        const owned = OwnedConfig.dup(allocator, cfg) catch {
            std.log.warn("otel: configured but failed to allocate state; disabling", .{});
            return .{
                .allocator = allocator,
                .io = io,
                .enabled = false,
                .cfg_owned = false,
                .cfg = OwnedConfig.disabled(),
                .rng = std.Random.DefaultPrng.init(nowUnixNano()),
            };
        };
        return .{
            .allocator = allocator,
            .io = io,
            .enabled = true,
            .cfg_owned = true,
            .cfg = owned,
            .rng = std.Random.DefaultPrng.init(nowUnixNano()),
        };
    }

    pub fn deinit(self: *Otel) void {
        if (self.enabled) {
            self.flush();
            std.log.info(
                "otel: shutdown — spans_sub={d} logs_sub={d} spans_drop={d} logs_drop={d} flushes_ok={d} flushes_failed={d}",
                .{
                    self.stats.spans_submitted,
                    self.stats.logs_submitted,
                    self.stats.spans_dropped,
                    self.stats.logs_dropped,
                    self.stats.flushes_ok,
                    self.stats.flushes_failed,
                },
            );
        }

        for (self.spans.items) |r| destroySpan(self.allocator, r);
        self.spans.deinit(self.allocator);
        for (self.logs.items) |r| destroyLog(self.allocator, r);
        self.logs.deinit(self.allocator);

        if (self.cfg_owned) self.cfg.deinit(self.allocator);
    }

    pub fn startSpan(self: *Otel, name: []const u8, kind: SpanKind, parent: ?SpanContext) Span {
        if (!self.enabled) return .{};

        const rec = self.allocator.create(SpanRecord) catch {
            self.stats.spans_dropped += 1;
            return .{};
        };
        rec.* = .{
            .trace_id = undefined,
            .span_id = undefined,
            .parent_span_id = null,
            .name = self.allocator.dupe(u8, name) catch {
                self.allocator.destroy(rec);
                self.stats.spans_dropped += 1;
                return .{};
            },
            .kind = kind,
            .start_unix_nano = nowUnixNano(),
            .end_unix_nano = 0,
            .status_code = .unset,
            .status_message = null,
            .attrs = .empty,
        };

        if (parent) |p| {
            rec.trace_id = p.trace_id;
            rec.parent_span_id = p.span_id;
        } else {
            self.rng.fill(&rec.trace_id);
        }
        self.rng.fill(&rec.span_id);

        return .{ .otel = self, .record = rec };
    }

    pub fn emitLog(
        self: *Otel,
        severity: Severity,
        body: []const u8,
        attrs: []const Attr,
        parent: ?SpanContext,
    ) void {
        if (!self.enabled) return;

        const rec = self.allocator.create(LogRecord) catch {
            self.stats.logs_dropped += 1;
            return;
        };
        rec.* = .{
            .time_unix_nano = nowUnixNano(),
            .severity = severity,
            .body = self.allocator.dupe(u8, body) catch {
                self.allocator.destroy(rec);
                self.stats.logs_dropped += 1;
                return;
            },
            .attrs = .empty,
            .trace_id = if (parent) |p| p.trace_id else null,
            .span_id = if (parent) |p| p.span_id else null,
        };

        for (attrs) |a| self.appendAttr(&rec.attrs, a) catch {};

        if (self.logs.items.len >= self.cfg.max_queue_size) {
            const oldest = self.logs.orderedRemove(0);
            destroyLog(self.allocator, oldest);
            self.stats.logs_dropped += 1;
        }
        self.logs.append(self.allocator, rec) catch {
            destroyLog(self.allocator, rec);
            self.stats.logs_dropped += 1;
            return;
        };
        self.stats.logs_submitted += 1;
    }

    fn submitSpan(self: *Otel, rec: *SpanRecord) void {
        if (self.spans.items.len >= self.cfg.max_queue_size) {
            const oldest = self.spans.orderedRemove(0);
            destroySpan(self.allocator, oldest);
            self.stats.spans_dropped += 1;
        }
        self.spans.append(self.allocator, rec) catch {
            destroySpan(self.allocator, rec);
            self.stats.spans_dropped += 1;
            return;
        };
        self.stats.spans_submitted += 1;
    }

    fn appendAttr(self: *Otel, list: *std.ArrayList(StoredAttr), a: Attr) !void {
        const key = try self.allocator.dupe(u8, a.key);
        errdefer self.allocator.free(key);
        var stored: StoredAttr = .{ .key = key, .value = .{ .int = 0 } };
        switch (a.value) {
            .str => |s| stored.value = .{ .str = try self.allocator.dupe(u8, s) },
            .int => |i| stored.value = .{ .int = i },
            .boolean => |b| stored.value = .{ .boolean = b },
            .dbl => |d| stored.value = .{ .dbl = d },
        }
        try list.append(self.allocator, stored);
    }

    /// Drain all queued spans and logs to the OTLP collector. Safe to call
    /// from a hot path — no-op when nothing is queued, and disabled
    /// instances skip the call entirely.
    pub fn flush(self: *Otel) void {
        if (!self.enabled) return;

        if (self.spans.items.len > 0) {
            const ok = self.postTraces(self.spans.items);
            if (ok) self.stats.flushes_ok += 1 else self.stats.flushes_failed += 1;
            for (self.spans.items) |r| destroySpan(self.allocator, r);
            self.spans.clearRetainingCapacity();
        }
        if (self.logs.items.len > 0) {
            const ok = self.postLogs(self.logs.items);
            if (ok) self.stats.flushes_ok += 1 else self.stats.flushes_failed += 1;
            for (self.logs.items) |r| destroyLog(self.allocator, r);
            self.logs.clearRetainingCapacity();
        }
    }

    fn postTraces(self: *Otel, records: []const *SpanRecord) bool {
        var body: std.ArrayList(u8) = .empty;
        defer body.deinit(self.allocator);
        writeTracesJson(&body, self.allocator, self.cfg.service_name, records) catch return false;

        const url = self.signalUrl(.traces) catch return false;
        defer self.allocator.free(url);
        return self.postOnce(url, body.items);
    }

    fn postLogs(self: *Otel, records: []const *LogRecord) bool {
        var body: std.ArrayList(u8) = .empty;
        defer body.deinit(self.allocator);
        writeLogsJson(&body, self.allocator, self.cfg.service_name, records) catch return false;

        const url = self.signalUrl(.logs) catch return false;
        defer self.allocator.free(url);
        return self.postOnce(url, body.items);
    }

    const Signal = enum { traces, logs };
    fn signalUrl(self: *Otel, signal: Signal) ![]u8 {
        const override = switch (signal) {
            .traces => self.cfg.traces_endpoint,
            .logs => self.cfg.logs_endpoint,
        };
        if (override.len > 0) return self.allocator.dupe(u8, override);

        const path = switch (signal) {
            .traces => "/v1/traces",
            .logs => "/v1/logs",
        };
        const trimmed = std.mem.trimEnd(u8, self.cfg.endpoint, "/");
        return std.fmt.allocPrint(self.allocator, "{s}{s}", .{ trimmed, path });
    }

    fn postOnce(self: *Otel, url: []const u8, body: []const u8) bool {
        var hdrs: std.ArrayList(http.Header) = .empty;
        defer hdrs.deinit(self.allocator);
        hdrs.append(self.allocator, .{ .name = "Content-Type", .value = "application/json" }) catch {
            std.log.warn("otel: could not build headers (out of memory); dropping batch", .{});
            return false;
        };
        hdrs.append(self.allocator, .{ .name = "Accept", .value = "application/json" }) catch {
            std.log.warn("otel: could not build headers (out of memory); dropping batch", .{});
            return false;
        };
        for (self.cfg.headers) |h| {
            hdrs.append(self.allocator, .{ .name = h.name, .value = h.value }) catch {
                std.log.warn("otel: could not build headers (out of memory); dropping batch", .{});
                return false;
            };
        }

        var resp = http.post(self.allocator, self.io, .{
            .url = url,
            .body = body,
            .headers = hdrs.items,
            .timeout_ms = self.cfg.request_timeout_ms,
        }) catch |err| {
            // Submit failure is never fatal — a flaky collector should not
            // disrupt the agent. Surface as a warning so the operator knows
            // the trace tree is incomplete.
            std.log.warn("otel: POST {s} failed: {}", .{ url, err });
            return false;
        };
        defer resp.deinit();
        if (resp.status < 200 or resp.status >= 300) {
            std.log.warn("otel: POST {s} returned status={d}", .{ url, resp.status });
            return false;
        }
        return true;
    }
};

const OwnedConfig = struct {
    /// In `enabled` mode these slices are heap-allocated dups owned by
    /// `Otel.allocator`. In `disabled` mode they point at static literals
    /// and `cfg_owned = false` skips `deinit`.
    endpoint: []const u8,
    traces_endpoint: []const u8,
    logs_endpoint: []const u8,
    service_name: []const u8,
    headers: []const OwnedHeader,
    flush_interval_ms: u64,
    max_queue_size: usize,
    request_timeout_ms: u64,

    const OwnedHeader = struct { name: []const u8, value: []const u8 };

    fn disabled() OwnedConfig {
        return .{
            .endpoint = "",
            .traces_endpoint = "",
            .logs_endpoint = "",
            .service_name = "phoenix",
            .headers = &.{},
            .flush_interval_ms = 0,
            .max_queue_size = 0,
            .request_timeout_ms = 0,
        };
    }

    fn dup(allocator: std.mem.Allocator, cfg: Config) !OwnedConfig {
        const ep = try allocator.dupe(u8, cfg.endpoint);
        errdefer allocator.free(ep);
        const tep = try allocator.dupe(u8, cfg.traces_endpoint);
        errdefer allocator.free(tep);
        const lep = try allocator.dupe(u8, cfg.logs_endpoint);
        errdefer allocator.free(lep);
        const svc = try allocator.dupe(u8, cfg.service_name);
        errdefer allocator.free(svc);
        const hdrs = try allocator.alloc(OwnedHeader, cfg.headers.len);
        errdefer allocator.free(hdrs);
        var i: usize = 0;
        errdefer for (hdrs[0..i]) |h| {
            allocator.free(h.name);
            allocator.free(h.value);
        };
        while (i < cfg.headers.len) : (i += 1) {
            hdrs[i] = .{
                .name = try allocator.dupe(u8, cfg.headers[i].name),
                .value = try allocator.dupe(u8, cfg.headers[i].value),
            };
        }
        return .{
            .endpoint = ep,
            .traces_endpoint = tep,
            .logs_endpoint = lep,
            .service_name = svc,
            .headers = hdrs,
            .flush_interval_ms = cfg.flush_interval_ms,
            .max_queue_size = cfg.max_queue_size,
            .request_timeout_ms = cfg.request_timeout_ms,
        };
    }

    fn deinit(self: OwnedConfig, allocator: std.mem.Allocator) void {
        allocator.free(self.endpoint);
        allocator.free(self.traces_endpoint);
        allocator.free(self.logs_endpoint);
        allocator.free(self.service_name);
        for (self.headers) |h| {
            allocator.free(h.name);
            allocator.free(h.value);
        }
        allocator.free(self.headers);
    }
};

fn destroySpan(allocator: std.mem.Allocator, r: *SpanRecord) void {
    allocator.free(r.name);
    if (r.status_message) |m| allocator.free(m);
    for (r.attrs.items) |a| {
        allocator.free(a.key);
        switch (a.value) {
            .str => |s| allocator.free(s),
            else => {},
        }
    }
    var attrs = r.attrs;
    attrs.deinit(allocator);
    allocator.destroy(r);
}

fn destroyLog(allocator: std.mem.Allocator, r: *LogRecord) void {
    allocator.free(r.body);
    for (r.attrs.items) |a| {
        allocator.free(a.key);
        switch (a.value) {
            .str => |s| allocator.free(s),
            else => {},
        }
    }
    var attrs = r.attrs;
    attrs.deinit(allocator);
    allocator.destroy(r);
}

pub fn nowUnixNano() u64 {
    var ts: std.c.timespec = undefined;
    if (std.c.clock_gettime(std.c.CLOCK.REALTIME, &ts) != 0) return 0;
    const sec: u64 = @intCast(ts.sec);
    const nsec: u64 = @intCast(ts.nsec);
    return sec * std.time.ns_per_s + nsec;
}

// ---- OTLP/JSON encoding ----

fn writeTracesJson(
    out: *std.ArrayList(u8),
    a: std.mem.Allocator,
    service_name: []const u8,
    records: []const *SpanRecord,
) !void {
    try out.appendSlice(a, "{\"resourceSpans\":[{\"resource\":");
    try writeResource(out, a, service_name);
    try out.appendSlice(a, ",\"scopeSpans\":[{\"scope\":{\"name\":\"phoenix\"},\"spans\":[");
    for (records, 0..) |r, i| {
        if (i > 0) try out.append(a, ',');
        try writeSpan(out, a, r);
    }
    try out.appendSlice(a, "]}]}]}");
}

fn writeLogsJson(
    out: *std.ArrayList(u8),
    a: std.mem.Allocator,
    service_name: []const u8,
    records: []const *LogRecord,
) !void {
    try out.appendSlice(a, "{\"resourceLogs\":[{\"resource\":");
    try writeResource(out, a, service_name);
    try out.appendSlice(a, ",\"scopeLogs\":[{\"scope\":{\"name\":\"phoenix\"},\"logRecords\":[");
    for (records, 0..) |r, i| {
        if (i > 0) try out.append(a, ',');
        try writeLog(out, a, r);
    }
    try out.appendSlice(a, "]}]}]}");
}

fn writeResource(out: *std.ArrayList(u8), a: std.mem.Allocator, service_name: []const u8) !void {
    try out.appendSlice(a, "{\"attributes\":[{\"key\":\"service.name\",\"value\":{\"stringValue\":");
    try writeJsonString(out, a, service_name);
    try out.appendSlice(a, "}}]}");
}

fn writeSpan(out: *std.ArrayList(u8), a: std.mem.Allocator, r: *const SpanRecord) !void {
    try out.appendSlice(a, "{\"traceId\":\"");
    try writeHex(out, a, &r.trace_id);
    try out.appendSlice(a, "\",\"spanId\":\"");
    try writeHex(out, a, &r.span_id);
    try out.append(a, '"');
    if (r.parent_span_id) |p| {
        try out.appendSlice(a, ",\"parentSpanId\":\"");
        try writeHex(out, a, &p);
        try out.append(a, '"');
    }
    try out.appendSlice(a, ",\"name\":");
    try writeJsonString(out, a, r.name);
    try out.print(a, ",\"kind\":{d},\"startTimeUnixNano\":\"{d}\",\"endTimeUnixNano\":\"{d}\"", .{
        @intFromEnum(r.kind),
        r.start_unix_nano,
        r.end_unix_nano,
    });
    if (r.attrs.items.len > 0) {
        try out.appendSlice(a, ",\"attributes\":");
        try writeAttrs(out, a, r.attrs.items);
    }
    if (r.status_code != .unset) {
        try out.print(a, ",\"status\":{{\"code\":{d}", .{@intFromEnum(r.status_code)});
        if (r.status_message) |m| {
            try out.appendSlice(a, ",\"message\":");
            try writeJsonString(out, a, m);
        }
        try out.append(a, '}');
    }
    try out.append(a, '}');
}

fn writeLog(out: *std.ArrayList(u8), a: std.mem.Allocator, r: *const LogRecord) !void {
    try out.print(a, "{{\"timeUnixNano\":\"{d}\",\"severityNumber\":{d},\"severityText\":", .{
        r.time_unix_nano,
        @intFromEnum(r.severity),
    });
    try writeJsonString(out, a, r.severity.text());
    try out.appendSlice(a, ",\"body\":{\"stringValue\":");
    try writeJsonString(out, a, r.body);
    try out.append(a, '}');
    if (r.attrs.items.len > 0) {
        try out.appendSlice(a, ",\"attributes\":");
        try writeAttrs(out, a, r.attrs.items);
    }
    if (r.trace_id) |tid| {
        try out.appendSlice(a, ",\"traceId\":\"");
        try writeHex(out, a, &tid);
        try out.append(a, '"');
    }
    if (r.span_id) |sid| {
        try out.appendSlice(a, ",\"spanId\":\"");
        try writeHex(out, a, &sid);
        try out.append(a, '"');
    }
    try out.append(a, '}');
}

fn writeAttrs(out: *std.ArrayList(u8), a: std.mem.Allocator, attrs: []const StoredAttr) !void {
    try out.append(a, '[');
    for (attrs, 0..) |attr, i| {
        if (i > 0) try out.append(a, ',');
        try out.appendSlice(a, "{\"key\":");
        try writeJsonString(out, a, attr.key);
        try out.appendSlice(a, ",\"value\":");
        switch (attr.value) {
            .str => |s| {
                try out.appendSlice(a, "{\"stringValue\":");
                try writeJsonString(out, a, s);
                try out.append(a, '}');
            },
            .int => |int_v| try out.print(a, "{{\"intValue\":\"{d}\"}}", .{int_v}),
            .boolean => |b| try out.print(a, "{{\"boolValue\":{}}}", .{b}),
            .dbl => |d| try out.print(a, "{{\"doubleValue\":{d}}}", .{d}),
        }
        try out.append(a, '}');
    }
    try out.append(a, ']');
}

fn writeHex(out: *std.ArrayList(u8), a: std.mem.Allocator, bytes: []const u8) !void {
    const hex = "0123456789abcdef";
    var buf: [64]u8 = undefined;
    if (bytes.len * 2 > buf.len) return error.OutOfMemory;
    for (bytes, 0..) |b, i| {
        buf[i * 2] = hex[b >> 4];
        buf[i * 2 + 1] = hex[b & 0x0f];
    }
    try out.appendSlice(a, buf[0 .. bytes.len * 2]);
}

fn writeJsonString(out: *std.ArrayList(u8), a: std.mem.Allocator, s: []const u8) !void {
    const encoded = try std.json.Stringify.valueAlloc(a, s, .{});
    defer a.free(encoded);
    try out.appendSlice(a, encoded);
}

// ---- Tests ----

test "init disabled when endpoint empty" {
    var o = Otel.init(std.testing.allocator, std.testing.io, .{});
    defer o.deinit();
    try std.testing.expect(!o.enabled);
    var s = o.startSpan("noop", .internal, null);
    try std.testing.expect(s.record == null);
    s.end();
    o.emitLog(.info, "noop", &.{}, null);
    o.flush(); // should be a no-op
}

test "queue and discard on disabled-mode flush is harmless" {
    var o = Otel.init(std.testing.allocator, std.testing.io, .{});
    defer o.deinit();
    o.flush();
    o.flush();
}

test "queue capacity drops oldest" {
    var o = Otel.init(std.testing.allocator, std.testing.io, .{
        .endpoint = "http://nope.invalid",
        .max_queue_size = 2,
    });
    // We never call flush() so no HTTP attempted; queue trimming is what we
    // assert here.
    defer {
        // Drop everything without POSTing. Bypass deinit's flush by clearing
        // first.
        for (o.spans.items) |r| destroySpan(o.allocator, r);
        o.spans.clearRetainingCapacity();
        for (o.logs.items) |r| destroyLog(o.allocator, r);
        o.logs.clearRetainingCapacity();
        o.enabled = false; // skip deinit's flush
        o.deinit();
    }
    var s1 = o.startSpan("a", .internal, null);
    s1.end();
    var s2 = o.startSpan("b", .internal, null);
    s2.end();
    var s3 = o.startSpan("c", .internal, null);
    s3.end();
    try std.testing.expectEqual(@as(usize, 2), o.spans.items.len);
    try std.testing.expectEqualStrings("b", o.spans.items[0].name);
    try std.testing.expectEqualStrings("c", o.spans.items[1].name);
    try std.testing.expectEqual(@as(u64, 1), o.stats.spans_dropped);
}

test "writeTracesJson basic shape" {
    const a = std.testing.allocator;
    var rec = SpanRecord{
        .trace_id = [_]u8{0xab} ** 16,
        .span_id = [_]u8{0xcd} ** 8,
        .parent_span_id = null,
        .name = try a.dupe(u8, "rpc.request"),
        .kind = .server,
        .start_unix_nano = 1000,
        .end_unix_nano = 2000,
        .status_code = .ok,
        .status_message = null,
        .attrs = .empty,
    };
    defer {
        a.free(rec.name);
        rec.attrs.deinit(a);
    }
    try rec.attrs.append(a, .{
        .key = try a.dupe(u8, "rpc.method"),
        .value = .{ .str = try a.dupe(u8, "session.send") },
    });
    defer {
        a.free(rec.attrs.items[0].key);
        a.free(rec.attrs.items[0].value.str);
    }

    var out: std.ArrayList(u8) = .empty;
    defer out.deinit(a);
    const recs = [_]*SpanRecord{&rec};
    try writeTracesJson(&out, a, "phoenix", &recs);

    try std.testing.expect(std.mem.indexOf(u8, out.items, "\"resourceSpans\"") != null);
    try std.testing.expect(std.mem.indexOf(u8, out.items, "\"service.name\"") != null);
    try std.testing.expect(std.mem.indexOf(u8, out.items, "\"phoenix\"") != null);
    try std.testing.expect(std.mem.indexOf(u8, out.items, "abababababababababababababababab") != null);
    try std.testing.expect(std.mem.indexOf(u8, out.items, "cdcdcdcdcdcdcdcd") != null);
    try std.testing.expect(std.mem.indexOf(u8, out.items, "\"name\":\"rpc.request\"") != null);
    try std.testing.expect(std.mem.indexOf(u8, out.items, "\"startTimeUnixNano\":\"1000\"") != null);
    try std.testing.expect(std.mem.indexOf(u8, out.items, "\"endTimeUnixNano\":\"2000\"") != null);
    try std.testing.expect(std.mem.indexOf(u8, out.items, "\"code\":1") != null);
    try std.testing.expect(std.mem.indexOf(u8, out.items, "\"rpc.method\"") != null);
    try std.testing.expect(std.mem.indexOf(u8, out.items, "\"stringValue\":\"session.send\"") != null);
}

test "writeLogsJson basic shape" {
    const a = std.testing.allocator;
    var rec = LogRecord{
        .time_unix_nano = 5,
        .severity = .info,
        .body = try a.dupe(u8, "hello"),
        .attrs = .empty,
        .trace_id = [_]u8{0x11} ** 16,
        .span_id = [_]u8{0x22} ** 8,
    };
    defer {
        a.free(rec.body);
        rec.attrs.deinit(a);
    }

    var out: std.ArrayList(u8) = .empty;
    defer out.deinit(a);
    const recs = [_]*LogRecord{&rec};
    try writeLogsJson(&out, a, "phoenix", &recs);

    try std.testing.expect(std.mem.indexOf(u8, out.items, "\"resourceLogs\"") != null);
    try std.testing.expect(std.mem.indexOf(u8, out.items, "\"severityNumber\":9") != null);
    try std.testing.expect(std.mem.indexOf(u8, out.items, "\"severityText\":\"INFO\"") != null);
    try std.testing.expect(std.mem.indexOf(u8, out.items, "\"stringValue\":\"hello\"") != null);
    try std.testing.expect(std.mem.indexOf(u8, out.items, "11111111111111111111111111111111") != null);
    try std.testing.expect(std.mem.indexOf(u8, out.items, "2222222222222222") != null);
}

test "signalUrl base + override" {
    const a = std.testing.allocator;
    var o = Otel.init(a, std.testing.io, .{
        .endpoint = "http://localhost:4318/",
    });
    defer {
        // Skip the deinit flush since there is nothing queued and we don't
        // want a network call attempted in unit tests.
        o.enabled = false;
        o.deinit();
    }
    const traces = try o.signalUrl(.traces);
    defer a.free(traces);
    try std.testing.expectEqualStrings("http://localhost:4318/v1/traces", traces);

    const logs = try o.signalUrl(.logs);
    defer a.free(logs);
    try std.testing.expectEqualStrings("http://localhost:4318/v1/logs", logs);
}

test "signalUrl with explicit traces override" {
    const a = std.testing.allocator;
    var o = Otel.init(a, std.testing.io, .{
        .endpoint = "http://localhost:4318",
        .traces_endpoint = "http://collector.example.com/some/path",
    });
    defer {
        o.enabled = false;
        o.deinit();
    }
    const traces = try o.signalUrl(.traces);
    defer a.free(traces);
    try std.testing.expectEqualStrings("http://collector.example.com/some/path", traces);
}
