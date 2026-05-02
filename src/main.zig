const std = @import("std");
const tui = @import("tui/tui.zig");
const rpc = @import("rpc/rpc.zig");

const usage =
    \\Usage: phoenix <command>
    \\
    \\Commands:
    \\  tui    Launch the terminal UI (default)
    \\  rpc    Launch the headless RPC server
    \\
    \\Options:
    \\  --help    Show this help
    \\  --version Show version
    \\
;

pub fn main(init: std.process.Init) !void {
    var args: std.process.Args.Iterator = .init(init.minimal.args);
    _ = args.skip(); // argv[0]
    const cmd = args.next();

    if (cmd == null) {
        try tui.run(init);
        return;
    }

    const c = cmd.?;

    if (std.mem.eql(u8, c, "tui")) {
        try tui.run(init);
    } else if (std.mem.eql(u8, c, "rpc")) {
        try rpc.run(init.gpa);
    } else if (std.mem.eql(u8, c, "--help") or std.mem.eql(u8, c, "-h")) {
        std.debug.print("{s}", .{usage});
    } else if (std.mem.eql(u8, c, "--version")) {
        std.debug.print("phoenix 0.0.0\n", .{});
    } else {
        std.debug.print("unknown command: {s}\n{s}", .{ c, usage });
        std.process.exit(1);
    }
}

test {
    std.testing.refAllDecls(@This());
}
