/// Handlers for /clear, /compact, /resume.
///
/// /clear and /compact return "marker" Result variants that the RPC server
/// intercepts and acts on (it owns the in-memory session). /resume scans the
/// session_store for the current project and returns a SessionPicker — the
/// TUI then calls `command.applySessionChoice` over RPC to actually load.
const std = @import("std");
const core = @import("phoenix_core");

const dispatcher = @import("dispatcher.zig");

pub fn handleClear(
    ctx: dispatcher.DispatchCtx,
    args: []const u8,
    out_arena: std.mem.Allocator,
) anyerror!dispatcher.Result {
    _ = ctx;
    if (args.len > 0) {
        return .{ .err = try out_arena.dupe(u8, "/clear takes no arguments") };
    }
    return .clear_session;
}

pub fn handleCompact(
    ctx: dispatcher.DispatchCtx,
    args: []const u8,
    out_arena: std.mem.Allocator,
) anyerror!dispatcher.Result {
    _ = ctx;
    if (args.len > 0) {
        return .{ .err = try out_arena.dupe(u8, "/compact takes no arguments") };
    }
    return .compact_session;
}

pub fn handleResume(
    ctx: dispatcher.DispatchCtx,
    args: []const u8,
    out_arena: std.mem.Allocator,
) anyerror!dispatcher.Result {
    if (args.len > 0) {
        return .{ .err = try out_arena.dupe(u8, "/resume takes no arguments") };
    }
    const home = ctx.home orelse {
        return .{ .err = try out_arena.dupe(u8, "/resume: HOME is not set") };
    };
    const project = ctx.project orelse {
        return .{ .err = try out_arena.dupe(u8, "/resume: project context unavailable") };
    };

    // session_store.list returns owned strings; copy into the dispatch arena
    // so callers can destroy the underlying allocations independently.
    const entries = core.session_store.list(ctx.io, ctx.gpa, home, project) catch |err| {
        const msg = try std.fmt.allocPrint(out_arena, "/resume: cannot list sessions ({s})", .{@errorName(err)});
        return .{ .err = msg };
    };
    defer core.session_store.freeList(ctx.gpa, entries);

    if (entries.len == 0) {
        return .{ .message = try out_arena.dupe(u8, "No saved sessions for this project.") };
    }

    var choices: std.ArrayList(dispatcher.SessionChoice) = .empty;
    for (entries) |e| {
        try choices.append(out_arena, .{
            .id = try out_arena.dupe(u8, e.id),
            .name = try out_arena.dupe(u8, e.name),
            .updated_at = e.updated_at,
            .message_count = e.message_count,
        });
    }

    const title = try out_arena.dupe(u8, "Resume which session? (Enter to confirm, Esc to cancel)");
    return .{ .session_picker = .{
        .title = title,
        .choices = try choices.toOwnedSlice(out_arena),
    } };
}

// ---- Tests ----

const testing = std.testing;

test "handleClear: rejects arguments" {
    const a = testing.allocator;
    var cfg = try core.Config.load(a, testing.io, .{ .home = null });
    defer cfg.deinit();

    var arena = std.heap.ArenaAllocator.init(a);
    defer arena.deinit();

    const ctx = dispatcher.DispatchCtx{
        .io = testing.io,
        .gpa = a,
        .home = null,
        .config = &cfg,
    };

    const r = try handleClear(ctx, "extra", arena.allocator());
    switch (r) {
        .err => |m| try testing.expect(std.mem.indexOf(u8, m, "no arguments") != null),
        else => return error.TestUnexpectedResult,
    }
}

test "handleClear: bare returns marker" {
    const a = testing.allocator;
    var cfg = try core.Config.load(a, testing.io, .{ .home = null });
    defer cfg.deinit();

    var arena = std.heap.ArenaAllocator.init(a);
    defer arena.deinit();

    const ctx = dispatcher.DispatchCtx{
        .io = testing.io,
        .gpa = a,
        .home = null,
        .config = &cfg,
    };

    const r = try handleClear(ctx, "", arena.allocator());
    try testing.expect(r == .clear_session);
}

test "handleResume: missing home returns err" {
    const a = testing.allocator;
    var cfg = try core.Config.load(a, testing.io, .{ .home = null });
    defer cfg.deinit();

    var arena = std.heap.ArenaAllocator.init(a);
    defer arena.deinit();

    const ctx = dispatcher.DispatchCtx{
        .io = testing.io,
        .gpa = a,
        .home = null,
        .config = &cfg,
    };
    const r = try handleResume(ctx, "", arena.allocator());
    switch (r) {
        .err => |m| try testing.expect(std.mem.indexOf(u8, m, "HOME") != null),
        else => return error.TestUnexpectedResult,
    }
}

test "handleResume: empty project returns message" {
    const a = testing.allocator;
    var cfg = try core.Config.load(a, testing.io, .{ .home = null });
    defer cfg.deinit();

    var tmp = testing.tmpDir(.{});
    defer tmp.cleanup();
    var buf: [std.Io.Dir.max_path_bytes]u8 = undefined;
    const ptr = std.c.getcwd(&buf, buf.len) orelse return error.CwdError;
    const cwd_path = std.mem.sliceTo(ptr, 0);
    const home = try std.fs.path.join(a, &.{ cwd_path, ".zig-cache", "tmp", &tmp.sub_path, "home" });
    defer a.free(home);
    try std.Io.Dir.cwd().createDirPath(testing.io, home);

    var arena = std.heap.ArenaAllocator.init(a);
    defer arena.deinit();

    const ctx = dispatcher.DispatchCtx{
        .io = testing.io,
        .gpa = a,
        .home = home,
        .config = &cfg,
        .project = "phoenix",
    };
    const r = try handleResume(ctx, "", arena.allocator());
    switch (r) {
        .message => |m| try testing.expect(std.mem.indexOf(u8, m, "No saved sessions") != null),
        else => return error.TestUnexpectedResult,
    }
}
