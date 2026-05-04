const std = @import("std");
const core = @import("phoenix_core");
const dispatcher = @import("dispatcher.zig");

/// `/model` with no args -> return a picker of every configured provider with
/// the currently-active row marked. If <=1 providers are configured, return an
/// info `.message` instead of a picker.
pub fn handle(
    ctx: dispatcher.DispatchCtx,
    args: []const u8,
    out: std.mem.Allocator,
) !dispatcher.Result {
    _ = args;
    const providers = ctx.config.providers;

    var choices: std.ArrayList(dispatcher.ModelChoice) = .empty;
    for (providers, 0..) |p, i| {
        try choices.append(out, .{
            .provider_index = @intCast(i),
            .kind = p.kind,
            .model = try out.dupe(u8, p.model),
            .is_active = p.active,
        });
    }

    if (choices.items.len <= 1) {
        const ap = ctx.config.activeProvider();
        const model_str: []const u8 = if (ap) |a| a.model else "(none)";
        const msg = try std.fmt.allocPrint(
            out,
            "Only one model configured: {s}. Add more entries to phoenix.json to switch.",
            .{model_str},
        );
        return .{ .message = msg };
    }

    return .{
        .model_picker = .{
            .title = try out.dupe(u8, "Select model  (\u{2191}/\u{2193} to navigate, Enter to confirm, Esc to cancel)"),
            .choices = try choices.toOwnedSlice(out),
        },
    };
}

/// `/models` -> the full-screen models page. Always succeeds, even when only
/// one provider is configured (the page itself surfaces the "Add new" affordance,
/// which is precisely what the user reaches for in the single-provider case).
pub fn handleModelsPage(
    ctx: dispatcher.DispatchCtx,
    args: []const u8,
    out: std.mem.Allocator,
) !dispatcher.Result {
    _ = args;
    const entries = try listEntries(ctx, out);
    return .{
        .models_page = .{
            .title = try out.dupe(u8, "Models"),
            .entries = entries,
        },
    };
}

/// Snapshot the current providers list as ModelEntry rows. Returned slice is
/// owned by `out`.
pub fn listEntries(
    ctx: dispatcher.DispatchCtx,
    out: std.mem.Allocator,
) ![]dispatcher.ModelEntry {
    var entries: std.ArrayList(dispatcher.ModelEntry) = .empty;
    for (ctx.config.providers, 0..) |p, i| {
        try entries.append(out, .{
            .provider_index = @intCast(i),
            .kind = p.kind,
            .model = try out.dupe(u8, p.model),
            .is_active = p.active,
            .base_url = if (p.base_url) |b| try out.dupe(u8, b) else "",
            .context_window = p.context_window orelse 0,
        });
    }
    return entries.toOwnedSlice(out);
}

/// Append `profile` to `config.providers` and rewrite ~/.phoenix/phoenix.json.
/// `profile.active` is preserved as-is — onboarding writes the first provider
/// active; new providers added via /models start inactive.
pub fn addProvider(
    ctx: dispatcher.DispatchCtx,
    profile: core.ProviderProfile,
    out: std.mem.Allocator,
) ![]const u8 {
    const home = ctx.home orelse return error.HomeUnavailable;

    const a = ctx.config.arena.allocator();
    const old = ctx.config.providers;
    const combined = try a.alloc(core.ProviderProfile, old.len + 1);
    @memcpy(combined[0..old.len], old);
    combined[old.len] = .{
        .kind = profile.kind,
        .model = try a.dupe(u8, profile.model),
        .active = profile.active,
        .auth = if (profile.auth) |auth| switch (auth) {
            .inline_value => |v| core.AuthEntry{ .inline_value = try a.dupe(u8, v) },
            .env_var => |v| core.AuthEntry{ .env_var = try a.dupe(u8, v) },
        } else null,
        .base_url = if (profile.base_url) |b| try a.dupe(u8, b) else null,
        .endpoint = if (profile.endpoint) |e| try a.dupe(u8, e) else null,
        .context_window = profile.context_window,
        .api = if (profile.api) |x| try a.dupe(u8, x) else null,
        .project = if (profile.project) |x| try a.dupe(u8, x) else null,
        .location = if (profile.location) |x| try a.dupe(u8, x) else null,
        .credentials_path = if (profile.credentials_path) |x| try a.dupe(u8, x) else null,
    };
    ctx.config.providers = combined;

    try core.config_writer.writeUserConfig(ctx.io, ctx.gpa, home, ctx.config.providers);

    return std.fmt.allocPrint(
        out,
        "Added {s} ({s}). Saved to ~/.phoenix/phoenix.json.",
        .{ profile.model, @tagName(profile.kind) },
    );
}

