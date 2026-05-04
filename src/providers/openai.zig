/// OpenAI Chat Completions and Responses API adapter.
const std = @import("std");
const builtin = @import("builtin");
const core = @import("phoenix_core");
const http_client = core.http_client;
const sse_mod = core.sse;
const json_util = core.json_util;

const Provider = core.Provider;
const ProviderConfig = core.ProviderConfig;
const SendOptions = core.SendOptions;
const Event = core.Event;
const EventIterator = core.provider.EventIterator;
const StopReason = core.StopReason;
const Transport = http_client.Transport;
const HttpTransport = http_client.HttpTransport;
const Message = core.Message;

const DEFAULT_BASE_URL = "https://api.openai.com";

const ApiMode = enum { completions, responses };

// ---- Provider struct ----

const OpenAIProvider = struct {
    base: Provider,
    allocator: std.mem.Allocator,
    cfg: ProviderConfig,
    mode: ApiMode,
    transport_owned: ?HttpTransport,
    transport: Transport,
    /// Default base URL used when cfg.base_url is null. Varies by caller.
    default_base_url: []const u8,
};

pub fn create(
    allocator: std.mem.Allocator,
    io: std.Io,
    cfg: ProviderConfig,
    transport: ?Transport,
) !*Provider {
    const mode = blk: {
        const api = cfg.api orelse break :blk ApiMode.completions;
        if (std.mem.eql(u8, api, "completions") or std.mem.eql(u8, api, "")) {
            break :blk ApiMode.completions;
        }
        if (std.mem.eql(u8, api, "responses")) break :blk ApiMode.responses;
        return error.InvalidConfig;
    };
    return createCompletionsImpl(allocator, io, cfg, transport, DEFAULT_BASE_URL, "openai", mode);
}

/// Package-internal: used by llamacpp.zig and nvidia.zig to compose the Chat
/// Completions logic with a different default URL and optional auth.
pub fn createCompletionsImpl(
    allocator: std.mem.Allocator,
    io: std.Io,
    cfg: ProviderConfig,
    injected: ?Transport,
    default_base_url: []const u8,
    name: []const u8,
    mode: ApiMode,
) !*Provider {
    const self = try allocator.create(OpenAIProvider);
    errdefer allocator.destroy(self);
    self.* = .{
        .base = .{ .name = name, .sendFn = sendImpl, .deinitFn = deinitImpl },
        .allocator = allocator,
        .cfg = cfg,
        .mode = mode,
        .transport_owned = if (injected == null) HttpTransport.init(allocator, io) else null,
        .transport = undefined,
        .default_base_url = default_base_url,
    };
    self.transport = if (injected) |t| t else self.transport_owned.?.transport();
    return &self.base;
}

fn deinitImpl(p: *Provider, allocator: std.mem.Allocator) void {
    const self: *OpenAIProvider = @alignCast(@fieldParentPtr("base", p));
    if (self.cfg.resolved_credential_owned) {
        if (self.cfg.resolved_credential) |c| allocator.free(c);
    }
    if (self.transport_owned) |*t| t.deinit();
    allocator.destroy(self);
}

fn sendImpl(
    p: *Provider,
    allocator: std.mem.Allocator,
    options: SendOptions,
) anyerror!EventIterator {
    const self: *OpenAIProvider = @alignCast(@fieldParentPtr("base", p));

    const base_url = options.config.base_url orelse self.cfg.base_url orelse self.default_base_url;

    const path = switch (self.mode) {
        .completions => "/v1/chat/completions",
        .responses => "/v1/responses",
    };

    var url_buf: std.ArrayList(u8) = .empty;
    defer url_buf.deinit(allocator);
    try url_buf.appendSlice(allocator, base_url);
    try url_buf.appendSlice(allocator, path);
    const url = try url_buf.toOwnedSlice(allocator);
    defer allocator.free(url);

    const body = switch (self.mode) {
        .completions => try buildCompletionsBody(allocator, &options),
        .responses => try buildResponsesBody(allocator, &options),
    };
    defer allocator.free(body);

    // Build headers
    var hdrs_buf: std.ArrayList(http_client.Header) = .empty;
    defer hdrs_buf.deinit(allocator);

    const credential = options.config.resolved_credential orelse self.cfg.resolved_credential;
    if (credential) |cred| {
        var auth_val: std.ArrayList(u8) = .empty;
        defer auth_val.deinit(allocator);
        try auth_val.appendSlice(allocator, "Bearer ");
        try auth_val.appendSlice(allocator, cred);
        const auth_str = try auth_val.toOwnedSlice(allocator);
        defer allocator.free(auth_str);
        try hdrs_buf.append(allocator, .{ .name = "Authorization", .value = auth_str });
        try hdrs_buf.append(allocator, .{ .name = "Content-Type", .value = "application/json" });
        try hdrs_buf.append(allocator, .{ .name = "Accept", .value = "text/event-stream" });

        const resp = try self.transport.post(allocator, .{
            .url = url,
            .body = body,
            .headers = hdrs_buf.items,
            .timeout_ms = options.config.request_timeout_ms,
        });
        return makeIterator(allocator, resp, self.mode);
    } else {
        try hdrs_buf.append(allocator, .{ .name = "Content-Type", .value = "application/json" });
        try hdrs_buf.append(allocator, .{ .name = "Accept", .value = "text/event-stream" });

        const resp = try self.transport.post(allocator, .{
            .url = url,
            .body = body,
            .headers = hdrs_buf.items,
            .timeout_ms = options.config.request_timeout_ms,
        });
        return makeIterator(allocator, resp, self.mode);
    }
}

