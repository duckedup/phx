const std = @import("std");
const vaxis = @import("vaxis");
const core = @import("phoenix_core");
const commands = @import("commands");
const rpc = @import("rpc");
const Picker = @import("picker.zig").Picker;
const ModelsPage = @import("models_page.zig").ModelsPage;
const add_model_wizard = @import("add_model_wizard.zig");
const theme_mod = @import("theme.zig");
const Theme = theme_mod.Theme;

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
    cache_info: ?[]const u8 = null,
};

/// Fire-growing thinking animation. Cycles through ember → flame → peak. Each
/// frame is a 3-cell glyph rendered to the right of the "phoenix" label
/// while the assistant bubble is still empty (i.e. waiting for the first
/// streamed token).
const FireFrame = struct {
    chars: []const u8, // 3 ASCII bytes
    rgb: [3]u8,
};

const fire_frames = [_]FireFrame{
    .{ .chars = " . ", .rgb = .{ 120, 50, 20 } },
    .{ .chars = " , ", .rgb = .{ 180, 80, 30 } },
    .{ .chars = ".v.", .rgb = .{ 220, 120, 40 } },
    .{ .chars = "/v\\", .rgb = .{ 240, 160, 60 } },
    .{ .chars = "/^\\", .rgb = .{ 255, 200, 80 } },
    .{ .chars = "/*\\", .rgb = .{ 255, 230, 130 } },
};

const StatusView = struct {
    provider_kind: []const u8,
    model: []const u8,
    sources_count: usize,
};

/// Screen-coordinate text selection driven by mouse click+drag. On release
/// the selected region is pushed to the system clipboard via OSC 52 so the
/// user's normal Cmd+C / Ctrl+Shift+C pastes it.
const Selection = struct {
    start_row: u16,
    start_col: u16,
    end_row: u16,
    end_col: u16,
    active: bool = false,

    fn isSet(self: Selection) bool {
        return self.start_row != self.end_row or self.start_col != self.end_col;
    }

    fn ordered(self: Selection) struct { sr: u16, sc: u16, er: u16, ec: u16 } {
        if (self.start_row < self.end_row or
            (self.start_row == self.end_row and self.start_col <= self.end_col))
        {
            return .{ .sr = self.start_row, .sc = self.start_col, .er = self.end_row, .ec = self.end_col };
        }
        return .{ .sr = self.end_row, .sc = self.end_col, .er = self.start_row, .ec = self.start_col };
    }
};

const paste_char_threshold = 80;

