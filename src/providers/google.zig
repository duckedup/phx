/// Google Vertex AI and Gemini shared adapter.
const std = @import("std");
const builtin = @import("builtin");
const core = @import("phoenix_core");
const http_client = core.http_client;
const sse_mod = core.sse;
const json_util = core.json_util;

const Provider = core.Provider;
const ProviderConfig = core.ProviderConfig;
const SendOptions = core.SendOptions;
const Event = core.Event;
const EventIterator = core.provider.EventIterator;
const StopReason = core.StopReason;
const Message = core.Message;
const Transport = http_client.Transport;
const HttpTransport = http_client.HttpTransport;

const DEFAULT_VERTEX_LOCATION = "us-central1";
const VERTEX_HOST_SUFFIX = "-aiplatform.googleapis.com";

const Mode = enum { vertex, gemini };

// ---- Provider struct ----

const GoogleProvider = struct {
    base: Provider,
    allocator: std.mem.Allocator,
    cfg: ProviderConfig,
    mode: Mode,
    transport_owned: ?HttpTransport,
    transport: Transport,
    /// io handle for file system operations (e.g. reading credential files).
    io: std.Io,
    /// Optional injectable gcloud runner for testing vertex auth fallback.
    gcloud_runner: ?*const fn (allocator: std.mem.Allocator) anyerror![]u8,
    /// Cached access token (lazily resolved on first send).
    cached_token: ?[]u8,
};

pub fn createVertex(
    allocator: std.mem.Allocator,
    io: std.Io,
    cfg: ProviderConfig,
    injected: ?Transport,
) !*Provider {
    return makeProvider(allocator, io, cfg, injected, .vertex, null);
}

pub fn createGemini(
    allocator: std.mem.Allocator,
    io: std.Io,
    cfg: ProviderConfig,
    injected: ?Transport,
) !*Provider {
    return makeProvider(allocator, io, cfg, injected, .gemini, null);
}

fn makeProvider(
    allocator: std.mem.Allocator,
    io: std.Io,
    cfg: ProviderConfig,
    injected: ?Transport,
    mode: Mode,
    gcloud_runner: ?*const fn (allocator: std.mem.Allocator) anyerror![]u8,
) !*Provider {
    const self = try allocator.create(GoogleProvider);
    errdefer allocator.destroy(self);
    self.* = .{
        .base = .{
            .name = if (mode == .vertex) "vertex" else "gemini",
            .sendFn = sendImpl,
            .deinitFn = deinitImpl,
        },
        .allocator = allocator,
        .cfg = cfg,
        .mode = mode,
        .transport_owned = if (injected == null) HttpTransport.init(allocator, io) else null,
        .transport = undefined,
        .io = io,
        .gcloud_runner = gcloud_runner,
        .cached_token = null,
    };
    self.transport = if (injected) |t| t else self.transport_owned.?.transport();
    return &self.base;
}

fn deinitImpl(p: *Provider, allocator: std.mem.Allocator) void {
    const self: *GoogleProvider = @alignCast(@fieldParentPtr("base", p));
    if (self.cfg.resolved_credential_owned) {
        if (self.cfg.resolved_credential) |c| allocator.free(c);
    }
    if (self.cached_token) |t| allocator.free(t);
    if (self.transport_owned) |*t| t.deinit();
    allocator.destroy(self);
}

fn resolveVertexToken(self: *GoogleProvider, allocator: std.mem.Allocator) ![]const u8 {
    if (self.cached_token) |t| return t;

    if (self.cfg.resolved_credential) |cred| {
        const copy = try allocator.dupe(u8, cred);
        self.cached_token = copy;
        return copy;
    }

    if (self.cfg.credentials_path) |path| {
        const data = std.Io.Dir.readFileAlloc(std.Io.Dir.cwd(), self.io, path, allocator, .limited(1024 * 1024)) catch {
            return tryGcloud(self, allocator);
        };
        defer allocator.free(data);

        const parsed = std.json.parseFromSlice(std.json.Value, allocator, data, .{}) catch {
            return tryGcloud(self, allocator);
        };
        defer parsed.deinit();

        if (json_util.dottedLookup(parsed.value, "type")) |t| {
            if (t == .string and std.mem.eql(u8, t.string, "service_account")) {
                return error.NotImplemented;
            }
        }

        if (json_util.dottedLookup(parsed.value, "access_token")) |token_val| {
            if (token_val == .string) {
                const copy = try allocator.dupe(u8, token_val.string);
                self.cached_token = copy;
                return copy;
            }
        }

        return tryGcloud(self, allocator);
    }

    return tryGcloud(self, allocator);
}

