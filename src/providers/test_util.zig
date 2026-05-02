/// Shared test helpers for provider adapter unit tests.
const std = @import("std");
const core = @import("phoenix_core");
const http_client = core.http_client;

pub const MockTransport = http_client.MockTransport;
pub const Header = http_client.Header;

/// An owned copy of a provider Event (slices are dup'd so they survive
/// past the next EventIterator.next() call).
pub const CollectedEvent = union(core.EventKind) {
    token: []u8,
    tool_call: struct { id: []u8, name: []u8, args_json: []u8 },
    tool_result: struct { id: []u8, output: []u8, is_error: bool },
    done: core.DoneEvent,
    err: []u8,
};

/// Drain an EventIterator into a heap-allocated slice of owned events.
/// Caller must call freeEvents() when done.
pub fn collectEvents(
    it: *core.provider.EventIterator,
    allocator: std.mem.Allocator,
) ![]CollectedEvent {
    var list: std.ArrayList(CollectedEvent) = .empty;
    errdefer {
        for (list.items) |ev| freeOne(allocator, ev);
        list.deinit(allocator);
    }

    while (it.next()) |ev| {
        const owned: CollectedEvent = switch (ev) {
            .token => |t| .{ .token = try allocator.dupe(u8, t) },
            .tool_call => |tc| .{ .tool_call = .{
                .id = try allocator.dupe(u8, tc.id),
                .name = try allocator.dupe(u8, tc.name),
                .args_json = try allocator.dupe(u8, tc.args_json),
            } },
            .tool_result => |tr| .{ .tool_result = .{
                .id = try allocator.dupe(u8, tr.id),
                .output = try allocator.dupe(u8, tr.output),
                .is_error = tr.is_error,
            } },
            .done => |d| .{ .done = d },
            .err => |e| .{ .err = try allocator.dupe(u8, e) },
        };
        try list.append(allocator, owned);
    }

    return try list.toOwnedSlice(allocator);
}

fn freeOne(allocator: std.mem.Allocator, ev: CollectedEvent) void {
    switch (ev) {
        .token => |t| allocator.free(t),
        .tool_call => |tc| {
            allocator.free(tc.id);
            allocator.free(tc.name);
            allocator.free(tc.args_json);
        },
        .tool_result => |tr| {
            allocator.free(tr.id);
            allocator.free(tr.output);
        },
        .done => {},
        .err => |e| allocator.free(e),
    }
}

/// Free a slice of CollectedEvent obtained from collectEvents.
pub fn freeEvents(allocator: std.mem.Allocator, events: []CollectedEvent) void {
    for (events) |ev| freeOne(allocator, ev);
    allocator.free(events);
}

/// Find a header by name (case-insensitive). Returns the value slice, or null.
pub fn headerValue(headers: []const Header, name: []const u8) ?[]const u8 {
    for (headers) |h| {
        if (std.ascii.eqlIgnoreCase(h.name, name)) return h.value;
    }
    return null;
}

/// Build a one-shot MockTransport that returns `body` with status 200.
pub fn mockOnce(allocator: std.mem.Allocator, body: []const u8) MockTransport {
    const canned = allocator.alloc(http_client.Canned, 1) catch @panic("OOM");
    canned[0] = .{ .status = 200, .body = body };
    return MockTransport.init(allocator, canned);
}

/// Minimal SSE payload that represents an empty message_stop (used when we
/// only care about the request shape, not the response events).
pub const minimal_done_sse =
    "event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"id\":\"m0\",\"usage\":{\"input_tokens\":0,\"output_tokens\":0}}}\n\n" ++
    "event: message_delta\ndata: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"},\"usage\":{\"output_tokens\":0}}\n\n" ++
    "event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n";
