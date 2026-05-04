const std = @import("std");
const config_paths = @import("config_paths.zig");
const skills_pkg = @import("skills.zig");
const otel_pkg = @import("otel.zig");

pub const RuntimeMode = enum { tui, rpc };
pub const ProviderKind = enum { claude, openai, ollama, llamacpp, vertex, gemini, nvidia };
pub const StoreBackend = enum { memory, beans };

/// Re-exports so callers can spell config types via `core.config.*`.
pub const OtelConfig = otel_pkg.Config;
pub const OtelHeader = otel_pkg.Header;

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

/// Per-provider auth credential, embedded directly in `ProviderProfile.auth`.
/// In JSONC:
///   "auth": { "env": "ANTHROPIC_API_KEY" }    -- env-var ref
///   "auth": "sk-ant-..."                        -- raw inline secret
pub const AuthEntry = union(enum) {
    inline_value: []const u8,
    env_var: []const u8,

    pub fn resolve(self: AuthEntry, allocator: std.mem.Allocator) !?[]u8 {
        switch (self) {
            .inline_value => |v| return try allocator.dupe(u8, v),
            .env_var => |env_name| {
                var buf: [257]u8 = undefined;
                if (env_name.len + 1 > buf.len) return error.OutOfMemory;
                @memcpy(buf[0..env_name.len], env_name);
                buf[env_name.len] = 0;
                const sentinel: [*:0]const u8 = buf[0..env_name.len :0];
                const ptr = std.c.getenv(sentinel) orelse return null;
                const s = std.mem.span(ptr);
                return try allocator.dupe(u8, s);
            },
        }
    }
};

pub const ProviderProfile = struct {
    kind: ProviderKind = .claude,
    model: []const u8 = "claude-opus-4-7",
    /// Exactly one provider in a Config's providers list should have
    /// `active = true`. When none does, `Config.activeProvider` falls back
    /// to the first entry.
    active: bool = false,
    auth: ?AuthEntry = null,
    base_url: ?[]const u8 = null,
    endpoint: ?[]const u8 = null,
    max_retries: u32 = 3,
    retry_base_delay_ms: u64 = 1000,
    retry_max_delay_ms: u64 = 30_000,
    request_timeout_ms: u64 = 120_000,
    /// OpenAI: "completions" or "responses". Ignored for other kinds.
    api: ?[]const u8 = null,
    /// Google Vertex AI: GCP project ID. Required for kind = .vertex.
    project: ?[]const u8 = null,
    /// Google Vertex AI: GCP region (e.g. "us-central1").
    location: ?[]const u8 = null,
    /// Google Vertex AI: path to service-account/ADC JSON.
    credentials_path: ?[]const u8 = null,
    /// User-set context window (tokens). When null, `model_info.lookup` falls
    /// back to a static table for cloud models. For local providers (ollama,
    /// llamacpp) this is the only source of truth — onboarding prompts for it.
    context_window: ?u32 = null,
};

