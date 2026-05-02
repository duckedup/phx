const std = @import("std");
const Message = @import("message.zig").Message;

pub const EventKind = enum {
    token,
    tool_call,
    tool_result,
    done,
    err,
};

pub const Event = struct {
    kind: EventKind,
    data: []const u8,
};

pub const EventIterator = struct {
    nextFn: *const fn (*EventIterator) ?Event,

    pub fn next(self: *EventIterator) ?Event {
        return self.nextFn(self);
    }
};

pub const ProviderConfig = struct {
    model: []const u8 = "claude-opus-4-7",
    api_key_env: []const u8 = "ANTHROPIC_API_KEY",
    base_url: ?[]const u8 = null,
    max_retries: u32 = 3,
    request_timeout_ms: u64 = 120_000,
};

pub const Provider = struct {
    name: []const u8,
    sendFn: *const fn (
        self: *Provider,
        messages: []const Message,
        config: ProviderConfig,
    ) anyerror!EventIterator,

    pub fn send(
        self: *Provider,
        messages: []const Message,
        config: ProviderConfig,
    ) !EventIterator {
        return self.sendFn(self, messages, config);
    }
};
