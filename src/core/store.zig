const std = @import("std");

pub const TodoStatus = enum {
    open,
    in_progress,
    done,
    cancelled,
};

pub const Todo = struct {
    id: u64,
    title: []const u8,
    status: TodoStatus,
    assignee: ?u64,
};

pub const ToolLogEntry = struct {
    session_id: u64,
    parent_session_id: ?u64,
    tool_name: []const u8,
    args: []const u8,
    result: []const u8,
    timestamp_ms: i64,
};

pub const Store = struct {
    allocator: std.mem.Allocator,
    todos: std.ArrayList(Todo),
    tool_log: std.ArrayList(ToolLogEntry),
    next_todo_id: u64,

    pub fn init(allocator: std.mem.Allocator) Store {
        return .{
            .allocator = allocator,
            .todos = .empty,
            .tool_log = .empty,
            .next_todo_id = 1,
        };
    }

    pub fn deinit(self: *Store) void {
        self.todos.deinit(self.allocator);
        self.tool_log.deinit(self.allocator);
    }

    pub fn appendToolLog(self: *Store, entry: ToolLogEntry) !void {
        try self.tool_log.append(self.allocator, entry);
    }
};

test "store init" {
    const allocator = std.testing.allocator;
    var store = Store.init(allocator);
    defer store.deinit();

    try std.testing.expectEqual(@as(usize, 0), store.tool_log.items.len);
}
