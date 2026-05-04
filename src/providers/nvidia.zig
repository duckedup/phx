/// NVIDIA NIM adapter — thin wrapper around OpenAI Chat Completions.
///
/// NVIDIA's NIM API exposes an OpenAI-compatible `/v1/chat/completions`
/// endpoint with SSE streaming and tool calling. We reuse the openai.zig Chat
/// Completions implementation with NVIDIA's default base URL.
const std = @import("std");
const core = @import("phoenix_core");
const http_client = core.http_client;
const openai = @import("providers_openai");

const Provider = core.Provider;
const ProviderConfig = core.ProviderConfig;
const Transport = http_client.Transport;

const DEFAULT_BASE_URL = "https://integrate.api.nvidia.com/v1";

pub fn create(
    allocator: std.mem.Allocator,
    io: std.Io,
    cfg: ProviderConfig,
    transport: ?Transport,
) !*Provider {
    return openai.createCompletionsImpl(
        allocator,
        io,
        cfg,
        transport,
        DEFAULT_BASE_URL,
        "nvidia",
        .completions,
    );
}