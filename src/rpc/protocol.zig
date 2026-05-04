const std = @import("std");
const core = @import("phoenix_core");
const commands = @import("commands");

pub const ErrorCode = enum {
    ParseError,
    InvalidRequest,
    MethodNotFound,
    InvalidParams,
    InternalError,

    pub fn name(self: ErrorCode) []const u8 {
        return @tagName(self);
    }
};

pub const Request = struct {
    id: i64,
    method: []const u8,
    params: std.json.Value,
};

pub const DispatchParams = struct {
    input: []const u8,
};

pub fn parseDispatchParams(arena: std.mem.Allocator, v: std.json.Value) !DispatchParams {
    if (v != .object) return error.InvalidParams;
    const input_v = v.object.get("input") orelse return error.InvalidParams;
    if (input_v != .string) return error.InvalidParams;
    return .{ .input = try arena.dupe(u8, input_v.string) };
}

pub const SessionSendParams = struct {
    text: []const u8,
};

pub fn parseSessionSendParams(arena: std.mem.Allocator, v: std.json.Value) !SessionSendParams {
    if (v != .object) return error.InvalidParams;
    const text_v = v.object.get("text") orelse return error.InvalidParams;
    if (text_v != .string) return error.InvalidParams;
    return .{ .text = try arena.dupe(u8, text_v.string) };
}

pub const ApplyModelChoiceParams = struct {
    provider_index: u32,
    kind: core.ProviderKind,
    model: []const u8,
    is_active: bool,
};

pub const ApplySessionChoiceParams = struct {
    id: []const u8,
};

pub fn parseApplySessionChoiceParams(arena: std.mem.Allocator, v: std.json.Value) !ApplySessionChoiceParams {
    if (v != .object) return error.InvalidParams;
    const id_v = v.object.get("id") orelse return error.InvalidParams;
    if (id_v != .string) return error.InvalidParams;
    return .{ .id = try arena.dupe(u8, id_v.string) };
}

pub fn writeCommandListResult(
    out: *std.ArrayList(u8),
    a: std.mem.Allocator,
    items: []const commands.CommandInfo,
) !void {
    try out.appendSlice(a, "{\"commands\":[");
    for (items, 0..) |it, i| {
        if (i > 0) try out.appendSlice(a, ",");
        try out.appendSlice(a, "{\"name\":");
        try writeJsonString(out, a, it.name);
        try out.appendSlice(a, ",\"summary\":");
        try writeJsonString(out, a, it.summary);
        try out.print(a, ",\"is_skill\":{}}}", .{it.is_skill});
    }
    try out.appendSlice(a, "]}");
}

pub fn writeApplySessionChoiceResult(
    out: *std.ArrayList(u8),
    a: std.mem.Allocator,
    message: []const u8,
    message_count: u32,
    messages: []const core.Message,
) !void {
    try out.appendSlice(a, "{\"message\":");
    try writeJsonString(out, a, message);
    try out.print(a, ",\"message_count\":{d},\"messages\":[", .{message_count});
    for (messages, 0..) |m, i| {
        if (i > 0) try out.appendSlice(a, ",");
        try out.appendSlice(a, "{\"role\":");
        try writeJsonString(out, a, @tagName(m.role));
        try out.appendSlice(a, ",\"content\":");
        try writeJsonString(out, a, m.content);
        try out.appendSlice(a, "}");
    }
    try out.appendSlice(a, "]}");
}

pub fn parseApplyModelChoiceParams(arena: std.mem.Allocator, v: std.json.Value) !ApplyModelChoiceParams {
    if (v != .object) return error.InvalidParams;
    const obj = v.object;
    const pi = obj.get("provider_index") orelse return error.InvalidParams;
    const kn = obj.get("kind") orelse return error.InvalidParams;
    const md = obj.get("model") orelse return error.InvalidParams;
    const ia = obj.get("is_active") orelse return error.InvalidParams;
    if (pi != .integer or kn != .string or md != .string or ia != .bool) return error.InvalidParams;
    if (pi.integer < 0) return error.InvalidParams;
    const kind = std.meta.stringToEnum(core.ProviderKind, kn.string) orelse return error.InvalidParams;
    return .{
        .provider_index = @intCast(pi.integer),
        .kind = kind,
        .model = try arena.dupe(u8, md.string),
        .is_active = ia.bool,
    };
}

