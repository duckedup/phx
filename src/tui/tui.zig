const std = @import("std");
const vaxis = @import("vaxis");
const core = @import("phoenix_core");

const VxEvent = union(enum) {
    key_press: vaxis.Key,
    winsize: vaxis.Winsize,
    focus_in,
    focus_out,
    mouse: vaxis.Mouse,
    paste_start,
    paste_end,
    paste: []const u8,
};

const ChatLine = struct {
    role: core.Role,
    text: []const u8,
};

const paste_char_threshold = 80;

pub fn run(init: std.process.Init) !void {
    const io = init.io;
    const allocator = init.gpa;

    var chat_lines: std.ArrayList(ChatLine) = .empty;
    defer {
        for (chat_lines.items) |line| allocator.free(line.text);
        chat_lines.deinit(allocator);
    }

    var scroll_offset: u16 = 0;
    var last_chat_h: u16 = 20;

    var paste_count: u32 = 0;
    var pasting = false;
    var paste_start_line: usize = 0;
    var paste_start_col: usize = 0;

    var pending_full: std.ArrayList([]const u8) = .empty;
    defer {
        for (pending_full.items) |p| allocator.free(p);
        pending_full.deinit(allocator);
    }

    // Multi-line input buffer (owns the text, supports newlines)
    var input_lines: std.ArrayList(std.ArrayList(u8)) = .empty;
    defer {
        for (input_lines.items) |*line| line.deinit(allocator);
        input_lines.deinit(allocator);
    }
    try input_lines.append(allocator, .empty);
    var cursor_line: usize = 0;
    var cursor_col: usize = 0;

    try chat_lines.append(allocator, .{
        .role = .system,
        .text = try allocator.dupe(u8, "Welcome to Phoenix. Type a message below and press Enter."),
    });

    var buffer: [4096]u8 = undefined;
    var tty = try vaxis.Tty.init(io, &buffer);
    defer tty.deinit();
    const writer = tty.writer();

    var vx = try vaxis.init(io, allocator, init.environ_map, .{});
    defer vx.deinit(allocator, writer);

    var loop: vaxis.Loop(VxEvent) = .init(io, &tty, &vx);
    try loop.start();
    defer loop.stop();

    try vx.enterAltScreen(writer);
    try vx.queryTerminal(writer, .fromSeconds(1));
    try vx.setBracketedPaste(writer, true);
    try vx.setMouseMode(writer, true);
    try writer.flush();


    while (true) {
        const event = try loop.nextEvent();

        switch (event) {
            .paste_start => {
                pasting = true;
                paste_start_line = cursor_line;
                paste_start_col = cursor_col;
            },
            .paste_end => {
                pasting = false;
                // Check if paste spans multiple lines
                if (cursor_line > paste_start_line) {
                    // Extract the pasted region
                    const pasted = extractRegion(&input_lines, paste_start_line, paste_start_col, cursor_line, cursor_col, allocator) catch null;
                    if (pasted) |full| {
                        paste_count += 1;
                        try pending_full.append(allocator, full);

                        // Remove the pasted region and replace with label
                        removeRegion(&input_lines, paste_start_line, paste_start_col, cursor_line, cursor_col, allocator);
                        cursor_line = paste_start_line;
                        cursor_col = paste_start_col;

                        var nl_count: usize = 0;
                        for (full) |ch| {
                            if (ch == '\n') nl_count += 1;
                        }
                        var label_buf: [80]u8 = undefined;
                        const label = std.fmt.bufPrint(&label_buf, "[Pasted text #{d} +{d} lines]", .{
                            paste_count, nl_count + 1,
                        }) catch "[Pasted text]";
                        insertText(&input_lines, &cursor_line, &cursor_col, label, allocator) catch {};
                    }
                }
            },
            .paste => |text| {
                // OSC 52 paste — insert all at once
                try insertText(&input_lines, &cursor_line, &cursor_col, text, allocator);
            },
            .key_press => |key| {
                if (pasting) {
                    // Newlines in paste arrive as Enter (0x0D), LF (0x0A),
                    // or Ctrl+J (codepoint 'j' with text=null) in Kitty protocol
                    const is_newline = key.codepoint == vaxis.Key.enter or
                        key.codepoint == '\n' or
                        (key.codepoint == 'j' and key.text == null);
                    if (is_newline) {
                        try insertNewline(&input_lines, &cursor_line, &cursor_col, allocator);
                    } else if (key.text) |text| {
                        try insertText(&input_lines, &cursor_line, &cursor_col, text, allocator);
                    } else if (key.codepoint >= 0x20 and key.codepoint < 0x110000) {
                        var buf: [4]u8 = undefined;
                        const len = std.unicode.utf8Encode(@intCast(key.codepoint), &buf) catch 0;
                        if (len > 0) {
                            try insertText(&input_lines, &cursor_line, &cursor_col, buf[0..len], allocator);
                        }
                    }
                    continue;
                }

                if (key.matches('c', .{ .ctrl = true })) break;

                // Shift+Enter always inserts a newline
                if ((key.codepoint == vaxis.Key.enter or key.codepoint == vaxis.Key.kp_enter) and key.mods.shift) {
                    try insertNewline(&input_lines, &cursor_line, &cursor_col, allocator);
                } else if (key.codepoint == vaxis.Key.enter or key.codepoint == vaxis.Key.kp_enter) {
                    const full = try buildFullInput(&input_lines, allocator);

                    if (pending_full.items.len > 0) {
                        // Expand paste labels in the input with actual content
                        const expanded = try expandPasteLabels(full, &pending_full, allocator);
                        allocator.free(full);
                        for (pending_full.items) |p| allocator.free(p);
                        pending_full.clearRetainingCapacity();

                        try chat_lines.append(allocator, .{
                            .role = .user,
                            .text = expanded,
                        });

                        try chat_lines.append(allocator, .{
                            .role = .assistant,
                            .text = try allocator.dupe(u8, "Provider support is coming soon -- this is the Phoenix skeleton build."),
                        });
                        scroll_offset = 0;
                        clearInput(&input_lines, &cursor_line, &cursor_col, allocator);
                    } else if (full.len > 0) {
                        try chat_lines.append(allocator, .{
                            .role = .user,
                            .text = full,
                        });

                        try chat_lines.append(allocator, .{
                            .role = .assistant,
                            .text = try allocator.dupe(u8, "Provider support is coming soon -- this is the Phoenix skeleton build."),
                        });
                        scroll_offset = 0;
                        clearInput(&input_lines, &cursor_line, &cursor_col, allocator);
                    } else {
                        allocator.free(full);
                    }
                } else if (key.codepoint == vaxis.Key.backspace) {
                    deleteBeforeCursor(&input_lines, &cursor_line, &cursor_col, allocator);
                } else if (key.codepoint == vaxis.Key.left) {
                    if (cursor_col > 0) cursor_col -= 1;
                } else if (key.codepoint == vaxis.Key.right) {
                    if (cursor_col < input_lines.items[cursor_line].items.len) cursor_col += 1;
                } else if (key.codepoint == vaxis.Key.up) {
                    if (cursor_line > 0) {
                        cursor_line -= 1;
                        cursor_col = @min(cursor_col, input_lines.items[cursor_line].items.len);
                    }
                } else if (key.codepoint == vaxis.Key.down) {
                    if (cursor_line + 1 < input_lines.items.len) {
                        cursor_line += 1;
                        cursor_col = @min(cursor_col, input_lines.items[cursor_line].items.len);
                    }
                } else if (key.codepoint == vaxis.Key.page_up) {
                    scroll_offset +|= last_chat_h / 2;
                } else if (key.codepoint == vaxis.Key.page_down) {
                    scroll_offset -|= last_chat_h / 2;
                } else if (key.matches('u', .{ .ctrl = true })) {
                    clearInput(&input_lines, &cursor_line, &cursor_col, allocator);
                } else {
                    if (key.text) |text| {
                        try insertText(&input_lines, &cursor_line, &cursor_col, text, allocator);
                    }
                }
            },
            .mouse => |mouse| {
                switch (mouse.button) {
                    .wheel_up => scroll_offset +|= 3,
                    .wheel_down => scroll_offset -|= 3,
                    else => {},
                }
            },
            .winsize => |ws| try vx.resize(allocator, writer, ws),
            else => {},
        }

        const win = vx.window();
        win.clear();

        const w = win.width;
        const h = win.height;
        if (w == 0 or h == 0) continue;

        const status_h: u16 = 1;
        // Input height: 1 line per input line, +2 for borders, min 3, max 10
        const raw_input_lines: u16 = @intCast(@min(input_lines.items.len, 8));
        const input_h: u16 = @min(raw_input_lines + 2, 10);
        const chat_h = h -| input_h -| status_h;

        last_chat_h = chat_h;
        if (chat_h > 0) {
            const cwin = win.child(.{ .x_off = 0, .y_off = 0, .width = w, .height = chat_h });
            scroll_offset = drawChat(cwin, chat_lines.items, w, scroll_offset, allocator);
        }

        {
            const iwin = win.child(.{
                .x_off = 0,
                .y_off = chat_h,
                .width = w,
                .height = input_h,
                .border = .{ .where = .{ .other = .{ .top = true, .bottom = true } }, .style = .{ .fg = .{ .index = 8 } } },
            });
            drawInput(iwin, input_lines.items, cursor_line, cursor_col, &vx);
        }

        {
            const swin = win.child(.{ .x_off = 0, .y_off = h - status_h, .width = w, .height = status_h });
            drawStatusBar(swin, w, scroll_offset);
        }

        try vx.render(writer);
        try writer.flush();
    }
}

