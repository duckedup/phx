/// Session persistence under the user's home directory.
///
/// Layout:
///   ~/.phoenix/sessions/{project}/{session_id}/messages.jsonl
///   ~/.phoenix/sessions/{project}/{session_id}/state.json
///
/// `{project}` is the basename of the launch directory, sanitized to a flat
/// filesystem-safe slug. `{session_id}` is a sortable timestamp + short random
/// suffix.
///
/// Session resume reads `messages.jsonl` and replays it into a `Session`. We
/// rewrite the whole file on every commit rather than O_APPEND-ing because
/// session sizes are bounded by the model context window — even a 200k-token
/// conversation is on the order of 1 MB on disk, and a full rewrite keeps the
/// implementation portable across the Io abstraction.
const std = @import("std");
const message = @import("message.zig");
const Message = message.Message;
const Role = message.Role;

pub const max_name_chars: usize = 60;

/// Metadata returned by `list` for the resume picker.
pub const Entry = struct {
    /// Session id (the directory name). Owned by the caller's allocator.
    id: []const u8,
    /// Display name derived from the first user message. Owned by allocator.
    name: []const u8,
    /// Unix epoch seconds. 0 if state.json was unreadable / partial.
    updated_at: i64,
    /// Message count from state.json.
    message_count: u32,
};

pub const State = struct {
    id: []const u8,
    name: []const u8,
    created_at: i64,
    updated_at: i64,
    message_count: u32,
    input_tokens: u64 = 0,
    output_tokens: u64 = 0,
    provider_kind: []const u8 = "",
    model: []const u8 = "",
};

/// Compute the project slug from a working-directory path. Strips any trailing
/// '/'es, takes the basename, and replaces unsafe bytes (/, \0, control chars)
/// with '_'. Empty / root inputs become "default".
pub fn projectSlug(allocator: std.mem.Allocator, cwd: []const u8) ![]u8 {
    var trimmed = cwd;
    while (trimmed.len > 1 and trimmed[trimmed.len - 1] == '/') {
        trimmed = trimmed[0 .. trimmed.len - 1];
    }
    const base = std.fs.path.basename(trimmed);
    if (base.len == 0) return try allocator.dupe(u8, "default");

    var buf: std.ArrayList(u8) = .empty;
    errdefer buf.deinit(allocator);
    for (base) |c| {
        const ok = (c >= 'a' and c <= 'z') or
            (c >= 'A' and c <= 'Z') or
            (c >= '0' and c <= '9') or
            c == '-' or c == '_' or c == '.';
        try buf.append(allocator, if (ok) c else '_');
    }
    if (buf.items.len == 0) {
        buf.deinit(allocator);
        return try allocator.dupe(u8, "default");
    }
    return try buf.toOwnedSlice(allocator);
}

/// Read the current wall clock as `(unix_seconds, nanoseconds)`. The split
/// keeps the nonce derivable from the same syscall that gave us the id prefix.
fn now() struct { sec: i64, nsec: i64 } {
    var ts: std.c.timespec = undefined;
    if (std.c.clock_gettime(std.c.CLOCK.REALTIME, &ts) != 0) return .{ .sec = 0, .nsec = 0 };
    return .{ .sec = @intCast(ts.sec), .nsec = @intCast(ts.nsec) };
}

/// Generate a fresh session id: `YYYYMMDD-HHMMSS-XXXX` where XXXX is hex from
/// the lower 16 bits of a wall-clock-derived nonce. Sortable by id ascending,
/// so directory listings come back oldest-first naturally.
pub fn generateId(allocator: std.mem.Allocator) ![]u8 {
    const t = now();
    const ymdhms = secondsToYMDHMS(t.sec);
    const nonce: u16 = @truncate(@as(u64, @intCast(t.nsec)) ^ (@as(u64, @intCast(t.nsec)) >> 16));
    return std.fmt.allocPrint(
        allocator,
        "{d:0>4}{d:0>2}{d:0>2}-{d:0>2}{d:0>2}{d:0>2}-{x:0>4}",
        .{
            ymdhms.year,
            ymdhms.month,
            ymdhms.day,
            ymdhms.hour,
            ymdhms.minute,
            ymdhms.second,
            nonce,
        },
    );
}

