const std = @import("std");
const core = @import("phoenix_core");

fn invoke(self: *const core.Tool, args: []const u8, allocator: std.mem.Allocator) anyerror!core.tool.ToolResult {
    _ = self;
    const file = std.fs.cwd().openFile(args, .{}) catch |err| {
        const msg = try std.fmt.allocPrint(allocator, "error opening file: {}", .{err});
        return .{ .output = msg };
    };
    defer file.close();

    const content = file.readToEndAlloc(allocator, 512 * 1024) catch |err| {
        const msg = try std.fmt.allocPrint(allocator, "error reading file: {}", .{err});
        return .{ .output = msg };
    };

    return .{ .output = content };
}

pub const read_file_tool: core.Tool = .{
    .name = "read_file",
    .description = "Read the contents of a file",
    .schema =
    \\{"type":"object","properties":{"path":{"type":"string"}},"required":["path"]}
    ,
    .max_output_bytes = 512 * 1024,
    .invokeFn = invoke,
};