pub fn run(init: std.process.Init, client: *rpc.Client, home: []const u8, theme_name: ?[]const u8) !void {
    const io = init.io;
    const allocator = init.gpa;

    var current_theme: Theme = blk: {
        if (theme_name) |n| {
            if (theme_mod.isPath(n)) {
                const resolved = resolveThemePath(allocator, home, n);
                defer if (resolved) |r| allocator.free(r);
                if (resolved) |path| {
                    if (theme_mod.loadFromFile(io, allocator, path)) |t| break :blk t;
                }
            }
            if (theme_mod.getByName(n)) |t| break :blk t;
        }
        const persisted = loadPersistedTheme(io, allocator, home);
        defer if (persisted) |p| allocator.free(p);
        if (persisted) |p| {
            if (theme_mod.getByName(p)) |t| break :blk t;
        }
        break :blk theme_mod.default();
    };

    var chat_lines: std.ArrayList(ChatLine) = .empty;
    defer {
        for (chat_lines.items) |line| {
            allocator.free(line.text);
            if (line.cache_info) |info| allocator.free(info);
        }
        chat_lines.deinit(allocator);
    }

    var scroll_offset: u16 = 0;
    var last_chat_h: u16 = 20;

    // Backing buffer for the status-bar text. vaxis.Window.print stores
    // grapheme slices by reference (no copy), so this buffer must outlive
    // each `vx.render` call — i.e. it lives for the entire run() scope.
    var status_buf: [128]u8 = undefined;

    var selection: ?Selection = null;

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

    // Command history (persisted to ~/.phoenix/history)
    const max_history = readHistorySize();
    var history: std.ArrayList([]const u8) = .empty;
    defer {
        for (history.items) |h| allocator.free(h);
        history.deinit(allocator);
    }
    loadHistory(&history, home, allocator, io, max_history);
    var history_index: ?usize = null;
    var saved_input: ?[]u8 = null;
    defer if (saved_input) |s| allocator.free(s);

    // Fetch initial config snapshot from the server.
    var status_view: StatusView = blk: {
        var snap_resp = client.getConfig() catch {
            break :blk .{
                .provider_kind = try allocator.dupe(u8, "none"),
                .model = try allocator.dupe(u8, ""),
                .sources_count = 0,
            };
        };
        defer snap_resp.response.deinit();
        if (snap_resp.snap.default_provider) |dp| {
            break :blk .{
                .provider_kind = try allocator.dupe(u8, @tagName(dp.kind)),
                .model = try allocator.dupe(u8, dp.model),
                .sources_count = snap_resp.snap.sources_count,
            };
        }
        break :blk .{
            .provider_kind = try allocator.dupe(u8, "none"),
            .model = try allocator.dupe(u8, ""),
            .sources_count = snap_resp.snap.sources_count,
        };
    };
    defer {
        allocator.free(status_view.provider_kind);
        allocator.free(status_view.model);
    }

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

    var picker: ?Picker = null;
    defer if (picker) |*p| p.deinit(allocator);

    var theme_before_preview: ?Theme = null;

    // /models page state. When non-null the TUI is in full-screen page mode:
    // chat input is suppressed and every key flows through the page.
    var models_page: ?ModelsPage = null;
    defer if (models_page) |*p| p.deinit(allocator);

    // Fetch the available slash commands once. The Response keeps the strings
    // alive for the life of the TUI; Picker.initCommand dupes onto its own
    // arena so we could free this earlier, but keeping it lets us re-open
    // the autocomplete picker at any time without another round trip.
    var cmd_list_items: []commands.CommandInfo = &.{};
    var cmd_list_response: ?rpc.client.Response = null;
    defer if (cmd_list_response) |*r| r.deinit();
    if (client.listCommands()) |result| {
        cmd_list_items = result.items;
        cmd_list_response = result.response;
    } else |_| {}

    // Two-press Ctrl+C exit (DESIGN.md §13.1). The first press warns; a
    // second within `ctrl_c_window_ns` aborts. Stored as ns from the
    // monotonic clock to avoid repeated syscalls.
    const ctrl_c_window_ns: u64 = 2 * std.time.ns_per_s;
    var last_ctrl_c_ns: u64 = 0;

    while (true) {
        const event = try loop.nextEvent();

        switch (event) {
            .paste_start => {
                if (models_page != null) continue;
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
                if (models_page) |*page| {
                    try page.handlePaste(text);
                } else {
                    // OSC 52 paste — insert all at once
                    try insertText(&input_lines, &cursor_line, &cursor_col, text, allocator);
                }
            },
            .key_press => |key| {
                if (models_page) |*page| {
                    const outcome = try page.handleKey(key, allocator, client);
                    switch (outcome) {
                        .stay => {},
                        .close => {
                            page.deinit(allocator);
                            models_page = null;
                        },
                        .activate_choice => |choice| {
                            var ar = client.applyModelChoice(choice) catch |err| {
                                try chat_lines.append(allocator, .{
                                    .role = .system,
                                    .text = try std.fmt.allocPrint(allocator, "/model: {s}", .{@errorName(err)}),
                                });
                                continue;
                            };
                            defer ar.response.deinit();

                            // Update local status view with the new active provider.
                            allocator.free(status_view.provider_kind);
                            allocator.free(status_view.model);
                            status_view.provider_kind = try allocator.dupe(u8, @tagName(ar.result.default_provider.kind));
                            status_view.model = try allocator.dupe(u8, ar.result.default_provider.model);

                            // Refetch entries so the * marker updates without
                            // the user having to leave the page.
                            if (client.dispatch("/models")) |refreshed| {
                                var r = refreshed;
                                defer r.response.deinit();
                                if (r.result == .models_page) {
                                    page.refresh(r.result.models_page.entries, ar.result.message) catch {};
                                }
                            } else |_| {
                                // Best-effort: at least flip the local flags so
                                // the * marker renders without a refetch.
                                for (page.entries) |*e| e.is_active = false;
                                if (choice.provider_index < page.entries.len) {
                                    page.entries[choice.provider_index].is_active = true;
                                }
                            }
                        },
                    }
                    try drawFrame(
                        &vx,
                        writer,
                        chat_lines.items,
                        input_lines.items,
                        cursor_line,
                        cursor_col,
                        if (picker) |*p| p else null,
                        if (models_page) |*p| p else null,
                        status_view,
                        &scroll_offset,
                        &last_chat_h,
                        allocator,
                        &status_buf,
                        0,
                        selection,
                        &current_theme,
                    );
                    continue;
                }

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

                if (key.matches('c', .{ .ctrl = true })) {
                    var ts: std.c.timespec = undefined;
                    const now_ns: u64 = if (std.c.clock_gettime(std.c.CLOCK.MONOTONIC, &ts) == 0)
                        @as(u64, @intCast(ts.sec)) * std.time.ns_per_s + @as(u64, @intCast(ts.nsec))
                    else
                        0;
                    if (last_ctrl_c_ns != 0 and now_ns -| last_ctrl_c_ns <= ctrl_c_window_ns) break;
                    last_ctrl_c_ns = now_ns;
                    try chat_lines.append(allocator, .{
                        .role = .system,
                        .text = try allocator.dupe(u8, "Press Ctrl+C again within 2s to exit (the session will be saved)."),
                    });
                    continue;
                }

                // Handle picker if active. The autocomplete picker is passive:
                // typing keys still go to the input box and Enter still
                // submits the line. Only Up/Down/Tab/Esc are intercepted.
                const autocomplete_active = if (picker) |*p| p.mode == .command_complete else false;
                // True when a navigation key was consumed by the picker — we
                // still want to redraw, but the bottom-of-switch input chain
                // must be skipped so Up/Down don't double-move and Tab
                // doesn't insert a literal '\t'.
                var picker_consumed_key = false;

                if (picker) |*p| {
                    if (key.codepoint == vaxis.Key.escape) {
                        if (theme_before_preview) |saved| {
                            current_theme = saved;
                            theme_before_preview = null;
                        }
                        p.deinit(allocator);
                        picker = null;
                        picker_consumed_key = true;
                    } else if (key.codepoint == vaxis.Key.up) {
                        p.moveUp();
                        if (p.selectedTheme()) |te| {
                            if (theme_mod.getByName(te.id)) |preview| current_theme = preview;
                        }
                        picker_consumed_key = true;
                    } else if (key.codepoint == vaxis.Key.down) {
                        p.moveDown();
                        if (p.selectedTheme()) |te| {
                            if (theme_mod.getByName(te.id)) |preview| current_theme = preview;
                        }
                        picker_consumed_key = true;
                    } else if (autocomplete_active) {
                        // Tab and Enter both complete: insert the highlighted
                        // command into the input and close the picker. A
                        // *second* Enter (with the picker now closed) submits
                        // the line. This matches the user-visible model
                        // "highlight, accept, then run".
                        const is_complete = key.codepoint == vaxis.Key.tab or
                            key.codepoint == vaxis.Key.enter or
                            key.codepoint == vaxis.Key.kp_enter;
                        if (is_complete) {
                            if (p.selectedCommand()) |c| {
                                clearInput(&input_lines, &cursor_line, &cursor_col, allocator);
                                try insertText(&input_lines, &cursor_line, &cursor_col, "/", allocator);
                                try insertText(&input_lines, &cursor_line, &cursor_col, c.name, allocator);
                                // Close the picker so the second Enter goes
                                // through normal submit. Skip the post-key
                                // refresh below by guarding via the flag.
                                p.deinit(allocator);
                                picker = null;
                            }
                            picker_consumed_key = true;
                        }
                        // Other keys (text, backspace, etc.) fall through to
                        // the normal input handling below.
                    } else {
                        if (key.codepoint == vaxis.Key.enter or key.codepoint == vaxis.Key.kp_enter) {
                            if (p.selectedModel()) |choice| {
                                var ar = client.applyModelChoice(choice) catch |err| {
                                    try chat_lines.append(allocator, .{
                                        .role = .system,
                                        .text = try std.fmt.allocPrint(allocator, "/model: {s}", .{@errorName(err)}),
                                    });
                                    p.deinit(allocator);
                                    picker = null;
                                    continue;
                                };
                                defer ar.response.deinit();

                                try chat_lines.append(allocator, .{
                                    .role = .system,
                                    .text = try allocator.dupe(u8, ar.result.message),
                                });

                                // Update local status view with new provider info.
                                allocator.free(status_view.provider_kind);
                                allocator.free(status_view.model);
                                status_view.provider_kind = try allocator.dupe(u8, @tagName(ar.result.default_provider.kind));
                                status_view.model = try allocator.dupe(u8, ar.result.default_provider.model);
                            } else if (p.selectedSession()) |choice| {
                                var ar = client.applySessionChoice(choice.id) catch |err| {
                                    try chat_lines.append(allocator, .{
                                        .role = .system,
                                        .text = try std.fmt.allocPrint(allocator, "/resume: {s}", .{@errorName(err)}),
                                    });
                                    p.deinit(allocator);
                                    picker = null;
                                    continue;
                                };
                                defer ar.response.deinit();

                                for (chat_lines.items) |line| {
                                    allocator.free(line.text);
                                    if (line.cache_info) |info| allocator.free(info);
                                }
                                chat_lines.clearRetainingCapacity();
                                scroll_offset = 0;
                                for (ar.result.messages) |m| {
                                    try chat_lines.append(allocator, .{
                                        .role = m.role,
                                        .text = try allocator.dupe(u8, m.content),
                                    });
                                }
                                try chat_lines.append(allocator, .{
                                    .role = .system,
                                    .text = try allocator.dupe(u8, ar.result.message),
                                });
                            } else if (p.selectedTheme()) |te| {
                                if (theme_mod.getByName(te.id)) |new_theme| {
                                    current_theme = new_theme;
                                    theme_before_preview = null;
                                    try chat_lines.append(allocator, .{
                                        .role = .system,
                                        .text = try std.fmt.allocPrint(allocator, "Theme set to {s}.", .{te.name}),
                                    });
                                    persistTheme(io, allocator, home, te.id);
                                }
                            }

                            p.deinit(allocator);
                            picker = null;
                            scroll_offset = 0;
                            continue;
                        }
                        // Modal pickers swallow everything else.
                        continue;
                    }
                }

                if (picker_consumed_key) {
                    // Picker handled the key; skip the input chain below but
                    // fall through to drawFrame so the change is visible.
                } else
                // Shift+Enter always inserts a newline
                if ((key.codepoint == vaxis.Key.enter or key.codepoint == vaxis.Key.kp_enter) and key.mods.shift) {
                    try insertNewline(&input_lines, &cursor_line, &cursor_col, allocator);
                } else if (key.codepoint == vaxis.Key.enter or key.codepoint == vaxis.Key.kp_enter) {
                    const full = try buildFullInput(&input_lines, allocator);

                    // If pasted regions were captured, splice their actual content
                    // back in before submitting.
                    const submitted: ?[]u8 = if (pending_full.items.len > 0) blk: {
                        const expanded = try expandPasteLabels(full, &pending_full, allocator);
                        allocator.free(full);
                        for (pending_full.items) |p2| allocator.free(p2);
                        pending_full.clearRetainingCapacity();
                        break :blk expanded;
                    } else if (full.len > 0) full else blk: {
                        allocator.free(full);
                        break :blk null;
                    };

                    if (submitted) |s| {
                        appendHistory(&history, s, home, allocator, io, max_history);
                        history_index = null;
                        if (saved_input) |si| {
                            allocator.free(si);
                            saved_input = null;
                        }

                        try handleSubmit(
                            &chat_lines,
                            &input_lines,
                            &cursor_line,
                            &cursor_col,
                            &picker,
                            &models_page,
                            &scroll_offset,
                            &last_chat_h,
                            &vx,
                            writer,
                            &status_view,
                            allocator,
                            client,
                            s,
                            &status_buf,
                            init,
                            home,
                            &current_theme,
                            io,
                            &theme_before_preview,
                        );
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
                    } else if (history.items.len > 0) {
                        if (history_index == null) {
                            saved_input = buildFullInput(&input_lines, allocator) catch null;
                            history_index = 0;
                        } else if (history_index.? + 1 < history.items.len) {
                            history_index = history_index.? + 1;
                        } else {
                            continue;
                        }
                        loadHistoryIntoInput(&input_lines, &cursor_line, &cursor_col, history.items[history.items.len - 1 - history_index.?], allocator);
                    }
                } else if (key.codepoint == vaxis.Key.down) {
                    if (history_index != null and cursor_line + 1 >= input_lines.items.len) {
                        if (history_index.? > 0) {
                            history_index = history_index.? - 1;
                            loadHistoryIntoInput(&input_lines, &cursor_line, &cursor_col, history.items[history.items.len - 1 - history_index.?], allocator);
                        } else {
                            history_index = null;
                            if (saved_input) |si| {
                                loadHistoryIntoInput(&input_lines, &cursor_line, &cursor_col, si, allocator);
                                allocator.free(si);
                                saved_input = null;
                            } else {
                                clearInput(&input_lines, &cursor_line, &cursor_col, allocator);
                            }
                        }
                    } else if (cursor_line + 1 < input_lines.items.len) {
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

                if (!picker_consumed_key) {
                    updateAutocompletePicker(&picker, cmd_list_items, &input_lines, allocator);
                }
            },
            .mouse => |mouse| {
                if (mouse.button == .wheel_up) {
                    scroll_offset +|= 3;
                } else if (mouse.button == .wheel_down) {
                    scroll_offset -|= 3;
                } else if (mouse.type == .press and mouse.button == .left) {
                    const r: u16 = if (mouse.row < 0) 0 else @intCast(mouse.row);
                    const c: u16 = if (mouse.col < 0) 0 else @intCast(mouse.col);
                    selection = .{
                        .start_row = r,
                        .start_col = c,
                        .end_row = r,
                        .end_col = c,
                        .active = true,
                    };
                } else if (mouse.type == .drag) {
                    if (selection) |*sel| {
                        sel.end_row = if (mouse.row < 0) 0 else @intCast(mouse.row);
                        sel.end_col = if (mouse.col < 0) 0 else @intCast(mouse.col);
                    }
                } else if (mouse.type == .release) {
                    if (selection) |*sel| {
                        sel.active = false;
                        sel.end_row = if (mouse.row < 0) 0 else @intCast(mouse.row);
                        sel.end_col = if (mouse.col < 0) 0 else @intCast(mouse.col);
                        if (sel.isSet()) {
                            copySelectionToClipboard(&vx, writer, sel.*, allocator);
                        } else {
                            selection = null;
                        }
                    }
                }
            },
            .winsize => |ws| try vx.resize(allocator, writer, ws),
            else => {},
        }

        try drawFrame(
            &vx,
            writer,
            chat_lines.items,
            input_lines.items,
            cursor_line,
            cursor_col,
            if (picker) |*p| p else null,
            if (models_page) |*p| p else null,
            status_view,
            &scroll_offset,
            &last_chat_h,
            allocator,
            &status_buf,
            0,
            selection,
            &current_theme,
        );
    }
}

fn drawFrame(
    vx: *vaxis.Vaxis,
    writer: *std.Io.Writer,
    chat_lines: []const ChatLine,
    input_lines: []const std.ArrayList(u8),
    cursor_line: usize,
    cursor_col: usize,
    picker: ?*Picker,
    models_page: ?*ModelsPage,
    status_view: StatusView,
    scroll_offset: *u16,
    last_chat_h: *u16,
    allocator: std.mem.Allocator,
    status_buf: []u8,
    anim_frame: u8,
    sel: ?Selection,
    t: *const Theme,
) !void {
    const win = vx.window();
    win.clear();

    const w = win.width;
    const h = win.height;
    if (w == 0 or h == 0) return;

    // Models page is full-screen and takes precedence over the chat layout.
    if (models_page) |page| {
        // Force a full repaint each frame. vaxis's diff renderer otherwise
        // leaves stale glyphs from the chat (or from a previous wizard step)
        // visible in cells that the new frame doesn't explicitly write to.
        // Same workaround the onboarding wizard uses.
        vx.queueRefresh();
        win.clear();
        vx.screen.cursor_vis = false;

        var page_arena = std.heap.ArenaAllocator.init(allocator);
        defer page_arena.deinit();
        page.paint(win, page_arena.allocator(), t);
        try vx.render(writer);
        try writer.flush();
        return;
    }

    const status_h: u16 = 1;
    // Input height: 1 line per input line, +2 for borders, min 3, max 10
    const raw_input_lines: u16 = @intCast(@min(input_lines.len, 8));
    const input_h: u16 = @min(raw_input_lines + 2, 10);

    const border_color: vaxis.Color = .{ .rgb = t.inputBorderColor() };

    if (picker) |p| {
        const picker_h = p.height(h);
        const chat_h = h -| input_h -| status_h -| picker_h;
        last_chat_h.* = chat_h;

        if (chat_h > 0) {
            const cwin = win.child(.{ .x_off = 0, .y_off = 0, .width = w, .height = chat_h });
            scroll_offset.* = drawChat(cwin, chat_lines, w, scroll_offset.*, allocator, anim_frame, t);
        }
        {
            const pwin = win.child(.{ .x_off = 0, .y_off = chat_h, .width = w, .height = picker_h });
            p.draw(pwin, t);
        }
        {
            const iwin = win.child(.{
                .x_off = 0,
                .y_off = chat_h + picker_h,
                .width = w,
                .height = input_h,
                .border = .{ .where = .{ .other = .{ .top = true, .bottom = true } }, .style = .{ .fg = border_color } },
            });
            drawInput(iwin, input_lines, cursor_line, cursor_col, vx, t);
        }
        {
            const swin = win.child(.{ .x_off = 0, .y_off = h - status_h, .width = w, .height = status_h });
            drawStatusBar(swin, w, scroll_offset.*, status_view, status_buf, t);
        }
    } else {
        const chat_h = h -| input_h -| status_h;
        last_chat_h.* = chat_h;
        if (chat_h > 0) {
            const cwin = win.child(.{ .x_off = 0, .y_off = 0, .width = w, .height = chat_h });
            scroll_offset.* = drawChat(cwin, chat_lines, w, scroll_offset.*, allocator, anim_frame, t);
        }
        {
            const iwin = win.child(.{
                .x_off = 0,
                .y_off = chat_h,
                .width = w,
                .height = input_h,
                .border = .{ .where = .{ .other = .{ .top = true, .bottom = true } }, .style = .{ .fg = border_color } },
            });
            drawInput(iwin, input_lines, cursor_line, cursor_col, vx, t);
        }
        {
            const swin = win.child(.{ .x_off = 0, .y_off = h - status_h, .width = w, .height = status_h });
            drawStatusBar(swin, w, scroll_offset.*, status_view, status_buf, t);
        }
    }

    if (sel) |s| applySelectionHighlight(&vx.screen, s);

    try vx.render(writer);
    try writer.flush();
}

/// Walk the screen buffer and reverse-video every cell inside the selection
/// rectangle. Called after all drawing but before vx.render() so the diff
/// renderer picks up the style change without an extra pass.
fn applySelectionHighlight(screen: *vaxis.Screen, s: Selection) void {
    const o = s.ordered();
    var row: u16 = o.sr;
    while (row <= o.er and row < screen.height) : (row += 1) {
        const col_start: u16 = if (row == o.sr) o.sc else 0;
        const col_end: u16 = if (row == o.er) o.ec else screen.width;
        var col: u16 = col_start;
        while (col < col_end and col < screen.width) : (col += 1) {
            const idx = @as(usize, row) * screen.width + col;
            if (idx < screen.buf.len) {
                screen.buf[idx].style.reverse = true;
            }
        }
    }
}

/// Read back grapheme content from the screen buffer for the given selection
/// and push it to the system clipboard via OSC 52.
fn copySelectionToClipboard(vx: *vaxis.Vaxis, writer: *std.Io.Writer, s: Selection, allocator: std.mem.Allocator) void {
    const text = extractSelectedText(&vx.screen, s, allocator) catch return;
    defer allocator.free(text);
    if (text.len == 0) return;
    vx.copyToSystemClipboard(writer, text, allocator) catch {};
}

/// Extract visible text from the screen buffer between the selection
/// endpoints. Trailing spaces on each row are trimmed so the result
/// is clean for pasting into an editor.
fn extractSelectedText(screen: *const vaxis.Screen, s: Selection, allocator: std.mem.Allocator) ![]u8 {
    const o = s.ordered();
    var result: std.ArrayList(u8) = .empty;
    errdefer result.deinit(allocator);

    var row: u16 = o.sr;
    while (row <= o.er and row < screen.height) : (row += 1) {
        if (row > o.sr) try result.append(allocator, '\n');
        const col_start: u16 = if (row == o.sr) o.sc else 0;
        const col_end: u16 = if (row == o.er) o.ec else screen.width;
        var col: u16 = col_start;
        const row_text_start = result.items.len;
        while (col < col_end and col < screen.width) : (col += 1) {
            if (screen.readCell(col, row)) |cell| {
                try result.appendSlice(allocator, cell.char.grapheme);
            }
        }
        // Trim trailing spaces from this row.
        while (result.items.len > row_text_start and result.items[result.items.len - 1] == ' ') {
            _ = result.pop();
        }
    }
    return result.toOwnedSlice(allocator);
}

/// Streaming context shared with the on_token / on_context / on_err / on_tick
/// callbacks for the duration of a single `sessionSend` call. Holds enough
/// state to re-render the screen on each token arrival (and each tick) so
/// the user sees the assistant reply as it's generated and the thinking
/// animation pulses while the model is silent.
const StreamCtx = struct {
    chat_lines: *std.ArrayList(ChatLine),
    assistant_idx: usize,
    streaming_buf: *std.ArrayList(u8),
    allocator: std.mem.Allocator,
    /// Animation frame counter for the "phoenix" thinking indicator.
    /// Bumped by on_tick; read by drawChat. Mod fire_frames.len at use.
    anim_frame: u8 = 0,
    /// Cap on how many content lines to render per tool call / tool error.
    /// Sourced from $PHOENIX_TOOL_OUTPUT_LINES at submit time.
    max_tool_lines: usize,
    // Drawing state — same set the main loop hands to drawFrame.
    vx: *vaxis.Vaxis,
    writer: *std.Io.Writer,
    input_lines: *std.ArrayList(std.ArrayList(u8)),
    cursor_line: usize,
    cursor_col: usize,
    picker: *?Picker,
    models_page: *?ModelsPage,
    status_view: StatusView,
    scroll_offset: *u16,
    last_chat_h: *u16,
    status_buf: []u8,
    theme: *const Theme,
};

const default_tool_output_lines: usize = 20;

/// Read $PHOENIX_TOOL_OUTPUT_LINES, falling back to the default. Values <= 0
/// are clamped to 1 so we always emit at least the header + one row.
fn readToolOutputLines() usize {
    const ptr = std.c.getenv("PHOENIX_TOOL_OUTPUT_LINES") orelse return default_tool_output_lines;
    const s = std.mem.span(ptr);
    if (s.len == 0) return default_tool_output_lines;
    const parsed = std.fmt.parseInt(i64, s, 10) catch return default_tool_output_lines;
    if (parsed <= 0) return 1;
    return @intCast(parsed);
}

fn streamRedraw(s: *StreamCtx) void {
    drawFrame(
        s.vx,
        s.writer,
        s.chat_lines.items,
        s.input_lines.items,
        s.cursor_line,
        s.cursor_col,
        if (s.picker.*) |*p| p else null,
        if (s.models_page.*) |*p| p else null,
        s.status_view,
        s.scroll_offset,
        s.last_chat_h,
        s.allocator,
        s.status_buf,
        s.anim_frame,
        null,
        s.theme,
    ) catch {};
}

fn onStreamTick(ctx_ptr: *anyopaque) void {
    const s: *StreamCtx = @ptrCast(@alignCast(ctx_ptr));
    s.anim_frame +%= 1;
    streamRedraw(s);
}

fn onStreamToken(ctx_ptr: *anyopaque, text: []const u8) void {
    const s: *StreamCtx = @ptrCast(@alignCast(ctx_ptr));
    s.streaming_buf.appendSlice(s.allocator, text) catch return;
    s.allocator.free(s.chat_lines.items[s.assistant_idx].text);
    s.chat_lines.items[s.assistant_idx].text =
        s.allocator.dupe(u8, s.streaming_buf.items) catch return;
    streamRedraw(s);
}

fn onStreamContext(ctx_ptr: *anyopaque, label: []const u8, body: []const u8) void {
    const s: *StreamCtx = @ptrCast(@alignCast(ctx_ptr));
    const header = std.fmt.allocPrint(s.allocator, "[{s} loaded]\n{s}", .{ label, body }) catch return;
    // Insert the system header just before the assistant placeholder so the
    // chat order reads: user, [skill loaded], assistant.
    s.chat_lines.insert(s.allocator, s.assistant_idx, .{
        .role = .system,
        .text = header,
    }) catch {
        s.allocator.free(header);
        return;
    };
    s.assistant_idx += 1;
    streamRedraw(s);
}

fn onStreamErr(ctx_ptr: *anyopaque, text: []const u8) void {
    const s: *StreamCtx = @ptrCast(@alignCast(ctx_ptr));
    const msg = std.fmt.allocPrint(s.allocator, "phoenix: stream error — {s}", .{text}) catch return;
    // Insert before the assistant placeholder so the bubble (if it ends up
    // populated) still appears last.
    s.chat_lines.insert(s.allocator, s.assistant_idx, .{
        .role = .system,
        .text = msg,
    }) catch {
        s.allocator.free(msg);
        return;
    };
    s.assistant_idx += 1;
    streamRedraw(s);
}

/// A tool call arrived. Finalize the current assistant bubble (drop it if
/// empty), render the call's header + truncated content as `.tool_call`
/// chat lines, and open a fresh assistant bubble so subsequent tokens land
/// in a visually distinct row from the round we just closed.
fn onStreamToolCall(ctx_ptr: *anyopaque, tool_id: []const u8, name: []const u8, args_json: []const u8) void {
    _ = tool_id;
    const s: *StreamCtx = @ptrCast(@alignCast(ctx_ptr));

    if (s.assistant_idx < s.chat_lines.items.len and
        s.chat_lines.items[s.assistant_idx].text.len == 0)
    {
        s.allocator.free(s.chat_lines.items[s.assistant_idx].text);
        _ = s.chat_lines.orderedRemove(s.assistant_idx);
    }

    appendToolCallLines(s.chat_lines, name, args_json, s.allocator, s.max_tool_lines) catch {};

    const new_text = s.allocator.dupe(u8, "") catch return;
    s.chat_lines.append(s.allocator, .{ .role = .assistant, .text = new_text }) catch {
        s.allocator.free(new_text);
        return;
    };
    s.assistant_idx = s.chat_lines.items.len - 1;
    s.streaming_buf.clearRetainingCapacity();
    streamRedraw(s);
}

/// A tool finished. We only surface errors — successful results would
/// double up with the tool_call line we already drew. Insert error lines
/// just before the freshly-opened assistant bubble.
fn onStreamToolResult(ctx_ptr: *anyopaque, tool_id: []const u8, output: []const u8, is_error: bool) void {
    _ = tool_id;
    if (!is_error) return;
    const s: *StreamCtx = @ptrCast(@alignCast(ctx_ptr));

    var buf: std.ArrayList(ChatLine) = .empty;
    defer buf.deinit(s.allocator);
    appendToolErrorLines(&buf, output, s.allocator, s.max_tool_lines) catch {
        for (buf.items) |line| s.allocator.free(line.text);
        return;
    };

    for (buf.items) |line| {
        s.chat_lines.insert(s.allocator, s.assistant_idx, line) catch {
            s.allocator.free(line.text);
            continue;
        };
        s.assistant_idx += 1;
    }
    streamRedraw(s);
}

/// Render a tool call into `chat_lines` as `.tool_call` rows. `write` and
/// `edit` get full content/diff treatment (the user wants to watch code
/// being written); `read` and `bash` get a one-line summary; unknown tools
/// fall back to "name + raw args".
fn appendToolCallLines(
    chat_lines: *std.ArrayList(ChatLine),
    name: []const u8,
    args_json: []const u8,
    allocator: std.mem.Allocator,
    max_lines: usize,
) !void {
    var parsed = std.json.parseFromSlice(std.json.Value, allocator, args_json, .{}) catch {
        const text = try std.fmt.allocPrint(allocator, "● {s} {s}", .{ name, args_json });
        try chat_lines.append(allocator, .{ .role = .tool_call, .text = text });
        return;
    };
    defer parsed.deinit();

    if (parsed.value != .object) {
        const text = try std.fmt.allocPrint(allocator, "● {s} {s}", .{ name, args_json });
        try chat_lines.append(allocator, .{ .role = .tool_call, .text = text });
        return;
    }
    const obj = parsed.value.object;

    if (std.mem.eql(u8, name, "write")) {
        const path = stringField(obj, "path", "?");
        const content = stringField(obj, "content", "");

        var line_count: usize = if (content.len == 0) 0 else 1;
        for (content) |ch| {
            if (ch == '\n') line_count += 1;
        }

        const header = try std.fmt.allocPrint(allocator, "● write {s} ({d} lines)", .{ path, line_count });
        try chat_lines.append(allocator, .{ .role = .tool_call, .text = header });

        var emitted: usize = 0;
        var line_no: usize = 1;
        var iter = std.mem.splitScalar(u8, content, '\n');
        while (iter.next()) |line| : (line_no += 1) {
            if (emitted >= max_lines) break;
            const formatted = try std.fmt.allocPrint(allocator, "  {d:>4}+ {s}", .{ line_no, line });
            try chat_lines.append(allocator, .{ .role = .tool_call, .text = formatted });
            emitted += 1;
        }
        if (line_count > emitted) {
            const more = line_count - emitted;
            const footer = try std.fmt.allocPrint(allocator, "  ... ({d} more line{s})", .{
                more,
                if (more == 1) "" else "s",
            });
            try chat_lines.append(allocator, .{ .role = .tool_call, .text = footer });
        }
    } else if (std.mem.eql(u8, name, "edit")) {
        const path = stringField(obj, "path", "?");
        const old_text = stringField(obj, "oldText", "");
        const new_text = stringField(obj, "newText", "");

        const header = try std.fmt.allocPrint(allocator, "● edit {s}", .{path});
        try chat_lines.append(allocator, .{ .role = .tool_call, .text = header });

        // Split the diff budget between old and new — half each, with the
        // remainder going to the "+" side so additions show fully on
        // single-line replacements.
        const old_budget = max_lines / 2;
        const new_budget = max_lines - old_budget;

        const old_total = try emitDiffLines(chat_lines, allocator, old_text, "  - ", old_budget);
        const new_total = try emitDiffLines(chat_lines, allocator, new_text, "  + ", new_budget);

        if (old_total > old_budget) {
            const footer = try std.fmt.allocPrint(allocator, "  ... ({d} more removed)", .{old_total - old_budget});
            try chat_lines.append(allocator, .{ .role = .tool_call, .text = footer });
        }
        if (new_total > new_budget) {
            const footer = try std.fmt.allocPrint(allocator, "  ... ({d} more added)", .{new_total - new_budget});
            try chat_lines.append(allocator, .{ .role = .tool_call, .text = footer });
        }
    } else if (std.mem.eql(u8, name, "read")) {
        const path = stringField(obj, "path", "?");
        const text = try std.fmt.allocPrint(allocator, "○ read {s}", .{path});
        try chat_lines.append(allocator, .{ .role = .tool_call, .text = text });
    } else if (std.mem.eql(u8, name, "bash")) {
        const cmd = stringField(obj, "command", "");
        const text = try std.fmt.allocPrint(allocator, "$ {s}", .{cmd});
        try chat_lines.append(allocator, .{ .role = .tool_call, .text = text });
    } else {
        const text = try std.fmt.allocPrint(allocator, "● {s} {s}", .{ name, args_json });
        try chat_lines.append(allocator, .{ .role = .tool_call, .text = text });
    }
}

/// Emit up to `budget` lines of `text` prefixed with `prefix`. Returns the
/// total number of lines `text` contains (so the caller can render a
/// "... N more" footer when truncated).
fn emitDiffLines(
    chat_lines: *std.ArrayList(ChatLine),
    allocator: std.mem.Allocator,
    text: []const u8,
    prefix: []const u8,
    budget: usize,
) !usize {
    if (text.len == 0) return 0;
    var iter = std.mem.splitScalar(u8, text, '\n');
    var emitted: usize = 0;
    var total: usize = 0;
    while (iter.next()) |line| {
        total += 1;
        if (emitted < budget) {
            const formatted = try std.fmt.allocPrint(allocator, "{s}{s}", .{ prefix, line });
            try chat_lines.append(allocator, .{ .role = .tool_call, .text = formatted });
            emitted += 1;
        }
    }
    return total;
}

fn appendToolErrorLines(
    chat_lines: *std.ArrayList(ChatLine),
    output: []const u8,
    allocator: std.mem.Allocator,
    max_lines: usize,
) !void {
    var iter = std.mem.splitScalar(u8, output, '\n');
    var emitted: usize = 0;
    while (iter.next()) |line| {
        if (emitted >= max_lines) {
            const footer = try allocator.dupe(u8, "  ... (output truncated)");
            try chat_lines.append(allocator, .{ .role = .tool_result, .text = footer });
            break;
        }
        const formatted = try std.fmt.allocPrint(allocator, "  ✗ {s}", .{line});
        try chat_lines.append(allocator, .{ .role = .tool_result, .text = formatted });
        emitted += 1;
    }
}

fn stringField(obj: std.json.ObjectMap, key: []const u8, fallback: []const u8) []const u8 {
    const v = obj.get(key) orelse return fallback;
    if (v != .string) return fallback;
    return v.string;
}

fn handleSubmit(
    chat_lines: *std.ArrayList(ChatLine),
    input_lines: *std.ArrayList(std.ArrayList(u8)),
    cursor_line: *usize,
    cursor_col: *usize,
    picker: *?Picker,
    models_page: *?ModelsPage,
    scroll_offset: *u16,
    last_chat_h: *u16,
    vx: *vaxis.Vaxis,
    writer: *std.Io.Writer,
    status_view: *StatusView,
    allocator: std.mem.Allocator,
    client: *rpc.Client,
    submitted: []u8,
    status_buf: []u8,
    init: std.process.Init,
    home: []const u8,
    current_theme: *Theme,
    io: std.Io,
    theme_before_preview: *?Theme,
) !void {
    // Echo the user's submission immediately. Whether the input turns out to
    // be a command or a chat message, the user sees what they typed.
    try chat_lines.append(allocator, .{ .role = .user, .text = submitted });

    // Reserve an empty assistant bubble that will mutate as tokens arrive.
    // Dropped later if the call resolves into a command outcome.
    try chat_lines.append(allocator, .{
        .role = .assistant,
        .text = try allocator.dupe(u8, ""),
    });
    const assistant_idx = chat_lines.items.len - 1;

    // Clear the input + reset scroll *before* the call so the redraw inside
    // the token callback shows an empty input box.
    scroll_offset.* = 0;
    clearInput(input_lines, cursor_line, cursor_col, allocator);

    var streaming_buf: std.ArrayList(u8) = .empty;
    defer streaming_buf.deinit(allocator);

    var stream_ctx = StreamCtx{
        .chat_lines = chat_lines,
        .assistant_idx = assistant_idx,
        .streaming_buf = &streaming_buf,
        .allocator = allocator,
        .max_tool_lines = readToolOutputLines(),
        .vx = vx,
        .writer = writer,
        .input_lines = input_lines,
        .cursor_line = cursor_line.*,
        .cursor_col = cursor_col.*,
        .picker = picker,
        .models_page = models_page,
        .status_view = status_view.*,
        .scroll_offset = scroll_offset,
        .last_chat_h = last_chat_h,
        .status_buf = status_buf,
        .theme = current_theme,
    };

    // Render once so the user sees their bubble + empty assistant before the
    // server starts streaming.
    streamRedraw(&stream_ctx);

    var sr = client.sessionSend(submitted, .{
        .ctx = &stream_ctx,
        .on_token = onStreamToken,
        .on_context = onStreamContext,
        .on_err = onStreamErr,
        .on_tool_call = onStreamToolCall,
        .on_tool_result = onStreamToolResult,
        .on_tick = onStreamTick,
    }) catch |err| {
        // The empty assistant bubble was reserved before the call; drop it
        // since nothing came back.
        allocator.free(chat_lines.items[stream_ctx.assistant_idx].text);
        _ = chat_lines.orderedRemove(stream_ctx.assistant_idx);
        const msg = try std.fmt.allocPrint(allocator, "phoenix: rpc error — {s}", .{@errorName(err)});
        try chat_lines.append(allocator, .{ .role = .system, .text = msg });
        return;
    };
    defer sr.response.deinit();

    // After streaming finishes the callback has already kept assistant_idx in
    // sync if context events shifted it. Use stream_ctx.assistant_idx as the
    // authoritative index.
    const asst_idx = stream_ctx.assistant_idx;

    switch (sr.outcome) {
        .conversation => |c| {
            // For both error and empty-token outcomes, drop the empty assistant
            // bubble (so the chat doesn't show a blank "you/assistant" turn)
            // and emit a phoenix system line in its place. Diagnostics belong
            // to the harness, not the model.
            if (!c.ok or streaming_buf.items.len == 0) {
                allocator.free(chat_lines.items[asst_idx].text);
                _ = chat_lines.orderedRemove(asst_idx);

                const msg = if (!c.ok)
                    try std.fmt.allocPrint(allocator, "phoenix: provider error — {s}", .{c.reason})
                else
                    try std.fmt.allocPrint(
                        allocator,
                        "phoenix: no response (stop_reason={s}, tokens in={d} out={d}). Check /tmp/phoenix-rpc.log.",
                        .{ c.stop_reason, c.input_tokens, c.output_tokens },
                    );
                try chat_lines.append(allocator, .{ .role = .system, .text = msg });
            } else if (c.input_tokens > 0) {
                var cb: [16]u8 = undefined;
                var tb: [16]u8 = undefined;
                const cached_str = fmtTokenCount(&cb, c.cache_read_input_tokens);
                const total_str = fmtTokenCount(&tb, c.input_tokens);
                const pct = if (c.input_tokens > 0) (c.cache_read_input_tokens * 100) / c.input_tokens else 0;
                chat_lines.items[asst_idx].cache_info = try std.fmt.allocPrint(
                    allocator,
                    "cached {s}/{s} ({d}%)",
                    .{ cached_str, total_str, pct },
                );
            }
        },
        .command => |cmd| {
            // No AI tokens came through. Drop the empty assistant placeholder.
            allocator.free(chat_lines.items[asst_idx].text);
            _ = chat_lines.orderedRemove(asst_idx);

            switch (cmd) {
                .not_a_command => {
                    // Server should never return this for session.send; treat
                    // as a no-op.
                },
                .message => |m| try chat_lines.append(allocator, .{
                    .role = .system,
                    .text = try allocator.dupe(u8, m),
                }),
                .cleared, .compacted => |m| {
                    for (chat_lines.items) |line| {
                        allocator.free(line.text);
                        if (line.cache_info) |info| allocator.free(info);
                    }
                    chat_lines.clearRetainingCapacity();
                    scroll_offset.* = 0;
                    try chat_lines.append(allocator, .{
                        .role = .system,
                        .text = try allocator.dupe(u8, m),
                    });
                },
                .err => |m| try chat_lines.append(allocator, .{
                    .role = .system,
                    .text = try allocator.dupe(u8, m),
                }),
                .model_picker => |p| {
                    if (picker.*) |*old| old.deinit(allocator);
                    picker.* = try Picker.initModel(allocator, .{
                        .title = p.title,
                        .choices = p.choices,
                    });
                },
                .session_picker => |p| {
                    if (picker.*) |*old| old.deinit(allocator);
                    picker.* = try Picker.initSession(allocator, .{
                        .title = p.title,
                        .choices = p.choices,
                    });
                },
                .models_page => |p| {
                    if (models_page.*) |*old| old.deinit(allocator);
                    models_page.* = try ModelsPage.init(allocator, .{
                        .title = p.title,
                        .entries = p.entries,
                    });
                },
                .inject_context => |frag| {
                    // Reachable when a skill is invoked with no follow-up
                    // text. The TUI already rendered the system header via
                    // the `context` event; nothing more to do unless the
                    // server omitted the event (older builds).
                    _ = frag;
                },
                .theme_picker => |tp| {
                    if (tp.requested) |name| {
                        if (theme_mod.getByName(name)) |new_theme| {
                            current_theme.* = new_theme;
                            try chat_lines.append(allocator, .{
                                .role = .system,
                                .text = try std.fmt.allocPrint(allocator, "Theme set to {s}.", .{name}),
                            });
                            persistTheme(io, allocator, home, name);
                        } else {
                            try chat_lines.append(allocator, .{
                                .role = .system,
                                .text = try std.fmt.allocPrint(allocator, "Unknown theme: {s}. Use /theme to see available themes.", .{name}),
                            });
                        }
                    } else {
                        if (picker.*) |*old| old.deinit(allocator);
                        theme_before_preview.* = current_theme.*;
                        picker.* = try Picker.initTheme(allocator, current_theme.name);
                    }
                },
                .connect_wizard => {
                    allocator.free(chat_lines.items[asst_idx].text);
                    _ = chat_lines.orderedRemove(asst_idx);

                    var wizard = add_model_wizard.Wizard.init(allocator);
                    defer wizard.deinit();

                    var buffer: [4096]u8 = undefined;
                    var tty = try vaxis.Tty.init(init.io, &buffer);
                    defer tty.deinit();
                    const tty_writer = tty.writer();

                    var vx_wiz = try vaxis.init(init.io, allocator, init.environ_map, .{});
                    defer vx_wiz.deinit(allocator, tty_writer);

                    var loop: vaxis.Loop(VxEvent) = .init(init.io, &tty, &vx_wiz);
                    try loop.start();
                    defer loop.stop();

                    try vx_wiz.enterAltScreen(tty_writer);
                    try vx_wiz.queryTerminal(tty_writer, .fromSeconds(1));
                    try tty_writer.flush();

                    var wizard_arena = std.heap.ArenaAllocator.init(allocator);
                    defer wizard_arena.deinit();

                    wizard.paint(vx_wiz.window(), wizard_arena.allocator());
                    try vx_wiz.render(tty_writer);
                    try tty_writer.flush();

                    var wizard_outcome: add_model_wizard.Outcome = .in_progress;
                    while (wizard_outcome == .in_progress) {
                        const event = try loop.nextEvent();
                        switch (event) {
                            .winsize => |ws| try vx_wiz.resize(allocator, tty_writer, ws),
                            .focus_in, .focus_out, .paste_start, .paste_end => {},
                            .paste => |s| try wizard.handlePaste(s),
                            .mouse => {},  // ignore mouse events in wizard
                            .key_press => |key| {
                                wizard_outcome = try wizard.handleKey(key);
                            },
                        }
                        _ = wizard_arena.reset(.retain_capacity);
                        wizard.paint(vx_wiz.window(), wizard_arena.allocator());
                        try vx_wiz.render(tty_writer);
                        try tty_writer.flush();
                    }

                    try vx_wiz.exitAltScreen(tty_writer);
                    try tty_writer.flush();

                    if (wizard_outcome == .completed) {
                        const result = wizard_outcome.completed;
                        var add_result = client.addModel(.{
                            .kind = result.kind,
                            .model = result.model,
                            .api_key = result.api_key,
                            .base_url = result.base_url,
                            .context_window = result.context_window,
                        }) catch |err| {
                            try chat_lines.append(allocator, .{
                                .role = .system,
                                .text = try std.fmt.allocPrint(allocator, "/connect: {s}", .{@errorName(err)}),
                            });
                            return;
                        };
                        defer add_result.response.deinit();

                        if (add_result.result.entries.len == 0) {
                            try chat_lines.append(allocator, .{
                                .role = .system,
                                .text = try allocator.dupe(u8, add_result.result.message),
                            });
                            scroll_offset.* = 0;
                            return;
                        }

                        const newest = add_result.result.entries[add_result.result.entries.len - 1];
                        var apply_result = client.applyModelChoice(.{
                            .provider_index = newest.provider_index,
                            .kind = newest.kind,
                            .model = newest.model,
                            .is_active = newest.is_active,
                        }) catch |err| {
                            try chat_lines.append(allocator, .{
                                .role = .system,
                                .text = try std.fmt.allocPrint(allocator, "/connect: added provider but failed to activate it: {s}", .{@errorName(err)}),
                            });
                            scroll_offset.* = 0;
                            return;
                        };
                        defer apply_result.response.deinit();

                        allocator.free(status_view.provider_kind);
                        allocator.free(status_view.model);
                        status_view.provider_kind = try allocator.dupe(u8, @tagName(apply_result.result.default_provider.kind));
                        status_view.model = try allocator.dupe(u8, apply_result.result.default_provider.model);

                        try chat_lines.append(allocator, .{
                            .role = .system,
                            .text = try allocator.dupe(u8, apply_result.result.message),
                        });
                    } else {
                        try chat_lines.append(allocator, .{
                            .role = .system,
                            .text = try allocator.dupe(u8, "/connect: cancelled"),
                        });
                    }
                    scroll_offset.* = 0;
                },
            }
        },
    }
}

/// Open, update, or close the inline slash-command autocomplete picker based
/// on the current input. Leaves non-autocomplete pickers (model/session)
/// untouched. Allocation failures collapse silently — the picker is just a
/// hint and shouldn't break input handling.
fn updateAutocompletePicker(
    picker: *?Picker,
    cmd_list: []commands.CommandInfo,
    input_lines: *std.ArrayList(std.ArrayList(u8)),
    allocator: std.mem.Allocator,
) void {
    if (picker.*) |*p| {
        if (p.mode != .command_complete) return;
    }

    const should_show = blk: {
        if (cmd_list.len == 0) break :blk false;
        if (input_lines.items.len != 1) break :blk false;
        const line = input_lines.items[0].items;
        if (line.len == 0 or line[0] != '/') break :blk false;
        if (line.len >= 2 and line[1] == '/') break :blk false; // escape: `//`
        for (line[1..]) |c| {
            if (c == ' ' or c == '\t') break :blk false; // past the command name
        }
        break :blk true;
    };

    if (!should_show) {
        if (picker.*) |*p| {
            p.deinit(allocator);
            picker.* = null;
        }
        return;
    }

    const prefix = input_lines.items[0].items[1..];
    if (picker.*) |*p| {
        p.setCommandFilter(prefix);
    } else {
        var p = Picker.initCommand(allocator, cmd_list) catch return;
        p.setCommandFilter(prefix);
        picker.* = p;
    }
}

fn persistTheme(io: std.Io, allocator: std.mem.Allocator, home: []const u8, theme_id: []const u8) void {
    const dir = std.fs.path.join(allocator, &.{ home, ".phoenix" }) catch return;
    defer allocator.free(dir);
    const file_path = std.fs.path.join(allocator, &.{ dir, "theme" }) catch return;
    defer allocator.free(file_path);
    std.Io.Dir.cwd().createDirPath(io, dir) catch return;
    std.Io.Dir.cwd().writeFile(io, .{
        .sub_path = file_path,
        .data = theme_id,
    }) catch {};
}

fn loadPersistedTheme(io: std.Io, allocator: std.mem.Allocator, home: []const u8) ?[]const u8 {
    const file_path = std.fs.path.join(allocator, &.{ home, ".phoenix", "theme" }) catch return null;
    defer allocator.free(file_path);
    const data = std.Io.Dir.cwd().readFileAlloc(io, file_path, allocator, .limited(256)) catch return null;
    defer allocator.free(data);
    const trimmed = std.mem.trim(u8, data, " \t\n\r");
    if (trimmed.len == 0) return null;
    if (theme_mod.getByName(trimmed) != null) {
        return allocator.dupe(u8, trimmed) catch null;
    }
    return null;
}

fn resolveThemePath(allocator: std.mem.Allocator, home: []const u8, path: []const u8) ?[]const u8 {
    if (path.len > 1 and path[0] == '~' and path[1] == '/') {
        return std.fs.path.join(allocator, &.{ home, path[2..] }) catch null;
    }
    return allocator.dupe(u8, path) catch null;
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

const default_history_size: usize = 50;

fn readHistorySize() usize {
    const ptr = std.c.getenv("PHOENIX_HISTORY_SIZE") orelse return default_history_size;
    const s = std.mem.span(ptr);
    if (s.len == 0) return default_history_size;
    const parsed = std.fmt.parseInt(i64, s, 10) catch return default_history_size;
    if (parsed <= 0) return 0;
    return @intCast(parsed);
}

fn loadHistory(
    history: *std.ArrayList([]const u8),
    home: []const u8,
    allocator: std.mem.Allocator,
    io: std.Io,
    max_entries: usize,
) void {
    if (max_entries == 0) return;
    const path = std.fs.path.join(allocator, &.{ home, ".phoenix", "history" }) catch return;
    defer allocator.free(path);

    const content = std.Io.Dir.cwd().readFileAlloc(io, path, allocator, .limited(10 * 1024 * 1024)) catch return;
    defer allocator.free(content);

    var iter = std.mem.splitScalar(u8, content, '\n');
    while (iter.next()) |line| {
        if (line.len == 0) continue;
        const unescaped = unescapeHistoryLine(line, allocator) catch continue;
        history.append(allocator, unescaped) catch {
            allocator.free(unescaped);
            continue;
        };
    }

    while (history.items.len > max_entries) {
        allocator.free(history.items[0]);
        _ = history.orderedRemove(0);
    }
}

fn appendHistory(
    history: *std.ArrayList([]const u8),
    text: []const u8,
    home: []const u8,
    allocator: std.mem.Allocator,
    io: std.Io,
    max_entries: usize,
) void {
    if (max_entries == 0) return;
    if (text.len == 0) return;

    const duped = allocator.dupe(u8, text) catch return;
    history.append(allocator, duped) catch {
        allocator.free(duped);
        return;
    };

    while (history.items.len > max_entries) {
        allocator.free(history.items[0]);
        _ = history.orderedRemove(0);
    }

    saveHistoryFile(history, home, allocator, io);
}

fn saveHistoryFile(
    history: *std.ArrayList([]const u8),
    home: []const u8,
    allocator: std.mem.Allocator,
    io: std.Io,
) void {
    const dir_path = std.fs.path.join(allocator, &.{ home, ".phoenix" }) catch return;
    defer allocator.free(dir_path);
    std.Io.Dir.cwd().createDirPath(io, dir_path) catch {};

    const path = std.fs.path.join(allocator, &.{ home, ".phoenix", "history" }) catch return;
    defer allocator.free(path);

    var buf: std.ArrayList(u8) = .empty;
    defer buf.deinit(allocator);
    for (history.items) |entry| {
        escapeHistoryLine(&buf, entry, allocator);
        buf.append(allocator, '\n') catch {};
    }

    std.Io.Dir.cwd().writeFile(io, .{ .sub_path = path, .data = buf.items }) catch {};
}

fn escapeHistoryLine(buf: *std.ArrayList(u8), text: []const u8, allocator: std.mem.Allocator) void {
    for (text) |ch| {
        if (ch == '\n') {
            buf.appendSlice(allocator, "\\n") catch {};
        } else if (ch == '\\') {
            buf.appendSlice(allocator, "\\\\") catch {};
        } else {
            buf.append(allocator, ch) catch {};
        }
    }
}

fn unescapeHistoryLine(line: []const u8, allocator: std.mem.Allocator) ![]u8 {
    var result: std.ArrayList(u8) = .empty;
    errdefer result.deinit(allocator);
    var i: usize = 0;
    while (i < line.len) {
        if (line[i] == '\\' and i + 1 < line.len) {
            if (line[i + 1] == 'n') {
                try result.append(allocator, '\n');
                i += 2;
            } else if (line[i + 1] == '\\') {
                try result.append(allocator, '\\');
                i += 2;
            } else {
                try result.append(allocator, line[i]);
                i += 1;
            }
        } else {
            try result.append(allocator, line[i]);
            i += 1;
        }
    }
    return try result.toOwnedSlice(allocator);
}

fn loadHistoryIntoInput(
    input_lines: *std.ArrayList(std.ArrayList(u8)),
    cursor_line: *usize,
    cursor_col: *usize,
    text: []const u8,
    allocator: std.mem.Allocator,
) void {
    for (input_lines.items) |*line| line.deinit(allocator);
    input_lines.clearRetainingCapacity();

    var iter = std.mem.splitScalar(u8, text, '\n');
    while (iter.next()) |line_text| {
        var new_line: std.ArrayList(u8) = .empty;
        new_line.appendSlice(allocator, line_text) catch {};
        input_lines.append(allocator, new_line) catch {};
    }

    if (input_lines.items.len == 0) {
        input_lines.append(allocator, .empty) catch {};
    }

    cursor_line.* = input_lines.items.len - 1;
    cursor_col.* = input_lines.items[cursor_line.*].items.len;
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

fn drawInput(win: vaxis.Window, lines: []const std.ArrayList(u8), cursor_line: usize, cursor_col: usize, vx: *vaxis.Vaxis, t: *const Theme) void {
    const inner_h = win.height;
    const inner_w = win.width;
    if (inner_h == 0 or inner_w == 0) return;

    const text_style: vaxis.Style = .{ .fg = .{ .rgb = t.foreground } };
    const cursor_style: vaxis.Style = .{ .fg = .{ .rgb = t.cursorFg() }, .bg = .{ .rgb = t.cursorBg() } };

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

/// Render the status bar. `buf` must outlive the surrounding `vx.render` call:
/// vaxis stores grapheme slices by reference, not by copy, so a stack-local
/// buffer here would dangle by the time render walks the cells.
fn drawStatusBar(win: vaxis.Window, w: u16, scroll_offset: u16, view: StatusView, buf: []u8, t: *const Theme) void {
    const style: vaxis.Style = .{
        .fg = .{ .rgb = t.statusBarFg() },
        .bg = .{ .rgb = t.statusBarBg() },
        .bold = true,
    };
    for (0..w) |x| {
        win.writeCell(@intCast(x), 0, .{ .char = .{ .grapheme = " " }, .style = style });
    }
    if (scroll_offset > 0) {
        const indicator = std.fmt.bufPrint(buf, " phoenix v0.0.0 | scrolled +{d} rows", .{scroll_offset}) catch " phoenix v0.0.0";
        _ = win.print(&.{.{ .text = indicator, .style = style }}, .{});
    } else {
        const text = std.fmt.bufPrint(buf, " phoenix v0.0.0 | provider: {s} | model: {s} | sources: {d}", .{
            view.provider_kind,
            view.model,
            view.sources_count,
        }) catch " phoenix v0.0.0";
        _ = win.print(&.{.{ .text = text, .style = style }}, .{});
    }
}

const bubble_padding = 2;
const max_bubble_pct = 70;

fn isToolRole(role: core.Role) bool {
    return role == .tool_call or role == .tool_result;
}

/// Detect markdown fenced code block delimiters: lines that are just
/// ``` (optionally followed by a language tag like ```go).
fn isFenceLine(text: []const u8) bool {
    var i: usize = 0;
    while (i < text.len and text[i] == ' ') i += 1;
    if (text.len - i < 3) return false;
    if (text[i] != '`') return false;
    var ticks: usize = 0;
    while (i < text.len and text[i] == '`') {
        ticks += 1;
        i += 1;
    }
    if (ticks < 3) return false;
    while (i < text.len) : (i += 1) {
        const ch = text[i];
        if (ch != ' ' and !std.ascii.isAlphanumeric(ch) and ch != '-' and ch != '_' and ch != '+') return false;
    }
    return true;
}

/// Parse a single line of text for inline markdown and fill `segs` with
/// styled segments. Returns the number of segments written.
///
/// Handles:
///   `# heading`   → bold, `#` prefix stripped
///   `**bold**`    → bold attribute, markers hidden
///   `` `code` ``  → amber fg, backticks hidden
fn parseInlineMarkdown(
    text: []const u8,
    base_style: vaxis.Style,
    segs: []vaxis.Cell.Segment,
    t: *const Theme,
) usize {
    if (text.len == 0 or segs.len == 0) return 0;

    // Headings: strip leading '#'s and the following space.
    if (text[0] == '#') {
        var skip: usize = 0;
        while (skip < text.len and text[skip] == '#') skip += 1;
        if (skip < text.len and text[skip] == ' ') skip += 1;
        var style = base_style;
        style.bold = true;
        segs[0] = .{ .text = text[skip..], .style = style };
        return 1;
    }

    var count: usize = 0;
    var i: usize = 0;
    var seg_start: usize = 0;
    var in_bold = false;
    var in_code = false;

    while (i < text.len) {
        if (!in_code and i + 1 < text.len and text[i] == '*' and text[i + 1] == '*') {
            if (i > seg_start and count < segs.len) {
                var style = base_style;
                if (in_bold) style.bold = true;
                segs[count] = .{ .text = text[seg_start..i], .style = style };
                count += 1;
            }
            in_bold = !in_bold;
            i += 2;
            seg_start = i;
        } else if (text[i] == '`') {
            if (i > seg_start and count < segs.len) {
                var style = base_style;
                if (in_bold) style.bold = true;
                if (in_code) style.fg = .{ .rgb = t.codeFg() };
                segs[count] = .{ .text = text[seg_start..i], .style = style };
                count += 1;
            }
            in_code = !in_code;
            i += 1;
            seg_start = i;
        } else {
            i += 1;
        }
    }

    if (seg_start < text.len and count < segs.len) {
        var style = base_style;
        if (in_bold) style.bold = true;
        if (in_code) style.fg = .{ .rgb = .{ 220, 190, 130 } };
        segs[count] = .{ .text = text[seg_start..], .style = style };
        count += 1;
    }

    return count;
}

/// Pick a color for a tool_call line based on its diff prefix.
///   "  + ..." / "  1234+ ..." → green   (additions / write content)
///   "  - ..."                  → red     (removals)
///   everything else            → dim blue (headers, footers, summaries)
fn toolLineStyle(text: []const u8, t: *const Theme) vaxis.Style {
    var start: usize = 0;
    while (start < text.len and text[start] == ' ') start += 1;
    const trimmed = text[start..];
    if (trimmed.len > 0) {
        if (trimmed[0] == '+') return .{ .fg = .{ .rgb = t.diff_add } };
        if (trimmed[0] == '-') return .{ .fg = .{ .rgb = t.diff_delete } };
        if (std.ascii.isDigit(trimmed[0])) {
            for (trimmed) |ch| {
                if (ch == '+') return .{ .fg = .{ .rgb = t.diff_add } };
                if (!std.ascii.isDigit(ch)) break;
            }
        }
    }
    return .{ .fg = .{ .rgb = t.toolDefaultColor() } };
}

fn fmtTokenCount(buf: []u8, n: u32) []const u8 {
    if (n >= 1_000_000) {
        const whole = n / 1_000_000;
        const frac = (n % 1_000_000) / 100_000;
        if (frac > 0) {
            return std.fmt.bufPrint(buf, "{d}.{d}m", .{ whole, frac }) catch "?";
        }
        return std.fmt.bufPrint(buf, "{d}m", .{whole}) catch "?";
    }
    if (n >= 1_000) {
        const whole = n / 1_000;
        const frac = (n % 1_000) / 100;
        if (frac > 0) {
            return std.fmt.bufPrint(buf, "{d}.{d}k", .{ whole, frac }) catch "?";
        }
        return std.fmt.bufPrint(buf, "{d}k", .{whole}) catch "?";
    }
    return std.fmt.bufPrint(buf, "{d}", .{n}) catch "?";
}

test "fmtTokenCount formats small numbers" {
    var buf: [16]u8 = undefined;
    try std.testing.expectEqualStrings("0", fmtTokenCount(&buf, 0));
    try std.testing.expectEqualStrings("999", fmtTokenCount(&buf, 999));
    try std.testing.expectEqualStrings("500", fmtTokenCount(&buf, 500));
}

test "fmtTokenCount formats thousands" {
    var buf: [16]u8 = undefined;
    try std.testing.expectEqualStrings("1k", fmtTokenCount(&buf, 1000));
    try std.testing.expectEqualStrings("1.2k", fmtTokenCount(&buf, 1200));
    try std.testing.expectEqualStrings("5k", fmtTokenCount(&buf, 5000));
    try std.testing.expectEqualStrings("10.5k", fmtTokenCount(&buf, 10500));
    try std.testing.expectEqualStrings("999.9k", fmtTokenCount(&buf, 999900));
}

test "fmtTokenCount formats millions" {
    var buf: [16]u8 = undefined;
    try std.testing.expectEqualStrings("1m", fmtTokenCount(&buf, 1000000));
    try std.testing.expectEqualStrings("1.5m", fmtTokenCount(&buf, 1500000));
    try std.testing.expectEqualStrings("2m", fmtTokenCount(&buf, 2000000));
}

fn bubbleWidth(text_len: usize, win_width: u16) u16 {
    const max_w = @max(20, (win_width * max_bubble_pct) / 100);
    const content_w: u16 = @intCast(@min(text_len + bubble_padding * 2, max_w));
    return content_w;
}

fn wrapLines(text: []const u8, width: usize, allocator: std.mem.Allocator) !std.ArrayList([]const u8) {
    var result: std.ArrayList([]const u8) = .empty;
    if (width == 0) return result;

    // Split on hard newlines first, then soft-wrap each paragraph.
    var line_iter = std.mem.splitScalar(u8, text, '\n');
    while (line_iter.next()) |paragraph| {
        if (paragraph.len == 0) {
            try result.append(allocator, "");
            continue;
        }
        var pos: usize = 0;
        while (pos < paragraph.len) {
            var end = @min(pos + width, paragraph.len);
            if (end < paragraph.len and end > pos) {
                var break_at = end;
                while (break_at > pos and paragraph[break_at] != ' ') {
                    break_at -= 1;
                }
                if (break_at > pos) {
                    end = break_at + 1;
                }
            }
            try result.append(allocator, paragraph[pos..@min(end, paragraph.len)]);
            pos = end;
        }
    }
    if (result.items.len == 0) {
        try result.append(allocator, "");
    }
    return result;
}

fn drawChat(win: vaxis.Window, lines: []const ChatLine, win_width: u16, scroll_offset: u16, allocator: std.mem.Allocator, anim_frame: u8, t: *const Theme) u16 {
    if (lines.len == 0) return 0;

    const rows: i64 = win.height;
    if (rows == 0 or win_width < 10) return 0;

    var total_rows: u32 = 0;
    var row_counts: [512]u32 = undefined;
    const line_count = @min(lines.len, 512);

    for (0..line_count) |i| {
        const line = lines[i];
        const prev_role: ?core.Role = if (i > 0) lines[i - 1].role else null;
        // Tighten consecutive tool rows so the body of one call (header +
        // many "+ ..." lines) reads as a single block without empty rows
        // between every entry.
        const tight = isToolRole(line.role) and prev_role != null and isToolRole(prev_role.?);
        const spacing: u32 = if (i > 0 and !tight) 1 else 0;

        if (line.role == .system or isToolRole(line.role)) {
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

        const meta_row: u32 = if (line.cache_info != null) 1 else 0;
        row_counts[i] = spacing + 1 + @as(u32, @intCast(wrapped.items.len)) + meta_row;
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
        const is_tool = isToolRole(line.role);
        const prev_role: ?core.Role = if (i > 0) lines[i - 1].role else null;
        const tight = is_tool and prev_role != null and isToolRole(prev_role.?);
        var cr = vrow;

        if (i > 0 and !tight) cr += 1;

        if (is_system) {
            const sy = cr - visible_top;
            if (sy >= 0 and sy < rows) {
                const y: u16 = @intCast(sy);
                const text_len: u16 = @intCast(@min(line.text.len, win_width));
                const x_off: u16 = (win_width -| text_len) / 2;
                const sys_style: vaxis.Style = .{ .fg = .{ .rgb = t.dim() }, .italic = true };
                const sys_win = win.child(.{ .x_off = x_off, .y_off = y, .width = text_len, .height = 1 });
                _ = sys_win.print(&.{.{ .text = line.text, .style = sys_style }}, .{});
            }
            vrow = msg_end;
            continue;
        }

        if (is_tool) {
            const sy = cr - visible_top;
            if (sy >= 0 and sy < rows) {
                const y: u16 = @intCast(sy);
                const text_len: u16 = @intCast(@min(line.text.len, win_width -| 2));
                const tool_style: vaxis.Style = if (line.role == .tool_result)
                    .{ .fg = .{ .rgb = t.toolResultColor() } }
                else
                    toolLineStyle(line.text, t);
                const tool_win = win.child(.{ .x_off = 1, .y_off = y, .width = text_len, .height = 1 });
                _ = tool_win.print(&.{.{ .text = line.text, .style = tool_style }}, .{});
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
                .{ .fg = .{ .rgb = t.userLabelColor() }, .bold = true }
            else
                .{ .fg = .{ .rgb = t.assistantLabelColor() }, .bold = true };

            const label_x = if (is_right) x_off + bw - @as(u16, @intCast(label.len)) else x_off;
            const label_win = win.child(.{
                .x_off = label_x,
                .y_off = y,
                .width = @intCast(@min(label.len, win_width)),
                .height = 1,
            });
            _ = label_win.print(&.{.{ .text = label, .style = label_style }}, .{});

            // Fire animation: only on assistant turn, only while the bubble
            // is empty (i.e. waiting for tokens). Lives one space to the
            // right of the label so it never collides with the bubble below.
            if (!is_right and line.text.len == 0) {
                const f = fire_frames[anim_frame % fire_frames.len];
                const fire_x: u16 = label_x + @as(u16, @intCast(label.len)) + 1;
                const fire_w: u16 = @intCast(f.chars.len);
                if (fire_x + fire_w <= win_width) {
                    const fire_style: vaxis.Style = .{ .fg = .{ .rgb = f.rgb }, .bold = true };
                    const fwin = win.child(.{
                        .x_off = fire_x,
                        .y_off = y,
                        .width = fire_w,
                        .height = 1,
                    });
                    for (0..f.chars.len) |ci| {
                        fwin.writeCell(@intCast(ci), 0, .{
                            .char = .{ .grapheme = f.chars[ci .. ci + 1] },
                            .style = fire_style,
                        });
                    }
                }
            }
        }
        cr += 1;

        // Bubble text
        const bubble_bg: vaxis.Style = if (is_right)
            .{ .fg = .{ .rgb = t.bubbleFg() }, .bg = .{ .rgb = t.userBubbleBg() } }
        else
            .{ .fg = .{ .rgb = t.bubbleFg() }, .bg = .{ .rgb = t.assistantBubbleBg() } };

        var in_code_block = false;
        for (wrapped.items) |wline| {
            const sy = cr - visible_top;
            if (sy >= 0 and sy < rows) {
                const y: u16 = @intCast(sy);
                const inner: u16 = bw -| (bubble_padding * 2);
                const bwin = win.child(.{ .x_off = x_off, .y_off = y, .width = bw, .height = 1 });

                // Code blocks get a slightly different bubble background so
                // the reader can see where code starts and stops.
                const row_bg: vaxis.Style = if (in_code_block and !is_right)
                    .{ .fg = bubble_bg.fg, .bg = .{ .rgb = t.codeBubbleBg() } }
                else
                    bubble_bg;

                for (0..bw) |bx| {
                    bwin.writeCell(@intCast(bx), 0, .{ .char = .{ .grapheme = " " }, .style = row_bg });
                }
                const text_win = win.child(.{
                    .x_off = x_off + bubble_padding,
                    .y_off = y,
                    .width = inner,
                    .height = 1,
                });

                if (isFenceLine(wline)) {
                    in_code_block = !in_code_block;
                    // Blank row — the bg color shift is enough visual cue.
                } else if (in_code_block) {
                    var code_style = row_bg;
                    code_style.fg = .{ .rgb = .{ 220, 190, 130 } };
                    _ = text_win.print(&.{.{ .text = wline, .style = code_style }}, .{});
                } else {
                    var segs_buf: [64]vaxis.Cell.Segment = undefined;
                    const seg_count = parseInlineMarkdown(wline, bubble_bg, &segs_buf, t);
                    if (seg_count > 0) {
                        _ = text_win.print(segs_buf[0..seg_count], .{});
                    } else {
                        _ = text_win.print(&.{.{ .text = wline, .style = bubble_bg }}, .{});
                    }
                }
            }
            cr += 1;
            if (cr - visible_top >= rows) break;
        }

        // Cache info line below assistant bubbles
        if (line.cache_info) |info| {
            const meta_sy = cr - visible_top;
            if (meta_sy >= 0 and meta_sy < rows) {
                const my: u16 = @intCast(meta_sy);
                const meta_style: vaxis.Style = .{ .fg = .{ .rgb = t.dim() }, .italic = true };
                const meta_win = win.child(.{ .x_off = 1, .y_off = my, .width = @intCast(@min(info.len, win_width -| 2)), .height = 1 });
                _ = meta_win.print(&.{.{ .text = info, .style = meta_style }}, .{});
            }
            cr += 1;
        }

        vrow = msg_end;
    }

    return @intCast(clamped_offset);
}
