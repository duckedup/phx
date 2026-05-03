const std = @import("std");
const core = @import("phoenix_core");
const tui = @import("tui/tui.zig");
const onboarding = @import("tui/onboarding.zig");
const rpc = @import("rpc");

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
        const home_for_tui: ?[]u8 = resolveHome(init.gpa);
        defer if (home_for_tui) |h| init.gpa.free(h);

        if (!cfg.activeProviderUsable(init.gpa)) {
            const home = home_for_tui orelse {
                std.debug.print("phoenix: cannot resolve $HOME for first-time setup\n", .{});
                std.process.exit(1);
            };

            switch (try onboarding.run(init, home)) {
                .cancelled => return,
                .completed => {
                    cfg.deinit();
                    cfg = try core.Config.load(init.gpa, init.io, .{ .explicit_path = explicit_config });
                },
            }
        }
        // cfg will be freed by the deferred deinit below; the server loads its own copy.
        // Use argv[0] as the path to the current executable.
        const argv0: []const u8 = if (init.minimal.args.vector.len > 0)
            std.mem.sliceTo(init.minimal.args.vector[0], 0)
        else
            "phoenix";

        var client = try rpc.Client.spawn(init.gpa, init.io, argv0, explicit_config);
        defer client.deinit();

        try tui.run(init, &client);
    } else if (std.mem.eql(u8, c, "rpc")) {
        const home_opt: ?[]u8 = resolveHome(init.gpa);
        defer if (home_opt) |h| init.gpa.free(h);
        try rpc.run(init.gpa, init.io, &cfg, home_opt);
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
