const std = @import("std");
const vaxis = @import("vaxis");
const commands = @import("commands");

pub const Picker = struct {
    arena: std.heap.ArenaAllocator,
    title: []const u8,
    choices: []const commands.ModelChoice,
    cursor: usize,

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
        return .{
            .arena = arena,
            .title = title,
            .choices = buf,
            .cursor = initial_cursor,
        };
    }

    pub fn deinit(self: *Picker, _: std.mem.Allocator) void {
        self.arena.deinit();
    }

    pub fn moveUp(self: *Picker) void {
        if (self.cursor > 0) self.cursor -= 1;
    }
    pub fn moveDown(self: *Picker) void {
        if (self.cursor + 1 < self.choices.len) self.cursor += 1;
    }
    pub fn selected(self: *const Picker) commands.ModelChoice {
        return self.choices[self.cursor];
    }

    /// How tall the picker should render given the terminal height: title row +
    /// up to 8 visible choices + 2 border rows. Caps at total_h/2 so it never
    /// crowds out chat entirely.
    pub fn height(self: *const Picker, total_h: u16) u16 {
        const visible: u16 = @intCast(@min(self.choices.len, 8));
        const want: u16 = visible + 3; // title + visible + bottom border
        const cap: u16 = total_h / 2;
        return @min(want, cap);
    }

    pub fn draw(self: *const Picker, win: vaxis.Window) void {
        if (win.height == 0 or win.width == 0) return;
        const border_style: vaxis.Style = .{ .fg = .{ .index = 8 } };
        // Use a bordered child window for the visual frame.
        const inner = win.child(.{
            .x_off = 0,
            .y_off = 0,
            .width = win.width,
            .height = win.height,
            .border = .{ .where = .all, .glyphs = .single_rounded, .style = border_style },
        });
        if (inner.height == 0) return;

        // Title on row 0.
        _ = inner.print(&.{.{ .text = self.title, .style = .{ .bold = true } }}, .{ .row_offset = 0, .col_offset = 1 });

        const list_top: u16 = 1;
        const list_h = inner.height -| list_top;
        const total = self.choices.len;
        // Window the choices around the cursor.
        var top: usize = 0;
        if (total > list_h) {
            if (self.cursor >= list_h) top = self.cursor + 1 - list_h;
        }
        const visible_n = @min(total - top, list_h);

        var i: usize = 0;
        while (i < visible_n) : (i += 1) {
            const idx = top + i;
            const c = self.choices[idx];
            const is_cursor = (idx == self.cursor);
            const style: vaxis.Style = if (is_cursor)
                .{ .fg = .{ .rgb = .{ 0, 0, 0 } }, .bg = .{ .rgb = .{ 220, 220, 220 } } }
            else
                .{};
            var line_buf: [256]u8 = undefined;
            const marker: []const u8 = if (c.is_active) "*" else " ";
            const text = std.fmt.bufPrint(&line_buf, " {s} {s} ({s})", .{ marker, c.model, @tagName(c.kind) }) catch c.model;
            _ = inner.print(&.{.{ .text = text, .style = style }}, .{ .row_offset = list_top + @as(u16, @intCast(i)), .col_offset = 1 });
        }
    }
};

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
        .choices = &choices,
        .cursor = 0,
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
        .choices = &choices,
        .cursor = 0,
    };
    p.moveDown();
    try std.testing.expectEqual(@as(usize, 1), p.cursor);
    p.moveDown();
    try std.testing.expectEqual(@as(usize, 1), p.cursor); // stopped at end
}