pub const SessionProfile = struct {
    /// Identifier for `extends` lookups and TUI/RPC routing.
    name: []const u8 = "default",
    extends: ?[]const u8 = null,
    system_prompt_path: ?[]const u8 = null,
    tools: []const []const u8 = &.{},
    persist: bool = true,
    compaction: []const u8 = "truncate",
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
    providers: []ProviderProfile,
    sessions: []SessionProfile,
    store: StoreConfig,
    otel: OtelConfig,
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
            .providers = &.{},
            .sessions = &.{},
            .store = .{},
            .otel = .{},
            .skills = &.{},
            .sources = &.{},
        };
        errdefer cfg.arena.deinit();
        const a = cfg.arena.allocator();

        cfg.store.path = try a.dupe(u8, "./.phoenix/store");

        // Built-in defaults: one claude provider (no auth, will be filled in by
        // env-var fallback or onboarding) and one default session.
        var default_providers = try a.alloc(ProviderProfile, 1);
        default_providers[0] = .{
            .kind = .claude,
            .model = try a.dupe(u8, "claude-opus-4-7"),
            .active = true,
        };
        cfg.providers = default_providers;

        var default_sessions = try a.alloc(SessionProfile, 1);
        default_sessions[0] = .{
            .name = try a.dupe(u8, "default"),
            .compaction = try a.dupe(u8, "truncate"),
        };
        cfg.sessions = default_sessions;

        const disc = config_paths.Discovery{
            .home = opts.home,
            .cwd = opts.cwd,
            .explicit_path = opts.explicit_path,
        };
        var paths = try disc.discover(io, gpa);
        defer paths.deinit(gpa);

        var sources_list: std.ArrayList([]const u8) = .empty;

        const layers: [3]?[]const u8 = .{ paths.user, paths.project, paths.explicit };
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

        cfg.skills = try skills_pkg.discoverLayered(
            io,
            a,
            paths.user_dir,
            paths.project_dir,
            opts.explicit_dir,
        );

        // Auto-wire well-known env vars (ANTHROPIC_API_KEY etc.) for the active
        // provider when no auth is configured. Lets a user with `export
        // ANTHROPIC_API_KEY=...` skip both the wizard and editing the config file.
        try cfg.applyEnvVarFallbacks(a);
        try cfg.applyOtelEnvOverrides(a);

        return cfg;
    }

    /// Pull standard OTLP env vars over whatever the config files set. Env
    /// wins so `export OTEL_EXPORTER_OTLP_ENDPOINT=...` enables observability
    /// without any project edits.
    fn applyOtelEnvOverrides(self: *Config, a: std.mem.Allocator) !void {
        if (readEnv(a, "OTEL_EXPORTER_OTLP_ENDPOINT")) |v| self.otel.endpoint = v;
        if (readEnv(a, "OTEL_EXPORTER_OTLP_TRACES_ENDPOINT")) |v| self.otel.traces_endpoint = v;
        if (readEnv(a, "OTEL_EXPORTER_OTLP_LOGS_ENDPOINT")) |v| self.otel.logs_endpoint = v;
        if (readEnv(a, "OTEL_SERVICE_NAME")) |v| self.otel.service_name = v;
        if (readEnv(a, "OTEL_EXPORTER_OTLP_HEADERS")) |raw| {
            self.otel.headers = try parseOtelHeaderList(a, raw);
        }
    }


    fn applyEnvVarFallbacks(self: *Config, a: std.mem.Allocator) !void {
        const ap = self.activeProviderMut() orelse return;
        if (ap.auth != null) return;

const env_name: []const u8 = switch (ap.kind) {
        .claude => "ANTHROPIC_API_KEY",
        .openai => "OPENAI_API_KEY",
        .gemini, .vertex => "GEMINI_API_KEY",
        .ollama, .llamacpp => return,
        .nvidia => "NVIDIA_API_KEY",
    };

        var name_buf: [64]u8 = undefined;
        if (env_name.len + 1 > name_buf.len) return;
        @memcpy(name_buf[0..env_name.len], env_name);
        name_buf[env_name.len] = 0;
        const sentinel: [*:0]const u8 = name_buf[0..env_name.len :0];
        const ptr = std.c.getenv(sentinel) orelse return;
        const value = std.mem.span(ptr);
        if (value.len == 0) return;

        ap.auth = .{ .env_var = try a.dupe(u8, env_name) };
    }

    pub fn loadDefault(gpa: std.mem.Allocator, io: std.Io) !Config {
        return load(gpa, io, .{});
    }

    pub fn deinit(self: *Config) void {
        self.arena.deinit();
    }

    /// Pick the provider that should drive new conversations: the first entry
    /// flagged `active = true`, or the first entry if none is flagged.
    pub fn activeProvider(self: *const Config) ?*const ProviderProfile {
        for (self.providers) |*p| if (p.active) return p;
        if (self.providers.len > 0) return &self.providers[0];
        return null;
    }

    pub fn activeProviderMut(self: *Config) ?*ProviderProfile {
        for (self.providers) |*p| if (p.active) return p;
        if (self.providers.len > 0) return &self.providers[0];
        return null;
    }

    pub fn findSession(self: *const Config, name: []const u8) ?*const SessionProfile {
        for (self.sessions) |*s| if (std.mem.eql(u8, s.name, name)) return s;
        return null;
    }

    /// True iff the active provider has a usable credential, or is local.
    pub fn activeProviderUsable(self: *const Config, gpa: std.mem.Allocator) bool {
        const p = self.activeProvider() orelse return false;
        switch (p.kind) {
            .ollama, .llamacpp => return true,
            else => {},
        }
        const auth = p.auth orelse return false;
        const resolved = auth.resolve(gpa) catch return false;
        if (resolved) |r| {
            defer gpa.free(r);
            return r.len > 0;
        }
        return false;
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

        const profile = self.findSession(name) orelse return error.SessionNotFound;

        if (profile.extends) |parent_name| {
            var parent = try resolveSessionInner(self, parent_name, depth + 1);
            if (profile.system_prompt_path) |spp| parent.system_prompt_path = spp;
            parent.persist = profile.persist;
            parent.compaction = profile.compaction;
            parent.compaction_threshold = profile.compaction_threshold;
            parent.compaction_tail_turns = profile.compaction_tail_turns;
            parent.token_budget = profile.token_budget;
            parent.token_budget_warn = profile.token_budget_warn;
            parent.extends = null;

            if (profile.tools.len > 0) {
                const combined = try self.arena.allocator().alloc([]const u8, parent.tools.len + profile.tools.len);
                @memcpy(combined[0..parent.tools.len], parent.tools);
                @memcpy(combined[parent.tools.len..], profile.tools);
                parent.tools = combined;
            }
            return parent;
        }

        return profile.*;
    }

    fn loadFile(self: *Config, a: std.mem.Allocator, io: std.Io, path: []const u8, is_project: bool) !void {
        const raw = try std.Io.Dir.cwd().readFileAlloc(io, path, a, .limited(2 * 1024 * 1024));
        defer a.free(raw);

        var parsed = try std.json.parseFromSlice(std.json.Value, a, raw, .{});
        defer parsed.deinit();

        if (parsed.value != .object) {
            std.log.err("config: top-level value must be an object", .{});
            return error.InvalidConfigValue;
        }

        var it = parsed.value.object.iterator();
        while (it.next()) |entry| {
            const key = entry.key_ptr.*;
            const val = entry.value_ptr.*;
            if (std.mem.eql(u8, key, "runtime")) {
                try self.applyRuntime(a, val);
            } else if (std.mem.eql(u8, key, "theme")) {
                try self.applyTheme(a, val);
            } else if (std.mem.eql(u8, key, "providers")) {
                try self.applyProviders(a, val, is_project);
            } else if (std.mem.eql(u8, key, "sessions")) {
                try self.applySessions(a, val);
            } else if (std.mem.eql(u8, key, "store")) {
                try self.applyStore(a, val);
            } else if (std.mem.eql(u8, key, "otel")) {
                try self.applyOtel(a, val);
            } else {
                std.log.warn("ignoring unknown config key: {s}", .{key});
            }
        }
    }

    fn applyRuntime(self: *Config, a: std.mem.Allocator, v: std.json.Value) !void {
        _ = a;
        const t = try expectObject(v, "runtime");
        var it = t.iterator();
        while (it.next()) |entry| {
            const k = entry.key_ptr.*;
            const val = entry.value_ptr.*;
            if (std.mem.eql(u8, k, "mode")) {
                const s = try expectString(val, "runtime.mode");
                if (std.mem.eql(u8, s, "tui")) {
                    self.runtime.mode = .tui;
                } else if (std.mem.eql(u8, s, "rpc")) {
                    self.runtime.mode = .rpc;
                } else {
                    std.log.err("config: invalid runtime.mode value: {s}", .{s});
                    return error.InvalidConfigValue;
                }
            } else if (std.mem.eql(u8, k, "log_level")) {
                const s = try expectString(val, "runtime.log_level");
                self.runtime.log_level = if (std.mem.eql(u8, s, "debug"))
                    .debug
                else if (std.mem.eql(u8, s, "info"))
                    .info
                else if (std.mem.eql(u8, s, "warn"))
                    .warn
                else if (std.mem.eql(u8, s, "err") or std.mem.eql(u8, s, "error"))
                    .err
                else {
                    std.log.err("config: invalid runtime.log_level value: {s}", .{s});
                    return error.InvalidConfigValue;
                };
            } else if (std.mem.eql(u8, k, "max_concurrent_sessions")) {
                switch (val) {
                    .string => |s| {
                        if (std.mem.eql(u8, s, "auto")) {
                            self.runtime.max_concurrent_sessions = null;
                        } else {
                            std.log.err("config: invalid runtime.max_concurrent_sessions value: {s}", .{s});
                            return error.InvalidConfigValue;
                        }
                    },
                    .integer => |i| {
                        if (i < 0) return error.InvalidConfigValue;
                        self.runtime.max_concurrent_sessions = @intCast(i);
                    },
                    else => return error.InvalidConfigValue,
                }
            } else {
                std.log.warn("ignoring unknown config key: runtime.{s}", .{k});
            }
        }
    }

    fn applyTheme(self: *Config, a: std.mem.Allocator, v: std.json.Value) !void {
        const t = try expectObject(v, "theme");
        var it = t.iterator();
        while (it.next()) |entry| {
            const k = entry.key_ptr.*;
            const val = entry.value_ptr.*;
            const s = try expectString(val, k);
            const dup = try a.dupe(u8, s);
            if (std.mem.eql(u8, k, "background")) {
                self.theme.background = dup;
            } else if (std.mem.eql(u8, k, "foreground")) {
                self.theme.foreground = dup;
            } else if (std.mem.eql(u8, k, "accent")) {
                self.theme.accent = dup;
            } else if (std.mem.eql(u8, k, "user_bubble_bg")) {
                self.theme.user_bubble_bg = dup;
            } else if (std.mem.eql(u8, k, "user_bubble_fg")) {
                self.theme.user_bubble_fg = dup;
            } else if (std.mem.eql(u8, k, "assistant_bubble_bg")) {
                self.theme.assistant_bubble_bg = dup;
            } else if (std.mem.eql(u8, k, "assistant_bubble_fg")) {
                self.theme.assistant_bubble_fg = dup;
            } else if (std.mem.eql(u8, k, "system_text")) {
                self.theme.system_text = dup;
            } else if (std.mem.eql(u8, k, "status_bg")) {
                self.theme.status_bg = dup;
            } else if (std.mem.eql(u8, k, "status_fg")) {
                self.theme.status_fg = dup;
            } else {
                std.log.warn("ignoring unknown config key: theme.{s}", .{k});
                a.free(dup);
            }
        }
    }

    fn applyProviders(self: *Config, a: std.mem.Allocator, v: std.json.Value, is_project: bool) !void {
        if (v != .array) {
            std.log.err("config: providers must be an array", .{});
            return error.InvalidConfigValue;
        }
        const items = v.array.items;
        const out = try a.alloc(ProviderProfile, items.len);
        for (items, 0..) |item, i| {
            out[i] = ProviderProfile{
                .model = try a.dupe(u8, "claude-opus-4-7"),
            };
            try applyProviderObject(a, &out[i], item, is_project);
            validateProvider(&out[i]);
        }
        // Layered semantics: each layer's providers array fully replaces
        // the running list. The arena is per-Config so previously-allocated
        // entries are simply unreferenced (will be freed at deinit).
        self.providers = out;
    }

    fn applySessions(self: *Config, a: std.mem.Allocator, v: std.json.Value) !void {
        if (v != .array) {
            std.log.err("config: sessions must be an array", .{});
            return error.InvalidConfigValue;
        }
        const items = v.array.items;
        const out = try a.alloc(SessionProfile, items.len);
        for (items, 0..) |item, i| {
            out[i] = SessionProfile{
                .name = try a.dupe(u8, "default"),
                .compaction = try a.dupe(u8, "truncate"),
            };
            try applySessionObject(a, &out[i], item);
        }
        self.sessions = out;
    }

    fn applyOtel(self: *Config, a: std.mem.Allocator, v: std.json.Value) !void {
        const t = try expectObject(v, "otel");
        var it = t.iterator();
        while (it.next()) |entry| {
            const k = entry.key_ptr.*;
            const val = entry.value_ptr.*;
            if (std.mem.eql(u8, k, "endpoint")) {
                self.otel.endpoint = try a.dupe(u8, try expectString(val, "otel.endpoint"));
            } else if (std.mem.eql(u8, k, "traces_endpoint")) {
                self.otel.traces_endpoint = try a.dupe(u8, try expectString(val, "otel.traces_endpoint"));
            } else if (std.mem.eql(u8, k, "logs_endpoint")) {
                self.otel.logs_endpoint = try a.dupe(u8, try expectString(val, "otel.logs_endpoint"));
            } else if (std.mem.eql(u8, k, "service_name")) {
                self.otel.service_name = try a.dupe(u8, try expectString(val, "otel.service_name"));
            } else if (std.mem.eql(u8, k, "headers")) {
                self.otel.headers = try parseOtelHeaderObject(a, val);
            } else if (std.mem.eql(u8, k, "flush_interval_ms")) {
                self.otel.flush_interval_ms = @intCast(try expectInteger(val, "otel.flush_interval_ms"));
            } else if (std.mem.eql(u8, k, "max_queue_size")) {
                self.otel.max_queue_size = @intCast(try expectInteger(val, "otel.max_queue_size"));
            } else if (std.mem.eql(u8, k, "request_timeout_ms")) {
                self.otel.request_timeout_ms = @intCast(try expectInteger(val, "otel.request_timeout_ms"));
            } else {
                std.log.warn("ignoring unknown config key: otel.{s}", .{k});
            }
        }
    }

    fn applyStore(self: *Config, a: std.mem.Allocator, v: std.json.Value) !void {
        const t = try expectObject(v, "store");
        var it = t.iterator();
        while (it.next()) |entry| {
            const k = entry.key_ptr.*;
            const val = entry.value_ptr.*;
            if (std.mem.eql(u8, k, "backend")) {
                const s = try expectString(val, "store.backend");
                if (std.mem.eql(u8, s, "memory")) {
                    self.store.backend = .memory;
                } else if (std.mem.eql(u8, s, "beans")) {
                    self.store.backend = .beans;
                } else {
                    std.log.err("config: invalid store.backend value: {s}", .{s});
                    return error.InvalidConfigValue;
                }
            } else if (std.mem.eql(u8, k, "path")) {
                self.store.path = try a.dupe(u8, try expectString(val, "store.path"));
            } else {
                std.log.warn("ignoring unknown config key: store.{s}", .{k});
            }
        }
    }
};

