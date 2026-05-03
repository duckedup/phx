const std = @import("std");
const vaxis = @import("vaxis");
const core = @import("phoenix_core");

pub const Outcome = enum { completed, cancelled };

const VxEvent = union(enum) {
    key_press: vaxis.Key,
    winsize: vaxis.Winsize,
    focus_in,
    focus_out,
    paste_start,
    paste_end,
    paste: []const u8,
};

const Step = enum { provider_kind, host_url, model, api_key, confirm };

const KindEntry = struct {
    kind: core.ProviderKind,
    label: []const u8,
    /// Auth-table key written to TOML. Empty for local providers.
    auth_key: []const u8,
    /// Suggested cloud models (selectable list). Ignored when needs_host_url is true.
    models: []const []const u8,
    /// True for ollama / llama.cpp: ask for a host URL and a free-form model name.
    needs_host_url: bool,
    /// Default base URL pre-filled in the host_url step.
    default_host_url: []const u8,
    /// Default model pre-filled in the (free-form) model step for local providers.
    default_local_model: []const u8,
};

const kinds = [_]KindEntry{
    .{
        .kind = .claude,
        .label = "Anthropic Claude",
        .auth_key = "anthropic_api_key",
        .models = &.{
            "claude-opus-4-7",
            "claude-opus-4-6",
            "claude-sonnet-4-6",
            "claude-sonnet-4-5",
            "claude-haiku-4-5",
        },
        .needs_host_url = false,
        .default_host_url = "",
        .default_local_model = "",
    },
    .{
        .kind = .openai,
        .label = "OpenAI",
        .auth_key = "openai_api_key",
        .models = &.{
            "gpt-5.5",
            "gpt-5.4",
            "gpt-5",
            "gpt-5-mini",
            "gpt-4o",
            "o3",
            "o3-mini",
        },
        .needs_host_url = false,
        .default_host_url = "",
        .default_local_model = "",
    },
    .{
        .kind = .gemini,
        .label = "Google Gemini",
        .auth_key = "gemini_api_key",
        .models = &.{
            "gemini-2.5-pro",
            "gemini-2.5-flash",
            "gemini-2.0-pro",
            "gemini-2.0-flash",
            "gemini-1.5-pro",
        },
        .needs_host_url = false,
        .default_host_url = "",
        .default_local_model = "",
    },
    .{
        .kind = .ollama,
        .label = "Ollama (local)",
        .auth_key = "",
        .models = &.{},
        .needs_host_url = true,
        .default_host_url = "http://localhost:11434",
        .default_local_model = "llama3.3",
    },
    .{
        .kind = .llamacpp,
        .label = "llama.cpp (local)",
        .auth_key = "",
        .models = &.{},
        .needs_host_url = true,
        .default_host_url = "http://localhost:8080",
        .default_local_model = "local-model",
    },
};