/// Trim and sanitize the first user message into a display name. Collapses
/// whitespace and caps length at `max_name_chars` (with a trailing ellipsis).
pub fn deriveName(allocator: std.mem.Allocator, first_user_message: []const u8) ![]u8 {
    var buf: std.ArrayList(u8) = .empty;
    errdefer buf.deinit(allocator);

    var prev_space = true;
    for (first_user_message) |c| {
        const is_space = c == ' ' or c == '\t' or c == '\n' or c == '\r';
        if (is_space) {
            if (!prev_space and buf.items.len > 0) try buf.append(allocator, ' ');
            prev_space = true;
        } else {
            try buf.append(allocator, c);
            prev_space = false;
        }
        if (buf.items.len >= max_name_chars) break;
    }
    while (buf.items.len > 0 and buf.items[buf.items.len - 1] == ' ') {
        _ = buf.pop();
    }

    if (buf.items.len == 0) {
        buf.deinit(allocator);
        return try allocator.dupe(u8, "(empty)");
    }
    if (first_user_message.len > max_name_chars) {
        try buf.appendSlice(allocator, "…");
    }
    return try buf.toOwnedSlice(allocator);
}

/// Build the absolute path to the directory that holds all sessions for one
/// project: `~/.phoenix/sessions/{project}/`.
pub fn projectDir(allocator: std.mem.Allocator, home: []const u8, project: []const u8) ![]u8 {
    return std.fs.path.join(allocator, &.{ home, ".phoenix", "sessions", project });
}

pub fn sessionDir(
    allocator: std.mem.Allocator,
    home: []const u8,
    project: []const u8,
    session_id: []const u8,
) ![]u8 {
    return std.fs.path.join(allocator, &.{ home, ".phoenix", "sessions", project, session_id });
}

/// Ensure `~/.phoenix/sessions/{project}/{session_id}/` exists.
pub fn ensureDir(
    io: std.Io,
    allocator: std.mem.Allocator,
    home: []const u8,
    project: []const u8,
    session_id: []const u8,
) !void {
    const path = try sessionDir(allocator, home, project, session_id);
    defer allocator.free(path);
    try std.Io.Dir.cwd().createDirPath(io, path);
}

/// Write the full message history as JSONL, replacing whatever was there.
/// Caller is responsible for calling `ensureDir` first.
pub fn writeMessages(
    io: std.Io,
    allocator: std.mem.Allocator,
    home: []const u8,
    project: []const u8,
    session_id: []const u8,
    messages: []const Message,
) !void {
    const dir = try sessionDir(allocator, home, project, session_id);
    defer allocator.free(dir);
    const path = try std.fs.path.join(allocator, &.{ dir, "messages.jsonl" });
    defer allocator.free(path);

    var buf: std.ArrayList(u8) = .empty;
    defer buf.deinit(allocator);
    for (messages) |msg| {
        try encodeMessage(&buf, allocator, msg);
        try buf.append(allocator, '\n');
    }

    try std.Io.Dir.cwd().writeFile(io, .{
        .sub_path = path,
        .data = buf.items,
    });
}