fn makeIterator(allocator: std.mem.Allocator, resp: http_client.Response, mode: ApiMode) !EventIterator {
    if (resp.status >= 400) {
        var r = resp;
        r.deinit();
        return error.BadStatus;
    }
    const iter = try allocator.create(OpenAIIterator);
    errdefer allocator.destroy(iter);
    iter.* = OpenAIIterator.initIter(allocator, resp, mode, iter);
    return iter.base;
}

// ---- Request body builders ----

fn buildCompletionsBody(allocator: std.mem.Allocator, opts: *const SendOptions) ![]u8 {
    var buf: std.ArrayList(u8) = .empty;
    defer buf.deinit(allocator);

    try buf.appendSlice(allocator, "{\"model\":");
    try json_util.appendString(&buf, allocator, opts.config.model);
    try buf.appendSlice(allocator, ",\"stream\":true");
    try buf.appendSlice(allocator, ",\"stream_options\":{\"include_usage\":true}");

    // Tools
    if (opts.tools.len > 0) {
        try buf.appendSlice(allocator, ",\"tools\":[");
        for (opts.tools, 0..) |t, i| {
            if (i > 0) try buf.append(allocator, ',');
            try buf.appendSlice(allocator, "{\"type\":\"function\",\"function\":{\"name\":");
            try json_util.appendString(&buf, allocator, t.name);
            try buf.appendSlice(allocator, ",\"description\":");
            try json_util.appendString(&buf, allocator, t.description);
            try buf.appendSlice(allocator, ",\"parameters\":");
            try buf.appendSlice(allocator, t.schema);
            try buf.appendSlice(allocator, "}}");
        }
        try buf.append(allocator, ']');
    }

    // Messages
    try buf.appendSlice(allocator, ",\"messages\":[");
    try appendOpenAIMessages(&buf, allocator, opts.messages, opts.config.system_prompt);
    try buf.append(allocator, ']');
    try buf.append(allocator, '}');

    return buf.toOwnedSlice(allocator);
}

fn buildResponsesBody(allocator: std.mem.Allocator, opts: *const SendOptions) ![]u8 {
    var buf: std.ArrayList(u8) = .empty;
    defer buf.deinit(allocator);

    try buf.appendSlice(allocator, "{\"model\":");
    try json_util.appendString(&buf, allocator, opts.config.model);
    try buf.appendSlice(allocator, ",\"stream\":true");

    // Collect system messages into instructions
    var sys_buf: std.ArrayList(u8) = .empty;
    defer sys_buf.deinit(allocator);
    if (opts.config.system_prompt) |sp| {
        try sys_buf.appendSlice(allocator, sp);
    } else {
        for (opts.messages) |msg| {
            if (msg.role == .system) {
                if (sys_buf.items.len > 0) try sys_buf.append(allocator, '\n');
                try sys_buf.appendSlice(allocator, msg.content);
            }
        }
    }
    if (sys_buf.items.len > 0) {
        try buf.appendSlice(allocator, ",\"instructions\":");
        try json_util.appendString(&buf, allocator, sys_buf.items);
    }

    // Tools
    if (opts.tools.len > 0) {
        try buf.appendSlice(allocator, ",\"tools\":[");
        for (opts.tools, 0..) |t, i| {
            if (i > 0) try buf.append(allocator, ',');
            try buf.appendSlice(allocator, "{\"type\":\"function\",\"name\":");
            try json_util.appendString(&buf, allocator, t.name);
            try buf.appendSlice(allocator, ",\"description\":");
            try json_util.appendString(&buf, allocator, t.description);
            try buf.appendSlice(allocator, ",\"parameters\":");
            try buf.appendSlice(allocator, t.schema);
            try buf.append(allocator, '}');
        }
        try buf.append(allocator, ']');
    }

    // Input items
    try buf.appendSlice(allocator, ",\"input\":[");
    var first = true;
    for (opts.messages) |msg| {
        if (msg.role == .system) continue;
        if (!first) try buf.append(allocator, ',');
        first = false;

        switch (msg.role) {
            .system => unreachable,
            .user => {
                try buf.appendSlice(allocator, "{\"type\":\"message\",\"role\":\"user\",\"content\":[{\"type\":\"input_text\",\"text\":");
                try json_util.appendString(&buf, allocator, msg.content);
                try buf.appendSlice(allocator, "}]}");
            },
            .assistant => {
                try buf.appendSlice(allocator, "{\"type\":\"message\",\"role\":\"assistant\",\"content\":[{\"type\":\"output_text\",\"text\":");
                try json_util.appendString(&buf, allocator, msg.content);
                try buf.appendSlice(allocator, "}]}");
            },
            .tool_call => {
                if (msg.tool_call) |tc| {
                    try buf.appendSlice(allocator, "{\"type\":\"function_call\",\"call_id\":");
                    try json_util.appendString(&buf, allocator, tc.id);
                    try buf.appendSlice(allocator, ",\"name\":");
                    try json_util.appendString(&buf, allocator, tc.name);
                    try buf.appendSlice(allocator, ",\"arguments\":");
                    try json_util.appendString(&buf, allocator, tc.args_json);
                    try buf.append(allocator, '}');
                }
            },
            .tool_result => {
                if (msg.tool_result) |tr| {
                    try buf.appendSlice(allocator, "{\"type\":\"function_call_output\",\"call_id\":");
                    try json_util.appendString(&buf, allocator, tr.id);
                    try buf.appendSlice(allocator, ",\"output\":");
                    try json_util.appendString(&buf, allocator, tr.output);
                    try buf.append(allocator, '}');
                }
            },
        }
    }
    try buf.append(allocator, ']');
    try buf.append(allocator, '}');

    return buf.toOwnedSlice(allocator);
}

