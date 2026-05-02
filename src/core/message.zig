pub const Role = enum {
    system,
    user,
    assistant,
    tool_call,
    tool_result,
};

pub const ToolCall = struct {
    id: []const u8,
    name: []const u8,
    args_json: []const u8,
};

pub const ToolResult = struct {
    id: []const u8,
    output: []const u8,
    is_error: bool = false,
};

pub const Message = struct {
    role: Role,
    /// Plain text content. May be empty for tool_call / tool_result roles.
    content: []const u8,
    /// Populated when role == .tool_call (assistant requested a tool).
    tool_call: ?ToolCall = null,
    /// Populated when role == .tool_result (caller is feeding a result back).
    tool_result: ?ToolResult = null,
};

// ---- Tests ----

const std = @import("std");

test "message tool_call and tool_result fields default to null" {
    const msg = Message{
        .role = .user,
        .content = "hello",
    };
    try std.testing.expect(msg.tool_call == null);
    try std.testing.expect(msg.tool_result == null);
    try std.testing.expectEqualStrings("hello", msg.content);
}

test "message with tool_call populated" {
    const msg = Message{
        .role = .tool_call,
        .content = "",
        .tool_call = .{
            .id = "tc_1",
            .name = "my_tool",
            .args_json = "{\"x\": 1}",
        },
    };
    try std.testing.expect(msg.tool_call != null);
    try std.testing.expectEqualStrings("tc_1", msg.tool_call.?.id);
    try std.testing.expectEqualStrings("my_tool", msg.tool_call.?.name);
}

test "message with tool_result populated" {
    const msg = Message{
        .role = .tool_result,
        .content = "",
        .tool_result = .{
            .id = "tc_1",
            .output = "42",
            .is_error = false,
        },
    };
    try std.testing.expect(msg.tool_result != null);
    try std.testing.expectEqualStrings("tc_1", msg.tool_result.?.id);
    try std.testing.expectEqualStrings("42", msg.tool_result.?.output);
    try std.testing.expect(!msg.tool_result.?.is_error);
}
