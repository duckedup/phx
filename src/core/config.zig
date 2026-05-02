const std = @import("std");
const toml = @import("toml");
const config_paths = @import("config_paths.zig");
const skills_pkg = @import("skills.zig");

pub const RuntimeMode = enum { tui, rpc };
pub const ProviderKind = enum { claude, openai, ollama, llamacpp };
pub const StoreBackend = enum { memory, beans };

pub const TokenBudget = union(enum) {
    unlimited,
    limit: u64,
};

pub const Runtime = struct {
    mode: RuntimeMode = .tui,
    log_level: std.log.Level = .info,
    max_concurrent_sessions: ?u32 = null,
};

pub const Theme = struct {
    background: ?[]const u8 = null,
    foreground: ?[]const u8 = null,
    accent: ?[]const u8 = null,
    user_bubble_bg: ?[]const u8 = null,
    user_bubble_fg: ?[]const u8 = null,
    assistant_bubble_bg: ?[]const u8 = null,
    assistant_bubble_fg: ?[]const u8 = null,
    system_text: ?[]const u8 = null,
    status_bg: ?[]const u8 = null,
    status_fg: ?[]const u8 = null,
};

pub const AuthEntry = union(enum) {
    inline_value: []const u8,
    env_var: []const u8,
};

pub const AuthConfig = struct {
    entries: std.StringArrayHashMapUnmanaged(AuthEntry) = .empty,

    pub fn resolve(self: *const AuthConfig, allocator: std.mem.Allocator, key: []const u8) !?[]u8 {
        const entry = self.entries.get(key) orelse return null;
        switch (entry) {
            .inline_value => |v| return try allocator.dupe(u8, v),
            .env_var => |env_name| {
                // Use POSIX getenv; copy into allocator
                var buf: [256]u8 = undefined;
                if (env_name.len + 1 > buf.len) return error.OutOfMemory;
                @memcpy(buf[0..env_name.len], env_name);
                buf[env_name.len] = 0;
                const ptr = std.c.getenv(&buf) orelse return null;
                const s = std.mem.span(ptr);
                return try allocator.dupe(u8, s);
            },
        }
    }
};

pub const ProviderProfile = struct {
    kind: ProviderKind = .claude,
    model: []const u8 = "claude-opus-4-7",
    auth: ?[]const u8 = null,
    base_url: ?[]const u8 = null,
    endpoint: ?[]const u8 = null,
    max_retries: u32 = 3,
    retry_base_delay_ms: u64 = 1000,
    retry_max_delay_ms: u64 = 30_000,
    request_timeout_ms: u64 = 120_000,
};

pub const SessionProfile = struct {
    extends: ?[]const u8 = null,
    system_prompt_path: ?[]const u8 = null,
    tools: []const []const u8 = &.{},
    persist: bool = true,
    compaction: []const u8 = "summarize",
    compaction_threshold: f32 = 0.8,
    compaction_tail_turns: u32 = 3,
    token_budget: TokenBudget = .unlimited,
    token_budget_warn: f32 = 0.8,
};

pub const StoreConfig = struct {
    backend: StoreBackend = .memory,
    path: []const u8 = "./.phoenix/store",
};

