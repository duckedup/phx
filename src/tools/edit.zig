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
        return errResult(allocator, "edit: invalid JSON args: {s}", .{args});
    };
    defer parsed.deinit();
    if (parsed.value != .object) return errResult(allocator, "edit: args must be a JSON object", .{});
    const obj = parsed.value.object;

    const path_v = obj.get("path") orelse return errResult(allocator, "edit: missing required field 'path'", .{});
    if (path_v != .string) return errResult(allocator, "edit: 'path' must be a string", .{});
    const path = path_v.string;

    const old_v = obj.get("oldText") orelse return errResult(allocator, "edit: missing required field 'oldText'", .{});
    if (old_v != .string) return errResult(allocator, "edit: 'oldText' must be a string", .{});
    const old_text = old_v.string;

    const new_v = obj.get("newText") orelse return errResult(allocator, "edit: missing required field 'newText'", .{});
    if (new_v != .string) return errResult(allocator, "edit: 'newText' must be a string", .{});
    const new_text = new_v.string;

    if (old_text.len == 0) return errResult(allocator, "edit: 'oldText' must not be empty", .{});

    const orig = std.Io.Dir.cwd().readFileAlloc(io, path, allocator, .limited(max_bytes)) catch |err| {
        return errResult(allocator, "edit: cannot read {s}: {s}", .{ path, @errorName(err) });
    };
    defer allocator.free(orig);

    const idx = std.mem.indexOf(u8, orig, old_text) orelse {
        return errResult(allocator, "edit: oldText not found in {s}", .{path});
    };

    var out: std.ArrayList(u8) = .empty;
    defer out.deinit(allocator);
    try out.appendSlice(allocator, orig[0..idx]);
    try out.appendSlice(allocator, new_text);
    try out.appendSlice(allocator, orig[idx + old_text.len ..]);

    std.Io.Dir.cwd().writeFile(io, .{ .sub_path = path, .data = out.items }) catch |err| {
        return errResult(allocator, "edit: cannot write {s}: {s}", .{ path, @errorName(err) });
    };

    const msg = try std.fmt.allocPrint(allocator, "edited {s} ({d} -> {d} bytes)", .{ path, orig.len, out.items.len });
    return .{ .output = msg };
}

pub const edit_tool: core.Tool = .{
    .name = "edit",
    .description = "Edit a file by replacing exact text. The oldText must match exactly (including whitespace). Use this for precise, surgical edits.",
    .schema =
    \\{"type":"object","properties":{"path":{"type":"string","description":"Path to the file to edit (relative or absolute)"},"oldText":{"type":"string","description":"Exact text to find and replace (must match exactly)"},"newText":{"type":"string","description":"New text to replace the old text with"}},"required":["path","oldText","newText"]}
    ,
    .max_output_bytes = max_bytes,
    .invokeFn = invoke,
};

// ---- Tests ----

test "edit replaces exact text" {
    const a = std.testing.allocator;
    const io = std.testing.io;
    const tmp_path = "phoenix_test_edit.txt";
    try std.Io.Dir.cwd().writeFile(io, .{ .sub_path = tmp_path, .data = "hello world\nfoo bar baz\n" });
    defer std.Io.Dir.cwd().deleteFile(io, tmp_path) catch {};

    const args =
        \\{"path":"phoenix_test_edit.txt","oldText":"foo bar","newText":"FOO BAR"}
    ;
    const r = try edit_tool.invoke(io, args, a);
    defer a.free(r.output);
    try std.testing.expect(!r.is_error);
    try std.testing.expect(std.mem.indexOf(u8, r.output, "edited") != null);

    const back = try std.Io.Dir.cwd().readFileAlloc(io, tmp_path, a, .limited(128));
    defer a.free(back);
    try std.testing.expectEqualStrings("hello world\nFOO BAR baz\n", back);
}

test "edit returns error when oldText not found" {
    const a = std.testing.allocator;
    const io = std.testing.io;
    const tmp_path = "phoenix_test_edit_missing.txt";
    try std.Io.Dir.cwd().writeFile(io, .{ .sub_path = tmp_path, .data = "nothing matches here\n" });
    defer std.Io.Dir.cwd().deleteFile(io, tmp_path) catch {};

    const args =
        \\{"path":"phoenix_test_edit_missing.txt","oldText":"absent","newText":"present"}
    ;
    const r = try edit_tool.invoke(io, args, a);
    defer a.free(r.output);
    try std.testing.expect(r.is_error);
    try std.testing.expect(std.mem.indexOf(u8, r.output, "oldText not found") != null);
}

test "edit empty oldText is error" {
    const a = std.testing.allocator;
    const io = std.testing.io;
    const args =
        \\{"path":"/tmp/x","oldText":"","newText":"y"}
    ;
    const r = try edit_tool.invoke(io, args, a);
    defer a.free(r.output);
    try std.testing.expect(r.is_error);
    try std.testing.expect(std.mem.indexOf(u8, r.output, "oldText") != null);
}
