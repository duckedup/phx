const std = @import("std");
const core = @import("phoenix_core");
const dispatcher = @import("dispatcher.zig");

/// `/model` with no args → return a picker of every model from every configured
/// provider, with the default-provider's current model marked active.
/// If exactly one model is configured, return an info `.message` instead of a
/// picker (per user preference).
pub fn handle(
    ctx: dispatcher.DispatchCtx,
    args: []const u8,
    out: std.mem.Allocator,
) !dispatcher.Result {
    _ = args; // /model takes no args in this iteration
    const default_p = ctx.config.provider("default");

    var choices: std.ArrayList(dispatcher.ModelChoice) = .empty;
    var it = ctx.config.providers.iterator();
    while (it.next()) |entry| {
        const name = entry.key_ptr.*;
        const profile = entry.value_ptr.*;
        const is_active = if (default_p) |d|
            (std.mem.eql(u8, name, "default") and std.mem.eql(u8, d.model, profile.model))
        else
            false;
        try choices.append(out, .{
            .provider_name = try out.dupe(u8, name),
            .kind = profile.kind,
            .model = try out.dupe(u8, profile.model),
            .is_active = is_active,
        });
    }

    if (choices.items.len <= 1) {
        const model_str: []const u8 = if (default_p) |d| d.model else "(none)";
        const msg = try std.fmt.allocPrint(
            out,
            "Only one model configured: {s}. Add more providers in phoenix.toml to switch.",
            .{model_str},
        );
        return .{ .message = msg };
    }

    const slice = try choices.toOwnedSlice(out);
    return .{
        .model_picker = .{
            .title = try out.dupe(u8, "Select model  (\u{2191}/\u{2193} to navigate, Enter to confirm, Esc to cancel)"),
            .choices = slice,
        },
    };
}

/// Persist the chosen model: rewrite the user's phoenix.toml with the new
/// model on `default`, preserving its kind/auth/base_url. Then mutate the
/// in-memory config so the status bar updates immediately.
pub fn apply(
    ctx: dispatcher.DispatchCtx,
    choice: dispatcher.ModelChoice,
    out: std.mem.Allocator,
) ![]const u8 {
    const home = ctx.home orelse return error.HomeUnavailable;

    // Look up the source profile (where this model came from). We carry over
    // its kind/auth/base_url to `default` so a one-line model swap doesn't
    // accidentally drop credentials.
    const src = ctx.config.provider(choice.provider_name) orelse return error.UnknownProvider;

    try core.config_writer.writeUserConfig(
        ctx.io,
        ctx.gpa,
        home,
        .{
            .name = "default",
            .kind = src.kind,
            .model = choice.model,
            .auth_key = src.auth,
            .base_url = src.base_url,
        },
        null, // do not touch [auth]; the existing entry remains valid by key
    );

    // Mutate in-memory default provider so the status bar reflects the change
    // without requiring a full Config.load (which would also require us to
    // free the old arena, which other code may be holding pointers into).
    if (ctx.config.providers.getPtr("default")) |dp| {
        // The model string must be owned by config.arena to outlive this call.
        const arena_alloc = ctx.config.arena.allocator();
        const owned = try arena_alloc.dupe(u8, choice.model);
        dp.model = owned;
        dp.kind = src.kind;
        if (src.auth) |a| dp.auth = try arena_alloc.dupe(u8, a) else {
            dp.auth = null;
        }
        if (src.base_url) |b| dp.base_url = try arena_alloc.dupe(u8, b) else {
            dp.base_url = null;
        }
    }

    return std.fmt.allocPrint(
        out,
        "Active model set to {s} ({s}). Saved to ~/.phoenix/phoenix.toml.",
        .{ choice.model, @tagName(src.kind) },
    );
}

// ---- Tests ----

