/// Model context-window lookup.
///
/// Phoenix needs to know "how big can the conversation get" before triggering
/// auto-compaction. There are three sources, queried in priority order:
///
/// 1. The provider profile's `context_window` override (set by onboarding for
///    local providers, or by users who want to cap a cloud model below its
///    nominal limit).
/// 2. A best-effort live query against the provider (currently implemented
///    for ollama; placeholder for others).
/// 3. A static fallback table, keyed by model id with longest-prefix matching.
///
/// The fallback table is curated rather than exhaustive — when a model name
/// matches no entry, callers get `default_context_window` and a debug log.
const std = @import("std");
const config = @import("config.zig");

/// Conservative default for unknown models. 8K tokens is large enough that
/// trivial conversations don't compact every turn, small enough that we
/// don't blow the real context for genuinely small models.
pub const default_context_window: u32 = 8192;

const Entry = struct {
    /// Prefix matched against `profile.model`. Longest match wins.
    prefix: []const u8,
    kind: config.ProviderKind,
    context_window: u32,
};

/// Curated table of well-known models. Order is irrelevant — selection is
/// longest-prefix-wins, with a kind-equality tiebreaker.
const fallback_table = [_]Entry{
    // Anthropic — full Claude 4.x family + 3.x legacy.
    .{ .prefix = "claude-opus-4", .kind = .claude, .context_window = 200_000 },
    .{ .prefix = "claude-sonnet-4", .kind = .claude, .context_window = 200_000 },
    .{ .prefix = "claude-haiku-4", .kind = .claude, .context_window = 200_000 },
    .{ .prefix = "claude-3-7", .kind = .claude, .context_window = 200_000 },
    .{ .prefix = "claude-3-5", .kind = .claude, .context_window = 200_000 },
    .{ .prefix = "claude-3", .kind = .claude, .context_window = 200_000 },

    // OpenAI — GPT-5 / 4o / o-series.
    .{ .prefix = "gpt-5", .kind = .openai, .context_window = 400_000 },
    .{ .prefix = "gpt-4.1", .kind = .openai, .context_window = 1_000_000 },
    .{ .prefix = "gpt-4o", .kind = .openai, .context_window = 128_000 },
    .{ .prefix = "gpt-4-turbo", .kind = .openai, .context_window = 128_000 },
    .{ .prefix = "gpt-4", .kind = .openai, .context_window = 8_192 },
    .{ .prefix = "o3", .kind = .openai, .context_window = 200_000 },
    .{ .prefix = "o1", .kind = .openai, .context_window = 200_000 },

    // Google Gemini.
    .{ .prefix = "gemini-2.5", .kind = .gemini, .context_window = 1_000_000 },
    .{ .prefix = "gemini-2.0", .kind = .gemini, .context_window = 1_000_000 },
    .{ .prefix = "gemini-1.5-pro", .kind = .gemini, .context_window = 2_000_000 },
    .{ .prefix = "gemini-1.5", .kind = .gemini, .context_window = 1_000_000 },
    .{ .prefix = "gemini-2.5", .kind = .vertex, .context_window = 1_000_000 },
    .{ .prefix = "gemini-2.0", .kind = .vertex, .context_window = 1_000_000 },
    .{ .prefix = "gemini-1.5", .kind = .vertex, .context_window = 1_000_000 },
};

/// Synchronous lookup: profile override → static fallback → default. Never
/// makes a network call. Use `refresh` for live values.
pub fn lookup(profile: *const config.ProviderProfile) u32 {
    if (profile.context_window) |cw| return cw;

    // Local providers have no canonical limit. Fall through to the default
    // until the user sets `context_window` on the profile (onboarding does
    // this for ollama/llamacpp).
    switch (profile.kind) {
        .ollama, .llamacpp => return default_context_window,
        else => {},
    }

    return staticLookup(profile.kind, profile.model) orelse default_context_window;
}

/// Search the curated table for the longest prefix matching `model` whose
/// kind equals `kind`. Returns null if nothing matches.
pub fn staticLookup(kind: config.ProviderKind, model: []const u8) ?u32 {
    var best_len: usize = 0;
    var best: ?u32 = null;
    for (fallback_table) |entry| {
        if (entry.kind != kind) continue;
        if (!std.mem.startsWith(u8, model, entry.prefix)) continue;
        if (entry.prefix.len > best_len) {
            best_len = entry.prefix.len;
            best = entry.context_window;
        }
    }
    return best;
}