pub fn writeConfigGetResult(
    out: *std.ArrayList(u8),
    a: std.mem.Allocator,
    default_provider: ?struct { kind: core.ProviderKind, model: []const u8 },
    sources_count: usize,
) !void {
    try out.appendSlice(a, "{\"default_provider\":");
    if (default_provider) |dp| {
        try out.appendSlice(a, "{\"kind\":");
        try writeJsonString(out, a, @tagName(dp.kind));
        try out.appendSlice(a, ",\"model\":");
        try writeJsonString(out, a, dp.model);
        try out.appendSlice(a, "}");
    } else {
        try out.appendSlice(a, "null");
    }
    try out.print(a, ",\"sources_count\":{d}}}", .{sources_count});
}

pub fn writeDispatchResult(
    out: *std.ArrayList(u8),
    a: std.mem.Allocator,
    r: commands.Result,
) !void {
    switch (r) {
        .not_a_command => try out.appendSlice(a, "{\"kind\":\"not_a_command\"}"),
        .message => |m| {
            try out.appendSlice(a, "{\"kind\":\"message\",\"text\":");
            try writeJsonString(out, a, m);
            try out.appendSlice(a, "}");
        },
        .cleared => |m| {
            try out.appendSlice(a, "{\"kind\":\"cleared\",\"text\":");
            try writeJsonString(out, a, m);
            try out.appendSlice(a, "}");
        },
        .compacted => |m| {
            try out.appendSlice(a, "{\"kind\":\"compacted\",\"text\":");
            try writeJsonString(out, a, m);
            try out.appendSlice(a, "}");
        },
        .err => |m| {
            try out.appendSlice(a, "{\"kind\":\"err\",\"text\":");
            try writeJsonString(out, a, m);
            try out.appendSlice(a, "}");
        },
        .model_picker => |p| {
            try out.appendSlice(a, "{\"kind\":\"model_picker\",\"title\":");
            try writeJsonString(out, a, p.title);
            try out.appendSlice(a, ",\"choices\":[");
            for (p.choices, 0..) |c, i| {
                if (i > 0) try out.appendSlice(a, ",");
                try out.print(a, "{{\"provider_index\":{d}", .{c.provider_index});
                try out.appendSlice(a, ",\"kind\":");
                try writeJsonString(out, a, @tagName(c.kind));
                try out.appendSlice(a, ",\"model\":");
                try writeJsonString(out, a, c.model);
                try out.print(a, ",\"is_active\":{}}}", .{c.is_active});
            }
            try out.appendSlice(a, "]}");
        },
        .inject_context => |frag| {
            try out.appendSlice(a, "{\"kind\":\"inject_context\",\"label\":");
            try writeJsonString(out, a, frag.label);
            try out.appendSlice(a, ",\"body\":");
            try writeJsonString(out, a, frag.body);
            try out.appendSlice(a, ",\"user_message\":");
            try writeJsonString(out, a, frag.user_message);
            try out.appendSlice(a, "}");
        },
        // The server intercepts these markers before encoding, so they
        // shouldn't reach the wire in normal operation. Encode minimally for
        // diagnostic completeness — the TUI will treat the kind as unknown.
        .clear_session => try out.appendSlice(a, "{\"kind\":\"clear_session\"}"),
        .compact_session => try out.appendSlice(a, "{\"kind\":\"compact_session\"}"),
        .session_picker => |p| {
            try out.appendSlice(a, "{\"kind\":\"session_picker\",\"title\":");
            try writeJsonString(out, a, p.title);
            try out.appendSlice(a, ",\"choices\":[");
            for (p.choices, 0..) |c, i| {
                if (i > 0) try out.appendSlice(a, ",");
                try out.appendSlice(a, "{\"id\":");
                try writeJsonString(out, a, c.id);
                try out.appendSlice(a, ",\"name\":");
                try writeJsonString(out, a, c.name);
                try out.print(a, ",\"updated_at\":{d}", .{c.updated_at});
                try out.print(a, ",\"message_count\":{d}}}", .{c.message_count});
            }
            try out.appendSlice(a, "]}");
        },
    }
}