fn insertText(
    input_lines: *std.ArrayList(std.ArrayList(u8)),
    cursor_line: *usize,
    cursor_col: *usize,
    text: []const u8,
    allocator: std.mem.Allocator,
) !void {
    for (text) |ch| {
        if (ch == '\n' or ch == '\r') {
            try insertNewline(input_lines, cursor_line, cursor_col, allocator);
        } else {
            try input_lines.items[cursor_line.*].insert(allocator, cursor_col.*, ch);
            cursor_col.* += 1;
        }
    }
}

fn insertNewline(
    input_lines: *std.ArrayList(std.ArrayList(u8)),
    cursor_line: *usize,
    cursor_col: *usize,
    allocator: std.mem.Allocator,
) !void {
    const current = &input_lines.items[cursor_line.*];
    var new_line: std.ArrayList(u8) = .empty;

    // Move text after cursor to new line
    if (cursor_col.* < current.items.len) {
        try new_line.appendSlice(allocator, current.items[cursor_col.*..]);
        current.shrinkRetainingCapacity(cursor_col.*);
    }

    try input_lines.insert(allocator, cursor_line.* + 1, new_line);
    cursor_line.* += 1;
    cursor_col.* = 0;
}

fn deleteBeforeCursor(
    input_lines: *std.ArrayList(std.ArrayList(u8)),
    cursor_line: *usize,
    cursor_col: *usize,
    allocator: std.mem.Allocator,
) void {
    if (cursor_col.* > 0) {
        _ = input_lines.items[cursor_line.*].orderedRemove(cursor_col.* - 1);
        cursor_col.* -= 1;
    } else if (cursor_line.* > 0) {
        // Merge with previous line
        const prev_len = input_lines.items[cursor_line.* - 1].items.len;
        input_lines.items[cursor_line.* - 1].appendSlice(allocator, input_lines.items[cursor_line.*].items) catch return;
        var removed = input_lines.orderedRemove(cursor_line.*);
        removed.deinit(allocator);
        cursor_line.* -= 1;
        cursor_col.* = prev_len;
    }
}

