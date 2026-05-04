/// First-run onboarding wizard. Wraps the shared `add_model_wizard` with a
/// vaxis event loop and the disk-write step so a fresh user can land in a
/// usable phoenix.json after their first launch.
const std = @import("std");
const vaxis = @import("vaxis");
const core = @import("phoenix_core");
const add_model = @import("add_model_wizard.zig");

pub const Outcome = enum { completed, cancelled };

const VxEvent = union(enum) {
    key_press: vaxis.Key,
    winsize: vaxis.Winsize,
    focus_in,
    focus_out,
    paste_start,
    paste_end,
    paste: []const u8,
};

pub fn run(init: std.process.Init, home: []const u8) !Outcome {
    const io = init.io;
    const allocator = init.gpa;

    var wizard = add_model.Wizard.init(allocator);
    defer wizard.deinit();

    // Arena for transient strings built during a paint cycle (titles, hints).
    // Cells in vx.screen.buf reference these byte ranges, so they must outlive
    // vx.render(). Reset at the start of each paint.
    var paint_arena = std.heap.ArenaAllocator.init(allocator);
    defer paint_arena.deinit();

    var buffer: [4096]u8 = undefined;
    var tty = try vaxis.Tty.init(io, &buffer);
    defer tty.deinit();
    const writer = tty.writer();

    var vx = try vaxis.init(io, allocator, init.environ_map, .{});
    defer vx.deinit(allocator, writer);

    var loop: vaxis.Loop(VxEvent) = .init(io, &tty, &vx);
    try loop.start();
    defer loop.stop();

    try vx.enterAltScreen(writer);
    try vx.queryTerminal(writer, .fromSeconds(1));
    try writer.flush();

    try paint(&vx, writer, &paint_arena, &wizard);

    while (true) {
        const event = try loop.nextEvent();

        switch (event) {
            .winsize => |ws| try vx.resize(allocator, writer, ws),
            .focus_in, .focus_out, .paste_start, .paste_end => {},
            .paste => |s| try wizard.handlePaste(s),
            .key_press => |key| {
                const outcome = try wizard.handleKey(key);
                switch (outcome) {
                    .in_progress => {},
                    .cancelled => return .cancelled,
                    .completed => |result| {
                        // The first model written by onboarding is the active
                        // one — no other providers exist yet.
                        var arena = std.heap.ArenaAllocator.init(allocator);
                        defer arena.deinit();
                        var profile = try result.toProfile(arena.allocator());
                        profile.active = true;
                        const profiles = [_]core.ProviderProfile{profile};
                        try core.config_writer.writeUserConfig(io, allocator, home, &profiles);
                        return .completed;
                    },
                }
            },
        }

        try paint(&vx, writer, &paint_arena, &wizard);
    }
}

fn paint(
    vx: *vaxis.Vaxis,
    writer: anytype,
    paint_arena: *std.heap.ArenaAllocator,
    wizard: *const add_model.Wizard,
) !void {
    _ = paint_arena.reset(.retain_capacity);

    // Force a full repaint each frame. The diff renderer's "default cell"
    // fastpath was leaving stale glyphs from prior steps visible; we'd rather
    // pay the redraw cost than chase the diff bug.
    vx.queueRefresh();

    const win = vx.window();
    win.clear();
    vx.screen.cursor_vis = false;

    wizard.paint(win, paint_arena.allocator());

    try vx.render(writer);
    try writer.flush();
}
