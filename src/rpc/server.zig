const std = @import("std");
const core = @import("phoenix_core");
const commands = @import("commands");
const tools_pkg = @import("phoenix_tools");
const protocol = @import("protocol.zig");

/// Cap on tool-use → re-send rounds within a single user turn. Defensive only:
/// the user always retains the cancel lever (DESIGN.md §11), but a bounded
/// loop prevents a model in a tight loop from burning tokens forever before
/// the human notices.
const max_tool_rounds: u32 = 16;

fn writeAll(fd: std.posix.fd_t, data: []const u8) !void {
    var remaining = data;
    while (remaining.len > 0) {
        const rc = std.c.write(fd, remaining.ptr, remaining.len);
        if (rc < 0) return error.WriteError;
        remaining = remaining[@intCast(rc)..];
    }
}

pub fn run(
    gpa: std.mem.Allocator,
    io: std.Io,
    config: *core.Config,
    home: ?[]const u8,
    stdin_fd: std.posix.fd_t,
    stdout_fd: std.posix.fd_t,
) !void {
    var line_buf: std.ArrayList(u8) = .empty;
    defer line_buf.deinit(gpa);

    // Single in-memory session for the life of the server process. v0
    // supports one implicit session; multi-session lands when session.create
    // is added.
    var session = core.Session.init(gpa, 1);
    defer session.deinit();

    var read_buf: [4096]u8 = undefined;

    while (true) {
        const n = try std.posix.read(stdin_fd, &read_buf);
        if (n == 0) {
            if (line_buf.items.len > 0) {
                const line = std.mem.trim(u8, line_buf.items, " \t\r");
                if (line.len > 0) {
                    try processLine(gpa, io, config, home, stdout_fd, &session, line);
                }
            }
            return;
        }

        const chunk = read_buf[0..n];
        var start: usize = 0;
        for (chunk, 0..) |byte, i| {
            if (byte == '\n') {
                try line_buf.appendSlice(gpa, chunk[start..i]);
                const line = std.mem.trim(u8, line_buf.items, " \t\r");
                if (line.len > 0) {
                    try processLine(gpa, io, config, home, stdout_fd, &session, line);
                }
                line_buf.clearRetainingCapacity();
                start = i + 1;
            }
        }
        if (start < chunk.len) {
            try line_buf.appendSlice(gpa, chunk[start..]);
        }
    }
}

fn processLine(
    gpa: std.mem.Allocator,
    io: std.Io,
    config: *core.Config,
    home: ?[]const u8,
    stdout_fd: std.posix.fd_t,
    session: *core.Session,
    line: []const u8,
) !void {
    var arena = std.heap.ArenaAllocator.init(gpa);
    defer arena.deinit();
    const a = arena.allocator();

    var resp: std.ArrayList(u8) = .empty;
    defer resp.deinit(gpa);

    const req = protocol.parseRequest(a, line) catch |err| {
        const code: protocol.ErrorCode = if (err == error.ParseError) .ParseError else .InvalidRequest;
        try protocol.writeError(&resp, gpa, 0, code, @errorName(err));
        try resp.appendSlice(gpa, "\n");
        try writeAll(stdout_fd, resp.items);
        return;
    };

    const ctx = commands.DispatchCtx{
        .io = io,
        .gpa = gpa,
        .home = home,
        .config = config,
    };

    if (std.mem.eql(u8, req.method, "config.get")) {
        try handleConfigGet(&resp, gpa, a, req.id, config);
    } else if (std.mem.eql(u8, req.method, "command.dispatch")) {
        try handleCommandDispatch(&resp, gpa, a, req.id, req.params, ctx);
    } else if (std.mem.eql(u8, req.method, "command.applyModelChoice")) {
        try handleApplyModelChoice(&resp, gpa, a, req.id, req.params, ctx);
    } else if (std.mem.eql(u8, req.method, "session.send")) {
        // session.send writes streaming bytes directly to stdout_fd; it does
        // not append to `resp`. Skip the post-handler flush.
        try handleSessionSend(gpa, a, req.id, req.params, ctx, session, stdout_fd);
        return;
    } else {
        const msg = try std.fmt.allocPrint(a, "no such method: {s}", .{req.method});
        try protocol.writeError(&resp, gpa, req.id, .MethodNotFound, msg);
    }

    try resp.appendSlice(gpa, "\n");
    try writeAll(stdout_fd, resp.items);
}