fn tryGcloud(self: *GoogleProvider, allocator: std.mem.Allocator) ![]const u8 {
    const token = if (self.gcloud_runner) |runner|
        try runner(allocator)
    else
        try runGcloud(allocator, self.io);

    const trimmed = std.mem.trim(u8, token, &std.ascii.whitespace);
    if (trimmed.len == 0) {
        allocator.free(token);
        return error.MissingCredential;
    }
    const copy = try allocator.dupe(u8, trimmed);
    allocator.free(token);
    self.cached_token = copy;
    return copy;
}

fn runGcloud(allocator: std.mem.Allocator, io: std.Io) ![]u8 {
    const result = try std.process.run(allocator, io, .{
        .argv = &.{ "gcloud", "auth", "print-access-token" },
    });
    defer allocator.free(result.stderr);
    errdefer allocator.free(result.stdout);
    return result.stdout;
}

fn buildUrl(allocator: std.mem.Allocator, self: *GoogleProvider) ![]u8 {
    const model = self.cfg.model;

    switch (self.mode) {
        .vertex => {
            const project = self.cfg.project orelse return error.InvalidConfig;
            const location = self.cfg.location orelse DEFAULT_VERTEX_LOCATION;

            var host: std.ArrayList(u8) = .empty;
            defer host.deinit(allocator);
            if (self.cfg.base_url) |bu| {
                try host.appendSlice(allocator, bu);
            } else {
                try host.appendSlice(allocator, "https://");
                try host.appendSlice(allocator, location);
                try host.appendSlice(allocator, VERTEX_HOST_SUFFIX);
            }

            var url: std.ArrayList(u8) = .empty;
            defer url.deinit(allocator);
            try url.appendSlice(allocator, host.items);
            try url.appendSlice(allocator, "/v1/projects/");
            try url.appendSlice(allocator, project);
            try url.appendSlice(allocator, "/locations/");
            try url.appendSlice(allocator, location);
            try url.appendSlice(allocator, "/publishers/google/models/");
            try url.appendSlice(allocator, model);
            try url.appendSlice(allocator, ":streamGenerateContent?alt=sse");
            return url.toOwnedSlice(allocator);
        },
        .gemini => {
            const base_url = self.cfg.base_url orelse "https://generativelanguage.googleapis.com";
            var url: std.ArrayList(u8) = .empty;
            defer url.deinit(allocator);
            try url.appendSlice(allocator, base_url);
            try url.appendSlice(allocator, "/v1beta/models/");
            try url.appendSlice(allocator, model);
            try url.appendSlice(allocator, ":streamGenerateContent?alt=sse");
            return url.toOwnedSlice(allocator);
        },
    }
}

