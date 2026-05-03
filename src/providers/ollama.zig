/// Ollama native chat adapter with NDJSON streaming.
const std = @import("std");
const builtin = @import("builtin");
const core = @import("phoenix_core");
const http_client = core.http_client;
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

const DEFAULT_BASE_URL = "http://localhost:11434";

// ---- Provider struct ----

const OllamaProvider = struct {
    base: Provider,
    allocator: std.mem.Allocator,
    cfg: ProviderConfig,
    transport_owned: ?HttpTransport,
    transport: Transport,
};

pub fn create(
    allocator: std.mem.Allocator,
    io: std.Io,
    cfg: ProviderConfig,
    injected: ?Transport,
) !*Provider {
    const self = try allocator.create(OllamaProvider);
    errdefer allocator.destroy(self);
    self.* = .{
        .base = .{ .name = "ollama", .sendFn = sendImpl, .deinitFn = deinitImpl },
        .allocator = allocator,
        .cfg = cfg,
        .transport_owned = if (injected == null) HttpTransport.init(allocator, io) else null,
        .transport = undefined,
    };
    self.transport = if (injected) |t| t else self.transport_owned.?.transport();
    return &self.base;
}

fn deinitImpl(p: *Provider, allocator: std.mem.Allocator) void {
    const self: *OllamaProvider = @alignCast(@fieldParentPtr("base", p));
    if (self.cfg.resolved_credential_owned) {
        if (self.cfg.resolved_credential) |c| allocator.free(c);
    }
    if (self.transport_owned) |*t| t.deinit();
    allocator.destroy(self);
}