/// Map a StopReason enum to the wire string used in session.send terminal results.
fn stopReasonName(sr: core.StopReason) []const u8 {
    return switch (sr) {
        .end_turn => "end_turn",
        .max_tokens => "max_tokens",
        .tool_use => "tool_use",
        .stop_sequence => "stop_sequence",
        .other => "other",
    };
}

/// Write a complete `event` line for a session.send stream:
///   {"id":N,"event":{"kind":"token","text":"..."}}
/// Caller appends the trailing '\n' before flushing.
pub fn writeEventTokenLine(
    out: *std.ArrayList(u8),
    a: std.mem.Allocator,
    id: i64,
    text: []const u8,
) !void {
    try out.print(a, "{{\"id\":{d},\"event\":{{\"kind\":\"token\",\"text\":", .{id});
    try writeJsonString(out, a, text);
    try out.appendSlice(a, "}}");
}

/// Write a complete `event` line for a streamed err notice:
///   {"id":N,"event":{"kind":"err","text":"..."}}
/// Caller appends the trailing '\n' before flushing.
pub fn writeEventErrLine(
    out: *std.ArrayList(u8),
    a: std.mem.Allocator,
    id: i64,
    text: []const u8,
) !void {
    try out.print(a, "{{\"id\":{d},\"event\":{{\"kind\":\"err\",\"text\":", .{id});
    try writeJsonString(out, a, text);
    try out.appendSlice(a, "}}");
}

/// Write a complete `event` line for an assistant tool invocation:
///   {"id":N,"event":{"kind":"tool_call","tool_id":"...","name":"...","args":"..."}}
/// `args` is the raw JSON string the model produced. Caller appends '\n'.
pub fn writeEventToolCallLine(
    out: *std.ArrayList(u8),
    a: std.mem.Allocator,
    id: i64,
    tool_id: []const u8,
    name: []const u8,
    args: []const u8,
) !void {
    try out.print(a, "{{\"id\":{d},\"event\":{{\"kind\":\"tool_call\",\"tool_id\":", .{id});
    try writeJsonString(out, a, tool_id);
    try out.appendSlice(a, ",\"name\":");
    try writeJsonString(out, a, name);
    try out.appendSlice(a, ",\"args\":");
    try writeJsonString(out, a, args);
    try out.appendSlice(a, "}}");
}

/// Write a complete `event` line for the harness-side result of a tool call:
///   {"id":N,"event":{"kind":"tool_result","tool_id":"...","output":"...","is_error":false}}
/// Caller appends '\n'.
pub fn writeEventToolResultLine(
    out: *std.ArrayList(u8),
    a: std.mem.Allocator,
    id: i64,
    tool_id: []const u8,
    output: []const u8,
    is_error: bool,
) !void {
    try out.print(a, "{{\"id\":{d},\"event\":{{\"kind\":\"tool_result\",\"tool_id\":", .{id});
    try writeJsonString(out, a, tool_id);
    try out.appendSlice(a, ",\"output\":");
    try writeJsonString(out, a, output);
    try out.appendSlice(a, ",\"is_error\":");
    try out.appendSlice(a, if (is_error) "true" else "false");
    try out.appendSlice(a, "}}");
}

/// Write a complete `event` line for an inject_context preamble:
///   {"id":N,"event":{"kind":"context","label":"...","body":"..."}}
/// Caller appends the trailing '\n' before flushing.
pub fn writeEventContextLine(
    out: *std.ArrayList(u8),
    a: std.mem.Allocator,
    id: i64,
    label: []const u8,
    body: []const u8,
) !void {
    try out.print(a, "{{\"id\":{d},\"event\":{{\"kind\":\"context\",\"label\":", .{id});
    try writeJsonString(out, a, label);
    try out.appendSlice(a, ",\"body\":");
    try writeJsonString(out, a, body);
    try out.appendSlice(a, "}}");
}

/// Write the body of a session.send terminal result for a successful conversation
/// turn. Caller wraps with `writeSuccess` and appends '\n'.
pub fn writeSendConversationOkBody(
    out: *std.ArrayList(u8),
    a: std.mem.Allocator,
    stop_reason: core.StopReason,
    input_tokens: u32,
    output_tokens: u32,
) !void {
    try out.appendSlice(a, "{\"kind\":\"conversation\",\"ok\":true,\"stop_reason\":");
    try writeJsonString(out, a, stopReasonName(stop_reason));
    try out.print(a, ",\"input_tokens\":{d},\"output_tokens\":{d}}}", .{ input_tokens, output_tokens });
}

