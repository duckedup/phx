const std = @import("std");

pub const ToolError = error{
    InvalidArgs,
    Timeout,
    ExecutionFailed,
};

pub const ToolResult = struct {
    output: []const u8,
    truncated: bool = false,
};

pub const Tool = struct {
    name: []const u8,
    description: []const u8,
    schema: []const u8,
    max_output_bytes: usize = 512 * 1024,
    invokeFn: *const fn (self: *const Tool, args: []const u8, allocator: std.mem.Allocator) anyerror!ToolResult,

    pub fn invoke(self: *const Tool, args: []const u8, allocator: std.mem.Allocator) !ToolResult {
        return self.invokeFn(self, args, allocator);
    }
};

pub const ToolRegistry = struct {
    tools: std.StringHashMap(*const Tool),

    pub fn init(allocator: std.mem.Allocator) ToolRegistry {
        return .{
            .tools = std.StringHashMap(*const Tool).init(allocator),
        };
    }

    pub fn deinit(self: *ToolRegistry) void {
        self.tools.deinit();
    }

    pub fn register(self: *ToolRegistry, tool: *const Tool) !void {
        try self.tools.put(tool.name, tool);
    }

    pub fn get(self: *const ToolRegistry, name: []const u8) ?*const Tool {
        return self.tools.get(name);
    }

    pub fn count(self: *const ToolRegistry) u32 {
        return self.tools.count();
    }
};

test "tool registry" {
    const allocator = std.testing.allocator;
    var registry = ToolRegistry.init(allocator);
    defer registry.deinit();

    try std.testing.expectEqual(@as(u32, 0), registry.count());
}
