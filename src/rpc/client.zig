const std = @import("std");
const core = @import("phoenix_core");
const commands = @import("commands");

pub const ConfigSnapshot = struct {
    default_provider: ?DefaultProvider,
    sources_count: usize,

    pub const DefaultProvider = struct {
        kind: core.ProviderKind,
        model: []const u8,
    };
};

/// Mirrors `commands.Result` but with strings owned by the response arena.
/// Caller must call `Response.deinit` when done with borrowed slices.
pub const DispatchResult = union(enum) {
    not_a_command,
    message: []const u8,
    err: []const u8,
    cleared: []const u8,
    compacted: []const u8,
    model_picker: ModelPicker,
    inject_context: ContextFragment,
    session_picker: SessionPicker,
    models_page: ModelsPage,
    connect_wizard,
    theme_picker: ThemePicker,

    pub const ModelPicker = struct {
        title: []const u8,
        choices: []commands.ModelChoice,
    };
    pub const ContextFragment = struct {
        label: []const u8,
        body: []const u8,
        user_message: []const u8,
    };
    pub const SessionPicker = struct {
        title: []const u8,
        choices: []commands.SessionChoice,
    };
    pub const ModelsPage = struct {
        title: []const u8,
        entries: []commands.ModelEntry,
    };
    pub const ThemePicker = struct {
        requested: ?[]const u8,
    };
};

pub const AddModelArgs = struct {
    kind: core.ProviderKind,
    model: []const u8,
    api_key: []const u8,
    base_url: []const u8,
    context_window: ?u32,
};

pub const AddModelResult = struct {
    message: []const u8,
    entries: []commands.ModelEntry,
};

pub const ApplySessionResult = struct {
    message: []const u8,
    message_count: u32,
    messages: []const RestoredMessage,

    pub const RestoredMessage = struct {
        role: core.Role,
        content: []const u8,
    };
};

pub const ApplyResult = struct {
    message: []const u8,
    default_provider: ConfigSnapshot.DefaultProvider,
};

/// Outcome of a session.send call. The TUI does not pre-classify input —
/// it submits text and the server decides whether the line was a slash
/// command (returns `.command`) or a conversational turn (returns
/// `.conversation` after streaming token events).
pub const SendOutcome = union(enum) {
    conversation: ConversationResult,
    command: DispatchResult,
};

pub const ConversationResult = struct {
    /// True iff the provider call ended with a clean `done` event.
    ok: bool,
    /// Wire string from the provider event (e.g. "end_turn").
    /// Owned by the response arena.
    stop_reason: []const u8,
    input_tokens: u32,
    output_tokens: u32,
    cache_creation_input_tokens: u32,
    cache_read_input_tokens: u32,
    /// Empty when ok=true; the last err event's text when ok=false.
    /// Owned by the response arena.
    reason: []const u8,
};

/// Optional per-event callbacks invoked while sessionSend streams. Slices
/// passed into a callback are valid only for the duration of that call —
/// copy them if you need to retain past the next event.
///
/// `on_tick` fires periodically (every `tick_interval_ms`) when no server
/// data has arrived — used by the TUI to drive the "thinking" animation
/// during the pre-first-token wait. It is *not* fired while data is
/// streaming through; the per-event callbacks already drive redraws then.
pub const StreamCallbacks = struct {
    ctx: *anyopaque,
    on_token: ?*const fn (ctx: *anyopaque, text: []const u8) void = null,
    on_context: ?*const fn (ctx: *anyopaque, label: []const u8, body: []const u8) void = null,
    on_err: ?*const fn (ctx: *anyopaque, text: []const u8) void = null,
    on_tool_call: ?*const fn (ctx: *anyopaque, tool_id: []const u8, name: []const u8, args_json: []const u8) void = null,
    on_tool_result: ?*const fn (ctx: *anyopaque, tool_id: []const u8, output: []const u8, is_error: bool) void = null,
    on_tick: ?*const fn (ctx: *anyopaque) void = null,
    tick_interval_ms: i32 = 150,
};

pub const Response = struct {
    arena: std.heap.ArenaAllocator,

    pub fn deinit(self: *Response) void {
        self.arena.deinit();
    }
};

