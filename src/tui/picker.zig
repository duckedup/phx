const std = @import("std");
const vaxis = @import("vaxis");
const commands = @import("commands");
const theme_mod = @import("theme.zig");

/// Inline picker shown under the input box. Supports the two flavors Phoenix
/// currently surfaces — model selection (/model) and saved-session selection
/// (/resume). The mode is fixed at init time; the TUI inspects it on Enter
/// to know which RPC to issue.
pub const Picker = struct {
    arena: std.heap.ArenaAllocator,
    title: []const u8,
    cursor: usize,
    mode: Mode,

    /// Pre-formatted display strings, one per choice in the *current* mode
    /// (or per filtered entry for command_complete). These live on
    /// `arena` so they outlive each `vx.render` call — vaxis stores grapheme
    /// slices by reference, not by copy.
    rendered: [][]const u8,

    pub const Mode = union(enum) {
        model: []const commands.ModelChoice,
        session: []const commands.SessionChoice,
        /// Inline autocomplete shown while the user is typing a slash command.
        /// `all` is the full sorted command set (owned by the picker arena);
        /// `filtered` is a slice of pointers into `all` matching the current
        /// prefix and is rebuilt by `setCommandFilter`.
        command_complete: CommandComplete,
        theme: []const theme_mod.ThemeEntry,
    };

    pub const CommandComplete = struct {
        all: []commands.CommandInfo,
        filtered: []u32,
    };

    pub fn initModel(
        gpa: std.mem.Allocator,
        p: commands.ModelPicker,
    ) !Picker {
        var arena = std.heap.ArenaAllocator.init(gpa);
        errdefer arena.deinit();
        const a = arena.allocator();

        const title = try a.dupe(u8, p.title);
        const buf = try a.alloc(commands.ModelChoice, p.choices.len);
        var initial_cursor: usize = 0;
        for (p.choices, 0..) |c, i| {
            buf[i] = .{
                .provider_index = c.provider_index,
                .kind = c.kind,
                .model = try a.dupe(u8, c.model),
                .is_active = c.is_active,
            };
            if (c.is_active) initial_cursor = i;
        }
        const rendered = try a.alloc([]const u8, buf.len);
        for (buf, 0..) |c, i| {
            const marker: []const u8 = if (c.is_active) "*" else " ";
            rendered[i] = try std.fmt.allocPrint(a, " {s} {s} ({s})", .{ marker, c.model, @tagName(c.kind) });
        }
        return .{
            .arena = arena,
            .title = title,
            .cursor = initial_cursor,
            .mode = .{ .model = buf },
            .rendered = rendered,
        };
    }

    pub fn initSession(
        gpa: std.mem.Allocator,
        p: commands.SessionPicker,
    ) !Picker {
        var arena = std.heap.ArenaAllocator.init(gpa);
        errdefer arena.deinit();
        const a = arena.allocator();

        const title = try a.dupe(u8, p.title);
        const buf = try a.alloc(commands.SessionChoice, p.choices.len);
        for (p.choices, 0..) |c, i| {
            buf[i] = .{
                .id = try a.dupe(u8, c.id),
                .name = try a.dupe(u8, c.name),
                .updated_at = c.updated_at,
                .message_count = c.message_count,
            };
        }
        const rendered = try a.alloc([]const u8, buf.len);
        for (buf, 0..) |c, i| {
            rendered[i] = try std.fmt.allocPrint(a, "   {s}  ({d} msgs)", .{ c.name, c.message_count });
        }
        return .{
            .arena = arena,
            .title = title,
            .cursor = 0,
            .mode = .{ .session = buf },
            .rendered = rendered,
        };
    }

    pub fn initTheme(
        gpa: std.mem.Allocator,
        current_theme: []const u8,
    ) !Picker {
        var arena = std.heap.ArenaAllocator.init(gpa);
        errdefer arena.deinit();
        const a = arena.allocator();

        const all = theme_mod.listAll();
        const entries = try a.alloc(theme_mod.ThemeEntry, all.len);
        var initial_cursor: usize = 0;
        for (all, 0..) |e, i| {
            entries[i] = .{ .id = e.id, .name = e.name };
            if (std.mem.eql(u8, e.id, current_theme)) initial_cursor = i;
        }
        const rendered = try a.alloc([]const u8, all.len);
        for (entries, 0..) |e, i| {
            const marker: []const u8 = if (std.mem.eql(u8, e.id, current_theme)) "*" else " ";
            rendered[i] = try std.fmt.allocPrint(a, " {s} {s}", .{ marker, e.name });
        }
        return .{
            .arena = arena,
            .title = try a.dupe(u8, "Select theme (Enter to confirm, Esc to cancel)"),
            .cursor = initial_cursor,
            .mode = .{ .theme = entries },
            .rendered = rendered,
        };
    }

    /// Build a command-autocomplete picker from a slice of CommandInfo values.
    /// `commands_in` is duped onto the picker's arena.
    pub fn initCommand(
        gpa: std.mem.Allocator,
        commands_in: []const commands.CommandInfo,
    ) !Picker {
        var arena = std.heap.ArenaAllocator.init(gpa);
        errdefer arena.deinit();
        const a = arena.allocator();

        const all = try a.alloc(commands.CommandInfo, commands_in.len);
        for (commands_in, 0..) |c, i| {
            all[i] = .{
                .name = try a.dupe(u8, c.name),
                .summary = try a.dupe(u8, c.summary),
                .is_skill = c.is_skill,
            };
        }
        const filtered = try a.alloc(u32, commands_in.len);
        for (0..commands_in.len) |i| filtered[i] = @intCast(i);

        var p: Picker = .{
            .arena = arena,
            .title = try a.dupe(u8, "Commands"),
            .cursor = 0,
            .mode = .{ .command_complete = .{ .all = all, .filtered = filtered } },
            .rendered = &.{},
        };
        p.rebuildCommandLines();
        return p;
    }

    /// Restrict the visible matches to commands whose names contain `query`
    /// as a case-insensitive subsequence. Empty query lists all commands in
    /// alphabetical order. Non-empty queries are scored: prefix matches sort
    /// first, then substring matches, then loose subsequence matches.
    pub fn setCommandFilter(self: *Picker, query: []const u8) void {
        switch (self.mode) {
            .command_complete => |*cc| {
                const a = self.arena.allocator();

                if (query.len == 0) {
                    const buf = a.alloc(u32, cc.all.len) catch {
                        cc.filtered = &.{};
                        self.cursor = 0;
                        return;
                    };
                    for (0..cc.all.len) |i| buf[i] = @intCast(i);
                    cc.filtered = buf;
                    self.cursor = 0;
                    self.rebuildCommandLines();
                    return;
                }

                const Scored = struct { idx: u32, score: u32 };
                const buf = a.alloc(Scored, cc.all.len) catch {
                    cc.filtered = &.{};
                    self.cursor = 0;
                    return;
                };
                var n: usize = 0;
                for (cc.all, 0..) |c, i| {
                    if (fuzzyScore(c.name, query)) |s| {
                        buf[n] = .{ .idx = @intCast(i), .score = s };
                        n += 1;
                    }
                }
                // Highest score first; ties keep alphabetical order
                // (`cc.all` is already sorted by name).
                std.mem.sort(Scored, buf[0..n], {}, struct {
                    fn lessThan(_: void, x: Scored, y: Scored) bool {
                        if (x.score != y.score) return x.score > y.score;
                        return x.idx < y.idx;
                    }
                }.lessThan);

                const out = a.alloc(u32, n) catch {
                    cc.filtered = &.{};
                    self.cursor = 0;
                    return;
                };
                for (buf[0..n], 0..) |s, i| out[i] = s.idx;
                cc.filtered = out;
                self.cursor = 0;
                self.rebuildCommandLines();
            },
            else => {},
        }
    }

    fn rebuildCommandLines(self: *Picker) void {
        const a = self.arena.allocator();
        const cc = switch (self.mode) {
            .command_complete => |c| c,
            else => return,
        };
        const lines = a.alloc([]const u8, cc.filtered.len) catch {
            self.rendered = &.{};
            return;
        };
        for (cc.filtered, 0..) |idx, i| {
            const c = cc.all[idx];
            lines[i] = std.fmt.allocPrint(a, "   /{s}  - {s}", .{ c.name, c.summary }) catch c.name;
        }
        self.rendered = lines;
    }

    pub fn selectedCommand(self: *const Picker) ?commands.CommandInfo {
        return switch (self.mode) {
            .command_complete => |cc| if (cc.filtered.len == 0) null else cc.all[cc.filtered[self.cursor]],
            else => null,
        };
    }

    pub fn deinit(self: *Picker, _: std.mem.Allocator) void {
        self.arena.deinit();
    }

    pub fn count(self: *const Picker) usize {
        return switch (self.mode) {
            .model => |c| c.len,
            .session => |c| c.len,
            .command_complete => |cc| cc.filtered.len,
            .theme => |t| t.len,
        };
    }

    pub fn moveUp(self: *Picker) void {
        if (self.cursor > 0) self.cursor -= 1;
    }
    pub fn moveDown(self: *Picker) void {
        if (self.cursor + 1 < self.count()) self.cursor += 1;
    }
    pub fn selectedModel(self: *const Picker) ?commands.ModelChoice {
        return switch (self.mode) {
            .model => |c| if (c.len == 0) null else c[self.cursor],
            else => null,
        };
    }
    pub fn selectedSession(self: *const Picker) ?commands.SessionChoice {
        return switch (self.mode) {
            .session => |c| if (c.len == 0) null else c[self.cursor],
            else => null,
        };
    }

    pub fn selectedTheme(self: *const Picker) ?theme_mod.ThemeEntry {
        return switch (self.mode) {
            .theme => |t| if (t.len == 0) null else t[self.cursor],
            else => null,
        };
    }

    fn startsWithIgnoreCase(s: []const u8, prefix: []const u8) bool {
        if (prefix.len > s.len) return false;
        for (prefix, 0..) |p, i| {
            if (std.ascii.toLower(s[i]) != std.ascii.toLower(p)) return false;
        }
        return true;
    }

    /// Return a match score for `query` against `name`, or null when there's
    /// no match. Higher score = better match. The exact numbers don't matter,
    /// only the ranking: prefix > substring > subsequence.
    fn fuzzyScore(name: []const u8, query: []const u8) ?u32 {
        if (query.len == 0) return 1;
        if (startsWithIgnoreCase(name, query)) return 1_000_000;
        if (containsIgnoreCase(name, query)) return 100_000;
        // Subsequence: every character of query appears in name in order.
        var qi: usize = 0;
        for (name) |c| {
            if (qi >= query.len) break;
            if (std.ascii.toLower(c) == std.ascii.toLower(query[qi])) qi += 1;
        }
        if (qi != query.len) return null;
        // Prefer shorter names so a 3-letter query against `clear` ranks
        // above the same query against a 20-char skill name.
        return 1000 -| @as(u32, @intCast(name.len));
    }

    fn containsIgnoreCase(haystack: []const u8, needle: []const u8) bool {
        if (needle.len == 0) return true;
        if (needle.len > haystack.len) return false;
        var i: usize = 0;
        while (i + needle.len <= haystack.len) : (i += 1) {
            var match = true;
            for (needle, 0..) |n, j| {
                if (std.ascii.toLower(haystack[i + j]) != std.ascii.toLower(n)) {
                    match = false;
                    break;
                }
            }
            if (match) return true;
        }
        return false;
    }

    /// How tall the picker should render given the terminal height: title row +
    /// up to 8 visible choices + 2 border rows. Caps at total_h/2 so it never
    /// crowds out chat entirely.
    pub fn height(self: *const Picker, total_h: u16) u16 {
        const visible: u16 = @intCast(@min(self.count(), 8));
        const want: u16 = visible + 3; // title + visible + bottom border
        const cap: u16 = total_h / 2;
        return @min(want, cap);
    }

    pub fn draw(self: *const Picker, win: vaxis.Window, t: *const theme_mod.Theme) void {
        if (win.height == 0 or win.width == 0) return;
        const border_style: vaxis.Style = .{ .fg = .{ .rgb = t.dim() } };
        const inner = win.child(.{
            .x_off = 0,
            .y_off = 0,
            .width = win.width,
            .height = win.height,
            .border = .{ .where = .all, .glyphs = .single_rounded, .style = border_style },
        });
        if (inner.height == 0) return;

        _ = inner.print(&.{.{ .text = self.title, .style = .{ .bold = true } }}, .{ .row_offset = 0, .col_offset = 1 });

        const list_top: u16 = 1;
        const list_h = inner.height -| list_top;
        const total = self.count();
        var top: usize = 0;
        if (total > list_h) {
            if (self.cursor >= list_h) top = self.cursor + 1 - list_h;
        }
        const visible_n = @min(total - top, list_h);

        var i: usize = 0;
        while (i < visible_n) : (i += 1) {
            const idx = top + i;
            if (idx >= self.rendered.len) break;
            const is_cursor = (idx == self.cursor);
            const style: vaxis.Style = if (is_cursor)
                .{ .fg = .{ .rgb = t.pickerCursorFg() }, .bg = .{ .rgb = t.pickerCursorBg() } }
            else
                .{};
            const text = self.rendered[idx];
            _ = inner.print(&.{.{ .text = text, .style = style }}, .{ .row_offset = list_top + @as(u16, @intCast(i)), .col_offset = 1 });
        }
    }
};