fn applyProviderObject(a: std.mem.Allocator, profile: *ProviderProfile, v: std.json.Value, is_project: bool) !void {
    const t = try expectObject(v, "providers[i]");
    var it = t.iterator();
    while (it.next()) |entry| {
        const k = entry.key_ptr.*;
        const val = entry.value_ptr.*;
        if (std.mem.eql(u8, k, "kind")) {
            profile.kind = try parseProviderKind(try expectString(val, "providers[i].kind"));
        } else if (std.mem.eql(u8, k, "model")) {
            profile.model = try a.dupe(u8, try expectString(val, "providers[i].model"));
        } else if (std.mem.eql(u8, k, "active")) {
            profile.active = try expectBool(val, "providers[i].active");
        } else if (std.mem.eql(u8, k, "auth")) {
            profile.auth = try parseAuthEntry(a, val, is_project);
        } else if (std.mem.eql(u8, k, "base_url")) {
            profile.base_url = try a.dupe(u8, try expectString(val, "providers[i].base_url"));
        } else if (std.mem.eql(u8, k, "endpoint")) {
            profile.endpoint = try a.dupe(u8, try expectString(val, "providers[i].endpoint"));
        } else if (std.mem.eql(u8, k, "max_retries")) {
            profile.max_retries = @intCast(try expectInteger(val, "providers[i].max_retries"));
        } else if (std.mem.eql(u8, k, "retry_base_delay_ms")) {
            profile.retry_base_delay_ms = @intCast(try expectInteger(val, "providers[i].retry_base_delay_ms"));
        } else if (std.mem.eql(u8, k, "retry_max_delay_ms")) {
            profile.retry_max_delay_ms = @intCast(try expectInteger(val, "providers[i].retry_max_delay_ms"));
        } else if (std.mem.eql(u8, k, "request_timeout_ms")) {
            profile.request_timeout_ms = @intCast(try expectInteger(val, "providers[i].request_timeout_ms"));
        } else if (std.mem.eql(u8, k, "api")) {
            profile.api = try a.dupe(u8, try expectString(val, "providers[i].api"));
        } else if (std.mem.eql(u8, k, "project")) {
            profile.project = try a.dupe(u8, try expectString(val, "providers[i].project"));
        } else if (std.mem.eql(u8, k, "location")) {
            profile.location = try a.dupe(u8, try expectString(val, "providers[i].location"));
        } else if (std.mem.eql(u8, k, "credentials_path")) {
            profile.credentials_path = try a.dupe(u8, try expectString(val, "providers[i].credentials_path"));
        } else if (std.mem.eql(u8, k, "context_window")) {
            const n = try expectInteger(val, "providers[i].context_window");
            if (n <= 0) return error.InvalidConfigValue;
            profile.context_window = @intCast(n);
        } else {
            std.log.warn("ignoring unknown providers[].{s}", .{k});
        }
    }
}