/// Write the body of a session.send terminal result for a conversation that
/// emitted at least one event but ultimately failed (e.g. provider err mid-stream).
pub fn writeSendConversationErrBody(
    out: *std.ArrayList(u8),
    a: std.mem.Allocator,
    reason: []const u8,
) !void {
    try out.appendSlice(a, "{\"kind\":\"conversation\",\"ok\":false,\"reason\":");
    try writeJsonString(out, a, reason);
    try out.appendSlice(a, "}");
}

/// Write the body of a session.send terminal result for a slash-command outcome
/// (no event lines were emitted, no AI call was made).
pub fn writeSendCommandBody(
    out: *std.ArrayList(u8),
    a: std.mem.Allocator,
    r: commands.Result,
) !void {
    try out.appendSlice(a, "{\"kind\":\"command\",\"command\":");
    try writeDispatchResult(out, a, r);
    try out.appendSlice(a, "}");
}

pub fn writeApplyModelChoiceResult(
    out: *std.ArrayList(u8),
    a: std.mem.Allocator,
    message: []const u8,
    default_provider: struct { kind: core.ProviderKind, model: []const u8 },
) !void {
    try out.appendSlice(a, "{\"message\":");
    try writeJsonString(out, a, message);
    try out.appendSlice(a, ",\"default_provider\":{\"kind\":");
    try writeJsonString(out, a, @tagName(default_provider.kind));
    try out.appendSlice(a, ",\"model\":");
    try writeJsonString(out, a, default_provider.model);
    try out.appendSlice(a, "}}");
}

pub fn writeSuccess(
    out: *std.ArrayList(u8),
    a: std.mem.Allocator,
    id: i64,
    result_body: []const u8,
) !void {
    try out.print(a, "{{\"id\":{d},\"result\":", .{id});
    try out.appendSlice(a, result_body);
    try out.appendSlice(a, "}");
}

pub fn writeError(
    out: *std.ArrayList(u8),
    a: std.mem.Allocator,
    id: i64,
    code: ErrorCode,
    message: []const u8,
) !void {
    try out.print(a, "{{\"id\":{d},\"error\":{{\"code\":\"{s}\",\"message\":", .{ id, code.name() });
    try writeJsonString(out, a, message);
    try out.appendSlice(a, "}}");
}

pub fn parseRequest(arena: std.mem.Allocator, line: []const u8) !Request {
    const parsed = std.json.parseFromSliceLeaky(std.json.Value, arena, line, .{}) catch return error.ParseError;
    if (parsed != .object) return error.InvalidRequest;
    const obj = parsed.object;
    const id_v = obj.get("id") orelse return error.InvalidRequest;
    const method_v = obj.get("method") orelse return error.InvalidRequest;
    if (id_v != .integer or method_v != .string) return error.InvalidRequest;
    const params = obj.get("params") orelse std.json.Value{ .null = {} };
    return .{
        .id = id_v.integer,
        .method = method_v.string,
        .params = params,
    };
}

fn writeJsonString(out: *std.ArrayList(u8), a: std.mem.Allocator, s: []const u8) !void {
    const encoded = try std.json.Stringify.valueAlloc(a, s, .{});
    defer a.free(encoded);
    try out.appendSlice(a, encoded);
}

test "parseRequest happy path" {
    var arena = std.heap.ArenaAllocator.init(std.testing.allocator);
    defer arena.deinit();
    const req = try parseRequest(
        arena.allocator(),
        "{\"id\":7,\"method\":\"command.dispatch\",\"params\":{\"input\":\"/model\"}}",
    );
    try std.testing.expectEqual(@as(i64, 7), req.id);
    try std.testing.expectEqualStrings("command.dispatch", req.method);
}

test "parseRequest missing method is InvalidRequest" {
    var arena = std.heap.ArenaAllocator.init(std.testing.allocator);
    defer arena.deinit();
    try std.testing.expectError(
        error.InvalidRequest,
        parseRequest(arena.allocator(), "{\"id\":1}"),
    );
}