pub const Client = struct {
    gpa: std.mem.Allocator,
    io: std.Io,
    child: std.process.Child,
    line_buf: std.ArrayList(u8),
    next_id: i64,

    /// Spawn `phoenix rpc` as a child process. `argv0` is the path to the
    /// phoenix binary. `io` is the process's Io implementation (from
    /// `std.process.Init.io`).
    ///
    /// Child stderr is redirected to `/tmp/phoenix-rpc.log` (truncated on each
    /// spawn). `.inherit` would route logs to the parent's TTY which the TUI
    /// has placed in raw mode — they'd render as garbage on top of the chat.
    /// If the log file can't be opened we fall back to `.ignore`.
    pub fn spawn(
        gpa: std.mem.Allocator,
        io: std.Io,
        argv0: []const u8,
        explicit_config: ?[]const u8,
    ) !Client {
        var argv: std.ArrayList([]const u8) = .empty;
        defer argv.deinit(gpa);
        try argv.append(gpa, argv0);
        if (explicit_config) |p| {
            try argv.append(gpa, "--config");
            try argv.append(gpa, p);
        }
        try argv.append(gpa, "rpc");

        const log_path = "/tmp/phoenix-rpc.log";
        const stderr_io: std.process.SpawnOptions.StdIo = blk: {
            const file = std.Io.Dir.cwd().createFile(io, log_path, .{
                .read = false,
                .truncate = true,
            }) catch break :blk .ignore;
            break :blk .{ .file = file };
        };

        const child = try std.process.spawn(io, .{
            .argv = argv.items,
            .stdin = .pipe,
            .stdout = .pipe,
            .stderr = stderr_io,
        });

        // The child has its own dup of the fd; close ours so EOF behavior is
        // correct on shutdown.
        if (stderr_io == .file) stderr_io.file.close(io);

        return .{
            .gpa = gpa,
            .io = io,
            .child = child,
            .line_buf = .empty,
            .next_id = 1,
        };
    }

    /// Close stdin (signals EOF to server), wait for child to exit.
    pub fn deinit(self: *Client) void {
        if (self.child.stdin) |stdin| {
            // Why: closing stdin signals EOF so the server's read loop exits.
            _ = std.c.close(stdin.handle);
            self.child.stdin = null;
        }
        _ = self.child.wait(self.io) catch {};
        self.line_buf.deinit(self.gpa);
    }

    pub fn getConfig(self: *Client) !struct { snap: ConfigSnapshot, response: Response } {
        var resp = try self.callParsed("config.get", null);
        errdefer resp.response.deinit();
        const a = resp.response.arena.allocator();

        const result_val = resp.result orelse return error.RpcError;
        if (result_val != .object) return error.RpcError;
        const obj = result_val.object;

        const sources_count: usize = blk: {
            const sv = obj.get("sources_count") orelse break :blk 0;
            if (sv == .integer) break :blk @intCast(sv.integer);
            break :blk 0;
        };

        const dp: ?ConfigSnapshot.DefaultProvider = blk: {
            const dpv = obj.get("default_provider") orelse break :blk null;
            if (dpv == .null) break :blk null;
            if (dpv != .object) break :blk null;
            const dpo = dpv.object;
            const kv = dpo.get("kind") orelse break :blk null;
            const mv = dpo.get("model") orelse break :blk null;
            if (kv != .string or mv != .string) break :blk null;
            const kind = std.meta.stringToEnum(core.ProviderKind, kv.string) orelse break :blk null;
            break :blk .{
                .kind = kind,
                .model = try a.dupe(u8, mv.string),
            };
        };

        return .{
            .snap = .{ .default_provider = dp, .sources_count = sources_count },
            .response = resp.response,
        };
    }

    pub fn listCommands(self: *Client) !struct { items: []commands.CommandInfo, response: Response } {
        var resp = try self.callParsed("command.list", null);
        errdefer resp.response.deinit();
        const a = resp.response.arena.allocator();

        const result_val = resp.result orelse return error.RpcError;
        if (result_val != .object) return error.RpcError;
        const cmds_v = result_val.object.get("commands") orelse return error.RpcError;
        if (cmds_v != .array) return error.RpcError;

        var list: std.ArrayList(commands.CommandInfo) = .empty;
        for (cmds_v.array.items) |cv| {
            if (cv != .object) continue;
            const co = cv.object;
            const nv = co.get("name") orelse continue;
            const sv = co.get("summary") orelse continue;
            const skv = co.get("is_skill") orelse std.json.Value{ .bool = false };
            if (nv != .string or sv != .string) continue;
            try list.append(a, .{
                .name = try a.dupe(u8, nv.string),
                .summary = try a.dupe(u8, sv.string),
                .is_skill = if (skv == .bool) skv.bool else false,
            });
        }
        return .{ .items = try list.toOwnedSlice(a), .response = resp.response };
    }

    pub fn dispatch(self: *Client, input: []const u8) !struct { result: DispatchResult, response: Response } {
        const params = try std.json.Stringify.valueAlloc(
            self.gpa,
            .{ .input = input },
            .{},
        );
        defer self.gpa.free(params);

        var resp = try self.callParsed("command.dispatch", params);
        errdefer resp.response.deinit();
        const a = resp.response.arena.allocator();

        const result_val = resp.result orelse return error.RpcError;
        const dr = try parseDispatchResult(a, result_val);
        return .{ .result = dr, .response = resp.response };
    }

    pub fn applySessionChoice(self: *Client, id: []const u8) !struct { result: ApplySessionResult, response: Response } {
        const params = try std.json.Stringify.valueAlloc(
            self.gpa,
            .{ .id = id },
            .{},
        );
        defer self.gpa.free(params);

        var resp = try self.callParsed("command.applySessionChoice", params);
        errdefer resp.response.deinit();
        const a = resp.response.arena.allocator();

        const result_val = resp.result orelse return error.RpcError;
        if (result_val != .object) return error.RpcError;
        const obj = result_val.object;

        const msg_v = obj.get("message") orelse return error.RpcError;
        const msg = if (msg_v == .string) try a.dupe(u8, msg_v.string) else try a.dupe(u8, "");
        const cnt_v = obj.get("message_count") orelse std.json.Value{ .integer = 0 };
        const cnt: u32 = if (cnt_v == .integer and cnt_v.integer >= 0) @intCast(cnt_v.integer) else 0;

        var msgs_list: std.ArrayList(ApplySessionResult.RestoredMessage) = .empty;
        if (obj.get("messages")) |mv| {
            if (mv == .array) {
                for (mv.array.items) |item| {
                    if (item != .object) continue;
                    const io = item.object;
                    const rv = io.get("role") orelse continue;
                    const cv = io.get("content") orelse continue;
                    if (rv != .string or cv != .string) continue;
                    const role = std.meta.stringToEnum(core.Role, rv.string) orelse continue;
                    try msgs_list.append(a, .{
                        .role = role,
                        .content = try a.dupe(u8, cv.string),
                    });
                }
            }
        }

        return .{
            .result = .{
                .message = msg,
                .message_count = cnt,
                .messages = try msgs_list.toOwnedSlice(a),
            },
            .response = resp.response,
        };
    }

    pub fn addModel(self: *Client, args: AddModelArgs) !struct { result: AddModelResult, response: Response } {
        // Stringify writes enums as their tag name and omits null optionals
        // when emit_null_optional_fields = false, so AddModelArgs serializes
        // straight to the wire shape the server expects.
        const params = try std.json.Stringify.valueAlloc(
            self.gpa,
            args,
            .{ .emit_null_optional_fields = false },
        );
        defer self.gpa.free(params);

        var resp = try self.callParsed("command.addModel", params);
        errdefer resp.response.deinit();
        const a = resp.response.arena.allocator();

        const result_val = resp.result orelse return error.RpcError;
        if (result_val != .object) return error.RpcError;
        const obj = result_val.object;

        const msg_v = obj.get("message") orelse return error.RpcError;
        const msg = if (msg_v == .string) try a.dupe(u8, msg_v.string) else try a.dupe(u8, "");

        const entries: []commands.ModelEntry = if (obj.get("entries")) |ev|
            try parseModelEntries(a, ev)
        else
            &.{};
        return .{
            .result = .{ .message = msg, .entries = entries },
            .response = resp.response,
        };
    }

    pub fn applyModelChoice(self: *Client, choice: commands.ModelChoice) !struct { result: ApplyResult, response: Response } {
        // commands.ModelChoice already matches the wire shape (provider_index,
        // kind, model, is_active), so Stringify.value can serialize it
        // directly. Stringify writes enums as their tag name.
        const params = try std.json.Stringify.valueAlloc(self.gpa, choice, .{});
        defer self.gpa.free(params);

        var resp = try self.callParsed("command.applyModelChoice", params);
        errdefer resp.response.deinit();
        const a = resp.response.arena.allocator();

        const result_val = resp.result orelse return error.RpcError;
        if (result_val != .object) return error.RpcError;
        const obj = result_val.object;

        const msg_v = obj.get("message") orelse return error.RpcError;
        const msg = if (msg_v == .string) try a.dupe(u8, msg_v.string) else try a.dupe(u8, "");

        const dp_v = obj.get("default_provider") orelse return error.RpcError;
        if (dp_v != .object) return error.RpcError;
        const dp_obj = dp_v.object;
        const kv = dp_obj.get("kind") orelse return error.RpcError;
        const mv = dp_obj.get("model") orelse return error.RpcError;
        if (kv != .string or mv != .string) return error.RpcError;
        const knd = std.meta.stringToEnum(core.ProviderKind, kv.string) orelse return error.RpcError;

        return .{
            .result = .{
                .message = msg,
                .default_provider = .{
                    .kind = knd,
                    .model = try a.dupe(u8, mv.string),
                },
            },
            .response = resp.response,
        };
    }

    /// Submit one user-typed line to the server. The server classifies it as
    /// either a slash command (single terminal `command` outcome) or a normal
    /// conversation turn (zero or more streamed events followed by a
    /// `conversation` outcome). The returned response arena owns any strings
    /// borrowed by the outcome; callbacks see arena-borrowed slices that are
    /// only valid for the duration of the call.
    pub fn sessionSend(
        self: *Client,
        text: []const u8,
        callbacks: StreamCallbacks,
    ) !struct { outcome: SendOutcome, response: Response } {
        const id = self.next_id;
        self.next_id += 1;

        const params = try std.json.Stringify.valueAlloc(
            self.gpa,
            .{ .text = text },
            .{},
        );
        defer self.gpa.free(params);

        try self.writeRequestLine(id, "session.send", params);

        var resp_arena = std.heap.ArenaAllocator.init(self.gpa);
        errdefer resp_arena.deinit();

        // Per-iteration arena for parsing each streamed line. Reused across
        // events; deinit'd at the end. Token slices passed to on_token are
        // freed before the next read.
        var line_arena = std.heap.ArenaAllocator.init(self.gpa);
        defer line_arena.deinit();

        while (true) {
            _ = line_arena.reset(.retain_capacity);
            try self.readLineWithTick(callbacks);
            const line = self.line_buf.items;

            const parsed = std.json.parseFromSliceLeaky(
                std.json.Value,
                line_arena.allocator(),
                line,
                .{},
            ) catch return error.RpcError;
            if (parsed != .object) return error.RpcError;
            const obj = parsed.object;

            if (obj.get("id")) |id_v| {
                if (id_v == .integer and id_v.integer != id) return error.RpcError;
            }

            // error envelope = pre-stream failure.
            if (obj.get("error") != null) return error.RpcError;

            // Streaming event line.
            if (obj.get("event")) |ev_v| {
                if (ev_v != .object) continue;
                const ev = ev_v.object;
                const kind_v = ev.get("kind") orelse continue;
                if (kind_v != .string) continue;

                if (std.mem.eql(u8, kind_v.string, "token")) {
                    const text_v = ev.get("text") orelse continue;
                    if (text_v != .string) continue;
                    if (callbacks.on_token) |cb| cb(callbacks.ctx, text_v.string);
                } else if (std.mem.eql(u8, kind_v.string, "context")) {
                    const label_v = ev.get("label") orelse continue;
                    const body_v = ev.get("body") orelse continue;
                    if (label_v != .string or body_v != .string) continue;
                    if (callbacks.on_context) |cb| cb(callbacks.ctx, label_v.string, body_v.string);
                } else if (std.mem.eql(u8, kind_v.string, "err")) {
                    const text_v = ev.get("text") orelse continue;
                    if (text_v != .string) continue;
                    if (callbacks.on_err) |cb| cb(callbacks.ctx, text_v.string);
                } else if (std.mem.eql(u8, kind_v.string, "tool_call")) {
                    const tid_v = ev.get("tool_id") orelse continue;
                    const name_v = ev.get("name") orelse continue;
                    const args_v = ev.get("args") orelse continue;
                    if (tid_v != .string or name_v != .string or args_v != .string) continue;
                    if (callbacks.on_tool_call) |cb| cb(callbacks.ctx, tid_v.string, name_v.string, args_v.string);
                } else if (std.mem.eql(u8, kind_v.string, "tool_result")) {
                    const tid_v = ev.get("tool_id") orelse continue;
                    const out_v = ev.get("output") orelse continue;
                    const err_v = ev.get("is_error") orelse std.json.Value{ .bool = false };
                    if (tid_v != .string or out_v != .string) continue;
                    const is_err = if (err_v == .bool) err_v.bool else false;
                    if (callbacks.on_tool_result) |cb| cb(callbacks.ctx, tid_v.string, out_v.string, is_err);
                }
                continue;
            }

            // Terminal result.
            const result_v = obj.get("result") orelse return error.RpcError;
            if (result_v != .object) return error.RpcError;
            const result = result_v.object;
            const rkind_v = result.get("kind") orelse return error.RpcError;
            if (rkind_v != .string) return error.RpcError;
            const rkind = rkind_v.string;

            const a = resp_arena.allocator();
            if (std.mem.eql(u8, rkind, "conversation")) {
                const ok_v = result.get("ok") orelse return error.RpcError;
                if (ok_v != .bool) return error.RpcError;
                const stop_v = result.get("stop_reason") orelse std.json.Value{ .string = "" };
                const stop_str: []const u8 = if (stop_v == .string) stop_v.string else "";
                const in_v = result.get("input_tokens") orelse std.json.Value{ .integer = 0 };
                const out_v = result.get("output_tokens") orelse std.json.Value{ .integer = 0 };
                const cache_create_v = result.get("cache_creation_input_tokens") orelse std.json.Value{ .integer = 0 };
                const cache_read_v = result.get("cache_read_input_tokens") orelse std.json.Value{ .integer = 0 };
                const reason_v = result.get("reason") orelse std.json.Value{ .string = "" };
                const reason_str: []const u8 = if (reason_v == .string) reason_v.string else "";
                return .{
                    .outcome = .{ .conversation = .{
                        .ok = ok_v.bool,
                        .stop_reason = try a.dupe(u8, stop_str),
                        .input_tokens = if (in_v == .integer) @intCast(in_v.integer) else 0,
                        .output_tokens = if (out_v == .integer) @intCast(out_v.integer) else 0,
                        .cache_creation_input_tokens = if (cache_create_v == .integer) @intCast(cache_create_v.integer) else 0,
                        .cache_read_input_tokens = if (cache_read_v == .integer) @intCast(cache_read_v.integer) else 0,
                        .reason = try a.dupe(u8, reason_str),
                    } },
                    .response = .{ .arena = resp_arena },
                };
            } else if (std.mem.eql(u8, rkind, "command")) {
                const cmd_v = result.get("command") orelse return error.RpcError;
                const dr = try parseDispatchResult(a, cmd_v);
                return .{
                    .outcome = .{ .command = dr },
                    .response = .{ .arena = resp_arena },
                };
            } else {
                return error.RpcError;
            }
        }
    }

    fn writeRequestLine(
        self: *Client,
        id: i64,
        method: []const u8,
        params_json: ?[]const u8,
    ) !void {
        const req = try buildRequestEnvelope(self.gpa, id, method, params_json);
        defer self.gpa.free(req);

        // Append the framing newline. Cheaper than asking Stringify to do it.
        const framed = try std.fmt.allocPrint(self.gpa, "{s}\n", .{req});
        defer self.gpa.free(framed);

        const stdin_fd = self.child.stdin.?.handle;
        try writeAll(stdin_fd, framed);
    }

    /// Internal: write one JSON-line request, read one JSON-line response,
    /// parse the envelope. Returns the parsed result value (or null on rpc error)
    /// and a Response owning the arena.
    fn callParsed(
        self: *Client,
        method: []const u8,
        params_json: ?[]const u8,
    ) !struct { result: ?std.json.Value, response: Response } {
        const id = self.next_id;
        self.next_id += 1;

        const req = try buildRequestEnvelope(self.gpa, id, method, params_json);
        defer self.gpa.free(req);
        const framed = try std.fmt.allocPrint(self.gpa, "{s}\n", .{req});
        defer self.gpa.free(framed);

        // Write to child stdin.
        const stdin_fd = self.child.stdin.?.handle;
        try writeAll(stdin_fd, framed);

        // Read one line from child stdout.
        try self.readLine();

        // Parse into arena.
        var arena = std.heap.ArenaAllocator.init(self.gpa);
        errdefer arena.deinit();
        const a = arena.allocator();

        const line = self.line_buf.items;
        const parsed = std.json.parseFromSliceLeaky(std.json.Value, a, line, .{}) catch {
            return error.RpcError;
        };

        if (parsed != .object) return error.RpcError;
        const obj = parsed.object;

        // Validate id matches.
        if (obj.get("id")) |id_v| {
            if (id_v == .integer and id_v.integer != id) {
                return error.RpcError;
            }
        }

        // Check for error envelope.
        if (obj.get("error") != null) {
            return .{ .result = null, .response = .{ .arena = arena } };
        }

        const result = obj.get("result");
        return .{
            .result = result,
            .response = .{ .arena = arena },
        };
    }

    fn readLine(self: *Client) !void {
        self.line_buf.clearRetainingCapacity();
        const stdout_fd = self.child.stdout.?.handle;
        var byte: [1]u8 = undefined;
        while (true) {
            const n = try std.posix.read(stdout_fd, &byte);
            if (n == 0) return error.ServerClosed;
            if (byte[0] == '\n') return;
            try self.line_buf.append(self.gpa, byte[0]);
        }
    }

    /// Like `readLine`, but fires `callbacks.on_tick` every
    /// `tick_interval_ms` while no bytes are available. Used by sessionSend
    /// so the TUI can animate a "thinking" indicator during the pre-first-
    /// token wait without spinning a separate timer thread (vaxis isn't
    /// thread-safe and the rendering surface lives on the main thread).
    fn readLineWithTick(self: *Client, callbacks: StreamCallbacks) !void {
        self.line_buf.clearRetainingCapacity();
        const stdout_fd = self.child.stdout.?.handle;
        const has_tick = callbacks.on_tick != null and callbacks.tick_interval_ms > 0;

        // Poll only at the start of a line / before each byte. Once data is
        // flowing, read() returns immediately on each call and we never
        // re-enter the timeout branch.
        var byte: [1]u8 = undefined;
        while (true) {
            if (has_tick) {
                var pfds = [_]std.posix.pollfd{.{
                    .fd = stdout_fd,
                    .events = std.posix.POLL.IN,
                    .revents = 0,
                }};
                const ready = try std.posix.poll(&pfds, callbacks.tick_interval_ms);
                if (ready == 0) {
                    callbacks.on_tick.?(callbacks.ctx);
                    continue;
                }
            }
            const n = try std.posix.read(stdout_fd, &byte);
            if (n == 0) return error.ServerClosed;
            if (byte[0] == '\n') return;
            try self.line_buf.append(self.gpa, byte[0]);
        }
    }
};

