/// Anthropic Messages API adapter with SSE streaming and tool calling.
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

const DEFAULT_BASE_URL = "https://api.anthropic.com";
const ANTHROPIC_VERSION = "2023-06-01";

// ---- Provider struct ----

const ClaudeProvider = struct {
    base: Provider,
    allocator: std.mem.Allocator,
    cfg: ProviderConfig,
    transport_owned: ?HttpTransport,
    transport: Transport,
};

/// Get an HttpTransport backed by the standard I/O system.
/// In test builds uses std.testing.io; in production builds requires that
/// callers inject a Transport (null transport in production is unsupported).
fn getHttpTransport(allocator: std.mem.Allocator) HttpTransport {
    if (builtin.is_test) {
        return HttpTransport.init(allocator, std.testing.io);
    } else {
        @panic("HttpTransport requires std.Io; inject a Transport or use createWithIo");
    }
}

pub fn create(
    allocator: std.mem.Allocator,
    cfg: ProviderConfig,
    injected: ?Transport,
) !*Provider {
    const self = try allocator.create(ClaudeProvider);
    errdefer allocator.destroy(self);
    self.* = .{
        .base = .{ .name = "claude", .sendFn = sendImpl, .deinitFn = deinitImpl },
        .allocator = allocator,
        .cfg = cfg,
        .transport_owned = if (injected == null) getHttpTransport(allocator) else null,
        .transport = undefined,
    };
    self.transport = if (injected) |t| t else self.transport_owned.?.transport();
    return &self.base;
}

fn deinitImpl(p: *Provider, allocator: std.mem.Allocator) void {
    const self: *ClaudeProvider = @alignCast(@fieldParentPtr("base", p));
    if (self.transport_owned) |*t| t.deinit();
    allocator.destroy(self);
}

fn sendImpl(
    p: *Provider,
    allocator: std.mem.Allocator,
    options: SendOptions,
) anyerror!EventIterator {
    const self: *ClaudeProvider = @alignCast(@fieldParentPtr("base", p));

    const credential = options.config.resolved_credential orelse
        self.cfg.resolved_credential orelse
        return error.MissingCredential;

    const base_url = options.config.base_url orelse
        self.cfg.base_url orelse
        DEFAULT_BASE_URL;

    var url_buf: std.ArrayList(u8) = .empty;
    defer url_buf.deinit(allocator);
    try url_buf.appendSlice(allocator, base_url);
    try url_buf.appendSlice(allocator, "/v1/messages");
    const url = try url_buf.toOwnedSlice(allocator);
    defer allocator.free(url);

    const body = try buildRequestBody(allocator, &options);
    defer allocator.free(body);

    const headers = [_]http_client.Header{
        .{ .name = "x-api-key", .value = credential },
        .{ .name = "anthropic-version", .value = ANTHROPIC_VERSION },
        .{ .name = "Content-Type", .value = "application/json" },
        .{ .name = "Accept", .value = "text/event-stream" },
    };

    const resp = try self.transport.post(allocator, .{
        .url = url,
        .body = body,
        .headers = &headers,
        .timeout_ms = options.config.request_timeout_ms,
    });

    if (resp.status >= 400) {
        var r = resp;
        r.deinit();
        return error.BadStatus;
    }

    const iter = try allocator.create(ClaudeIterator);
    errdefer allocator.destroy(iter);
    iter.* = ClaudeIterator.initIter(allocator, resp, iter);
    return iter.base;
}

/// Build the Anthropic Messages API request body JSON.
fn buildRequestBody(allocator: std.mem.Allocator, opts: *const SendOptions) ![]u8 {
    var buf: std.ArrayList(u8) = .empty;
    defer buf.deinit(allocator);

    // Collect system messages into a string
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

    try buf.appendSlice(allocator, "{");
    try buf.appendSlice(allocator, "\"model\":");
    try json_util.appendString(&buf, allocator, opts.config.model);
    try buf.appendSlice(allocator, ",\"max_tokens\":4096");
    try buf.appendSlice(allocator, ",\"stream\":true");

    if (sys_buf.items.len > 0) {
        try buf.appendSlice(allocator, ",\"system\":");
        try json_util.appendString(&buf, allocator, sys_buf.items);
    }

    if (opts.tools.len > 0) {
        try buf.appendSlice(allocator, ",\"tools\":[");
        for (opts.tools, 0..) |t, i| {
            if (i > 0) try buf.append(allocator, ',');
            try buf.appendSlice(allocator, "{\"name\":");
            try json_util.appendString(&buf, allocator, t.name);
            try buf.appendSlice(allocator, ",\"description\":");
            try json_util.appendString(&buf, allocator, t.description);
            try buf.appendSlice(allocator, ",\"input_schema\":");
            try buf.appendSlice(allocator, t.schema);
            try buf.append(allocator, '}');
        }
        try buf.append(allocator, ']');
    }

    try buf.appendSlice(allocator, ",\"messages\":[");
    try appendClaudeMessages(&buf, allocator, opts.messages);
    try buf.append(allocator, ']');
    try buf.append(allocator, '}');

    return buf.toOwnedSlice(allocator);
}

