const std = @import("std");

pub const ConfigPaths = struct {
    user: ?[]u8 = null,
    project: ?[]u8 = null,
    explicit: ?[]u8 = null,
    user_dir: ?[]u8 = null,
    project_dir: ?[]u8 = null,

    pub fn deinit(self: *ConfigPaths, allocator: std.mem.Allocator) void {
        if (self.user) |p| allocator.free(p);
        if (self.project) |p| allocator.free(p);
        if (self.explicit) |p| allocator.free(p);
        if (self.user_dir) |p| allocator.free(p);
        if (self.project_dir) |p| allocator.free(p);
        self.* = .{};
    }
};

pub const Discovery = struct {
    home: ?[]const u8 = null,
    cwd: ?[]const u8 = null,
    explicit_path: ?[]const u8 = null,

    pub fn discover(self: Discovery, io: std.Io, allocator: std.mem.Allocator) !ConfigPaths {
        var paths = ConfigPaths{};
        errdefer paths.deinit(allocator);

        // Resolve home directory
        var owned_home: ?[]u8 = null;
        defer if (owned_home) |h| allocator.free(h);

        const home_dir: ?[]const u8 = self.home orelse blk: {
            // Use POSIX getenv to get HOME
            if (std.c.getenv("HOME")) |ptr| {
                const s = std.mem.span(ptr);
                owned_home = try allocator.dupe(u8, s);
                break :blk owned_home;
            }
            break :blk null;
        };

        // Resolve cwd
        var owned_cwd: ?[]u8 = null;
        defer if (owned_cwd) |c| allocator.free(c);

        const cwd_dir: ?[]const u8 = self.cwd orelse blk: {
            var buf: [std.Io.Dir.max_path_bytes]u8 = undefined;
            const ptr = std.c.getcwd(&buf, buf.len) orelse break :blk null;
            const cwd_slice = std.mem.sliceTo(ptr, 0);
            owned_cwd = try allocator.dupe(u8, cwd_slice);
            break :blk owned_cwd;
        };

        _ = io;

        // User dir = <home>/.phoenix
        if (home_dir) |home| {
            paths.user_dir = try std.fs.path.join(allocator, &.{ home, ".phoenix" });
        }

        // Project dir = <cwd>/.phoenix
        if (cwd_dir) |cwd| {
            paths.project_dir = try std.fs.path.join(allocator, &.{ cwd, ".phoenix" });
        }

        // User config file = <user_dir>/phoenix.json if it exists. Missing => null.
        if (paths.user_dir) |udir| {
            const candidate = try std.fs.path.join(allocator, &.{ udir, "phoenix.json" });
            if (fileExistsPosix(candidate)) {
                paths.user = candidate;
            } else {
                allocator.free(candidate);
                paths.user = null;
            }
        }

        // Project config file = <project_dir>/phoenix.json if it exists. Missing => null.
        if (paths.project_dir) |pdir| {
            const candidate = try std.fs.path.join(allocator, &.{ pdir, "phoenix.json" });
            if (fileExistsPosix(candidate)) {
                paths.project = candidate;
            } else {
                allocator.free(candidate);
                paths.project = null;
            }
        }

        // Explicit path stored verbatim, no existence check.
        if (self.explicit_path) |ep| {
            paths.explicit = try allocator.dupe(u8, ep);
        }

        std.log.debug("config: user={s} project={s} explicit={s}", .{
            paths.user orelse "(none)",
            paths.project orelse "(none)",
            paths.explicit orelse "(none)",
        });

        return paths;
    }
};

/// Check if a file exists using POSIX access(2).
fn fileExistsPosix(path: []const u8) bool {
    var buf: [std.Io.Dir.max_path_bytes]u8 = undefined;
    if (path.len + 1 > buf.len) return false;
    @memcpy(buf[0..path.len], path);
    buf[path.len] = 0;
    const cstr: [*:0]const u8 = @ptrCast(&buf);
    const rc = std.c.access(cstr, 0); // F_OK = 0
    return rc == 0;
}

test "no files returns nulls" {
    const allocator = std.testing.allocator;
    var tmp = std.testing.tmpDir(.{});
    defer tmp.cleanup();

    const tmp_path = try getTmpDirPath(allocator, &tmp);
    defer allocator.free(tmp_path);

    const home = try std.fs.path.join(allocator, &.{ tmp_path, "home" });
    defer allocator.free(home);
    const cwd = try std.fs.path.join(allocator, &.{ tmp_path, "cwd" });
    defer allocator.free(cwd);

    try std.Io.Dir.cwd().createDirPath(std.testing.io, home);
    try std.Io.Dir.cwd().createDirPath(std.testing.io, cwd);

    const d: Discovery = .{ .home = home, .cwd = cwd };
    var paths = try d.discover(std.testing.io, allocator);
    defer paths.deinit(allocator);

    try std.testing.expect(paths.user == null);
    try std.testing.expect(paths.project == null);
    try std.testing.expect(paths.explicit == null);
    try std.testing.expect(paths.user_dir != null);
    try std.testing.expect(paths.project_dir != null);
}