pub const Config = struct {
    arena: std.heap.ArenaAllocator,

    runtime: Runtime,
    theme: Theme,
    auth: AuthConfig,
    providers: std.StringArrayHashMapUnmanaged(ProviderProfile),
    sessions: std.StringArrayHashMapUnmanaged(SessionProfile),
    store: StoreConfig,
    skills: []skills_pkg.Skill,
    sources: []const []const u8,

    pub const LoadOptions = struct {
        explicit_path: ?[]const u8 = null,
        explicit_dir: ?[]const u8 = null,
        home: ?[]const u8 = null,
        cwd: ?[]const u8 = null,
    };

    pub fn load(gpa: std.mem.Allocator, io: std.Io, opts: LoadOptions) !Config {
        var cfg = Config{
            .arena = std.heap.ArenaAllocator.init(gpa),
            .runtime = .{},
            .theme = .{},
            .auth = .{},
            .providers = .empty,
            .sessions = .empty,
            .store = .{},
            .skills = &.{},
            .sources = &.{},
        };
        errdefer cfg.arena.deinit();
        const a = cfg.arena.allocator();

        // Set default store path (needs to be in arena)
        cfg.store.path = try a.dupe(u8, "./.phoenix/store");

        // Default provider and session
        try cfg.providers.put(a, try a.dupe(u8, "default"), .{
            .model = try a.dupe(u8, "claude-opus-4-7"),
        });
        try cfg.sessions.put(a, try a.dupe(u8, "default"), .{
            .compaction = try a.dupe(u8, "summarize"),
        });

        // Discover config paths
        const disc = config_paths.Discovery{
            .home = opts.home,
            .cwd = opts.cwd,
            .explicit_path = opts.explicit_path,
        };
        var paths = try disc.discover(io, gpa);
        defer paths.deinit(gpa);

        var sources_list: std.ArrayList([]const u8) = .empty;

        // Load each layer in order: user, project, explicit
        const layers: [3]?[]const u8 = .{
            paths.user,
            paths.project,
            paths.explicit,
        };
        const is_project_layer: [3]bool = .{ false, true, false };

        for (layers, is_project_layer) |maybe_path, is_project| {
            const path = maybe_path orelse continue;
            cfg.loadFile(a, io, path, is_project) catch |err| {
                std.log.err("failed to load config file {s}: {}", .{ path, err });
                return err;
            };
            const src_path = try a.dupe(u8, path);
            try sources_list.append(a, src_path);
        }

        cfg.sources = try sources_list.toOwnedSlice(a);

        // Discover skills
        cfg.skills = try skills_pkg.discoverLayered(
            io,
            a,
            paths.user_dir,
            paths.project_dir,
            opts.explicit_dir,
        );

        return cfg;
    }

    pub fn loadDefault(gpa: std.mem.Allocator, io: std.Io) !Config {
        return load(gpa, io, .{});
    }

    pub fn deinit(self: *Config) void {
        self.arena.deinit();
    }

    pub fn provider(self: *const Config, name: []const u8) ?*const ProviderProfile {
        return self.providers.getPtr(name);
    }

    pub fn effectiveConcurrency(self: *const Config) u32 {
        return self.runtime.max_concurrent_sessions orelse blk: {
            const cpu_count = std.Thread.getCpuCount() catch 4;
            break :blk @intCast(cpu_count * 2);
        };
    }

    pub fn resolveSession(self: *Config, name: []const u8) !SessionProfile {
        return resolveSessionInner(self, name, 0);
    }

    fn resolveSessionInner(self: *Config, name: []const u8, depth: u32) !SessionProfile {
        if (depth > 16) return error.CircularExtends;

        const profile = self.sessions.get(name) orelse return error.SessionNotFound;

        if (profile.extends) |parent_name| {
            var parent = try resolveSessionInner(self, parent_name, depth + 1);

            // Merge: child overrides parent scalars
            if (profile.system_prompt_path) |spp| parent.system_prompt_path = spp;
            parent.persist = profile.persist;
            parent.compaction = profile.compaction;
            parent.compaction_threshold = profile.compaction_threshold;
            parent.compaction_tail_turns = profile.compaction_tail_turns;
            parent.token_budget = profile.token_budget;
            parent.token_budget_warn = profile.token_budget_warn;
            parent.extends = null;

            // tools: child appends to parent
            if (profile.tools.len > 0) {
                const combined = try self.arena.allocator().alloc([]const u8, parent.tools.len + profile.tools.len);
                @memcpy(combined[0..parent.tools.len], parent.tools);
                @memcpy(combined[parent.tools.len..], profile.tools);
                parent.tools = combined;
            }

            return parent;
        }

        return profile;
    }

    /// Load a single TOML file into this config. `is_project` controls warnings for raw auth secrets.
    fn loadFile(self: *Config, a: std.mem.Allocator, io: std.Io, path: []const u8, is_project: bool) !void {
        // Read file contents
        const contents = try std.Io.Dir.cwd().readFileAlloc(io, path, a, .limited(2 * 1024 * 1024));
        defer a.free(contents);

        var parser = toml.Parser(toml.Table).init(a);
        defer parser.deinit();

        var result = try parser.parseString(contents);
        defer result.deinit();

        const root = result.value;

        // Walk known root keys
        var it = root.iterator();
        while (it.next()) |entry| {
            const key = entry.key_ptr.*;
            const val = entry.value_ptr.*;

            if (std.mem.eql(u8, key, "runtime")) {
                switch (val) {
                    .table => |t| try self.applyRuntime(a, t),
                    else => {
                        std.log.err("config: [runtime] must be a table", .{});
                        return error.InvalidConfigValue;
                    },
                }
            } else if (std.mem.eql(u8, key, "theme")) {
                switch (val) {
                    .table => |t| try self.applyTheme(a, t),
                    else => {
                        std.log.err("config: [theme] must be a table", .{});
                        return error.InvalidConfigValue;
                    },
                }
            } else if (std.mem.eql(u8, key, "auth")) {
                switch (val) {
                    .table => |t| try self.applyAuth(a, t, is_project),
                    else => {
                        std.log.err("config: [auth] must be a table", .{});
                        return error.InvalidConfigValue;
                    },
                }
            } else if (std.mem.eql(u8, key, "provider")) {
                switch (val) {
                    .table => |t| try self.applyProviders(a, t),
                    else => {
                        std.log.err("config: [provider] must be a table", .{});
                        return error.InvalidConfigValue;
                    },
                }
            } else if (std.mem.eql(u8, key, "session")) {
                switch (val) {
                    .table => |t| try self.applySessions(a, t),
                    else => {
                        std.log.err("config: [session] must be a table", .{});
                        return error.InvalidConfigValue;
                    },
                }
            } else if (std.mem.eql(u8, key, "store")) {
                switch (val) {
                    .table => |t| try self.applyStore(a, t),
                    else => {
                        std.log.err("config: [store] must be a table", .{});
                        return error.InvalidConfigValue;
                    },
                }
            } else {
                std.log.warn("ignoring unknown config key: {s}", .{key});
            }
        }
    }

    fn applyRuntime(self: *Config, a: std.mem.Allocator, t: *toml.Table) !void {
        _ = a;
        var it = t.iterator();
        while (it.next()) |entry| {
            const k = entry.key_ptr.*;
            const v = entry.value_ptr.*;

            if (std.mem.eql(u8, k, "mode")) {
                const s = try expectString(v, "runtime.mode");
                if (std.mem.eql(u8, s, "tui")) {
                    self.runtime.mode = .tui;
                } else if (std.mem.eql(u8, s, "rpc")) {
                    self.runtime.mode = .rpc;
                } else {
                    std.log.err("config: invalid runtime.mode value: {s}", .{s});
                    return error.InvalidConfigValue;
                }
            } else if (std.mem.eql(u8, k, "log_level")) {
                const s = try expectString(v, "runtime.log_level");
                if (std.mem.eql(u8, s, "debug")) {
                    self.runtime.log_level = .debug;
                } else if (std.mem.eql(u8, s, "info")) {
                    self.runtime.log_level = .info;
                } else if (std.mem.eql(u8, s, "warn")) {
                    self.runtime.log_level = .warn;
                } else if (std.mem.eql(u8, s, "err") or std.mem.eql(u8, s, "error")) {
                    self.runtime.log_level = .err;
                } else {
                    std.log.err("config: invalid runtime.log_level value: {s}", .{s});
                    return error.InvalidConfigValue;
                }
            } else if (std.mem.eql(u8, k, "max_concurrent_sessions")) {
                switch (v) {
                    .string => |s| {
                        if (std.mem.eql(u8, s, "auto")) {
                            self.runtime.max_concurrent_sessions = null;
                        } else {
                            std.log.err("config: invalid runtime.max_concurrent_sessions value: {s}", .{s});
                            return error.InvalidConfigValue;
                        }
                    },
                    .integer => |i| {
                        if (i < 0) {
                            std.log.err("config: runtime.max_concurrent_sessions must be non-negative", .{});
                            return error.InvalidConfigValue;
                        }
                        self.runtime.max_concurrent_sessions = @intCast(i);
                    },
                    else => {
                        std.log.err("config: runtime.max_concurrent_sessions must be a string or integer", .{});
                        return error.InvalidConfigValue;
                    },
                }
            } else {
                std.log.warn("ignoring unknown config key: runtime.{s}", .{k});
            }
        }
    }

    fn applyTheme(self: *Config, a: std.mem.Allocator, t: *toml.Table) !void {
        var it = t.iterator();
        while (it.next()) |entry| {
            const k = entry.key_ptr.*;
            const v = entry.value_ptr.*;
            const s = try expectString(v, k);
            const duped = try a.dupe(u8, s);

            if (std.mem.eql(u8, k, "background")) {
                self.theme.background = duped;
            } else if (std.mem.eql(u8, k, "foreground")) {
                self.theme.foreground = duped;
            } else if (std.mem.eql(u8, k, "accent")) {
                self.theme.accent = duped;
            } else if (std.mem.eql(u8, k, "user_bubble_bg")) {
                self.theme.user_bubble_bg = duped;
            } else if (std.mem.eql(u8, k, "user_bubble_fg")) {
                self.theme.user_bubble_fg = duped;
            } else if (std.mem.eql(u8, k, "assistant_bubble_bg")) {
                self.theme.assistant_bubble_bg = duped;
            } else if (std.mem.eql(u8, k, "assistant_bubble_fg")) {
                self.theme.assistant_bubble_fg = duped;
            } else if (std.mem.eql(u8, k, "system_text")) {
                self.theme.system_text = duped;
            } else if (std.mem.eql(u8, k, "status_bg")) {
                self.theme.status_bg = duped;
            } else if (std.mem.eql(u8, k, "status_fg")) {
                self.theme.status_fg = duped;
            } else {
                std.log.warn("ignoring unknown config key: theme.{s}", .{k});
                a.free(duped);
            }
        }
    }

    fn applyAuth(self: *Config, a: std.mem.Allocator, t: *toml.Table, is_project: bool) !void {
        var it = t.iterator();
        while (it.next()) |entry| {
            const k = entry.key_ptr.*;
            const v = entry.value_ptr.*;
            const s = try expectString(v, k);

            // Keys ending in _env are env-var refs; others are inline values
            const is_env = std.mem.endsWith(u8, k, "_env");
            const key_dup = try a.dupe(u8, k);
            errdefer a.free(key_dup);
            const val_dup = try a.dupe(u8, s);
            errdefer a.free(val_dup);

            const auth_entry: AuthEntry = if (is_env)
                .{ .env_var = val_dup }
            else
                .{ .inline_value = val_dup };

            if (!is_env and is_project) {
                std.log.warn("project-level auth contains a raw secret for {s}; prefer *_api_key_env", .{k});
            }

            try self.auth.entries.put(a, key_dup, auth_entry);
        }
    }

    fn applyProviders(self: *Config, a: std.mem.Allocator, t: *toml.Table) !void {
        var it = t.iterator();
        while (it.next()) |entry| {
            const name = entry.key_ptr.*;
            const val = entry.value_ptr.*;

            switch (val) {
                .table => |provider_table| {
                    var profile = self.providers.get(name) orelse ProviderProfile{};
                    // Ensure model is arena-owned for new profiles
                    if (self.providers.get(name) == null) {
                        profile.model = try a.dupe(u8, profile.model);
                    }
                    try applyProviderTable(a, &profile, provider_table);
                    const name_key = if (self.providers.contains(name))
                        name
                    else
                        try a.dupe(u8, name);
                    try self.providers.put(a, name_key, profile);
                },
                else => {
                    std.log.warn("ignoring unknown config key: provider.{s} (expected table)", .{name});
                },
            }
        }
    }

    fn applySessions(self: *Config, a: std.mem.Allocator, t: *toml.Table) !void {
        var it = t.iterator();
        while (it.next()) |entry| {
            const name = entry.key_ptr.*;
            const val = entry.value_ptr.*;

            switch (val) {
                .table => |session_table| {
                    var profile = self.sessions.get(name) orelse SessionProfile{
                        .compaction = try a.dupe(u8, "summarize"),
                    };
                    try applySessionTable(a, &profile, session_table);
                    const name_key = if (self.sessions.contains(name))
                        name
                    else
                        try a.dupe(u8, name);
                    try self.sessions.put(a, name_key, profile);
                },
                else => {
                    std.log.warn("ignoring unknown config key: session.{s} (expected table)", .{name});
                },
            }
        }
    }

    fn applyStore(self: *Config, a: std.mem.Allocator, t: *toml.Table) !void {
        var it = t.iterator();
        while (it.next()) |entry| {
            const k = entry.key_ptr.*;
            const v = entry.value_ptr.*;

            if (std.mem.eql(u8, k, "backend")) {
                const s = try expectString(v, "store.backend");
                if (std.mem.eql(u8, s, "memory")) {
                    self.store.backend = .memory;
                } else if (std.mem.eql(u8, s, "beans")) {
                    self.store.backend = .beans;
                } else {
                    std.log.err("config: invalid store.backend value: {s}", .{s});
                    return error.InvalidConfigValue;
                }
            } else if (std.mem.eql(u8, k, "path")) {
                const s = try expectString(v, "store.path");
                self.store.path = try a.dupe(u8, s);
            } else {
                std.log.warn("ignoring unknown config key: store.{s}", .{k});
            }
        }
    }
};