fn parseProviderKind(s: []const u8) !ProviderKind {
    if (std.mem.eql(u8, s, "claude") or std.mem.eql(u8, s, "anthropic")) return .claude;
    if (std.mem.eql(u8, s, "openai")) return .openai;
    if (std.mem.eql(u8, s, "ollama")) return .ollama;
    if (std.mem.eql(u8, s, "llamacpp") or std.mem.eql(u8, s, "llama.cpp")) return .llamacpp;
    if (std.mem.eql(u8, s, "vertex") or std.mem.eql(u8, s, "vertex_ai")) return .vertex;
    if (std.mem.eql(u8, s, "gemini")) return .gemini;
    if (std.mem.eql(u8, s, "nvidia")) return .nvidia;
    std.log.err("config: invalid provider.kind value: {s}", .{s});
    return error.InvalidConfigValue;
}

fn parseAuthEntry(a: std.mem.Allocator, v: std.json.Value, is_project: bool) !AuthEntry {
    switch (v) {
        .string => |s| {
            if (is_project) {
                std.log.warn("project-level auth contains a raw secret; prefer {{\"env\": \"...\"}}", .{});
            }
            return AuthEntry{ .inline_value = try a.dupe(u8, s) };
        },
        .object => |obj| {
            if (obj.get("env")) |env_v| {
                const s = try expectString(env_v, "auth.env");
                return AuthEntry{ .env_var = try a.dupe(u8, s) };
            }
            if (obj.get("inline")) |inline_v| {
                const s = try expectString(inline_v, "auth.inline");
                if (is_project) {
                    std.log.warn("project-level auth contains a raw secret; prefer {{\"env\": \"...\"}}", .{});
                }
                return AuthEntry{ .inline_value = try a.dupe(u8, s) };
            }
            std.log.err("config: auth object must have either 'env' or 'inline' key", .{});
            return error.InvalidConfigValue;
        },
        else => {
            std.log.err("config: auth must be a string or object", .{});
            return error.InvalidConfigValue;
        },
    }
}