fn appendClaudeMessages(
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
                try buf.appendSlice(allocator, "{\"role\":\"user\",\"content\":[{\"type\":\"text\",\"text\":");
                try json_util.appendString(buf, allocator, msg.content);
                try buf.appendSlice(allocator, "}]}");
                i += 1;
            },
            .assistant => {
                try buf.appendSlice(allocator, "{\"role\":\"assistant\",\"content\":[");
                var content_first = true;

                if (msg.content.len > 0) {
                    try buf.appendSlice(allocator, "{\"type\":\"text\",\"text\":");
                    try json_util.appendString(buf, allocator, msg.content);
                    try buf.append(allocator, '}');
                    content_first = false;
                }

                i += 1;
                while (i < messages.len and messages[i].role == .tool_call) {
                    const tc_msg = messages[i];
                    if (tc_msg.tool_call) |tc| {
                        if (!content_first) try buf.append(allocator, ',');
                        content_first = false;
                        try buf.appendSlice(allocator, "{\"type\":\"tool_use\",\"id\":");
                        try json_util.appendString(buf, allocator, tc.id);
                        try buf.appendSlice(allocator, ",\"name\":");
                        try json_util.appendString(buf, allocator, tc.name);
                        try buf.appendSlice(allocator, ",\"input\":");
                        if (tc.args_json.len > 0) {
                            try buf.appendSlice(allocator, tc.args_json);
                        } else {
                            try buf.appendSlice(allocator, "{}");
                        }
                        try buf.append(allocator, '}');
                    }
                    i += 1;
                }

                if (content_first) {
                    try buf.appendSlice(allocator, "{\"type\":\"text\",\"text\":\"\"}");
                }

                try buf.appendSlice(allocator, "]}");
            },
            .tool_call => {
                if (msg.tool_call) |tc| {
                    try buf.appendSlice(allocator, "{\"role\":\"assistant\",\"content\":[{\"type\":\"tool_use\",\"id\":");
                    try json_util.appendString(buf, allocator, tc.id);
                    try buf.appendSlice(allocator, ",\"name\":");
                    try json_util.appendString(buf, allocator, tc.name);
                    try buf.appendSlice(allocator, ",\"input\":");
                    if (tc.args_json.len > 0) {
                        try buf.appendSlice(allocator, tc.args_json);
                    } else {
                        try buf.appendSlice(allocator, "{}");
                    }
                    try buf.appendSlice(allocator, "}]}");
                }
                i += 1;
            },
            .tool_result => {
                if (msg.tool_result) |tr| {
                    try buf.appendSlice(allocator, "{\"role\":\"user\",\"content\":[{\"type\":\"tool_result\",\"tool_use_id\":");
                    try json_util.appendString(buf, allocator, tr.id);
                    try buf.appendSlice(allocator, ",\"content\":");
                    try json_util.appendString(buf, allocator, tr.output);
                    try buf.appendSlice(allocator, ",\"is_error\":");
                    try buf.appendSlice(allocator, if (tr.is_error) "true" else "false");
                    try buf.appendSlice(allocator, "}]}");
                }
                i += 1;
            },
        }
    }
}

// ---- Iterator ----

const ToolUseBlock = struct {
    id: std.ArrayList(u8),
    name: std.ArrayList(u8),
    args: std.ArrayList(u8),
};