fn handleConfigGet(
    resp: *std.ArrayList(u8),
    gpa: std.mem.Allocator,
    a: std.mem.Allocator,
    id: i64,
    config: *core.Config,
) !void {
    var body: std.ArrayList(u8) = .empty;
    defer body.deinit(a);

    const dp = config.activeProvider();
    if (dp) |p| {
        try protocol.writeConfigGetResult(&body, a, .{ .kind = p.kind, .model = p.model }, config.sources.len);
    } else {
        try protocol.writeConfigGetResult(&body, a, null, config.sources.len);
    }
    try protocol.writeSuccess(resp, gpa, id, body.items);
}

fn handleCommandDispatch(
    resp: *std.ArrayList(u8),
    gpa: std.mem.Allocator,
    a: std.mem.Allocator,
    id: i64,
    params: std.json.Value,
    ctx: commands.DispatchCtx,
) !void {
    const p = protocol.parseDispatchParams(a, params) catch |err| {
        try protocol.writeError(resp, gpa, id, .InvalidParams, @errorName(err));
        return;
    };

    var outcome = commands.dispatch(ctx, p.input) catch |err| {
        try protocol.writeError(resp, gpa, id, .InternalError, @errorName(err));
        return;
    };
    defer outcome.deinit();

    var body: std.ArrayList(u8) = .empty;
    defer body.deinit(a);
    try protocol.writeDispatchResult(&body, a, outcome.result);
    try protocol.writeSuccess(resp, gpa, id, body.items);
}

fn handleApplyModelChoice(
    resp: *std.ArrayList(u8),
    gpa: std.mem.Allocator,
    a: std.mem.Allocator,
    id: i64,
    params: std.json.Value,
    ctx: commands.DispatchCtx,
) !void {
    const p = protocol.parseApplyModelChoiceParams(a, params) catch |err| {
        try protocol.writeError(resp, gpa, id, .InvalidParams, @errorName(err));
        return;
    };

    const choice = commands.ModelChoice{
        .provider_index = p.provider_index,
        .kind = p.kind,
        .model = p.model,
        .is_active = p.is_active,
    };

    var apply_arena = std.heap.ArenaAllocator.init(gpa);
    defer apply_arena.deinit();
    const msg = commands.applyModelChoice(ctx, choice, apply_arena.allocator()) catch |err| {
        try protocol.writeError(resp, gpa, id, .InternalError, @errorName(err));
        return;
    };

    const dp = ctx.config.activeProvider() orelse {
        try protocol.writeError(resp, gpa, id, .InternalError, "active provider missing after apply");
        return;
    };

    var body: std.ArrayList(u8) = .empty;
    defer body.deinit(a);
    try protocol.writeApplyModelChoiceResult(&body, a, msg, .{ .kind = dp.kind, .model = dp.model });
    try protocol.writeSuccess(resp, gpa, id, body.items);
}