test "parseRequest missing id is InvalidRequest" {
    var arena = std.heap.ArenaAllocator.init(std.testing.allocator);
    defer arena.deinit();
    try std.testing.expectError(
        error.InvalidRequest,
        parseRequest(arena.allocator(), "{\"method\":\"config.get\"}"),
    );
}

test "parseRequest bad JSON is ParseError" {
    var arena = std.heap.ArenaAllocator.init(std.testing.allocator);
    defer arena.deinit();
    try std.testing.expectError(
        error.ParseError,
        parseRequest(arena.allocator(), "not json at all {{{"),
    );
}

test "writeDispatchResult not_a_command" {
    const a = std.testing.allocator;
    var out: std.ArrayList(u8) = .empty;
    defer out.deinit(a);
    try writeDispatchResult(&out, a, .not_a_command);
    try std.testing.expectEqualStrings("{\"kind\":\"not_a_command\"}", out.items);
}

test "writeDispatchResult message" {
    const a = std.testing.allocator;
    var out: std.ArrayList(u8) = .empty;
    defer out.deinit(a);
    try writeDispatchResult(&out, a, .{ .message = "hello" });
    try std.testing.expectEqualStrings("{\"kind\":\"message\",\"text\":\"hello\"}", out.items);
}

test "writeDispatchResult err" {
    const a = std.testing.allocator;
    var out: std.ArrayList(u8) = .empty;
    defer out.deinit(a);
    try writeDispatchResult(&out, a, .{ .err = "bad input" });
    try std.testing.expectEqualStrings("{\"kind\":\"err\",\"text\":\"bad input\"}", out.items);
}

test "writeDispatchResult model_picker" {
    const a = std.testing.allocator;
    var out: std.ArrayList(u8) = .empty;
    defer out.deinit(a);
    const choices = [_]commands.ModelChoice{.{
        .provider_index = 0,
        .kind = .claude,
        .model = "claude-opus-4-7",
        .is_active = true,
    }};
    try writeDispatchResult(&out, a, .{ .model_picker = .{
        .title = "Pick a model",
        .choices = &choices,
    } });
    try std.testing.expect(std.mem.indexOf(u8, out.items, "model_picker") != null);
    try std.testing.expect(std.mem.indexOf(u8, out.items, "claude-opus-4-7") != null);
    try std.testing.expect(std.mem.indexOf(u8, out.items, "true") != null);
}

test "writeDispatchResult inject_context" {
    const a = std.testing.allocator;
    var out: std.ArrayList(u8) = .empty;
    defer out.deinit(a);
    try writeDispatchResult(&out, a, .{ .inject_context = .{
        .label = "skill:test",
        .body = "some body",
        .user_message = "hi",
    } });
    try std.testing.expect(std.mem.indexOf(u8, out.items, "inject_context") != null);
    try std.testing.expect(std.mem.indexOf(u8, out.items, "skill:test") != null);
}

test "writeConfigGetResult with provider" {
    const a = std.testing.allocator;
    var out: std.ArrayList(u8) = .empty;
    defer out.deinit(a);
    try writeConfigGetResult(&out, a, .{ .kind = .claude, .model = "claude-opus-4-7" }, 2);
    try std.testing.expect(std.mem.indexOf(u8, out.items, "default_provider") != null);
    try std.testing.expect(std.mem.indexOf(u8, out.items, "claude-opus-4-7") != null);
    try std.testing.expect(std.mem.indexOf(u8, out.items, "\"sources_count\":2") != null);
}

test "writeConfigGetResult null provider" {
    const a = std.testing.allocator;
    var out: std.ArrayList(u8) = .empty;
    defer out.deinit(a);
    try writeConfigGetResult(&out, a, null, 0);
    try std.testing.expect(std.mem.indexOf(u8, out.items, "\"default_provider\":null") != null);
}

test "writeError round-trip" {
    const a = std.testing.allocator;
    var out: std.ArrayList(u8) = .empty;
    defer out.deinit(a);
    try writeError(&out, a, 5, .MethodNotFound, "no such method: foo");
    try std.testing.expect(std.mem.indexOf(u8, out.items, "\"id\":5") != null);
    try std.testing.expect(std.mem.indexOf(u8, out.items, "MethodNotFound") != null);
    try std.testing.expect(std.mem.indexOf(u8, out.items, "no such method: foo") != null);
}