fn sendImpl(
    p: *Provider,
    allocator: std.mem.Allocator,
    options: SendOptions,
) anyerror!EventIterator {
    const self: *GoogleProvider = @alignCast(@fieldParentPtr("base", p));

    if (self.mode == .vertex and self.cfg.project == null) {
        return error.InvalidConfig;
    }

    const url = try buildUrl(allocator, self);
    defer allocator.free(url);

    const body = try buildGoogleBody(allocator, &options);
    defer allocator.free(body);

    var hdrs_buf: std.ArrayList(http_client.Header) = .empty;
    defer hdrs_buf.deinit(allocator);

    try hdrs_buf.append(allocator, .{ .name = "Content-Type", .value = "application/json" });
    try hdrs_buf.append(allocator, .{ .name = "Accept", .value = "text/event-stream" });

    // Build auth header value before calling post (must outlive hdrs_buf use).
    var auth_str: ?[]u8 = null;
    defer if (auth_str) |s| allocator.free(s);

    switch (self.mode) {
        .vertex => {
            const token = try resolveVertexToken(self, allocator);
            var auth_val: std.ArrayList(u8) = .empty;
            defer auth_val.deinit(allocator);
            try auth_val.appendSlice(allocator, "Bearer ");
            try auth_val.appendSlice(allocator, token);
            auth_str = try auth_val.toOwnedSlice(allocator);
            try hdrs_buf.append(allocator, .{ .name = "Authorization", .value = auth_str.? });
        },
        .gemini => {
            const api_key = self.cfg.resolved_credential orelse return error.MissingCredential;
            try hdrs_buf.append(allocator, .{ .name = "x-goog-api-key", .value = api_key });
        },
    }

    const resp = try self.transport.post(allocator, .{
        .url = url,
        .body = body,
        .headers = hdrs_buf.items,
        .timeout_ms = options.config.request_timeout_ms,
    });

    if (resp.status >= 400) {
        var r = resp;
        r.deinit();
        return error.BadStatus;
    }

    const iter = try allocator.create(GoogleIterator);
    errdefer allocator.destroy(iter);
    iter.* = GoogleIterator.initIter(allocator, resp, iter);
    return iter.base;
}

fn buildGoogleBody(allocator: std.mem.Allocator, opts: *const SendOptions) ![]u8 {
    var buf: std.ArrayList(u8) = .empty;
    defer buf.deinit(allocator);

    try buf.append(allocator, '{');

    var sys_buf: std.ArrayList(u8) = .empty;
    defer sys_buf.deinit(allocator);
    if (opts.config.system_prompt) |sp| {
        try sys_buf.appendSlice(allocator, sp);
    } else {
        for (opts.messages) |msg| {
            if (msg.role == .system) {
                if (sys_buf.items.len > 0) try sys_buf.append(allocator, '\n');
                try sys_buf.appendSlice(allocator, msg.content);
            }
        }
    }
    if (sys_buf.items.len > 0) {
        try buf.appendSlice(allocator, "\"systemInstruction\":{\"parts\":[{\"text\":");
        try json_util.appendString(&buf, allocator, sys_buf.items);
        try buf.appendSlice(allocator, "}]},");
    }

    if (opts.tools.len > 0) {
        try buf.appendSlice(allocator, "\"tools\":[{\"functionDeclarations\":[");
        for (opts.tools, 0..) |t, i| {
            if (i > 0) try buf.append(allocator, ',');
            try buf.appendSlice(allocator, "{\"name\":");
            try json_util.appendString(&buf, allocator, t.name);
            try buf.appendSlice(allocator, ",\"description\":");
            try json_util.appendString(&buf, allocator, t.description);
            try buf.appendSlice(allocator, ",\"parameters\":");
            try buf.appendSlice(allocator, t.schema);
            try buf.append(allocator, '}');
        }
        try buf.appendSlice(allocator, "]}],");
    }

    try buf.appendSlice(allocator, "\"contents\":[");
    try appendGoogleContents(&buf, allocator, opts.messages);
    try buf.append(allocator, ']');
    try buf.append(allocator, '}');

    return buf.toOwnedSlice(allocator);
}