fn clearInput(
    input_lines: *std.ArrayList(std.ArrayList(u8)),
    cursor_line: *usize,
    cursor_col: *usize,
    allocator: std.mem.Allocator,
) void {
    for (input_lines.items) |*line| line.deinit(allocator);
    input_lines.clearRetainingCapacity();
    input_lines.append(allocator, .empty) catch {};
    cursor_line.* = 0;
    cursor_col.* = 0;
}

fn buildFullInput(
    input_lines: *std.ArrayList(std.ArrayList(u8)),
    allocator: std.mem.Allocator,
) ![]u8 {
    var total: usize = 0;
    for (input_lines.items, 0..) |line, i| {
        total += line.items.len;
        if (i + 1 < input_lines.items.len) total += 1; // newline between lines
    }

    const result = try allocator.alloc(u8, total);
    var pos: usize = 0;
    for (input_lines.items, 0..) |line, i| {
        @memcpy(result[pos .. pos + line.items.len], line.items);
        pos += line.items.len;
        if (i + 1 < input_lines.items.len) {
            result[pos] = '\n';
            pos += 1;
        }
    }
    return result;
}

fn expandPasteLabels(
    input: []const u8,
    pending: *std.ArrayList([]const u8),
    allocator: std.mem.Allocator,
) ![]u8 {
    // Replace each [Pasted text #N ...] label with the actual content
    var result: std.ArrayList(u8) = .empty;
    var paste_idx: usize = 0;
    var i: usize = 0;

    while (i < input.len) {
        if (input[i] == '[' and i + 13 < input.len and std.mem.startsWith(u8, input[i..], "[Pasted text #")) {
            // Find the closing bracket
            var end = i + 1;
            while (end < input.len and input[end] != ']') end += 1;
            if (end < input.len) end += 1; // skip ']'

            // Insert the actual paste content
            if (paste_idx < pending.items.len) {
                const pasted = pending.items[paste_idx];
                for (pasted) |ch| {
                    try result.append(allocator, if (ch == '\n' or ch == '\r') ' ' else ch);
                }
                paste_idx += 1;
            }
            i = end;
        } else {
            try result.append(allocator, input[i]);
            i += 1;
        }
    }

    return try result.toOwnedSlice(allocator);
}

