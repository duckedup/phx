/// Wraps `core.Session` with the persistence metadata the RPC server needs:
/// session id, derived display name, created/updated timestamps, and the
/// counters that drive auto-compaction.
///
/// The wrapper centralizes the "commit + persist" rhythm: every code path that
/// adds a message goes through this struct so messages.jsonl + state.json are
/// always in lockstep with the in-memory history. Persistence failures are
/// logged but don't break the agent loop — auditability is best-effort, the
/// conversation is the source of truth.
const std = @import("std");
const core = @import("phoenix_core");

const Session = core.Session;
const Message = core.Message;
const Role = core.Role;
const ToolCall = core.ToolCall;
const ToolResult = core.ToolResult;

pub const ActiveSession = struct {
    /// Session-id-counter — only used as the numeric id inside core.Session.
    /// The persistence id (the on-disk directory name) is `disk_id` below.
    seq: u64,
    session: Session,

    gpa: std.mem.Allocator,
    io: std.Io,

    /// `~/.phoenix` parent path. May be null (HOME unresolved or tests); when
    /// null, persistence is silently skipped.
    home: ?[]const u8,
    /// Project slug. Same null-skip semantics as `home`.
    project: ?[]const u8,

    /// On-disk directory name, allocated. Generated lazily on first commit so
    /// new sessions don't litter the filesystem until the user actually says
    /// something.
    disk_id: ?[]u8,
    /// Display name derived from the first user message.
    name: ?[]u8,
    /// Active provider kind/model captured at first commit, for state.json.
    provider_kind: ?[]u8,
    model: ?[]u8,

    created_at: i64,
    updated_at: i64,
    input_tokens: u64,
    output_tokens: u64,

    /// Most recent provider input_tokens count (the running context size that
    /// auto-compaction watches).
    last_input_tokens: u32,

    pub fn init(
        gpa: std.mem.Allocator,
        io: std.Io,
        home: ?[]const u8,
        project: ?[]const u8,
    ) ActiveSession {
        return .{
            .seq = 1,
            .session = Session.init(gpa, 1),
            .gpa = gpa,
            .io = io,
            .home = home,
            .project = project,
            .disk_id = null,
            .name = null,
            .provider_kind = null,
            .model = null,
            .created_at = nowSeconds(),
            .updated_at = 0,
            .input_tokens = 0,
            .output_tokens = 0,
            .last_input_tokens = 0,
        };
    }

    pub fn deinit(self: *ActiveSession) void {
        self.session.deinit();
        if (self.disk_id) |s| self.gpa.free(s);
        if (self.name) |s| self.gpa.free(s);
        if (self.provider_kind) |s| self.gpa.free(s);
        if (self.model) |s| self.gpa.free(s);
        self.disk_id = null;
        self.name = null;
        self.provider_kind = null;
        self.model = null;
    }

    /// True iff persistence is wired up. False inside tests with home=null.
    pub fn canPersist(self: *const ActiveSession) bool {
        return self.home != null and self.project != null;
    }

    /// Overwrite the captured provider info. Called once per turn from the
    /// server so state.json reflects whatever's active right now.
    pub fn setProvider(self: *ActiveSession, kind: []const u8, model: []const u8) !void {
        if (self.provider_kind) |s| self.gpa.free(s);
        if (self.model) |s| self.gpa.free(s);
        self.provider_kind = try self.gpa.dupe(u8, kind);
        self.model = try self.gpa.dupe(u8, model);
    }

    /// Lazy generate the disk id, ensure the directory exists, derive the
    /// display name from `first_user` if not yet set. Idempotent.
    pub fn ensureDisk(self: *ActiveSession, first_user: []const u8) !void {
        if (!self.canPersist()) return;
        if (self.disk_id == null) {
            self.disk_id = try core.session_store.generateId(self.gpa);
            try core.session_store.ensureDir(
                self.io,
                self.gpa,
                self.home.?,
                self.project.?,
                self.disk_id.?,
            );
        }
        if (self.name == null and first_user.len > 0) {
            self.name = try core.session_store.deriveName(self.gpa, first_user);
        }
    }

    /// Write messages.jsonl + state.json. Best-effort: errors are logged but
    /// not propagated.
    pub fn persist(self: *ActiveSession) void {
        if (!self.canPersist()) return;
        const id = self.disk_id orelse return;
        const home = self.home.?;
        const project = self.project.?;

        self.updated_at = nowSeconds();

        core.session_store.writeMessages(
            self.io,
            self.gpa,
            home,
            project,
            id,
            self.session.messages.items,
        ) catch |err| {
            std.log.warn("active_session: persist messages failed: {s}", .{@errorName(err)});
        };

        core.session_store.writeState(
            self.io,
            self.gpa,
            home,
            project,
            id,
            .{
                .id = id,
                .name = self.name orelse "(unnamed)",
                .created_at = self.created_at,
                .updated_at = self.updated_at,
                .message_count = @intCast(self.session.messages.items.len),
                .input_tokens = self.input_tokens,
                .output_tokens = self.output_tokens,
                .provider_kind = self.provider_kind orelse "",
                .model = self.model orelse "",
            },
        ) catch |err| {
            std.log.warn("active_session: persist state failed: {s}", .{@errorName(err)});
        };
    }

    pub fn appendUser(self: *ActiveSession, content: []const u8) !void {
        try self.ensureDisk(content);
        try self.session.addMessage(.user, content);
        self.persist();
    }

    pub fn appendAssistant(self: *ActiveSession, content: []const u8) !void {
        try self.session.addMessage(.assistant, content);
        self.persist();
    }

    pub fn appendSystem(self: *ActiveSession, content: []const u8) !void {
        try self.session.addMessage(.system, content);
        self.persist();
    }

    pub fn appendToolCall(self: *ActiveSession, tc: ToolCall) !void {
        try self.session.addToolCall(tc);
        self.persist();
    }

    pub fn appendToolResult(self: *ActiveSession, tr: ToolResult) !void {
        try self.session.addToolResult(tr);
        self.persist();
    }

    /// Save the most recent provider usage. The server calls this at the end
    /// of each round so `last_input_tokens` drives auto-compaction.
    pub fn recordUsage(self: *ActiveSession, usage: core.Usage) void {
        self.last_input_tokens = usage.input_tokens;
        self.input_tokens +%= usage.input_tokens;
        self.output_tokens +%= usage.output_tokens;
    }

    /// Persist (if non-empty) and replace the in-memory session with a fresh
    /// empty one. Disk id resets so the next user message starts a new file.
    pub fn clear(self: *ActiveSession) !void {
        if (self.session.messages.items.len > 0) self.persist();
        self.session.deinit();
        self.seq += 1;
        self.session = Session.init(self.gpa, self.seq);

        if (self.disk_id) |s| self.gpa.free(s);
        if (self.name) |s| self.gpa.free(s);
        self.disk_id = null;
        self.name = null;
        self.created_at = nowSeconds();
        self.updated_at = 0;
        self.input_tokens = 0;
        self.output_tokens = 0;
        self.last_input_tokens = 0;
    }

    /// Apply the truncate compactor: keep all leading .system messages
    /// (pinned head) and the last `tail_user_turns` user-rooted segments.
    /// Drop everything in between. Returns the number of messages dropped.
    ///
    /// A "user-rooted segment" starts at a .user message and runs through
    /// every following message up to (but not including) the next .user
    /// message. Tool calls and tool results are kept with their parent turn.
    pub fn truncate(self: *ActiveSession, tail_user_turns: u32) !u32 {
        const items = self.session.messages.items;
        if (items.len == 0) return 0;

        // 1. Find the head_end — first non-system message.
        var head_end: usize = 0;
        while (head_end < items.len and items[head_end].role == .system) : (head_end += 1) {}

        // 2. Find the indices of the last `tail_user_turns` user messages.
        var user_indices: std.ArrayList(usize) = .empty;
        defer user_indices.deinit(self.gpa);
        var idx: usize = items.len;
        while (idx > head_end) {
            idx -= 1;
            if (items[idx].role == .user) {
                try user_indices.append(self.gpa, idx);
                if (user_indices.items.len >= tail_user_turns) break;
            }
        }

        // If there are fewer user messages than the tail size, nothing to drop.
        const tail_start: usize = if (user_indices.items.len > 0)
            user_indices.items[user_indices.items.len - 1]
        else
            items.len;

        if (tail_start <= head_end) return 0; // already minimal

        // 3. Free messages in [head_end, tail_start), retain head + tail.
        const drop_count: u32 = @intCast(tail_start - head_end);
        if (drop_count == 0) return 0;

        // Free owned slices for the dropped range.
        for (items[head_end..tail_start]) |m| {
            if (m.content.len > 0) self.gpa.free(m.content);
            if (m.tool_call) |tc| {
                self.gpa.free(tc.id);
                self.gpa.free(tc.name);
                self.gpa.free(tc.args_json);
            }
            if (m.tool_result) |tr| {
                self.gpa.free(tr.id);
                self.gpa.free(tr.output);
            }
        }

        // Compact in place: shift tail down over the dropped range.
        const new_len = head_end + (items.len - tail_start);
        std.mem.copyForwards(
            Message,
            items[head_end..new_len],
            items[tail_start..items.len],
        );
        self.session.messages.shrinkRetainingCapacity(new_len);

        self.persist();
        return drop_count;
    }

    /// Replace the session with one loaded from disk. The previous session,
    /// if non-empty, is persisted before being dropped so the user doesn't
    /// silently lose work.
    pub fn replaceWith(
        self: *ActiveSession,
        target_id: []const u8,
    ) !u32 {
        if (!self.canPersist()) return error.NoPersistence;
        const home = self.home.?;
        const project = self.project.?;

        if (self.session.messages.items.len > 0) self.persist();

        const loaded = try core.session_store.loadMessages(self.io, self.gpa, home, project, target_id);
        // We hand ownership of each Message struct off to the new session;
        // shrink to zero before deinit so freeMessages doesn't double-free.
        defer {
            self.gpa.free(loaded.messages);
        }

        // Try to read the state.json so we can populate name/created_at/etc.
        var state_name: []const u8 = "";
        var state_created: i64 = nowSeconds();
        var state_input: u64 = 0;
        var state_output: u64 = 0;
        var state_provider: []const u8 = "";
        var state_model: []const u8 = "";
        const state_dir = try core.session_store.sessionDir(self.gpa, home, project, target_id);
        defer self.gpa.free(state_dir);
        const state_path = try std.fs.path.join(self.gpa, &.{ state_dir, "state.json" });
        defer self.gpa.free(state_path);
        const state_raw = std.Io.Dir.cwd().readFileAlloc(
            self.io,
            state_path,
            self.gpa,
            .limited(64 * 1024),
        ) catch null;
        var state_arena = std.heap.ArenaAllocator.init(self.gpa);
        defer state_arena.deinit();
        if (state_raw) |raw| {
            defer self.gpa.free(raw);
            const parsed = std.json.parseFromSliceLeaky(std.json.Value, state_arena.allocator(), raw, .{}) catch null;
            if (parsed) |v| if (v == .object) {
                if (v.object.get("name")) |x| if (x == .string) {
                    state_name = x.string;
                };
                if (v.object.get("created_at")) |x| if (x == .integer) {
                    state_created = x.integer;
                };
                if (v.object.get("input_tokens")) |x| if (x == .integer) {
                    state_input = @intCast(@max(@as(i64, 0), x.integer));
                };
                if (v.object.get("output_tokens")) |x| if (x == .integer) {
                    state_output = @intCast(@max(@as(i64, 0), x.integer));
                };
                if (v.object.get("provider_kind")) |x| if (x == .string) {
                    state_provider = x.string;
                };
                if (v.object.get("model")) |x| if (x == .string) {
                    state_model = x.string;
                };
            };
        }

        // Tear down the old in-memory session and rebuild with the loaded
        // messages. We swap the underlying ArrayList contents directly to
        // avoid re-duping every string.
        self.session.deinit();
        self.seq += 1;
        self.session = Session.init(self.gpa, self.seq);

        try self.session.messages.ensureTotalCapacity(self.gpa, loaded.messages.len);
        for (loaded.messages) |m| {
            self.session.messages.appendAssumeCapacity(m);
        }

        if (self.disk_id) |s| self.gpa.free(s);
        self.disk_id = try self.gpa.dupe(u8, target_id);

        if (self.name) |s| self.gpa.free(s);
        self.name = if (state_name.len > 0) try self.gpa.dupe(u8, state_name) else null;

        if (self.provider_kind) |s| self.gpa.free(s);
        if (self.model) |s| self.gpa.free(s);
        self.provider_kind = if (state_provider.len > 0) try self.gpa.dupe(u8, state_provider) else null;
        self.model = if (state_model.len > 0) try self.gpa.dupe(u8, state_model) else null;

        self.created_at = state_created;
        self.updated_at = nowSeconds();
        self.input_tokens = state_input;
        self.output_tokens = state_output;
        self.last_input_tokens = 0;

        return @intCast(self.session.messages.items.len);
    }
};