fn appendGoogleContents(
    buf: *std.ArrayList(u8),
    allocator: std.mem.Allocator,
    messages: []const Message,
) !void {
    var first = true;
    var i: usize = 0;
    while (i < messages.len) {
        const msg = messages[i];

        if (msg.role == .system) {
            i += 1;
            continue;
        }

        if (!first) try buf.append(allocator, ',');
        first = false;

        switch (msg.role) {
            .system => unreachable,
            .user => {
                try buf.appendSlice(allocator, "{\"role\":\"user\",\"parts\":[{\"text\":");
                try json_util.appendString(buf, allocator, msg.content);
                try buf.appendSlice(allocator, "}]}");
                i += 1;
            },
            .assistant => {
                try buf.appendSlice(allocator, "{\"role\":\"model\",\"parts\":[");
                var parts_first = true;

                if (msg.content.len > 0) {
                    try buf.appendSlice(allocator, "{\"text\":");
                    try json_util.appendString(buf, allocator, msg.content);
                    try buf.append(allocator, '}');
                    parts_first = false;
                }

                i += 1;
                while (i < messages.len and messages[i].role == .tool_call) {
                    const tc_msg = messages[i];
                    if (tc_msg.tool_call) |tc| {
                        if (!parts_first) try buf.append(allocator, ',');
                        parts_first = false;
                        try buf.appendSlice(allocator, "{\"functionCall\":{\"name\":");
                        try json_util.appendString(buf, allocator, tc.name);
                        try buf.appendSlice(allocator, ",\"args\":");
                        if (tc.args_json.len > 0) {
                            try buf.appendSlice(allocator, tc.args_json);
                        } else {
                            try buf.appendSlice(allocator, "{}");
                        }
                        try buf.appendSlice(allocator, "}}");
                    }
                    i += 1;
                }

                if (parts_first) {
                    try buf.appendSlice(allocator, "{\"text\":\"\"}");
                }

                try buf.appendSlice(allocator, "]}");
            },
            .tool_call => {
                if (msg.tool_call) |tc| {
                    try buf.appendSlice(allocator, "{\"role\":\"model\",\"parts\":[{\"functionCall\":{\"name\":");
                    try json_util.appendString(buf, allocator, tc.name);
                    try buf.appendSlice(allocator, ",\"args\":");
                    if (tc.args_json.len > 0) {
                        try buf.appendSlice(allocator, tc.args_json);
                    } else {
                        try buf.appendSlice(allocator, "{}");
                    }
                    try buf.appendSlice(allocator, "}}]}");
                }
                i += 1;
            },
            .tool_result => {
                if (msg.tool_result) |tr| {
                    var fn_name = tr.id;
                    if (i > 0) {
                        var j = i;
                        while (j > 0) {
                            j -= 1;
                            if (messages[j].role == .tool_call) {
                                if (messages[j].tool_call) |tc| {
                                    if (std.mem.eql(u8, tc.id, tr.id)) {
                                        fn_name = tc.name;
                                    }
                                }
                                break;
                            }
                        }
                    }
                    try buf.appendSlice(allocator, "{\"role\":\"user\",\"parts\":[{\"functionResponse\":{\"name\":");
                    try json_util.appendString(buf, allocator, fn_name);
                    try buf.appendSlice(allocator, ",\"response\":{\"result\":");
                    try json_util.appendString(buf, allocator, tr.output);
                    try buf.appendSlice(allocator, "}}}]}");
                }
                i += 1;
            },
        }
    }
}

// ---- Iterator ----

const GoogleIterator = struct {
    base: EventIterator,
    allocator: std.mem.Allocator,
    response: http_client.Response,
    sse_parser: sse_mod.Parser,
    ended: bool,
    input_tokens: u32,
    output_tokens: u32,
    fc_seq: u32,
    // When a chunk has both content AND finishReason, we emit the content first
    // and store a pending done event to emit next call.
    pending_done: bool,
    pending_done_ev: core.DoneEvent,
    // Scratch storage
    scratch: ?[]u8,
    scratch2: ?[]u8,
    scratch3: ?[]u8,

    fn initIter(allocator: std.mem.Allocator, resp: http_client.Response, self_ptr: *GoogleIterator) GoogleIterator {
        return .{
            .base = .{ .nextFn = nextImpl, .deinitFn = deinitIter, .ctx = self_ptr },
            .allocator = allocator,
            .response = resp,
            .sse_parser = sse_mod.Parser.init(allocator, resp.body_reader),
            .ended = false,
            .input_tokens = 0,
            .output_tokens = 0,
            .fc_seq = 0,
            .pending_done = false,
            .pending_done_ev = .{},
            .scratch = null,
            .scratch2 = null,
            .scratch3 = null,
        };
    }

    fn freeScratch(self: *GoogleIterator) void {
        if (self.scratch) |s| {
            self.allocator.free(s);
            self.scratch = null;
        }
        if (self.scratch2) |s| {
            self.allocator.free(s);
            self.scratch2 = null;
        }
        if (self.scratch3) |s| {
            self.allocator.free(s);
            self.scratch3 = null;
        }
    }
};