pub fn run(init: std.process.Init, home: []const u8) !Outcome {
    const io = init.io;
    const allocator = init.gpa;

    var key_buf: std.ArrayList(u8) = .empty;
    defer key_buf.deinit(allocator);
    var host_buf: std.ArrayList(u8) = .empty;
    defer host_buf.deinit(allocator);
    var model_buf: std.ArrayList(u8) = .empty;
    defer model_buf.deinit(allocator);

    // Arena for transient strings built during a paint cycle (titles, headings,
    // hints). Cells in vx.screen.buf reference these byte ranges, so they must
    // outlive vx.render(). Reset at the start of each paint.
    var paint_arena = std.heap.ArenaAllocator.init(allocator);
    defer paint_arena.deinit();

    var step: Step = .provider_kind;
    var kind_index: usize = 0;
    var model_index: usize = 0;
    var error_msg: ?[]const u8 = null;

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
    try writer.flush();

    try paint(&vx, writer, &paint_arena, step, kind_index, model_index, host_buf.items, model_buf.items, key_buf.items, error_msg);

    while (true) {
        const event = try loop.nextEvent();

        switch (event) {
            .winsize => |ws| try vx.resize(allocator, writer, ws),
            .focus_in, .focus_out, .paste_start, .paste_end => {},
            .paste => |s| {
                switch (step) {
                    .api_key => try appendSanitized(&key_buf, allocator, s),
                    .host_url => try appendSanitized(&host_buf, allocator, s),
                    .model => if (kinds[kind_index].needs_host_url) try appendSanitized(&model_buf, allocator, s),
                    else => {},
                }
            },
            .key_press => |key| {
                if (key.matches('c', .{ .ctrl = true }) or key.codepoint == vaxis.Key.escape) {
                    return .cancelled;
                }
                error_msg = null;

                // Universal back navigation. We never use Left for cursor movement
                // inside text fields (no in-buffer cursor positioning yet), so it's
                // safe to repurpose for "go back".
                if (key.codepoint == vaxis.Key.left) {
                    step = stepBack(step, kinds[kind_index]);
                    continue;
                }

                switch (step) {
                    .provider_kind => {
                        if (key.codepoint == vaxis.Key.up) {
                            if (kind_index > 0) kind_index -= 1;
                        } else if (key.codepoint == vaxis.Key.down) {
                            if (kind_index + 1 < kinds.len) kind_index += 1;
                        } else if (key.codepoint == vaxis.Key.enter) {
                            const k = kinds[kind_index];
                            // Reset downstream state when picking a new provider.
                            host_buf.clearRetainingCapacity();
                            model_buf.clearRetainingCapacity();
                            key_buf.clearRetainingCapacity();
                            model_index = 0;
                            if (k.needs_host_url) {
                                try host_buf.appendSlice(allocator, k.default_host_url);
                                try model_buf.appendSlice(allocator, k.default_local_model);
                                step = .host_url;
                            } else {
                                step = .model;
                            }
                        }
                    },
                    .host_url => {
                        if (key.codepoint == vaxis.Key.enter) {
                            if (host_buf.items.len == 0) {
                                error_msg = "host URL cannot be empty";
                            } else {
                                step = .model;
                            }
                        } else if (key.codepoint == vaxis.Key.backspace) {
                            if (host_buf.items.len > 0) _ = host_buf.pop();
                        } else if (key.text) |t| {
                            try appendSanitized(&host_buf, allocator, t);
                        }
                    },
                    .model => {
                        if (kinds[kind_index].needs_host_url) {
                            // Free-form text entry for local providers.
                            if (key.codepoint == vaxis.Key.enter) {
                                if (model_buf.items.len == 0) {
                                    error_msg = "model name cannot be empty";
                                } else {
                                    step = .confirm;
                                }
                            } else if (key.codepoint == vaxis.Key.backspace) {
                                if (model_buf.items.len > 0) _ = model_buf.pop();
                            } else if (key.text) |t| {
                                try appendSanitized(&model_buf, allocator, t);
                            }
                        } else {
                            const models = kinds[kind_index].models;
                            if (key.codepoint == vaxis.Key.up) {
                                if (model_index > 0) model_index -= 1;
                            } else if (key.codepoint == vaxis.Key.down) {
                                if (model_index + 1 < models.len) model_index += 1;
                            } else if (key.codepoint == vaxis.Key.enter) {
                                step = .api_key;
                            }
                        }
                    },
                    .api_key => {
                        if (key.codepoint == vaxis.Key.enter) {
                            if (key_buf.items.len == 0) {
                                error_msg = "api key cannot be empty";
                            } else {
                                step = .confirm;
                            }
                        } else if (key.codepoint == vaxis.Key.backspace) {
                            if (key_buf.items.len > 0) _ = key_buf.pop();
                        } else if (key.text) |t| {
                            try appendSanitized(&key_buf, allocator, t);
                        }
                    },
                    .confirm => {
                        if (key.matches('y', .{}) or key.matches('Y', .{}) or key.codepoint == vaxis.Key.enter) {
                            try writeChoice(io, allocator, home, kind_index, model_index, host_buf.items, model_buf.items, key_buf.items);
                            return .completed;
                        } else if (key.matches('n', .{}) or key.matches('N', .{})) {
                            step = .provider_kind;
                        }
                    },
                }
            },
        }

        try paint(&vx, writer, &paint_arena, step, kind_index, model_index, host_buf.items, model_buf.items, key_buf.items, error_msg);
    }
}

