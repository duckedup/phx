const std = @import("std");
const core = @import("phoenix_core");
const commands = @import("commands");
const protocol = @import("protocol.zig");

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

    var read_buf: [4096]u8 = undefined;

    while (true) {
        const n = try std.posix.read(stdin_fd, &read_buf);
        if (n == 0) {
            if (line_buf.items.len > 0) {
                const line = std.mem.trim(u8, line_buf.items, " \t\r");
                if (line.len > 0) {
                    try processLine(gpa, io, config, home, stdout_fd, line);
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
                    try processLine(gpa, io, config, home, stdout_fd, line);
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

    const dp = config.provider("default");
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
        .provider_name = p.provider_name,
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

    const dp = ctx.config.provider("default") orelse {
        try protocol.writeError(resp, gpa, id, .InternalError, "default provider missing after apply");
        return;
    };

    var body: std.ArrayList(u8) = .empty;
    defer body.deinit(a);
    try protocol.writeApplyModelChoiceResult(&body, a, msg, .{ .kind = dp.kind, .model = dp.model });
    try protocol.writeSuccess(resp, gpa, id, body.items);
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
