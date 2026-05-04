/// Reusable provider/model wizard, shared between first-run onboarding and the
/// /models "Add new" flow. The wizard owns only its UI state (step, buffers,
/// indices) — callers drive the event loop and decide where to render. On
/// completion it emits a `Result` describing the chosen provider; the caller
/// is responsible for persisting it (writing the user config).
const std = @import("std");
const vaxis = @import("vaxis");
const core = @import("phoenix_core");

pub const Step = enum { provider_kind, host_url, model, context_window, api_key, confirm };

pub const KindEntry = struct {
    kind: core.ProviderKind,
    label: []const u8,
    /// Auth-table key. Empty for local providers.
    auth_key: []const u8,
    /// Suggested cloud models. Ignored when needs_host_url is true.
    models: []const []const u8,
    /// True for ollama / llama.cpp: ask for host URL + free-form model name +
    /// context window instead of API key.
    needs_host_url: bool,
    default_host_url: []const u8,
    default_local_model: []const u8,
    default_context_window: u32,
};

pub const kinds = [_]KindEntry{
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
        .default_context_window = 0,
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
        .default_context_window = 0,
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
        .default_context_window = 0,
    },
    .{
        .kind = .ollama,
        .label = "Ollama (local)",
        .auth_key = "",
        .models = &.{},
        .needs_host_url = true,
        .default_host_url = "http://localhost:11434",
        .default_local_model = "llama3.3",
        .default_context_window = 32_768,
    },
    .{
        .kind = .llamacpp,
        .label = "llama.cpp (local)",
        .auth_key = "",
        .models = &.{},
        .needs_host_url = true,
        .default_host_url = "http://localhost:8080",
        .default_local_model = "local-model",
        .default_context_window = 8_192,
    },
};

/// Returned from `handleKey`. `in_progress` keeps the wizard open; `cancelled`
/// and `completed` mean the wizard's state should be torn down by the caller.
/// `completed.profile` and friends are owned by the wizard's allocator and
/// remain valid until `deinit`.
pub const Outcome = union(enum) {
    in_progress,
    cancelled,
    completed: Result,
};

pub const Result = struct {
    kind: core.ProviderKind,
    model: []const u8,
    /// Inline secret string for cloud providers; empty for local providers.
    api_key: []const u8,
    /// Host URL for local providers; empty otherwise.
    base_url: []const u8,
    /// Set for local providers; null otherwise.
    context_window: ?u32,

    /// Build a `ProviderProfile` from this result, allocating fresh strings on
    /// `out`. The caller marks the profile active = true if the new model
    /// should become the default.
    pub fn toProfile(self: Result, out: std.mem.Allocator) !core.ProviderProfile {
        const local = self.base_url.len > 0;
        return .{
            .kind = self.kind,
            .model = try out.dupe(u8, self.model),
            .active = false,
            .auth = if (local) null else core.AuthEntry{ .inline_value = try out.dupe(u8, self.api_key) },
            .base_url = if (local) try out.dupe(u8, self.base_url) else null,
            .context_window = self.context_window,
        };
    }
};