pub fn writeState(
    io: std.Io,
    allocator: std.mem.Allocator,
    home: []const u8,
    project: []const u8,
    session_id: []const u8,
    state: State,
) !void {
    const dir = try sessionDir(allocator, home, project, session_id);
    defer allocator.free(dir);
    const path = try std.fs.path.join(allocator, &.{ dir, "state.json" });
    defer allocator.free(path);

    var buf: std.ArrayList(u8) = .empty;
    defer buf.deinit(allocator);

    try buf.appendSlice(allocator, "{\n  \"id\": ");
    try writeJsonString(&buf, allocator, state.id);
    try buf.appendSlice(allocator, ",\n  \"name\": ");
    try writeJsonString(&buf, allocator, state.name);
    try buf.print(allocator, ",\n  \"created_at\": {d}", .{state.created_at});
    try buf.print(allocator, ",\n  \"updated_at\": {d}", .{state.updated_at});
    try buf.print(allocator, ",\n  \"message_count\": {d}", .{state.message_count});
    try buf.print(allocator, ",\n  \"input_tokens\": {d}", .{state.input_tokens});
    try buf.print(allocator, ",\n  \"output_tokens\": {d}", .{state.output_tokens});
    try buf.appendSlice(allocator, ",\n  \"provider_kind\": ");
    try writeJsonString(&buf, allocator, state.provider_kind);
    try buf.appendSlice(allocator, ",\n  \"model\": ");
    try writeJsonString(&buf, allocator, state.model);
    try buf.appendSlice(allocator, "\n}\n");

    try std.Io.Dir.cwd().writeFile(io, .{
        .sub_path = path,
        .data = buf.items,
    });
}

/// List sessions for a project, sorted newest-first by `updated_at`.
/// Returns an empty slice if the project directory does not exist.
pub fn list(
    io: std.Io,
    allocator: std.mem.Allocator,
    home: []const u8,
    project: []const u8,
) ![]Entry {
    const path = try projectDir(allocator, home, project);
    defer allocator.free(path);

    var entries: std.ArrayList(Entry) = .empty;
    errdefer {
        for (entries.items) |e| {
            allocator.free(e.id);
            allocator.free(e.name);
        }
        entries.deinit(allocator);
    }

    // Must open with .iterate = true; otherwise dirReadLinux panics
    // BADF on the first getdents (the FD lacks iteration permission).
    var ids: std.ArrayList([]u8) = .empty;
    defer {
        for (ids.items) |s| allocator.free(s);
        ids.deinit(allocator);
    }
    {
        var dir = std.Io.Dir.cwd().openDir(io, path, .{ .iterate = true }) catch |err| switch (err) {
            error.FileNotFound => return entries.toOwnedSlice(allocator),
            else => return err,
        };
        defer dir.close(io);

        var it = dir.iterate();
        while (try it.next(io)) |entry| {
            if (entry.kind != .directory) continue;
            if (entry.name.len == 0 or entry.name[0] == '.') continue;
            const dup = try allocator.dupe(u8, entry.name);
            errdefer allocator.free(dup);
            try ids.append(allocator, dup);
        }
    }

    for (ids.items) |session_id| {
        const id = try allocator.dupe(u8, session_id);
        errdefer allocator.free(id);

        const state_path = try std.fs.path.join(allocator, &.{ path, session_id, "state.json" });
        defer allocator.free(state_path);

        const state_raw = std.Io.Dir.cwd().readFileAlloc(
            io,
            state_path,
            allocator,
            .limited(64 * 1024),
        ) catch |err| switch (err) {
            error.FileNotFound => {
                // Directory exists but no state.json yet. Surface it with a
                // placeholder name so the user can still pick / delete it.
                const name = try allocator.dupe(u8, "(no state)");
                try entries.append(allocator, .{
                    .id = id,
                    .name = name,
                    .updated_at = 0,
                    .message_count = 0,
                });
                continue;
            },
            else => return err,
        };
        defer allocator.free(state_raw);

        const parsed = std.json.parseFromSlice(std.json.Value, allocator, state_raw, .{}) catch {
            const name = try allocator.dupe(u8, "(corrupt state)");
            try entries.append(allocator, .{
                .id = id,
                .name = name,
                .updated_at = 0,
                .message_count = 0,
            });
            continue;
        };
        defer parsed.deinit();

        var name_text: []const u8 = "(unnamed)";
        var updated: i64 = 0;
        var msg_count: u32 = 0;
        if (parsed.value == .object) {
            if (parsed.value.object.get("name")) |v| if (v == .string) {
                name_text = v.string;
            };
            if (parsed.value.object.get("updated_at")) |v| if (v == .integer) {
                updated = v.integer;
            };
            if (parsed.value.object.get("message_count")) |v| if (v == .integer) {
                msg_count = @intCast(@max(@as(i64, 0), v.integer));
            };
        }

        const name = try allocator.dupe(u8, name_text);
        try entries.append(allocator, .{
            .id = id,
            .name = name,
            .updated_at = updated,
            .message_count = msg_count,
        });
    }

    // Newest first: sort descending by updated_at, then by id desc as a tie-breaker.
    const items = try entries.toOwnedSlice(allocator);
    std.mem.sort(Entry, items, {}, entryNewerFirst);
    return items;
}