const ClaudeIterator = struct {
    base: EventIterator,
    allocator: std.mem.Allocator,
    response: http_client.Response,
    sse_parser: sse_mod.Parser,
    tool_blocks: std.AutoHashMap(u64, ToolUseBlock),
    stop_reason: StopReason,
    input_tokens: u32,
    output_tokens: u32,
    ended: bool,
    // Scratch storage: slices valid until next next() call
    scratch: ?[]u8,
    scratch_tool_id: ?[]u8,
    scratch_tool_name: ?[]u8,
    scratch_tool_args: ?[]u8,

    fn initIter(allocator: std.mem.Allocator, resp: http_client.Response, self_ptr: *ClaudeIterator) ClaudeIterator {
        return .{
            .base = .{ .nextFn = nextImpl, .deinitFn = deinitIter, .ctx = self_ptr },
            .allocator = allocator,
            .response = resp,
            .sse_parser = sse_mod.Parser.init(allocator, resp.body_reader),
            .tool_blocks = std.AutoHashMap(u64, ToolUseBlock).init(allocator),
            .stop_reason = .other,
            .input_tokens = 0,
            .output_tokens = 0,
            .ended = false,
            .scratch = null,
            .scratch_tool_id = null,
            .scratch_tool_name = null,
            .scratch_tool_args = null,
        };
    }

    fn freeScratch(self: *ClaudeIterator) void {
        if (self.scratch) |s| {
            self.allocator.free(s);
            self.scratch = null;
        }
        if (self.scratch_tool_id) |s| {
            self.allocator.free(s);
            self.scratch_tool_id = null;
        }
        if (self.scratch_tool_name) |s| {
            self.allocator.free(s);
            self.scratch_tool_name = null;
        }
        if (self.scratch_tool_args) |s| {
            self.allocator.free(s);
            self.scratch_tool_args = null;
        }
    }
};

fn deinitIter(it: *EventIterator) void {
    const self: *ClaudeIterator = @ptrCast(@alignCast(it.ctx));
    self.freeScratch();
    var iter = self.tool_blocks.valueIterator();
    while (iter.next()) |block| {
        block.id.deinit(self.allocator);
        block.name.deinit(self.allocator);
        block.args.deinit(self.allocator);
    }
    self.tool_blocks.deinit();
    self.sse_parser.deinit();
    self.response.deinit();
    self.allocator.destroy(self);
}