fn stepBack(step: Step, k: KindEntry) Step {
    return switch (step) {
        .provider_kind => .provider_kind,
        .host_url => .provider_kind,
        .model => if (k.needs_host_url) .host_url else .provider_kind,
        .api_key => .model,
        .confirm => if (k.needs_host_url) .model else .api_key,
    };
}

fn paint(
    vx: *vaxis.Vaxis,
    writer: anytype,
    paint_arena: *std.heap.ArenaAllocator,
    step: Step,
    kind_index: usize,
    model_index: usize,
    host_value: []const u8,
    local_model: []const u8,
    key_value: []const u8,
    error_msg: ?[]const u8,
) !void {
    // Reset before drawing; the new frame's transient strings live in this
    // arena and must remain valid until vx.render() returns below.
    _ = paint_arena.reset(.retain_capacity);

    // Force a full repaint each frame. The diff renderer's "default cell"
    // fastpath was leaving stale glyphs from prior steps visible; we'd rather
    // pay the redraw cost than chase the diff bug.
    vx.queueRefresh();

    const win = vx.window();
    win.clear();
    vx.screen.cursor_vis = false;

    if (win.width < 40 or win.height < 14) {
        writeText(win, 0, 0, "Resize terminal to at least 40x14...", .{});
    } else {
        drawWizard(win, paint_arena.allocator(), step, kind_index, model_index, host_value, local_model, key_value, error_msg);
    }

    try vx.render(writer);
    try writer.flush();
}

fn isLocal(k: core.ProviderKind) bool {
    return k == .ollama or k == .llamacpp;
}

fn appendSanitized(buf: *std.ArrayList(u8), allocator: std.mem.Allocator, s: []const u8) !void {
    for (s) |c| {
        if (c == '\n' or c == '\r' or c == 0) continue;
        try buf.append(allocator, c);
    }
}

fn writeChoice(
    io: std.Io,
    allocator: std.mem.Allocator,
    home: []const u8,
    kind_index: usize,
    model_index: usize,
    host_value: []const u8,
    local_model: []const u8,
    key_value: []const u8,
) !void {
    const k = kinds[kind_index];
    const local = k.needs_host_url;
    const model: []const u8 = if (local) local_model else k.models[model_index];

    const auth: ?core.AuthEntry = if (local) null else .{ .inline_value = key_value };
    const base_url: ?[]const u8 = if (local and host_value.len > 0) host_value else null;

    const profiles = [_]core.ProviderProfile{
        .{
            .kind = k.kind,
            .model = model,
            .active = true,
            .auth = auth,
            .base_url = base_url,
        },
    };
    try core.config_writer.writeUserConfig(io, allocator, home, &profiles);
}

/// Write `text` starting at (col, row) of `win`, advancing one cell per ASCII
/// byte. Multi-byte UTF-8 sequences are batched into a single grapheme cell.
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

