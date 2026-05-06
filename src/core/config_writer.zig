const std = @import("std");
const config = @import("config.zig");

/// Serialize the full providers list (preserving everything except comments)
/// and overwrite `<home>/.phoenix/phoenix.json`. Creates the directory if
/// needed and chmods the file to 0600 on POSIX.
///
/// Comment preservation across rewrites is explicitly out of scope: we emit
/// the file from scratch every time. Users who add comments to `phoenix.json`
/// by hand will lose them on the next `/model` swap. Same trade as the old
/// TOML implementation.
pub fn writeUserConfig(
    io: std.Io,
    allocator: std.mem.Allocator,
    home: []const u8,
    providers: []const config.ProviderProfile,
) !void {
    return writeUserConfigFull(io, allocator, home, providers, null);
}

pub fn writeUserConfigFull(
    io: std.Io,
    allocator: std.mem.Allocator,
    home: []const u8,
    providers: []const config.ProviderProfile,
    theme_name: ?[]const u8,
) !void {
    if (home.len == 0) return error.HomeRequired;
    for (providers) |p| try validateProvider(p);
    if (theme_name) |t| try validateValue(t);

    const dir = try std.fs.path.join(allocator, &.{ home, ".phoenix" });
    defer allocator.free(dir);

    const file_path = try std.fs.path.join(allocator, &.{ dir, "phoenix.json" });
    defer allocator.free(file_path);

    try std.Io.Dir.cwd().createDirPath(io, dir);

    var buf: std.ArrayList(u8) = .empty;
    defer buf.deinit(allocator);
    try renderJsonc(&buf, allocator, providers, theme_name);

    try std.Io.Dir.cwd().writeFile(io, .{
        .sub_path = file_path,
        .data = buf.items,
    });

    chmod0600(file_path) catch |err| {
        std.log.warn("config_writer: chmod 0600 failed for {s}: {}", .{ file_path, err });
    };
}

fn renderJsonc(
    buf: *std.ArrayList(u8),
    a: std.mem.Allocator,
    providers: []const config.ProviderProfile,
    theme_name: ?[]const u8,
) !void {
    try buf.appendSlice(a, "{\n");
    if (theme_name) |t| {
        try buf.appendSlice(a, "  ");
        try writeKeyString(buf, a, "", "theme", t);
        try buf.appendSlice(a, ",\n");
    }
    try buf.appendSlice(a, "  \"providers\": [\n");
    for (providers, 0..) |p, i| {
        try buf.appendSlice(a, "    {\n");
        try writeKeyString(buf, a, "      ", "kind", kindString(p.kind));
        try buf.appendSlice(a, ",\n");
        try writeKeyString(buf, a, "      ", "model", p.model);
        if (p.active) {
            try buf.appendSlice(a, ",\n      \"active\": true");
        }
        if (p.auth) |auth| {
            try buf.appendSlice(a, ",\n");
            switch (auth) {
                .env_var => |env_name| {
                    try buf.appendSlice(a, "      \"auth\": { \"env\": ");
                    try writeJsonString(buf, a, env_name);
                    try buf.appendSlice(a, " }");
                },
                .inline_value => |secret| {
                    try buf.appendSlice(a, "      \"auth\": ");
                    try writeJsonString(buf, a, secret);
                },
            }
        }
        if (p.base_url) |b| {
            try buf.appendSlice(a, ",\n");
            try writeKeyString(buf, a, "      ", "base_url", b);
        }
        if (p.endpoint) |e| {
            try buf.appendSlice(a, ",\n");
            try writeKeyString(buf, a, "      ", "endpoint", e);
        }
        if (p.api) |api| {
            try buf.appendSlice(a, ",\n");
            try writeKeyString(buf, a, "      ", "api", api);
        }
        if (p.project) |proj| {
            try buf.appendSlice(a, ",\n");
            try writeKeyString(buf, a, "      ", "project", proj);
        }
        if (p.location) |loc| {
            try buf.appendSlice(a, ",\n");
            try writeKeyString(buf, a, "      ", "location", loc);
        }
        if (p.credentials_path) |cp| {
            try buf.appendSlice(a, ",\n");
            try writeKeyString(buf, a, "      ", "credentials_path", cp);
        }
        if (p.context_window) |cw| {
            try buf.print(a, ",\n      \"context_window\": {d}", .{cw});
        }
        if (p.cache_ttl) |ttl| {
            try buf.appendSlice(a, ",\n");
            try writeKeyString(buf, a, "      ", "cache_ttl", ttl);
        }
        try buf.appendSlice(a, "\n    }");
        if (i + 1 < providers.len) try buf.appendSlice(a, ",");
        try buf.appendSlice(a, "\n");
    }
    try buf.appendSlice(a, "  ]\n}\n");
}

fn writeKeyString(
    buf: *std.ArrayList(u8),
    a: std.mem.Allocator,
    indent: []const u8,
    key: []const u8,
    value: []const u8,
) !void {
    try buf.appendSlice(a, indent);
    try writeJsonString(buf, a, key);
    try buf.appendSlice(a, ": ");
    try writeJsonString(buf, a, value);
}