fn entryNewerFirst(_: void, a: Entry, b: Entry) bool {
    if (a.updated_at != b.updated_at) return a.updated_at > b.updated_at;
    return std.mem.order(u8, a.id, b.id) == .gt;
}

/// Free the slice returned by `list`.
pub fn freeList(allocator: std.mem.Allocator, items: []Entry) void {
    for (items) |e| {
        allocator.free(e.id);
        allocator.free(e.name);
    }
    allocator.free(items);
}

/// Read messages.jsonl and rebuild the in-memory message list. Each appended
/// message dupes its strings on `allocator`; caller owns them.
pub const LoadedMessages = struct {
    /// Heap-allocated messages with all string fields freshly owned.
    messages: []Message,
    allocator: std.mem.Allocator,

    pub fn deinit(self: *LoadedMessages) void {
        for (self.messages) |m| {
            if (m.content.len > 0) self.allocator.free(m.content);
            if (m.tool_call) |tc| {
                self.allocator.free(tc.id);
                self.allocator.free(tc.name);
                self.allocator.free(tc.args_json);
            }
            if (m.tool_result) |tr| {
                self.allocator.free(tr.id);
                self.allocator.free(tr.output);
            }
        }
        self.allocator.free(self.messages);
    }
};

pub fn loadMessages(
    io: std.Io,
    allocator: std.mem.Allocator,
    home: []const u8,
    project: []const u8,
    session_id: []const u8,
) !LoadedMessages {
    const dir = try sessionDir(allocator, home, project, session_id);
    defer allocator.free(dir);
    const path = try std.fs.path.join(allocator, &.{ dir, "messages.jsonl" });
    defer allocator.free(path);

    const raw = try std.Io.Dir.cwd().readFileAlloc(
        io,
        path,
        allocator,
        .limited(8 * 1024 * 1024),
    );
    defer allocator.free(raw);

    var messages: std.ArrayList(Message) = .empty;
    errdefer {
        for (messages.items) |m| {
            if (m.content.len > 0) allocator.free(m.content);
            if (m.tool_call) |tc| {
                allocator.free(tc.id);
                allocator.free(tc.name);
                allocator.free(tc.args_json);
            }
            if (m.tool_result) |tr| {
                allocator.free(tr.id);
                allocator.free(tr.output);
            }
        }
        messages.deinit(allocator);
    }

    var it = std.mem.splitScalar(u8, raw, '\n');
    while (it.next()) |line| {
        const trimmed = std.mem.trim(u8, line, " \t\r");
        if (trimmed.len == 0) continue;
        const msg = try decodeMessage(allocator, trimmed);
        try messages.append(allocator, msg);
    }

    return .{
        .messages = try messages.toOwnedSlice(allocator),
        .allocator = allocator,
    };
}

