const std = @import("std");
const config = @import("config.zig");

pub const ProviderEntry = struct {
    /// e.g. "default"
    name: []const u8,
    kind: config.ProviderKind,
    model: []const u8,
    /// Auth-table key referenced by `provider.<name>.auth`. Null for local
    /// providers that need no credential. When the credential is stored as
    /// an env-var ref, this should already include the `_env` suffix.
    auth_key: ?[]const u8 = null,
    /// Optional base URL override (used for local providers like ollama).
    base_url: ?[]const u8 = null,
};

pub const AuthEntryWrite = struct {
    /// Bare auth-table key (e.g. "anthropic_api_key" or "anthropic_api_key_env").
    key: []const u8,
    /// Either the inline secret (for inline mode) or the env var name (for env-ref mode).
    value: []const u8,
    is_env_ref: bool,
};

/// Serialize the wizard's choice and overwrite `<home>/.phoenix/phoenix.toml`.
/// Creates the directory if needed and chmods the file to 0600 on POSIX.
pub fn writeUserConfig(
    io: std.Io,
    allocator: std.mem.Allocator,
    home: []const u8,
    provider: ProviderEntry,
    auth: ?AuthEntryWrite,
) !void {
    if (home.len == 0) return error.HomeRequired;

    try validateValue(provider.model);
    if (provider.auth_key) |k| try validateValue(k);
    if (provider.base_url) |b| try validateValue(b);
    if (auth) |a| {
        try validateValue(a.key);
        try validateValue(a.value);
    }

    const dir = try std.fs.path.join(allocator, &.{ home, ".phoenix" });
    defer allocator.free(dir);

    const file_path = try std.fs.path.join(allocator, &.{ dir, "phoenix.toml" });
    defer allocator.free(file_path);

    try std.Io.Dir.cwd().createDirPath(io, dir);

    var buf: std.ArrayList(u8) = .empty;
    defer buf.deinit(allocator);

    try renderToml(&buf, allocator, provider, auth);

    try std.Io.Dir.cwd().writeFile(io, .{
        .sub_path = file_path,
        .data = buf.items,
    });

    chmod0600(file_path) catch |err| {
        std.log.warn("config_writer: chmod 0600 failed for {s}: {}", .{ file_path, err });
    };
}

fn renderToml(
    buf: *std.ArrayList(u8),
    allocator: std.mem.Allocator,
    provider: ProviderEntry,
    auth: ?AuthEntryWrite,
) !void {
    if (auth) |a| {
        try buf.appendSlice(allocator, "[auth]\n");
        try buf.appendSlice(allocator, a.key);
        try buf.appendSlice(allocator, " = \"");
        try appendEscaped(buf, allocator, a.value);
        try buf.appendSlice(allocator, "\"\n\n");
    }

    try buf.appendSlice(allocator, "[provider.");
    try buf.appendSlice(allocator, provider.name);
    try buf.appendSlice(allocator, "]\n");

    try buf.appendSlice(allocator, "kind = \"");
    try buf.appendSlice(allocator, kindString(provider.kind));
    try buf.appendSlice(allocator, "\"\n");

    try buf.appendSlice(allocator, "model = \"");
    try appendEscaped(buf, allocator, provider.model);
    try buf.appendSlice(allocator, "\"\n");

    if (provider.auth_key) |key| {
        try buf.appendSlice(allocator, "auth = \"");
        try appendEscaped(buf, allocator, key);
        try buf.appendSlice(allocator, "\"\n");
    }

    if (provider.base_url) |b| {
        try buf.appendSlice(allocator, "base_url = \"");
        try appendEscaped(buf, allocator, b);
        try buf.appendSlice(allocator, "\"\n");
    }
}

fn kindString(k: config.ProviderKind) []const u8 {
    return switch (k) {
        .claude => "claude",
        .openai => "openai",
        .ollama => "ollama",
        .llamacpp => "llamacpp",
        .vertex => "vertex",
        .gemini => "gemini",
    };
}

fn appendEscaped(buf: *std.ArrayList(u8), allocator: std.mem.Allocator, s: []const u8) !void {
    for (s) |c| {
        switch (c) {
            '\\' => try buf.appendSlice(allocator, "\\\\"),
            '"' => try buf.appendSlice(allocator, "\\\""),
            else => try buf.append(allocator, c),
        }
    }
}

/// Disallow newlines and other control bytes; the wizard input layer should
/// already guard against these but we belt-and-suspenders here so we never
/// emit invalid TOML.
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

test "round-trip inline auth" {
    const allocator = testing.allocator;
    var tmp = testing.tmpDir(.{});
    defer tmp.cleanup();

    const home = try tmpHome(allocator, &tmp);
    defer allocator.free(home);
    try std.Io.Dir.cwd().createDirPath(testing.io, home);

    try writeUserConfig(
        testing.io,
        allocator,
        home,
        .{
            .name = "default",
            .kind = .claude,
            .model = "claude-opus-4-7",
            .auth_key = "anthropic_api_key",
        },
        .{
            .key = "anthropic_api_key",
            .value = "sk-test-123",
            .is_env_ref = false,
        },
    );

    var cfg = try config.Config.load(allocator, testing.io, .{ .home = home });
    defer cfg.deinit();

    const p = cfg.provider("default") orelse return error.TestUnexpectedResult;
    try testing.expectEqual(config.ProviderKind.claude, p.kind);
    try testing.expectEqualStrings("claude-opus-4-7", p.model);
    try testing.expectEqualStrings("anthropic_api_key", p.auth.?);

    const resolved = (try cfg.auth.resolve(allocator, "anthropic_api_key")) orelse return error.TestUnexpectedResult;
    defer allocator.free(resolved);
    try testing.expectEqualStrings("sk-test-123", resolved);
}