fn applyProviderTable(a: std.mem.Allocator, profile: *ProviderProfile, t: *toml.Table) !void {
    var it = t.iterator();
    while (it.next()) |entry| {
        const k = entry.key_ptr.*;
        const v = entry.value_ptr.*;

        if (std.mem.eql(u8, k, "kind")) {
            const s = try expectString(v, "provider.kind");
            if (std.mem.eql(u8, s, "claude")) {
                profile.kind = .claude;
            } else if (std.mem.eql(u8, s, "openai")) {
                profile.kind = .openai;
            } else if (std.mem.eql(u8, s, "ollama")) {
                profile.kind = .ollama;
            } else if (std.mem.eql(u8, s, "llamacpp")) {
                profile.kind = .llamacpp;
            } else {
                std.log.err("config: invalid provider.kind value: {s}", .{s});
                return error.InvalidConfigValue;
            }
        } else if (std.mem.eql(u8, k, "model")) {
            profile.model = try a.dupe(u8, try expectString(v, "provider.model"));
        } else if (std.mem.eql(u8, k, "auth")) {
            profile.auth = try a.dupe(u8, try expectString(v, "provider.auth"));
        } else if (std.mem.eql(u8, k, "base_url")) {
            profile.base_url = try a.dupe(u8, try expectString(v, "provider.base_url"));
        } else if (std.mem.eql(u8, k, "endpoint")) {
            profile.endpoint = try a.dupe(u8, try expectString(v, "provider.endpoint"));
        } else if (std.mem.eql(u8, k, "max_retries")) {
            const i = try expectInteger(v, "provider.max_retries");
            profile.max_retries = @intCast(i);
        } else if (std.mem.eql(u8, k, "retry_base_delay_ms")) {
            const i = try expectInteger(v, "provider.retry_base_delay_ms");
            profile.retry_base_delay_ms = @intCast(i);
        } else if (std.mem.eql(u8, k, "retry_max_delay_ms")) {
            const i = try expectInteger(v, "provider.retry_max_delay_ms");
            profile.retry_max_delay_ms = @intCast(i);
        } else if (std.mem.eql(u8, k, "request_timeout_ms")) {
            const i = try expectInteger(v, "provider.request_timeout_ms");
            profile.request_timeout_ms = @intCast(i);
        } else {
            std.log.warn("ignoring unknown config key: provider.<name>.{s}", .{k});
        }
    }
}