fn nextImpl(it: *EventIterator) ?Event {
    const self: *ClaudeIterator = @ptrCast(@alignCast(it.ctx));
    if (self.ended) return null;
    self.freeScratch();

    while (true) {
        const maybe_ev = self.sse_parser.nextEvent() catch return null;
        const ev = maybe_ev orelse {
            self.ended = true;
            return null;
        };

        const event_type = ev.event;
        const data = ev.data;

        if (std.mem.eql(u8, event_type, "error")) {
            self.ended = true;
            const parsed = std.json.parseFromSlice(
                std.json.Value,
                self.allocator,
                data,
                .{},
            ) catch {
                return Event{ .err = "unknown error" };
            };
            defer parsed.deinit();
            if (json_util.dottedLookup(parsed.value, "error.message")) |msg_val| {
                if (msg_val == .string) {
                    const msg_copy = self.allocator.dupe(u8, msg_val.string) catch return Event{ .err = "error" };
                    self.scratch = msg_copy;
                    return Event{ .err = msg_copy };
                }
            }
            return Event{ .err = "server error" };
        }

        if (std.mem.eql(u8, event_type, "message_start")) {
            const parsed = std.json.parseFromSlice(
                std.json.Value,
                self.allocator,
                data,
                .{},
            ) catch continue;
            defer parsed.deinit();
            if (json_util.dottedLookup(parsed.value, "message.usage.input_tokens")) |v| {
                if (v == .integer) self.input_tokens = @intCast(v.integer);
            }
            continue;
        }

        if (std.mem.eql(u8, event_type, "content_block_start")) {
            const parsed = std.json.parseFromSlice(
                std.json.Value,
                self.allocator,
                data,
                .{},
            ) catch continue;
            defer parsed.deinit();

            const index_val = json_util.dottedLookup(parsed.value, "index") orelse continue;
            if (index_val != .integer) continue;
            const index: u64 = @intCast(index_val.integer);

            const cb_type = json_util.dottedLookup(parsed.value, "content_block.type") orelse continue;
            if (cb_type != .string) continue;

            if (std.mem.eql(u8, cb_type.string, "tool_use")) {
                const id_val = json_util.dottedLookup(parsed.value, "content_block.id") orelse continue;
                const name_val = json_util.dottedLookup(parsed.value, "content_block.name") orelse continue;
                if (id_val != .string or name_val != .string) continue;

                var block = ToolUseBlock{
                    .id = .empty,
                    .name = .empty,
                    .args = .empty,
                };
                block.id.appendSlice(self.allocator, id_val.string) catch continue;
                block.name.appendSlice(self.allocator, name_val.string) catch continue;

                self.tool_blocks.put(index, block) catch continue;
            }
            continue;
        }

        if (std.mem.eql(u8, event_type, "content_block_delta")) {
            const parsed = std.json.parseFromSlice(
                std.json.Value,
                self.allocator,
                data,
                .{},
            ) catch continue;
            defer parsed.deinit();

            const index_val = json_util.dottedLookup(parsed.value, "index") orelse continue;
            if (index_val != .integer) continue;
            const index: u64 = @intCast(index_val.integer);

            const delta_type = json_util.dottedLookup(parsed.value, "delta.type") orelse continue;
            if (delta_type != .string) continue;

            if (std.mem.eql(u8, delta_type.string, "text_delta")) {
                const text_val = json_util.dottedLookup(parsed.value, "delta.text") orelse continue;
                if (text_val != .string) continue;
                const text_copy = self.allocator.dupe(u8, text_val.string) catch continue;
                self.scratch = text_copy;
                return Event{ .token = text_copy };
            }

            if (std.mem.eql(u8, delta_type.string, "input_json_delta")) {
                const partial = json_util.dottedLookup(parsed.value, "delta.partial_json") orelse continue;
                if (partial != .string) continue;
                if (self.tool_blocks.getPtr(index)) |block| {
                    block.args.appendSlice(self.allocator, partial.string) catch continue;
                }
            }
            continue;
        }

        if (std.mem.eql(u8, event_type, "content_block_stop")) {
            const parsed = std.json.parseFromSlice(
                std.json.Value,
                self.allocator,
                data,
                .{},
            ) catch continue;
            defer parsed.deinit();

            const index_val = json_util.dottedLookup(parsed.value, "index") orelse continue;
            if (index_val != .integer) continue;
            const index: u64 = @intCast(index_val.integer);

            if (self.tool_blocks.fetchRemove(index)) |entry| {
                var block = entry.value;
                defer {
                    block.id.deinit(self.allocator);
                    block.name.deinit(self.allocator);
                    block.args.deinit(self.allocator);
                }
                const id_copy = self.allocator.dupe(u8, block.id.items) catch continue;
                const name_copy = self.allocator.dupe(u8, block.name.items) catch {
                    self.allocator.free(id_copy);
                    continue;
                };
                const args_copy = self.allocator.dupe(u8, block.args.items) catch {
                    self.allocator.free(id_copy);
                    self.allocator.free(name_copy);
                    continue;
                };
                self.scratch_tool_id = id_copy;
                self.scratch_tool_name = name_copy;
                self.scratch_tool_args = args_copy;
                return Event{ .tool_call = .{
                    .id = id_copy,
                    .name = name_copy,
                    .args_json = args_copy,
                } };
            }
            continue;
        }

        if (std.mem.eql(u8, event_type, "message_delta")) {
            const parsed = std.json.parseFromSlice(
                std.json.Value,
                self.allocator,
                data,
                .{},
            ) catch continue;
            defer parsed.deinit();

            if (json_util.dottedLookup(parsed.value, "delta.stop_reason")) |sr| {
                if (sr == .string) {
                    self.stop_reason = parseStopReason(sr.string);
                }
            }
            if (json_util.dottedLookup(parsed.value, "usage.output_tokens")) |v| {
                if (v == .integer) self.output_tokens = @intCast(v.integer);
            }
            continue;
        }

        if (std.mem.eql(u8, event_type, "message_stop")) {
            self.ended = true;
            return Event{ .done = .{
                .stop_reason = self.stop_reason,
                .usage = .{
                    .input_tokens = self.input_tokens,
                    .output_tokens = self.output_tokens,
                },
            } };
        }
        // Skip ping and other events
    }
}

fn parseStopReason(s: []const u8) StopReason {
    if (std.mem.eql(u8, s, "end_turn")) return .end_turn;
    if (std.mem.eql(u8, s, "max_tokens")) return .max_tokens;
    if (std.mem.eql(u8, s, "tool_use")) return .tool_use;
    if (std.mem.eql(u8, s, "stop_sequence")) return .stop_sequence;
    return .other;
}

// ---- Tests ----

const test_util = @import("test_util");
const MockTransport = http_client.MockTransport;

