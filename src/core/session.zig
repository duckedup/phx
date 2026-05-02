const std = @import("std");
const Message = @import("message.zig").Message;
const Role = @import("message.zig").Role;
const ToolCall = @import("message.zig").ToolCall;
const ToolResult = @import("message.zig").ToolResult;

pub const SessionState = enum {
    idle,
    running,
    done,
    err,
    cancelled,
};

pub const Session = struct {
    id: u64,
    messages: std.ArrayList(Message),
    state: SessionState,
    token_count: usize,
    allocator: std.mem.Allocator,

    pub fn init(allocator: std.mem.Allocator, id: u64) Session {
        return .{
            .id = id,
            .messages = .empty,
            .state = .idle,
            .token_count = 0,
            .allocator = allocator,
        };
    }

    pub fn deinit(self: *Session) void {
        for (self.messages.items) |msg| {
            if (msg.content.len > 0) self.allocator.free(msg.content);
            if (msg.tool_call) |tc| {
                self.allocator.free(tc.id);
                self.allocator.free(tc.name);
                self.allocator.free(tc.args_json);
            }
            if (msg.tool_result) |tr| {
                self.allocator.free(tr.id);
                self.allocator.free(tr.output);
            }
        }
        self.messages.deinit(self.allocator);
    }

    pub fn addMessage(self: *Session, role: Role, content: []const u8) !void {
        const owned = try self.allocator.dupe(u8, content);
        try self.messages.append(self.allocator, .{
            .role = role,
            .content = owned,
        });
    }

    pub fn addToolCall(self: *Session, tc: ToolCall) !void {
        const id = try self.allocator.dupe(u8, tc.id);
        errdefer self.allocator.free(id);
        const name = try self.allocator.dupe(u8, tc.name);
        errdefer self.allocator.free(name);
        const args = try self.allocator.dupe(u8, tc.args_json);
        errdefer self.allocator.free(args);
        try self.messages.append(self.allocator, .{
            .role = .tool_call,
            .content = "",
            .tool_call = .{ .id = id, .name = name, .args_json = args },
        });
    }

    pub fn addToolResult(self: *Session, tr: ToolResult) !void {
        const id = try self.allocator.dupe(u8, tr.id);
        errdefer self.allocator.free(id);
        const output = try self.allocator.dupe(u8, tr.output);
        errdefer self.allocator.free(output);
        try self.messages.append(self.allocator, .{
            .role = .tool_result,
            .content = "",
            .tool_result = .{ .id = id, .output = output, .is_error = tr.is_error },
        });
    }
};

// ---- Tests ----

test "session create and add message" {
    const allocator = std.testing.allocator;
    var session = Session.init(allocator, 1);
    defer session.deinit();

    try session.addMessage(.user, "hello");
    try std.testing.expectEqual(@as(usize, 1), session.messages.items.len);
    try std.testing.expectEqualStrings("hello", session.messages.items[0].content);
}

test "addToolCall round-trip, no leak" {
    const allocator = std.testing.allocator;
    var session = Session.init(allocator, 2);
    defer session.deinit();

    try session.addToolCall(.{
        .id = "tc_abc",
        .name = "read_file",
        .args_json = "{\"path\":\"/tmp/x\"}",
    });

    try std.testing.expectEqual(@as(usize, 1), session.messages.items.len);
    const msg = session.messages.items[0];
    try std.testing.expect(msg.role == .tool_call);
    try std.testing.expectEqualStrings("", msg.content);
    try std.testing.expect(msg.tool_call != null);
    try std.testing.expectEqualStrings("tc_abc", msg.tool_call.?.id);
    try std.testing.expectEqualStrings("read_file", msg.tool_call.?.name);
    try std.testing.expectEqualStrings("{\"path\":\"/tmp/x\"}", msg.tool_call.?.args_json);
}

test "addToolResult round-trip, no leak" {
    const allocator = std.testing.allocator;
    var session = Session.init(allocator, 3);
    defer session.deinit();

    try session.addToolResult(.{
        .id = "tc_abc",
        .output = "file contents here",
        .is_error = false,
    });

    try std.testing.expectEqual(@as(usize, 1), session.messages.items.len);
    const msg = session.messages.items[0];
    try std.testing.expect(msg.role == .tool_result);
    try std.testing.expectEqualStrings("", msg.content);
    try std.testing.expect(msg.tool_result != null);
    try std.testing.expectEqualStrings("tc_abc", msg.tool_result.?.id);
    try std.testing.expectEqualStrings("file contents here", msg.tool_result.?.output);
    try std.testing.expect(!msg.tool_result.?.is_error);
}

test "addToolResult with is_error=true, no leak" {
    const allocator = std.testing.allocator;
    var session = Session.init(allocator, 4);
    defer session.deinit();

    try session.addToolResult(.{
        .id = "tc_err",
        .output = "tool failed",
        .is_error = true,
    });

    const msg = session.messages.items[0];
    try std.testing.expect(msg.tool_result.?.is_error);
}

test "mixed messages deinit no leak" {
    const allocator = std.testing.allocator;
    var session = Session.init(allocator, 5);
    defer session.deinit();

    try session.addMessage(.user, "What files are in /tmp?");
    try session.addToolCall(.{ .id = "tc_1", .name = "list_dir", .args_json = "{\"path\":\"/tmp\"}" });
    try session.addToolResult(.{ .id = "tc_1", .output = "a.txt\nb.txt", .is_error = false });
    try session.addMessage(.assistant, "I found two files.");

    try std.testing.expectEqual(@as(usize, 4), session.messages.items.len);
}
