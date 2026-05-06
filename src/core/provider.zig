const std = @import("std");
const Message = @import("message.zig").Message;
const Tool = @import("tool.zig").Tool;

pub const ProviderError = error{
    NotImplemented,
    InvalidConfig,
    MissingCredential,
    HttpError,
    BadStatus,
    BadResponse,
    Truncated,
    Cancelled,
    Timeout,
    OutOfMemory,
};

pub const StopReason = enum {
    end_turn, // model decided it was done
    max_tokens, // hit output cap
    tool_use, // model wants to call a tool; caller should run it and resume
    stop_sequence,
    other,
};

pub const ToolCallEvent = struct {
    /// Provider-assigned identifier (Anthropic block id, OpenAI tool_call id,
    /// etc.). Borrowed; valid for the lifetime documented on EventIterator.
    id: []const u8,
    name: []const u8,
    /// Raw JSON arguments object as sent by the model. May be empty `"{}"`.
    args_json: []const u8,
};

pub const ToolResultEvent = struct {
    /// Echoes the originating ToolCallEvent.id so the model can correlate.
    id: []const u8,
    output: []const u8,
    is_error: bool = false,
};

pub const Usage = struct {
    input_tokens: u32 = 0,
    output_tokens: u32 = 0,
    cache_creation_input_tokens: u32 = 0,
    cache_read_input_tokens: u32 = 0,
};

pub const DoneEvent = struct {
    stop_reason: StopReason = .other,
    usage: Usage = .{},
};

pub const EventKind = enum {
    token,
    tool_call,
    tool_result,
    done,
    err,
};

pub const Event = union(EventKind) {
    /// A streamed text fragment (UTF-8, may split codepoints across events).
    token: []const u8,
    tool_call: ToolCallEvent,
    /// Adapters may emit `tool_result` to acknowledge a tool result they
    /// received in `messages` (rare). Most callers will only see
    /// `tool_result` events when the session loop synthesizes them.
    tool_result: ToolResultEvent,
    done: DoneEvent,
    /// Human-readable error message; pair with `getError(it)` for the typed code.
    err: []const u8,
};

/// Iterator yielding events from a single `send` call.
///
/// Memory model: each call to `next()` MAY invalidate slices held by the
/// previously returned `Event`. Callers that need to retain event data past
/// the next `next()` call must `dupe` it themselves. All borrowed slices are
/// guaranteed valid until the *next* `next()` call or `deinit()`, whichever
/// comes first.
///
/// Errors during streaming are surfaced as `Event{.err}` rather than as Zig
/// errors from `next()`, so a single iteration loop can handle them inline.
pub const EventIterator = struct {
    nextFn: *const fn (*EventIterator) ?Event,
    deinitFn: *const fn (*EventIterator) void,
    /// Opaque pointer to the concrete iterator struct. Adapters cast this via
    /// `@ptrCast` / `@alignCast` to recover their own state without relying on
    /// `@fieldParentPtr`, which is unsafe when the EventIterator is returned
    /// by value and copied to the caller's stack.
    ctx: *anyopaque,

    pub fn next(self: *EventIterator) ?Event {
        return self.nextFn(self);
    }

    pub fn deinit(self: *EventIterator) void {
        self.deinitFn(self);
    }
};