/// Parse a server dispatch payload (the inside of `result` from command.dispatch
/// or `result.command` from session.send) into a DispatchResult. Allocates
/// borrowed strings on `a`.
fn parseDispatchResult(a: std.mem.Allocator, result_val: std.json.Value) !DispatchResult {
    if (result_val != .object) return error.RpcError;
    const obj = result_val.object;

    const kind_v = obj.get("kind") orelse return error.RpcError;
    if (kind_v != .string) return error.RpcError;
    const kind = kind_v.string;

    if (std.mem.eql(u8, kind, "not_a_command")) {
        return .not_a_command;
    } else if (std.mem.eql(u8, kind, "message")) {
        const tv = obj.get("text") orelse return DispatchResult{ .message = "" };
        const text = if (tv == .string) try a.dupe(u8, tv.string) else try a.dupe(u8, "");
        return DispatchResult{ .message = text };
    } else if (std.mem.eql(u8, kind, "cleared")) {
        const tv = obj.get("text") orelse return DispatchResult{ .cleared = "" };
        const text = if (tv == .string) try a.dupe(u8, tv.string) else try a.dupe(u8, "");
        return DispatchResult{ .cleared = text };
    } else if (std.mem.eql(u8, kind, "compacted")) {
        const tv = obj.get("text") orelse return DispatchResult{ .compacted = "" };
        const text = if (tv == .string) try a.dupe(u8, tv.string) else try a.dupe(u8, "");
        return DispatchResult{ .compacted = text };
    } else if (std.mem.eql(u8, kind, "err")) {
        const tv = obj.get("text") orelse return DispatchResult{ .err = "" };
        const text = if (tv == .string) try a.dupe(u8, tv.string) else try a.dupe(u8, "");
        return DispatchResult{ .err = text };
    } else if (std.mem.eql(u8, kind, "model_picker")) {
        const title_v = obj.get("title") orelse return DispatchResult{ .err = "missing title" };
        const title = if (title_v == .string) try a.dupe(u8, title_v.string) else try a.dupe(u8, "");
        const choices_v = obj.get("choices") orelse return DispatchResult{ .model_picker = .{ .title = title, .choices = &.{} } };
        var choices_list: std.ArrayList(commands.ModelChoice) = .empty;
        if (choices_v == .array) {
            for (choices_v.array.items) |cv| {
                if (cv != .object) continue;
                const co = cv.object;
                const pi_v = co.get("provider_index") orelse continue;
                const kn_v = co.get("kind") orelse continue;
                const md_v = co.get("model") orelse continue;
                const ia_v = co.get("is_active") orelse continue;
                if (pi_v != .integer or kn_v != .string or md_v != .string or ia_v != .bool) continue;
                if (pi_v.integer < 0) continue;
                const ck = std.meta.stringToEnum(core.ProviderKind, kn_v.string) orelse continue;
                try choices_list.append(a, commands.ModelChoice{
                    .provider_index = @intCast(pi_v.integer),
                    .kind = ck,
                    .model = try a.dupe(u8, md_v.string),
                    .is_active = ia_v.bool,
                });
            }
        }
        return DispatchResult{ .model_picker = .{
            .title = title,
            .choices = try choices_list.toOwnedSlice(a),
        } };
    } else if (std.mem.eql(u8, kind, "inject_context")) {
        const label_v = obj.get("label") orelse return DispatchResult{ .err = "missing label" };
        const body_v = obj.get("body") orelse return DispatchResult{ .err = "missing body" };
        const um_v = obj.get("user_message") orelse return DispatchResult{ .err = "missing user_message" };
        return DispatchResult{ .inject_context = .{
            .label = if (label_v == .string) try a.dupe(u8, label_v.string) else try a.dupe(u8, ""),
            .body = if (body_v == .string) try a.dupe(u8, body_v.string) else try a.dupe(u8, ""),
            .user_message = if (um_v == .string) try a.dupe(u8, um_v.string) else try a.dupe(u8, ""),
        } };
    } else if (std.mem.eql(u8, kind, "models_page")) {
        const title_v = obj.get("title") orelse return DispatchResult{ .err = "missing title" };
        const title = if (title_v == .string) try a.dupe(u8, title_v.string) else try a.dupe(u8, "");
        const entries_v = obj.get("entries") orelse return DispatchResult{ .models_page = .{ .title = title, .entries = &.{} } };
        const entries = try parseModelEntries(a, entries_v);
        return DispatchResult{ .models_page = .{ .title = title, .entries = entries } };
    } else if (std.mem.eql(u8, kind, "session_picker")) {
        const title_v = obj.get("title") orelse return DispatchResult{ .err = "missing title" };
        const title = if (title_v == .string) try a.dupe(u8, title_v.string) else try a.dupe(u8, "");
        const choices_v = obj.get("choices") orelse return DispatchResult{ .session_picker = .{ .title = title, .choices = &.{} } };
        var choices_list: std.ArrayList(commands.SessionChoice) = .empty;
        if (choices_v == .array) {
            for (choices_v.array.items) |cv| {
                if (cv != .object) continue;
                const co = cv.object;
                const id_v = co.get("id") orelse continue;
                const name_v = co.get("name") orelse continue;
                const upd_v = co.get("updated_at") orelse continue;
                const cnt_v = co.get("message_count") orelse continue;
                if (id_v != .string or name_v != .string or upd_v != .integer or cnt_v != .integer) continue;
                if (cnt_v.integer < 0) continue;
                try choices_list.append(a, commands.SessionChoice{
                    .id = try a.dupe(u8, id_v.string),
                    .name = try a.dupe(u8, name_v.string),
                    .updated_at = upd_v.integer,
                    .message_count = @intCast(cnt_v.integer),
                });
            }
        }
        return DispatchResult{ .session_picker = .{
            .title = title,
            .choices = try choices_list.toOwnedSlice(a),
        } };
    } else if (std.mem.eql(u8, kind, "connect_wizard")) {
        return .connect_wizard;
    } else if (std.mem.eql(u8, kind, "theme_picker")) {
        const req_v = obj.get("requested");
        const requested: ?[]const u8 = if (req_v) |rv| switch (rv) {
            .string => |s| try a.dupe(u8, s),
            else => null,
        } else null;
        return .{ .theme_picker = .{ .requested = requested } };
    } else if (std.mem.eql(u8, kind, "clear_session") or std.mem.eql(u8, kind, "compact_session")) {
        // Server-internal markers that should have been transformed before
        // hitting the wire. If we see one, surface a benign message rather
        // than an error so the chat continues.
        return DispatchResult{ .message = try a.dupe(u8, "") };
    } else {
        return DispatchResult{ .err = try std.fmt.allocPrint(a, "unknown kind: {s}", .{kind}) };
    }
}