fn extractRegion(
    input_lines: *std.ArrayList(std.ArrayList(u8)),
    start_line: usize,
    start_col: usize,
    end_line: usize,
    end_col: usize,
    allocator: std.mem.Allocator,
) ![]u8 {
    var total: usize = 0;
    for (start_line..end_line + 1) |i| {
        if (i >= input_lines.items.len) break;
        const line = input_lines.items[i].items;
        const from = if (i == start_line) @min(start_col, line.len) else 0;
        const to = if (i == end_line) @min(end_col, line.len) else line.len;
        total += to -| from;
        if (i < end_line) total += 1;
    }

    const result = try allocator.alloc(u8, total);
    var pos: usize = 0;
    for (start_line..end_line + 1) |i| {
        if (i >= input_lines.items.len) break;
        const line = input_lines.items[i].items;
        const from = if (i == start_line) @min(start_col, line.len) else 0;
        const to = if (i == end_line) @min(end_col, line.len) else line.len;
        const chunk = line[from..to];
        @memcpy(result[pos .. pos + chunk.len], chunk);
        pos += chunk.len;
        if (i < end_line) {
            result[pos] = '\n';
            pos += 1;
        }
    }
    return result;
}

fn removeRegion(
    input_lines: *std.ArrayList(std.ArrayList(u8)),
    start_line: usize,
    start_col: usize,
    end_line: usize,
    end_col: usize,
    allocator: std.mem.Allocator,
) void {
    if (start_line >= input_lines.items.len) return;
    const actual_end = @min(end_line, input_lines.items.len - 1);

    // Keep text before start_col on start_line and after end_col on end_line
    const end_remainder = if (actual_end < input_lines.items.len)
        allocator.dupe(u8, input_lines.items[actual_end].items[@min(end_col, input_lines.items[actual_end].items.len)..]) catch ""
    else
        "";
    defer if (end_remainder.len > 0) allocator.free(end_remainder);

    // Truncate start line at start_col
    input_lines.items[start_line].shrinkRetainingCapacity(@min(start_col, input_lines.items[start_line].items.len));

    // Append end remainder to start line
    input_lines.items[start_line].appendSlice(allocator, end_remainder) catch {};

    // Remove lines between start+1 and end (inclusive)
    if (actual_end > start_line) {
        var i = actual_end;
        while (i > start_line) : (i -= 1) {
            var removed = input_lines.orderedRemove(i);
            removed.deinit(allocator);
        }
    }
}