/// session.send: classifies input via commands.dispatch, then either runs the
/// configured provider (streaming token events) or emits a terminal command
/// result. Skill activations (inject_context) emit a `context` event preamble
/// and, if the user supplied a follow-up message, run the conversation with
/// the skill body injected into the session as a system message.
fn handleSessionSend(
    gpa: std.mem.Allocator,
    a: std.mem.Allocator,
    id: i64,
    params: std.json.Value,
    ctx: commands.DispatchCtx,
    session: *core.Session,
    stdout_fd: std.posix.fd_t,
) !void {
    const p = protocol.parseSessionSendParams(a, params) catch |err| {
        try writeErrorEnvelope(gpa, stdout_fd, id, .InvalidParams, @errorName(err));
        return;
    };

    var outcome = commands.dispatch(ctx, p.text) catch |err| {
        try writeErrorEnvelope(gpa, stdout_fd, id, .InternalError, @errorName(err));
        return;
    };
    defer outcome.deinit();

    switch (outcome.result) {
        .not_a_command => {
            try runConversation(gpa, a, id, ctx, session, stdout_fd, p.text);
        },
        .inject_context => |frag| {
            // Always tell the TUI a skill loaded so it can render the system header.
            {
                var line: std.ArrayList(u8) = .empty;
                defer line.deinit(gpa);
                try protocol.writeEventContextLine(&line, gpa, id, frag.label, frag.body);
                try line.append(gpa, '\n');
                try writeAll(stdout_fd, line.items);
            }

            if (frag.user_message.len > 0) {
                // Inject skill body as a system turn so the model sees it on
                // this and subsequent turns. Then run the conversation with
                // the trailing user_message as the user turn.
                try session.addMessage(.system, frag.body);
                try runConversation(gpa, a, id, ctx, session, stdout_fd, frag.user_message);
            } else {
                // Skill loaded but no follow-up text — terminal command result.
                // The TUI already rendered the context event; the command
                // payload echoes the same fragment for redundancy/clarity.
                try writeCommandResult(gpa, stdout_fd, id, outcome.result);
            }
        },
        .message, .err, .model_picker => {
            try writeCommandResult(gpa, stdout_fd, id, outcome.result);
        },
    }
}

/// Pending tool invocation collected during one provider streaming pass.
/// All slices are heap-allocated (duped from event borrowed slices) so they
/// remain valid past the next `next()` call.
const PendingToolCall = struct {
    id: []u8,
    name: []u8,
    args_json: []u8,

    fn free(self: *PendingToolCall, allocator: std.mem.Allocator) void {
        allocator.free(self.id);
        allocator.free(self.name);
        allocator.free(self.args_json);
    }
};