/// Parse a JSON array of ModelEntry objects (the wire shape used by both
/// `command.dispatch` -> models_page and `command.addModel` -> result.entries).
fn parseModelEntries(a: std.mem.Allocator, v: std.json.Value) ![]commands.ModelEntry {
    if (v != .array) return &.{};
    var list: std.ArrayList(commands.ModelEntry) = .empty;
    for (v.array.items) |ev| {
        if (ev != .object) continue;
        const eo = ev.object;
        const pi_v = eo.get("provider_index") orelse continue;
        const kn_v = eo.get("kind") orelse continue;
        const md_v = eo.get("model") orelse continue;
        const ia_v = eo.get("is_active") orelse continue;
        const bu_v = eo.get("base_url") orelse continue;
        const cw_v = eo.get("context_window") orelse continue;
        if (pi_v != .integer or kn_v != .string or md_v != .string or ia_v != .bool) continue;
        if (bu_v != .string or cw_v != .integer) continue;
        if (pi_v.integer < 0 or cw_v.integer < 0) continue;
        const knd = std.meta.stringToEnum(core.ProviderKind, kn_v.string) orelse continue;
        try list.append(a, .{
            .provider_index = @intCast(pi_v.integer),
            .kind = knd,
            .model = try a.dupe(u8, md_v.string),
            .is_active = ia_v.bool,
            .base_url = try a.dupe(u8, bu_v.string),
            .context_window = @intCast(cw_v.integer),
        });
    }
    return list.toOwnedSlice(a);
}

