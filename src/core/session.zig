const std = @import("std");
const Message = @import("message.zig").Message;
const Role = @import("message.zig").Role;

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
            self.allocator.free(msg.content);
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
};

test "session create and add message" {
    const allocator = std.testing.allocator;
    var session = Session.init(allocator, 1);
    defer session.deinit();

    try session.addMessage(.user, "hello");
    try std.testing.expectEqual(@as(usize, 1), session.messages.items.len);
    try std.testing.expectEqualStrings("hello", session.messages.items[0].content);
}
