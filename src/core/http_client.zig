/// Thin HTTP transport wrapper used by provider adapters.
///
/// The public surface is a `Transport` interface so adapters can be tested
/// without a real network by swapping in a `MockTransport`.
const std = @import("std");

pub const Header = struct {
    name: []const u8,
    value: []const u8,
};

pub const PostOptions = struct {
    url: []const u8,
    body: []const u8,
    headers: []const Header = &.{},
    timeout_ms: u64 = 120_000,
};

/// Response from a POST call. The caller reads the body via `reader()` and
/// must call `deinit()` when done.
pub const Response = struct {
    status: u16,
    /// Owned arena that holds all response-internal allocations (body copy for
    /// mock, reader state, etc.). Freed by deinit.
    arena: std.heap.ArenaAllocator,
    /// Mutable reader the caller streams the body from.
    body_reader: *std.Io.Reader,

    pub fn reader(self: *Response) *std.Io.Reader {
        return self.body_reader;
    }

    pub fn deinit(self: *Response) void {
        self.arena.deinit();
    }
};

/// Pluggable transport. Tests swap in MockTransport; production uses HttpTransport.
pub const Transport = struct {
    state: *anyopaque,
    postFn: *const fn (
        state: *anyopaque,
        allocator: std.mem.Allocator,
        opts: PostOptions,
    ) anyerror!Response,

    pub fn post(self: Transport, allocator: std.mem.Allocator, opts: PostOptions) !Response {
        return self.postFn(self.state, allocator, opts);
    }
};

/// Real HTTP transport backed by std.http.Client.
/// One instance per Provider; holds a persistent connection pool.
pub const HttpTransport = struct {
    client: std.http.Client,

    pub fn init(allocator: std.mem.Allocator, io: std.Io) HttpTransport {
        return .{ .client = .{ .allocator = allocator, .io = io } };
    }

    pub fn deinit(self: *HttpTransport) void {
        self.client.deinit();
    }

    pub fn transport(self: *HttpTransport) Transport {
        return .{
            .state = self,
            .postFn = httpPost,
        };
    }

    fn httpPost(
        state: *anyopaque,
        allocator: std.mem.Allocator,
        opts: PostOptions,
    ) anyerror!Response {
        const self: *HttpTransport = @ptrCast(@alignCast(state));

        const uri = try std.Uri.parse(opts.url);

        // Build extra_headers from opts.headers, adding Content-Type and Accept
        // defaults if not overridden by caller.
        var hdrs: std.ArrayList(std.http.Header) = .empty;
        defer hdrs.deinit(allocator);

        var has_content_type = false;
        var has_accept = false;
        for (opts.headers) |h| {
            if (std.ascii.eqlIgnoreCase(h.name, "content-type")) has_content_type = true;
            if (std.ascii.eqlIgnoreCase(h.name, "accept")) has_accept = true;
            try hdrs.append(allocator, .{ .name = h.name, .value = h.value });
        }
        if (!has_content_type) {
            try hdrs.append(allocator, .{ .name = "Content-Type", .value = "application/json" });
        }
        if (!has_accept) {
            try hdrs.append(allocator, .{ .name = "Accept", .value = "text/event-stream" });
        }

        var req = try self.client.request(.POST, uri, .{
            .extra_headers = hdrs.items,
            .keep_alive = false,
        });
        defer req.deinit();

        req.transfer_encoding = .{ .content_length = opts.body.len };
        var bw = try req.sendBodyUnflushed(&.{});
        try bw.writer.writeAll(@constCast(opts.body));
        try bw.end();
        try req.connection.?.flush();

        // Use an 8KB redirect buffer
        var redirect_buf: [8192]u8 = undefined;
        var response = try req.receiveHead(&redirect_buf);

        const status_code: u16 = @intFromEnum(response.head.status);

        // Read the entire body into an arena so deinit can free it later.
        var arena = std.heap.ArenaAllocator.init(allocator);
        errdefer arena.deinit();
        const a = arena.allocator();

        // Allocate a transfer buffer for the reader
        const transfer_buf = try a.alloc(u8, 8192);
        const body_reader_ptr = response.reader(transfer_buf);

        // Read entire body
        const body_data = try body_reader_ptr.allocRemaining(a, .unlimited);

        // Replace reader with a fixed reader over the copied body
        const fixed_reader_ptr = try a.create(std.Io.Reader);
        fixed_reader_ptr.* = std.Io.Reader.fixed(body_data);

        return Response{
            .status = status_code,
            .arena = arena,
            .body_reader = fixed_reader_ptr,
        };
    }
};

/// Convenience function: one-shot POST using a temporary HttpTransport.
/// Not suitable for production use (no connection reuse); use HttpTransport
/// directly in providers.
pub fn post(allocator: std.mem.Allocator, io: std.Io, options: PostOptions) !Response {
    var t = HttpTransport.init(allocator, io);
    defer t.deinit();
    return t.transport().post(allocator, options);
}

// ---- MockTransport (always compiled in; used by tests in other files too) ----

/// A canned response to return from MockTransport.post.
pub const Canned = struct {
    status: u16 = 200,
    headers: []const Header = &.{},
    body: []const u8,
};

/// Request captured by MockTransport for assertion.
pub const CapturedRequest = struct {
    url: []u8,
    body: []u8,
    headers: []Header,
};