fn applySessionTable(a: std.mem.Allocator, profile: *SessionProfile, t: *toml.Table) !void {
    var it = t.iterator();
    while (it.next()) |entry| {
        const k = entry.key_ptr.*;
        const v = entry.value_ptr.*;

        if (std.mem.eql(u8, k, "extends")) {
            profile.extends = try a.dupe(u8, try expectString(v, "session.extends"));
        } else if (std.mem.eql(u8, k, "system_prompt_path")) {
            profile.system_prompt_path = try a.dupe(u8, try expectString(v, "session.system_prompt_path"));
        } else if (std.mem.eql(u8, k, "tools")) {
            const new_tools = try expectStringArray(a, v, "session.tools");
            if (profile.tools.len == 0) {
                profile.tools = new_tools;
            } else {
                // Append new tools to existing tools
                const combined = try a.alloc([]const u8, profile.tools.len + new_tools.len);
                @memcpy(combined[0..profile.tools.len], profile.tools);
                @memcpy(combined[profile.tools.len..], new_tools);
                profile.tools = combined;
            }
        } else if (std.mem.eql(u8, k, "persist")) {
            profile.persist = try expectBool(v, "session.persist");
        } else if (std.mem.eql(u8, k, "compaction")) {
            profile.compaction = try a.dupe(u8, try expectString(v, "session.compaction"));
        } else if (std.mem.eql(u8, k, "compaction_threshold")) {
            const f = try expectFloat(v, "session.compaction_threshold");
            if (f < 0.0 or f > 1.0) {
                std.log.warn("config: session.compaction_threshold {d} outside [0.0, 1.0]", .{f});
            }
            profile.compaction_threshold = @floatCast(f);
        } else if (std.mem.eql(u8, k, "compaction_tail_turns")) {
            const i = try expectInteger(v, "session.compaction_tail_turns");
            profile.compaction_tail_turns = @intCast(i);
        } else if (std.mem.eql(u8, k, "token_budget")) {
            switch (v) {
                .string => |s| {
                    if (std.mem.eql(u8, s, "unlimited")) {
                        profile.token_budget = .unlimited;
                    } else {
                        std.log.err("config: invalid session.token_budget value: {s}", .{s});
                        return error.InvalidConfigValue;
                    }
                },
                .integer => |i| {
                    profile.token_budget = .{ .limit = @intCast(i) };
                },
                else => {
                    std.log.err("config: session.token_budget must be \"unlimited\" or integer", .{});
                    return error.InvalidConfigValue;
                },
            }
        } else if (std.mem.eql(u8, k, "token_budget_warn")) {
            const f = try expectFloat(v, "session.token_budget_warn");
            if (f < 0.0 or f > 1.0) {
                std.log.warn("config: session.token_budget_warn {d} outside [0.0, 1.0]", .{f});
            }
            profile.token_budget_warn = @floatCast(f);
        } else {
            std.log.warn("ignoring unknown config key: session.<name>.{s}", .{k});
        }
    }
}