fn deinitIter(it: *EventIterator) void {
    const self: *GoogleIterator = @ptrCast(@alignCast(it.ctx));
    self.freeScratch();
    self.sse_parser.deinit();
    self.response.deinit();
    self.allocator.destroy(self);
}

fn nextImpl(it: *EventIterator) ?Event {
    const self: *GoogleIterator = @ptrCast(@alignCast(it.ctx));
    if (self.ended) return null;
    self.freeScratch();

    // Emit pending done from previous chunk
    if (self.pending_done) {
        self.pending_done = false;
        self.ended = true;
        return Event{ .done = self.pending_done_ev };
    }

    while (true) {
        const maybe_ev = self.sse_parser.nextEvent() catch return null;
        const sse_ev = maybe_ev orelse {
            self.ended = true;
            return null;
        };

        const data = sse_ev.data;

        const parsed = std.json.parseFromSlice(
            std.json.Value,
            self.allocator,
            data,
            .{},
        ) catch continue;
        defer parsed.deinit();

        if (json_util.dottedLookup(parsed.value, "usageMetadata.promptTokenCount")) |v| {
            if (v == .integer) self.input_tokens = @intCast(v.integer);
        }
        if (json_util.dottedLookup(parsed.value, "usageMetadata.candidatesTokenCount")) |v| {
            if (v == .integer) self.output_tokens = @intCast(v.integer);
        }

        const candidates = json_util.dottedLookup(parsed.value, "candidates") orelse continue;
        if (candidates != .array or candidates.array.items.len == 0) continue;
        const candidate = candidates.array.items[0];

        var finish_reason: ?StopReason = null;
        if (json_util.dottedLookup(candidate, "finishReason")) |fr| {
            if (fr == .string) {
                finish_reason = mapFinishReason(fr.string);
            }
        }

        const parts = json_util.dottedLookup(candidate, "content.parts") orelse {
            if (finish_reason) |sr| {
                self.ended = true;
                return Event{ .done = .{
                    .stop_reason = sr,
                    .usage = .{ .input_tokens = self.input_tokens, .output_tokens = self.output_tokens },
                } };
            }
            continue;
        };

        if (parts != .array) continue;

        // Process parts: emit first content event, buffer done for next call
        for (parts.array.items) |part| {
            if (part != .object) continue;

            if (part.object.get("text")) |text_val| {
                if (text_val == .string and text_val.string.len > 0) {
                    const copy = self.allocator.dupe(u8, text_val.string) catch continue;
                    self.scratch = copy;

                    if (finish_reason) |sr| {
                        self.pending_done = true;
                        self.pending_done_ev = .{
                            .stop_reason = sr,
                            .usage = .{ .input_tokens = self.input_tokens, .output_tokens = self.output_tokens },
                        };
                    }

                    return Event{ .token = copy };
                }
            }

            if (part.object.get("functionCall")) |fc_val| {
                if (fc_val == .object) {
                    const name_val = fc_val.object.get("name") orelse continue;
                    if (name_val != .string) continue;

                    var id_buf: [32]u8 = undefined;
                    const id_str = std.fmt.bufPrint(&id_buf, "gemini-fc-{d}", .{self.fc_seq}) catch continue;
                    self.fc_seq += 1;

                    const id_copy = self.allocator.dupe(u8, id_str) catch continue;
                    const name_copy = self.allocator.dupe(u8, name_val.string) catch {
                        self.allocator.free(id_copy);
                        continue;
                    };

                    const args_val = fc_val.object.get("args") orelse std.json.Value{ .object = .{} };
                    const args_json = std.json.Stringify.valueAlloc(self.allocator, args_val, .{}) catch {
                        self.allocator.free(id_copy);
                        self.allocator.free(name_copy);
                        continue;
                    };

                    self.scratch = id_copy;
                    self.scratch2 = name_copy;
                    self.scratch3 = args_json;

                    if (finish_reason) |sr| {
                        self.pending_done = true;
                        self.pending_done_ev = .{
                            .stop_reason = sr,
                            .usage = .{ .input_tokens = self.input_tokens, .output_tokens = self.output_tokens },
                        };
                    }

                    return Event{ .tool_call = .{
                        .id = id_copy,
                        .name = name_copy,
                        .args_json = args_json,
                    } };
                }
            }
        }

        // No parts emitted — check finish_reason
        if (finish_reason) |sr| {
            self.ended = true;
            return Event{ .done = .{
                .stop_reason = sr,
                .usage = .{ .input_tokens = self.input_tokens, .output_tokens = self.output_tokens },
            } };
        }
    }
}

