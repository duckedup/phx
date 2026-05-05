const std = @import("std");

pub const Rgb = [3]u8;

pub const Theme = struct {
    name: []const u8,
    background: Rgb,
    foreground: Rgb,
    primary: Rgb,
    accent: Rgb,
    success: Rgb,
    warning: Rgb,
    err: Rgb,
    info: Rgb,
    diff_add: Rgb,
    diff_delete: Rgb,

    pub fn dim(self: *const Theme) Rgb {
        return blend(self.foreground, self.background, 40);
    }

    pub fn codeFg(self: *const Theme) Rgb {
        return self.accent;
    }

    pub fn userBubbleBg(self: *const Theme) Rgb {
        return darken(self.success, 60);
    }

    pub fn assistantBubbleBg(self: *const Theme) Rgb {
        return darken(self.primary, 60);
    }

    pub fn codeBubbleBg(self: *const Theme) Rgb {
        return darken(self.assistantBubbleBg(), 20);
    }

    pub fn bubbleFg(self: *const Theme) Rgb {
        _ = self;
        return .{ 240, 240, 240 };
    }

    pub fn statusBarBg(self: *const Theme) Rgb {
        return self.primary;
    }

    pub fn statusBarFg(_: *const Theme) Rgb {
        return .{ 0, 0, 0 };
    }

    pub fn cursorBg(self: *const Theme) Rgb {
        return self.foreground;
    }

    pub fn cursorFg(self: *const Theme) Rgb {
        return self.background;
    }

    pub fn userLabelColor(self: *const Theme) Rgb {
        return self.success;
    }

    pub fn assistantLabelColor(self: *const Theme) Rgb {
        return self.primary;
    }

    pub fn toolResultColor(self: *const Theme) Rgb {
        return self.err;
    }

    pub fn toolDefaultColor(self: *const Theme) Rgb {
        return blend(self.info, self.foreground, 60);
    }

    pub fn inputBorderColor(self: *const Theme) Rgb {
        return self.dim();
    }

    pub fn pickerCursorBg(self: *const Theme) Rgb {
        return self.foreground;
    }

    pub fn pickerCursorFg(self: *const Theme) Rgb {
        return self.background;
    }
};

pub const ThemeEntry = struct {
    name: []const u8,
    id: []const u8,
};

fn blend(a: Rgb, b: Rgb, pct: u8) Rgb {
    return .{
        @intCast((@as(u16, a[0]) * pct + @as(u16, b[0]) * (100 - pct)) / 100),
        @intCast((@as(u16, a[1]) * pct + @as(u16, b[1]) * (100 - pct)) / 100),
        @intCast((@as(u16, a[2]) * pct + @as(u16, b[2]) * (100 - pct)) / 100),
    };
}

fn darken(c: Rgb, pct: u8) Rgb {
    return .{
        @intCast(@as(u16, c[0]) * pct / 100),
        @intCast(@as(u16, c[1]) * pct / 100),
        @intCast(@as(u16, c[2]) * pct / 100),
    };
}

pub fn hexToRgb(hex: []const u8) ?Rgb {
    if (hex.len < 6) return null;
    const start: usize = if (hex[0] == '#') 1 else 0;
    const s = hex[start..];
    if (s.len < 6) return null;
    const r = std.fmt.parseInt(u8, s[0..2], 16) catch return null;
    const g = std.fmt.parseInt(u8, s[2..4], 16) catch return null;
    const b = std.fmt.parseInt(u8, s[4..6], 16) catch return null;
    return .{ r, g, b };
}

const embedded_themes = .{
    .{ "amoled", "AMOLED", @embedFile("themes/amoled.json") },
    .{ "aura", "Aura", @embedFile("themes/aura.json") },
    .{ "ayu", "Ayu", @embedFile("themes/ayu.json") },
    .{ "carbonfox", "Carbonfox", @embedFile("themes/carbonfox.json") },
    .{ "catppuccin", "Catppuccin", @embedFile("themes/catppuccin.json") },
    .{ "catppuccin-frappe", "Catppuccin Frappé", @embedFile("themes/catppuccin-frappe.json") },
    .{ "catppuccin-macchiato", "Catppuccin Macchiato", @embedFile("themes/catppuccin-macchiato.json") },
    .{ "cobalt2", "Cobalt2", @embedFile("themes/cobalt2.json") },
    .{ "cursor", "Cursor", @embedFile("themes/cursor.json") },
    .{ "dracula", "Dracula", @embedFile("themes/dracula.json") },
    .{ "everforest", "Everforest", @embedFile("themes/everforest.json") },
    .{ "flexoki", "Flexoki", @embedFile("themes/flexoki.json") },
    .{ "github", "GitHub", @embedFile("themes/github.json") },
    .{ "gruvbox", "Gruvbox", @embedFile("themes/gruvbox.json") },
    .{ "kanagawa", "Kanagawa", @embedFile("themes/kanagawa.json") },
    .{ "lucent-orng", "Lucent ORNG", @embedFile("themes/lucent-orng.json") },
    .{ "material", "Material", @embedFile("themes/material.json") },
    .{ "matrix", "Matrix", @embedFile("themes/matrix.json") },
    .{ "mercury", "Mercury", @embedFile("themes/mercury.json") },
    .{ "monokai", "Monokai", @embedFile("themes/monokai.json") },
    .{ "nightowl", "Night Owl", @embedFile("themes/nightowl.json") },
    .{ "nord", "Nord", @embedFile("themes/nord.json") },
    .{ "one-dark", "One Dark", @embedFile("themes/one-dark.json") },
    .{ "onedarkpro", "One Dark Pro", @embedFile("themes/onedarkpro.json") },
    .{ "orng", "ORNG", @embedFile("themes/orng.json") },
    .{ "osaka-jade", "Osaka Jade", @embedFile("themes/osaka-jade.json") },
    .{ "palenight", "Palenight", @embedFile("themes/palenight.json") },
    .{ "rosepine", "Rosé Pine", @embedFile("themes/rosepine.json") },
    .{ "shadesofpurple", "Shades of Purple", @embedFile("themes/shadesofpurple.json") },
    .{ "solarized", "Solarized", @embedFile("themes/solarized.json") },
    .{ "synthwave84", "Synthwave '84", @embedFile("themes/synthwave84.json") },
    .{ "tokyonight", "Tokyo Night", @embedFile("themes/tokyonight.json") },
    .{ "vercel", "Vercel", @embedFile("themes/vercel.json") },
    .{ "vesper", "Vesper", @embedFile("themes/vesper.json") },
    .{ "zenburn", "Zenburn", @embedFile("themes/zenburn.json") },
};