fn sendImpl(
    p: *Provider,
    allocator: std.mem.Allocator,
    options: SendOptions,
) anyerror!EventIterator {
    const self: *OllamaProvider = @alignCast(@fieldParentPtr("base", p));

    const base_url = options.config.base_url orelse self.cfg.base_url orelse DEFAULT_BASE_URL;

    var url_buf: std.ArrayList(u8) = .empty;
    defer url_buf.deinit(allocator);
    try url_buf.appendSlice(allocator, base_url);
    try url_buf.appendSlice(allocator, "/api/chat");
    const url = try url_buf.toOwnedSlice(allocator);
    defer allocator.free(url);

    const body = try buildOllamaBody(allocator, &options);
    defer allocator.free(body);

    var hdrs_buf: std.ArrayList(http_client.Header) = .empty;
    defer hdrs_buf.deinit(allocator);

    try hdrs_buf.append(allocator, .{ .name = "Content-Type", .value = "application/json" });

    const credential = options.config.resolved_credential orelse self.cfg.resolved_credential;
    if (credential) |cred| {
        var auth_val: std.ArrayList(u8) = .empty;
        defer auth_val.deinit(allocator);
        try auth_val.appendSlice(allocator, "Bearer ");
        try auth_val.appendSlice(allocator, cred);
        const auth_str = try auth_val.toOwnedSlice(allocator);
        defer allocator.free(auth_str);
        try hdrs_buf.append(allocator, .{ .name = "Authorization", .value = auth_str });
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

    const iter = try allocator.create(OllamaIterator);
    errdefer allocator.destroy(iter);
    iter.* = OllamaIterator.initIter(allocator, resp, iter);
    return iter.base;
}

fn buildOllamaBody(allocator: std.mem.Allocator, opts: *const SendOptions) ![]u8 {
    var buf: std.ArrayList(u8) = .empty;
    defer buf.deinit(allocator);

    try buf.appendSlice(allocator, "{\"model\":");
    try json_util.appendString(&buf, allocator, opts.config.model);
    try buf.appendSlice(allocator, ",\"stream\":true");

    if (opts.tools.len > 0) {
        try buf.appendSlice(allocator, ",\"tools\":[");
        for (opts.tools, 0..) |t, i| {
            if (i > 0) try buf.append(allocator, ',');
            try buf.appendSlice(allocator, "{\"type\":\"function\",\"function\":{\"name\":");
            try json_util.appendString(&buf, allocator, t.name);
            try buf.appendSlice(allocator, ",\"description\":");
            try json_util.appendString(&buf, allocator, t.description);
            try buf.appendSlice(allocator, ",\"parameters\":");
            try buf.appendSlice(allocator, t.schema);
            try buf.appendSlice(allocator, "}}");
        }
        try buf.append(allocator, ']');
    }

    try buf.appendSlice(allocator, ",\"messages\":[");
    try appendOllamaMessages(&buf, allocator, opts.messages);
    try buf.append(allocator, ']');
    try buf.append(allocator, '}');

    return buf.toOwnedSlice(allocator);
}

fn appendOllamaMessages(
    buf: *std.ArrayList(u8),
    allocator: std.mem.Allocator,
    messages: []const Message,
) !void {
    var first = true;
    for (messages) |msg| {
        if (!first) try buf.append(allocator, ',');
        first = false;

        switch (msg.role) {
            .system => {
                try buf.appendSlice(allocator, "{\"role\":\"system\",\"content\":");
                try json_util.appendString(buf, allocator, msg.content);
                try buf.append(allocator, '}');
            },
            .user => {
                try buf.appendSlice(allocator, "{\"role\":\"user\",\"content\":");
                try json_util.appendString(buf, allocator, msg.content);
                try buf.append(allocator, '}');
            },
            .assistant => {
                try buf.appendSlice(allocator, "{\"role\":\"assistant\",\"content\":");
                try json_util.appendString(buf, allocator, msg.content);
                try buf.append(allocator, '}');
            },
            .tool_call => {
                // Ollama tool_calls: arguments is an object (not string)
                if (msg.tool_call) |tc| {
                    try buf.appendSlice(allocator, "{\"role\":\"assistant\",\"content\":\"\",\"tool_calls\":[{\"function\":{\"name\":");
                    try json_util.appendString(buf, allocator, tc.name);
                    try buf.appendSlice(allocator, ",\"arguments\":");
                    if (tc.args_json.len > 0) {
                        try buf.appendSlice(allocator, tc.args_json);
                    } else {
                        try buf.appendSlice(allocator, "{}");
                    }
                    try buf.appendSlice(allocator, "}}]}");
                }
            },
            .tool_result => {
                // Ollama: role "tool", no tool_call_id
                if (msg.tool_result) |tr| {
                    try buf.appendSlice(allocator, "{\"role\":\"tool\",\"content\":");
                    try json_util.appendString(buf, allocator, tr.output);
                    try buf.append(allocator, '}');
                }
            },
        }
    }
}

// ---- NDJSON Iterator ----

const OllamaIterator = struct {
    base: EventIterator,
    allocator: std.mem.Allocator,
    response: http_client.Response,
    ended: bool,
    tc_seq: u32,
    // Line buffer for partial reads
    line_buf: std.ArrayList(u8),
    // Scratch
    scratch: ?[]u8,
    scratch2: ?[]u8,
    scratch3: ?[]u8,

    fn initIter(allocator: std.mem.Allocator, resp: http_client.Response, self_ptr: *OllamaIterator) OllamaIterator {
        return .{
            .base = .{ .nextFn = nextImpl, .deinitFn = deinitIter, .ctx = self_ptr },
            .allocator = allocator,
            .response = resp,
            .ended = false,
            .tc_seq = 0,
            .line_buf = .empty,
            .scratch = null,
            .scratch2 = null,
            .scratch3 = null,
        };
    }

    fn freeScratch(self: *OllamaIterator) void {
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
    const self: *OllamaIterator = @ptrCast(@alignCast(it.ctx));
    self.freeScratch();
    self.line_buf.deinit(self.allocator);
    self.response.deinit();
    self.allocator.destroy(self);
}

fn nextImpl(it: *EventIterator) ?Event {
    const self: *OllamaIterator = @ptrCast(@alignCast(it.ctx));
    if (self.ended) return null;
    self.freeScratch();

    while (true) {
        // Read next line from response
        const maybe_line = self.response.body_reader.takeDelimiter('\n') catch {
            self.ended = true;
            return null;
        };
        const line_raw = maybe_line orelse {
            // EOF — if we have buffered data, try to parse it
            if (self.line_buf.items.len > 0) {
                const line = self.line_buf.items;
                if (processLine(self, line)) |ev| return ev;
            }
            self.ended = true;
            return null;
        };

        // Strip carriage return if present
        const line = if (line_raw.len > 0 and line_raw[line_raw.len - 1] == '\r')
            line_raw[0 .. line_raw.len - 1]
        else
            line_raw;

        if (line.len == 0) continue;

        // Check if line is complete JSON
        if (processLine(self, line)) |ev| return ev;
    }
}

fn processLine(self: *OllamaIterator, line: []const u8) ?Event {
    const parsed = std.json.parseFromSlice(
        std.json.Value,
        self.allocator,
        line,
        .{},
    ) catch return null;
    defer parsed.deinit();

    // Check done
    if (json_util.dottedLookup(parsed.value, "done")) |done_val| {
        if (done_val == .bool and done_val.bool) {
            self.ended = true;

            var sr = StopReason.other;
            if (json_util.dottedLookup(parsed.value, "done_reason")) |dr| {
                if (dr == .string) sr = mapDoneReason(dr.string);
            }

            var input_tokens: u32 = 0;
            var output_tokens: u32 = 0;
            if (json_util.dottedLookup(parsed.value, "prompt_eval_count")) |v| {
                if (v == .integer) input_tokens = @intCast(v.integer);
            }
            if (json_util.dottedLookup(parsed.value, "eval_count")) |v| {
                if (v == .integer) output_tokens = @intCast(v.integer);
            }

            return Event{ .done = .{
                .stop_reason = sr,
                .usage = .{ .input_tokens = input_tokens, .output_tokens = output_tokens },
            } };
        }
    }

    // Check message content
    if (json_util.dottedLookup(parsed.value, "message.content")) |content_val| {
        if (content_val == .string and content_val.string.len > 0) {
            const copy = self.allocator.dupe(u8, content_val.string) catch return null;
            self.scratch = copy;
            return Event{ .token = copy };
        }
    }

    // Check message tool_calls
    if (json_util.dottedLookup(parsed.value, "message.tool_calls")) |tc_arr| {
        if (tc_arr == .array and tc_arr.array.items.len > 0) {
            const tc_item = tc_arr.array.items[0];
            if (tc_item == .object) {
                const fn_val = tc_item.object.get("function") orelse return null;
                if (fn_val != .object) return null;

                const name_val = fn_val.object.get("name") orelse return null;
                if (name_val != .string) return null;

                var id_buf: [32]u8 = undefined;
                const id_str = std.fmt.bufPrint(&id_buf, "ollama-tc-{d}", .{self.tc_seq}) catch return null;
                self.tc_seq += 1;

                const id_copy = self.allocator.dupe(u8, id_str) catch return null;
                const name_copy = self.allocator.dupe(u8, name_val.string) catch {
                    self.allocator.free(id_copy);
                    return null;
                };

                // Serialize args as JSON
                const args_val = fn_val.object.get("arguments") orelse std.json.Value{ .object = .{} };
                const args_json = std.json.Stringify.valueAlloc(self.allocator, args_val, .{}) catch {
                    self.allocator.free(id_copy);
                    self.allocator.free(name_copy);
                    return null;
                };

                self.scratch = id_copy;
                self.scratch2 = name_copy;
                self.scratch3 = args_json;
                return Event{ .tool_call = .{
                    .id = id_copy,
                    .name = name_copy,
                    .args_json = args_json,
                } };
            }
        }
    }

    return null;
}

fn mapDoneReason(s: []const u8) StopReason {
    if (std.mem.eql(u8, s, "stop")) return .end_turn;
    if (std.mem.eql(u8, s, "length")) return .max_tokens;
    if (std.mem.eql(u8, s, "tool_calls")) return .tool_use;
    return .other;
}

// ---- Tests ----

const test_util = @import("test_util");
const MockTransport = http_client.MockTransport;

test "ollama: builds /api/chat request" {
    const allocator = std.testing.allocator;
    const fixture =
        "{\"model\":\"x\",\"done\":true,\"done_reason\":\"stop\",\"prompt_eval_count\":0,\"eval_count\":0}\n";
    const canned = [_]http_client.Canned{.{ .body = fixture }};
    var mock = MockTransport.init(allocator, &canned);
    defer mock.deinit();

    var provider = try create(allocator, std.testing.io, .{
        .model = "llama3.1",
    }, mock.transport());
    defer provider.deinit(allocator);

    const messages = [_]Message{.{ .role = .user, .content = "hi" }};
    var it = try provider.send(allocator, .{
        .messages = &messages,
        .config = .{ .model = "llama3.1" },
    });
    defer it.deinit();
    while (it.next()) |_| {}

    const captured = mock.captured.items[0];
    try std.testing.expectEqualStrings("http://localhost:11434/api/chat", captured.url);
    try std.testing.expect(std.mem.indexOf(u8, captured.body, "\"stream\":true") != null);
    try std.testing.expect(std.mem.indexOf(u8, captured.body, "\"messages\"") != null);
    // No Authorization header without credential
    try std.testing.expect(test_util.headerValue(captured.headers, "Authorization") == null);
}

test "ollama: NDJSON token streaming" {
    const allocator = std.testing.allocator;
    const fixture =
        "{\"model\":\"x\",\"message\":{\"role\":\"assistant\",\"content\":\"He\"},\"done\":false}\n" ++
        "{\"model\":\"x\",\"message\":{\"role\":\"assistant\",\"content\":\"llo\"},\"done\":false}\n" ++
        "{\"model\":\"x\",\"done\":true,\"done_reason\":\"stop\",\"prompt_eval_count\":3,\"eval_count\":2}\n";

    const canned = [_]http_client.Canned{.{ .body = fixture }};
    var mock = MockTransport.init(allocator, &canned);
    defer mock.deinit();

    var provider = try create(allocator, std.testing.io, .{ .model = "llama3.1" }, mock.transport());
    defer provider.deinit(allocator);

    const messages = [_]Message{.{ .role = .user, .content = "hi" }};
    var it = try provider.send(allocator, .{
        .messages = &messages,
        .config = .{ .model = "llama3.1" },
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
    try std.testing.expectEqual(@as(u32, 3), done_ev.?.usage.input_tokens);
    try std.testing.expectEqual(@as(u32, 2), done_ev.?.usage.output_tokens);
}

test "ollama: tool_calls yield tool_call events" {
    const allocator = std.testing.allocator;
    const fixture =
        "{\"model\":\"x\",\"message\":{\"role\":\"assistant\",\"tool_calls\":[{\"function\":{\"name\":\"read_file\",\"arguments\":{\"path\":\"a.txt\"}}}]},\"done\":false}\n" ++
        "{\"model\":\"x\",\"done\":true,\"done_reason\":\"tool_calls\"}\n";

    const canned = [_]http_client.Canned{.{ .body = fixture }};
    var mock = MockTransport.init(allocator, &canned);
    defer mock.deinit();

    var provider = try create(allocator, std.testing.io, .{ .model = "llama3.1" }, mock.transport());
    defer provider.deinit(allocator);

    const messages = [_]Message{.{ .role = .user, .content = "use tool" }};
    var it = try provider.send(allocator, .{
        .messages = &messages,
        .config = .{ .model = "llama3.1" },
    });
    defer it.deinit();

    const events = try test_util.collectEvents(&it, allocator);
    defer test_util.freeEvents(allocator, events);

    var tc_found = false;
    var done_ev: ?core.DoneEvent = null;
    for (events) |ev| switch (ev) {
        .tool_call => |tc| {
            try std.testing.expectEqualStrings("read_file", tc.name);
            try std.testing.expect(std.mem.indexOf(u8, tc.args_json, "a.txt") != null);
            tc_found = true;
        },
        .done => |d| done_ev = d,
        else => {},
    };
    try std.testing.expect(tc_found);
    try std.testing.expect(done_ev != null);
    try std.testing.expectEqual(StopReason.tool_use, done_ev.?.stop_reason);
}

test "ollama: tool_result message serializes" {
    const allocator = std.testing.allocator;
    const fixture =
        "{\"model\":\"x\",\"done\":true,\"done_reason\":\"stop\"}\n";

    const canned = [_]http_client.Canned{.{ .body = fixture }};
    var mock = MockTransport.init(allocator, &canned);
    defer mock.deinit();

    var provider = try create(allocator, std.testing.io, .{ .model = "llama3.1" }, mock.transport());
    defer provider.deinit(allocator);

    const messages = [_]Message{
        .{ .role = .user, .content = "use tool" },
        .{ .role = .tool_result, .content = "", .tool_result = .{
            .id = "tc_1",
            .output = "result here",
            .is_error = false,
        } },
    };

    var it = try provider.send(allocator, .{
        .messages = &messages,
        .config = .{ .model = "llama3.1" },
    });
    defer it.deinit();
    while (it.next()) |_| {}

    const body = mock.captured.items[0].body;
    // Ollama serializes tool_result as role "tool" with content, no tool_call_id
    try std.testing.expect(std.mem.indexOf(u8, body, "\"tool\"") != null);
    try std.testing.expect(std.mem.indexOf(u8, body, "result here") != null);
    // No tool_call_id
    try std.testing.expect(std.mem.indexOf(u8, body, "tool_call_id") == null);
}

test "ollama: partial line buffering" {
    // This test verifies that the iterator correctly parses NDJSON even when
    // the entire body is provided as a continuous stream. The actual chunked
    // delivery is transparent because std.Io.Reader.takeDelimiter handles it.
    const allocator = std.testing.allocator;
    // Provide two token lines and a done line, concatenated
    const fixture =
        "{\"model\":\"x\",\"message\":{\"role\":\"assistant\",\"content\":\"token1\"},\"done\":false}\n" ++
        "{\"model\":\"x\",\"message\":{\"role\":\"assistant\",\"content\":\"token2\"},\"done\":false}\n" ++
        "{\"model\":\"x\",\"done\":true,\"done_reason\":\"stop\",\"prompt_eval_count\":1,\"eval_count\":2}\n";

    const canned = [_]http_client.Canned{.{ .body = fixture }};
    var mock = MockTransport.init(allocator, &canned);
    defer mock.deinit();

    var provider = try create(allocator, std.testing.io, .{ .model = "llama3.1" }, mock.transport());
    defer provider.deinit(allocator);

    const messages = [_]Message{.{ .role = .user, .content = "hi" }};
    var it = try provider.send(allocator, .{
        .messages = &messages,
        .config = .{ .model = "llama3.1" },
    });
    defer it.deinit();

    const events = try test_util.collectEvents(&it, allocator);
    defer test_util.freeEvents(allocator, events);

    var token_count: usize = 0;
    var done_seen = false;
    for (events) |ev| switch (ev) {
        .token => token_count += 1,
        .done => done_seen = true,
        else => {},
    };

    try std.testing.expectEqual(@as(usize, 2), token_count);
    try std.testing.expect(done_seen);
}

test "ollama: e2e roundtrip" {
    if (std.c.getenv("PHOENIX_E2E") == null) return error.SkipZigTest;

    var p = try create(std.testing.allocator, std.testing.io, .{
        .model = "llama3.2",
    }, null);
    defer p.deinit(std.testing.allocator);

    const msgs = [_]Message{.{ .role = .user, .content = "Reply with the word OK and nothing else." }};
    var it = try p.send(std.testing.allocator, .{
        .messages = &msgs,
        .config = .{ .model = "llama3.2" },
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
