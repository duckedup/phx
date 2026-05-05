const std = @import("std");
const dispatcher = @import("dispatcher.zig");

pub fn handle(
    ctx: dispatcher.DispatchCtx,
    args: []const u8,
    out_arena: std.mem.Allocator,
) anyerror!dispatcher.Result {
    _ = ctx;
    if (args.len > 0) {
        return .{ .theme_picker = .{ .requested = try out_arena.dupe(u8, args) } };
    }
    return .{ .theme_picker = .{ .requested = null } };
}