/// Run a single conversational turn against the configured default provider,
/// streaming token events to stdout_fd and writing a terminal `conversation`
/// result line. On pre-stream failure (no provider, missing credential, send
/// error before any bytes), emits a single `error` envelope instead.
///
/// When the provider emits tool_call events with stop_reason == .tool_use,
/// each call is dispatched against the session's tool registry; the
/// tool_call and resulting tool_result are appended to the session history
/// and surfaced as RPC events, then the provider is re-invoked. The loop
/// repeats until the model stops on something other than tool_use, or the
/// `max_tool_rounds` ceiling fires.
fn runConversation(
    gpa: std.mem.Allocator,
    a: std.mem.Allocator,
    id: i64,
    ctx: commands.DispatchCtx,
    session: *core.Session,
    stdout_fd: std.posix.fd_t,
    user_text: []const u8,
) !void {
    const profile = ctx.config.activeProvider() orelse {
        try writeErrorEnvelope(gpa, stdout_fd, id, .InternalError, "no active provider configured");
        return;
    };

    try session.addMessage(.user, user_text);

    // v0: registry is the full built-in set. When session-name plumbing
    // exists, swap to tools_pkg.buildRegistry(gpa, active_session.tools).
    var registry = tools_pkg.buildRegistryAll(gpa) catch |err| {
        try writeErrorEnvelope(gpa, stdout_fd, id, .InternalError, @errorName(err));
        return;
    };
    defer registry.deinit();

    var tool_slice: std.ArrayList(*const core.Tool) = .empty;
    defer tool_slice.deinit(gpa);
    {
        var it_reg = registry.tools.valueIterator();
        while (it_reg.next()) |t_ptr| try tool_slice.append(gpa, t_ptr.*);
    }

    // Debug hack: PHOENIX_FORCE_EMPTY=1 short-circuits the provider drain so
    // every turn surfaces the "(empty response)" path. Useful for exercising
    // the no-token diagnostic UI without making real Anthropic calls.
    const force_empty = blk: {
        const v = std.c.getenv("PHOENIX_FORCE_EMPTY") orelse break :blk false;
        const s = std.mem.span(v);
        break :blk s.len > 0 and !std.mem.eql(u8, s, "0");
    };

    var stop_reason: core.StopReason = .other;
    var usage_total: core.Usage = .{};
    var last_err: ?[]const u8 = null;

    var round: u32 = 0;
    while (round < max_tool_rounds) : (round += 1) {
        // Provider lifecycle is per-send. createProvider is cheap (one alloc
        // plus credential dup) and keeps us free to swap providers across
        // turns when /model changes between sends without rebuilding state.
        const provider = core.createProvider(gpa, ctx.io, profile) catch |err| {
            try writeErrorEnvelope(gpa, stdout_fd, id, .InternalError, @errorName(err));
            return;
        };
        defer core.destroyProvider(gpa, provider);

        var it = provider.send(gpa, .{
            .messages = session.messages.items,
            .config = .{ .model = profile.model },
            .tools = tool_slice.items,
        }) catch |err| {
            try writeErrorEnvelope(gpa, stdout_fd, id, .InternalError, @errorName(err));
            return;
        };
        defer it.deinit();

        var assistant_buf: std.ArrayList(u8) = .empty;
        defer assistant_buf.deinit(gpa);

        var pending: std.ArrayList(PendingToolCall) = .empty;
        defer {
            for (pending.items) |*pc| pc.free(gpa);
            pending.deinit(gpa);
        }

        var event_count: u32 = 0;
        var round_stop: core.StopReason = .other;
        var round_usage: core.Usage = .{};

        while (!force_empty) {
            const maybe_ev = it.next();
            const ev = maybe_ev orelse break;
            event_count += 1;
            switch (ev) {
                .token => |t| {
                    try assistant_buf.appendSlice(gpa, t);
                    var line: std.ArrayList(u8) = .empty;
                    defer line.deinit(gpa);
                    try protocol.writeEventTokenLine(&line, gpa, id, t);
                    try line.append(gpa, '\n');
                    try writeAll(stdout_fd, line.items);
                },
                .err => |e| {
                    last_err = try a.dupe(u8, e);
                    var line: std.ArrayList(u8) = .empty;
                    defer line.deinit(gpa);
                    try protocol.writeEventErrLine(&line, gpa, id, e);
                    try line.append(gpa, '\n');
                    try writeAll(stdout_fd, line.items);
                },
                .done => |d| {
                    round_stop = d.stop_reason;
                    round_usage = d.usage;
                },
                .tool_call => |tc| {
                    // Slices borrowed from the iterator are invalidated on
                    // the next next() call — dupe before holding on.
                    const id_owned = try gpa.dupe(u8, tc.id);
                    errdefer gpa.free(id_owned);
                    const name_owned = try gpa.dupe(u8, tc.name);
                    errdefer gpa.free(name_owned);
                    const args_owned = try gpa.dupe(u8, tc.args_json);
                    errdefer gpa.free(args_owned);
                    try pending.append(gpa, .{
                        .id = id_owned,
                        .name = name_owned,
                        .args_json = args_owned,
                    });
                },
                .tool_result => {
                    // Providers don't synthesize these in normal operation;
                    // the harness creates them. Ignore if one shows up.
                },
            }
        }

        if (event_count == 0) {
            std.log.warn("session.send: provider iterator yielded zero events (HTTP body may have been empty or SSE parsed nothing)", .{});
        } else if (assistant_buf.items.len == 0 and last_err == null and pending.items.len == 0) {
            std.log.warn("session.send: {d} events but no tokens or tool calls; stop_reason={s}, in={d}, out={d}", .{
                event_count,
                @tagName(round_stop),
                round_usage.input_tokens,
                round_usage.output_tokens,
            });
        }

        // Carry usage forward (cumulative across tool-use rounds).
        usage_total.input_tokens +%= round_usage.input_tokens;
        usage_total.output_tokens +%= round_usage.output_tokens;
        stop_reason = round_stop;

        if (last_err != null) break;

        // Persist assistant text (if any) before any associated tool_calls,
        // so providers like Claude can coalesce them into one assistant turn.
        if (assistant_buf.items.len > 0) {
            try session.addMessage(.assistant, assistant_buf.items);
        }

        if (pending.items.len == 0) {
            // Plain assistant turn — done.
            break;
        }

        // Tool-use round: persist + invoke + re-send.
        for (pending.items) |pc| {
            try session.addToolCall(.{
                .id = pc.id,
                .name = pc.name,
                .args_json = pc.args_json,
            });

            var call_line: std.ArrayList(u8) = .empty;
            defer call_line.deinit(gpa);
            try protocol.writeEventToolCallLine(&call_line, gpa, id, pc.id, pc.name, pc.args_json);
            try call_line.append(gpa, '\n');
            try writeAll(stdout_fd, call_line.items);

            const result = invokeOne(gpa, ctx.io, &registry, pc.name, pc.args_json) catch |err| blk: {
                const msg = try std.fmt.allocPrint(gpa, "tool dispatch error: {s}", .{@errorName(err)});
                break :blk core.tool.ToolResult{ .output = msg, .truncated = false, .is_error = true };
            };
            defer gpa.free(result.output);

            try session.addToolResult(.{
                .id = pc.id,
                .output = result.output,
                .is_error = result.is_error,
            });

            var res_line: std.ArrayList(u8) = .empty;
            defer res_line.deinit(gpa);
            try protocol.writeEventToolResultLine(&res_line, gpa, id, pc.id, result.output, result.is_error);
            try res_line.append(gpa, '\n');
            try writeAll(stdout_fd, res_line.items);
        }
        // Loop back for another provider.send with the new tool_result messages.
    }

    if (round >= max_tool_rounds and last_err == null) {
        const msg = try std.fmt.allocPrint(a, "tool-use loop exceeded {d} rounds", .{max_tool_rounds});
        last_err = msg;
        var line: std.ArrayList(u8) = .empty;
        defer line.deinit(gpa);
        try protocol.writeEventErrLine(&line, gpa, id, msg);
        try line.append(gpa, '\n');
        try writeAll(stdout_fd, line.items);
    }

    // Terminal result line.
    var body: std.ArrayList(u8) = .empty;
    defer body.deinit(gpa);
    if (last_err) |reason| {
        try protocol.writeSendConversationErrBody(&body, gpa, reason);
    } else {
        try protocol.writeSendConversationOkBody(&body, gpa, stop_reason, usage_total.input_tokens, usage_total.output_tokens);
    }
    var resp_line: std.ArrayList(u8) = .empty;
    defer resp_line.deinit(gpa);
    try protocol.writeSuccess(&resp_line, gpa, id, body.items);
    try resp_line.append(gpa, '\n');
    try writeAll(stdout_fd, resp_line.items);
}