fn drawWizard(
    parent: vaxis.Window,
    arena: std.mem.Allocator,
    step: Step,
    kind_index: usize,
    model_index: usize,
    host_value: []const u8,
    local_model: []const u8,
    key_value: []const u8,
    error_msg: ?[]const u8,
) void {
    const modal_w: u16 = @min(parent.width, 64);
    const modal_h: u16 = @min(parent.height, 22);

    const modal = vaxis.widgets.alignment.center(parent, modal_w, modal_h);
    const inner = modal.child(.{
        .border = .{
            .where = .all,
            .glyphs = .single_rounded,
        },
    });

    const title_style: vaxis.Style = .{ .bold = true };
    const dim_style: vaxis.Style = .{ .fg = .{ .index = 8 } };
    const text_style: vaxis.Style = .{};
    const error_style: vaxis.Style = .{ .fg = .{ .index = 1 }, .bold = true };

    const total_steps = totalSteps(kinds[kind_index]);
    const step_n = stepNumber(step, kinds[kind_index]);
    const title = std.fmt.allocPrint(arena, "Phoenix - first-time setup ({d}/{d})", .{ step_n, total_steps }) catch "Phoenix - first-time setup";
    writeText(inner, 1, 0, title, title_style);

    switch (step) {
        .provider_kind => drawProviderKind(inner, kind_index, text_style, dim_style),
        .host_url => drawHostUrl(inner, arena, kind_index, host_value, text_style, dim_style),
        .model => drawModel(inner, arena, kind_index, model_index, local_model, text_style, dim_style),
        .api_key => drawApiKey(inner, arena, kind_index, key_value, text_style, dim_style),
        .confirm => drawConfirm(inner, arena, kind_index, model_index, host_value, local_model, key_value, text_style, dim_style),
    }

    const hint = stepHint(step);
    const hint_row: u16 = inner.height -| 2;
    writeText(inner, 1, hint_row, hint, dim_style);

    if (error_msg) |msg| {
        const err_row: u16 = inner.height -| 3;
        writeText(inner, 1, err_row, msg, error_style);
    }
}

fn totalSteps(k: KindEntry) usize {
    // provider_kind + (host_url?) + model + (api_key?) + confirm
    var n: usize = 3; // provider_kind, model, confirm
    if (k.needs_host_url) n += 1;
    if (!isLocal(k.kind)) n += 1;
    return n;
}

fn stepNumber(step: Step, k: KindEntry) usize {
    var n: usize = 0;
    n += 1; // provider_kind
    if (step == .provider_kind) return n;
    if (k.needs_host_url) {
        n += 1;
        if (step == .host_url) return n;
    }
    n += 1;
    if (step == .model) return n;
    if (!isLocal(k.kind)) {
        n += 1;
        if (step == .api_key) return n;
    }
    n += 1;
    return n; // confirm
}

fn stepHint(step: Step) []const u8 {
    return switch (step) {
        .provider_kind => "up/down select   Enter next   Esc cancel",
        .host_url => "type to edit   Enter next   Left back   Esc cancel",
        .model => "select or type   Enter next   Left back   Esc cancel",
        .api_key => "paste your API key   Enter next   Left back   Esc cancel",
        .confirm => "y confirm   n back   Left back   Esc cancel",
    };
}

fn drawProviderKind(
    inner: vaxis.Window,
    kind_index: usize,
    text_style: vaxis.Style,
    dim_style: vaxis.Style,
) void {
    writeText(inner, 1, 2, "Choose a provider:", text_style);
    const sel_style: vaxis.Style = .{ .reverse = true, .bold = true };
    for (kinds, 0..) |entry, i| {
        const row: u16 = @intCast(4 + i);
        const is_sel = (i == kind_index);
        const prefix: []const u8 = if (is_sel) " > " else "   ";
        const style = if (is_sel) sel_style else text_style;
        writeText(inner, 1, row, prefix, style);
        writeText(inner, 4, row, entry.label, style);
    }
    writeText(inner, 1, @intCast(4 + kinds.len + 1), "Local providers need a host URL but no API key.", dim_style);
}

fn drawHostUrl(
    inner: vaxis.Window,
    arena: std.mem.Allocator,
    kind_index: usize,
    host_value: []const u8,
    text_style: vaxis.Style,
    dim_style: vaxis.Style,
) void {
    const k = kinds[kind_index];
    const heading = std.fmt.allocPrint(arena, "Host URL for {s}:", .{k.label}) catch "Host URL:";
    writeText(inner, 1, 2, heading, text_style);

    writeText(inner, 1, 4, "URL: ", text_style);
    writeText(inner, 6, 4, host_value, .{ .bold = true });

    const hint = std.fmt.allocPrint(arena, "Default: {s}", .{k.default_host_url}) catch "";
    writeText(inner, 1, 6, hint, dim_style);
}

