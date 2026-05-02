const std = @import("std");

pub const RuntimeMode = enum {
    tui,
    rpc,
};

pub const ProviderKind = enum {
    claude,
    openai,
    ollama,
    llamacpp,
};

pub const StoreBackend = enum {
    memory,
    beans,
};

pub const Config = struct {
    mode: RuntimeMode = .tui,
    log_level: std.log.Level = .info,
    max_concurrent_sessions: ?u32 = null,

    provider_kind: ProviderKind = .claude,
    provider_model: []const u8 = "claude-opus-4-7",
    provider_api_key_env: []const u8 = "ANTHROPIC_API_KEY",
    provider_base_url: ?[]const u8 = null,

    store_backend: StoreBackend = .memory,
    store_path: []const u8 = "./.phoenix/store",

    compaction: []const u8 = "summarize",
    compaction_threshold: f32 = 0.8,
    compaction_tail_turns: u32 = 3,

    pub fn effectiveConcurrency(self: *const Config) u32 {
        return self.max_concurrent_sessions orelse blk: {
            const cpu_count = std.Thread.getCpuCount() catch 4;
            break :blk @intCast(cpu_count * 2);
        };
    }
};