fn writeAll(fd: std.posix.fd_t, data: []const u8) !void {
    var remaining = data;
    while (remaining.len > 0) {
        const rc = std.c.write(fd, remaining.ptr, remaining.len);
        if (rc < 0) return error.WriteError;
        remaining = remaining[@intCast(rc)..];
    }
}

/// Encode a JSON-RPC request envelope: `{"id":N,"method":"...","params":<raw>}`.
/// `params_json` is spliced in as raw JSON (already produced by another
/// `Stringify` pass); pass null for methods with no params. Returned slice
/// is owned by the caller.
fn buildRequestEnvelope(
    gpa: std.mem.Allocator,
    id: i64,
    method: []const u8,
    params_json: ?[]const u8,
) ![]u8 {
    var aw: std.Io.Writer.Allocating = .init(gpa);
    errdefer aw.deinit();

    var s: std.json.Stringify = .{ .writer = &aw.writer, .options = .{} };
    try s.beginObject();
    try s.objectField("id");
    try s.write(id);
    try s.objectField("method");
    try s.write(method);
    if (params_json) |pj| {
        try s.objectField("params");
        try s.beginWriteRaw();
        aw.writer.writeAll(pj) catch return error.OutOfMemory;
        s.endWriteRaw();
    }
    try s.endObject();

    return aw.toOwnedSlice();
}