pub const theme_count = embedded_themes.len;

pub fn listAll() [theme_count]ThemeEntry {
    var entries: [theme_count]ThemeEntry = undefined;
    inline for (embedded_themes, 0..) |t, i| {
        entries[i] = .{ .id = t[0], .name = t[1] };
    }
    return entries;
}

pub fn getByName(name: []const u8) ?Theme {
    inline for (embedded_themes) |t| {
        if (std.mem.eql(u8, t[0], name)) {
            return parseThemeJson(t[0], t[1], t[2]);
        }
    }
    return null;
}

fn parseThemeJson(id: []const u8, display_name: []const u8, json_data: []const u8) ?Theme {
    const parsed = std.json.parseFromSlice(std.json.Value, std.heap.page_allocator, json_data, .{}) catch return null;
    defer parsed.deinit();

    const root = parsed.value.object;
    const dark = (root.get("dark") orelse return null).object;
    const palette = (dark.get("palette") orelse return null).object;

    _ = display_name;

    return .{
        .name = id,
        .background = hexToRgb(strVal(palette.get("neutral"))) orelse return null,
        .foreground = hexToRgb(strVal(palette.get("ink"))) orelse return null,
        .primary = hexToRgb(strVal(palette.get("primary"))) orelse return null,
        .accent = hexToRgb(strVal(palette.get("accent"))) orelse return null,
        .success = hexToRgb(strVal(palette.get("success"))) orelse return null,
        .warning = hexToRgb(strVal(palette.get("warning"))) orelse return null,
        .err = hexToRgb(strVal(palette.get("error"))) orelse return null,
        .info = hexToRgb(strVal(palette.get("info"))) orelse return null,
        .diff_add = hexToRgb(strVal(palette.get("diffAdd"))) orelse return null,
        .diff_delete = hexToRgb(strVal(palette.get("diffDelete"))) orelse return null,
    };
}

fn strVal(v: ?std.json.Value) []const u8 {
    return switch (v orelse return "") {
        .string => |s| s,
        else => "",
    };
}

pub fn isPath(name: []const u8) bool {
    if (name.len == 0) return false;
    if (name[0] == '/' or name[0] == '~') return true;
    if (std.mem.endsWith(u8, name, ".json")) return true;
    return false;
}

pub fn loadFromFile(io: std.Io, allocator: std.mem.Allocator, path: []const u8) ?Theme {
    const data = std.Io.Dir.cwd().readFileAlloc(io, path, allocator, .limited(64 * 1024)) catch return null;
    defer allocator.free(data);
    return parseThemeJson("custom", "Custom", data);
}

pub fn default() Theme {
    return .{
        .name = "default",
        .background = .{ 0, 0, 0 },
        .foreground = .{ 220, 220, 220 },
        .primary = .{ 120, 160, 255 },
        .accent = .{ 220, 190, 130 },
        .success = .{ 130, 220, 130 },
        .warning = .{ 255, 200, 80 },
        .err = .{ 220, 100, 100 },
        .info = .{ 150, 170, 200 },
        .diff_add = .{ 100, 200, 100 },
        .diff_delete = .{ 220, 100, 100 },
    };
}

// ---- Tests ----

test "hexToRgb parses valid hex" {
    const c = hexToRgb("#ff8040").?;
    try std.testing.expectEqual(@as(u8, 255), c[0]);
    try std.testing.expectEqual(@as(u8, 128), c[1]);
    try std.testing.expectEqual(@as(u8, 64), c[2]);
}

test "hexToRgb rejects short string" {
    try std.testing.expect(hexToRgb("#fff") == null);
}

test "getByName returns dracula" {
    const t = getByName("dracula") orelse return error.TestUnexpectedResult;
    try std.testing.expectEqualStrings("dracula", t.name);
    try std.testing.expect(t.background[0] < 50);
}

test "getByName returns null for unknown" {
    try std.testing.expect(getByName("nonexistent") == null);
}

test "listAll returns all themes" {
    const all = listAll();
    try std.testing.expectEqual(theme_count, all.len);
    try std.testing.expectEqualStrings("amoled", all[0].id);
}

test "default theme matches original hardcoded colors" {
    const d = default();
    try std.testing.expectEqual(@as(u8, 130), d.success[0]);
    try std.testing.expectEqual(@as(u8, 220), d.success[1]);
}

test "isPath detects file paths" {
    try std.testing.expect(isPath("/home/user/theme.json"));
    try std.testing.expect(isPath("~/mytheme.json"));
    try std.testing.expect(isPath("custom.json"));
    try std.testing.expect(!isPath("dracula"));
    try std.testing.expect(!isPath("tokyonight"));
    try std.testing.expect(!isPath(""));
}