test "parseDispatchParams ok" {
    var arena = std.heap.ArenaAllocator.init(std.testing.allocator);
    defer arena.deinit();
    const v = try std.json.parseFromSliceLeaky(std.json.Value, arena.allocator(), "{\"input\":\"/model\"}", .{});
    const p = try parseDispatchParams(arena.allocator(), v);
    try std.testing.expectEqualStrings("/model", p.input);
}

test "parseDispatchParams missing input is InvalidParams" {
    var arena = std.heap.ArenaAllocator.init(std.testing.allocator);
    defer arena.deinit();
    const v = try std.json.parseFromSliceLeaky(std.json.Value, arena.allocator(), "{}", .{});
    try std.testing.expectError(error.InvalidParams, parseDispatchParams(arena.allocator(), v));
}

test "parseApplyModelChoiceParams ok" {
    var arena = std.heap.ArenaAllocator.init(std.testing.allocator);
    defer arena.deinit();
    const v = try std.json.parseFromSliceLeaky(
        std.json.Value,
        arena.allocator(),
        "{\"provider_index\":0,\"kind\":\"claude\",\"model\":\"claude-opus-4-7\",\"is_active\":true}",
        .{},
    );
    const p = try parseApplyModelChoiceParams(arena.allocator(), v);
    try std.testing.expectEqual(@as(u32, 0), p.provider_index);
    try std.testing.expectEqual(core.ProviderKind.claude, p.kind);
    try std.testing.expectEqualStrings("claude-opus-4-7", p.model);
    try std.testing.expect(p.is_active);
}

test "parseApplyModelChoiceParams unknown kind is InvalidParams" {
    var arena = std.heap.ArenaAllocator.init(std.testing.allocator);
    defer arena.deinit();
    const v = try std.json.parseFromSliceLeaky(
        std.json.Value,
        arena.allocator(),
        "{\"provider_index\":0,\"kind\":\"notreal\",\"model\":\"m\",\"is_active\":false}",
        .{},
    );
    try std.testing.expectError(error.InvalidParams, parseApplyModelChoiceParams(arena.allocator(), v));
}

test "parseSessionSendParams ok" {
    var arena = std.heap.ArenaAllocator.init(std.testing.allocator);
    defer arena.deinit();
    const v = try std.json.parseFromSliceLeaky(std.json.Value, arena.allocator(), "{\"text\":\"hi there\"}", .{});
    const p = try parseSessionSendParams(arena.allocator(), v);
    try std.testing.expectEqualStrings("hi there", p.text);
}

test "parseSessionSendParams missing text is InvalidParams" {
    var arena = std.heap.ArenaAllocator.init(std.testing.allocator);
    defer arena.deinit();
    const v = try std.json.parseFromSliceLeaky(std.json.Value, arena.allocator(), "{}", .{});
    try std.testing.expectError(error.InvalidParams, parseSessionSendParams(arena.allocator(), v));
}

test "writeEventTokenLine escapes special chars" {
    const a = std.testing.allocator;
    var out: std.ArrayList(u8) = .empty;
    defer out.deinit(a);
    try writeEventTokenLine(&out, a, 7, "He\"l\nlo");
    try std.testing.expect(std.mem.indexOf(u8, out.items, "\"id\":7") != null);
    try std.testing.expect(std.mem.indexOf(u8, out.items, "\"kind\":\"token\"") != null);
    // The text field must contain JSON-escaped contents.
    try std.testing.expect(std.mem.indexOf(u8, out.items, "He\\\"l\\nlo") != null);
}

test "writeEventErrLine round-trip" {
    const a = std.testing.allocator;
    var out: std.ArrayList(u8) = .empty;
    defer out.deinit(a);
    try writeEventErrLine(&out, a, 3, "server overloaded");
    try std.testing.expect(std.mem.indexOf(u8, out.items, "\"id\":3") != null);
    try std.testing.expect(std.mem.indexOf(u8, out.items, "\"kind\":\"err\"") != null);
    try std.testing.expect(std.mem.indexOf(u8, out.items, "server overloaded") != null);
}