test "finds project config" {
    const allocator = std.testing.allocator;
    var tmp = std.testing.tmpDir(.{});
    defer tmp.cleanup();

    const tmp_path = try getTmpDirPath(allocator, &tmp);
    defer allocator.free(tmp_path);

    const home = try std.fs.path.join(allocator, &.{ tmp_path, "home" });
    defer allocator.free(home);
    const cwd = try std.fs.path.join(allocator, &.{ tmp_path, "project" });
    defer allocator.free(cwd);

    try std.Io.Dir.cwd().createDirPath(std.testing.io, home);
    try std.Io.Dir.cwd().createDirPath(std.testing.io, cwd);

    const phoenix_dir = try std.fs.path.join(allocator, &.{ cwd, ".phoenix" });
    defer allocator.free(phoenix_dir);
    try std.Io.Dir.cwd().createDirPath(std.testing.io, phoenix_dir);

    const json_path = try std.fs.path.join(allocator, &.{ phoenix_dir, "phoenix.json" });
    defer allocator.free(json_path);
    const f = try std.Io.Dir.cwd().createFile(std.testing.io, json_path, .{});
    f.close(std.testing.io);

    const d: Discovery = .{ .home = home, .cwd = cwd };
    var paths = try d.discover(std.testing.io, allocator);
    defer paths.deinit(allocator);

    try std.testing.expect(paths.user == null);
    try std.testing.expect(paths.project != null);
    try std.testing.expect(std.mem.endsWith(u8, paths.project.?, "phoenix.json"));
}

test "finds user config" {
    const allocator = std.testing.allocator;
    var tmp = std.testing.tmpDir(.{});
    defer tmp.cleanup();

    const tmp_path = try getTmpDirPath(allocator, &tmp);
    defer allocator.free(tmp_path);

    const home = try std.fs.path.join(allocator, &.{ tmp_path, "home" });
    defer allocator.free(home);
    const cwd = try std.fs.path.join(allocator, &.{ tmp_path, "cwd" });
    defer allocator.free(cwd);

    try std.Io.Dir.cwd().createDirPath(std.testing.io, home);

    const phoenix_dir = try std.fs.path.join(allocator, &.{ home, ".phoenix" });
    defer allocator.free(phoenix_dir);
    try std.Io.Dir.cwd().createDirPath(std.testing.io, phoenix_dir);

    const json_path = try std.fs.path.join(allocator, &.{ phoenix_dir, "phoenix.json" });
    defer allocator.free(json_path);
    const f = try std.Io.Dir.cwd().createFile(std.testing.io, json_path, .{});
    f.close(std.testing.io);

    try std.Io.Dir.cwd().createDirPath(std.testing.io, cwd);

    const d: Discovery = .{ .home = home, .cwd = cwd };
    var paths = try d.discover(std.testing.io, allocator);
    defer paths.deinit(allocator);

    try std.testing.expect(paths.user != null);
    try std.testing.expect(std.mem.endsWith(u8, paths.user.?, "phoenix.json"));
    try std.testing.expect(paths.project == null);
}

test "explicit pass-through" {
    const allocator = std.testing.allocator;
    var tmp = std.testing.tmpDir(.{});
    defer tmp.cleanup();

    const tmp_path = try getTmpDirPath(allocator, &tmp);
    defer allocator.free(tmp_path);

    const home = try std.fs.path.join(allocator, &.{ tmp_path, "home" });
    defer allocator.free(home);
    const cwd = try std.fs.path.join(allocator, &.{ tmp_path, "cwd" });
    defer allocator.free(cwd);

    try std.Io.Dir.cwd().createDirPath(std.testing.io, home);
    try std.Io.Dir.cwd().createDirPath(std.testing.io, cwd);

    const d: Discovery = .{ .home = home, .cwd = cwd, .explicit_path = "/some/custom/path.json" };
    var paths = try d.discover(std.testing.io, allocator);
    defer paths.deinit(allocator);

    try std.testing.expect(paths.explicit != null);
    try std.testing.expectEqualStrings("/some/custom/path.json", paths.explicit.?);
}

fn getTmpDirPath(allocator: std.mem.Allocator, tmp: *std.testing.TmpDir) ![]u8 {
    var buf: [std.Io.Dir.max_path_bytes]u8 = undefined;
    const ptr = std.c.getcwd(&buf, buf.len) orelse return error.CwdError;
    const cwd_path = std.mem.sliceTo(ptr, 0);
    return try std.fs.path.join(allocator, &.{ cwd_path, ".zig-cache", "tmp", &tmp.sub_path });
}