fn nowSeconds() i64 {
    var ts: std.c.timespec = undefined;
    if (std.c.clock_gettime(std.c.CLOCK.REALTIME, &ts) != 0) return 0;
    return @intCast(ts.sec);
}

// ---- Tests ----

const testing = std.testing;

test "ActiveSession: init/deinit no leaks" {
    var s = ActiveSession.init(testing.allocator, testing.io, null, null);
    defer s.deinit();
    try testing.expect(!s.canPersist());
}

test "ActiveSession: append + clear (no persistence)" {
    var s = ActiveSession.init(testing.allocator, testing.io, null, null);
    defer s.deinit();

    try s.appendUser("hello");
    try s.appendAssistant("hi");
    try testing.expectEqual(@as(usize, 2), s.session.messages.items.len);

    try s.clear();
    try testing.expectEqual(@as(usize, 0), s.session.messages.items.len);
}

test "ActiveSession: truncate keeps system head + last user turn" {
    var s = ActiveSession.init(testing.allocator, testing.io, null, null);
    defer s.deinit();

    try s.session.addMessage(.system, "rules");
    try s.session.addMessage(.user, "old user 1");
    try s.session.addMessage(.assistant, "old reply 1");
    try s.session.addMessage(.user, "old user 2");
    try s.session.addMessage(.assistant, "old reply 2");
    try s.session.addMessage(.user, "current user");
    try s.session.addMessage(.assistant, "current reply");

    const dropped = try s.truncate(1);
    try testing.expect(dropped > 0);

    // After truncate: [system, current user, current reply]
    try testing.expectEqual(@as(usize, 3), s.session.messages.items.len);
    try testing.expectEqual(core.Role.system, s.session.messages.items[0].role);
    try testing.expectEqual(core.Role.user, s.session.messages.items[1].role);
    try testing.expectEqualStrings("current user", s.session.messages.items[1].content);
    try testing.expectEqual(core.Role.assistant, s.session.messages.items[2].role);
}

