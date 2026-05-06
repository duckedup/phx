const std = @import("std");
const core = @import("phoenix_core");
const dispatcher = @import("dispatcher.zig");

pub fn handle(
    ctx: dispatcher.DispatchCtx,
    args: []const u8,
    out_arena: std.mem.Allocator,
) anyerror!dispatcher.Result {
    const ap = ctx.config.activeProviderMut() orelse return .{ .err = try out_arena.dupe(u8, "no active provider") };

    if (ap.kind != .claude) {
        return .{ .err = try std.fmt.allocPrint(out_arena, "cache TTL is only supported for claude (current: {s})", .{@tagName(ap.kind)}) };
    }

    const trimmed = std.mem.trim(u8, args, " \t");

    if (trimmed.len == 0) {
        const current = ap.cache_ttl orelse "5m";
        return .{ .message = try std.fmt.allocPrint(out_arena, "cache TTL: {s} (use /cache 5m or /cache 1h)", .{current}) };
    }

    if (!std.mem.eql(u8, trimmed, "5m") and !std.mem.eql(u8, trimmed, "off") and !std.mem.eql(u8, trimmed, "1h")) {
        return .{ .err = try std.fmt.allocPrint(out_arena, "invalid TTL: {s} (use 5m or 1h)", .{trimmed}) };
    }

    const home = ctx.home orelse return .{ .err = try out_arena.dupe(u8, "HOME not available") };

    if (std.mem.eql(u8, trimmed, "5m") or std.mem.eql(u8, trimmed, "off")) {
        ap.cache_ttl = null;
        try core.config_writer.writeUserConfig(ctx.io, ctx.gpa, home, ctx.config.providers);
        return .{ .message = try out_arena.dupe(u8, "cache TTL set to 5m (default)") };
    }

    ap.cache_ttl = try ctx.gpa.dupe(u8, "1h");
    try core.config_writer.writeUserConfig(ctx.io, ctx.gpa, home, ctx.config.providers);
    return .{ .message = try out_arena.dupe(u8, "cache TTL set to 1h (2x base input price)") };
}