/// Look up a tool by name in the registry and invoke it. If the tool is
/// unknown, returns a synthesized error result rather than failing the loop —
/// the model can recover by adjusting its next call.
fn invokeOne(
    gpa: std.mem.Allocator,
    io: std.Io,
    registry: *const core.ToolRegistry,
    name: []const u8,
    args_json: []const u8,
) !core.tool.ToolResult {
    if (registry.get(name)) |tool| {
        var result = try tool.invoke(io, args_json, gpa);
        if (!result.truncated and result.output.len > tool.max_output_bytes) {
            const head = result.output[0..tool.max_output_bytes];
            const replacement = try std.fmt.allocPrint(
                gpa,
                "{s}\n[truncated at {d} bytes]\n",
                .{ head, tool.max_output_bytes },
            );
            gpa.free(result.output);
            result = .{ .output = replacement, .truncated = true, .is_error = result.is_error };
        }
        return result;
    }
    const msg = try std.fmt.allocPrint(gpa, "unknown tool: {s}", .{name});
    return .{ .output = msg, .truncated = false, .is_error = true };
}

/// Emit a terminal session.send `command` result wrapping a dispatch payload.
fn writeCommandResult(
    gpa: std.mem.Allocator,
    stdout_fd: std.posix.fd_t,
    id: i64,
    r: commands.Result,
) !void {
    var body: std.ArrayList(u8) = .empty;
    defer body.deinit(gpa);
    try protocol.writeSendCommandBody(&body, gpa, r);
    var line: std.ArrayList(u8) = .empty;
    defer line.deinit(gpa);
    try protocol.writeSuccess(&line, gpa, id, body.items);
    try line.append(gpa, '\n');
    try writeAll(stdout_fd, line.items);
}

