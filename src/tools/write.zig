const std = @import("std");
const core = @import("phoenix_core");

const max_bytes: usize = 512 * 1024;

fn errResult(allocator: std.mem.Allocator, comptime fmt: []const u8, args: anytype) anyerror!core.tool.ToolResult {
    const msg = try std.fmt.allocPrint(allocator, fmt, args);
    return .{ .output = msg, .is_error = true };
}

fn invoke(self: *const core.Tool, io: std.Io, args: []const u8, allocator: std.mem.Allocator) anyerror!core.tool.ToolResult {
    _ = self;

    var parsed = std.json.parseFromSlice(std.json.Value, allocator, args, .{}) catch {
        return errResult(allocator, "write: invalid JSON args: {s}", .{args});
    };
    defer parsed.deinit();
    if (parsed.value != .object) return errResult(allocator, "write: args must be a JSON object", .{});
    const obj = parsed.value.object;

    const path_v = obj.get("path") orelse return errResult(allocator, "write: missing required field 'path'", .{});
    if (path_v != .string) return errResult(allocator, "write: 'path' must be a string", .{});
    const path = path_v.string;

    const content_v = obj.get("content") orelse return errResult(allocator, "write: missing required field 'content'", .{});
    if (content_v != .string) return errResult(allocator, "write: 'content' must be a string", .{});
    const content = content_v.string;

    if (std.fs.path.dirname(path)) |dir| {
        if (dir.len > 0) {
            std.Io.Dir.cwd().createDirPath(io, dir) catch |err| {
                return errResult(allocator, "write: cannot create parent directory {s}: {s}", .{ dir, @errorName(err) });
            };
        }
    }

    std.Io.Dir.cwd().writeFile(io, .{ .sub_path = path, .data = content }) catch |err| {
        return errResult(allocator, "write: cannot write {s}: {s}", .{ path, @errorName(err) });
    };

    const msg = try std.fmt.allocPrint(allocator, "wrote {d} bytes to {s}", .{ content.len, path });
    return .{ .output = msg };
}

pub const write_tool: core.Tool = .{
    .name = "write",
    .description = "Write content to a file. Creates the file if it doesn't exist, overwrites if it does. Automatically creates parent directories.",
    .schema =
    \\{"type":"object","properties":{"path":{"type":"string","description":"Path to the file to write (relative or absolute)"},"content":{"type":"string","description":"Content to write to the file"}},"required":["path","content"]}
    ,
    .max_output_bytes = max_bytes,
    .invokeFn = invoke,
};

// ---- Tests ----

test "write creates file and parent dir" {
    const a = std.testing.allocator;
    const io = std.testing.io;
    const tmp_path = "phoenix_test_write_dir/nested/dir/out.txt";
    defer {
        std.Io.Dir.cwd().deleteTree(io, "phoenix_test_write_dir") catch {};
    }

    const args =
        \\{"path":"phoenix_test_write_dir/nested/dir/out.txt","content":"hello world"}
    ;
    const r = try write_tool.invoke(io, args, a);
    defer a.free(r.output);
    try std.testing.expect(!r.is_error);
    try std.testing.expect(std.mem.indexOf(u8, r.output, "wrote 11 bytes") != null);

    const back = try std.Io.Dir.cwd().readFileAlloc(io, tmp_path, a, .limited(128));
    defer a.free(back);
    try std.testing.expectEqualStrings("hello world", back);
}

test "write overwrites existing file" {
    const a = std.testing.allocator;
    const io = std.testing.io;
    const tmp_path = "phoenix_test_write_overwrite.txt";
    try std.Io.Dir.cwd().writeFile(io, .{ .sub_path = tmp_path, .data = "OLD CONTENT THAT IS LONGER" });
    defer std.Io.Dir.cwd().deleteFile(io, tmp_path) catch {};

    const args =
        \\{"path":"phoenix_test_write_overwrite.txt","content":"new"}
    ;
    const r = try write_tool.invoke(io, args, a);
    defer a.free(r.output);

    const back = try std.Io.Dir.cwd().readFileAlloc(io, tmp_path, a, .limited(128));
    defer a.free(back);
    try std.testing.expectEqualStrings("new", back);
}

test "write missing content is error" {
    const a = std.testing.allocator;
    const io = std.testing.io;
    const args =
        \\{"path":"/tmp/x"}
    ;
    const r = try write_tool.invoke(io, args, a);
    defer a.free(r.output);
    try std.testing.expect(r.is_error);
    try std.testing.expect(std.mem.indexOf(u8, r.output, "missing required field 'content'") != null);
}