test "round-trip env-var ref" {
    const allocator = testing.allocator;
    var tmp = testing.tmpDir(.{});
    defer tmp.cleanup();

    const home = try tmpHome(allocator, &tmp);
    defer allocator.free(home);
    try std.Io.Dir.cwd().createDirPath(testing.io, home);

    try writeUserConfig(
        testing.io,
        allocator,
        home,
        .{
            .name = "default",
            .kind = .openai,
            .model = "gpt-4o",
            .auth_key = "openai_api_key_env",
        },
        .{
            .key = "openai_api_key_env",
            .value = "OPENAI_API_KEY",
            .is_env_ref = true,
        },
    );

    var cfg = try config.Config.load(allocator, testing.io, .{ .home = home });
    defer cfg.deinit();

    const p = cfg.provider("default") orelse return error.TestUnexpectedResult;
    try testing.expectEqual(config.ProviderKind.openai, p.kind);
    try testing.expectEqualStrings("openai_api_key_env", p.auth.?);

    const entry = cfg.auth.entries.get("openai_api_key_env") orelse return error.TestUnexpectedResult;
    switch (entry) {
        .env_var => |v| try testing.expectEqualStrings("OPENAI_API_KEY", v),
        .inline_value => return error.TestUnexpectedResult,
    }
}

test "local provider emits no auth block" {
    const allocator = testing.allocator;
    var tmp = testing.tmpDir(.{});
    defer tmp.cleanup();

    const home = try tmpHome(allocator, &tmp);
    defer allocator.free(home);
    try std.Io.Dir.cwd().createDirPath(testing.io, home);

    try writeUserConfig(
        testing.io,
        allocator,
        home,
        .{
            .name = "default",
            .kind = .ollama,
            .model = "llama3",
            .auth_key = null,
        },
        null,
    );

    const file_path = try std.fs.path.join(allocator, &.{ home, ".phoenix", "phoenix.toml" });
    defer allocator.free(file_path);
    const contents = try std.Io.Dir.cwd().readFileAlloc(testing.io, file_path, allocator, .limited(64 * 1024));
    defer allocator.free(contents);

    try testing.expect(std.mem.indexOf(u8, contents, "[auth]") == null);
    try testing.expect(std.mem.indexOf(u8, contents, "kind = \"ollama\"") != null);
    try testing.expect(std.mem.indexOf(u8, contents, "auth = ") == null);

    var cfg = try config.Config.load(allocator, testing.io, .{ .home = home });
    defer cfg.deinit();

    const p = cfg.provider("default") orelse return error.TestUnexpectedResult;
    try testing.expectEqual(config.ProviderKind.ollama, p.kind);
    try testing.expect(p.auth == null);
}

test "escapes special characters in values" {
    const allocator = testing.allocator;
    var tmp = testing.tmpDir(.{});
    defer tmp.cleanup();

    const home = try tmpHome(allocator, &tmp);
    defer allocator.free(home);
    try std.Io.Dir.cwd().createDirPath(testing.io, home);

    const wild = "weird\\key\"value";

    try writeUserConfig(
        testing.io,
        allocator,
        home,
        .{
            .name = "default",
            .kind = .claude,
            .model = "claude-opus-4-7",
            .auth_key = "anthropic_api_key",
        },
        .{
            .key = "anthropic_api_key",
            .value = wild,
            .is_env_ref = false,
        },
    );

    var cfg = try config.Config.load(allocator, testing.io, .{ .home = home });
    defer cfg.deinit();

    const resolved = (try cfg.auth.resolve(allocator, "anthropic_api_key")) orelse return error.TestUnexpectedResult;
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

    const result = writeUserConfig(
        testing.io,
        allocator,
        home,
        .{
            .name = "default",
            .kind = .claude,
            .model = "bad\nmodel",
            .auth_key = "anthropic_api_key",
        },
        .{
            .key = "anthropic_api_key",
            .value = "sk-x",
            .is_env_ref = false,
        },
    );
    try testing.expectError(error.InvalidString, result);
}

test "file is readable after write" {
    const allocator = testing.allocator;
    var tmp = testing.tmpDir(.{});
    defer tmp.cleanup();

    const home = try tmpHome(allocator, &tmp);
    defer allocator.free(home);
    try std.Io.Dir.cwd().createDirPath(testing.io, home);

    try writeUserConfig(
        testing.io,
        allocator,
        home,
        .{
            .name = "default",
            .kind = .claude,
            .model = "claude-opus-4-7",
            .auth_key = "anthropic_api_key",
        },
        .{
            .key = "anthropic_api_key",
            .value = "sk-mode",
            .is_env_ref = false,
        },
    );

    const file_path = try std.fs.path.join(allocator, &.{ home, ".phoenix", "phoenix.toml" });
    defer allocator.free(file_path);

    var pbuf: [std.Io.Dir.max_path_bytes]u8 = undefined;
    @memcpy(pbuf[0..file_path.len], file_path);
    pbuf[file_path.len] = 0;
    const cstr: [*:0]const u8 = pbuf[0..file_path.len :0];

    try testing.expectEqual(@as(c_int, 0), std.c.access(cstr, 4)); // R_OK
}