fn expectString(v: toml.Value, field: []const u8) ![]const u8 {
    return switch (v) {
        .string => |s| s,
        else => {
            std.log.err("config: {s} must be a string", .{field});
            return error.InvalidConfigValue;
        },
    };
}

fn expectInteger(v: toml.Value, field: []const u8) !i64 {
    return switch (v) {
        .integer => |i| i,
        else => {
            std.log.err("config: {s} must be an integer", .{field});
            return error.InvalidConfigValue;
        },
    };
}

fn expectFloat(v: toml.Value, field: []const u8) !f64 {
    return switch (v) {
        .float => |f| f,
        .integer => |i| @floatFromInt(i),
        else => {
            std.log.err("config: {s} must be a float", .{field});
            return error.InvalidConfigValue;
        },
    };
}

fn expectBool(v: toml.Value, field: []const u8) !bool {
    return switch (v) {
        .boolean => |b| b,
        else => {
            std.log.err("config: {s} must be a boolean", .{field});
            return error.InvalidConfigValue;
        },
    };
}

fn expectStringArray(a: std.mem.Allocator, v: toml.Value, field: []const u8) ![]const []const u8 {
    const arr = switch (v) {
        .array => |ar| ar,
        else => {
            std.log.err("config: {s} must be an array", .{field});
            return error.InvalidConfigValue;
        },
    };

    const result = try a.alloc([]const u8, arr.items.len);
    for (arr.items, 0..) |item, i| {
        result[i] = switch (item) {
            .string => |s| try a.dupe(u8, s),
            else => {
                std.log.err("config: {s} must be an array of strings", .{field});
                return error.InvalidConfigValue;
            },
        };
    }
    return result;
}

// ---- Tests ----

test "defaults: no files, sources=0, skills=0" {
    const allocator = std.testing.allocator;
    var tmp = std.testing.tmpDir(.{});
    defer tmp.cleanup();

    const tmp_path = try getTmpDirPath(allocator, &tmp);
    defer allocator.free(tmp_path);

    var cfg = try Config.load(allocator, std.testing.io, .{
        .home = tmp_path,
        .cwd = tmp_path,
    });
    defer cfg.deinit();

    try std.testing.expectEqual(@as(usize, 0), cfg.sources.len);
    try std.testing.expectEqual(@as(usize, 0), cfg.skills.len);
    try std.testing.expect(cfg.provider("default") != null);
    try std.testing.expectEqualStrings("claude-opus-4-7", cfg.provider("default").?.model);
    try std.testing.expect(cfg.runtime.mode == .tui);
    try std.testing.expect(cfg.store.backend == .memory);
}

