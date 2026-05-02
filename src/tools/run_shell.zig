const std = @import("std");
const core = @import("phoenix_core");

fn invoke(self: *const core.Tool, args: []const u8, allocator: std.mem.Allocator) anyerror!core.tool.ToolResult {
    _ = self;

    var child = std.process.Child.init(
        &.{ "/bin/sh", "-c", args },
        allocator,
    );
    child.stdout_behavior = .Pipe;
    child.stderr_behavior = .Pipe;

    try child.spawn();
    const stdout = try child.stdout.?.reader().readAllAlloc(allocator, 512 * 1024);
    const stderr_out = try child.stderr.?.reader().readAllAlloc(allocator, 512 * 1024);
    const term = try child.wait();

    if (term.Exited != 0) {
        const msg = try std.fmt.allocPrint(allocator, "exit code {d}\n{s}", .{ term.Exited, stderr_out });
        return .{ .output = msg };
    }

    return .{ .output = stdout };
}

pub const run_shell_tool: core.Tool = .{
    .name = "run_shell",
    .description = "Run a shell command",
    .schema =
    \\{"type":"object","properties":{"command":{"type":"string"}},"required":["command"]}
    ,
    .max_output_bytes = 512 * 1024,
    .invokeFn = invoke,
};