/// Persist the chosen provider as active. Mutates the in-memory config to flip
/// the `active` flag on the chosen row (clearing it from all others), then
/// rewrites ~/.phoenix/phoenix.json to match.
pub fn apply(
    ctx: dispatcher.DispatchCtx,
    choice: dispatcher.ModelChoice,
    out: std.mem.Allocator,
) ![]const u8 {
    const home = ctx.home orelse return error.HomeUnavailable;

    if (choice.provider_index >= ctx.config.providers.len) return error.UnknownProvider;

    // Clear all existing active flags, then set the chosen one.
    for (ctx.config.providers) |*p| p.active = false;
    ctx.config.providers[choice.provider_index].active = true;

    try core.config_writer.writeUserConfig(ctx.io, ctx.gpa, home, ctx.config.providers);

    const chosen = &ctx.config.providers[choice.provider_index];
    return std.fmt.allocPrint(
        out,
        "Active model set to {s} ({s}). Saved to ~/.phoenix/phoenix.json.",
        .{ chosen.model, @tagName(chosen.kind) },
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

test "/model two providers returns picker with active flag" {
    const allocator = std.testing.allocator;
    var cfg = try core.Config.load(allocator, std.testing.io, .{ .home = null });
    defer cfg.deinit();

    // Replace the default single-entry list with two entries.
    const a = cfg.arena.allocator();
    const list = try a.alloc(core.ProviderProfile, 2);
    list[0] = .{ .kind = .claude, .model = try a.dupe(u8, "claude-opus-4-7"), .active = true };
    list[1] = .{ .kind = .openai, .model = try a.dupe(u8, "gpt-4o") };
    cfg.providers = list;

    var out_arena = std.heap.ArenaAllocator.init(allocator);
    defer out_arena.deinit();

    const result = try handle(
        .{ .io = std.testing.io, .gpa = allocator, .home = null, .config = &cfg },
        "",
        out_arena.allocator(),
    );
    switch (result) {
        .model_picker => |picker| {
            try std.testing.expectEqual(@as(usize, 2), picker.choices.len);
            try std.testing.expect(picker.choices[0].is_active);
            try std.testing.expect(!picker.choices[1].is_active);
        },
        else => return error.TestUnexpectedResult,
    }
}

test "applyModelChoice flips the active flag" {
    const allocator = std.testing.allocator;
    var tmp = std.testing.tmpDir(.{});
    defer tmp.cleanup();
    const home = try tmpHome(allocator, &tmp);
    defer allocator.free(home);
    try std.Io.Dir.cwd().createDirPath(std.testing.io, home);

    var cfg = try core.Config.load(allocator, std.testing.io, .{ .home = home });
    defer cfg.deinit();
    const a = cfg.arena.allocator();
    const list = try a.alloc(core.ProviderProfile, 2);
    list[0] = .{ .kind = .claude, .model = try a.dupe(u8, "claude-opus-4-7"), .active = true };
    list[1] = .{ .kind = .openai, .model = try a.dupe(u8, "gpt-4o") };
    cfg.providers = list;

    var out_arena = std.heap.ArenaAllocator.init(allocator);
    defer out_arena.deinit();

    const msg = try apply(
        .{ .io = std.testing.io, .gpa = allocator, .home = home, .config = &cfg },
        .{ .provider_index = 1, .kind = .openai, .model = "gpt-4o", .is_active = false },
        out_arena.allocator(),
    );
    try std.testing.expect(std.mem.indexOf(u8, msg, "gpt-4o") != null);

    // Flag flipped in memory.
    try std.testing.expect(!cfg.providers[0].active);
    try std.testing.expect(cfg.providers[1].active);
    try std.testing.expectEqualStrings("gpt-4o", cfg.activeProvider().?.model);

    // Written to disk.
    const cfg_path = try std.fs.path.join(allocator, &.{ home, ".phoenix", "phoenix.json" });
    defer allocator.free(cfg_path);
    const contents = try std.Io.Dir.cwd().readFileAlloc(std.testing.io, cfg_path, allocator, .limited(64 * 1024));
    defer allocator.free(contents);
    try std.testing.expect(std.mem.indexOf(u8, contents, "gpt-4o") != null);
    try std.testing.expect(std.mem.indexOf(u8, contents, "\"active\": true") != null);
}
