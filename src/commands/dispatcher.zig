const std = @import("std");
const core = @import("phoenix_core");

const model_cmd = @import("model.zig");
const skill_cmd = @import("skill.zig");

/// One model entry shown in the picker. Owned by the Result arena.
pub const ModelChoice = struct {
    /// Index into `Config.providers`. The TUI passes this back via
    /// `applyModelChoice` so the server flips `active = true` on the right row.
    provider_index: u32,
    /// Provider kind, for display (e.g. "claude").
    kind: core.ProviderKind,
    /// Model id (e.g. "claude-opus-4-7").
    model: []const u8,
    /// True if this entry is currently the active provider.
    is_active: bool,
};

/// Picker payload. The TUI renders this as an inline list below the input area
/// and calls `dispatcher.applyModelChoice` when the user presses Enter.
pub const ModelPicker = struct {
    title: []const u8, // e.g. "Select model (Enter to confirm, Esc to cancel)"
    choices: []const ModelChoice,
};

/// A "context fragment" produced by activating a skill. The TUI displays it as a
/// system-role chat line today; once the LLM is wired in, the same fragment is
/// passed as additional context preceding the user message.
pub const ContextFragment = struct {
    /// Origin label (e.g. "skill:research"). Used for the system-line header.
    label: []const u8,
    /// The body to inject (skill.md contents).
    body: []const u8,
    /// The user message after the leading "/cmd" token has been stripped.
    /// May be empty (the user activated the skill with no follow-up text).
    user_message: []const u8,
};

pub const Result = union(enum) {
    /// Command produced a simple text result. Render as a system-role chat line.
    /// Owned by `arena`.
    message: []const u8,

    /// Command failed. Render as a system-role chat line with error styling.
    /// Owned by `arena`.
    err: []const u8,

    /// Show the inline model picker. Owned by `arena`.
    model_picker: ModelPicker,

    /// Inject the fragment as system context, then submit `user_message` as the
    /// user turn. Owned by `arena`.
    inject_context: ContextFragment,

    /// The input was not a slash command. The TUI should treat the original
    /// input as a normal user message.
    not_a_command,
};

/// Returned to the TUI on every dispatch. The TUI must call `deinit` after it is
/// done reading any borrowed slices.
pub const Outcome = struct {
    arena: std.heap.ArenaAllocator,
    result: Result,

    pub fn deinit(self: *Outcome) void {
        self.arena.deinit();
    }
};

pub const Registry = struct {
    /// Built-in handler descriptors. Skills are looked up against `config.skills`
    /// at dispatch time, so we don't store them here.
    builtins: []const Builtin,

    pub const Builtin = struct {
        name: []const u8, // without the leading slash
        summary: []const u8,
        handler: *const fn (
            ctx: DispatchCtx,
            args: []const u8,
            out_arena: std.mem.Allocator,
        ) anyerror!Result,
    };

    pub fn init() Registry {
        return .{ .builtins = &builtin_table };
    }

    fn lookup(self: Registry, name: []const u8) ?Builtin {
        for (self.builtins) |b| {
            if (std.mem.eql(u8, b.name, name)) return b;
        }
        return null;
    }
};

const builtin_table = [_]Registry.Builtin{
    .{ .name = "model", .summary = "Select the active model", .handler = model_cmd.handle },
};

pub const DispatchCtx = struct {
    io: std.Io,
    gpa: std.mem.Allocator,
    home: ?[]const u8, // null if HOME unresolved; commands that need it must error
    config: *core.Config,
};

/// Returns true iff `input` (after no leading whitespace stripping) starts with
/// '/' followed by an alphanumeric byte. A bare "/" is not a command. A line
/// that begins with "//" is not a command (lets users escape).
pub fn isCommand(input: []const u8) bool {
    if (input.len < 2) return false;
    if (input[0] != '/') return false;
    const c = input[1];
    if (c == '/') return false;
    return std.ascii.isAlphanumeric(c) or c == '_' or c == '-';
}