fn writeJsonString(buf: *std.ArrayList(u8), a: std.mem.Allocator, s: []const u8) !void {
    const encoded = try std.json.Stringify.valueAlloc(a, s, .{});
    defer a.free(encoded);
    try buf.appendSlice(a, encoded);
}

fn kindString(k: config.ProviderKind) []const u8 {
    return switch (k) {
        .claude => "claude",
        .openai => "openai",
        .ollama => "ollama",
        .llamacpp => "llamacpp",
        .vertex => "vertex",
        .gemini => "gemini",
        .nvidia => "nvidia",
    };
}

fn validateProvider(p: config.ProviderProfile) !void {
    try validateValue(p.model);
    if (p.base_url) |b| try validateValue(b);
    if (p.endpoint) |e| try validateValue(e);
    if (p.api) |x| try validateValue(x);
    if (p.project) |x| try validateValue(x);
    if (p.location) |x| try validateValue(x);
    if (p.credentials_path) |x| try validateValue(x);
    if (p.cache_ttl) |x| try validateValue(x);
    if (p.auth) |auth| switch (auth) {
        .env_var => |v| try validateValue(v),
        .inline_value => |v| try validateValue(v),
    };
}

fn validateValue(s: []const u8) !void {
    for (s) |c| {
        if (c == '\n' or c == '\r' or c == 0) return error.InvalidString;
    }
}

fn chmod0600(path: []const u8) !void {
    var pbuf: [std.Io.Dir.max_path_bytes]u8 = undefined;
    if (path.len + 1 > pbuf.len) return error.PathTooLong;
    @memcpy(pbuf[0..path.len], path);
    pbuf[path.len] = 0;
    const cstr: [*:0]const u8 = pbuf[0..path.len :0];
    const rc = std.c.chmod(cstr, 0o600);
    if (rc != 0) return error.ChmodFailed;
}

// ---- Tests ----

const testing = std.testing;

fn tmpHome(allocator: std.mem.Allocator, tmp: *std.testing.TmpDir) ![]u8 {
    var buf: [std.Io.Dir.max_path_bytes]u8 = undefined;
    const ptr = std.c.getcwd(&buf, buf.len) orelse return error.CwdError;
    const cwd_path = std.mem.sliceTo(ptr, 0);
    return try std.fs.path.join(allocator, &.{ cwd_path, ".zig-cache", "tmp", &tmp.sub_path, "home" });
}

test "round-trip env-var auth" {
    const allocator = testing.allocator;
    var tmp = testing.tmpDir(.{});
    defer tmp.cleanup();

    const home = try tmpHome(allocator, &tmp);
    defer allocator.free(home);
    try std.Io.Dir.cwd().createDirPath(testing.io, home);

    const providers = [_]config.ProviderProfile{
        .{ .kind = .openai, .model = "gpt-4o", .active = true, .auth = .{ .env_var = "OPENAI_API_KEY" } },
    };
    try writeUserConfig(testing.io, allocator, home, &providers);

    var cfg = try config.Config.load(allocator, testing.io, .{ .home = home });
    defer cfg.deinit();
    const ap = cfg.activeProvider().?;
    try testing.expectEqual(config.ProviderKind.openai, ap.kind);
    try testing.expectEqualStrings("gpt-4o", ap.model);
    switch (ap.auth.?) {
        .env_var => |v| try testing.expectEqualStrings("OPENAI_API_KEY", v),
        else => return error.TestUnexpectedResult,
    }
}

test "round-trip inline secret" {
    const allocator = testing.allocator;
    var tmp = testing.tmpDir(.{});
    defer tmp.cleanup();
    const home = try tmpHome(allocator, &tmp);
    defer allocator.free(home);
    try std.Io.Dir.cwd().createDirPath(testing.io, home);

    const providers = [_]config.ProviderProfile{
        .{ .kind = .claude, .model = "claude-opus-4-7", .active = true, .auth = .{ .inline_value = "sk-test-123" } },
    };
    try writeUserConfig(testing.io, allocator, home, &providers);

    var cfg = try config.Config.load(allocator, testing.io, .{ .home = home });
    defer cfg.deinit();
    const ap = cfg.activeProvider().?;
    const resolved = (try ap.auth.?.resolve(allocator)) orelse return error.TestUnexpectedResult;
    defer allocator.free(resolved);
    try testing.expectEqualStrings("sk-test-123", resolved);
}

