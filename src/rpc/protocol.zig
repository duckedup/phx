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
    // CommandInfo's struct fields already match the wire shape, so wrapping
    // in `{ commands = items }` and letting Stringify do the work is enough.
    try appendValue(out, a, .{ .commands = items }, .{});
}

pub fn writeApplySessionChoiceResult(
    out: *std.ArrayList(u8),
    a: std.mem.Allocator,
    message: []const u8,
    message_count: u32,
    messages: []const core.Message,
) !void {
    // core.Message's tag (Role) already serializes as its tagName via
    // Stringify, but core.Message also has fields we don't want on the wire
    // (e.g. tool_calls). Project to a wire-only struct.
    const MessageWire = struct { role: []const u8, content: []const u8 };
    const wire = try a.alloc(MessageWire, messages.len);
    defer a.free(wire);
    for (messages, 0..) |m, i| wire[i] = .{ .role = @tagName(m.role), .content = m.content };

    try appendValue(out, a, .{
        .message = message,
        .message_count = message_count,
        .messages = wire,
    }, .{});
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
    try appendValue(out, a, .{
        .default_provider = default_provider,
        .sources_count = sources_count,
    }, .{});
}

pub fn writeDispatchResult(
    out: *std.ArrayList(u8),
    a: std.mem.Allocator,
    r: commands.Result,
) !void {
    // Wire shape is a tagged object: {"kind":"<variant>", ...payload}.
    // Each variant gets a small struct that interleaves `kind` with its
    // payload fields; Stringify handles encoding.
    switch (r) {
        .not_a_command => try appendValue(out, a, .{ .kind = "not_a_command" }, .{}),
        .message => |m| try appendValue(out, a, .{ .kind = "message", .text = m }, .{}),
        .cleared => |m| try appendValue(out, a, .{ .kind = "cleared", .text = m }, .{}),
        .compacted => |m| try appendValue(out, a, .{ .kind = "compacted", .text = m }, .{}),
        .err => |m| try appendValue(out, a, .{ .kind = "err", .text = m }, .{}),
        .model_picker => |p| try appendValue(out, a, .{
            .kind = "model_picker",
            .title = p.title,
            .choices = p.choices,
        }, .{}),
        .inject_context => |frag| try appendValue(out, a, .{
            .kind = "inject_context",
            .label = frag.label,
            .body = frag.body,
            .user_message = frag.user_message,
        }, .{}),
        // The server intercepts these markers before encoding, so they
        // shouldn't reach the wire in normal operation. Encode minimally for
        // diagnostic completeness — the TUI will treat the kind as unknown.
        .clear_session => try appendValue(out, a, .{ .kind = "clear_session" }, .{}),
        .compact_session => try appendValue(out, a, .{ .kind = "compact_session" }, .{}),
        .session_picker => |p| try appendValue(out, a, .{
            .kind = "session_picker",
            .title = p.title,
            .choices = p.choices,
        }, .{}),
        .models_page => |p| try appendValue(out, a, .{
            .kind = "models_page",
            .title = p.title,
            .entries = p.entries,
        }, .{}),
        .connect_wizard => try appendValue(out, a, .{ .kind = "connect_wizard" }, .{}),
        .theme_picker => |p| try appendValue(out, a, .{
            .kind = "theme_picker",
            .requested = p.requested,
        }, .{}),
    }
}

pub const AddModelParams = struct {
    kind: core.ProviderKind,
    model: []const u8,
    /// Inline secret. Empty for local providers.
    api_key: []const u8,
    /// Empty for cloud providers.
    base_url: []const u8,
    /// Null when the model uses default context window (cloud).
    context_window: ?u32,
};