fn validateProvider(profile: *const ProviderProfile) void {
    if (profile.api != null and profile.kind != .openai) {
        std.log.warn("config: provider.api is set but kind is not openai; the field will be ignored", .{});
    }
    if (profile.kind == .vertex and profile.project == null) {
        std.log.warn("config: provider.kind = vertex but project is not set; send will fail at runtime", .{});
    }
}

fn applySessionObject(a: std.mem.Allocator, profile: *SessionProfile, v: std.json.Value) !void {
    const t = try expectObject(v, "sessions[i]");
    var it = t.iterator();
    while (it.next()) |entry| {
        const k = entry.key_ptr.*;
        const val = entry.value_ptr.*;
        if (std.mem.eql(u8, k, "name")) {
            profile.name = try a.dupe(u8, try expectString(val, "sessions[i].name"));
        } else if (std.mem.eql(u8, k, "extends")) {
            profile.extends = try a.dupe(u8, try expectString(val, "sessions[i].extends"));
        } else if (std.mem.eql(u8, k, "system_prompt_path")) {
            profile.system_prompt_path = try a.dupe(u8, try expectString(val, "sessions[i].system_prompt_path"));
        } else if (std.mem.eql(u8, k, "tools")) {
            profile.tools = try expectStringArray(a, val, "sessions[i].tools");
        } else if (std.mem.eql(u8, k, "persist")) {
            profile.persist = try expectBool(val, "sessions[i].persist");
        } else if (std.mem.eql(u8, k, "compaction")) {
            profile.compaction = try a.dupe(u8, try expectString(val, "sessions[i].compaction"));
        } else if (std.mem.eql(u8, k, "compaction_threshold")) {
            profile.compaction_threshold = @floatCast(try expectFloat(val, "sessions[i].compaction_threshold"));
        } else if (std.mem.eql(u8, k, "compaction_tail_turns")) {
            profile.compaction_tail_turns = @intCast(try expectInteger(val, "sessions[i].compaction_tail_turns"));
        } else if (std.mem.eql(u8, k, "token_budget")) {
            switch (val) {
                .string => |s| {
                    if (std.mem.eql(u8, s, "unlimited")) {
                        profile.token_budget = .unlimited;
                    } else return error.InvalidConfigValue;
                },
                .integer => |i| profile.token_budget = .{ .limit = @intCast(i) },
                else => return error.InvalidConfigValue,
            }
        } else if (std.mem.eql(u8, k, "token_budget_warn")) {
            profile.token_budget_warn = @floatCast(try expectFloat(val, "sessions[i].token_budget_warn"));
        } else {
            std.log.warn("ignoring unknown sessions[].{s}", .{k});
        }
    }
}

fn expectObject(v: std.json.Value, field: []const u8) !std.json.ObjectMap {
    return switch (v) {
        .object => |o| o,
        else => {
            std.log.err("config: {s} must be an object", .{field});
            return error.InvalidConfigValue;
        },
    };
}

fn expectString(v: std.json.Value, field: []const u8) ![]const u8 {
    return switch (v) {
        .string => |s| s,
        else => {
            std.log.err("config: {s} must be a string", .{field});
            return error.InvalidConfigValue;
        },
    };
}

fn expectInteger(v: std.json.Value, field: []const u8) !i64 {
    return switch (v) {
        .integer => |i| i,
        else => {
            std.log.err("config: {s} must be an integer", .{field});
            return error.InvalidConfigValue;
        },
    };
}

fn expectFloat(v: std.json.Value, field: []const u8) !f64 {
    return switch (v) {
        .float => |f| f,
        .integer => |i| @floatFromInt(i),
        else => {
            std.log.err("config: {s} must be a number", .{field});
            return error.InvalidConfigValue;
        },
    };
}

fn expectBool(v: std.json.Value, field: []const u8) !bool {
    return switch (v) {
        .bool => |b| b,
        else => {
            std.log.err("config: {s} must be a boolean", .{field});
            return error.InvalidConfigValue;
        },
    };
}

/// Read a non-empty env var into the arena. Returns null when unset or empty.
fn readEnv(a: std.mem.Allocator, name: []const u8) ?[]const u8 {
    var buf: [128]u8 = undefined;
    if (name.len + 1 > buf.len) return null;
    @memcpy(buf[0..name.len], name);
    buf[name.len] = 0;
    const sentinel: [*:0]const u8 = buf[0..name.len :0];
    const ptr = std.c.getenv(sentinel) orelse return null;
    const s = std.mem.span(ptr);
    if (s.len == 0) return null;
    return a.dupe(u8, s) catch null;
}