test "scalar replace: provider model overridden" {
    const allocator = std.testing.allocator;
    var tmp = std.testing.tmpDir(.{});
    defer tmp.cleanup();

    const tmp_path = try getTmpDirPath(allocator, &tmp);
    defer allocator.free(tmp_path);

    const home = try std.fs.path.join(allocator, &.{ tmp_path, "home" });
    defer allocator.free(home);
    const cwd = try std.fs.path.join(allocator, &.{ tmp_path, "cwd" });
    defer allocator.free(cwd);
    try std.Io.Dir.cwd().createDirPath(std.testing.io, home);
    try std.Io.Dir.cwd().createDirPath(std.testing.io, cwd);

    const phoenix_dir = try std.fs.path.join(allocator, &.{ home, ".phoenix" });
    defer allocator.free(phoenix_dir);
    try std.Io.Dir.cwd().createDirPath(std.testing.io, phoenix_dir);

    const toml_path = try std.fs.path.join(allocator, &.{ phoenix_dir, "phoenix.toml" });
    defer allocator.free(toml_path);
    const content =
        \\[provider.default]
        \\model = "my-model"
        \\
    ;
    try std.Io.Dir.cwd().writeFile(std.testing.io, .{
        .sub_path = toml_path,
        .data = content,
    });

    var cfg = try Config.load(allocator, std.testing.io, .{
        .home = home,
        .cwd = cwd,
    });
    defer cfg.deinit();

    try std.testing.expectEqualStrings("my-model", cfg.provider("default").?.model);
    try std.testing.expectEqual(@as(usize, 1), cfg.sources.len);
}

test "list append: tools accumulate across layers" {
    const allocator = std.testing.allocator;
    var tmp = std.testing.tmpDir(.{});
    defer tmp.cleanup();

    const tmp_path = try getTmpDirPath(allocator, &tmp);
    defer allocator.free(tmp_path);

    const home = try std.fs.path.join(allocator, &.{ tmp_path, "home" });
    defer allocator.free(home);
    const cwd = try std.fs.path.join(allocator, &.{ tmp_path, "project" });
    defer allocator.free(cwd);

    const home_phoenix = try std.fs.path.join(allocator, &.{ home, ".phoenix" });
    defer allocator.free(home_phoenix);
    try std.Io.Dir.cwd().createDirPath(std.testing.io, home);
    try std.Io.Dir.cwd().createDirPath(std.testing.io, home_phoenix);

    const user_toml = try std.fs.path.join(allocator, &.{ home_phoenix, "phoenix.toml" });
    defer allocator.free(user_toml);
    try std.Io.Dir.cwd().writeFile(std.testing.io, .{
        .sub_path = user_toml,
        .data =
        \\[session.default]
        \\tools = ["read_file"]
        \\
        ,
    });

    const proj_phoenix = try std.fs.path.join(allocator, &.{ cwd, ".phoenix" });
    defer allocator.free(proj_phoenix);
    try std.Io.Dir.cwd().createDirPath(std.testing.io, cwd);
    try std.Io.Dir.cwd().createDirPath(std.testing.io, proj_phoenix);

    const proj_toml = try std.fs.path.join(allocator, &.{ proj_phoenix, "phoenix.toml" });
    defer allocator.free(proj_toml);
    try std.Io.Dir.cwd().writeFile(std.testing.io, .{
        .sub_path = proj_toml,
        .data =
        \\[session.default]
        \\tools = ["run_shell"]
        \\
        ,
    });

    var cfg = try Config.load(allocator, std.testing.io, .{
        .home = home,
        .cwd = cwd,
    });
    defer cfg.deinit();

    const sess = cfg.sessions.get("default").?;
    try std.testing.expectEqual(@as(usize, 2), sess.tools.len);
    try std.testing.expectEqualStrings("read_file", sess.tools[0]);
    try std.testing.expectEqualStrings("run_shell", sess.tools[1]);
}