fn mapFinishReason(s: []const u8) StopReason {
    if (std.mem.eql(u8, s, "STOP")) return .end_turn;
    if (std.mem.eql(u8, s, "MAX_TOKENS")) return .max_tokens;
    return .other;
}

// ---- Tests ----

const test_util = @import("test_util");
const MockTransport = http_client.MockTransport;

test "vertex: missing project errors" {
    const allocator = std.testing.allocator;
    const canned = [_]http_client.Canned{};
    var mock = MockTransport.init(allocator, &canned);
    defer mock.deinit();

    var p = try createVertex(allocator, std.testing.io, .{
        .model = "gemini-1.5-pro",
        .resolved_credential = "tok",
    }, mock.transport());
    defer p.deinit(allocator);

    const msgs = [_]Message{.{ .role = .user, .content = "hi" }};
    const result = p.send(allocator, .{
        .messages = &msgs,
        .config = .{ .model = "gemini-1.5-pro", .resolved_credential = "tok" },
    });
    try std.testing.expectError(error.InvalidConfig, result);
}

test "vertex: builds streamGenerateContent URL with project + location" {
    const allocator = std.testing.allocator;
    const fixture =
        "data: {\"candidates\":[{\"content\":{\"role\":\"model\",\"parts\":[{\"text\":\"ok\"}]},\"finishReason\":\"STOP\",\"index\":0}],\"usageMetadata\":{\"promptTokenCount\":0,\"candidatesTokenCount\":1}}\n\n";
    const canned = [_]http_client.Canned{.{ .body = fixture }};
    var mock = MockTransport.init(allocator, &canned);
    defer mock.deinit();

    var p = try createVertex(allocator, std.testing.io, .{
        .model = "gemini-1.5-pro",
        .project = "my-proj",
        .location = "us-east4",
        .resolved_credential = "tok",
    }, mock.transport());
    defer p.deinit(allocator);

    const msgs = [_]Message{.{ .role = .user, .content = "hi" }};
    var it = try p.send(allocator, .{
        .messages = &msgs,
        .config = .{ .model = "gemini-1.5-pro", .project = "my-proj", .location = "us-east4", .resolved_credential = "tok" },
    });
    defer it.deinit();
    while (it.next()) |_| {}

    const captured = mock.captured.items[0];
    try std.testing.expectEqualStrings(
        "https://us-east4-aiplatform.googleapis.com/v1/projects/my-proj/locations/us-east4/publishers/google/models/gemini-1.5-pro:streamGenerateContent?alt=sse",
        captured.url,
    );
    const auth = test_util.headerValue(captured.headers, "Authorization").?;
    try std.testing.expectEqualStrings("Bearer tok", auth);
}

test "gemini: builds public-API URL with x-goog-api-key" {
    const allocator = std.testing.allocator;
    const fixture =
        "data: {\"candidates\":[{\"content\":{\"role\":\"model\",\"parts\":[{\"text\":\"ok\"}]},\"finishReason\":\"STOP\",\"index\":0}],\"usageMetadata\":{\"promptTokenCount\":0,\"candidatesTokenCount\":1}}\n\n";
    const canned = [_]http_client.Canned{.{ .body = fixture }};
    var mock = MockTransport.init(allocator, &canned);
    defer mock.deinit();

    var p = try createGemini(allocator, std.testing.io, .{
        .model = "gemini-1.5-pro",
        .resolved_credential = "AIzatest",
    }, mock.transport());
    defer p.deinit(allocator);

    const msgs = [_]Message{.{ .role = .user, .content = "hi" }};
    var it = try p.send(allocator, .{
        .messages = &msgs,
        .config = .{ .model = "gemini-1.5-pro", .resolved_credential = "AIzatest" },
    });
    defer it.deinit();
    while (it.next()) |_| {}

    const captured = mock.captured.items[0];
    try std.testing.expectEqualStrings(
        "https://generativelanguage.googleapis.com/v1beta/models/gemini-1.5-pro:streamGenerateContent?alt=sse",
        captured.url,
    );
    try std.testing.expectEqualStrings("AIzatest", test_util.headerValue(captured.headers, "x-goog-api-key").?);
    try std.testing.expect(test_util.headerValue(captured.headers, "Authorization") == null);
}

