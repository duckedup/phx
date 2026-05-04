const std = @import("std");
const core = @import("phoenix_core");

const default_limit: usize = 2000;
const max_bytes: usize = 512 * 1024;

const image_exts = [_][]const u8{ ".jpg", ".jpeg", ".png", ".gif", ".webp" };

fn errResult(allocator: std.mem.Allocator, comptime fmt: []const u8, args: anytype) anyerror!core.tool.ToolResult {
    const msg = try std.fmt.allocPrint(allocator, fmt, args);
    return .{ .output = msg, .is_error = true };
}

fn isImagePath(path: []const u8) bool {
    var i: usize = path.len;
    while (i > 0) : (i -= 1) {
        if (path[i - 1] == '.') {
            const ext = path[i - 1 ..];
            for (image_exts) |e| {
                if (std.ascii.eqlIgnoreCase(ext, e)) return true;
            }
            return false;
        }
        if (path[i - 1] == '/') return false;
    }
    return false;
}

fn invoke(self: *const core.Tool, io: std.Io, args: []const u8, allocator: std.mem.Allocator) anyerror!core.tool.ToolResult {
    _ = self;

    var parsed = std.json.parseFromSlice(std.json.Value, allocator, args, .{}) catch {
        return errResult(allocator, "read: invalid JSON args: {s}", .{args});
    };
    defer parsed.deinit();
    if (parsed.value != .object) return errResult(allocator, "read: args must be a JSON object", .{});
    const obj = parsed.value.object;

    const path_v = obj.get("path") orelse return errResult(allocator, "read: missing required field 'path'", .{});
    if (path_v != .string) return errResult(allocator, "read: 'path' must be a string", .{});
    const path = path_v.string;

    if (isImagePath(path)) {
        const msg = try std.fmt.allocPrint(allocator, "read: image attachments not yet supported (path={s})", .{path});
        return .{ .output = msg, .is_error = true };
    }

    var offset: usize = 1;
    if (obj.get("offset")) |v| {
        if (v != .integer or v.integer < 1) return errResult(allocator, "read: 'offset' must be a positive integer (1-indexed)", .{});
        offset = @intCast(v.integer);
    }

    var limit: usize = default_limit;
    if (obj.get("limit")) |v| {
        if (v != .integer or v.integer < 1) return errResult(allocator, "read: 'limit' must be a positive integer", .{});
        limit = @intCast(v.integer);
    }

    const all = std.Io.Dir.cwd().readFileAlloc(io, path, allocator, .limited(max_bytes)) catch |err| {
        return errResult(allocator, "read: cannot read {s}: {s}", .{ path, @errorName(err) });
    };
    defer allocator.free(all);

    // Find byte offsets of the (offset)th and (offset+limit)th newlines
    // and slice between them. This preserves trailing-newline semantics
    // exactly (no synthetic newlines added or dropped).
    var start: usize = 0;
    var skipped: usize = 0;
    while (skipped + 1 < offset) {
        const nl = std.mem.indexOfScalarPos(u8, all, start, '\n') orelse {
            // File has fewer lines than `offset` — return empty.
            const owned = try allocator.dupe(u8, "");
            return .{ .output = owned };
        };
        start = nl + 1;
        skipped += 1;
    }

    var end: usize = start;
    var taken: usize = 0;
    while (taken < limit) {
        const nl = std.mem.indexOfScalarPos(u8, all, end, '\n') orelse {
            end = all.len;
            break;
        };
        end = nl + 1;
        taken += 1;
    }

    const slice = all[start..end];
    const owned = try allocator.dupe(u8, slice);
    return .{ .output = owned };
}

pub const read_tool: core.Tool = .{
    .name = "read",
    .description = "Read the contents of a file. Supports text files. For text files, defaults to first 2000 lines. Use offset/limit for large files. Image files (jpg, png, gif, webp) are not yet supported.",
    .schema =
    \\{"type":"object","properties":{"path":{"type":"string","description":"Path to the file to read (relative or absolute)"},"offset":{"type":"integer","minimum":1,"description":"Line number to start reading from (1-indexed)"},"limit":{"type":"integer","minimum":1,"description":"Maximum number of lines to read"}},"required":["path"]}
    ,
    .max_output_bytes = max_bytes,
    .invokeFn = invoke,
};

// ---- Tests ----

test "read default returns whole small file" {
    const a = std.testing.allocator;
    const io = std.testing.io;
    const tmp_path = "phoenix_test_read_default.txt";
    try std.Io.Dir.cwd().writeFile(io, .{ .sub_path = tmp_path, .data = "alpha\nbeta\ngamma\n" });
    defer std.Io.Dir.cwd().deleteFile(io, tmp_path) catch {};

    const args =
        \\{"path":"phoenix_test_read_default.txt"}
    ;
    const r = try read_tool.invoke(io, args, a);
    defer a.free(r.output);
    try std.testing.expectEqualStrings("alpha\nbeta\ngamma\n", r.output);
}

test "read offset and limit slice" {
    const a = std.testing.allocator;
    const io = std.testing.io;
    const tmp_path = "phoenix_test_read_slice.txt";
    try std.Io.Dir.cwd().writeFile(io, .{ .sub_path = tmp_path, .data = "one\ntwo\nthree\nfour\nfive\n" });
    defer std.Io.Dir.cwd().deleteFile(io, tmp_path) catch {};

    const args =
        \\{"path":"phoenix_test_read_slice.txt","offset":2,"limit":2}
    ;
    const r = try read_tool.invoke(io, args, a);
    defer a.free(r.output);
    try std.testing.expectEqualStrings("two\nthree\n", r.output);
}

test "read image extension is rejected" {
    const a = std.testing.allocator;
    const io = std.testing.io;
    const args =
        \\{"path":"foo/bar.PNG"}
    ;
    const r = try read_tool.invoke(io, args, a);
    defer a.free(r.output);
    try std.testing.expect(r.is_error);
    try std.testing.expect(std.mem.indexOf(u8, r.output, "image attachments not yet supported") != null);
}

test "read missing path is error" {
    const a = std.testing.allocator;
    const io = std.testing.io;
    const r = try read_tool.invoke(io, "{}", a);
    defer a.free(r.output);
    try std.testing.expect(r.is_error);
    try std.testing.expect(std.mem.indexOf(u8, r.output, "missing required field 'path'") != null);
}

test "read invalid JSON is error" {
    const a = std.testing.allocator;
    const io = std.testing.io;
    const r = try read_tool.invoke(io, "not json", a);
    defer a.free(r.output);
    try std.testing.expect(r.is_error);
    try std.testing.expect(std.mem.indexOf(u8, r.output, "invalid JSON") != null);
}
