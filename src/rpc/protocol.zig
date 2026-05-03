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

pub const ApplyModelChoiceParams = struct {
    provider_name: []const u8,
    kind: core.ProviderKind,
    model: []const u8,
    is_active: bool,
};

pub fn parseApplyModelChoiceParams(arena: std.mem.Allocator, v: std.json.Value) !ApplyModelChoiceParams {
    if (v != .object) return error.InvalidParams;
    const obj = v.object;
    const pn = obj.get("provider_name") orelse return error.InvalidParams;
    const kn = obj.get("kind") orelse return error.InvalidParams;
    const md = obj.get("model") orelse return error.InvalidParams;
    const ia = obj.get("is_active") orelse return error.InvalidParams;
    if (pn != .string or kn != .string or md != .string or ia != .bool) return error.InvalidParams;
    const kind = std.meta.stringToEnum(core.ProviderKind, kn.string) orelse return error.InvalidParams;
    return .{
        .provider_name = try arena.dupe(u8, pn.string),
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
                try out.appendSlice(a, "{\"provider_name\":");
                try writeJsonString(out, a, c.provider_name);
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
    }
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
        .provider_name = "default",
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
        "{\"provider_name\":\"default\",\"kind\":\"claude\",\"model\":\"claude-opus-4-7\",\"is_active\":true}",
        .{},
    );
    const p = try parseApplyModelChoiceParams(arena.allocator(), v);
    try std.testing.expectEqualStrings("default", p.provider_name);
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
        "{\"provider_name\":\"default\",\"kind\":\"notreal\",\"model\":\"m\",\"is_active\":false}",
        .{},
    );
    try std.testing.expectError(error.InvalidParams, parseApplyModelChoiceParams(arena.allocator(), v));
}