fn encodeMessage(buf: *std.ArrayList(u8), a: std.mem.Allocator, msg: Message) !void {
    try buf.appendSlice(a, "{\"role\":");
    try writeJsonString(buf, a, @tagName(msg.role));
    try buf.appendSlice(a, ",\"content\":");
    try writeJsonString(buf, a, msg.content);
    if (msg.tool_call) |tc| {
        try buf.appendSlice(a, ",\"tool_call\":{\"id\":");
        try writeJsonString(buf, a, tc.id);
        try buf.appendSlice(a, ",\"name\":");
        try writeJsonString(buf, a, tc.name);
        try buf.appendSlice(a, ",\"args_json\":");
        try writeJsonString(buf, a, tc.args_json);
        try buf.appendSlice(a, "}");
    }
    if (msg.tool_result) |tr| {
        try buf.appendSlice(a, ",\"tool_result\":{\"id\":");
        try writeJsonString(buf, a, tr.id);
        try buf.appendSlice(a, ",\"output\":");
        try writeJsonString(buf, a, tr.output);
        try buf.appendSlice(a, ",\"is_error\":");
        try buf.appendSlice(a, if (tr.is_error) "true" else "false");
        try buf.appendSlice(a, "}");
    }
    try buf.appendSlice(a, "}");
}

fn decodeMessage(allocator: std.mem.Allocator, line: []const u8) !Message {
    var arena = std.heap.ArenaAllocator.init(allocator);
    defer arena.deinit();
    const aa = arena.allocator();

    const parsed = try std.json.parseFromSliceLeaky(std.json.Value, aa, line, .{});
    if (parsed != .object) return error.InvalidSessionLine;

    const role_v = parsed.object.get("role") orelse return error.InvalidSessionLine;
    if (role_v != .string) return error.InvalidSessionLine;
    const role = std.meta.stringToEnum(Role, role_v.string) orelse return error.InvalidSessionLine;

    const content: []const u8 = blk: {
        const v = parsed.object.get("content") orelse break :blk "";
        break :blk if (v == .string) v.string else "";
    };
    const owned_content: []const u8 = if (content.len == 0) "" else try allocator.dupe(u8, content);

    var msg = Message{
        .role = role,
        .content = owned_content,
    };

    if (parsed.object.get("tool_call")) |tc_v| if (tc_v == .object) {
        const id_v = tc_v.object.get("id") orelse return error.InvalidSessionLine;
        const name_v = tc_v.object.get("name") orelse return error.InvalidSessionLine;
        const args_v = tc_v.object.get("args_json") orelse return error.InvalidSessionLine;
        if (id_v != .string or name_v != .string or args_v != .string) return error.InvalidSessionLine;
        msg.tool_call = .{
            .id = try allocator.dupe(u8, id_v.string),
            .name = try allocator.dupe(u8, name_v.string),
            .args_json = try allocator.dupe(u8, args_v.string),
        };
    };
    if (parsed.object.get("tool_result")) |tr_v| if (tr_v == .object) {
        const id_v = tr_v.object.get("id") orelse return error.InvalidSessionLine;
        const out_v = tr_v.object.get("output") orelse return error.InvalidSessionLine;
        if (id_v != .string or out_v != .string) return error.InvalidSessionLine;
        const is_err: bool = blk: {
            const v = tr_v.object.get("is_error") orelse break :blk false;
            break :blk (v == .bool and v.bool);
        };
        msg.tool_result = .{
            .id = try allocator.dupe(u8, id_v.string),
            .output = try allocator.dupe(u8, out_v.string),
            .is_error = is_err,
        };
    };

    return msg;
}

fn writeJsonString(out: *std.ArrayList(u8), a: std.mem.Allocator, s: []const u8) !void {
    const encoded = try std.json.Stringify.valueAlloc(a, s, .{});
    defer a.free(encoded);
    try out.appendSlice(a, encoded);
}

const Ymdhms = struct {
    year: u32,
    month: u8,
    day: u8,
    hour: u8,
    minute: u8,
    second: u8,
};