fn drawModel(
    inner: vaxis.Window,
    arena: std.mem.Allocator,
    kind_index: usize,
    model_index: usize,
    local_model: []const u8,
    text_style: vaxis.Style,
    dim_style: vaxis.Style,
) void {
    const k = kinds[kind_index];
    const heading = std.fmt.allocPrint(arena, "Model for {s}:", .{k.label}) catch "Model:";
    writeText(inner, 1, 2, heading, text_style);

    if (k.needs_host_url) {
        writeText(inner, 1, 4, "Name: ", text_style);
        writeText(inner, 7, 4, local_model, .{ .bold = true });
        writeText(inner, 1, 6, "Free-form text - type the model name as known to your local server.", dim_style);
    } else {
        const sel_style: vaxis.Style = .{ .reverse = true, .bold = true };
        for (k.models, 0..) |m, i| {
            const row: u16 = @intCast(4 + i);
            const is_sel = (i == model_index);
            const prefix: []const u8 = if (is_sel) " > " else "   ";
            const style = if (is_sel) sel_style else text_style;
            writeText(inner, 1, row, prefix, style);
            writeText(inner, 4, row, m, style);
        }
        writeText(inner, 1, @intCast(4 + k.models.len + 1), "Edit phoenix.json to use a model not in this list.", dim_style);
    }
}

fn drawApiKey(
    inner: vaxis.Window,
    arena: std.mem.Allocator,
    kind_index: usize,
    key_value: []const u8,
    text_style: vaxis.Style,
    dim_style: vaxis.Style,
) void {
    const k = kinds[kind_index];
    const heading = std.fmt.allocPrint(arena, "API key for {s}:", .{k.label}) catch "API key:";
    writeText(inner, 1, 2, heading, text_style);

    writeText(inner, 1, 4, "Saved inline in ~/.phoenix/phoenix.json (chmod 0600).", dim_style);

    writeText(inner, 1, 6, "Key: ", text_style);
    const visible = @min(key_value.len, inner.width -| 8);
    var col: u16 = 6;
    for (0..visible) |_| {
        inner.writeCell(col, 6, .{
            .char = .{ .grapheme = "*", .width = 1 },
            .style = .{ .bold = true },
        });
        col += 1;
    }

    writeText(inner, 1, 8, "Tip: prefer setting the env var instead;", dim_style);
    writeText(inner, 1, 9, "Phoenix auto-uses ANTHROPIC_API_KEY / OPENAI_API_KEY / GEMINI_API_KEY.", dim_style);
}

fn drawConfirm(
    inner: vaxis.Window,
    arena: std.mem.Allocator,
    kind_index: usize,
    model_index: usize,
    host_value: []const u8,
    local_model: []const u8,
    key_value: []const u8,
    text_style: vaxis.Style,
    dim_style: vaxis.Style,
) void {
    const k = kinds[kind_index];
    const local = k.needs_host_url;
    const model: []const u8 = if (local) local_model else k.models[model_index];

    writeText(inner, 1, 2, "Review your choices:", text_style);

    var row: u16 = 4;
    writeText(inner, 1, row, "Provider: ", dim_style);
    writeText(inner, 11, row, k.label, .{ .bold = true });
    row += 1;

    if (local) {
        writeText(inner, 1, row, "Host URL: ", dim_style);
        writeText(inner, 11, row, host_value, .{ .bold = true });
        row += 1;
    }

    writeText(inner, 1, row, "Model:    ", dim_style);
    writeText(inner, 11, row, model, .{ .bold = true });
    row += 1;

    writeText(inner, 1, row, "Auth:     ", dim_style);
    if (local) {
        writeText(inner, 11, row, "(none - local provider)", text_style);
    } else {
        const ksum = std.fmt.allocPrint(arena, "{d} chars (saved inline)", .{key_value.len}) catch "(saved inline)";
        writeText(inner, 11, row, ksum, text_style);
    }
    row += 1;

    writeText(inner, 1, row, "Path:     ", dim_style);
    writeText(inner, 11, row, "~/.phoenix/phoenix.json", text_style);
    row += 2;

    writeText(inner, 1, row, "Press y to write, n to go back.", text_style);
}