/// Parse the `OTEL_EXPORTER_OTLP_HEADERS` format: `key1=value1,key2=value2`.
/// Whitespace around keys/values is trimmed. Malformed entries are skipped
/// rather than failing config load. Outside test builds we also emit a warning.
fn parseOtelHeaderList(a: std.mem.Allocator, raw: []const u8) ![]otel_pkg.Header {
    var out: std.ArrayList(otel_pkg.Header) = .empty;
    errdefer out.deinit(a);
    var it = std.mem.splitScalar(u8, raw, ',');
    while (it.next()) |entry| {
        const trimmed = std.mem.trim(u8, entry, " \t");
        if (trimmed.len == 0) continue;
        const eq = std.mem.indexOfScalar(u8, trimmed, '=') orelse {
            if (!@import("builtin").is_test) {
                std.log.warn("OTEL_EXPORTER_OTLP_HEADERS: skipping malformed entry: {s}", .{trimmed});
            }
            continue;
        };
        const k = std.mem.trim(u8, trimmed[0..eq], " \t");
        const v = std.mem.trim(u8, trimmed[eq + 1 ..], " \t");
        try out.append(a, .{
            .name = try a.dupe(u8, k),
            .value = try a.dupe(u8, v),
        });
    }
    return out.toOwnedSlice(a);
}

/// Parse the `otel.headers` JSON object: { "Key": "Value", ... }.
fn parseOtelHeaderObject(a: std.mem.Allocator, v: std.json.Value) ![]otel_pkg.Header {
    const obj = try expectObject(v, "otel.headers");
    var out: std.ArrayList(otel_pkg.Header) = .empty;
    errdefer out.deinit(a);
    var it = obj.iterator();
    while (it.next()) |entry| {
        const k = entry.key_ptr.*;
        const val = try expectString(entry.value_ptr.*, "otel.headers.value");
        try out.append(a, .{
            .name = try a.dupe(u8, k),
            .value = try a.dupe(u8, val),
        });
    }
    return out.toOwnedSlice(a);
}

fn expectStringArray(a: std.mem.Allocator, v: std.json.Value, field: []const u8) ![]const []const u8 {
    const arr = switch (v) {
        .array => |x| x,
        else => {
            std.log.err("config: {s} must be an array", .{field});
            return error.InvalidConfigValue;
        },
    };
    const out = try a.alloc([]const u8, arr.items.len);
    for (arr.items, 0..) |item, i| {
        out[i] = switch (item) {
            .string => |s| try a.dupe(u8, s),
            else => {
                std.log.err("config: {s} must be an array of strings", .{field});
                return error.InvalidConfigValue;
            },
        };
    }
    return out;
}

// ---- Tests ----

test "defaults: empty config has one active claude provider" {
    const allocator = std.testing.allocator;
    var cfg = try Config.load(allocator, std.testing.io, .{
        .home = null,
        .cwd = null,
    });
    defer cfg.deinit();

    try std.testing.expectEqual(@as(usize, 1), cfg.providers.len);
    const ap = cfg.activeProvider() orelse return error.TestUnexpectedResult;
    try std.testing.expectEqual(ProviderKind.claude, ap.kind);
    try std.testing.expectEqualStrings("claude-opus-4-7", ap.model);
    try std.testing.expect(ap.active);
}

test "user file replaces default providers" {
    const allocator = std.testing.allocator;
    var tmp = std.testing.tmpDir(.{});
    defer tmp.cleanup();
    const tmp_path = try getTmpDirPath(allocator, &tmp);
    defer allocator.free(tmp_path);

    const home = try std.fs.path.join(allocator, &.{ tmp_path, "home" });
    defer allocator.free(home);
    try std.Io.Dir.cwd().createDirPath(std.testing.io, home);
    const phoenix_dir = try std.fs.path.join(allocator, &.{ home, ".phoenix" });
    defer allocator.free(phoenix_dir);
    try std.Io.Dir.cwd().createDirPath(std.testing.io, phoenix_dir);
    const cfg_path = try std.fs.path.join(allocator, &.{ phoenix_dir, "phoenix.json" });
    defer allocator.free(cfg_path);
    try std.Io.Dir.cwd().writeFile(std.testing.io, .{
        .sub_path = cfg_path,
        .data =
        \\{
        \\  "providers": [
        \\    { "kind": "openai", "model": "gpt-4o", "active": true,
        \\      "auth": { "env": "OPENAI_API_KEY" } },
        \\    { "kind": "claude", "model": "claude-opus-4-7" }
        \\  ]
        \\}
        ,
    });

    var cfg = try Config.load(allocator, std.testing.io, .{ .home = home });
    defer cfg.deinit();

    try std.testing.expectEqual(@as(usize, 2), cfg.providers.len);
    const ap = cfg.activeProvider() orelse return error.TestUnexpectedResult;
    try std.testing.expectEqual(ProviderKind.openai, ap.kind);
    try std.testing.expectEqualStrings("gpt-4o", ap.model);
}

test "auth: env-var ref" {
    const allocator = std.testing.allocator;
    var tmp = std.testing.tmpDir(.{});
    defer tmp.cleanup();
    const tmp_path = try getTmpDirPath(allocator, &tmp);
    defer allocator.free(tmp_path);
    const home = try std.fs.path.join(allocator, &.{ tmp_path, "home" });
    defer allocator.free(home);
    try std.Io.Dir.cwd().createDirPath(std.testing.io, home);
    const phoenix_dir = try std.fs.path.join(allocator, &.{ home, ".phoenix" });
    defer allocator.free(phoenix_dir);
    try std.Io.Dir.cwd().createDirPath(std.testing.io, phoenix_dir);
    const cfg_path = try std.fs.path.join(allocator, &.{ phoenix_dir, "phoenix.json" });
    defer allocator.free(cfg_path);
    try std.Io.Dir.cwd().writeFile(std.testing.io, .{
        .sub_path = cfg_path,
        .data =
        \\{ "providers": [{ "kind": "claude", "model": "x", "active": true,
        \\  "auth": { "env": "MY_KEY_VAR" } }] }
        ,
    });
    var cfg = try Config.load(allocator, std.testing.io, .{ .home = home });
    defer cfg.deinit();

    const ap = cfg.activeProvider().?;
    try std.testing.expect(ap.auth != null);
    switch (ap.auth.?) {
        .env_var => |v| try std.testing.expectEqualStrings("MY_KEY_VAR", v),
        else => return error.TestUnexpectedResult,
    }
}