/// Live-query the provider for the active model's context window.
///
/// Currently:
///   - `.ollama`         — POST {base_url}/api/show, parse `model_info.*.context_length`.
///   - everything else   — falls back to `lookup(profile)` (no network).
///
/// This is the extension point for adding more providers; the API surface
/// stays the same so callers don't branch on kind.
pub fn refresh(
    io: std.Io,
    allocator: std.mem.Allocator,
    profile: *const config.ProviderProfile,
) !u32 {
    if (profile.kind == .ollama) {
        if (try fetchOllamaContextLength(io, allocator, profile)) |n| return n;
    }
    return lookup(profile);
}

fn fetchOllamaContextLength(
    io: std.Io,
    allocator: std.mem.Allocator,
    profile: *const config.ProviderProfile,
) !?u32 {
    const base = profile.base_url orelse "http://localhost:11434";

    const url = try std.fmt.allocPrint(allocator, "{s}/api/show", .{base});
    defer allocator.free(url);

    const body = try std.fmt.allocPrint(allocator, "{{\"model\":\"{s}\"}}", .{profile.model});
    defer allocator.free(body);

    var http = std.http.Client{ .allocator = allocator, .io = io };
    defer http.deinit();

    var resp_storage: std.ArrayList(u8) = .empty;
    defer resp_storage.deinit(allocator);

    const result = http.fetch(.{
        .location = .{ .url = url },
        .method = .POST,
        .payload = body,
        .headers = .{ .content_type = .{ .override = "application/json" } },
        .response_storage = .{ .dynamic = &resp_storage },
        .max_append_size = 1 * 1024 * 1024,
    }) catch return null;

    if (result.status != .ok) return null;

    var arena = std.heap.ArenaAllocator.init(allocator);
    defer arena.deinit();
    const parsed = std.json.parseFromSliceLeaky(
        std.json.Value,
        arena.allocator(),
        resp_storage.items,
        .{},
    ) catch return null;
    if (parsed != .object) return null;

    // Ollama returns `model_info` keyed by the model architecture, e.g.
    // `llama.context_length`. We don't know the architecture up front, so
    // scan for any value whose key ends with ".context_length".
    const mi = parsed.object.get("model_info") orelse return null;
    if (mi != .object) return null;
    var it = mi.object.iterator();
    while (it.next()) |kv| {
        if (!std.mem.endsWith(u8, kv.key_ptr.*, ".context_length")) continue;
        switch (kv.value_ptr.*) {
            .integer => |i| {
                if (i <= 0) return null;
                return @intCast(@min(i, std.math.maxInt(u32)));
            },
            else => continue,
        }
    }
    return null;
}

// ---- Tests ----

const testing = std.testing;

test "lookup: profile override wins" {
    var p = config.ProviderProfile{
        .kind = .claude,
        .model = "claude-opus-4-7",
        .context_window = 1234,
    };
    try testing.expectEqual(@as(u32, 1234), lookup(&p));
}

test "lookup: claude-opus matches via static table" {
    var p = config.ProviderProfile{
        .kind = .claude,
        .model = "claude-opus-4-7",
    };
    try testing.expectEqual(@as(u32, 200_000), lookup(&p));
}

test "lookup: openai gpt-5 matches" {
    var p = config.ProviderProfile{
        .kind = .openai,
        .model = "gpt-5.5",
    };
    try testing.expectEqual(@as(u32, 400_000), lookup(&p));
}

test "lookup: longest-prefix wins (gpt-4o vs gpt-4)" {
    var p = config.ProviderProfile{
        .kind = .openai,
        .model = "gpt-4o-2024-11",
    };
    try testing.expectEqual(@as(u32, 128_000), lookup(&p));
}

test "lookup: ollama with no override falls back to default" {
    var p = config.ProviderProfile{
        .kind = .ollama,
        .model = "llama3.3",
    };
    try testing.expectEqual(default_context_window, lookup(&p));
}

test "lookup: ollama with profile override returns it" {
    var p = config.ProviderProfile{
        .kind = .ollama,
        .model = "llama3.3",
        .context_window = 32_000,
    };
    try testing.expectEqual(@as(u32, 32_000), lookup(&p));
}

test "lookup: unknown openai model returns default" {
    var p = config.ProviderProfile{
        .kind = .openai,
        .model = "totally-fake-9000",
    };
    try testing.expectEqual(default_context_window, lookup(&p));
}

test "staticLookup: respects kind boundary" {
    // gpt-* is openai-only; the same model name under .claude must miss.
    try testing.expect(staticLookup(.claude, "gpt-5") == null);
    try testing.expect(staticLookup(.openai, "gpt-5") != null);
}
