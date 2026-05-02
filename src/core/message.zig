pub const Role = enum {
    system,
    user,
    assistant,
    tool_call,
    tool_result,
};

pub const Message = struct {
    role: Role,
    content: []const u8,
};