fn drawInput(win: vaxis.Window, lines: []const std.ArrayList(u8), cursor_line: usize, cursor_col: usize, vx: *vaxis.Vaxis) void {
    const inner_h = win.height;
    const inner_w = win.width;
    if (inner_h == 0 or inner_w == 0) return;

    const text_style: vaxis.Style = .{ .fg = .{ .rgb = .{ 220, 220, 220 } } };
    const cursor_style: vaxis.Style = .{ .fg = .{ .rgb = .{ 0, 0, 0 } }, .bg = .{ .rgb = .{ 220, 220, 220 } } };

    // Show last N lines that fit
    const visible = @min(lines.len, inner_h);
    const start = if (lines.len > inner_h) lines.len - inner_h else 0;

    for (0..visible) |i| {
        const li = start + i;
        const line = lines[li].items;
        const row: u16 = @intCast(i);

        const len = @min(line.len, inner_w);
        for (0..len) |col| {
            const is_cursor = (li == cursor_line and col == cursor_col);
            const style = if (is_cursor) cursor_style else text_style;
            win.writeCell(@intCast(col), row, .{
                .char = .{ .grapheme = line[col .. col + 1] },
                .style = style,
            });
        }

        // Draw cursor at end of line
        if (li == cursor_line and cursor_col >= len and cursor_col < inner_w) {
            win.writeCell(@intCast(cursor_col), row, .{
                .char = .{ .grapheme = " " },
                .style = cursor_style,
            });
        }
    }

    // Place the terminal cursor
    const cursor_row: u16 = @intCast(if (cursor_line >= start) cursor_line - start else 0);
    const cursor_c: u16 = @intCast(@min(cursor_col, inner_w -| 1));
    vx.screen.cursor = .{
        .row = @intCast(@as(i32, win.y_off) + cursor_row),
        .col = @intCast(@as(i32, win.x_off) + cursor_c),
    };
    vx.screen.cursor_vis = true;
}

fn drawStatusBar(win: vaxis.Window, w: u16, scroll_offset: u16) void {
    const style: vaxis.Style = .{
        .fg = .{ .rgb = .{ 0, 0, 0 } },
        .bg = .{ .rgb = .{ 100, 140, 255 } },
        .bold = true,
    };
    for (0..w) |x| {
        win.writeCell(@intCast(x), 0, .{ .char = .{ .grapheme = " " }, .style = style });
    }
    if (scroll_offset > 0) {
        var buf: [64]u8 = undefined;
        const indicator = std.fmt.bufPrint(&buf, " phoenix v0.0.0 | scrolled +{d} rows", .{scroll_offset}) catch " phoenix v0.0.0";
        _ = win.print(&.{.{ .text = indicator, .style = style }}, .{});
    } else {
        _ = win.print(
            &.{.{ .text = " phoenix v0.0.0 | provider: none", .style = style }},
            .{},
        );
    }
}

const bubble_padding = 2;
const max_bubble_pct = 70;

fn bubbleWidth(text_len: usize, win_width: u16) u16 {
    const max_w = @max(20, (win_width * max_bubble_pct) / 100);
    const content_w: u16 = @intCast(@min(text_len + bubble_padding * 2, max_w));
    return content_w;
}

fn wrapLines(text: []const u8, width: usize, allocator: std.mem.Allocator) !std.ArrayList([]const u8) {
    var result: std.ArrayList([]const u8) = .empty;
    if (width == 0) return result;

    var pos: usize = 0;
    while (pos < text.len) {
        var end = @min(pos + width, text.len);
        if (end < text.len and end > pos) {
            var break_at = end;
            while (break_at > pos and text[break_at] != ' ') {
                break_at -= 1;
            }
            if (break_at > pos) {
                end = break_at + 1;
            }
        }
        try result.append(allocator, text[pos..@min(end, text.len)]);
        pos = end;
    }
    if (result.items.len == 0) {
        try result.append(allocator, "");
    }
    return result;
}