pub const Wizard = struct {
    allocator: std.mem.Allocator,
    step: Step,
    kind_index: usize,
    model_index: usize,
    host_buf: std.ArrayList(u8),
    model_buf: std.ArrayList(u8),
    context_buf: std.ArrayList(u8),
    key_buf: std.ArrayList(u8),
    error_msg: ?[]const u8,

    pub fn init(allocator: std.mem.Allocator) Wizard {
        return .{
            .allocator = allocator,
            .step = .provider_kind,
            .kind_index = 0,
            .model_index = 0,
            .host_buf = .empty,
            .model_buf = .empty,
            .context_buf = .empty,
            .key_buf = .empty,
            .error_msg = null,
        };
    }

    pub fn deinit(self: *Wizard) void {
        self.host_buf.deinit(self.allocator);
        self.model_buf.deinit(self.allocator);
        self.context_buf.deinit(self.allocator);
        self.key_buf.deinit(self.allocator);
    }

    /// Append paste content into whichever step is currently accepting text.
    /// Steps that don't take text input ignore the paste.
    pub fn handlePaste(self: *Wizard, text: []const u8) !void {
        switch (self.step) {
            .api_key => try appendSanitized(&self.key_buf, self.allocator, text),
            .host_url => try appendSanitized(&self.host_buf, self.allocator, text),
            .model => if (kinds[self.kind_index].needs_host_url) try appendSanitized(&self.model_buf, self.allocator, text),
            .context_window => try appendDigits(&self.context_buf, self.allocator, text),
            else => {},
        }
    }

    /// Process one key press. Returns `.in_progress` while the wizard is still
    /// running. The caller treats `.cancelled` / `.completed` as terminal and
    /// must call `deinit` (the Result's slices live on the wizard allocator
    /// only until then — `Result.toProfile` is the way to lift them out).
    pub fn handleKey(self: *Wizard, key: vaxis.Key) !Outcome {
        if (key.matches('c', .{ .ctrl = true }) or key.codepoint == vaxis.Key.escape) {
            return .cancelled;
        }
        self.error_msg = null;

        // Universal back navigation: Left always steps backward. Text fields
        // here have no in-buffer cursor, so this never collides with editing.
        if (key.codepoint == vaxis.Key.left) {
            self.step = stepBack(self.step, kinds[self.kind_index]);
            return .in_progress;
        }

        switch (self.step) {
            .provider_kind => {
                if (key.codepoint == vaxis.Key.up) {
                    if (self.kind_index > 0) self.kind_index -= 1;
                } else if (key.codepoint == vaxis.Key.down) {
                    if (self.kind_index + 1 < kinds.len) self.kind_index += 1;
                } else if (key.codepoint == vaxis.Key.enter) {
                    const k = kinds[self.kind_index];
                    self.host_buf.clearRetainingCapacity();
                    self.model_buf.clearRetainingCapacity();
                    self.context_buf.clearRetainingCapacity();
                    self.key_buf.clearRetainingCapacity();
                    self.model_index = 0;
                    if (k.needs_host_url) {
                        try self.host_buf.appendSlice(self.allocator, k.default_host_url);
                        try self.model_buf.appendSlice(self.allocator, k.default_local_model);
                        try self.context_buf.print(self.allocator, "{d}", .{k.default_context_window});
                        self.step = .host_url;
                    } else {
                        self.step = .model;
                    }
                }
            },
            .host_url => {
                if (key.codepoint == vaxis.Key.enter) {
                    if (self.host_buf.items.len == 0) {
                        self.error_msg = "host URL cannot be empty";
                    } else {
                        self.step = .model;
                    }
                } else if (key.codepoint == vaxis.Key.backspace) {
                    if (self.host_buf.items.len > 0) _ = self.host_buf.pop();
                } else if (key.text) |t| {
                    try appendSanitized(&self.host_buf, self.allocator, t);
                }
            },
            .model => {
                if (kinds[self.kind_index].needs_host_url) {
                    if (key.codepoint == vaxis.Key.enter) {
                        if (self.model_buf.items.len == 0) {
                            self.error_msg = "model name cannot be empty";
                        } else {
                            self.step = .context_window;
                        }
                    } else if (key.codepoint == vaxis.Key.backspace) {
                        if (self.model_buf.items.len > 0) _ = self.model_buf.pop();
                    } else if (key.text) |t| {
                        try appendSanitized(&self.model_buf, self.allocator, t);
                    }
                } else {
                    const models = kinds[self.kind_index].models;
                    if (key.codepoint == vaxis.Key.up) {
                        if (self.model_index > 0) self.model_index -= 1;
                    } else if (key.codepoint == vaxis.Key.down) {
                        if (self.model_index + 1 < models.len) self.model_index += 1;
                    } else if (key.codepoint == vaxis.Key.enter) {
                        self.step = .api_key;
                    }
                }
            },
            .context_window => {
                if (key.codepoint == vaxis.Key.enter) {
                    if (parseContextWindow(self.context_buf.items) == null) {
                        self.error_msg = "context window must be a positive integer";
                    } else {
                        self.step = .confirm;
                    }
                } else if (key.codepoint == vaxis.Key.backspace) {
                    if (self.context_buf.items.len > 0) _ = self.context_buf.pop();
                } else if (key.text) |t| {
                    try appendDigits(&self.context_buf, self.allocator, t);
                }
            },
            .api_key => {
                if (key.codepoint == vaxis.Key.enter) {
                    if (self.key_buf.items.len == 0) {
                        self.error_msg = "api key cannot be empty";
                    } else {
                        self.step = .confirm;
                    }
                } else if (key.codepoint == vaxis.Key.backspace) {
                    if (self.key_buf.items.len > 0) _ = self.key_buf.pop();
                } else if (key.text) |t| {
                    try appendSanitized(&self.key_buf, self.allocator, t);
                }
            },
            .confirm => {
                if (key.matches('y', .{}) or key.matches('Y', .{}) or key.codepoint == vaxis.Key.enter) {
                    return .{ .completed = self.buildResult() };
                } else if (key.matches('n', .{}) or key.matches('N', .{})) {
                    self.step = .provider_kind;
                }
            },
        }

        return .in_progress;
    }

    fn buildResult(self: *const Wizard) Result {
        const k = kinds[self.kind_index];
        const local = k.needs_host_url;
        return .{
            .kind = k.kind,
            .model = if (local) self.model_buf.items else k.models[self.model_index],
            .api_key = if (local) "" else self.key_buf.items,
            .base_url = if (local) self.host_buf.items else "",
            .context_window = if (local) parseContextWindow(self.context_buf.items) else null,
        };
    }

    /// Render the wizard into `parent`. `arena` should be reset on every
    /// frame: vaxis stores grapheme slices by reference, so the strings
    /// allocated here must outlive vx.render(). Caller picks the window
    /// dimensions; the wizard will center a 64x22 modal inside it.
    pub fn paint(self: *const Wizard, parent: vaxis.Window, arena: std.mem.Allocator) void {
        if (parent.width < 40 or parent.height < 14) {
            writeText(parent, 0, 0, "Resize terminal to at least 40x14...", .{});
            return;
        }
        drawWizard(
            parent,
            arena,
            self.step,
            self.kind_index,
            self.model_index,
            self.host_buf.items,
            self.model_buf.items,
            self.context_buf.items,
            self.key_buf.items,
            self.error_msg,
        );
    }
};

