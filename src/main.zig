const std = @import("std");
const core = @import("phoenix_core");
const tui = @import("tui/tui.zig");
const onboarding = @import("tui/onboarding.zig");
const rpc = @import("rpc/rpc.zig");

const usage =
    \\Usage: phoenix [--config <path>] <command>
    \\
    \\Commands:
    \\  tui    Launch the terminal UI (default)
    \\  rpc    Launch the headless RPC server
    \\
    \\Options:
    \\  --config <path>  Override config file (also reads $PHOENIX_CONFIG)
    \\  --help           Show this help
    \\  --version        Show version
    \\
;

pub fn main(init: std.process.Init) !void {
    var args: std.process.Args.Iterator = .init(init.minimal.args);
    _ = args.skip(); // argv[0]

    var explicit_config: ?[]const u8 = null;
    var cmd: ?[]const u8 = null;

    while (args.next()) |a| {
        if (std.mem.eql(u8, a, "--config")) {
            explicit_config = args.next() orelse {
                std.debug.print("--config requires a path\n{s}", .{usage});
                std.process.exit(2);
            };
        } else if (std.mem.eql(u8, a, "--help") or std.mem.eql(u8, a, "-h")) {
            std.debug.print("{s}", .{usage});
            return;
        } else if (std.mem.eql(u8, a, "--version")) {
            std.debug.print("phoenix 0.0.0\n", .{});
            return;
        } else if (cmd == null) {
            cmd = a;
        } else {
            std.debug.print("unexpected argument: {s}\n{s}", .{ a, usage });
            std.process.exit(2);
        }
    }

    // Check PHOENIX_CONFIG env var if no --config was given
    var owned_env: ?[]u8 = null;
    defer if (owned_env) |s| init.gpa.free(s);
    if (explicit_config == null) {
        if (std.c.getenv("PHOENIX_CONFIG")) |ptr| {
            const s = std.mem.span(ptr);
            if (s.len > 0) {
                owned_env = try init.gpa.dupe(u8, s);
                explicit_config = owned_env;
            }
        }
    }

    var cfg = try core.Config.load(init.gpa, init.io, .{ .explicit_path = explicit_config });
    defer cfg.deinit();

    const c = cmd orelse "tui";

    if (std.mem.eql(u8, c, "tui")) {
        if (!cfg.defaultProviderUsable(init.gpa)) {
            const home = resolveHome(init.gpa) orelse {
                std.debug.print("phoenix: cannot resolve $HOME for first-time setup\n", .{});
                std.process.exit(1);
            };
            defer init.gpa.free(home);

            switch (try onboarding.run(init, home)) {
                .cancelled => return,
                .completed => {
                    cfg.deinit();
                    cfg = try core.Config.load(init.gpa, init.io, .{ .explicit_path = explicit_config });
                },
            }
        }
        try tui.run(init, &cfg);
    } else if (std.mem.eql(u8, c, "rpc")) {
        try rpc.run(init.gpa, &cfg);
    } else {
        std.debug.print("unknown command: {s}\n{s}", .{ c, usage });
        std.process.exit(1);
    }
}

fn resolveHome(allocator: std.mem.Allocator) ?[]u8 {
    const ptr = std.c.getenv("HOME") orelse return null;
    const s = std.mem.span(ptr);
    if (s.len == 0) return null;
    return allocator.dupe(u8, s) catch null;
}

test {
    std.testing.refAllDecls(@This());
}