test "ActiveSession: truncate is no-op when within tail" {
    var s = ActiveSession.init(testing.allocator, testing.io, null, null);
    defer s.deinit();

    try s.session.addMessage(.system, "rules");
    try s.session.addMessage(.user, "u1");
    try s.session.addMessage(.assistant, "a1");

    const dropped = try s.truncate(3);
    try testing.expectEqual(@as(u32, 0), dropped);
    try testing.expectEqual(@as(usize, 3), s.session.messages.items.len);
}

test "ActiveSession: persist + replaceWith round-trip" {
    const a = testing.allocator;
    var tmp = testing.tmpDir(.{});
    defer tmp.cleanup();
    var buf: [std.Io.Dir.max_path_bytes]u8 = undefined;
    const ptr = std.c.getcwd(&buf, buf.len) orelse return error.CwdError;
    const cwd_path = std.mem.sliceTo(ptr, 0);
    const home = try std.fs.path.join(a, &.{ cwd_path, ".zig-cache", "tmp", &tmp.sub_path, "home" });
    defer a.free(home);
    try std.Io.Dir.cwd().createDirPath(testing.io, home);

    var s = ActiveSession.init(a, testing.io, home, "p");
    defer s.deinit();

    try s.appendUser("hello");
    try s.appendAssistant("hi");
    const saved_id = try a.dupe(u8, s.disk_id.?);
    defer a.free(saved_id);

    try s.clear();
    try testing.expectEqual(@as(usize, 0), s.session.messages.items.len);

    const restored = try s.replaceWith(saved_id);
    try testing.expectEqual(@as(u32, 2), restored);
    try testing.expectEqualStrings("hello", s.session.messages.items[0].content);
    try testing.expectEqualStrings("hi", s.session.messages.items[1].content);
}