test "provider scalar replace across layers" {
    const allocator = std.testing.allocator;
    var tmp = std.testing.tmpDir(.{});
    defer tmp.cleanup();

    const tmp_path = try getTmpDirPath(allocator, &tmp);
    defer allocator.free(tmp_path);

    const home = try std.fs.path.join(allocator, &.{ tmp_path, "home" });
    defer allocator.free(home);
    const cwd = try std.fs.path.join(allocator, &.{ tmp_path, "project" });
    defer allocator.free(cwd);

    const home_phoenix = try std.fs.path.join(allocator, &.{ home, ".phoenix" });
    defer allocator.free(home_phoenix);
    try std.Io.Dir.cwd().createDirPath(std.testing.io, home);
    try std.Io.Dir.cwd().createDirPath(std.testing.io, home_phoenix);

    const home_toml = try std.fs.path.join(allocator, &.{ home_phoenix, "phoenix.toml" });
    defer allocator.free(home_toml);
    try std.Io.Dir.cwd().writeFile(std.testing.io, .{
        .sub_path = home_toml,
        .data =
        \\[provider.default]
        \\model = "x"
        \\
        ,
    });

    const proj_phoenix = try std.fs.path.join(allocator, &.{ cwd, ".phoenix" });
    defer allocator.free(proj_phoenix);
    try std.Io.Dir.cwd().createDirPath(std.testing.io, cwd);
    try std.Io.Dir.cwd().createDirPath(std.testing.io, proj_phoenix);

    const proj_toml = try std.fs.path.join(allocator, &.{ proj_phoenix, "phoenix.toml" });
    defer allocator.free(proj_toml);
    try std.Io.Dir.cwd().writeFile(std.testing.io, .{
        .sub_path = proj_toml,
        .data =
        \\[provider.default]
        \\model = "y"
        \\
        ,
    });

    var cfg = try Config.load(allocator, std.testing.io, .{
        .home = home,
        .cwd = cwd,
    });
    defer cfg.deinit();

    try std.testing.expectEqualStrings("y", cfg.provider("default").?.model);
}

test "unknown key warning: no error" {
    const allocator = std.testing.allocator;
    var tmp = std.testing.tmpDir(.{});
    defer tmp.cleanup();

    const tmp_path = try getTmpDirPath(allocator, &tmp);
    defer allocator.free(tmp_path);

    const home = try std.fs.path.join(allocator, &.{ tmp_path, "home" });
    defer allocator.free(home);
    const cwd = try std.fs.path.join(allocator, &.{ tmp_path, "cwd" });
    defer allocator.free(cwd);
    try std.Io.Dir.cwd().createDirPath(std.testing.io, home);
    try std.Io.Dir.cwd().createDirPath(std.testing.io, cwd);

    const phoenix_dir = try std.fs.path.join(allocator, &.{ home, ".phoenix" });
    defer allocator.free(phoenix_dir);
    try std.Io.Dir.cwd().createDirPath(std.testing.io, phoenix_dir);

    const toml_path_unknown = try std.fs.path.join(allocator, &.{ phoenix_dir, "phoenix.toml" });
    defer allocator.free(toml_path_unknown);
    try std.Io.Dir.cwd().writeFile(std.testing.io, .{
        .sub_path = toml_path_unknown,
        .data =
        \\[unknown_section]
        \\foo = "bar"
        \\
        ,
    });

    // Should not error
    var cfg = try Config.load(allocator, std.testing.io, .{
        .home = home,
        .cwd = cwd,
    });
    defer cfg.deinit();
}

test "resolveSession nonexistent returns error" {
    const allocator = std.testing.allocator;
    var cfg = try Config.load(allocator, std.testing.io, .{
        .home = null,
        .cwd = null,
    });
    defer cfg.deinit();

    const result = cfg.resolveSession("nonexistent");
    try std.testing.expectError(error.SessionNotFound, result);
}

test "extends resolution" {
    const allocator = std.testing.allocator;
    var tmp = std.testing.tmpDir(.{});
    defer tmp.cleanup();

    const tmp_path = try getTmpDirPath(allocator, &tmp);
    defer allocator.free(tmp_path);

    const home = try std.fs.path.join(allocator, &.{ tmp_path, "home" });
    defer allocator.free(home);
    const cwd = try std.fs.path.join(allocator, &.{ tmp_path, "cwd" });
    defer allocator.free(cwd);
    try std.Io.Dir.cwd().createDirPath(std.testing.io, home);
    try std.Io.Dir.cwd().createDirPath(std.testing.io, cwd);

    const phoenix_dir = try std.fs.path.join(allocator, &.{ home, ".phoenix" });
    defer allocator.free(phoenix_dir);
    try std.Io.Dir.cwd().createDirPath(std.testing.io, phoenix_dir);

    const toml_path_extends = try std.fs.path.join(allocator, &.{ phoenix_dir, "phoenix.toml" });
    defer allocator.free(toml_path_extends);
    try std.Io.Dir.cwd().writeFile(std.testing.io, .{
        .sub_path = toml_path_extends,
        .data =
        \\[session.base]
        \\tools = ["read_file"]
        \\persist = true
        \\
        \\[session.child]
        \\extends = "base"
        \\tools = ["run_shell"]
        \\
        ,
    });

    var cfg = try Config.load(allocator, std.testing.io, .{
        .home = home,
        .cwd = cwd,
    });
    defer cfg.deinit();

    const resolved = try cfg.resolveSession("child");
    try std.testing.expectEqual(@as(usize, 2), resolved.tools.len);
    try std.testing.expectEqualStrings("read_file", resolved.tools[0]);
    try std.testing.expectEqualStrings("run_shell", resolved.tools[1]);
}