test "local provider emits no auth field" {
    const allocator = testing.allocator;
    var tmp = testing.tmpDir(.{});
    defer tmp.cleanup();
    const home = try tmpHome(allocator, &tmp);
    defer allocator.free(home);
    try std.Io.Dir.cwd().createDirPath(testing.io, home);

    const providers = [_]config.ProviderProfile{
        .{ .kind = .ollama, .model = "llama3", .active = true, .base_url = "http://localhost:11434" },
    };
    try writeUserConfig(testing.io, allocator, home, &providers);

    const file_path = try std.fs.path.join(allocator, &.{ home, ".phoenix", "phoenix.json" });
    defer allocator.free(file_path);
    const contents = try std.Io.Dir.cwd().readFileAlloc(testing.io, file_path, allocator, .limited(64 * 1024));
    defer allocator.free(contents);
    try testing.expect(std.mem.indexOf(u8, contents, "\"auth\"") == null);
    try testing.expect(std.mem.indexOf(u8, contents, "\"kind\": \"ollama\"") != null);
    try testing.expect(std.mem.indexOf(u8, contents, "\"base_url\"") != null);

    var cfg = try config.Config.load(allocator, testing.io, .{ .home = home });
    defer cfg.deinit();
    const ap = cfg.activeProvider().?;
    try testing.expectEqual(config.ProviderKind.ollama, ap.kind);
    try testing.expect(ap.auth == null);
}

test "escapes special characters in values" {
    const allocator = testing.allocator;
    var tmp = testing.tmpDir(.{});
    defer tmp.cleanup();
    const home = try tmpHome(allocator, &tmp);
    defer allocator.free(home);
    try std.Io.Dir.cwd().createDirPath(testing.io, home);

    const wild = "weird\\key\"value";
    const providers = [_]config.ProviderProfile{
        .{ .kind = .claude, .model = "claude-opus-4-7", .active = true, .auth = .{ .inline_value = wild } },
    };
    try writeUserConfig(testing.io, allocator, home, &providers);

    var cfg = try config.Config.load(allocator, testing.io, .{ .home = home });
    defer cfg.deinit();
    const resolved = (try cfg.activeProvider().?.auth.?.resolve(allocator)) orelse return error.TestUnexpectedResult;
    defer allocator.free(resolved);
    try testing.expectEqualStrings(wild, resolved);
}

test "rejects newline in model" {
    const allocator = testing.allocator;
    var tmp = testing.tmpDir(.{});
    defer tmp.cleanup();
    const home = try tmpHome(allocator, &tmp);
    defer allocator.free(home);
    try std.Io.Dir.cwd().createDirPath(testing.io, home);

    const providers = [_]config.ProviderProfile{
        .{ .kind = .claude, .model = "bad\nmodel", .active = true, .auth = .{ .inline_value = "sk-x" } },
    };
    try testing.expectError(error.InvalidString, writeUserConfig(testing.io, allocator, home, &providers));
}

test "round-trip cache_ttl" {
    const allocator = testing.allocator;
    var tmp = testing.tmpDir(.{});
    defer tmp.cleanup();
    const home = try tmpHome(allocator, &tmp);
    defer allocator.free(home);
    try std.Io.Dir.cwd().createDirPath(testing.io, home);

    const providers = [_]config.ProviderProfile{
        .{ .kind = .claude, .model = "claude-opus-4-7", .active = true, .auth = .{ .env_var = "ANTHROPIC_API_KEY" }, .cache_ttl = "1h" },
    };
    try writeUserConfig(testing.io, allocator, home, &providers);

    var cfg = try config.Config.load(allocator, testing.io, .{ .home = home });
    defer cfg.deinit();
    const ap = cfg.activeProvider().?;
    try testing.expectEqualStrings("1h", ap.cache_ttl.?);
}

test "round-trip cache_ttl null" {
    const allocator = testing.allocator;
    var tmp = testing.tmpDir(.{});
    defer tmp.cleanup();
    const home = try tmpHome(allocator, &tmp);
    defer allocator.free(home);
    try std.Io.Dir.cwd().createDirPath(testing.io, home);

    const providers = [_]config.ProviderProfile{
        .{ .kind = .claude, .model = "claude-opus-4-7", .active = true, .auth = .{ .env_var = "ANTHROPIC_API_KEY" } },
    };
    try writeUserConfig(testing.io, allocator, home, &providers);

    var cfg = try config.Config.load(allocator, testing.io, .{ .home = home });
    defer cfg.deinit();
    const ap = cfg.activeProvider().?;
    try testing.expect(ap.cache_ttl == null);
}

test "writes multiple providers, only one active" {
    const allocator = testing.allocator;
    var tmp = testing.tmpDir(.{});
    defer tmp.cleanup();
    const home = try tmpHome(allocator, &tmp);
    defer allocator.free(home);
    try std.Io.Dir.cwd().createDirPath(testing.io, home);

    const providers = [_]config.ProviderProfile{
        .{ .kind = .claude, .model = "claude-opus-4-7", .active = true, .auth = .{ .env_var = "ANTHROPIC_API_KEY" } },
        .{ .kind = .openai, .model = "gpt-4o", .auth = .{ .env_var = "OPENAI_API_KEY" } },
    };
    try writeUserConfig(testing.io, allocator, home, &providers);

    var cfg = try config.Config.load(allocator, testing.io, .{ .home = home });
    defer cfg.deinit();
    try testing.expectEqual(@as(usize, 2), cfg.providers.len);
    try testing.expectEqual(config.ProviderKind.claude, cfg.activeProvider().?.kind);
}
