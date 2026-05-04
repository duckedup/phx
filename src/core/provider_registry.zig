/// Provider registry: maps ProviderKind to adapter constructors.
///
/// Multiple profiles of the same kind are fully supported. Two entries in
/// `Config.providers` can both have `kind = "openai"` and differ on `model`,
/// `api`, `auth`, etc. — they are independent rows in the array. Calling
/// `createProvider` for each produces independent `*Provider` instances with
/// their own resolved configs.
const std = @import("std");
const config = @import("config.zig");
const Provider = @import("provider.zig").Provider;
const ProviderConfig = @import("provider.zig").ProviderConfig;
const http_client = @import("http_client.zig");
const ProviderProfile = config.ProviderProfile;

const claude_mod = @import("providers_claude");
const openai_mod = @import("providers_openai");
const ollama_mod = @import("providers_ollama");
const llamacpp_mod = @import("providers_llamacpp");
const google_mod = @import("providers_google");
const nvidia_mod = @import("providers_nvidia");

/// Create a Provider from a ProviderProfile. The provider takes ownership of
/// any credential it allocates (resolved via `profile.auth`).
pub fn createProvider(
    allocator: std.mem.Allocator,
    io: std.Io,
    profile: *const ProviderProfile,
) !*Provider {
    return createProviderInner(allocator, io, profile, null);
}

/// Same as createProvider but injects a custom Transport (for testing).
pub fn createProviderWithTransport(
    allocator: std.mem.Allocator,
    io: std.Io,
    profile: *const ProviderProfile,
    transport: http_client.Transport,
) !*Provider {
    return createProviderInner(allocator, io, profile, transport);
}

fn createProviderInner(
    allocator: std.mem.Allocator,
    io: std.Io,
    profile: *const ProviderProfile,
    transport: ?http_client.Transport,
) !*Provider {
    // Resolve credential up front so adapters don't have to reason about
    // AuthEntry shapes. The adapter's deinit frees the resolved string when
    // resolved_credential_owned is true.
    var resolved: ?[]u8 = null;
    if (profile.auth) |entry| {
        resolved = try entry.resolve(allocator);
    }
    errdefer if (resolved) |c| allocator.free(c);

    const cfg = ProviderConfig{
        .model = profile.model,
        .auth_key = null,
        .base_url = profile.base_url,
        .endpoint = profile.endpoint,
        .max_retries = profile.max_retries,
        .request_timeout_ms = profile.request_timeout_ms,
        .api = profile.api,
        .project = profile.project,
        .location = profile.location,
        .credentials_path = profile.credentials_path,
        .resolved_credential = resolved,
        .resolved_credential_owned = (resolved != null),
    };

    return switch (profile.kind) {
        .claude => try claude_mod.create(allocator, io, cfg, transport),
        .openai => try openai_mod.create(allocator, io, cfg, transport),
        .ollama => try ollama_mod.create(allocator, io, cfg, transport),
        .llamacpp => try llamacpp_mod.create(allocator, io, cfg, transport),
        .vertex => try google_mod.createVertex(allocator, io, cfg, transport),
        .gemini => try google_mod.createGemini(allocator, io, cfg, transport),
        .nvidia => try nvidia_mod.create(allocator, io, cfg, transport),
    };
}

/// Destroy a Provider created by createProvider / createProviderWithTransport.
pub fn destroyProvider(allocator: std.mem.Allocator, p: *Provider) void {
    p.deinit(allocator);
}

// ---- Tests ----

test "every kind round-trips through createProvider" {
    const allocator = std.testing.allocator;

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
        .{ .profile = .{ .kind = .nvidia, .model = "nvidia/llama-3.1-nemotron-70b-instruct" }, .expected_name = "nvidia" },
    };

    for (kinds) |entry| {
        const p = try createProvider(allocator, std.testing.io, &entry.profile);
        defer destroyProvider(allocator, p);
        try std.testing.expectEqualStrings(entry.expected_name, p.name);
    }
}

test "multiple profiles of same kind produce independent Provider instances" {
    const allocator = std.testing.allocator;

    const profile_a = ProviderProfile{ .kind = .openai, .model = "gpt-4o", .api = "completions" };
    const profile_b = ProviderProfile{ .kind = .openai, .model = "gpt-4o-mini", .api = "responses" };

    const pa = try createProvider(allocator, std.testing.io, &profile_a);
    defer destroyProvider(allocator, pa);
    const pb = try createProvider(allocator, std.testing.io, &profile_b);
    defer destroyProvider(allocator, pb);

    try std.testing.expect(pa != pb);
    try std.testing.expectEqualStrings("openai", pa.name);
    try std.testing.expectEqualStrings("openai", pb.name);
}
