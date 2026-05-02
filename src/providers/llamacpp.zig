/// llama.cpp server adapter — thin wrapper around OpenAI Chat Completions.
///
/// llama.cpp's HTTP server exposes an OpenAI-compatible `/v1/chat/completions`
/// endpoint with SSE streaming and tool calling. We reuse the openai.zig Chat
/// Completions implementation with a different default base URL and optional auth.
const std = @import("std");
const core = @import("phoenix_core");
const http_client = core.http_client;
const openai = @import("providers_openai");

const Provider = core.Provider;
const ProviderConfig = core.ProviderConfig;
const Transport = http_client.Transport;

const DEFAULT_BASE_URL = "http://localhost:8080";

pub fn create(
    allocator: std.mem.Allocator,
    cfg: ProviderConfig,
    transport: ?Transport,
) !*Provider {
    var p = try openai.createCompletionsImpl(
        allocator,
        cfg,
        transport,
        DEFAULT_BASE_URL,
        false,
        .completions,
    );
    // Override the name to "llamacpp" so registry tests pass.
    p.name = "llamacpp";
    return p;
}

// ---- Tests ----

const test_util = @import("test_util");
const MockTransport = http_client.MockTransport;
const Message = core.Message;
const StopReason = core.StopReason;

const minimal_done_sse =
    "data: {\"choices\":[{\"index\":0,\"finish_reason\":\"stop\"}]}\n\n" ++
    "data: {\"usage\":{\"prompt_tokens\":0,\"completion_tokens\":0}}\n\n" ++
    "data: [DONE]\n\n";

test "llamacpp: defaults to localhost:8080" {
    const allocator = std.testing.allocator;
    const canned = [_]http_client.Canned{.{ .body = minimal_done_sse }};
    var mock = MockTransport.init(allocator, &canned);
    defer mock.deinit();

    var provider = try create(allocator, .{ .model = "llama-3" }, mock.transport());
    defer provider.deinit(allocator);

    const messages = [_]Message{.{ .role = .user, .content = "hi" }};
    var it = try provider.send(allocator, .{
        .messages = &messages,
        .config = .{ .model = "llama-3" },
    });
    defer it.deinit();
    while (it.next()) |_| {}

    const captured = mock.captured.items[0];
    try std.testing.expectEqualStrings("http://localhost:8080/v1/chat/completions", captured.url);
}

test "llamacpp: no Authorization header when no credential" {
    const allocator = std.testing.allocator;
    const canned = [_]http_client.Canned{.{ .body = minimal_done_sse }};
    var mock = MockTransport.init(allocator, &canned);
    defer mock.deinit();

    var provider = try create(allocator, .{
        .model = "llama-3",
        .resolved_credential = null,
    }, mock.transport());
    defer provider.deinit(allocator);

    const messages = [_]Message{.{ .role = .user, .content = "hi" }};
    var it = try provider.send(allocator, .{
        .messages = &messages,
        .config = .{ .model = "llama-3", .resolved_credential = null },
    });
    defer it.deinit();
    while (it.next()) |_| {}

    const captured = mock.captured.items[0];
    try std.testing.expect(test_util.headerValue(captured.headers, "Authorization") == null);
}

test "llamacpp: with credential, sends Bearer auth" {
    const allocator = std.testing.allocator;
    const canned = [_]http_client.Canned{.{ .body = minimal_done_sse }};
    var mock = MockTransport.init(allocator, &canned);
    defer mock.deinit();

    var provider = try create(allocator, .{
        .model = "llama-3",
        .resolved_credential = "secret",
    }, mock.transport());
    defer provider.deinit(allocator);

    const messages = [_]Message{.{ .role = .user, .content = "hi" }};
    var it = try provider.send(allocator, .{
        .messages = &messages,
        .config = .{ .model = "llama-3", .resolved_credential = "secret" },
    });
    defer it.deinit();
    while (it.next()) |_| {}

    const captured = mock.captured.items[0];
    try std.testing.expectEqualStrings("Bearer secret", test_util.headerValue(captured.headers, "Authorization").?);
}

test "llamacpp: token streaming works (shares openai parser)" {
    const allocator = std.testing.allocator;
    const fixture =
        "data: {\"choices\":[{\"index\":0,\"delta\":{\"role\":\"assistant\",\"content\":\"\"}}]}\n\n" ++
        "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"Hi\"}}]}\n\n" ++
        "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"!\"}}]}\n\n" ++
        "data: {\"choices\":[{\"index\":0,\"finish_reason\":\"stop\"}]}\n\n" ++
        "data: {\"usage\":{\"prompt_tokens\":3,\"completion_tokens\":2}}\n\n" ++
        "data: [DONE]\n\n";

    const canned = [_]http_client.Canned{.{ .body = fixture }};
    var mock = MockTransport.init(allocator, &canned);
    defer mock.deinit();

    var provider = try create(allocator, .{ .model = "llama-3" }, mock.transport());
    defer provider.deinit(allocator);

    const messages = [_]Message{.{ .role = .user, .content = "hi" }};
    var it = try provider.send(allocator, .{
        .messages = &messages,
        .config = .{ .model = "llama-3" },
    });
    defer it.deinit();

    const events = try test_util.collectEvents(&it, allocator);
    defer test_util.freeEvents(allocator, events);

    var token_count: usize = 0;
    var done_ev: ?core.DoneEvent = null;
    for (events) |ev| switch (ev) {
        .token => token_count += 1,
        .done => |d| done_ev = d,
        else => {},
    };

    try std.testing.expectEqual(@as(usize, 2), token_count);
    try std.testing.expect(done_ev != null);
    try std.testing.expectEqual(StopReason.end_turn, done_ev.?.stop_reason);
}

test "llamacpp: e2e roundtrip" {
    if (std.c.getenv("PHOENIX_E2E") == null) return error.SkipZigTest;

    var p = try create(std.testing.allocator, .{ .model = "local-model" }, null);
    defer p.deinit(std.testing.allocator);

    const msgs = [_]Message{.{ .role = .user, .content = "Reply with the word OK and nothing else." }};
    var it = try p.send(std.testing.allocator, .{
        .messages = &msgs,
        .config = .{ .model = "local-model" },
    });
    defer it.deinit();

    var buf: std.ArrayList(u8) = .empty;
    defer buf.deinit(std.testing.allocator);
    while (it.next()) |ev| switch (ev) {
        .token => |t| try buf.appendSlice(std.testing.allocator, t),
        .done => break,
        .err => |e| std.debug.panic("provider error: {s}", .{e}),
        else => {},
    };
    try std.testing.expect(std.mem.indexOf(u8, buf.items, "OK") != null);
}
