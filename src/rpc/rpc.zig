const std = @import("std");
const core = @import("phoenix_core");

pub const protocol = @import("protocol.zig");
pub const server = @import("server.zig");
pub const client = @import("client.zig");
pub const Client = client.Client;

pub fn run(gpa: std.mem.Allocator, io: std.Io, config: *core.Config, home: ?[]const u8) !void {
    // OTel is strictly optional and never breaks the agent loop. When
    // `config.otel.endpoint` is empty (the default), `Otel.init` returns a
    // disabled instance whose every method is a no-op. Even if a user
    // configured an endpoint and allocation failed, init falls back to a
    // disabled instance and logs a warning — `try` is never used here on
    // purpose.
    var otel = core.Otel.init(gpa, io, config.otel);
    defer otel.deinit();

    if (otel.enabled) {
        std.log.info("otel: exporting to {s} (service.name={s})", .{
            config.otel.endpoint,
            config.otel.service_name,
        });
    }

    try server.run(
        gpa,
        io,
        config,
        home,
        std.posix.STDIN_FILENO,
        std.posix.STDOUT_FILENO,
        &otel,
    );
}

test {
    std.testing.refAllDecls(@This());
}