fn stepBack(step: Step, k: KindEntry) Step {
    return switch (step) {
        .provider_kind => .provider_kind,
        .host_url => .provider_kind,
        .model => if (k.needs_host_url) .host_url else .provider_kind,
        .context_window => .model,
        .api_key => .model,
        .confirm => if (k.needs_host_url) .context_window else .api_key,
    };
}

/// Drop everything that isn't a base-10 digit before appending. Used by both
/// the keystroke and paste paths in the context_window step.
fn appendDigits(buf: *std.ArrayList(u8), allocator: std.mem.Allocator, s: []const u8) !void {
    for (s) |c| {
        if (c >= '0' and c <= '9') try buf.append(allocator, c);
    }
}

/// Parse the context-window text field. Returns null on empty / overflow.
fn parseContextWindow(s: []const u8) ?u32 {
    if (s.len == 0) return null;
    var n: u64 = 0;
    for (s) |c| {
        if (c < '0' or c > '9') return null;
        n = n * 10 + (c - '0');
        if (n > std.math.maxInt(u32)) return null;
    }
    if (n == 0) return null;
    return @intCast(n);
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
    context_value: []const u8,
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
    const title = std.fmt.allocPrint(arena, "Add a model ({d}/{d})", .{ step_n, total_steps }) catch "Add a model";
    writeText(inner, 1, 0, title, title_style);

    switch (step) {
        .provider_kind => drawProviderKind(inner, kind_index, text_style, dim_style),
        .host_url => drawHostUrl(inner, arena, kind_index, host_value, text_style, dim_style),
        .model => drawModel(inner, arena, kind_index, model_index, local_model, text_style, dim_style),
        .context_window => drawContextWindow(inner, arena, kind_index, context_value, text_style, dim_style),
        .api_key => drawApiKey(inner, arena, kind_index, key_value, text_style, dim_style),
        .confirm => drawConfirm(inner, arena, kind_index, model_index, host_value, local_model, context_value, key_value, text_style, dim_style),
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
    var n: usize = 3; // provider_kind, model, confirm
    if (k.needs_host_url) n += 2; // host_url + context_window
    if (!isLocal(k.kind)) n += 1; // api_key
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
    if (k.needs_host_url) {
        n += 1;
        if (step == .context_window) return n;
    }
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
        .context_window => "digits only   Enter next   Left back   Esc cancel",
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

fn drawContextWindow(
    inner: vaxis.Window,
    arena: std.mem.Allocator,
    kind_index: usize,
    context_value: []const u8,
    text_style: vaxis.Style,
    dim_style: vaxis.Style,
) void {
    const k = kinds[kind_index];
    const heading = std.fmt.allocPrint(arena, "Context window for {s}:", .{k.label}) catch "Context window:";
    writeText(inner, 1, 2, heading, text_style);

    writeText(inner, 1, 4, "Tokens: ", text_style);
    writeText(inner, 9, 4, context_value, .{ .bold = true });

    writeText(inner, 1, 6, "Local models don't expose this; Phoenix needs the number to know", dim_style);
    writeText(inner, 1, 7, "when to auto-compact. Match the limit you started your server with.", dim_style);

    const def = std.fmt.allocPrint(arena, "Default: {d}", .{k.default_context_window}) catch "";
    writeText(inner, 1, 9, def, dim_style);
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
    context_value: []const u8,
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

    if (local) {
        writeText(inner, 1, row, "Context:  ", dim_style);
        const ctx_summary = std.fmt.allocPrint(arena, "{s} tokens", .{context_value}) catch context_value;
        writeText(inner, 11, row, ctx_summary, .{ .bold = true });
        row += 1;
    }

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

// ---- Tests ----

test "parseContextWindow" {
    try std.testing.expectEqual(@as(?u32, 1024), parseContextWindow("1024"));
    try std.testing.expectEqual(@as(?u32, null), parseContextWindow(""));
    try std.testing.expectEqual(@as(?u32, null), parseContextWindow("0"));
    try std.testing.expectEqual(@as(?u32, null), parseContextWindow("12a"));
}

test "wizard initial step is provider_kind" {
    var w = Wizard.init(std.testing.allocator);
    defer w.deinit();
    try std.testing.expectEqual(Step.provider_kind, w.step);
    try std.testing.expectEqual(@as(usize, 0), w.kind_index);
}

test "stepBack from confirm honors host_url path" {
    const k_local = kinds[3]; // ollama
    const k_cloud = kinds[0]; // claude
    try std.testing.expectEqual(Step.context_window, stepBack(.confirm, k_local));
    try std.testing.expectEqual(Step.api_key, stepBack(.confirm, k_cloud));
    try std.testing.expectEqual(Step.host_url, stepBack(.model, k_local));
    try std.testing.expectEqual(Step.provider_kind, stepBack(.model, k_cloud));
}