pub fn parseAddModelParams(arena: std.mem.Allocator, v: std.json.Value) !AddModelParams {
    if (v != .object) return error.InvalidParams;
    const obj = v.object;
    const kn = obj.get("kind") orelse return error.InvalidParams;
    const md = obj.get("model") orelse return error.InvalidParams;
    if (kn != .string or md != .string) return error.InvalidParams;
    const kind = std.meta.stringToEnum(core.ProviderKind, kn.string) orelse return error.InvalidParams;

    const ak = obj.get("api_key") orelse std.json.Value{ .string = "" };
    const bu = obj.get("base_url") orelse std.json.Value{ .string = "" };
    if (ak != .string or bu != .string) return error.InvalidParams;

    const cw_v = obj.get("context_window") orelse std.json.Value{ .null = {} };
    const cw: ?u32 = switch (cw_v) {
        .integer => |i| if (i > 0) @intCast(i) else null,
        .null => null,
        else => return error.InvalidParams,
    };

    return .{
        .kind = kind,
        .model = try arena.dupe(u8, md.string),
        .api_key = try arena.dupe(u8, ak.string),
        .base_url = try arena.dupe(u8, bu.string),
        .context_window = cw,
    };
}

pub fn writeAddModelResult(
    out: *std.ArrayList(u8),
    a: std.mem.Allocator,
    message: []const u8,
    entries: []const commands.ModelEntry,
) !void {
    try appendValue(out, a, .{
        .message = message,
        .entries = entries,
    }, .{});
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
    try appendValue(out, a, .{
        .id = id,
        .event = .{ .kind = "token", .text = text },
    }, .{});
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
    try appendValue(out, a, .{
        .id = id,
        .event = .{ .kind = "err", .text = text },
    }, .{});
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
    try appendValue(out, a, .{
        .id = id,
        .event = .{
            .kind = "tool_call",
            .tool_id = tool_id,
            .name = name,
            .args = args,
        },
    }, .{});
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
    try appendValue(out, a, .{
        .id = id,
        .event = .{
            .kind = "tool_result",
            .tool_id = tool_id,
            .output = output,
            .is_error = is_error,
        },
    }, .{});
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
    try appendValue(out, a, .{
        .id = id,
        .event = .{ .kind = "context", .label = label, .body = body },
    }, .{});
}

/// Write the body of a session.send terminal result for a successful conversation
/// turn. Caller wraps with `writeSuccess` and appends '\n'.
pub fn writeSendConversationOkBody(
    out: *std.ArrayList(u8),
    a: std.mem.Allocator,
    stop_reason: core.StopReason,
    usage: core.Usage,
) !void {
    try appendValue(out, a, .{
        .kind = "conversation",
        .ok = true,
        .stop_reason = stopReasonName(stop_reason),
        .input_tokens = usage.input_tokens,
        .output_tokens = usage.output_tokens,
        .cache_creation_input_tokens = usage.cache_creation_input_tokens,
        .cache_read_input_tokens = usage.cache_read_input_tokens,
    }, .{});
}

/// Write the body of a session.send terminal result for a conversation that
/// emitted at least one event but ultimately failed (e.g. provider err mid-stream).
pub fn writeSendConversationErrBody(
    out: *std.ArrayList(u8),
    a: std.mem.Allocator,
    reason: []const u8,
) !void {
    try appendValue(out, a, .{
        .kind = "conversation",
        .ok = false,
        .reason = reason,
    }, .{});
}

/// Write the body of a session.send terminal result for a slash-command outcome
/// (no event lines were emitted, no AI call was made).
pub fn writeSendCommandBody(
    out: *std.ArrayList(u8),
    a: std.mem.Allocator,
    r: commands.Result,
) !void {
    // Build the inner dispatch result first, then splice it as a raw field
    // value alongside the `kind` discriminator. Two passes keeps the encoder
    // straightforward; the inner buffer is short-lived.
    var inner: std.ArrayList(u8) = .empty;
    defer inner.deinit(a);
    try writeDispatchResult(&inner, a, r);

    var aw = beginAllocating(a, out);
    defer endAllocating(out, &aw);
    var s: std.json.Stringify = .{ .writer = &aw.writer, .options = .{} };
    try s.beginObject();
    try s.objectField("kind");
    try s.write("command");
    try s.objectField("command");
    try s.beginWriteRaw();
    aw.writer.writeAll(inner.items) catch return error.OutOfMemory;
    s.endWriteRaw();
    try s.endObject();
}