// ---- Tests ----

test "picker height caps at total/2" {
    const choices = [_]commands.ModelChoice{
        .{ .provider_index = 0, .kind = .claude, .model = "a", .is_active = true },
        .{ .provider_index = 1, .kind = .openai, .model = "b", .is_active = false },
    };
    var arena = std.heap.ArenaAllocator.init(std.testing.allocator);
    defer arena.deinit();
    const p = Picker{
        .arena = arena,
        .title = "t",
        .cursor = 0,
        .mode = .{ .model = &choices },
        .rendered = &.{},
    };
    try std.testing.expectEqual(@as(u16, 5), p.height(20)); // 2 choices + 3 = 5, cap=10
    try std.testing.expectEqual(@as(u16, 4), p.height(8)); // cap = 8/2 = 4
}

test "picker moveDown stops at end" {
    const choices = [_]commands.ModelChoice{
        .{ .provider_index = 0, .kind = .claude, .model = "a", .is_active = true },
        .{ .provider_index = 1, .kind = .openai, .model = "b", .is_active = false },
    };
    var arena = std.heap.ArenaAllocator.init(std.testing.allocator);
    defer arena.deinit();
    var p = Picker{
        .arena = arena,
        .title = "t",
        .cursor = 0,
        .mode = .{ .model = &choices },
        .rendered = &.{},
    };
    p.moveDown();
    try std.testing.expectEqual(@as(usize, 1), p.cursor);
    p.moveDown();
    try std.testing.expectEqual(@as(usize, 1), p.cursor); // stopped at end
}

