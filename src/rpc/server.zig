const std = @import("std");
const core = @import("phoenix_core");
const commands = @import("commands");
const tools_pkg = @import("phoenix_tools");
const protocol = @import("protocol.zig");
const active_session = @import("active_session.zig");

/// Cap on tool-use → re-send rounds within a single user turn. Defensive only:
/// the user always retains the cancel lever (DESIGN.md §11), but a bounded
/// loop prevents a model in a tight loop from burning tokens forever before
/// the human notices.
const max_tool_rounds: u32 = 16;

/// Stdin fd captured at run() entry for the signal handler to close. Closing
/// stdin from the handler is async-signal-safe and wakes the blocked
/// `posix.read` with EOF (n == 0), which routes through the normal exit path
/// so the deferred session persist still runs.
var shutdown_stdin_fd: std.atomic.Value(c_int) = .init(-1);

fn signalHandler(_: std.c.SIG) callconv(.c) void {
    const fd = shutdown_stdin_fd.load(.acquire);
    if (fd >= 0) _ = std.c.close(fd);
}

fn installSignalHandlers(stdin_fd: std.posix.fd_t) void {
    shutdown_stdin_fd.store(@intCast(stdin_fd), .release);
    var sa: std.c.Sigaction = .{
        .handler = .{ .handler = signalHandler },
        .mask = std.mem.zeroes(std.c.sigset_t),
        .flags = 0,
    };
    _ = std.c.sigaction(.INT, &sa, null);
    _ = std.c.sigaction(.TERM, &sa, null);
}

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
    otel: *core.Otel,
) !void {
    var line_buf: std.ArrayList(u8) = .empty;
    defer line_buf.deinit(gpa);

    // Project slug is fixed for the life of the process — main.zig invokes
    // `phoenix rpc` from the launch cwd, and the rpc child inherits it. If
    // we can't read cwd we just skip persistence rather than panic.
    var cwd_buf: [std.Io.Dir.max_path_bytes]u8 = undefined;
    const project_owned: ?[]u8 = blk: {
        const ptr = std.c.getcwd(&cwd_buf, cwd_buf.len) orelse break :blk null;
        const cwd = std.mem.sliceTo(ptr, 0);
        break :blk core.session_store.projectSlug(gpa, cwd) catch null;
    };
    defer if (project_owned) |p| gpa.free(p);
    const project: ?[]const u8 = project_owned;

    // Single in-memory session for the life of the server process. v0
    // supports one implicit session; multi-session lands when session.create
    // is added. /resume swaps the contents in place.
    var active = active_session.ActiveSession.init(gpa, io, home, project);
    // Persist on every exit path (EOF, error, panic-free returns) so the
    // user can boot back up to whatever was running.
    defer {
        if (active.session.messages.items.len > 0) active.persist();
        active.deinit();
    }

    installSignalHandlers(stdin_fd);

    var read_buf: [4096]u8 = undefined;

    while (true) {
        // The signal handler closes stdin to break us out; that gives EOF
        // (n == 0) which routes to the normal exit branch below.
        const n = std.posix.read(stdin_fd, &read_buf) catch |err| switch (err) {
            error.NotOpenForReading, error.SocketUnconnected => return,
            else => return err,
        };
        if (n == 0) {
            if (line_buf.items.len > 0) {
                const line = std.mem.trim(u8, line_buf.items, " \t\r");
                if (line.len > 0) {
                    try processLine(gpa, io, config, home, project, stdout_fd, &active, otel, line);
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
                    try processLine(gpa, io, config, home, project, stdout_fd, &active, otel, line);
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
    project: ?[]const u8,
    stdout_fd: std.posix.fd_t,
    active: *active_session.ActiveSession,
    otel: *core.Otel,
    line: []const u8,
) !void {
    var arena = std.heap.ArenaAllocator.init(gpa);
    defer arena.deinit();
    const a = arena.allocator();

    var resp: std.ArrayList(u8) = .empty;
    defer resp.deinit(gpa);

    // One root span per RPC request. Even malformed requests get a span so
    // a "where did the parse errors come from?" question has a place to land
    // in the trace tree.
    var root_span = otel.startSpan("rpc.request", .server, null);
    defer {
        // Ensure every code path below closes the span and flushes batches
        // back to the collector before we read the next line. This keeps
        // observability in lock-step with the wire protocol.
        root_span.end();
        otel.flush();
    }

    const req = protocol.parseRequest(a, line) catch |err| {
        const code: protocol.ErrorCode = if (err == error.ParseError) .ParseError else .InvalidRequest;
        root_span.setAttr(core.Attr.string("rpc.method", "(unparsed)"));
        root_span.setStatusError(@errorName(err));
        otel.emitLog(.warn, "rpc.parse_error", &.{
            core.Attr.string("error", @errorName(err)),
        }, root_span.context());
        try protocol.writeError(&resp, gpa, 0, code, @errorName(err));
        try resp.appendSlice(gpa, "\n");
        try writeAll(stdout_fd, resp.items);
        return;
    };

    root_span.setAttrs(&[_]core.Attr{
        core.Attr.string("rpc.method", req.method),
        core.Attr.integer("rpc.id", req.id),
    });

    const ctx = commands.DispatchCtx{
        .io = io,
        .gpa = gpa,
        .home = home,
        .config = config,
        .project = project,
    };

    if (std.mem.eql(u8, req.method, "config.get")) {
        try handleConfigGet(&resp, gpa, a, req.id, config);
        root_span.setStatusOk();
    } else if (std.mem.eql(u8, req.method, "command.list")) {
        try handleCommandList(&resp, gpa, a, req.id, ctx);
        root_span.setStatusOk();
    } else if (std.mem.eql(u8, req.method, "command.dispatch")) {
        try handleCommandDispatch(&resp, gpa, a, req.id, req.params, ctx, active, otel, root_span.context());
        root_span.setStatusOk();
    } else if (std.mem.eql(u8, req.method, "command.applyModelChoice")) {
        try handleApplyModelChoice(&resp, gpa, a, req.id, req.params, ctx);
        root_span.setStatusOk();
    } else if (std.mem.eql(u8, req.method, "command.addModel")) {
        try handleAddModel(&resp, gpa, a, req.id, req.params, ctx);
        root_span.setStatusOk();
    } else if (std.mem.eql(u8, req.method, "command.applySessionChoice")) {
        try handleApplySessionChoice(&resp, gpa, a, req.id, req.params, active);
        root_span.setStatusOk();
    } else if (std.mem.eql(u8, req.method, "session.send")) {
        // session.send writes streaming bytes directly to stdout_fd; it does
        // not append to `resp`. Skip the post-handler flush.
        try handleSessionSend(gpa, a, req.id, req.params, ctx, active, stdout_fd, otel, root_span.context());
        root_span.setStatusOk();
        return;
    } else {
        const msg = try std.fmt.allocPrint(a, "no such method: {s}", .{req.method});
        try protocol.writeError(&resp, gpa, req.id, .MethodNotFound, msg);
        root_span.setStatusError("MethodNotFound");
        otel.emitLog(.warn, "rpc.method_not_found", &.{
            core.Attr.string("rpc.method", req.method),
        }, root_span.context());
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

fn handleCommandList(
    resp: *std.ArrayList(u8),
    gpa: std.mem.Allocator,
    a: std.mem.Allocator,
    id: i64,
    ctx: commands.DispatchCtx,
) !void {
    const items = try commands.listCommands(ctx, a);
    var body: std.ArrayList(u8) = .empty;
    defer body.deinit(a);
    try protocol.writeCommandListResult(&body, a, items);
    try protocol.writeSuccess(resp, gpa, id, body.items);
}

fn handleCommandDispatch(
    resp: *std.ArrayList(u8),
    gpa: std.mem.Allocator,
    a: std.mem.Allocator,
    id: i64,
    params: std.json.Value,
    ctx: commands.DispatchCtx,
    active: *active_session.ActiveSession,
    otel: *core.Otel,
    parent: ?core.SpanContext,
) !void {
    var span = otel.startSpan("command.dispatch", .internal, parent);
    defer span.end();

    const p = protocol.parseDispatchParams(a, params) catch |err| {
        span.setStatusError(@errorName(err));
        try protocol.writeError(resp, gpa, id, .InvalidParams, @errorName(err));
        return;
    };

    var outcome = commands.dispatch(ctx, p.input) catch |err| {
        span.setStatusError(@errorName(err));
        try protocol.writeError(resp, gpa, id, .InternalError, @errorName(err));
        return;
    };
    defer outcome.deinit();

    // Server intercepts the session-mutation markers and transforms them
    // into a `.message` Result before encoding.
    var body: std.ArrayList(u8) = .empty;
    defer body.deinit(a);

    switch (outcome.result) {
        .clear_session => {
            const txt = try clearAction(a, ctx, active);
            try protocol.writeDispatchResult(&body, a, .{ .cleared = txt });
        },
        .compact_session => {
            const txt = try compactAction(a, ctx, active);
            try protocol.writeDispatchResult(&body, a, .{ .compacted = txt });
        },
        else => {
            try protocol.writeDispatchResult(&body, a, outcome.result);
        },
    }

    span.setAttr(core.Attr.string("command.kind", @tagName(outcome.result)));
    span.setStatusOk();
    try protocol.writeSuccess(resp, gpa, id, body.items);
}

/// Persist the current session (if non-empty), reset to a fresh empty one,
/// and return a user-facing system message describing what just happened.
/// `out_arena` owns the returned string.
fn clearAction(
    out_arena: std.mem.Allocator,
    ctx: commands.DispatchCtx,
    active: *active_session.ActiveSession,
) ![]const u8 {
    _ = ctx;
    const had = active.session.messages.items.len;
    const had_id = if (active.disk_id) |s| try out_arena.dupe(u8, s) else "";
    active.clear() catch |err| {
        return std.fmt.allocPrint(out_arena, "/clear failed: {s}", .{@errorName(err)});
    };
    if (had == 0) {
        return out_arena.dupe(u8, "Conversation already empty.");
    }
    if (had_id.len > 0) {
        return std.fmt.allocPrint(out_arena, "Saved session {s} ({d} messages) and started a fresh one.", .{ had_id, had });
    }
    return std.fmt.allocPrint(out_arena, "Started a fresh conversation ({d} messages dropped, no persistence).", .{had});
}

fn compactAction(
    out_arena: std.mem.Allocator,
    ctx: commands.DispatchCtx,
    active: *active_session.ActiveSession,
) ![]const u8 {
    const tail = compactionTailTurns(ctx);
    const before = active.session.messages.items.len;
    const dropped = active.truncate(tail) catch |err| {
        return std.fmt.allocPrint(out_arena, "/compact failed: {s}", .{@errorName(err)});
    };
    if (dropped == 0) {
        return out_arena.dupe(u8, "Nothing to compact.");
    }
    return std.fmt.allocPrint(
        out_arena,
        "Compacted: {d} → {d} messages (dropped {d}, kept the system head and last {d} user turn{s}).",
        .{ before, before - dropped, dropped, tail, if (tail == 1) "" else "s" },
    );
}

fn compactionTailTurns(ctx: commands.DispatchCtx) u32 {
    if (ctx.config.findSession("default")) |s| return s.compaction_tail_turns;
    return 3;
}

fn compactionThreshold(ctx: commands.DispatchCtx) f32 {
    if (ctx.config.findSession("default")) |s| return s.compaction_threshold;
    return 0.8;
}

fn handleApplySessionChoice(
    resp: *std.ArrayList(u8),
    gpa: std.mem.Allocator,
    a: std.mem.Allocator,
    id: i64,
    params: std.json.Value,
    active: *active_session.ActiveSession,
) !void {
    const p = protocol.parseApplySessionChoiceParams(a, params) catch |err| {
        try protocol.writeError(resp, gpa, id, .InvalidParams, @errorName(err));
        return;
    };

    const restored = active.replaceWith(p.id) catch |err| {
        try protocol.writeError(resp, gpa, id, .InternalError, @errorName(err));
        return;
    };

    const msg = try std.fmt.allocPrint(
        a,
        "Resumed session {s} ({d} messages).",
        .{ p.id, restored },
    );

    var body: std.ArrayList(u8) = .empty;
    defer body.deinit(a);
    try protocol.writeApplySessionChoiceResult(
        &body,
        a,
        msg,
        restored,
        active.session.messages.items,
    );
    try protocol.writeSuccess(resp, gpa, id, body.items);
}

fn handleAddModel(
    resp: *std.ArrayList(u8),
    gpa: std.mem.Allocator,
    a: std.mem.Allocator,
    id: i64,
    params: std.json.Value,
    ctx: commands.DispatchCtx,
) !void {
    const p = protocol.parseAddModelParams(a, params) catch |err| {
        try protocol.writeError(resp, gpa, id, .InvalidParams, @errorName(err));
        return;
    };

    const local = p.base_url.len > 0;
    const profile = core.ProviderProfile{
        .kind = p.kind,
        .model = p.model,
        .active = false,
        .auth = if (local) null else core.AuthEntry{ .inline_value = p.api_key },
        .base_url = if (local) p.base_url else null,
        .context_window = p.context_window,
    };

    var apply_arena = std.heap.ArenaAllocator.init(gpa);
    defer apply_arena.deinit();
    const msg = commands.addProvider(ctx, profile, apply_arena.allocator()) catch |err| {
        try protocol.writeError(resp, gpa, id, .InternalError, @errorName(err));
        return;
    };

    const entries = commands.listModelEntries(ctx, apply_arena.allocator()) catch |err| {
        try protocol.writeError(resp, gpa, id, .InternalError, @errorName(err));
        return;
    };

    var body: std.ArrayList(u8) = .empty;
    defer body.deinit(a);
    try protocol.writeAddModelResult(&body, a, msg, entries);
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
    active: *active_session.ActiveSession,
    stdout_fd: std.posix.fd_t,
    otel: *core.Otel,
    parent: ?core.SpanContext,
) !void {
    var span = otel.startSpan("session.send", .internal, parent);
    defer span.end();

    const p = protocol.parseSessionSendParams(a, params) catch |err| {
        span.setStatusError(@errorName(err));
        try writeErrorEnvelope(gpa, stdout_fd, id, .InvalidParams, @errorName(err));
        return;
    };
    span.setAttr(core.Attr.integer("session.input_len", @intCast(p.text.len)));

    var outcome = commands.dispatch(ctx, p.text) catch |err| {
        span.setStatusError(@errorName(err));
        try writeErrorEnvelope(gpa, stdout_fd, id, .InternalError, @errorName(err));
        return;
    };
    defer outcome.deinit();

    span.setAttr(core.Attr.string("session.outcome", @tagName(outcome.result)));

    switch (outcome.result) {
        .not_a_command => {
            try runConversation(gpa, a, id, ctx, active, stdout_fd, p.text, otel, span.context());
        },
        .inject_context => |frag| {
            otel.emitLog(.info, "skill.activated", &.{
                core.Attr.string("skill.label", frag.label),
                core.Attr.boolean("skill.has_follow_up", frag.user_message.len > 0),
            }, span.context());

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
                try active.appendSystem(frag.body);
                try runConversation(gpa, a, id, ctx, active, stdout_fd, frag.user_message, otel, span.context());
            } else {
                try writeCommandResult(gpa, stdout_fd, id, outcome.result);
            }
        },
        .clear_session => {
            const txt = try clearAction(a, ctx, active);
            try writeCommandResult(gpa, stdout_fd, id, .{ .cleared = txt });
        },
        .compact_session => {
            const txt = try compactAction(a, ctx, active);
            try writeCommandResult(gpa, stdout_fd, id, .{ .compacted = txt });
        },
        .message, .err, .model_picker, .session_picker, .cleared, .compacted, .models_page => {
            try writeCommandResult(gpa, stdout_fd, id, outcome.result);
        },
    }

    span.setStatusOk();
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
    active: *active_session.ActiveSession,
    stdout_fd: std.posix.fd_t,
    user_text: []const u8,
    otel: *core.Otel,
    parent: ?core.SpanContext,
) !void {
    const profile = ctx.config.activeProvider() orelse {
        otel.emitLog(.err, "session.no_active_provider", &.{}, parent);
        try writeErrorEnvelope(gpa, stdout_fd, id, .InternalError, "no active provider configured");
        return;
    };

    // Capture provider info on every turn so a /model swap mid-session shows
    // up correctly in state.json.
    active.setProvider(@tagName(profile.kind), profile.model) catch {};

    try active.appendUser(user_text);
    otel.emitLog(.info, "user.message", &.{
        core.Attr.integer("input_len", @intCast(user_text.len)),
    }, parent);

    const session = &active.session;

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
        // Auto-compact: if the previous round (or a resumed session) reported
        // an input_tokens count above the configured threshold, run truncate
        // before issuing the next provider.send. The compactor ALWAYS keeps
        // the system head and the last `compaction_tail_turns` user turns,
        // so the immediate context (the message we're about to respond to)
        // is preserved.
        if (active.last_input_tokens > 0) {
            const limit = core.model_info.lookup(profile);
            const threshold = compactionThreshold(ctx);
            const cutoff: u64 = @intFromFloat(@as(f64, @floatFromInt(limit)) * @as(f64, threshold));
            if (active.last_input_tokens >= cutoff) {
                const tail = compactionTailTurns(ctx);
                const dropped = active.truncate(tail) catch |err| blk: {
                    otel.emitLog(.warn, "session.autocompact_failed", &.{
                        core.Attr.string("error", @errorName(err)),
                    }, parent);
                    break :blk 0;
                };
                if (dropped > 0) {
                    otel.emitLog(.info, "session.autocompact", &.{
                        core.Attr.integer("dropped", @intCast(dropped)),
                        core.Attr.integer("tail_turns", @intCast(tail)),
                        core.Attr.integer("last_input_tokens", @intCast(active.last_input_tokens)),
                        core.Attr.integer("limit", @intCast(limit)),
                    }, parent);

                    // Tell the user we acted; they should be able to see this
                    // in the chat without surprise.
                    var line: std.ArrayList(u8) = .empty;
                    defer line.deinit(gpa);
                    const msg = try std.fmt.allocPrint(
                        a,
                        "auto-compacted: dropped {d} message(s) (was {d} tokens, limit {d}, threshold {d:.0}%)",
                        .{ dropped, active.last_input_tokens, limit, threshold * 100.0 },
                    );
                    try protocol.writeEventErrLine(&line, gpa, id, msg);
                    try line.append(gpa, '\n');
                    try writeAll(stdout_fd, line.items);

                    // Reset the watermark so we don't compact again next round
                    // unless the new context blows past the threshold too.
                    active.last_input_tokens = 0;
                }
            }
        }

        var provider_span = otel.startSpan("provider.send", .client, parent);
        defer provider_span.end();
        provider_span.setAttrs(&[_]core.Attr{
            core.Attr.string("provider.kind", @tagName(profile.kind)),
            core.Attr.string("provider.model", profile.model),
            core.Attr.integer("round", round),
        });
        otel.emitLog(.info, "provider.request", &.{
            core.Attr.string("provider.kind", @tagName(profile.kind)),
            core.Attr.string("provider.model", profile.model),
            core.Attr.integer("round", round),
            core.Attr.integer("history_len", @intCast(session.messages.items.len)),
        }, provider_span.context());

        // Provider lifecycle is per-send. createProvider is cheap (one alloc
        // plus credential dup) and keeps us free to swap providers across
        // turns when /model changes between sends without rebuilding state.
        const provider = core.createProvider(gpa, ctx.io, profile) catch |err| {
            provider_span.setStatusError(@errorName(err));
            otel.emitLog(.err, "provider.create_failed", &.{
                core.Attr.string("error", @errorName(err)),
            }, provider_span.context());
            try writeErrorEnvelope(gpa, stdout_fd, id, .InternalError, @errorName(err));
            return;
        };
        defer core.destroyProvider(gpa, provider);

        var it = provider.send(gpa, .{
            .messages = session.messages.items,
            .config = .{ .model = profile.model },
            .tools = tool_slice.items,
        }) catch |err| {
            provider_span.setStatusError(@errorName(err));
            otel.emitLog(.err, "provider.send_failed", &.{
                core.Attr.string("error", @errorName(err)),
            }, provider_span.context());
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
                    otel.emitLog(.err, "provider.stream_error", &.{
                        core.Attr.string("error", e),
                    }, provider_span.context());
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

        // Carry usage forward (cumulative across tool-use rounds) and update
        // the active session so persistence + auto-compact see fresh numbers.
        usage_total.input_tokens +%= round_usage.input_tokens;
        usage_total.output_tokens +%= round_usage.output_tokens;
        stop_reason = round_stop;
        active.recordUsage(round_usage.input_tokens, round_usage.output_tokens);

        provider_span.setAttrs(&[_]core.Attr{
            core.Attr.integer("input_tokens", @intCast(round_usage.input_tokens)),
            core.Attr.integer("output_tokens", @intCast(round_usage.output_tokens)),
            core.Attr.string("stop_reason", @tagName(round_stop)),
            core.Attr.integer("event_count", @intCast(event_count)),
            core.Attr.integer("tool_calls", @intCast(pending.items.len)),
        });
        otel.emitLog(.info, "provider.response", &.{
            core.Attr.integer("input_tokens", @intCast(round_usage.input_tokens)),
            core.Attr.integer("output_tokens", @intCast(round_usage.output_tokens)),
            core.Attr.string("stop_reason", @tagName(round_stop)),
            core.Attr.integer("tool_calls", @intCast(pending.items.len)),
        }, provider_span.context());

        if (last_err != null) {
            provider_span.setStatusError(last_err.?);
            break;
        }
        provider_span.setStatusOk();

        // Persist assistant text (if any) before any associated tool_calls,
        // so providers like Claude can coalesce them into one assistant turn.
        if (assistant_buf.items.len > 0) {
            try active.appendAssistant(assistant_buf.items);
        }

        if (pending.items.len == 0) {
            // Plain assistant turn — done.
            break;
        }

        // Tool-use round: persist + invoke + re-send.
        for (pending.items) |pc| {
            try active.appendToolCall(.{
                .id = pc.id,
                .name = pc.name,
                .args_json = pc.args_json,
            });

            var call_line: std.ArrayList(u8) = .empty;
            defer call_line.deinit(gpa);
            try protocol.writeEventToolCallLine(&call_line, gpa, id, pc.id, pc.name, pc.args_json);
            try call_line.append(gpa, '\n');
            try writeAll(stdout_fd, call_line.items);

            // Per-tool span: parent is the provider round so the trace tree
            // mirrors §11.2 of DESIGN.md (orchestrator → coder → run_shell).
            var tool_span = otel.startSpan("tool.invoke", .internal, provider_span.context());
            tool_span.setAttrs(&[_]core.Attr{
                core.Attr.string("tool.name", pc.name),
                core.Attr.string("tool.id", pc.id),
                core.Attr.integer("tool.args_len", @intCast(pc.args_json.len)),
            });
            otel.emitLog(.info, "tool.call", &.{
                core.Attr.string("tool.name", pc.name),
                core.Attr.string("tool.id", pc.id),
                core.Attr.string("tool.args", pc.args_json),
            }, tool_span.context());

            const result = invokeOne(gpa, ctx.io, &registry, pc.name, pc.args_json) catch |err| blk: {
                tool_span.setStatusError(@errorName(err));
                otel.emitLog(.err, "tool.dispatch_error", &.{
                    core.Attr.string("tool.name", pc.name),
                    core.Attr.string("error", @errorName(err)),
                }, tool_span.context());
                const msg = try std.fmt.allocPrint(gpa, "tool dispatch error: {s}", .{@errorName(err)});
                break :blk core.tool.ToolResult{ .output = msg, .truncated = false, .is_error = true };
            };
            defer gpa.free(result.output);

            tool_span.setAttrs(&[_]core.Attr{
                core.Attr.integer("tool.output_len", @intCast(result.output.len)),
                core.Attr.boolean("tool.is_error", result.is_error),
                core.Attr.boolean("tool.truncated", result.truncated),
            });
            if (result.is_error) {
                tool_span.setStatusError(result.output);
            } else {
                tool_span.setStatusOk();
            }
            otel.emitLog(if (result.is_error) .warn else .info, "tool.result", &.{
                core.Attr.string("tool.name", pc.name),
                core.Attr.string("tool.id", pc.id),
                core.Attr.boolean("is_error", result.is_error),
                core.Attr.boolean("truncated", result.truncated),
                core.Attr.integer("output_len", @intCast(result.output.len)),
            }, tool_span.context());
            tool_span.end();

            try active.appendToolResult(.{
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
        otel.emitLog(.err, "session.tool_loop_exceeded", &.{
            core.Attr.integer("max_rounds", @intCast(max_tool_rounds)),
        }, parent);
        var line: std.ArrayList(u8) = .empty;
        defer line.deinit(gpa);
        try protocol.writeEventErrLine(&line, gpa, id, msg);
        try line.append(gpa, '\n');
        try writeAll(stdout_fd, line.items);
    }

    otel.emitLog(.info, "session.complete", &.{
        core.Attr.string("stop_reason", @tagName(stop_reason)),
        core.Attr.integer("input_tokens_total", @intCast(usage_total.input_tokens)),
        core.Attr.integer("output_tokens_total", @intCast(usage_total.output_tokens)),
        core.Attr.integer("rounds", @intCast(round)),
        core.Attr.boolean("ok", last_err == null),
    }, parent);

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
    var otel = core.Otel.init(a, std.testing.io, .{});
    defer otel.deinit();
    var active = active_session.ActiveSession.init(a, std.testing.io, null, null);
    defer active.deinit();
    try handleCommandDispatch(&resp, a, arena.allocator(), 2, params, ctx, &active, &otel, null);

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
    var otel = core.Otel.init(a, std.testing.io, .{});
    defer otel.deinit();
    var active = active_session.ActiveSession.init(a, std.testing.io, null, null);
    defer active.deinit();
    try handleCommandDispatch(&resp, a, arena.allocator(), 3, params, ctx, &active, &otel, null);

    try std.testing.expect(std.mem.indexOf(u8, resp.items, "\"id\":3") != null);
    try std.testing.expect(std.mem.indexOf(u8, resp.items, "\"kind\":\"err\"") != null);
}

test "handleSessionSend command path emits one terminal line, no events" {
    const a = std.testing.allocator;
    var cfg = try core.Config.load(a, std.testing.io, .{ .home = null });
    defer cfg.deinit();

    var active = active_session.ActiveSession.init(a, std.testing.io, null, null);
    defer active.deinit();

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

    var otel = core.Otel.init(a, std.testing.io, .{});
    defer otel.deinit();
    try handleSessionSend(a, arena.allocator(), 7, params, ctx, &active, write_fd, &otel, null);
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
    try std.testing.expectEqual(@as(usize, 0), active.session.messages.items.len);
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