fn tmpHome(allocator: std.mem.Allocator, tmp: *std.testing.TmpDir) ![]u8 {
    var buf: [std.Io.Dir.max_path_bytes]u8 = undefined;
    const ptr = std.c.getcwd(&buf, buf.len) orelse return error.CwdError;
    const cwd_path = std.mem.sliceTo(ptr, 0);
    return try std.fs.path.join(allocator, &.{ cwd_path, ".zig-cache", "tmp", &tmp.sub_path, "home" });
}

test "/model single provider returns message" {
    const allocator = std.testing.allocator;
    var cfg = try core.Config.load(allocator, std.testing.io, .{ .home = null });
    defer cfg.deinit();

    var out_arena = std.heap.ArenaAllocator.init(allocator);
    defer out_arena.deinit();

    const result = try handle(
        .{ .io = std.testing.io, .gpa = allocator, .home = null, .config = &cfg },
        "",
        out_arena.allocator(),
    );
    switch (result) {
        .message => |msg| try std.testing.expect(std.mem.indexOf(u8, msg, "claude-opus-4-7") != null),
        else => return error.TestUnexpectedResult,
    }
}

test "/model two providers returns picker" {
    const allocator = std.testing.allocator;
    var cfg = try core.Config.load(allocator, std.testing.io, .{ .home = null });
    defer cfg.deinit();

    const a = cfg.arena.allocator();
    try cfg.providers.put(a, try a.dupe(u8, "alt"), .{
        .kind = .openai,
        .model = try a.dupe(u8, "gpt-4o"),
    });

    var out_arena = std.heap.ArenaAllocator.init(allocator);
    defer out_arena.deinit();

    const result = try handle(
        .{ .io = std.testing.io, .gpa = allocator, .home = null, .config = &cfg },
        "",
        out_arena.allocator(),
    );
    switch (result) {
        .model_picker => |picker| {
            try std.testing.expect(picker.choices.len >= 2);
            var found_active = false;
            for (picker.choices) |c| {
                if (c.is_active) found_active = true;
            }
            try std.testing.expect(found_active);
        },
        else => return error.TestUnexpectedResult,
    }
}

test "applyModelChoice rewrites toml and mutates in-memory" {
    const allocator = std.testing.allocator;
    var tmp = std.testing.tmpDir(.{});
    defer tmp.cleanup();
    const home = try tmpHome(allocator, &tmp);
    defer allocator.free(home);
    try std.Io.Dir.cwd().createDirPath(std.testing.io, home);

    var cfg = try core.Config.load(allocator, std.testing.io, .{ .home = home });
    defer cfg.deinit();
    const arena_alloc = cfg.arena.allocator();
    try cfg.providers.put(arena_alloc, try arena_alloc.dupe(u8, "alt"), .{
        .kind = .openai,
        .model = try arena_alloc.dupe(u8, "gpt-4o"),
    });

    var out_arena = std.heap.ArenaAllocator.init(allocator);
    defer out_arena.deinit();
    const msg = try apply(
        .{ .io = std.testing.io, .gpa = allocator, .home = home, .config = &cfg },
        .{
            .provider_name = "alt",
            .kind = .openai,
            .model = "gpt-4o",
            .is_active = false,
        },
        out_arena.allocator(),
    );
    try std.testing.expect(std.mem.indexOf(u8, msg, "gpt-4o") != null);

    // In-memory default provider was updated.
    const dp = cfg.provider("default").?;
    try std.testing.expectEqualStrings("gpt-4o", dp.model);
    try std.testing.expectEqual(core.ProviderKind.openai, dp.kind);

    // ~/.phoenix/phoenix.toml on disk reflects the change too.
    const cfg_path = try std.fs.path.join(allocator, &.{ home, ".phoenix", "phoenix.toml" });
    defer allocator.free(cfg_path);
    const contents = try std.Io.Dir.cwd().readFileAlloc(std.testing.io, cfg_path, allocator, .limited(64 * 1024));
    defer allocator.free(contents);
    try std.testing.expect(std.mem.indexOf(u8, contents, "gpt-4o") != null);
}