pub const ProviderConfig = struct {
    model: []const u8 = "claude-opus-4-7",
    /// Name of the auth entry to look up in AuthConfig; null = no auth header.
    auth_key: ?[]const u8 = null,
    base_url: ?[]const u8 = null,
    /// Override the request path (rare; provider-defaults usually correct).
    endpoint: ?[]const u8 = null,
    max_retries: u32 = 3,
    request_timeout_ms: u64 = 120_000,

    // Provider-specific extras (only used by some adapters):
    /// OpenAI: "completions" | "responses". Ignored by other adapters.
    api: ?[]const u8 = null,
    /// Google Vertex: GCP project ID.
    project: ?[]const u8 = null,
    /// Google Vertex: GCP region (e.g. "us-central1").
    location: ?[]const u8 = null,
    /// Google Vertex: optional path to service account / ADC JSON. If null,
    /// adapter falls back to `auth_key` (pre-issued access token) or shells
    /// out to `gcloud auth print-access-token`.
    credentials_path: ?[]const u8 = null,

    /// Resolved bearer token / api key (filled in by createProvider before send).
    /// Adapters do NOT call AuthConfig.resolve themselves.
    resolved_credential: ?[]const u8 = null,
    /// True iff `resolved_credential` was heap-allocated by `createProvider`
    /// and must be freed by the adapter on deinit. False when the caller
    /// (tests, mainly) supplies a string-literal or otherwise non-owned
    /// slice. The registry sets this to true when it dupes through
    /// `AuthConfig.resolve`.
    resolved_credential_owned: bool = false,

    /// Optional system prompt; some providers take this as a separate field
    /// (Anthropic), others embed it in the messages array (OpenAI).
    system_prompt: ?[]const u8 = null,

    /// Cache TTL for prompt caching (e.g. "5m", "1h"). Provider-specific.
    cache_ttl: ?[]const u8 = null,
};

pub const SendOptions = struct {
    messages: []const Message,
    tools: []const *const Tool = &.{},
    config: ProviderConfig,
    /// Cancellation flag adapters poll between SSE chunks. Optional.
    cancelled: ?*const std.atomic.Value(bool) = null,
};

pub const Provider = struct {
    name: []const u8,
    sendFn: *const fn (
        self: *Provider,
        allocator: std.mem.Allocator,
        options: SendOptions,
    ) anyerror!EventIterator,
    /// Free any provider-owned state. Called by createProvider's `destroyProvider`.
    deinitFn: *const fn (self: *Provider, allocator: std.mem.Allocator) void,

    pub fn send(
        self: *Provider,
        allocator: std.mem.Allocator,
        options: SendOptions,
    ) !EventIterator {
        return self.sendFn(self, allocator, options);
    }

    pub fn deinit(self: *Provider, allocator: std.mem.Allocator) void {
        self.deinitFn(self, allocator);
    }
};

// ---- Tests ----

test "provider event union construction and switch" {
    const token_ev = Event{ .token = "hello" };
    const tool_call_ev = Event{ .tool_call = .{
        .id = "tc_1",
        .name = "my_tool",
        .args_json = "{}",
    } };
    const tool_result_ev = Event{ .tool_result = .{
        .id = "tc_1",
        .output = "result",
        .is_error = false,
    } };
    const done_ev = Event{ .done = .{
        .stop_reason = .end_turn,
        .usage = .{ .input_tokens = 10, .output_tokens = 20 },
    } };
    const err_ev = Event{ .err = "something failed" };

    // Verify switch works correctly on each variant
    switch (token_ev) {
        .token => |t| try std.testing.expectEqualStrings("hello", t),
        else => return error.WrongVariant,
    }
    switch (tool_call_ev) {
        .tool_call => |tc| {
            try std.testing.expectEqualStrings("tc_1", tc.id);
            try std.testing.expectEqualStrings("my_tool", tc.name);
            try std.testing.expectEqualStrings("{}", tc.args_json);
        },
        else => return error.WrongVariant,
    }
    switch (tool_result_ev) {
        .tool_result => |tr| {
            try std.testing.expectEqualStrings("tc_1", tr.id);
            try std.testing.expectEqualStrings("result", tr.output);
            try std.testing.expect(!tr.is_error);
        },
        else => return error.WrongVariant,
    }
    switch (done_ev) {
        .done => |d| {
            try std.testing.expectEqual(StopReason.end_turn, d.stop_reason);
            try std.testing.expectEqual(@as(u32, 10), d.usage.input_tokens);
            try std.testing.expectEqual(@as(u32, 20), d.usage.output_tokens);
        },
        else => return error.WrongVariant,
    }
    switch (err_ev) {
        .err => |e| try std.testing.expectEqualStrings("something failed", e),
        else => return error.WrongVariant,
    }
}