test "claude: builds request body with system + tools" {
    const allocator = std.testing.allocator;
    const fixture = test_util.minimal_done_sse;
    const canned = [_]http_client.Canned{.{ .body = fixture }};
    var mock = MockTransport.init(allocator, &canned);
    defer mock.deinit();

    var provider = try create(allocator, .{
        .model = "claude-opus-4-5",
        .resolved_credential = "sk-test",
    }, mock.transport());
    defer provider.deinit(allocator);

    const messages = [_]Message{
        .{ .role = .system, .content = "You are helpful." },
        .{ .role = .user, .content = "Hi" },
    };

    var it = try provider.send(allocator, .{
        .messages = &messages,
        .config = .{ .model = "claude-opus-4-5", .resolved_credential = "sk-test" },
    });
    defer it.deinit();
    while (it.next()) |_| {}

    const captured = mock.captured.items[0];
    try std.testing.expect(std.mem.endsWith(u8, captured.url, "/v1/messages"));
    try std.testing.expect(std.mem.indexOf(u8, captured.body, "\"system\"") != null);
    try std.testing.expect(std.mem.indexOf(u8, captured.body, "You are helpful.") != null);
    try std.testing.expect(std.mem.indexOf(u8, captured.body, "\"stream\":true") != null);
    try std.testing.expectEqualStrings("sk-test", test_util.headerValue(captured.headers, "x-api-key").?);
    try std.testing.expect(test_util.headerValue(captured.headers, "anthropic-version") != null);
}