test "buildRequestEnvelope: no params" {
    const a = std.testing.allocator;
    const req = try buildRequestEnvelope(a, 1, "config.get", null);
    defer a.free(req);
    try std.testing.expectEqualStrings("{\"id\":1,\"method\":\"config.get\"}", req);
}

test "buildRequestEnvelope: with raw params" {
    const a = std.testing.allocator;
    const params = try std.json.Stringify.valueAlloc(a, .{ .input = "/model" }, .{});
    defer a.free(params);
    const req = try buildRequestEnvelope(a, 2, "command.dispatch", params);
    defer a.free(req);
    try std.testing.expect(std.mem.indexOf(u8, req, "command.dispatch") != null);
    try std.testing.expect(std.mem.indexOf(u8, req, "\"input\":\"/model\"") != null);
}

test "client parseConfigGetResponse" {
    // Unit test: parse a canned server response for config.get.
    var arena = std.heap.ArenaAllocator.init(std.testing.allocator);
    defer arena.deinit();
    const a = arena.allocator();

    const line = "{\"id\":1,\"result\":{\"default_provider\":{\"kind\":\"claude\",\"model\":\"claude-opus-4-7\"},\"sources_count\":1}}";
    const parsed = try std.json.parseFromSliceLeaky(std.json.Value, a, line, .{});
    try std.testing.expect(parsed == .object);
    const obj = parsed.object;
    const result = obj.get("result").?;
    try std.testing.expect(result == .object);
    const dp = result.object.get("default_provider").?;
    try std.testing.expect(dp == .object);
    const model = dp.object.get("model").?;
    try std.testing.expectEqualStrings("claude-opus-4-7", model.string);
}