test "google: text streaming yields tokens + done" {
    const allocator = std.testing.allocator;
    const fixture =
        "data: {\"candidates\":[{\"content\":{\"role\":\"model\",\"parts\":[{\"text\":\"Hi \"}]},\"index\":0}]}\n\n" ++
        "data: {\"candidates\":[{\"content\":{\"role\":\"model\",\"parts\":[{\"text\":\"there\"}]},\"finishReason\":\"STOP\",\"index\":0}],\"usageMetadata\":{\"promptTokenCount\":4,\"candidatesTokenCount\":2}}\n\n";

    const canned = [_]http_client.Canned{.{ .body = fixture }};
    var mock = MockTransport.init(allocator, &canned);
    defer mock.deinit();

    var p = try createGemini(allocator, std.testing.io, .{
        .model = "gemini-1.5-flash",
        .resolved_credential = "AIza",
    }, mock.transport());
    defer p.deinit(allocator);

    const msgs = [_]Message{.{ .role = .user, .content = "hi" }};
    var it = try p.send(allocator, .{
        .messages = &msgs,
        .config = .{ .model = "gemini-1.5-flash", .resolved_credential = "AIza" },
    });
    defer it.deinit();

    const events = try test_util.collectEvents(&it, allocator);
    defer test_util.freeEvents(allocator, events);

    var token_count: usize = 0;
    var done_ev: ?core.DoneEvent = null;
    for (events) |ev| switch (ev) {
        .token => token_count += 1,
        .done => |d| done_ev = d,
        else => {},
    };

    try std.testing.expect(token_count >= 2);
    try std.testing.expect(done_ev != null);
    if (done_ev) |d| {
        try std.testing.expectEqual(StopReason.end_turn, d.stop_reason);
        try std.testing.expectEqual(@as(u32, 4), d.usage.input_tokens);
        try std.testing.expectEqual(@as(u32, 2), d.usage.output_tokens);
    }
}

test "google: functionCall yields tool_call event" {
    const allocator = std.testing.allocator;
    const fixture =
        "data: {\"candidates\":[{\"content\":{\"role\":\"model\",\"parts\":[{\"functionCall\":{\"name\":\"read_file\",\"args\":{\"path\":\"a.txt\"}}}]},\"finishReason\":\"STOP\"}]}\n\n";

    const canned = [_]http_client.Canned{.{ .body = fixture }};
    var mock = MockTransport.init(allocator, &canned);
    defer mock.deinit();

    var p = try createGemini(allocator, std.testing.io, .{
        .model = "gemini-1.5-flash",
        .resolved_credential = "AIza",
    }, mock.transport());
    defer p.deinit(allocator);

    const msgs = [_]Message{.{ .role = .user, .content = "use tool" }};
    var it = try p.send(allocator, .{
        .messages = &msgs,
        .config = .{ .model = "gemini-1.5-flash", .resolved_credential = "AIza" },
    });
    defer it.deinit();

    const events = try test_util.collectEvents(&it, allocator);
    defer test_util.freeEvents(allocator, events);

    var tc_found = false;
    for (events) |ev| switch (ev) {
        .tool_call => |tc| {
            try std.testing.expectEqualStrings("read_file", tc.name);
            try std.testing.expect(tc.id.len > 0);
            try std.testing.expect(std.mem.indexOf(u8, tc.args_json, "a.txt") != null);
            tc_found = true;
        },
        else => {},
    };
    try std.testing.expect(tc_found);
}