test "command picker filters by prefix" {
    const all = [_]commands.CommandInfo{
        .{ .name = "clear", .summary = "save", .is_skill = false },
        .{ .name = "compact", .summary = "compact", .is_skill = false },
        .{ .name = "model", .summary = "select", .is_skill = false },
        .{ .name = "resume", .summary = "resume", .is_skill = false },
    };
    var p = try Picker.initCommand(std.testing.allocator, &all);
    defer p.deinit(std.testing.allocator);

    try std.testing.expectEqual(@as(usize, 4), p.count());

    p.setCommandFilter("c");
    try std.testing.expectEqual(@as(usize, 2), p.count());
    const sel0 = p.selectedCommand() orelse return error.TestUnexpectedResult;
    try std.testing.expectEqualStrings("clear", sel0.name);

    p.setCommandFilter("cle");
    try std.testing.expectEqual(@as(usize, 1), p.count());

    p.setCommandFilter("zzz");
    try std.testing.expectEqual(@as(usize, 0), p.count());
    try std.testing.expect(p.selectedCommand() == null);
}

test "command picker fuzzy matches subsequence" {
    const all = [_]commands.CommandInfo{
        .{ .name = "clear", .summary = "", .is_skill = false },
        .{ .name = "compact", .summary = "", .is_skill = false },
        .{ .name = "model", .summary = "", .is_skill = false },
        .{ .name = "models", .summary = "", .is_skill = false },
        .{ .name = "resume", .summary = "", .is_skill = false },
    };
    var p = try Picker.initCommand(std.testing.allocator, &all);
    defer p.deinit(std.testing.allocator);

    // Subsequence: "mdl" -> "model" (m, d? no... "model" has m,o,d,e,l)
    p.setCommandFilter("mdl");
    try std.testing.expect(p.count() >= 1);
    const sel = p.selectedCommand() orelse return error.TestUnexpectedResult;
    try std.testing.expectEqualStrings("model", sel.name);

    // Substring: "ode" -> "model" and "models". Prefix beats substring,
    // but neither has the prefix; substring matches both.
    p.setCommandFilter("ode");
    try std.testing.expectEqual(@as(usize, 2), p.count());

    // Prefix wins over substring. "co" prefixes "compact" and is a substring
    // of nothing else here, so only one match — but importantly the prefix
    // ranks first.
    p.setCommandFilter("com");
    try std.testing.expectEqual(@as(usize, 1), p.count());
    const compact = p.selectedCommand() orelse return error.TestUnexpectedResult;
    try std.testing.expectEqualStrings("compact", compact.name);
}

test "session picker exposes count and selection" {
    const choices = [_]commands.SessionChoice{
        .{ .id = "a", .name = "first", .updated_at = 1, .message_count = 3 },
        .{ .id = "b", .name = "second", .updated_at = 2, .message_count = 5 },
    };
    var arena = std.heap.ArenaAllocator.init(std.testing.allocator);
    defer arena.deinit();
    var p = Picker{
        .arena = arena,
        .title = "t",
        .cursor = 0,
        .mode = .{ .session = &choices },
        .rendered = &.{},
    };
    try std.testing.expectEqual(@as(usize, 2), p.count());
    try std.testing.expect(p.selectedModel() == null);
    p.moveDown();
    const sel = p.selectedSession() orelse return error.TestUnexpectedResult;
    try std.testing.expectEqualStrings("b", sel.id);
}