test "client parseDispatchResponse not_a_command" {
    var arena = std.heap.ArenaAllocator.init(std.testing.allocator);
    defer arena.deinit();
    const a = arena.allocator();

    const line = "{\"id\":1,\"result\":{\"kind\":\"not_a_command\"}}";
    const parsed = try std.json.parseFromSliceLeaky(std.json.Value, a, line, .{});
    const result = parsed.object.get("result").?;
    const kind = result.object.get("kind").?.string;
    try std.testing.expectEqualStrings("not_a_command", kind);
}

test "client parseDispatchResponse model_picker" {
    var arena = std.heap.ArenaAllocator.init(std.testing.allocator);
    defer arena.deinit();
    const a = arena.allocator();

    const line =
        \\{"id":1,"result":{"kind":"model_picker","title":"Pick","choices":[{"provider_index":0,"kind":"claude","model":"claude-opus-4-7","is_active":true}]}}
    ;
    const parsed = try std.json.parseFromSliceLeaky(std.json.Value, a, line, .{});
    const result = parsed.object.get("result").?;
    const choices = result.object.get("choices").?;
    try std.testing.expect(choices.array.items.len == 1);
    const first = choices.array.items[0];
    try std.testing.expectEqualStrings("claude-opus-4-7", first.object.get("model").?.string);
}