/// Convert a unix epoch second count to UTC YMDHMS using the proleptic
/// Gregorian calendar. Negative inputs (pre-1970) return year 1970.
fn secondsToYMDHMS(s: i64) Ymdhms {
    const total: i64 = if (s < 0) 0 else s;
    const day_seconds: i64 = 86_400;
    const days: i64 = @divFloor(total, day_seconds);
    const tod: i64 = @mod(total, day_seconds);

    const hour: u8 = @intCast(@divFloor(tod, 3600));
    const minute: u8 = @intCast(@divFloor(@mod(tod, 3600), 60));
    const second: u8 = @intCast(@mod(tod, 60));

    // Algorithm: civil_from_days (Howard Hinnant). Days since 1970-01-01.
    var z: i64 = days + 719_468;
    const era: i64 = if (z >= 0) @divFloor(z, 146_097) else @divFloor(z - 146_096, 146_097);
    const doe: u32 = @intCast(z - era * 146_097);
    const yoe: u32 = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    const y: i64 = @as(i64, yoe) + era * 400;
    const doy: u32 = doe - (365 * yoe + yoe / 4 - yoe / 100);
    const mp: u32 = (5 * doy + 2) / 153;
    const d: u32 = doy - (153 * mp + 2) / 5 + 1;
    const m: u32 = if (mp < 10) mp + 3 else mp - 9;
    const year: i64 = y + @as(i64, if (m <= 2) 1 else 0);
    z = year;
    return .{
        .year = if (z < 0) 1970 else @intCast(z),
        .month = @intCast(m),
        .day = @intCast(d),
        .hour = hour,
        .minute = minute,
        .second = second,
    };
}

// ---- Tests ----

const testing = std.testing;

test "projectSlug: simple basename" {
    const slug = try projectSlug(testing.allocator, "/Users/austin/Projects/phoenix");
    defer testing.allocator.free(slug);
    try testing.expectEqualStrings("phoenix", slug);
}

test "projectSlug: trailing slashes stripped" {
    const slug = try projectSlug(testing.allocator, "/tmp/work//");
    defer testing.allocator.free(slug);
    try testing.expectEqualStrings("work", slug);
}

test "projectSlug: unsafe chars sanitized" {
    const slug = try projectSlug(testing.allocator, "/tmp/foo bar+baz");
    defer testing.allocator.free(slug);
    try testing.expectEqualStrings("foo_bar_baz", slug);
}

test "projectSlug: empty falls back to default" {
    const slug = try projectSlug(testing.allocator, "/");
    defer testing.allocator.free(slug);
    try testing.expectEqualStrings("default", slug);
}

test "deriveName: short message kept verbatim" {
    const name = try deriveName(testing.allocator, "hello world");
    defer testing.allocator.free(name);
    try testing.expectEqualStrings("hello world", name);
}

test "deriveName: collapses whitespace" {
    const name = try deriveName(testing.allocator, "  hello   world  \n");
    defer testing.allocator.free(name);
    try testing.expectEqualStrings("hello world", name);
}

test "deriveName: long message truncated with ellipsis" {
    const long = "a" ** 200;
    const name = try deriveName(testing.allocator, long);
    defer testing.allocator.free(name);
    try testing.expect(std.mem.endsWith(u8, name, "…"));
    // Length is in bytes, not chars; '…' is 3 bytes in UTF-8.
    try testing.expect(name.len <= max_name_chars + 3);
}

test "deriveName: empty falls back" {
    const name = try deriveName(testing.allocator, "   \n\t");
    defer testing.allocator.free(name);
    try testing.expectEqualStrings("(empty)", name);
}

test "generateId is sortable and well-formed" {
    const id1 = try generateId(testing.allocator);
    defer testing.allocator.free(id1);
    // Format: YYYYMMDD-HHMMSS-XXXX  (8 + 1 + 6 + 1 + 4 = 20)
    try testing.expectEqual(@as(usize, 20), id1.len);
    try testing.expectEqual(@as(u8, '-'), id1[8]);
    try testing.expectEqual(@as(u8, '-'), id1[15]);
}

test "secondsToYMDHMS: epoch" {
    const t = secondsToYMDHMS(0);
    try testing.expectEqual(@as(u32, 1970), t.year);
    try testing.expectEqual(@as(u8, 1), t.month);
    try testing.expectEqual(@as(u8, 1), t.day);
}