test "claude: text streaming yields token events" {
    const allocator = std.testing.allocator;
    const fixture =
        "event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"id\":\"m1\",\"usage\":{\"input_tokens\":10,\"output_tokens\":0}}}\n\n" ++
        "event: content_block_start\ndata: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n\n" ++
        "event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"Hel\"}}\n\n" ++
        "event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"lo\"}}\n\n" ++
        "event: content_block_stop\ndata: {\"type\":\"content_block_stop\",\"index\":0}\n\n" ++
        "event: message_delta\ndata: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"},\"usage\":{\"output_tokens\":7}}\n\n" ++
        "event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n";

    const canned = [_]http_client.Canned{.{ .body = fixture }};
    var mock = MockTransport.init(allocator, &canned);
    defer mock.deinit();

    var provider = try create(allocator, .{
        .model = "claude-opus-4-5",
        .resolved_credential = "sk-test",
    }, mock.transport());
    defer provider.deinit(allocator);

    const messages = [_]Message{.{ .role = .user, .content = "say hello" }};
    var it = try provider.send(allocator, .{
        .messages = &messages,
        .config = .{ .model = "claude-opus-4-5", .resolved_credential = "sk-test" },
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

    try std.testing.expectEqual(@as(usize, 2), token_count);
    try std.testing.expect(done_ev != null);
    try std.testing.expectEqual(StopReason.end_turn, done_ev.?.stop_reason);
    try std.testing.expectEqual(@as(u32, 10), done_ev.?.usage.input_tokens);
    try std.testing.expectEqual(@as(u32, 7), done_ev.?.usage.output_tokens);
}

test "claude: tool_use streaming yields tool_call event" {
    const allocator = std.testing.allocator;
    const fixture =
        "event: content_block_start\ndata: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"tool_use\",\"id\":\"toolu_1\",\"name\":\"read_file\",\"input\":{}}}\n\n" ++
        "event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\"{\\\"path\\\":\"}}\n\n" ++
        "event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\"\\\"a.txt\\\"}\"}}\n\n" ++
        "event: content_block_stop\ndata: {\"type\":\"content_block_stop\",\"index\":0}\n\n" ++
        "event: message_delta\ndata: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"tool_use\"}}\n\n" ++
        "event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n";

    const canned = [_]http_client.Canned{.{ .body = fixture }};
    var mock = MockTransport.init(allocator, &canned);
    defer mock.deinit();

    var provider = try create(allocator, .{
        .model = "claude-opus-4-5",
        .resolved_credential = "sk-test",
    }, mock.transport());
    defer provider.deinit(allocator);

    const messages = [_]Message{.{ .role = .user, .content = "use a tool" }};
    var it = try provider.send(allocator, .{
        .messages = &messages,
        .config = .{ .model = "claude-opus-4-5", .resolved_credential = "sk-test" },
    });
    defer it.deinit();

    const events = try test_util.collectEvents(&it, allocator);
    defer test_util.freeEvents(allocator, events);

    var tc_found = false;
    var done_ev: ?core.DoneEvent = null;
    for (events) |ev| switch (ev) {
        .tool_call => |tc| {
            try std.testing.expectEqualStrings("toolu_1", tc.id);
            try std.testing.expectEqualStrings("read_file", tc.name);
            try std.testing.expectEqualStrings("{\"path\":\"a.txt\"}", tc.args_json);
            tc_found = true;
        },
        .done => |d| done_ev = d,
        else => {},
    };
    try std.testing.expect(tc_found);
    try std.testing.expect(done_ev != null);
    try std.testing.expectEqual(StopReason.tool_use, done_ev.?.stop_reason);
}

test "claude: error event surfaces" {
    const allocator = std.testing.allocator;
    const fixture =
        "event: error\ndata: {\"type\":\"error\",\"error\":{\"type\":\"overloaded_error\",\"message\":\"server overloaded\"}}\n\n";

    const canned = [_]http_client.Canned{.{ .body = fixture }};
    var mock = MockTransport.init(allocator, &canned);
    defer mock.deinit();

    var provider = try create(allocator, .{
        .model = "claude-opus-4-5",
        .resolved_credential = "sk-test",
    }, mock.transport());
    defer provider.deinit(allocator);

    const messages = [_]Message{.{ .role = .user, .content = "hi" }};
    var it = try provider.send(allocator, .{
        .messages = &messages,
        .config = .{ .model = "claude-opus-4-5", .resolved_credential = "sk-test" },
    });
    defer it.deinit();

    const events = try test_util.collectEvents(&it, allocator);
    defer test_util.freeEvents(allocator, events);

    try std.testing.expectEqual(@as(usize, 1), events.len);
    switch (events[0]) {
        .err => |e| try std.testing.expect(std.mem.indexOf(u8, e, "overloaded") != null),
        else => return error.WrongEvent,
    }
}

test "claude: tool_result message serializes correctly" {
    const allocator = std.testing.allocator;
    const fixture = test_util.minimal_done_sse;
    const canned = [_]http_client.Canned{.{ .body = fixture }};
    var mock = MockTransport.init(allocator, &canned);
    defer mock.deinit();

    var provider = try create(allocator, .{
        .model = "claude-opus-4-5",
        .resolved_credential = "sk-test",
    }, mock.transport());
    defer provider.deinit(allocator);

    const messages = [_]Message{
        .{ .role = .user, .content = "use a tool" },
        .{ .role = .tool_result, .content = "", .tool_result = .{
            .id = "toolu_abc",
            .output = "file contents here",
            .is_error = false,
        } },
    };

    var it = try provider.send(allocator, .{
        .messages = &messages,
        .config = .{ .model = "claude-opus-4-5", .resolved_credential = "sk-test" },
    });
    defer it.deinit();
    while (it.next()) |_| {}

    const body = mock.captured.items[0].body;
    try std.testing.expect(std.mem.indexOf(u8, body, "tool_result") != null);
    try std.testing.expect(std.mem.indexOf(u8, body, "toolu_abc") != null);
    try std.testing.expect(std.mem.indexOf(u8, body, "file contents here") != null);
}

test "claude: missing credential errors" {
    const allocator = std.testing.allocator;
    const canned = [_]http_client.Canned{};
    var mock = MockTransport.init(allocator, &canned);
    defer mock.deinit();

    var provider = try create(allocator, .{
        .model = "claude-opus-4-5",
        .resolved_credential = null,
    }, mock.transport());
    defer provider.deinit(allocator);

    const messages = [_]Message{.{ .role = .user, .content = "hi" }};
    const result = provider.send(allocator, .{
        .messages = &messages,
        .config = .{ .model = "claude-opus-4-5", .resolved_credential = null },
    });
    try std.testing.expectError(error.MissingCredential, result);
}

test "claude: e2e roundtrip" {
    if (std.c.getenv("PHOENIX_E2E") == null) return error.SkipZigTest;
    const key = std.mem.span(std.c.getenv("ANTHROPIC_API_KEY") orelse return error.SkipZigTest);

    var p = try create(std.testing.allocator, .{
        .model = "claude-haiku-4-5",
        .resolved_credential = key,
    }, null);
    defer p.deinit(std.testing.allocator);

    const msgs = [_]Message{.{ .role = .user, .content = "Reply with the word OK and nothing else." }};
    var it = try p.send(std.testing.allocator, .{
        .messages = &msgs,
        .config = .{ .model = "claude-haiku-4-5", .resolved_credential = key },
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