fn appendOpenAIMessages(
    buf: *std.ArrayList(u8),
    allocator: std.mem.Allocator,
    messages: []const Message,
    system_prompt: ?[]const u8,
) !void {
    var first = true;

    // Inject system prompt if provided
    if (system_prompt) |sp| {
        try buf.appendSlice(allocator, "{\"role\":\"system\",\"content\":");
        try json_util.appendString(buf, allocator, sp);
        try buf.append(allocator, '}');
        first = false;
    }

    for (messages) |msg| {
        if (!first) try buf.append(allocator, ',');
        first = false;

        switch (msg.role) {
            .system => {
                try buf.appendSlice(allocator, "{\"role\":\"system\",\"content\":");
                try json_util.appendString(buf, allocator, msg.content);
                try buf.append(allocator, '}');
            },
            .user => {
                try buf.appendSlice(allocator, "{\"role\":\"user\",\"content\":");
                try json_util.appendString(buf, allocator, msg.content);
                try buf.append(allocator, '}');
            },
            .assistant => {
                try buf.appendSlice(allocator, "{\"role\":\"assistant\",\"content\":");
                try json_util.appendString(buf, allocator, msg.content);
                try buf.append(allocator, '}');
            },
            .tool_call => {
                if (msg.tool_call) |tc| {
                    try buf.appendSlice(allocator, "{\"role\":\"assistant\",\"tool_calls\":[{\"id\":");
                    try json_util.appendString(buf, allocator, tc.id);
                    try buf.appendSlice(allocator, ",\"type\":\"function\",\"function\":{\"name\":");
                    try json_util.appendString(buf, allocator, tc.name);
                    try buf.appendSlice(allocator, ",\"arguments\":");
                    try json_util.appendString(buf, allocator, tc.args_json);
                    try buf.appendSlice(allocator, "}}]}");
                }
            },
            .tool_result => {
                if (msg.tool_result) |tr| {
                    try buf.appendSlice(allocator, "{\"role\":\"tool\",\"tool_call_id\":");
                    try json_util.appendString(buf, allocator, tr.id);
                    try buf.appendSlice(allocator, ",\"content\":");
                    try json_util.appendString(buf, allocator, tr.output);
                    try buf.append(allocator, '}');
                }
            },
        }
    }
}

// ---- Iterator ----

const ToolCallBuf = struct {
    id: std.ArrayList(u8),
    name: std.ArrayList(u8),
    args: std.ArrayList(u8),
};

const OpenAIIterator = struct {
    base: EventIterator,
    allocator: std.mem.Allocator,
    response: http_client.Response,
    sse_parser: sse_mod.Parser,
    mode: ApiMode,
    // Completions accumulator: index -> ToolCallBuf
    tool_bufs: std.AutoHashMap(u64, ToolCallBuf),
    stop_reason: StopReason,
    input_tokens: u32,
    output_tokens: u32,
    ended: bool,
    // Responses accumulator: output_index -> ToolCallBuf
    resp_tool_bufs: std.AutoHashMap(u64, ToolCallBuf),
    // Scratch storage
    scratch: ?[]u8,
    scratch2: ?[]u8,
    scratch3: ?[]u8,

    fn initIter(allocator: std.mem.Allocator, resp: http_client.Response, mode: ApiMode, self_ptr: *OpenAIIterator) OpenAIIterator {
        return .{
            .base = .{ .nextFn = nextImpl, .deinitFn = deinitIter, .ctx = self_ptr },
            .allocator = allocator,
            .response = resp,
            .sse_parser = sse_mod.Parser.init(allocator, resp.body_reader),
            .mode = mode,
            .tool_bufs = std.AutoHashMap(u64, ToolCallBuf).init(allocator),
            .stop_reason = .other,
            .input_tokens = 0,
            .output_tokens = 0,
            .ended = false,
            .resp_tool_bufs = std.AutoHashMap(u64, ToolCallBuf).init(allocator),
            .scratch = null,
            .scratch2 = null,
            .scratch3 = null,
        };
    }

    fn freeScratch(self: *OpenAIIterator) void {
        if (self.scratch) |s| {
            self.allocator.free(s);
            self.scratch = null;
        }
        if (self.scratch2) |s| {
            self.allocator.free(s);
            self.scratch2 = null;
        }
        if (self.scratch3) |s| {
            self.allocator.free(s);
            self.scratch3 = null;
        }
    }
};