/// Mock transport for tests. Returns pre-canned responses in order.
/// Errors if called more times than there are canned responses.
pub const MockTransport = struct {
    responses: []const Canned,
    captured: std.ArrayList(CapturedRequest),
    cursor: usize,
    allocator: std.mem.Allocator,

    pub fn init(allocator: std.mem.Allocator, responses: []const Canned) MockTransport {
        return .{
            .responses = responses,
            .captured = .empty,
            .cursor = 0,
            .allocator = allocator,
        };
    }

    pub fn deinit(self: *MockTransport) void {
        for (self.captured.items) |cap| {
            self.allocator.free(cap.url);
            self.allocator.free(cap.body);
            for (cap.headers) |h| {
                self.allocator.free(h.name);
                self.allocator.free(h.value);
            }
            self.allocator.free(cap.headers);
        }
        self.captured.deinit(self.allocator);
    }

    pub fn transport(self: *MockTransport) Transport {
        return .{
            .state = self,
            .postFn = mockPost,
        };
    }

    fn mockPost(
        state: *anyopaque,
        allocator: std.mem.Allocator,
        opts: PostOptions,
    ) anyerror!Response {
        const self: *MockTransport = @ptrCast(@alignCast(state));

        if (self.cursor >= self.responses.len) return error.NoMoreResponses;

        const canned = self.responses[self.cursor];
        self.cursor += 1;

        // Capture the request
        const url_dup = try self.allocator.dupe(u8, opts.url);
        errdefer self.allocator.free(url_dup);
        const body_dup = try self.allocator.dupe(u8, opts.body);
        errdefer self.allocator.free(body_dup);

        const hdrs = try self.allocator.alloc(Header, opts.headers.len);
        errdefer self.allocator.free(hdrs);
        for (opts.headers, 0..) |h, i| {
            hdrs[i] = .{
                .name = try self.allocator.dupe(u8, h.name),
                .value = try self.allocator.dupe(u8, h.value),
            };
        }
        try self.captured.append(self.allocator, .{
            .url = url_dup,
            .body = body_dup,
            .headers = hdrs,
        });

        // Build Response: arena owns the fixed reader
        var arena = std.heap.ArenaAllocator.init(allocator);
        errdefer arena.deinit();
        const a = arena.allocator();

        const body_copy = try a.dupe(u8, canned.body);
        const reader_ptr = try a.create(std.Io.Reader);
        reader_ptr.* = std.Io.Reader.fixed(body_copy);

        return Response{
            .status = canned.status,
            .arena = arena,
            .body_reader = reader_ptr,
        };
    }
};

// ---- Tests ----

test "MockTransport round-trip" {
    const allocator = std.testing.allocator;
    const canned = [_]Canned{
        .{ .status = 200, .body = "hello world" },
    };
    var mock = MockTransport.init(allocator, &canned);
    defer mock.deinit();

    var resp = try mock.transport().post(allocator, .{
        .url = "http://example.com/v1/chat",
        .body = "{\"model\":\"test\"}",
        .headers = &.{.{ .name = "Authorization", .value = "Bearer key123" }},
    });
    defer resp.deinit();

    try std.testing.expectEqual(@as(u16, 200), resp.status);

    // Read the body
    const body = try resp.reader().allocRemaining(allocator, .unlimited);
    defer allocator.free(body);
    try std.testing.expectEqualStrings("hello world", body);

    // Check captured request
    try std.testing.expectEqual(@as(usize, 1), mock.captured.items.len);
    try std.testing.expectEqualStrings("http://example.com/v1/chat", mock.captured.items[0].url);
    try std.testing.expectEqualStrings("{\"model\":\"test\"}", mock.captured.items[0].body);
    try std.testing.expectEqual(@as(usize, 1), mock.captured.items[0].headers.len);
    try std.testing.expectEqualStrings("Authorization", mock.captured.items[0].headers[0].name);
}

test "MockTransport multi-response" {
    const allocator = std.testing.allocator;
    const canned = [_]Canned{
        .{ .status = 200, .body = "first" },
        .{ .status = 201, .body = "second" },
    };
    var mock = MockTransport.init(allocator, &canned);
    defer mock.deinit();

    var resp1 = try mock.transport().post(allocator, .{
        .url = "http://example.com/a",
        .body = "req1",
    });
    defer resp1.deinit();
    const body1 = try resp1.reader().allocRemaining(allocator, .unlimited);
    defer allocator.free(body1);
    try std.testing.expectEqualStrings("first", body1);

    var resp2 = try mock.transport().post(allocator, .{
        .url = "http://example.com/b",
        .body = "req2",
    });
    defer resp2.deinit();
    const body2 = try resp2.reader().allocRemaining(allocator, .unlimited);
    defer allocator.free(body2);
    try std.testing.expectEqualStrings("second", body2);

    try std.testing.expectEqual(@as(usize, 2), mock.captured.items.len);
    try std.testing.expectEqualStrings("http://example.com/a", mock.captured.items[0].url);
    try std.testing.expectEqualStrings("http://example.com/b", mock.captured.items[1].url);
}

test "MockTransport over-consumption errors" {
    const allocator = std.testing.allocator;
    const canned = [_]Canned{
        .{ .status = 200, .body = "only one" },
    };
    var mock = MockTransport.init(allocator, &canned);
    defer mock.deinit();

    var resp = try mock.transport().post(allocator, .{ .url = "http://x.com", .body = "" });
    defer resp.deinit();

    // This second call should fail
    const result = mock.transport().post(allocator, .{ .url = "http://x.com", .body = "" });
    try std.testing.expectError(error.NoMoreResponses, result);
}