test "auth: inline string secret" {
    const allocator = std.testing.allocator;
    var tmp = std.testing.tmpDir(.{});
    defer tmp.cleanup();
    const tmp_path = try getTmpDirPath(allocator, &tmp);
    defer allocator.free(tmp_path);
    const home = try std.fs.path.join(allocator, &.{ tmp_path, "home" });
    defer allocator.free(home);
    try std.Io.Dir.cwd().createDirPath(std.testing.io, home);
    const phoenix_dir = try std.fs.path.join(allocator, &.{ home, ".phoenix" });
    defer allocator.free(phoenix_dir);
    try std.Io.Dir.cwd().createDirPath(std.testing.io, phoenix_dir);
    const cfg_path = try std.fs.path.join(allocator, &.{ phoenix_dir, "phoenix.json" });
    defer allocator.free(cfg_path);
    try std.Io.Dir.cwd().writeFile(std.testing.io, .{
        .sub_path = cfg_path,
        .data =
        \\{ "providers": [{ "kind": "claude", "model": "x", "active": true,
        \\  "auth": "sk-test-1" }] }
        ,
    });
    var cfg = try Config.load(allocator, std.testing.io, .{ .home = home });
    defer cfg.deinit();

    const ap = cfg.activeProvider().?;
    switch (ap.auth.?) {
        .inline_value => |v| try std.testing.expectEqualStrings("sk-test-1", v),
        else => return error.TestUnexpectedResult,
    }
    const resolved = (try ap.auth.?.resolve(allocator)) orelse return error.TestUnexpectedResult;
    defer allocator.free(resolved);
    try std.testing.expectEqualStrings("sk-test-1", resolved);
}

test "no active flag falls back to first provider" {
    const allocator = std.testing.allocator;
    var tmp = std.testing.tmpDir(.{});
    defer tmp.cleanup();
    const tmp_path = try getTmpDirPath(allocator, &tmp);
    defer allocator.free(tmp_path);
    const home = try std.fs.path.join(allocator, &.{ tmp_path, "home" });
    defer allocator.free(home);
    try std.Io.Dir.cwd().createDirPath(std.testing.io, home);
    const phoenix_dir = try std.fs.path.join(allocator, &.{ home, ".phoenix" });
    defer allocator.free(phoenix_dir);
    try std.Io.Dir.cwd().createDirPath(std.testing.io, phoenix_dir);
    const cfg_path = try std.fs.path.join(allocator, &.{ phoenix_dir, "phoenix.json" });
    defer allocator.free(cfg_path);
    try std.Io.Dir.cwd().writeFile(std.testing.io, .{
        .sub_path = cfg_path,
        .data =
        \\{ "providers": [
        \\  { "kind": "openai", "model": "gpt-4o" },
        \\  { "kind": "claude", "model": "claude-opus-4-7" }
        \\] }
        ,
    });
    var cfg = try Config.load(allocator, std.testing.io, .{ .home = home });
    defer cfg.deinit();
    const ap = cfg.activeProvider().?;
    try std.testing.expectEqual(ProviderKind.openai, ap.kind);
}

test "session resolves and extends" {
    const allocator = std.testing.allocator;
    var tmp = std.testing.tmpDir(.{});
    defer tmp.cleanup();
    const tmp_path = try getTmpDirPath(allocator, &tmp);
    defer allocator.free(tmp_path);
    const home = try std.fs.path.join(allocator, &.{ tmp_path, "home" });
    defer allocator.free(home);
    try std.Io.Dir.cwd().createDirPath(std.testing.io, home);
    const phoenix_dir = try std.fs.path.join(allocator, &.{ home, ".phoenix" });
    defer allocator.free(phoenix_dir);
    try std.Io.Dir.cwd().createDirPath(std.testing.io, phoenix_dir);
    const cfg_path = try std.fs.path.join(allocator, &.{ phoenix_dir, "phoenix.json" });
    defer allocator.free(cfg_path);
    try std.Io.Dir.cwd().writeFile(std.testing.io, .{
        .sub_path = cfg_path,
        .data =
        \\{
        \\  "sessions": [
        \\    { "name": "base", "tools": ["read"], "persist": true },
        \\    { "name": "child", "extends": "base", "tools": ["bash"] }
        \\  ]
        \\}
        ,
    });
    var cfg = try Config.load(allocator, std.testing.io, .{ .home = home });
    defer cfg.deinit();
    const s = try cfg.resolveSession("child");
    try std.testing.expectEqual(@as(usize, 2), s.tools.len);
    try std.testing.expectEqualStrings("read", s.tools[0]);
    try std.testing.expectEqualStrings("bash", s.tools[1]);
}