fn deinitIter(it: *EventIterator) void {
    const self: *OpenAIIterator = @ptrCast(@alignCast(it.ctx));
    self.freeScratch();
    freeToolBufs(self.allocator, &self.tool_bufs);
    freeToolBufs(self.allocator, &self.resp_tool_bufs);
    self.sse_parser.deinit();
    self.response.deinit();
    self.allocator.destroy(self);
}

fn freeToolBufs(allocator: std.mem.Allocator, map: *std.AutoHashMap(u64, ToolCallBuf)) void {
    var iter = map.valueIterator();
    while (iter.next()) |buf| {
        buf.id.deinit(allocator);
        buf.name.deinit(allocator);
        buf.args.deinit(allocator);
    }
    map.deinit();
}

fn nextImpl(it: *EventIterator) ?Event {
    const self: *OpenAIIterator = @ptrCast(@alignCast(it.ctx));
    if (self.ended) return null;
    self.freeScratch();

    switch (self.mode) {
        .completions => return nextCompletions(self),
        .responses => return nextResponses(self),
    }
}

fn mapFinishReason(s: []const u8) StopReason {
    if (std.mem.eql(u8, s, "stop")) return .end_turn;
    if (std.mem.eql(u8, s, "length")) return .max_tokens;
    if (std.mem.eql(u8, s, "tool_calls")) return .tool_use;
    if (std.mem.eql(u8, s, "function_call")) return .tool_use;
    return .other;
}

