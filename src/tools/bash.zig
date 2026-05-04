const std = @import("std");
const core = @import("phoenix_core");

const max_bytes: usize = 512 * 1024;

fn errResult(allocator: std.mem.Allocator, comptime fmt: []const u8, args: anytype) anyerror!core.tool.ToolResult {
    const msg = try std.fmt.allocPrint(allocator, fmt, args);
    return .{ .output = msg, .is_error = true };
}

fn invoke(self: *const core.Tool, io: std.Io, args: []const u8, allocator: std.mem.Allocator) anyerror!core.tool.ToolResult {
    _ = self;

    var parsed = std.json.parseFromSlice(std.json.Value, allocator, args, .{}) catch {
        return errResult(allocator, "bash: invalid JSON args: {s}", .{args});
    };
    defer parsed.deinit();
    if (parsed.value != .object) return errResult(allocator, "bash: args must be a JSON object", .{});
    const obj = parsed.value.object;

    const cmd_v = obj.get("command") orelse return errResult(allocator, "bash: missing required field 'command'", .{});
    if (cmd_v != .string) return errResult(allocator, "bash: 'command' must be a string", .{});
    const command = cmd_v.string;

    var timeout_secs: ?i64 = null;
    if (obj.get("timeout")) |v| {
        if (v != .integer or v.integer < 1) return errResult(allocator, "bash: 'timeout' must be a positive integer (seconds)", .{});
        timeout_secs = v.integer;
    }

    const timeout: std.Io.Timeout = if (timeout_secs) |s|
        .{ .duration = .{ .clock = .awake, .raw = .fromSeconds(s) } }
    else
        .none;

    const result = std.process.run(allocator, io, .{
        .argv = &.{ "/bin/sh", "-c", command },
        .stdout_limit = .limited(max_bytes),
        .stderr_limit = .limited(max_bytes),
        .timeout = timeout,
    }) catch |err| {
        if (err == error.Timeout) {
            const msg = try std.fmt.allocPrint(
                allocator,
                "[timed out after {d}s]\nbash: command did not complete before deadline",
                .{timeout_secs.?},
            );
            return .{ .output = msg, .is_error = true };
        }
        return errResult(allocator, "bash: spawn/run failed: {s}", .{@errorName(err)});
    };
    defer allocator.free(result.stdout);
    defer allocator.free(result.stderr);

    var out: std.ArrayList(u8) = .empty;
    defer out.deinit(allocator);

    switch (result.term) {
        .exited => |code| try out.print(allocator, "exit code: {d}\n", .{code}),
        .signal => |sig| try out.print(allocator, "killed by signal: {s}\n", .{@tagName(sig)}),
        .stopped => |sig| try out.print(allocator, "stopped by signal: {s}\n", .{@tagName(sig)}),
        .unknown => |code| try out.print(allocator, "unknown termination: {d}\n", .{code}),
    }

    if (result.stdout.len > 0) {
        try out.appendSlice(allocator, "--- stdout ---\n");
        try out.appendSlice(allocator, result.stdout);
        if (result.stdout[result.stdout.len - 1] != '\n') try out.append(allocator, '\n');
    }
    if (result.stderr.len > 0) {
        try out.appendSlice(allocator, "--- stderr ---\n");
        try out.appendSlice(allocator, result.stderr);
        if (result.stderr[result.stderr.len - 1] != '\n') try out.append(allocator, '\n');
    }

    const owned = try out.toOwnedSlice(allocator);
    const is_error = switch (result.term) {
        .exited => |code| code != 0,
        else => true,
    };
    return .{ .output = owned, .is_error = is_error };
}

pub const bash_tool: core.Tool = .{
    .name = "bash",
    .description = "Execute a bash command in the current working directory. Returns stdout and stderr. Optionally provide a timeout in seconds.",
    .schema =
    \\{"type":"object","properties":{"command":{"type":"string","description":"Bash command to execute"},"timeout":{"type":"integer","minimum":1,"description":"Timeout in seconds (optional, no default timeout)"}},"required":["command"]}
    ,
    .max_output_bytes = max_bytes,
    .invokeFn = invoke,
};

// ---- Tests ----

test "bash captures stdout" {
    const a = std.testing.allocator;
    const io = std.testing.io;
    const args =
        \\{"command":"printf hello"}
    ;
    const r = try bash_tool.invoke(io, args, a);
    defer a.free(r.output);
    try std.testing.expect(std.mem.indexOf(u8, r.output, "exit code: 0") != null);
    try std.testing.expect(std.mem.indexOf(u8, r.output, "hello") != null);
    try std.testing.expect(!r.is_error);
}

test "bash non-zero exit is error" {
    const a = std.testing.allocator;
    const io = std.testing.io;
    const args =
        \\{"command":"exit 7"}
    ;
    const r = try bash_tool.invoke(io, args, a);
    defer a.free(r.output);
    try std.testing.expect(std.mem.indexOf(u8, r.output, "exit code: 7") != null);
    try std.testing.expect(r.is_error);
}

test "bash captures stderr" {
    const a = std.testing.allocator;
    const io = std.testing.io;
    const args =
        \\{"command":"printf err 1>&2; exit 0"}
    ;
    const r = try bash_tool.invoke(io, args, a);
    defer a.free(r.output);
    try std.testing.expect(std.mem.indexOf(u8, r.output, "stderr") != null);
    try std.testing.expect(std.mem.indexOf(u8, r.output, "err") != null);
}

test "bash timeout kills long-running command" {
    const a = std.testing.allocator;
    const io = std.testing.io;
    const args =
        \\{"command":"sleep 10","timeout":1}
    ;
    const r = try bash_tool.invoke(io, args, a);
    defer a.free(r.output);
    try std.testing.expect(std.mem.indexOf(u8, r.output, "timed out") != null);
    try std.testing.expect(r.is_error);
}

test "bash missing command is error" {
    const a = std.testing.allocator;
    const io = std.testing.io;
    const r = try bash_tool.invoke(io, "{}", a);
    defer a.free(r.output);
    try std.testing.expect(r.is_error);
    try std.testing.expect(std.mem.indexOf(u8, r.output, "missing required field 'command'") != null);
}
