const std = @import("std");
const core = @import("phoenix_core");

pub fn run(allocator: std.mem.Allocator, config: *const core.Config) !void {
    _ = allocator;
    std.debug.print("phoenix rpc: not yet implemented (loaded {d} config sources)\n", .{config.sources.len});
}