test "writeEventToolCallLine round-trip" {
    const a = std.testing.allocator;
    var out: std.ArrayList(u8) = .empty;
    defer out.deinit(a);
    try writeEventToolCallLine(&out, a, 9, "tu_01", "read", "{\"path\":\"/tmp/x\"}");
    try std.testing.expect(std.mem.indexOf(u8, out.items, "\"id\":9") != null);
    try std.testing.expect(std.mem.indexOf(u8, out.items, "\"kind\":\"tool_call\"") != null);
    try std.testing.expect(std.mem.indexOf(u8, out.items, "tu_01") != null);
    try std.testing.expect(std.mem.indexOf(u8, out.items, "\"name\":\"read\"") != null);
}

test "writeEventToolResultLine round-trip with error flag" {
    const a = std.testing.allocator;
    var out: std.ArrayList(u8) = .empty;
    defer out.deinit(a);
    try writeEventToolResultLine(&out, a, 11, "tu_02", "boom", true);
    try std.testing.expect(std.mem.indexOf(u8, out.items, "\"id\":11") != null);
    try std.testing.expect(std.mem.indexOf(u8, out.items, "\"kind\":\"tool_result\"") != null);
    try std.testing.expect(std.mem.indexOf(u8, out.items, "tu_02") != null);
    try std.testing.expect(std.mem.indexOf(u8, out.items, "\"output\":\"boom\"") != null);
    try std.testing.expect(std.mem.indexOf(u8, out.items, "\"is_error\":true") != null);
}

test "writeEventContextLine includes label and body" {
    const a = std.testing.allocator;
    var out: std.ArrayList(u8) = .empty;
    defer out.deinit(a);
    try writeEventContextLine(&out, a, 4, "skill:research", "do thorough research");
    try std.testing.expect(std.mem.indexOf(u8, out.items, "\"id\":4") != null);
    try std.testing.expect(std.mem.indexOf(u8, out.items, "\"kind\":\"context\"") != null);
    try std.testing.expect(std.mem.indexOf(u8, out.items, "skill:research") != null);
    try std.testing.expect(std.mem.indexOf(u8, out.items, "do thorough research") != null);
}

test "writeSendConversationOkBody shape" {
    const a = std.testing.allocator;
    var out: std.ArrayList(u8) = .empty;
    defer out.deinit(a);
    try writeSendConversationOkBody(&out, a, .end_turn, 12, 34);
    try std.testing.expectEqualStrings(
        "{\"kind\":\"conversation\",\"ok\":true,\"stop_reason\":\"end_turn\",\"input_tokens\":12,\"output_tokens\":34}",
        out.items,
    );
}

test "writeSendConversationErrBody shape" {
    const a = std.testing.allocator;
    var out: std.ArrayList(u8) = .empty;
    defer out.deinit(a);
    try writeSendConversationErrBody(&out, a, "MissingCredential");
    try std.testing.expectEqualStrings(
        "{\"kind\":\"conversation\",\"ok\":false,\"reason\":\"MissingCredential\"}",
        out.items,
    );
}

test "writeSendCommandBody wraps a dispatch payload" {
    const a = std.testing.allocator;
    var out: std.ArrayList(u8) = .empty;
    defer out.deinit(a);
    try writeSendCommandBody(&out, a, .{ .message = "config dump" });
    try std.testing.expect(std.mem.startsWith(u8, out.items, "{\"kind\":\"command\",\"command\":"));
    try std.testing.expect(std.mem.indexOf(u8, out.items, "\"kind\":\"message\"") != null);
    try std.testing.expect(std.mem.indexOf(u8, out.items, "config dump") != null);
    try std.testing.expect(std.mem.endsWith(u8, out.items, "}"));
}

test "writeSendCommandBody with model_picker" {
    const a = std.testing.allocator;
    var out: std.ArrayList(u8) = .empty;
    defer out.deinit(a);
    const choices = [_]commands.ModelChoice{.{
        .provider_index = 0,
        .kind = .claude,
        .model = "claude-opus-4-7",
        .is_active = true,
    }};
    try writeSendCommandBody(&out, a, .{ .model_picker = .{
        .title = "Pick a model",
        .choices = &choices,
    } });
    try std.testing.expect(std.mem.indexOf(u8, out.items, "\"kind\":\"command\"") != null);
    try std.testing.expect(std.mem.indexOf(u8, out.items, "\"kind\":\"model_picker\"") != null);
    try std.testing.expect(std.mem.indexOf(u8, out.items, "claude-opus-4-7") != null);
}