pub fn writeApplyModelChoiceResult(
    out: *std.ArrayList(u8),
    a: std.mem.Allocator,
    message: []const u8,
    default_provider: struct { kind: core.ProviderKind, model: []const u8 },
) !void {
    try appendValue(out, a, .{
        .message = message,
        .default_provider = default_provider,
    }, .{});
}

pub fn writeSuccess(
    out: *std.ArrayList(u8),
    a: std.mem.Allocator,
    id: i64,
    result_body: []const u8,
) !void {
    var aw = beginAllocating(a, out);
    defer endAllocating(out, &aw);
    var s: std.json.Stringify = .{ .writer = &aw.writer, .options = .{} };
    try s.beginObject();
    try s.objectField("id");
    try s.write(id);
    try s.objectField("result");
    try s.beginWriteRaw();
    aw.writer.writeAll(result_body) catch return error.OutOfMemory;
    s.endWriteRaw();
    try s.endObject();
}

pub fn writeError(
    out: *std.ArrayList(u8),
    a: std.mem.Allocator,
    id: i64,
    code: ErrorCode,
    message: []const u8,
) !void {
    try appendValue(out, a, .{
        .id = id,
        .@"error" = .{ .code = code.name(), .message = message },
    }, .{});
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

/// Wrap `out` so a `std.json.Stringify` call can append directly without an
/// intermediate buffer. Pair with `endAllocating` (deferred) to hand the
/// memory back to the ArrayList — including any growth Stringify did.
fn beginAllocating(a: std.mem.Allocator, out: *std.ArrayList(u8)) std.Io.Writer.Allocating {
    return std.Io.Writer.Allocating.fromArrayList(a, out);
}

fn endAllocating(out: *std.ArrayList(u8), aw: *std.Io.Writer.Allocating) void {
    out.* = aw.toArrayList();
}

/// Convenience: serialize `v` and append it to `out`. Replaces hand-rolled
/// `appendSlice("{\"x\":")` + `writeJsonString` + ... patterns. Stringify
/// handles enums (tag name), optionals, slices, and nested structs.
fn appendValue(out: *std.ArrayList(u8), a: std.mem.Allocator, v: anytype, options: std.json.Stringify.Options) !void {
    var aw = beginAllocating(a, out);
    defer endAllocating(out, &aw);
    std.json.Stringify.value(v, options, &aw.writer) catch return error.OutOfMemory;
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
    try writeSendConversationOkBody(&out, a, .end_turn, .{ .input_tokens = 12, .output_tokens = 34, .cache_creation_input_tokens = 5, .cache_read_input_tokens = 8 });
    try std.testing.expectEqualStrings(
        "{\"kind\":\"conversation\",\"ok\":true,\"stop_reason\":\"end_turn\",\"input_tokens\":12,\"output_tokens\":34,\"cache_creation_input_tokens\":5,\"cache_read_input_tokens\":8}",
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

test "writeSendConversationOkBody includes cache fields" {
    const a = std.testing.allocator;
    var out: std.ArrayList(u8) = .empty;
    defer out.deinit(a);
    try writeSendConversationOkBody(&out, a, .end_turn, .{
        .input_tokens = 100,
        .output_tokens = 20,
        .cache_creation_input_tokens = 0,
        .cache_read_input_tokens = 0,
    });
    try std.testing.expect(std.mem.indexOf(u8, out.items, "\"cache_creation_input_tokens\":0") != null);
    try std.testing.expect(std.mem.indexOf(u8, out.items, "\"cache_read_input_tokens\":0") != null);
}
