/// Full-screen /models page. Displays the configured providers and an
/// "Add new" affordance; opens the shared add-model wizard when the user
/// selects "Add new". The page owns its own state — the TUI's main loop
/// hands every key press to `handleKey` while the page is visible and
/// renders by calling `paint`.
const std = @import("std");
const vaxis = @import("vaxis");
const commands = @import("commands");
const core = @import("phoenix_core");
const rpc = @import("rpc");
const add_model = @import("add_model_wizard.zig");
const Theme = @import("theme.zig").Theme;

pub const ModelsPage = struct {
    /// Arena that owns the title, entries, and any rebuilt-list strings. The
    /// arena is reset (not deinit'd) when entries are replaced after addModel
    /// so the page can refresh in place.
    arena: std.heap.ArenaAllocator,
    title: []const u8,
    entries: []commands.ModelEntry,
    /// 0..entries.len = entry row; entries.len = "Add new" row.
    cursor: usize,
    /// Non-null while the add-model wizard is overlaid on the page.
    wizard: ?add_model.Wizard,
    /// Optional toast shown above the list (e.g. "Added gpt-5"). Owned by the
    /// arena. Cleared on next key press.
    flash: ?[]const u8,

    /// Outcome of `handleKey`. `stay` keeps the page open; `close` tells the
    /// TUI to drop the page back to chat; `activate_choice` requests the TUI
    /// flip the active provider via `applyModelChoice` (the page itself
    /// stays open and refreshes).
    pub const KeyOutcome = union(enum) {
        stay,
        close,
        activate_choice: commands.ModelChoice,
    };

    pub fn init(
        gpa: std.mem.Allocator,
        page: rpc.client.DispatchResult.ModelsPage,
    ) !ModelsPage {
        var arena = std.heap.ArenaAllocator.init(gpa);
        errdefer arena.deinit();
        const a = arena.allocator();

        const title = try a.dupe(u8, page.title);
        const entries = try cloneEntries(a, page.entries);

        // Default to the active provider's row (or top if none active).
        var cursor: usize = 0;
        for (entries, 0..) |e, i| {
            if (e.is_active) {
                cursor = i;
                break;
            }
        }

        return .{
            .arena = arena,
            .title = title,
            .entries = entries,
            .cursor = cursor,
            .wizard = null,
            .flash = null,
        };
    }

    pub fn deinit(self: *ModelsPage, gpa: std.mem.Allocator) void {
        _ = gpa;
        if (self.wizard) |*w| w.deinit();
        self.arena.deinit();
    }

    /// Replace the visible list of entries (used after addModel or activate
    /// returns). Clones inputs onto the page's arena.
    ///
    /// Note: this does NOT reset the arena. Resetting would invalidate
    /// `self.title`, which is the same arena. Memory grows by entries-list
    /// size per refresh; in practice the user adds a handful of providers
    /// per session, so this is fine. If we ever care, we can keep the title
    /// on a separate stable allocator.
    pub fn refresh(
        self: *ModelsPage,
        new_entries: []commands.ModelEntry,
        flash: ?[]const u8,
    ) !void {
        const a = self.arena.allocator();
        self.entries = try cloneEntries(a, new_entries);
        self.flash = if (flash) |f| try a.dupe(u8, f) else null;
        if (self.cursor > self.entries.len) self.cursor = self.entries.len;
    }

    pub fn handlePaste(self: *ModelsPage, text: []const u8) !void {
        if (self.wizard) |*w| try w.handlePaste(text);
    }

    pub fn handleKey(
        self: *ModelsPage,
        key: vaxis.Key,
        gpa: std.mem.Allocator,
        client: *rpc.Client,
    ) !KeyOutcome {
        // Wizard takes precedence when shown.
        if (self.wizard) |*w| {
            const outcome = try w.handleKey(key);
            switch (outcome) {
                .in_progress => return .stay,
                .cancelled => {
                    w.deinit();
                    self.wizard = null;
                    return .stay;
                },
                .completed => |result| {
                    // Submit the new profile to the server, then refresh.
                    const args = rpc.client.AddModelArgs{
                        .kind = result.kind,
                        .model = try gpa.dupe(u8, result.model),
                        .api_key = try gpa.dupe(u8, result.api_key),
                        .base_url = try gpa.dupe(u8, result.base_url),
                        .context_window = result.context_window,
                    };
                    defer gpa.free(args.model);
                    defer gpa.free(args.api_key);
                    defer gpa.free(args.base_url);

                    w.deinit();
                    self.wizard = null;

                    var ar = client.addModel(args) catch |err| {
                        const a = self.arena.allocator();
                        self.flash = std.fmt.allocPrint(a, "Add failed: {s}", .{@errorName(err)}) catch null;
                        return .stay;
                    };
                    defer ar.response.deinit();
                    try self.refresh(ar.result.entries, ar.result.message);
                    return .stay;
                },
            }
        }

        // List-mode keys.
        self.flash = null;

        if (key.matches('c', .{ .ctrl = true }) or
            key.codepoint == vaxis.Key.escape or
            key.matches('q', .{}))
        {
            return .close;
        }

        const row_count = self.entries.len + 1; // +1 for the "Add new" row

        if (key.codepoint == vaxis.Key.up) {
            if (self.cursor > 0) self.cursor -= 1;
            return .stay;
        }
        if (key.codepoint == vaxis.Key.down) {
            if (self.cursor + 1 < row_count) self.cursor += 1;
            return .stay;
        }
        if (key.matches('a', .{})) {
            // Shortcut: "a" jumps to "Add new" and opens the wizard.
            self.cursor = self.entries.len;
            self.wizard = add_model.Wizard.init(gpa);
            return .stay;
        }

        if (key.codepoint == vaxis.Key.enter or key.codepoint == vaxis.Key.kp_enter) {
            if (self.cursor == self.entries.len) {
                // "Add new" row.
                if (self.wizard) |*w| w.deinit();
                self.wizard = add_model.Wizard.init(gpa);
                return .stay;
            }
            // Activate the selected entry. The TUI flips the active flag via
            // applyModelChoice; we just hand it the choice and let it deal.
            const e = self.entries[self.cursor];
            return .{ .activate_choice = .{
                .provider_index = e.provider_index,
                .kind = e.kind,
                .model = e.model,
                .is_active = e.is_active,
            } };
        }

        return .stay;
    }

    pub fn paint(self: *const ModelsPage, parent: vaxis.Window, arena: std.mem.Allocator, t: *const Theme) void {
        if (parent.width < 40 or parent.height < 14) {
            writeText(parent, 0, 0, "Resize terminal to at least 40x14...", .{});
            return;
        }

        // Wizard takes over the whole screen when active. Drawing the list
        // first and then overlaying the wizard would leave the list text
        // bleeding through anywhere the wizard's centered modal doesn't paint
        // a cell — vaxis only writes the cells we tell it to. Easier to flip:
        // when the wizard is up, the page is hidden.
        if (self.wizard) |*w| {
            w.paint(parent, arena);
            return;
        }

        const modal_w: u16 = @min(parent.width, 78);
        const modal_h: u16 = @min(parent.height, 24);
        const modal = vaxis.widgets.alignment.center(parent, modal_w, modal_h);
        const bg: vaxis.Color = .{ .rgb = t.background };
        const inner = modal.child(.{
            .border = .{
                .where = .all,
                .glyphs = .single_rounded,
                .style = .{ .fg = .{ .rgb = t.dim() }, .bg = bg },
            },
        });
        if (inner.height == 0) return;

        const title_style: vaxis.Style = .{ .fg = .{ .rgb = t.foreground }, .bg = bg, .bold = true };
        const dim_style: vaxis.Style = .{ .fg = .{ .rgb = t.dim() }, .bg = bg };
        const sel_style: vaxis.Style = .{ .fg = .{ .rgb = t.background }, .bg = .{ .rgb = t.foreground }, .bold = true };
        const active_style: vaxis.Style = .{ .fg = .{ .rgb = t.success }, .bg = bg, .bold = true };
        const flash_style: vaxis.Style = .{ .fg = .{ .rgb = t.success }, .bg = bg, .bold = true };
        const normal_style: vaxis.Style = .{ .fg = .{ .rgb = t.foreground }, .bg = bg };

        writeText(inner, 1, 0, self.title, title_style);

        var row: u16 = 2;
        if (self.flash) |f| {
            writeText(inner, 1, row, f, flash_style);
            row += 2;
        }

        // Entries.
        for (self.entries, 0..) |e, i| {
            const is_sel = (i == self.cursor);
            const prefix: []const u8 = if (is_sel) " > " else "   ";
            const style: vaxis.Style = if (is_sel) sel_style else normal_style;
            writeText(inner, 1, row, prefix, style);
            const star: []const u8 = if (e.is_active) "* " else "  ";
            writeText(inner, 4, row, star, if (e.is_active) active_style else style);
            const line = std.fmt.allocPrint(arena, "{s}  ({s})", .{ e.model, @tagName(e.kind) }) catch e.model;
            writeText(inner, 6, row, line, style);

            // Right-side detail: base_url for local, "active" tag for the
            // current default. Truncated by the modal width.
            const detail: []const u8 = blk: {
                if (e.is_active) break :blk "[active]";
                if (e.base_url.len > 0) break :blk e.base_url;
                break :blk "";
            };
            if (detail.len > 0) {
                const x: u16 = inner.width -| @as(u16, @intCast(@min(detail.len, inner.width))) -| 2;
                writeText(inner, x, row, detail, dim_style);
            }
            row += 1;
        }
        row += 1;

        // "Add new" row.
        {
            const i = self.entries.len;
            const is_sel = (i == self.cursor);
            const prefix: []const u8 = if (is_sel) " > " else "   ";
            const style: vaxis.Style = if (is_sel) sel_style else normal_style;
            writeText(inner, 1, row, prefix, style);
            writeText(inner, 4, row, "+ Add new model", style);
        }

        const hint_row: u16 = inner.height -| 2;
        writeText(inner, 1, hint_row, "up/down navigate   Enter activate or add   a Add new   q/Esc close", dim_style);
    }
};

fn cloneEntries(a: std.mem.Allocator, src: []const commands.ModelEntry) ![]commands.ModelEntry {
    const out = try a.alloc(commands.ModelEntry, src.len);
    for (src, 0..) |e, i| {
        out[i] = .{
            .provider_index = e.provider_index,
            .kind = e.kind,
            .model = try a.dupe(u8, e.model),
            .is_active = e.is_active,
            .base_url = try a.dupe(u8, e.base_url),
            .context_window = e.context_window,
        };
    }
    return out;
}

fn writeText(win: vaxis.Window, col: u16, row: u16, text: []const u8, style: vaxis.Style) void {
    var c: u16 = col;
    var i: usize = 0;
    while (i < text.len) {
        const byte = text[i];
        const len: usize = if (byte < 0x80) 1 else if (byte < 0xC0) 1 else if (byte < 0xE0) 2 else if (byte < 0xF0) 3 else 4;
        const end = @min(i + len, text.len);
        if (c >= win.width) break;
        win.writeCell(c, row, .{
            .char = .{ .grapheme = text[i..end], .width = 1 },
            .style = style,
        });
        c += 1;
        i = end;
    }
}