fn drawChat(win: vaxis.Window, lines: []const ChatLine, win_width: u16, scroll_offset: u16, allocator: std.mem.Allocator) u16 {
    if (lines.len == 0) return 0;

    const rows: i64 = win.height;
    if (rows == 0 or win_width < 10) return 0;

    var total_rows: u32 = 0;
    var row_counts: [512]u32 = undefined;
    const line_count = @min(lines.len, 512);

    for (0..line_count) |i| {
        const line = lines[i];
        const spacing: u32 = if (i > 0) 1 else 0;

        if (line.role == .system) {
            row_counts[i] = spacing + 1;
            total_rows += row_counts[i];
            continue;
        }

        const bw = bubbleWidth(line.text.len, win_width);
        const inner_w = bw -| (bubble_padding * 2);
        var wrapped = wrapLines(line.text, inner_w, allocator) catch {
            row_counts[i] = spacing + 2;
            total_rows += row_counts[i];
            continue;
        };
        defer wrapped.deinit(allocator);

        row_counts[i] = spacing + 1 + @as(u32, @intCast(wrapped.items.len));
        total_rows += row_counts[i];
    }

    const max_scroll: u32 = if (total_rows > win.height) total_rows - win.height else 0;
    const clamped_offset: u32 = @min(scroll_offset, max_scroll);

    // visible_top: virtual row at top of viewport (negative = content bottom-aligned)
    const visible_top: i64 = @as(i64, total_rows) - rows - @as(i64, clamped_offset);

    var vrow: i64 = 0;

    for (0..line_count) |i| {
        const line = lines[i];
        const msg_end: i64 = vrow + row_counts[i];

        if (msg_end <= visible_top) {
            vrow = msg_end;
            continue;
        }
        if (vrow - visible_top >= rows) break;

        const is_right = line.role == .user;
        const is_system = line.role == .system;
        var cr = vrow;

        if (i > 0) cr += 1;

        if (is_system) {
            const sy = cr - visible_top;
            if (sy >= 0 and sy < rows) {
                const y: u16 = @intCast(sy);
                const text_len: u16 = @intCast(@min(line.text.len, win_width));
                const x_off: u16 = (win_width -| text_len) / 2;
                const sys_style: vaxis.Style = .{ .fg = .{ .index = 8 }, .italic = true };
                const sys_win = win.child(.{ .x_off = x_off, .y_off = y, .width = text_len, .height = 1 });
                _ = sys_win.print(&.{.{ .text = line.text, .style = sys_style }}, .{});
            }
            vrow = msg_end;
            continue;
        }

        const bw = bubbleWidth(line.text.len, win_width);
        const inner_w = bw -| (bubble_padding * 2);
        var wrapped = wrapLines(line.text, inner_w, allocator) catch {
            vrow = msg_end;
            continue;
        };
        defer wrapped.deinit(allocator);

        const x_off: u16 = if (is_right) win_width -| bw -| 1 else 1;

        // Label
        const label_sy = cr - visible_top;
        if (label_sy >= 0 and label_sy < rows) {
            const y: u16 = @intCast(label_sy);
            const label = if (is_right) "you" else "phoenix";
            const label_style: vaxis.Style = if (is_right)
                .{ .fg = .{ .rgb = .{ 130, 220, 130 } }, .bold = true }
            else
                .{ .fg = .{ .rgb = .{ 120, 160, 255 } }, .bold = true };

            const label_x = if (is_right) x_off + bw - @as(u16, @intCast(label.len)) else x_off;
            const label_win = win.child(.{
                .x_off = label_x,
                .y_off = y,
                .width = @intCast(@min(label.len, win_width)),
                .height = 1,
            });
            _ = label_win.print(&.{.{ .text = label, .style = label_style }}, .{});
        }
        cr += 1;

        // Bubble text
        const bubble_bg: vaxis.Style = if (is_right)
            .{ .fg = .{ .rgb = .{ 240, 240, 240 } }, .bg = .{ .rgb = .{ 40, 80, 40 } } }
        else
            .{ .fg = .{ .rgb = .{ 240, 240, 240 } }, .bg = .{ .rgb = .{ 50, 50, 70 } } };

        for (wrapped.items) |wline| {
            const sy = cr - visible_top;
            if (sy >= 0 and sy < rows) {
                const y: u16 = @intCast(sy);
                const bwin = win.child(.{ .x_off = x_off, .y_off = y, .width = bw, .height = 1 });
                for (0..bw) |bx| {
                    bwin.writeCell(@intCast(bx), 0, .{ .char = .{ .grapheme = " " }, .style = bubble_bg });
                }
                const text_win = win.child(.{
                    .x_off = x_off + bubble_padding,
                    .y_off = y,
                    .width = bw -| (bubble_padding * 2),
                    .height = 1,
                });
                _ = text_win.print(&.{.{ .text = wline, .style = bubble_bg }}, .{});
            }
            cr += 1;
            if (cr - visible_top >= rows) break;
        }

        vrow = msg_end;
    }

    return @intCast(clamped_offset);
}