/// Parse "/name rest...". `name` is the bytes after '/' up to the first space,
/// tab, or newline. `args` is everything after the separator (no leading space),
/// or "" if there were no further bytes.
pub const Parsed = struct {
    name: []const u8,
    args: []const u8,
};
pub fn parse(input: []const u8) ?Parsed {
    if (!isCommand(input)) return null;
    var end: usize = 1;
    while (end < input.len) : (end += 1) {
        const ch = input[end];
        if (ch == ' ' or ch == '\t' or ch == '\n') break;
    }
    const name = input[1..end];
    const args = if (end >= input.len) "" else blk: {
        var s = end;
        while (s < input.len and (input[s] == ' ' or input[s] == '\t')) s += 1;
        break :blk input[s..];
    };
    return .{ .name = name, .args = args };
}

/// Dispatch a single line. If the line is not a slash command, returns a result
/// of `.not_a_command` — caller treats the input as a normal user message. The
/// returned `Outcome` owns its strings via an arena; caller must deinit when
/// done with the borrowed slices.
pub fn dispatch(ctx: DispatchCtx, input: []const u8) !Outcome {
    var arena = std.heap.ArenaAllocator.init(ctx.gpa);
    errdefer arena.deinit();

    if (parse(input)) |p| {
        const reg = Registry.init();
        if (reg.lookup(p.name)) |b| {
            const r = b.handler(ctx, p.args, arena.allocator()) catch |err| blk: {
                const msg = std.fmt.allocPrint(arena.allocator(), "/{s}: {s}", .{ p.name, @errorName(err) }) catch "command failed";
                break :blk Result{ .err = msg };
            };
            return .{ .arena = arena, .result = r };
        }
        // Not a built-in — try skills.
        if (skill_cmd.findSkill(ctx.config, p.name)) |skill| {
            const r = skill_cmd.handle(ctx, skill, p.args, arena.allocator()) catch |err| blk: {
                const msg = std.fmt.allocPrint(arena.allocator(), "/{s}: {s}", .{ p.name, @errorName(err) }) catch "skill failed";
                break :blk Result{ .err = msg };
            };
            return .{ .arena = arena, .result = r };
        }
        const msg = try std.fmt.allocPrint(arena.allocator(), "Unknown command: /{s}", .{p.name});
        return .{ .arena = arena, .result = .{ .err = msg } };
    }
    return .{ .arena = arena, .result = .not_a_command };
}

/// Apply the user's selected ModelChoice: rewrite ~/.phoenix/phoenix.json with
/// the new model on `default`, then mutate the in-memory `config.providers`
/// entry so the status bar updates without a reload. Returns a system message
/// describing what happened (owned by `out_arena`).
pub fn applyModelChoice(
    ctx: DispatchCtx,
    choice: ModelChoice,
    out_arena: std.mem.Allocator,
) ![]const u8 {
    return model_cmd.apply(ctx, choice, out_arena);
}

// ---- Tests ----

test "parse: /model" {
    const p = parse("/model").?;
    try std.testing.expectEqualStrings("model", p.name);
    try std.testing.expectEqualStrings("", p.args);
}

test "parse: /model gpt-4o" {
    const p = parse("/model gpt-4o").?;
    try std.testing.expectEqualStrings("model", p.name);
    try std.testing.expectEqualStrings("gpt-4o", p.args);
}

test "parse: plain text returns null" {
    try std.testing.expect(parse("hello") == null);
}

test "parse: bare slash returns null" {
    try std.testing.expect(parse("/") == null);
}

test "parse: double slash returns null" {
    try std.testing.expect(parse("//escaped") == null);
}

test "dispatch: unknown command returns err" {
    const allocator = std.testing.allocator;
    var cfg = try core.Config.load(allocator, std.testing.io, .{ .home = null });
    defer cfg.deinit();

    var outcome = try dispatch(
        .{ .io = std.testing.io, .gpa = allocator, .home = null, .config = &cfg },
        "/unknown",
    );
    defer outcome.deinit();

    switch (outcome.result) {
        .err => |msg| try std.testing.expect(std.mem.indexOf(u8, msg, "Unknown command: /unknown") != null),
        else => return error.TestUnexpectedResult,
    }
}

test "dispatch: plain text returns not_a_command" {
    const allocator = std.testing.allocator;
    var cfg = try core.Config.load(allocator, std.testing.io, .{ .home = null });
    defer cfg.deinit();

    var outcome = try dispatch(
        .{ .io = std.testing.io, .gpa = allocator, .home = null, .config = &cfg },
        "plain text",
    );
    defer outcome.deinit();

    try std.testing.expect(outcome.result == .not_a_command);
}
