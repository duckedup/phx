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
    model_picker: ModelPicker,
    inject_context: ContextFragment,

    pub const ModelPicker = struct {
        title: []const u8,
        choices: []commands.ModelChoice,
    };
    pub const ContextFragment = struct {
        label: []const u8,
        body: []const u8,
        user_message: []const u8,
    };
};

pub const ApplyResult = struct {
    message: []const u8,
    default_provider: ConfigSnapshot.DefaultProvider,
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

        const child = try std.process.spawn(io, .{
            .argv = argv.items,
            .stdin = .pipe,
            .stdout = .pipe,
            .stderr = .inherit,
        });

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

    pub fn dispatch(self: *Client, input: []const u8) !struct { result: DispatchResult, response: Response } {
        var params_buf: std.ArrayList(u8) = .empty;
        defer params_buf.deinit(self.gpa);
        try writeJsonString(&params_buf, self.gpa, input);
        var params: std.ArrayList(u8) = .empty;
        defer params.deinit(self.gpa);
        try params.appendSlice(self.gpa, "{\"input\":");
        try params.appendSlice(self.gpa, params_buf.items);
        try params.appendSlice(self.gpa, "}");

        var resp = try self.callParsed("command.dispatch", params.items);
        errdefer resp.response.deinit();
        const a = resp.response.arena.allocator();

        const result_val = resp.result orelse return error.RpcError;
        if (result_val != .object) return error.RpcError;
        const obj = result_val.object;

        const kind_v = obj.get("kind") orelse return error.RpcError;
        if (kind_v != .string) return error.RpcError;
        const kind = kind_v.string;

        const dr: DispatchResult = blk: {
            if (std.mem.eql(u8, kind, "not_a_command")) {
                break :blk .not_a_command;
            } else if (std.mem.eql(u8, kind, "message")) {
                const tv = obj.get("text") orelse break :blk DispatchResult{ .message = "" };
                const text = if (tv == .string) try a.dupe(u8, tv.string) else try a.dupe(u8, "");
                break :blk DispatchResult{ .message = text };
            } else if (std.mem.eql(u8, kind, "err")) {
                const tv = obj.get("text") orelse break :blk DispatchResult{ .err = "" };
                const text = if (tv == .string) try a.dupe(u8, tv.string) else try a.dupe(u8, "");
                break :blk DispatchResult{ .err = text };
            } else if (std.mem.eql(u8, kind, "model_picker")) {
                const title_v = obj.get("title") orelse break :blk DispatchResult{ .err = "missing title" };
                const title = if (title_v == .string) try a.dupe(u8, title_v.string) else try a.dupe(u8, "");
                const choices_v = obj.get("choices") orelse break :blk DispatchResult{ .model_picker = .{ .title = title, .choices = &.{} } };
                var choices_list: std.ArrayList(commands.ModelChoice) = .empty;
                if (choices_v == .array) {
                    for (choices_v.array.items) |cv| {
                        if (cv != .object) continue;
                        const co = cv.object;
                        const pn_v = co.get("provider_name") orelse continue;
                        const kn_v = co.get("kind") orelse continue;
                        const md_v = co.get("model") orelse continue;
                        const ia_v = co.get("is_active") orelse continue;
                        if (pn_v != .string or kn_v != .string or md_v != .string or ia_v != .bool) continue;
                        const ck = std.meta.stringToEnum(core.ProviderKind, kn_v.string) orelse continue;
                        try choices_list.append(a, commands.ModelChoice{
                            .provider_name = try a.dupe(u8, pn_v.string),
                            .kind = ck,
                            .model = try a.dupe(u8, md_v.string),
                            .is_active = ia_v.bool,
                        });
                    }
                }
                break :blk DispatchResult{ .model_picker = .{
                    .title = title,
                    .choices = try choices_list.toOwnedSlice(a),
                } };
            } else if (std.mem.eql(u8, kind, "inject_context")) {
                const label_v = obj.get("label") orelse break :blk DispatchResult{ .err = "missing label" };
                const body_v = obj.get("body") orelse break :blk DispatchResult{ .err = "missing body" };
                const um_v = obj.get("user_message") orelse break :blk DispatchResult{ .err = "missing user_message" };
                break :blk DispatchResult{ .inject_context = .{
                    .label = if (label_v == .string) try a.dupe(u8, label_v.string) else try a.dupe(u8, ""),
                    .body = if (body_v == .string) try a.dupe(u8, body_v.string) else try a.dupe(u8, ""),
                    .user_message = if (um_v == .string) try a.dupe(u8, um_v.string) else try a.dupe(u8, ""),
                } };
            } else {
                break :blk DispatchResult{ .err = try std.fmt.allocPrint(a, "unknown kind: {s}", .{kind}) };
            }
        };

        return .{ .result = dr, .response = resp.response };
    }

    pub fn applyModelChoice(self: *Client, choice: commands.ModelChoice) !struct { result: ApplyResult, response: Response } {
        var params_buf: std.ArrayList(u8) = .empty;
        defer params_buf.deinit(self.gpa);
        try params_buf.appendSlice(self.gpa, "{\"provider_name\":");
        try writeJsonString(&params_buf, self.gpa, choice.provider_name);
        try params_buf.appendSlice(self.gpa, ",\"kind\":");
        try writeJsonString(&params_buf, self.gpa, @tagName(choice.kind));
        try params_buf.appendSlice(self.gpa, ",\"model\":");
        try writeJsonString(&params_buf, self.gpa, choice.model);
        try params_buf.print(self.gpa, ",\"is_active\":{}}}", .{choice.is_active});

        var resp = try self.callParsed("command.applyModelChoice", params_buf.items);
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

        // Build request line.
        var req: std.ArrayList(u8) = .empty;
        defer req.deinit(self.gpa);
        try req.print(self.gpa, "{{\"id\":{d},\"method\":", .{id});
        try writeJsonString(&req, self.gpa, method);
        if (params_json) |pj| {
            try req.appendSlice(self.gpa, ",\"params\":");
            try req.appendSlice(self.gpa, pj);
        }
        try req.appendSlice(self.gpa, "}\n");

        // Write to child stdin.
        const stdin_fd = self.child.stdin.?.handle;
        try writeAll(stdin_fd, req.items);

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
};

fn writeAll(fd: std.posix.fd_t, data: []const u8) !void {
    var remaining = data;
    while (remaining.len > 0) {
        const rc = std.c.write(fd, remaining.ptr, remaining.len);
        if (rc < 0) return error.WriteError;
        remaining = remaining[@intCast(rc)..];
    }
}

fn writeJsonString(out: *std.ArrayList(u8), a: std.mem.Allocator, s: []const u8) !void {
    const encoded = try std.json.Stringify.valueAlloc(a, s, .{});
    defer a.free(encoded);
    try out.appendSlice(a, encoded);
}

test "client buildRequest encoding" {
    // Unit test: verify request encoding without spawning a child.
    const a = std.testing.allocator;
    var req: std.ArrayList(u8) = .empty;
    defer req.deinit(a);

    const id: i64 = 1;
    try req.print(a, "{{\"id\":{d},\"method\":", .{id});
    try writeJsonString(&req, a, "config.get");
    try req.appendSlice(a, "}\n");

    try std.testing.expectEqualStrings("{\"id\":1,\"method\":\"config.get\"}\n", req.items);
}

test "client buildDispatchRequest encoding" {
    const a = std.testing.allocator;
    var req: std.ArrayList(u8) = .empty;
    defer req.deinit(a);

    const id: i64 = 2;
    try req.print(a, "{{\"id\":{d},\"method\":", .{id});
    try writeJsonString(&req, a, "command.dispatch");
    try req.appendSlice(a, ",\"params\":{\"input\":");
    try writeJsonString(&req, a, "/model");
    try req.appendSlice(a, "}}\n");

    try std.testing.expect(std.mem.indexOf(u8, req.items, "command.dispatch") != null);
    try std.testing.expect(std.mem.indexOf(u8, req.items, "/model") != null);
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
        \\{"id":1,"result":{"kind":"model_picker","title":"Pick","choices":[{"provider_name":"default","kind":"claude","model":"claude-opus-4-7","is_active":true}]}}
    ;
    const parsed = try std.json.parseFromSliceLeaky(std.json.Value, a, line, .{});
    const result = parsed.object.get("result").?;
    const choices = result.object.get("choices").?;
    try std.testing.expect(choices.array.items.len == 1);
    const first = choices.array.items[0];
    try std.testing.expectEqualStrings("claude-opus-4-7", first.object.get("model").?.string);
}