test "secondsToYMDHMS: known date" {
    // 2026-05-03 00:00:00 UTC = 1777795200
    const t = secondsToYMDHMS(1_777_795_200);
    try testing.expectEqual(@as(u32, 2026), t.year);
    try testing.expectEqual(@as(u8, 5), t.month);
    try testing.expectEqual(@as(u8, 3), t.day);
}

fn tmpHome(allocator: std.mem.Allocator, tmp: *std.testing.TmpDir) ![]u8 {
    var buf: [std.Io.Dir.max_path_bytes]u8 = undefined;
    const ptr = std.c.getcwd(&buf, buf.len) orelse return error.CwdError;
    const cwd_path = std.mem.sliceTo(ptr, 0);
    return try std.fs.path.join(allocator, &.{ cwd_path, ".zig-cache", "tmp", &tmp.sub_path, "home" });
}

test "round-trip: write + read messages" {
    const a = testing.allocator;
    var tmp = testing.tmpDir(.{});
    defer tmp.cleanup();
    const home = try tmpHome(a, &tmp);
    defer a.free(home);
    try std.Io.Dir.cwd().createDirPath(testing.io, home);

    const project = "myproj";
    const session_id = "20260503-120000-aaaa";
    try ensureDir(testing.io, a, home, project, session_id);

    const messages = [_]Message{
        .{ .role = .user, .content = "hello" },
        .{ .role = .assistant, .content = "hi there" },
        .{
            .role = .tool_call,
            .content = "",
            .tool_call = .{ .id = "tu_1", .name = "read", .args_json = "{\"path\":\"x\"}" },
        },
        .{
            .role = .tool_result,
            .content = "",
            .tool_result = .{ .id = "tu_1", .output = "file body", .is_error = false },
        },
    };
    try writeMessages(testing.io, a, home, project, session_id, &messages);

    var loaded = try loadMessages(testing.io, a, home, project, session_id);
    defer loaded.deinit();
    try testing.expectEqual(@as(usize, 4), loaded.messages.len);
    try testing.expectEqual(Role.user, loaded.messages[0].role);
    try testing.expectEqualStrings("hello", loaded.messages[0].content);
    try testing.expectEqualStrings("hi there", loaded.messages[1].content);
    try testing.expect(loaded.messages[2].tool_call != null);
    try testing.expectEqualStrings("read", loaded.messages[2].tool_call.?.name);
    try testing.expect(loaded.messages[3].tool_result != null);
    try testing.expectEqualStrings("file body", loaded.messages[3].tool_result.?.output);
}

test "list: missing project directory returns empty" {
    const a = testing.allocator;
    var tmp = testing.tmpDir(.{});
    defer tmp.cleanup();
    const home = try tmpHome(a, &tmp);
    defer a.free(home);
    try std.Io.Dir.cwd().createDirPath(testing.io, home);

    const items = try list(testing.io, a, home, "nope");
    defer freeList(a, items);
    try testing.expectEqual(@as(usize, 0), items.len);
}

test "list: returns newest-first" {
    const a = testing.allocator;
    var tmp = testing.tmpDir(.{});
    defer tmp.cleanup();
    const home = try tmpHome(a, &tmp);
    defer a.free(home);
    try std.Io.Dir.cwd().createDirPath(testing.io, home);

    const project = "p";
    const ids = [_][]const u8{
        "20260101-120000-aaaa",
        "20260201-120000-bbbb",
        "20260301-120000-cccc",
    };
    for (ids, 0..) |id, i| {
        try ensureDir(testing.io, a, home, project, id);
        try writeState(testing.io, a, home, project, id, .{
            .id = id,
            .name = "n",
            .created_at = 0,
            .updated_at = @as(i64, @intCast(i + 1)) * 1000,
            .message_count = 1,
        });
    }
    const items = try list(testing.io, a, home, project);
    defer freeList(a, items);
    try testing.expectEqual(@as(usize, 3), items.len);
    try testing.expectEqualStrings(ids[2], items[0].id); // newest first
    try testing.expectEqualStrings(ids[0], items[2].id);
}
