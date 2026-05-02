/// JSON utility helpers shared by provider adapters.
///
/// We do not implement a JSON encoder from scratch. `std.json.Stringify` is
/// the workhorse; this file adds thin cross-cutting helpers.
const std = @import("std");

/// Append a JSON-encoded string literal (with surrounding quotes) to `out`.
/// Properly escapes quotes, backslashes, and control characters.
///
/// `out` is a `std.ArrayList(u8)` (Zig 0.16 unmanaged variant); `allocator`
/// is used for growth.
pub fn appendString(out: *std.ArrayList(u8), allocator: std.mem.Allocator, s: []const u8) !void {
    // Use valueAlloc to get the fully-escaped JSON string, then append to out.
    const encoded = try std.json.Stringify.valueAlloc(allocator, s, .{});
    defer allocator.free(encoded);
    try out.appendSlice(allocator, encoded);
}

/// Look up `path` (dot-separated key sequence) in an arbitrary `std.json.Value`,
/// returning the leaf value or null if any segment is missing.
///
/// Example: `dottedLookup(val, "choices.0.delta.content")` descends through
/// object keys "choices", then array index 0, then "delta", then "content".
pub fn dottedLookup(v: std.json.Value, path: []const u8) ?std.json.Value {
    var current = v;
    var it = std.mem.splitScalar(u8, path, '.');
    while (it.next()) |segment| {
        if (segment.len == 0) return null;
        switch (current) {
            .object => |obj| {
                current = obj.get(segment) orelse return null;
            },
            .array => |arr| {
                const idx = std.fmt.parseInt(usize, segment, 10) catch return null;
                if (idx >= arr.items.len) return null;
                current = arr.items[idx];
            },
            else => return null,
        }
    }
    return current;
}

// ---- Tests ----

test "appendString basic" {
    const allocator = std.testing.allocator;
    var out: std.ArrayList(u8) = .empty;
    defer out.deinit(allocator);

    try appendString(&out, allocator, "hello");
    try std.testing.expectEqualStrings("\"hello\"", out.items);
}

test "appendString escapes quotes" {
    const allocator = std.testing.allocator;
    var out: std.ArrayList(u8) = .empty;
    defer out.deinit(allocator);

    try appendString(&out, allocator, "say \"hi\"");
    try std.testing.expectEqualStrings("\"say \\\"hi\\\"\"", out.items);
}

test "appendString escapes backslash" {
    const allocator = std.testing.allocator;
    var out: std.ArrayList(u8) = .empty;
    defer out.deinit(allocator);

    try appendString(&out, allocator, "path\\to\\file");
    try std.testing.expectEqualStrings("\"path\\\\to\\\\file\"", out.items);
}

test "appendString escapes control chars" {
    const allocator = std.testing.allocator;
    var out: std.ArrayList(u8) = .empty;
    defer out.deinit(allocator);

    try appendString(&out, allocator, "line1\nline2\ttab");
    // \n -> \n, \t -> \t in JSON encoding
    const result = out.items;
    try std.testing.expect(std.mem.indexOf(u8, result, "\\n") != null);
    try std.testing.expect(std.mem.indexOf(u8, result, "\\t") != null);
}

test "appendString empty string" {
    const allocator = std.testing.allocator;
    var out: std.ArrayList(u8) = .empty;
    defer out.deinit(allocator);

    try appendString(&out, allocator, "");
    try std.testing.expectEqualStrings("\"\"", out.items);
}

test "dottedLookup simple object key" {
    const allocator = std.testing.allocator;
    const json_str =
        \\{"model":"gpt-4o","usage":{"total_tokens":100}}
    ;
    const parsed = try std.json.parseFromSlice(std.json.Value, allocator, json_str, .{});
    defer parsed.deinit();

    const model = dottedLookup(parsed.value, "model");
    try std.testing.expect(model != null);
    try std.testing.expectEqualStrings("gpt-4o", model.?.string);

    const total = dottedLookup(parsed.value, "usage.total_tokens");
    try std.testing.expect(total != null);
    try std.testing.expectEqual(@as(i64, 100), total.?.integer);
}

test "dottedLookup missing key returns null" {
    const allocator = std.testing.allocator;
    const json_str = "{\"a\":1}";
    const parsed = try std.json.parseFromSlice(std.json.Value, allocator, json_str, .{});
    defer parsed.deinit();

    const result = dottedLookup(parsed.value, "b");
    try std.testing.expect(result == null);

    const nested = dottedLookup(parsed.value, "a.b");
    try std.testing.expect(nested == null);
}

test "dottedLookup array index" {
    const allocator = std.testing.allocator;
    const json_str =
        \\{"choices":[{"delta":{"content":"hello"}}]}
    ;
    const parsed = try std.json.parseFromSlice(std.json.Value, allocator, json_str, .{});
    defer parsed.deinit();

    const content = dottedLookup(parsed.value, "choices.0.delta.content");
    try std.testing.expect(content != null);
    try std.testing.expectEqualStrings("hello", content.?.string);
}

test "dottedLookup out of bounds array" {
    const allocator = std.testing.allocator;
    const json_str = "{\"arr\":[1,2,3]}";
    const parsed = try std.json.parseFromSlice(std.json.Value, allocator, json_str, .{});
    defer parsed.deinit();

    const result = dottedLookup(parsed.value, "arr.5");
    try std.testing.expect(result == null);
}
