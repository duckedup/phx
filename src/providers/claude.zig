const std = @import("std");
const core = @import("phoenix_core");

pub const ClaudeProvider = struct {
    base: core.Provider,

    pub fn init() ClaudeProvider {
        return .{
            .base = .{
                .name = "claude",
                .sendFn = send,
            },
        };
    }

    fn send(
        self: *core.Provider,
        messages: []const core.Message,
        config: core.ProviderConfig,
    ) anyerror!core.provider.EventIterator {
        _ = self;
        _ = messages;
        _ = config;
        return error.NotImplemented;
    }
};