/// Emit an `error` envelope (pre-stream failure) directly to stdout_fd.
fn writeErrorEnvelope(
    gpa: std.mem.Allocator,
    stdout_fd: std.posix.fd_t,
    id: i64,
    code: protocol.ErrorCode,
    message: []const u8,
) !void {
    var line: std.ArrayList(u8) = .empty;
    defer line.deinit(gpa);
    try protocol.writeError(&line, gpa, id, code, message);
    try line.append(gpa, '\n');
    try writeAll(stdout_fd, line.items);
}

test "handleConfigGet returns valid JSON" {
    const a = std.testing.allocator;
    var cfg = try core.Config.load(a, std.testing.io, .{ .home = null });
    defer cfg.deinit();

    var arena = std.heap.ArenaAllocator.init(a);
    defer arena.deinit();

    var resp: std.ArrayList(u8) = .empty;
    defer resp.deinit(a);

    try handleConfigGet(&resp, a, arena.allocator(), 1, &cfg);

    try std.testing.expect(std.mem.indexOf(u8, resp.items, "\"id\":1") != null);
    try std.testing.expect(std.mem.indexOf(u8, resp.items, "\"result\"") != null);
    try std.testing.expect(std.mem.indexOf(u8, resp.items, "default_provider") != null);
}

test "handleCommandDispatch not_a_command" {
    const a = std.testing.allocator;
    var cfg = try core.Config.load(a, std.testing.io, .{ .home = null });
    defer cfg.deinit();

    var arena = std.heap.ArenaAllocator.init(a);
    defer arena.deinit();

    var resp: std.ArrayList(u8) = .empty;
    defer resp.deinit(a);

    var params_arena = std.heap.ArenaAllocator.init(a);
    defer params_arena.deinit();
    const params = try std.json.parseFromSliceLeaky(std.json.Value, params_arena.allocator(), "{\"input\":\"hello world\"}", .{});

    const ctx = commands.DispatchCtx{
        .io = std.testing.io,
        .gpa = a,
        .home = null,
        .config = &cfg,
    };
    try handleCommandDispatch(&resp, a, arena.allocator(), 2, params, ctx);

    try std.testing.expect(std.mem.indexOf(u8, resp.items, "\"id\":2") != null);
    try std.testing.expect(std.mem.indexOf(u8, resp.items, "not_a_command") != null);
}