test "google: maps system to systemInstruction" {
    const allocator = std.testing.allocator;
    const fixture =
        "data: {\"candidates\":[{\"content\":{\"role\":\"model\",\"parts\":[{\"text\":\"ok\"}]},\"finishReason\":\"STOP\",\"index\":0}]}\n\n";

    const canned = [_]http_client.Canned{.{ .body = fixture }};
    var mock = MockTransport.init(allocator, &canned);
    defer mock.deinit();

    var p = try createGemini(allocator, std.testing.io, .{
        .model = "gemini-1.5-flash",
        .resolved_credential = "AIza",
    }, mock.transport());
    defer p.deinit(allocator);

    const msgs = [_]Message{
        .{ .role = .system, .content = "You are a helpful assistant." },
        .{ .role = .user, .content = "hi" },
    };

    var it = try p.send(allocator, .{
        .messages = &msgs,
        .config = .{ .model = "gemini-1.5-flash", .resolved_credential = "AIza" },
    });
    defer it.deinit();
    while (it.next()) |_| {}

    const body = mock.captured.items[0].body;
    try std.testing.expect(std.mem.indexOf(u8, body, "systemInstruction") != null);
    try std.testing.expect(std.mem.indexOf(u8, body, "You are a helpful assistant.") != null);
    try std.testing.expect(std.mem.indexOf(u8, body, "\"system\"") == null);
}

test "google: vertex auth fallback is exercised by resolution helper" {
    const allocator = std.testing.allocator;
    const fixture =
        "data: {\"candidates\":[{\"content\":{\"role\":\"model\",\"parts\":[{\"text\":\"ok\"}]},\"finishReason\":\"STOP\"}]}\n\n";
    const canned = [_]http_client.Canned{.{ .body = fixture }};
    var mock = MockTransport.init(allocator, &canned);
    defer mock.deinit();

    const gp = try allocator.create(GoogleProvider);
    errdefer allocator.destroy(gp);
    gp.* = .{
        .base = .{ .name = "vertex", .sendFn = sendImpl, .deinitFn = deinitImpl },
        .allocator = allocator,
        .cfg = .{
            .model = "gemini-1.5-pro",
            .project = "test-proj",
            .location = "us-central1",
            .resolved_credential = null,
        },
        .mode = .vertex,
        .transport_owned = null,
        .transport = mock.transport(),
        .io = std.testing.io,
        .gcloud_runner = fakeGcloudRunner,
        .cached_token = null,
    };
    var p = &gp.base;
    defer p.deinit(allocator);

    const msgs = [_]Message{.{ .role = .user, .content = "hi" }};
    var it = try p.send(allocator, .{
        .messages = &msgs,
        .config = .{ .model = "gemini-1.5-pro", .project = "test-proj", .location = "us-central1" },
    });
    defer it.deinit();
    while (it.next()) |_| {}

    const auth = test_util.headerValue(mock.captured.items[0].headers, "Authorization").?;
    try std.testing.expectEqualStrings("Bearer fake-gcloud-token", auth);
}

fn fakeGcloudRunner(allocator: std.mem.Allocator) anyerror![]u8 {
    return allocator.dupe(u8, "fake-gcloud-token\n");
}

test "google: e2e roundtrip gemini" {
    if (std.c.getenv("PHOENIX_E2E") == null) return error.SkipZigTest;
    const key = std.mem.span(std.c.getenv("GEMINI_API_KEY") orelse return error.SkipZigTest);

    var p = try createGemini(std.testing.allocator, std.testing.io, .{
        .model = "gemini-1.5-flash",
        .resolved_credential = key,
    }, null);
    defer p.deinit(std.testing.allocator);

    const msgs = [_]Message{.{ .role = .user, .content = "Reply with the word OK and nothing else." }};
    var it = try p.send(std.testing.allocator, .{
        .messages = &msgs,
        .config = .{ .model = "gemini-1.5-flash", .resolved_credential = key },
    });
    defer it.deinit();

    var buf: std.ArrayList(u8) = .empty;
    defer buf.deinit(std.testing.allocator);
    while (it.next()) |ev| switch (ev) {
        .token => |t| try buf.appendSlice(std.testing.allocator, t),
        .done => break,
        .err => |e| std.debug.panic("provider error: {s}", .{e}),
        else => {},
    };
    try std.testing.expect(std.mem.indexOf(u8, buf.items, "OK") != null);
}