test "circular extends returns error" {
    const allocator = std.testing.allocator;
    var tmp = std.testing.tmpDir(.{});
    defer tmp.cleanup();
    const tmp_path = try getTmpDirPath(allocator, &tmp);
    defer allocator.free(tmp_path);
    const home = try std.fs.path.join(allocator, &.{ tmp_path, "home" });
    defer allocator.free(home);
    try std.Io.Dir.cwd().createDirPath(std.testing.io, home);
    const phoenix_dir = try std.fs.path.join(allocator, &.{ home, ".phoenix" });
    defer allocator.free(phoenix_dir);
    try std.Io.Dir.cwd().createDirPath(std.testing.io, phoenix_dir);
    const cfg_path = try std.fs.path.join(allocator, &.{ phoenix_dir, "phoenix.json" });
    defer allocator.free(cfg_path);
    try std.Io.Dir.cwd().writeFile(std.testing.io, .{
        .sub_path = cfg_path,
        .data =
        \\{ "sessions": [
        \\  { "name": "a", "extends": "b" },
        \\  { "name": "b", "extends": "a" }
        \\] }
        ,
    });
    var cfg = try Config.load(allocator, std.testing.io, .{ .home = home });
    defer cfg.deinit();
    try std.testing.expectError(error.CircularExtends, cfg.resolveSession("a"));
}

test "otel: empty by default" {
    const allocator = std.testing.allocator;
    var cfg = try Config.load(allocator, std.testing.io, .{ .home = null });
    defer cfg.deinit();
    try std.testing.expectEqualStrings("", cfg.otel.endpoint);
    try std.testing.expectEqualStrings("phoenix", cfg.otel.service_name);
}

test "otel: parses block from config file" {
    const allocator = std.testing.allocator;
    var tmp = std.testing.tmpDir(.{});
    defer tmp.cleanup();
    const tmp_path = try getTmpDirPath(allocator, &tmp);
    defer allocator.free(tmp_path);
    const home = try std.fs.path.join(allocator, &.{ tmp_path, "home" });
    defer allocator.free(home);
    try std.Io.Dir.cwd().createDirPath(std.testing.io, home);
    const phoenix_dir = try std.fs.path.join(allocator, &.{ home, ".phoenix" });
    defer allocator.free(phoenix_dir);
    try std.Io.Dir.cwd().createDirPath(std.testing.io, phoenix_dir);
    const cfg_path = try std.fs.path.join(allocator, &.{ phoenix_dir, "phoenix.json" });
    defer allocator.free(cfg_path);
    try std.Io.Dir.cwd().writeFile(std.testing.io, .{
        .sub_path = cfg_path,
        .data =
        \\{
        \\  "otel": {
        \\    "endpoint": "http://localhost:4318",
        \\    "service_name": "phoenix-rpc",
        \\    "headers": { "x-tenant": "acme" },
        \\    "max_queue_size": 64
        \\  }
        \\}
        ,
    });

    var cfg = try Config.load(allocator, std.testing.io, .{ .home = home });
    defer cfg.deinit();
    try std.testing.expectEqualStrings("http://localhost:4318", cfg.otel.endpoint);
    try std.testing.expectEqualStrings("phoenix-rpc", cfg.otel.service_name);
    try std.testing.expectEqual(@as(usize, 64), cfg.otel.max_queue_size);
    try std.testing.expectEqual(@as(usize, 1), cfg.otel.headers.len);
    try std.testing.expectEqualStrings("x-tenant", cfg.otel.headers[0].name);
    try std.testing.expectEqualStrings("acme", cfg.otel.headers[0].value);
}

test "otel: parseOtelHeaderList env-var format" {
    const a = std.testing.allocator;
    var arena = std.heap.ArenaAllocator.init(a);
    defer arena.deinit();
    const hdrs = try parseOtelHeaderList(arena.allocator(), "k1=v1, k2 = v2 ,bare,k3=v3");
    try std.testing.expectEqual(@as(usize, 3), hdrs.len);
    try std.testing.expectEqualStrings("k1", hdrs[0].name);
    try std.testing.expectEqualStrings("v1", hdrs[0].value);
    try std.testing.expectEqualStrings("k2", hdrs[1].name);
    try std.testing.expectEqualStrings("v2", hdrs[1].value);
    try std.testing.expectEqualStrings("k3", hdrs[2].name);
    try std.testing.expectEqualStrings("v3", hdrs[2].value);
}

test "anthropic alias for claude" {
    const allocator = std.testing.allocator;
    var tmp = std.testing.tmpDir(.{});
    defer tmp.cleanup();
    const tmp_path = try getTmpDirPath(allocator, &tmp);
    defer allocator.free(tmp_path);
    const home = try std.fs.path.join(allocator, &.{ tmp_path, "home" });
    defer allocator.free(home);
    try std.Io.Dir.cwd().createDirPath(std.testing.io, home);
    const phoenix_dir = try std.fs.path.join(allocator, &.{ home, ".phoenix" });
    defer allocator.free(phoenix_dir);
    try std.Io.Dir.cwd().createDirPath(std.testing.io, phoenix_dir);
    const cfg_path = try std.fs.path.join(allocator, &.{ phoenix_dir, "phoenix.json" });
    defer allocator.free(cfg_path);
    try std.Io.Dir.cwd().writeFile(std.testing.io, .{
        .sub_path = cfg_path,
        .data =
        \\{ "providers": [{ "kind": "anthropic", "model": "x", "active": true }] }
        ,
    });
    var cfg = try Config.load(allocator, std.testing.io, .{ .home = home });
    defer cfg.deinit();
    try std.testing.expectEqual(ProviderKind.claude, cfg.activeProvider().?.kind);
}

fn getTmpDirPath(allocator: std.mem.Allocator, tmp: *std.testing.TmpDir) ![]u8 {
    var buf: [std.Io.Dir.max_path_bytes]u8 = undefined;
    const ptr = std.c.getcwd(&buf, buf.len) orelse return error.CwdError;
    const cwd_path = std.mem.sliceTo(ptr, 0);
    return try std.fs.path.join(allocator, &.{ cwd_path, ".zig-cache", "tmp", &tmp.sub_path });
}
