/// Provider registry: maps ProviderKind to adapter constructors.
///
/// Multiple profiles of the same kind are fully supported because profiles are
/// stored in `Config.providers` keyed by their bracket-suffix name (e.g.
/// `[provider.gpt4o]`, `[provider.gpt4mini]`). Two profiles can both have
/// `kind = "openai"` and differ on `model`, `api`, `auth`, etc. — they are
/// independent rows in the map. Calling `createProvider` for each produces
/// independent `*Provider` instances with their own resolved configs.
const std = @import("std");
const config = @import("config.zig");
const Provider = @import("provider.zig").Provider;
const ProviderConfig = @import("provider.zig").ProviderConfig;
const http_client = @import("http_client.zig");
const AuthConfig = config.AuthConfig;
const ProviderProfile = config.ProviderProfile;

const claude_mod = @import("providers_claude");
const openai_mod = @import("providers_openai");
const ollama_mod = @import("providers_ollama");
const llamacpp_mod = @import("providers_llamacpp");
const google_mod = @import("providers_google");

/// Create a Provider from a ProviderProfile, resolving credentials via AuthConfig.
/// The returned *Provider is heap-allocated; free with `destroyProvider`.
pub fn createProvider(
    allocator: std.mem.Allocator,
    profile: *const ProviderProfile,
    auth: *const AuthConfig,
) !*Provider {
    return createProviderInner(allocator, profile, auth, null);
}

/// Same as createProvider but injects a custom Transport (for testing).
pub fn createProviderWithTransport(
    allocator: std.mem.Allocator,
    profile: *const ProviderProfile,
    auth: *const AuthConfig,
    transport: http_client.Transport,
) !*Provider {
    return createProviderInner(allocator, profile, auth, transport);
}

fn createProviderInner(
    allocator: std.mem.Allocator,
    profile: *const ProviderProfile,
    auth: *const AuthConfig,
    transport: ?http_client.Transport,
) !*Provider {
    // Resolve credential up front so adapters don't reach back into AuthConfig.
    var resolved: ?[]u8 = null;
    if (profile.auth) |key| {
        resolved = try auth.resolve(allocator, key);
    }
    errdefer if (resolved) |c| allocator.free(c);

    const cfg = ProviderConfig{
        .model = profile.model,
        .auth_key = profile.auth,
        .base_url = profile.base_url,
        .endpoint = profile.endpoint,
        .max_retries = profile.max_retries,
        .request_timeout_ms = profile.request_timeout_ms,
        .api = profile.api,
        .project = profile.project,
        .location = profile.location,
        .credentials_path = profile.credentials_path,
        .resolved_credential = resolved,
    };

    return switch (profile.kind) {
        .claude => try claude_mod.create(allocator, cfg, transport),
        .openai => try openai_mod.create(allocator, cfg, transport),
        .ollama => try ollama_mod.create(allocator, cfg, transport),
        .llamacpp => try llamacpp_mod.create(allocator, cfg, transport),
        .vertex => try google_mod.createVertex(allocator, cfg, transport),
        .gemini => try google_mod.createGemini(allocator, cfg, transport),
    };
}

/// Destroy a Provider created by createProvider / createProviderWithTransport.
pub fn destroyProvider(allocator: std.mem.Allocator, p: *Provider) void {
    p.deinit(allocator);
}

// ---- Tests ----

test "every kind round-trips through createProvider" {
    const allocator = std.testing.allocator;
    // Minimal AuthConfig with no entries
    const auth: AuthConfig = .{};

    const kinds = [_]struct {
        profile: ProviderProfile,
        expected_name: []const u8,
    }{
        .{ .profile = .{ .kind = .claude, .model = "claude-opus-4-7" }, .expected_name = "claude" },
        .{ .profile = .{ .kind = .openai, .model = "gpt-4o" }, .expected_name = "openai" },
        .{ .profile = .{ .kind = .ollama, .model = "llama3.1" }, .expected_name = "ollama" },
        .{ .profile = .{ .kind = .llamacpp, .model = "ignored" }, .expected_name = "llamacpp" },
        .{ .profile = .{ .kind = .vertex, .model = "gemini-1.5-pro", .project = "my-proj" }, .expected_name = "vertex" },
        .{ .profile = .{ .kind = .gemini, .model = "gemini-1.5-flash" }, .expected_name = "gemini" },
    };

    for (kinds) |entry| {
        const p = try createProvider(allocator, &entry.profile, &auth);
        defer destroyProvider(allocator, p);
        try std.testing.expectEqualStrings(entry.expected_name, p.name);
    }
}

test "multiple profiles of same kind produce independent Provider instances" {
    const allocator = std.testing.allocator;
    const auth: AuthConfig = .{};

    const profile_a = ProviderProfile{ .kind = .openai, .model = "gpt-4o", .api = "completions" };
    const profile_b = ProviderProfile{ .kind = .openai, .model = "gpt-4o-mini", .api = "responses" };

    const pa = try createProvider(allocator, &profile_a, &auth);
    defer destroyProvider(allocator, pa);
    const pb = try createProvider(allocator, &profile_b, &auth);
    defer destroyProvider(allocator, pb);

    // They should be distinct pointers
    try std.testing.expect(pa != pb);
    // Both named "openai"
    try std.testing.expectEqualStrings("openai", pa.name);
    try std.testing.expectEqualStrings("openai", pb.name);
}