test "handleCommandDispatch unknown command returns err" {
    const a = std.testing.allocator;
    var cfg = try core.Config.load(a, std.testing.io, .{ .home = null });
    defer cfg.deinit();

    var arena = std.heap.ArenaAllocator.init(a);
    defer arena.deinit();

    var resp: std.ArrayList(u8) = .empty;
    defer resp.deinit(a);

    var params_arena = std.heap.ArenaAllocator.init(a);
    defer params_arena.deinit();
    const params = try std.json.parseFromSliceLeaky(std.json.Value, params_arena.allocator(), "{\"input\":\"/unknown\"}", .{});

    const ctx = commands.DispatchCtx{
        .io = std.testing.io,
        .gpa = a,
        .home = null,
        .config = &cfg,
    };
    try handleCommandDispatch(&resp, a, arena.allocator(), 3, params, ctx);

    try std.testing.expect(std.mem.indexOf(u8, resp.items, "\"id\":3") != null);
    try std.testing.expect(std.mem.indexOf(u8, resp.items, "\"kind\":\"err\"") != null);
}

test "handleSessionSend command path emits one terminal line, no events" {
    const a = std.testing.allocator;
    var cfg = try core.Config.load(a, std.testing.io, .{ .home = null });
    defer cfg.deinit();

    var session = core.Session.init(a, 1);
    defer session.deinit();

    var arena = std.heap.ArenaAllocator.init(a);
    defer arena.deinit();

    var params_arena = std.heap.ArenaAllocator.init(a);
    defer params_arena.deinit();
    const params = try std.json.parseFromSliceLeaky(
        std.json.Value,
        params_arena.allocator(),
        "{\"text\":\"/unknown\"}",
        .{},
    );

    const ctx = commands.DispatchCtx{
        .io = std.testing.io,
        .gpa = a,
        .home = null,
        .config = &cfg,
    };

    var fds: [2]c_int = undefined;
    if (std.c.pipe(&fds) != 0) return error.PipeFailed;
    const read_fd: std.posix.fd_t = fds[0];
    var write_fd: std.posix.fd_t = fds[1];
    // Reader stays open until we drain. Writer is closed after the handler
    // returns so the read sees EOF.
    defer _ = std.c.close(read_fd);

    try handleSessionSend(a, arena.allocator(), 7, params, ctx, &session, write_fd);
    _ = std.c.close(write_fd);
    write_fd = -1;

    var sink: std.ArrayList(u8) = .empty;
    defer sink.deinit(a);
    var buf: [1024]u8 = undefined;
    while (true) {
        const n = std.c.read(read_fd, &buf, buf.len);
        if (n <= 0) break;
        try sink.appendSlice(a, buf[0..@intCast(n)]);
    }

    // Exactly one line.
    var newlines: usize = 0;
    for (sink.items) |c| if (c == '\n') {
        newlines += 1;
    };
    try std.testing.expectEqual(@as(usize, 1), newlines);

    try std.testing.expect(std.mem.indexOf(u8, sink.items, "\"id\":7") != null);
    try std.testing.expect(std.mem.indexOf(u8, sink.items, "\"kind\":\"command\"") != null);
    try std.testing.expect(std.mem.indexOf(u8, sink.items, "\"kind\":\"err\"") != null);
    // Conversation history must NOT be touched on a command-only path.
    try std.testing.expectEqual(@as(usize, 0), session.messages.items.len);
}

test "unknown method returns MethodNotFound" {
    const a = std.testing.allocator;
    var cfg = try core.Config.load(a, std.testing.io, .{ .home = null });
    defer cfg.deinit();

    var req_arena = std.heap.ArenaAllocator.init(a);
    defer req_arena.deinit();
    const req = try protocol.parseRequest(req_arena.allocator(), "{\"id\":9,\"method\":\"no.such.method\"}");

    var arena = std.heap.ArenaAllocator.init(a);
    defer arena.deinit();

    var resp: std.ArrayList(u8) = .empty;
    defer resp.deinit(a);

    const msg = try std.fmt.allocPrint(arena.allocator(), "no such method: {s}", .{req.method});
    try protocol.writeError(&resp, a, req.id, .MethodNotFound, msg);

    try std.testing.expect(std.mem.indexOf(u8, resp.items, "MethodNotFound") != null);
    try std.testing.expect(std.mem.indexOf(u8, resp.items, "\"id\":9") != null);
}
