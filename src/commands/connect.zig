const std = @import("std");
const dispatcher = @import("dispatcher.zig");

pub fn handle(
    ctx: dispatcher.DispatchCtx,
    args: []const u8,
    out: std.mem.Allocator,
) !dispatcher.Result {
    _ = args;
    _ = out;
    _ = ctx;
    return .connect_wizard;
}