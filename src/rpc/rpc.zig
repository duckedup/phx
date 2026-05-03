const std = @import("std");
const core = @import("phoenix_core");

pub const protocol = @import("protocol.zig");
pub const server = @import("server.zig");
pub const client = @import("client.zig");
pub const Client = client.Client;

pub fn run(gpa: std.mem.Allocator, io: std.Io, config: *core.Config, home: ?[]const u8) !void {
    try server.run(
        gpa,
        io,
        config,
        home,
        std.posix.STDIN_FILENO,
        std.posix.STDOUT_FILENO,
    );
}

test {
    std.testing.refAllDecls(@This());
}
