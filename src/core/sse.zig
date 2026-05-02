/// Line-buffered Server-Sent Events parser.
///
/// Reads from any `*std.Io.Reader` and yields parsed SSE events on blank-line
/// boundaries.
///
/// SSE wire format:
///   event: <name>          (optional; empty string = "message")
///   data: <content>        (may appear multiple times; joined with '\n')
///   : comment              (ignored)
///   retry: <ms>            (ignored)
///                          (blank line dispatches the event)
const std = @import("std");

pub const SseEvent = struct {
    /// `event:` field; empty slice if absent (default event type "message").
    /// Borrowed from the parser's internal buffer; valid until the next
    /// `nextEvent()` call.
    event: []const u8,
    /// Concatenated `data:` lines joined with '\n'.
    /// Borrowed from the parser's internal buffer; valid until the next
    /// `nextEvent()` call.
    data: []const u8,
};

pub const Parser = struct {
    source: *std.Io.Reader,
    allocator: std.mem.Allocator,
    /// Accumulated event name for current event
    event_name: std.ArrayList(u8),
    /// Accumulated data for current event
    data_buf: std.ArrayList(u8),

    pub fn init(allocator: std.mem.Allocator, source: *std.Io.Reader) Parser {
        return Parser{
            .source = source,
            .allocator = allocator,
            .event_name = .empty,
            .data_buf = .empty,
        };
    }

    pub fn deinit(self: *Parser) void {
        self.event_name.deinit(self.allocator);
        self.data_buf.deinit(self.allocator);
    }

    /// Returns the next SSE event, or null at EOF.
    /// The returned `SseEvent` borrows from the parser's internal buffers;
    /// it is only valid until the next call to `nextEvent()`.
    pub fn nextEvent(self: *Parser) !?SseEvent {
        // Reset accumulators for this event
        self.event_name.clearRetainingCapacity();
        self.data_buf.clearRetainingCapacity();

        var got_field = false;

        while (true) {
            // Read one line (up to '\n', not included)
            const maybe_line = self.source.takeDelimiter('\n') catch |err| switch (err) {
                error.ReadFailed => return error.ReadFailed,
                error.StreamTooLong => return error.StreamTooLong,
            };
            const raw_line = maybe_line orelse {
                // EOF
                if (got_field and self.data_buf.items.len > 0) {
                    // Dispatch event accumulated so far
                    return SseEvent{
                        .event = self.event_name.items,
                        .data = self.data_buf.items,
                    };
                }
                return null;
            };

            // Strip trailing '\r' if present (CRLF line endings)
            const line = if (raw_line.len > 0 and raw_line[raw_line.len - 1] == '\r')
                raw_line[0 .. raw_line.len - 1]
            else
                raw_line;

            if (line.len == 0) {
                // Blank line: dispatch event if we have data
                if (got_field and self.data_buf.items.len > 0) {
                    return SseEvent{
                        .event = self.event_name.items,
                        .data = self.data_buf.items,
                    };
                }
                // Otherwise reset and continue (e.g. leading blank lines)
                self.event_name.clearRetainingCapacity();
                self.data_buf.clearRetainingCapacity();
                got_field = false;
                continue;
            }

            // Skip comment lines
            if (line[0] == ':') continue;

            // Parse field: value
            const colon_pos = std.mem.indexOfScalar(u8, line, ':');
            if (colon_pos) |pos| {
                const field = line[0..pos];
                // Value: skip one leading space if present
                const raw_value = line[pos + 1 ..];
                const value = if (raw_value.len > 0 and raw_value[0] == ' ')
                    raw_value[1..]
                else
                    raw_value;

                if (std.mem.eql(u8, field, "event")) {
                    self.event_name.clearRetainingCapacity();
                    try self.event_name.appendSlice(self.allocator, value);
                    got_field = true;
                } else if (std.mem.eql(u8, field, "data")) {
                    if (self.data_buf.items.len > 0) {
                        try self.data_buf.append(self.allocator, '\n');
                    }
                    try self.data_buf.appendSlice(self.allocator, value);
                    got_field = true;
                }
                // retry: and other fields are ignored
            }
            // Lines without a colon (field names with no value) are ignored
        }
    }
};

// ---- Tests ----

test "single data event" {
    const allocator = std.testing.allocator;
    const input = "data: hello\n\n";
    var fixed = std.Io.Reader.fixed(input);
    var parser = Parser.init(allocator, &fixed);
    defer parser.deinit();

    const ev = (try parser.nextEvent()).?;
    try std.testing.expectEqualStrings("", ev.event);
    try std.testing.expectEqualStrings("hello", ev.data);

    // EOF
    const next = try parser.nextEvent();
    try std.testing.expect(next == null);
}

test "named event with multi-line data" {
    const allocator = std.testing.allocator;
    const input = "event: message_delta\ndata: {\"a\":\ndata: 1}\n\n";
    var fixed = std.Io.Reader.fixed(input);
    var parser = Parser.init(allocator, &fixed);
    defer parser.deinit();

    const ev = (try parser.nextEvent()).?;
    try std.testing.expectEqualStrings("message_delta", ev.event);
    try std.testing.expectEqualStrings("{\"a\":\n1}", ev.data);
}

test "comment lines ignored" {
    const allocator = std.testing.allocator;
    const input = ": this is a comment\ndata: value\n\n";
    var fixed = std.Io.Reader.fixed(input);
    var parser = Parser.init(allocator, &fixed);
    defer parser.deinit();

    const ev = (try parser.nextEvent()).?;
    try std.testing.expectEqualStrings("", ev.event);
    try std.testing.expectEqualStrings("value", ev.data);
}

test "EOF mid-event yields null" {
    const allocator = std.testing.allocator;
    // Incomplete event (no blank line to dispatch, no data line)
    const input = ": comment only";
    var fixed = std.Io.Reader.fixed(input);
    var parser = Parser.init(allocator, &fixed);
    defer parser.deinit();

    // No data accumulated, so should yield null
    const ev = try parser.nextEvent();
    try std.testing.expect(ev == null);
}

test "multiple events in sequence" {
    const allocator = std.testing.allocator;
    const input = "data: first\n\ndata: second\n\n";
    var fixed = std.Io.Reader.fixed(input);
    var parser = Parser.init(allocator, &fixed);
    defer parser.deinit();

    const ev1 = (try parser.nextEvent()).?;
    try std.testing.expectEqualStrings("first", ev1.data);

    const ev2 = (try parser.nextEvent()).?;
    try std.testing.expectEqualStrings("second", ev2.data);

    try std.testing.expect(try parser.nextEvent() == null);
}

test "retry field ignored" {
    const allocator = std.testing.allocator;
    const input = "retry: 3000\ndata: payload\n\n";
    var fixed = std.Io.Reader.fixed(input);
    var parser = Parser.init(allocator, &fixed);
    defer parser.deinit();

    const ev = (try parser.nextEvent()).?;
    try std.testing.expectEqualStrings("payload", ev.data);
}

test "CRLF line endings" {
    const allocator = std.testing.allocator;
    const input = "data: crlf\r\n\r\n";
    var fixed = std.Io.Reader.fixed(input);
    var parser = Parser.init(allocator, &fixed);
    defer parser.deinit();

    const ev = (try parser.nextEvent()).?;
    try std.testing.expectEqualStrings("crlf", ev.data);
}