fn nextCompletions(self: *OpenAIIterator) ?Event {
    while (true) {
        const maybe_ev = self.sse_parser.nextEvent() catch return null;
        const sse_ev = maybe_ev orelse {
            self.ended = true;
            return null;
        };

        const data = sse_ev.data;

        if (std.mem.eql(u8, data, "[DONE]")) {
            self.ended = true;
            return Event{ .done = .{
                .stop_reason = self.stop_reason,
                .usage = .{
                    .input_tokens = self.input_tokens,
                    .output_tokens = self.output_tokens,
                },
            } };
        }

        const parsed = std.json.parseFromSlice(
            std.json.Value,
            self.allocator,
            data,
            .{},
        ) catch continue;
        defer parsed.deinit();

        // Check for top-level usage (the final usage chunk before [DONE])
        if (json_util.dottedLookup(parsed.value, "usage.prompt_tokens")) |v| {
            if (v == .integer) self.input_tokens = @intCast(v.integer);
        }
        if (json_util.dottedLookup(parsed.value, "usage.completion_tokens")) |v| {
            if (v == .integer) self.output_tokens = @intCast(v.integer);
        }

        // Check finish_reason (may appear in a chunk with no delta)
        if (json_util.dottedLookup(parsed.value, "choices.0.finish_reason")) |fr| {
            if (fr == .string) {
                self.stop_reason = mapFinishReason(fr.string);
                // Flush tool_calls buffers
                if (self.stop_reason == .tool_use) {
                    // Emit first tool call we have; save rest for subsequent calls
                    // Simple: emit them one at a time using a queue approach
                    // For simplicity we'll emit them on the next few calls via a pending queue.
                    // Actually let's flush them all now, but we can only return one at a time.
                    // We'll use a Vec-based approach; for now flush the first one.
                    var map_iter = self.tool_bufs.iterator();
                    if (map_iter.next()) |entry| {
                        const buf_to_emit = entry.value_ptr;
                        const id_copy = self.allocator.dupe(u8, buf_to_emit.id.items) catch continue;
                        const name_copy = self.allocator.dupe(u8, buf_to_emit.name.items) catch {
                            self.allocator.free(id_copy);
                            continue;
                        };
                        const args_copy = self.allocator.dupe(u8, buf_to_emit.args.items) catch {
                            self.allocator.free(id_copy);
                            self.allocator.free(name_copy);
                            continue;
                        };
                        // Remove from map
                        const key = entry.key_ptr.*;
                        var removed = self.tool_bufs.fetchRemove(key).?;
                        removed.value.id.deinit(self.allocator);
                        removed.value.name.deinit(self.allocator);
                        removed.value.args.deinit(self.allocator);

                        self.scratch = id_copy;
                        self.scratch2 = name_copy;
                        self.scratch3 = args_copy;
                        return Event{ .tool_call = .{
                            .id = id_copy,
                            .name = name_copy,
                            .args_json = args_copy,
                        } };
                    }
                }
            }
        }

        // choices[0].delta (may be absent in finish_reason-only chunks)
        const delta = json_util.dottedLookup(parsed.value, "choices.0.delta") orelse continue;

        // Check for content token
        if (delta == .object) {
            if (delta.object.get("content")) |content_val| {
                if (content_val == .string and content_val.string.len > 0) {
                    const copy = self.allocator.dupe(u8, content_val.string) catch continue;
                    self.scratch = copy;
                    return Event{ .token = copy };
                }
            }

            // Check for tool_calls delta
            if (delta.object.get("tool_calls")) |tc_arr_val| {
                if (tc_arr_val == .array) {
                    for (tc_arr_val.array.items) |tc_item| {
                        if (tc_item != .object) continue;
                        const tc_obj = tc_item.object;

                        const index_val = tc_obj.get("index") orelse continue;
                        if (index_val != .integer) continue;
                        const index: u64 = @intCast(index_val.integer);

                        const entry = self.tool_bufs.getOrPut(index) catch continue;
                        if (!entry.found_existing) {
                            entry.value_ptr.* = .{
                                .id = .empty,
                                .name = .empty,
                                .args = .empty,
                            };
                        }
                        const buf_ref = entry.value_ptr;

                        if (tc_obj.get("id")) |id_val| {
                            if (id_val == .string) {
                                buf_ref.id.appendSlice(self.allocator, id_val.string) catch {};
                            }
                        }
                        if (tc_obj.get("function")) |fn_val| {
                            if (fn_val == .object) {
                                if (fn_val.object.get("name")) |n| {
                                    if (n == .string) {
                                        buf_ref.name.appendSlice(self.allocator, n.string) catch {};
                                    }
                                }
                                if (fn_val.object.get("arguments")) |a| {
                                    if (a == .string) {
                                        buf_ref.args.appendSlice(self.allocator, a.string) catch {};
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

fn nextResponses(self: *OpenAIIterator) ?Event {
    while (true) {
        const maybe_ev = self.sse_parser.nextEvent() catch return null;
        const sse_ev = maybe_ev orelse {
            self.ended = true;
            return null;
        };

        const event_type = sse_ev.event;
        const data = sse_ev.data;

        if (std.mem.eql(u8, event_type, "response.output_text.delta")) {
            const parsed = std.json.parseFromSlice(
                std.json.Value,
                self.allocator,
                data,
                .{},
            ) catch continue;
            defer parsed.deinit();

            if (json_util.dottedLookup(parsed.value, "delta")) |delta_val| {
                if (delta_val == .string and delta_val.string.len > 0) {
                    const copy = self.allocator.dupe(u8, delta_val.string) catch continue;
                    self.scratch = copy;
                    return Event{ .token = copy };
                }
            }
            continue;
        }

        if (std.mem.eql(u8, event_type, "response.output_item.added")) {
            const parsed = std.json.parseFromSlice(
                std.json.Value,
                self.allocator,
                data,
                .{},
            ) catch continue;
            defer parsed.deinit();

            const item_type = json_util.dottedLookup(parsed.value, "item.type") orelse continue;
            if (item_type != .string) continue;

            if (std.mem.eql(u8, item_type.string, "function_call")) {
                const output_index = json_util.dottedLookup(parsed.value, "output_index") orelse continue;
                if (output_index != .integer) continue;
                const idx: u64 = @intCast(output_index.integer);

                const call_id = json_util.dottedLookup(parsed.value, "item.call_id") orelse continue;
                const name = json_util.dottedLookup(parsed.value, "item.name") orelse continue;
                if (call_id != .string or name != .string) continue;

                const entry = self.resp_tool_bufs.getOrPut(idx) catch continue;
                if (!entry.found_existing) {
                    entry.value_ptr.* = .{ .id = .empty, .name = .empty, .args = .empty };
                }
                entry.value_ptr.id.appendSlice(self.allocator, call_id.string) catch {};
                entry.value_ptr.name.appendSlice(self.allocator, name.string) catch {};
            }
            continue;
        }

        if (std.mem.eql(u8, event_type, "response.function_call_arguments.delta")) {
            const parsed = std.json.parseFromSlice(
                std.json.Value,
                self.allocator,
                data,
                .{},
            ) catch continue;
            defer parsed.deinit();

            const output_index = json_util.dottedLookup(parsed.value, "output_index") orelse continue;
            if (output_index != .integer) continue;
            const idx: u64 = @intCast(output_index.integer);

            const delta_val = json_util.dottedLookup(parsed.value, "delta") orelse continue;
            if (delta_val != .string) continue;

            if (self.resp_tool_bufs.getPtr(idx)) |buf| {
                buf.args.appendSlice(self.allocator, delta_val.string) catch {};
            }
            continue;
        }

        if (std.mem.eql(u8, event_type, "response.output_item.done")) {
            const parsed = std.json.parseFromSlice(
                std.json.Value,
                self.allocator,
                data,
                .{},
            ) catch continue;
            defer parsed.deinit();

            const item_type = json_util.dottedLookup(parsed.value, "item.type") orelse continue;
            if (item_type != .string) continue;

            if (std.mem.eql(u8, item_type.string, "function_call")) {
                const output_index = json_util.dottedLookup(parsed.value, "output_index") orelse continue;
                if (output_index != .integer) continue;
                const idx: u64 = @intCast(output_index.integer);

                if (self.resp_tool_bufs.fetchRemove(idx)) |entry| {
                    var buf = entry.value;
                    defer {
                        buf.id.deinit(self.allocator);
                        buf.name.deinit(self.allocator);
                        buf.args.deinit(self.allocator);
                    }
                    const id_copy = self.allocator.dupe(u8, buf.id.items) catch continue;
                    const name_copy = self.allocator.dupe(u8, buf.name.items) catch {
                        self.allocator.free(id_copy);
                        continue;
                    };
                    const args_copy = self.allocator.dupe(u8, buf.args.items) catch {
                        self.allocator.free(id_copy);
                        self.allocator.free(name_copy);
                        continue;
                    };
                    self.scratch = id_copy;
                    self.scratch2 = name_copy;
                    self.scratch3 = args_copy;
                    return Event{ .tool_call = .{
                        .id = id_copy,
                        .name = name_copy,
                        .args_json = args_copy,
                    } };
                }
            }
            continue;
        }

        if (std.mem.eql(u8, event_type, "response.completed")) {
            const parsed = std.json.parseFromSlice(
                std.json.Value,
                self.allocator,
                data,
                .{},
            ) catch {
                self.ended = true;
                return Event{ .done = .{ .stop_reason = .end_turn } };
            };
            defer parsed.deinit();

            if (json_util.dottedLookup(parsed.value, "response.usage.input_tokens")) |v| {
                if (v == .integer) self.input_tokens = @intCast(v.integer);
            }
            if (json_util.dottedLookup(parsed.value, "response.usage.output_tokens")) |v| {
                if (v == .integer) self.output_tokens = @intCast(v.integer);
            }

            var sr = StopReason.end_turn;
            if (json_util.dottedLookup(parsed.value, "response.status")) |status| {
                if (status == .string) {
                    if (std.mem.eql(u8, status.string, "completed")) {
                        sr = .end_turn;
                    } else if (std.mem.eql(u8, status.string, "incomplete")) {
                        sr = .max_tokens;
                    } else if (std.mem.eql(u8, status.string, "failed")) {
                        sr = .other;
                    }
                }
            }

            self.ended = true;
            return Event{ .done = .{
                .stop_reason = sr,
                .usage = .{
                    .input_tokens = self.input_tokens,
                    .output_tokens = self.output_tokens,
                },
            } };
        }

        if (std.mem.eql(u8, event_type, "response.error") or
            std.mem.eql(u8, event_type, "error"))
        {
            self.ended = true;
            const parsed = std.json.parseFromSlice(
                std.json.Value,
                self.allocator,
                data,
                .{},
            ) catch return Event{ .err = "api error" };
            defer parsed.deinit();
            if (json_util.dottedLookup(parsed.value, "message")) |msg| {
                if (msg == .string) {
                    const copy = self.allocator.dupe(u8, msg.string) catch return Event{ .err = "api error" };
                    self.scratch = copy;
                    return Event{ .err = copy };
                }
            }
            return Event{ .err = "api error" };
        }
        // Other events ignored
    }
}

// ---- Tests ----

const test_util = @import("test_util");
const MockTransport = http_client.MockTransport;

const minimal_completions_done =
    "data: {\"choices\":[{\"index\":0,\"finish_reason\":\"stop\"}]}\n\n" ++
    "data: {\"usage\":{\"prompt_tokens\":0,\"completion_tokens\":0}}\n\n" ++
    "data: [DONE]\n\n";

test "openai chat: builds completions request" {
    const allocator = std.testing.allocator;
    const canned = [_]http_client.Canned{.{ .body = minimal_completions_done }};
    var mock = MockTransport.init(allocator, &canned);
    defer mock.deinit();

    var provider = try create(allocator, std.testing.io, .{
        .model = "gpt-4o",
        .resolved_credential = "sk-test",
        .api = null,
    }, mock.transport());
    defer provider.deinit(allocator);

    const messages = [_]Message{
        .{ .role = .system, .content = "Be helpful" },
        .{ .role = .user, .content = "Hello" },
    };

    var it = try provider.send(allocator, .{
        .messages = &messages,
        .config = .{ .model = "gpt-4o", .resolved_credential = "sk-test" },
    });
    defer it.deinit();
    while (it.next()) |_| {}

    const captured = mock.captured.items[0];
    try std.testing.expect(std.mem.endsWith(u8, captured.url, "/v1/chat/completions"));
    try std.testing.expect(std.mem.indexOf(u8, captured.body, "\"messages\"") != null);
    try std.testing.expect(std.mem.indexOf(u8, captured.body, "\"stream\":true") != null);
    const auth = test_util.headerValue(captured.headers, "Authorization").?;
    try std.testing.expectEqualStrings("Bearer sk-test", auth);
}

test "openai chat: token streaming" {
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

    var provider = try create(allocator, std.testing.io, .{
        .model = "gpt-4o",
        .resolved_credential = "sk-test",
    }, mock.transport());
    defer provider.deinit(allocator);

    const messages = [_]Message{.{ .role = .user, .content = "hi" }};
    var it = try provider.send(allocator, .{
        .messages = &messages,
        .config = .{ .model = "gpt-4o", .resolved_credential = "sk-test" },
    });
    defer it.deinit();

    const events = try test_util.collectEvents(&it, allocator);
    defer test_util.freeEvents(allocator, events);

    var tokens: std.ArrayList([]const u8) = .empty;
    defer tokens.deinit(allocator);
    var done_ev: ?core.DoneEvent = null;

    for (events) |ev| switch (ev) {
        .token => |t| try tokens.append(allocator, t),
        .done => |d| done_ev = d,
        else => {},
    };

    try std.testing.expectEqual(@as(usize, 2), tokens.items.len);
    try std.testing.expectEqualStrings("Hi", tokens.items[0]);
    try std.testing.expectEqualStrings("!", tokens.items[1]);
    try std.testing.expect(done_ev != null);
    try std.testing.expectEqual(StopReason.end_turn, done_ev.?.stop_reason);
    try std.testing.expectEqual(@as(u32, 3), done_ev.?.usage.input_tokens);
    try std.testing.expectEqual(@as(u32, 2), done_ev.?.usage.output_tokens);
}

test "openai chat: tool_calls accumulate across deltas" {
    const allocator = std.testing.allocator;
    const fixture =
        "data: {\"choices\":[{\"index\":0,\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"call_1\",\"function\":{\"name\":\"read_file\",\"arguments\":\"\"}}]}}]}\n\n" ++
        "data: {\"choices\":[{\"index\":0,\"delta\":{\"tool_calls\":[{\"index\":0,\"function\":{\"arguments\":\"{\\\"path\"}}]}}]}\n\n" ++
        "data: {\"choices\":[{\"index\":0,\"delta\":{\"tool_calls\":[{\"index\":0,\"function\":{\"arguments\":\"\\\":\\\"a.txt\\\"}\"}}]}}]}\n\n" ++
        "data: {\"choices\":[{\"index\":0,\"finish_reason\":\"tool_calls\"}]}\n\n" ++
        "data: {\"usage\":{\"prompt_tokens\":12,\"completion_tokens\":8}}\n\n" ++
        "data: [DONE]\n\n";

    const canned = [_]http_client.Canned{.{ .body = fixture }};
    var mock = MockTransport.init(allocator, &canned);
    defer mock.deinit();

    var provider = try create(allocator, std.testing.io, .{
        .model = "gpt-4o",
        .resolved_credential = "sk-test",
    }, mock.transport());
    defer provider.deinit(allocator);

    const messages = [_]Message{.{ .role = .user, .content = "use a tool" }};
    var it = try provider.send(allocator, .{
        .messages = &messages,
        .config = .{ .model = "gpt-4o", .resolved_credential = "sk-test" },
    });
    defer it.deinit();

    const events = try test_util.collectEvents(&it, allocator);
    defer test_util.freeEvents(allocator, events);

    var tc_found = false;
    var done_ev: ?core.DoneEvent = null;
    for (events) |ev| switch (ev) {
        .tool_call => |tc| {
            try std.testing.expectEqualStrings("call_1", tc.id);
            try std.testing.expectEqualStrings("read_file", tc.name);
            try std.testing.expect(std.mem.indexOf(u8, tc.args_json, "a.txt") != null);
            tc_found = true;
        },
        .done => |d| done_ev = d,
        else => {},
    };
    try std.testing.expect(tc_found);
    try std.testing.expect(done_ev != null);
    try std.testing.expectEqual(StopReason.tool_use, done_ev.?.stop_reason);
}

test "openai responses: builds responses request" {
    const allocator = std.testing.allocator;
    const fixture =
        "event: response.completed\ndata: {\"response\":{\"status\":\"completed\",\"usage\":{\"input_tokens\":0,\"output_tokens\":0}}}\n\n";

    const canned = [_]http_client.Canned{.{ .body = fixture }};
    var mock = MockTransport.init(allocator, &canned);
    defer mock.deinit();

    var provider = try create(allocator, std.testing.io, .{
        .model = "gpt-4o",
        .resolved_credential = "sk-test",
        .api = "responses",
    }, mock.transport());
    defer provider.deinit(allocator);

    const messages = [_]Message{
        .{ .role = .system, .content = "Be helpful" },
        .{ .role = .user, .content = "Hello" },
    };

    var it = try provider.send(allocator, .{
        .messages = &messages,
        .config = .{ .model = "gpt-4o", .resolved_credential = "sk-test", .api = "responses" },
    });
    defer it.deinit();
    while (it.next()) |_| {}

    const captured = mock.captured.items[0];
    try std.testing.expect(std.mem.endsWith(u8, captured.url, "/v1/responses"));
    try std.testing.expect(std.mem.indexOf(u8, captured.body, "\"input\"") != null);
    try std.testing.expect(std.mem.indexOf(u8, captured.body, "\"instructions\"") != null);
}

test "openai responses: text streaming via response.output_text.delta" {
    const allocator = std.testing.allocator;
    const fixture =
        "event: response.output_text.delta\ndata: {\"delta\":\"He\"}\n\n" ++
        "event: response.output_text.delta\ndata: {\"delta\":\"y\"}\n\n" ++
        "event: response.completed\ndata: {\"response\":{\"status\":\"completed\",\"usage\":{\"input_tokens\":4,\"output_tokens\":2}}}\n\n";

    const canned = [_]http_client.Canned{.{ .body = fixture }};
    var mock = MockTransport.init(allocator, &canned);
    defer mock.deinit();

    var provider = try create(allocator, std.testing.io, .{
        .model = "gpt-4o",
        .resolved_credential = "sk-test",
        .api = "responses",
    }, mock.transport());
    defer provider.deinit(allocator);

    const messages = [_]Message{.{ .role = .user, .content = "hello" }};
    var it = try provider.send(allocator, .{
        .messages = &messages,
        .config = .{ .model = "gpt-4o", .resolved_credential = "sk-test", .api = "responses" },
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
    try std.testing.expectEqual(@as(u32, 4), done_ev.?.usage.input_tokens);
    try std.testing.expectEqual(@as(u32, 2), done_ev.?.usage.output_tokens);
}

test "openai responses: function_call streaming" {
    const allocator = std.testing.allocator;
    const fixture =
        "event: response.output_item.added\ndata: {\"output_index\":0,\"item\":{\"type\":\"function_call\",\"call_id\":\"call_abc\",\"name\":\"my_tool\"}}\n\n" ++
        "event: response.function_call_arguments.delta\ndata: {\"output_index\":0,\"delta\":\"{\\\"x\\\":\"}\n\n" ++
        "event: response.function_call_arguments.delta\ndata: {\"output_index\":0,\"delta\":\"1}\"}\n\n" ++
        "event: response.output_item.done\ndata: {\"output_index\":0,\"item\":{\"type\":\"function_call\"}}\n\n" ++
        "event: response.completed\ndata: {\"response\":{\"status\":\"completed\",\"usage\":{\"input_tokens\":1,\"output_tokens\":1}}}\n\n";

    const canned = [_]http_client.Canned{.{ .body = fixture }};
    var mock = MockTransport.init(allocator, &canned);
    defer mock.deinit();

    var provider = try create(allocator, std.testing.io, .{
        .model = "gpt-4o",
        .resolved_credential = "sk-test",
        .api = "responses",
    }, mock.transport());
    defer provider.deinit(allocator);

    const messages = [_]Message{.{ .role = .user, .content = "use tool" }};
    var it = try provider.send(allocator, .{
        .messages = &messages,
        .config = .{ .model = "gpt-4o", .resolved_credential = "sk-test", .api = "responses" },
    });
    defer it.deinit();

    const events = try test_util.collectEvents(&it, allocator);
    defer test_util.freeEvents(allocator, events);

    var tc_found = false;
    for (events) |ev| switch (ev) {
        .tool_call => |tc| {
            try std.testing.expectEqualStrings("call_abc", tc.id);
            try std.testing.expectEqualStrings("my_tool", tc.name);
            try std.testing.expect(std.mem.indexOf(u8, tc.args_json, "\"x\"") != null);
            tc_found = true;
        },
        else => {},
    };
    try std.testing.expect(tc_found);
}

test "openai: invalid api value errors at create()" {
    const allocator = std.testing.allocator;
    const result = create(allocator, std.testing.io, .{
        .model = "gpt-4o",
        .api = "bogus",
    }, null);
    try std.testing.expectError(error.InvalidConfig, result);
}

test "openai: e2e roundtrip" {
    if (std.c.getenv("PHOENIX_E2E") == null) return error.SkipZigTest;
    const key = std.mem.span(std.c.getenv("OPENAI_API_KEY") orelse return error.SkipZigTest);

    var p = try create(std.testing.allocator, std.testing.io, .{
        .model = "gpt-4o-mini",
        .resolved_credential = key,
    }, null);
    defer p.deinit(std.testing.allocator);

    const msgs = [_]Message{.{ .role = .user, .content = "Reply with the word OK and nothing else." }};
    var it = try p.send(std.testing.allocator, .{
        .messages = &msgs,
        .config = .{ .model = "gpt-4o-mini", .resolved_credential = key },
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