test "circular extends returns error" {
    const allocator = std.testing.allocator;
    var tmp = std.testing.tmpDir(.{});
    defer tmp.cleanup();

    const tmp_path = try getTmpDirPath(allocator, &tmp);
    defer allocator.free(tmp_path);

    const home = try std.fs.path.join(allocator, &.{ tmp_path, "home" });
    defer allocator.free(home);
    const cwd = try std.fs.path.join(allocator, &.{ tmp_path, "cwd" });
    defer allocator.free(cwd);
    try std.Io.Dir.cwd().createDirPath(std.testing.io, home);
    try std.Io.Dir.cwd().createDirPath(std.testing.io, cwd);

    const phoenix_dir = try std.fs.path.join(allocator, &.{ home, ".phoenix" });
    defer allocator.free(phoenix_dir);
    try std.Io.Dir.cwd().createDirPath(std.testing.io, phoenix_dir);

    const toml_path_circular = try std.fs.path.join(allocator, &.{ phoenix_dir, "phoenix.toml" });
    defer allocator.free(toml_path_circular);
    try std.Io.Dir.cwd().writeFile(std.testing.io, .{
        .sub_path = toml_path_circular,
        .data =
        \\[session.a]
        \\extends = "b"
        \\
        \\[session.b]
        \\extends = "a"
        \\
        ,
    });

    var cfg = try Config.load(allocator, std.testing.io, .{
        .home = home,
        .cwd = cwd,
    });
    defer cfg.deinit();

    const result = cfg.resolveSession("a");
    try std.testing.expectError(error.CircularExtends, result);
}

test "raw secret in project warning does not error" {
    const allocator = std.testing.allocator;
    var tmp = std.testing.tmpDir(.{});
    defer tmp.cleanup();

    const tmp_path = try getTmpDirPath(allocator, &tmp);
    defer allocator.free(tmp_path);

    const home = try std.fs.path.join(allocator, &.{ tmp_path, "home" });
    defer allocator.free(home);
    const cwd = try std.fs.path.join(allocator, &.{ tmp_path, "cwd" });
    defer allocator.free(cwd);
    try std.Io.Dir.cwd().createDirPath(std.testing.io, home);
    try std.Io.Dir.cwd().createDirPath(std.testing.io, cwd);

    const phoenix_dir = try std.fs.path.join(allocator, &.{ cwd, ".phoenix" });
    defer allocator.free(phoenix_dir);
    try std.Io.Dir.cwd().createDirPath(std.testing.io, phoenix_dir);

    const toml_path_secret = try std.fs.path.join(allocator, &.{ phoenix_dir, "phoenix.toml" });
    defer allocator.free(toml_path_secret);
    try std.Io.Dir.cwd().writeFile(std.testing.io, .{
        .sub_path = toml_path_secret,
        .data =
        \\[auth]
        \\anthropic_api_key = "sk-secret"
        \\
        ,
    });

    // Should warn but not error (project layer)
    var cfg = try Config.load(allocator, std.testing.io, .{
        .home = home,
        .cwd = cwd,
    });
    defer cfg.deinit();

    const entry = cfg.auth.entries.get("anthropic_api_key");
    try std.testing.expect(entry != null);
    switch (entry.?) {
        .inline_value => |v| try std.testing.expectEqualStrings("sk-secret", v),
        .env_var => unreachable,
    }
}

test "deinit cleans arena (leak hygiene)" {
    const allocator = std.testing.allocator;
    var tmp = std.testing.tmpDir(.{});
    defer tmp.cleanup();

    const tmp_path = try getTmpDirPath(allocator, &tmp);
    defer allocator.free(tmp_path);

    const home = try std.fs.path.join(allocator, &.{ tmp_path, "home" });
    defer allocator.free(home);
    const cwd = try std.fs.path.join(allocator, &.{ tmp_path, "cwd" });
    defer allocator.free(cwd);
    try std.Io.Dir.cwd().createDirPath(std.testing.io, home);
    try std.Io.Dir.cwd().createDirPath(std.testing.io, cwd);

    const phoenix_dir = try std.fs.path.join(allocator, &.{ home, ".phoenix" });
    defer allocator.free(phoenix_dir);
    try std.Io.Dir.cwd().createDirPath(std.testing.io, phoenix_dir);

    const toml_path_leak = try std.fs.path.join(allocator, &.{ phoenix_dir, "phoenix.toml" });
    defer allocator.free(toml_path_leak);
    try std.Io.Dir.cwd().writeFile(std.testing.io, .{
        .sub_path = toml_path_leak,
        .data =
        \\[runtime]
        \\mode = "rpc"
        \\
        \\[provider.default]
        \\model = "test-model"
        \\
        \\[session.default]
        \\tools = ["read_file", "run_shell"]
        \\
        ,
    });

    var cfg = try Config.load(allocator, std.testing.io, .{
        .home = home,
        .cwd = cwd,
    });
    cfg.deinit(); // no leak expected
}

fn getTmpDirPath(allocator: std.mem.Allocator, tmp: *std.testing.TmpDir) ![]u8 {
    var buf: [std.Io.Dir.max_path_bytes]u8 = undefined;
    const ptr = std.c.getcwd(&buf, buf.len) orelse return error.CwdError;
    const cwd_path = std.mem.sliceTo(ptr, 0);
    return try std.fs.path.join(allocator, &.{ cwd_path, ".zig-cache", "tmp", &tmp.sub_path });
}
