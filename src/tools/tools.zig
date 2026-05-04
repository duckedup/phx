const std = @import("std");
const core = @import("phoenix_core");

const read_mod = @import("read.zig");
const write_mod = @import("write.zig");
const edit_mod = @import("edit.zig");
const bash_mod = @import("bash.zig");

pub const read_tool = read_mod.read_tool;
pub const write_tool = write_mod.write_tool;
pub const edit_tool = edit_mod.edit_tool;
pub const bash_tool = bash_mod.bash_tool;

pub const all: []const *const core.Tool = &.{
    &read_tool,
    &write_tool,
    &edit_tool,
    &bash_tool,
};

/// Look up a tool by name. Returns null if no built-in tool matches.
pub fn lookup(name: []const u8) ?*const core.Tool {
    for (all) |t| {
        if (std.mem.eql(u8, t.name, name)) return t;
    }
    return null;
}

/// Build a registry containing every requested tool. Unknown names are
/// silently skipped, matching the lenient posture used elsewhere in config.
/// Caller owns the returned registry and must call `deinit` on it.
pub fn buildRegistry(
    allocator: std.mem.Allocator,
    names: []const []const u8,
) !core.ToolRegistry {
    var reg = core.ToolRegistry.init(allocator);
    errdefer reg.deinit();
    for (names) |n| {
        if (lookup(n)) |t| try reg.register(t);
    }
    return reg;
}

/// Convenience: registry with every built-in tool.
pub fn buildRegistryAll(allocator: std.mem.Allocator) !core.ToolRegistry {
    var reg = core.ToolRegistry.init(allocator);
    errdefer reg.deinit();
    for (all) |t| try reg.register(t);
    return reg;
}

// ---- Tests ----

test "lookup finds known tools" {
    try std.testing.expect(lookup("read") != null);
    try std.testing.expect(lookup("write") != null);
    try std.testing.expect(lookup("edit") != null);
    try std.testing.expect(lookup("bash") != null);
    try std.testing.expect(lookup("nope") == null);
}

test "buildRegistry skips unknown names" {
    const a = std.testing.allocator;
    var reg = try buildRegistry(a, &.{ "read", "nonsense", "bash" });
    defer reg.deinit();
    try std.testing.expectEqual(@as(u32, 2), reg.count());
    try std.testing.expect(reg.get("read") != null);
    try std.testing.expect(reg.get("bash") != null);
    try std.testing.expect(reg.get("nonsense") == null);
}

test "buildRegistryAll registers four" {
    const a = std.testing.allocator;
    var reg = try buildRegistryAll(a);
    defer reg.deinit();
    try std.testing.expectEqual(@as(u32, 4), reg.count());
}

test {
    std.testing.refAllDecls(@This());
}
