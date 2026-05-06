const std = @import("std");

pub const SkillSource = enum { user, project, explicit };

pub const Skill = struct {
    name: []const u8,
    dir: []const u8,
    skill_md: []const u8,
    source: SkillSource,
};

/// Discover skills under <base_dir>/skills/<name>/skill.md.
/// Returns a slice owned by `allocator`. Caller must call `freeSkills` to release.
/// If <base_dir>/skills is missing, returns an empty slice (not an error).
/// Results are sorted alphabetically by name within the layer.
pub fn discoverIn(io: std.Io, allocator: std.mem.Allocator, base_dir: []const u8, source: SkillSource) ![]Skill {
    var list: std.ArrayList(Skill) = .empty;
    errdefer {
        for (list.items) |skill| {
            allocator.free(skill.name);
            allocator.free(skill.dir);
            allocator.free(skill.skill_md);
        }
        list.deinit(allocator);
    }

    const skills_path = try std.fs.path.join(allocator, &.{ base_dir, "skills" });
    defer allocator.free(skills_path);

    var skills_dir = std.Io.Dir.cwd().openDir(io, skills_path, .{ .iterate = true }) catch |err| switch (err) {
        error.FileNotFound, error.NotDir, error.AccessDenied, error.PermissionDenied => return &.{},
        else => return err,
    };
    defer skills_dir.close(io);

    var it = skills_dir.iterate();
    while (try it.next(io)) |entry| {
        // Skip hidden directories and non-directories
        if (entry.name.len == 0 or entry.name[0] == '.') continue;
        if (entry.kind != .directory) continue;

        // Check for skill.md (case-insensitive) inside this directory
        const actual_md = findSkillMdCaseInsensitive(io, skills_dir, entry.name, allocator) catch continue;

        const name = try allocator.dupe(u8, entry.name);
        errdefer allocator.free(name);
        const dir = try std.fs.path.join(allocator, &.{ skills_path, entry.name });
        errdefer allocator.free(dir);

        try list.append(allocator, .{
            .name = name,
            .dir = dir,
            .skill_md = actual_md,
            .source = source,
        });
    }

    // Sort alphabetically by name
    std.mem.sort(Skill, list.items, {}, struct {
        fn lessThan(_: void, a: Skill, b: Skill) bool {
            return std.mem.order(u8, a.name, b.name) == .lt;
        }
    }.lessThan);

    return list.toOwnedSlice(allocator);
}

/// Discover skills from user, project, and explicit layers in order.
/// Returns concatenated slice: user → project → explicit.
/// Caller must call `freeSkills` on the result.
pub fn discoverLayered(
    io: std.Io,
    allocator: std.mem.Allocator,
    user_dir: ?[]const u8,
    project_dir: ?[]const u8,
    explicit_dir: ?[]const u8,
) ![]Skill {
    var all: std.ArrayList(Skill) = .empty;
    errdefer {
        for (all.items) |skill| {
            allocator.free(skill.name);
            allocator.free(skill.dir);
            allocator.free(skill.skill_md);
        }
        all.deinit(allocator);
    }

    const layers = [_]?[]const u8{ user_dir, project_dir, explicit_dir };
    const sources = [_]SkillSource{ .user, .project, .explicit };
    for (layers, sources) |maybe_dir, layer_source| {
        const d = maybe_dir orelse continue;
        const skills = try discoverIn(io, allocator, d, layer_source);
        // appendSlice may fail; if it does, free the skills and let errdefer handle rest
        all.appendSlice(allocator, skills) catch |err| {
            freeSkills(allocator, skills);
            return err;
        };
        // Ownership of strings is now in `all`; just free the outer slice
        allocator.free(skills);
    }

    return all.toOwnedSlice(allocator);
}

/// Free all strings in a skills slice and the slice itself.
pub fn freeSkills(allocator: std.mem.Allocator, skills: []Skill) void {
    for (skills) |skill| {
        allocator.free(skill.name);
        allocator.free(skill.dir);
        allocator.free(skill.skill_md);
    }
    allocator.free(skills);
}

test "empty when no skills dir" {
    const allocator = std.testing.allocator;
    var tmp = std.testing.tmpDir(.{});
    defer tmp.cleanup();

    const tmp_path = try getTmpDirPath(allocator, &tmp);
    defer allocator.free(tmp_path);

    const skills = try discoverIn(std.testing.io, allocator, tmp_path, .user);
    defer freeSkills(allocator, skills);

    try std.testing.expectEqual(@as(usize, 0), skills.len);
}

test "finds skills sorted alphabetically" {
    const allocator = std.testing.allocator;
    var tmp = std.testing.tmpDir(.{});
    defer tmp.cleanup();

    const tmp_path = try getTmpDirPath(allocator, &tmp);
    defer allocator.free(tmp_path);

    const skills_dir_path = try std.fs.path.join(allocator, &.{ tmp_path, "skills" });
    defer allocator.free(skills_dir_path);

    const zebra_dir = try std.fs.path.join(allocator, &.{ skills_dir_path, "zebra" });
    defer allocator.free(zebra_dir);
    const alpha_dir = try std.fs.path.join(allocator, &.{ skills_dir_path, "alpha" });
    defer allocator.free(alpha_dir);

    try std.Io.Dir.cwd().createDirPath(std.testing.io, skills_dir_path);
    try std.Io.Dir.cwd().createDirPath(std.testing.io, zebra_dir);
    try std.Io.Dir.cwd().createDirPath(std.testing.io, alpha_dir);

    const z_md = try std.fs.path.join(allocator, &.{ zebra_dir, "skill.md" });
    defer allocator.free(z_md);
    const a_md = try std.fs.path.join(allocator, &.{ alpha_dir, "skill.md" });
    defer allocator.free(a_md);

    const f1 = try std.Io.Dir.cwd().createFile(std.testing.io, z_md, .{});
    f1.close(std.testing.io);
    const f2 = try std.Io.Dir.cwd().createFile(std.testing.io, a_md, .{});
    f2.close(std.testing.io);

    const skills = try discoverIn(std.testing.io, allocator, tmp_path, .user);
    defer freeSkills(allocator, skills);

    try std.testing.expectEqual(@as(usize, 2), skills.len);
    try std.testing.expectEqualStrings("alpha", skills[0].name);
    try std.testing.expectEqualStrings("zebra", skills[1].name);
}

test "skips dirs without skill.md" {
    const allocator = std.testing.allocator;
    var tmp = std.testing.tmpDir(.{});
    defer tmp.cleanup();

    const tmp_path = try getTmpDirPath(allocator, &tmp);
    defer allocator.free(tmp_path);

    const skills_dir_path = try std.fs.path.join(allocator, &.{ tmp_path, "skills" });
    defer allocator.free(skills_dir_path);
    const withmd_dir = try std.fs.path.join(allocator, &.{ skills_dir_path, "withmd" });
    defer allocator.free(withmd_dir);
    const nomd_dir = try std.fs.path.join(allocator, &.{ skills_dir_path, "nomd" });
    defer allocator.free(nomd_dir);

    try std.Io.Dir.cwd().createDirPath(std.testing.io, skills_dir_path);
    try std.Io.Dir.cwd().createDirPath(std.testing.io, withmd_dir);
    try std.Io.Dir.cwd().createDirPath(std.testing.io, nomd_dir);

    const md_path = try std.fs.path.join(allocator, &.{ withmd_dir, "skill.md" });
    defer allocator.free(md_path);
    const f = try std.Io.Dir.cwd().createFile(std.testing.io, md_path, .{});
    f.close(std.testing.io);

    const skills = try discoverIn(std.testing.io, allocator, tmp_path, .user);
    defer freeSkills(allocator, skills);

    try std.testing.expectEqual(@as(usize, 1), skills.len);
    try std.testing.expectEqualStrings("withmd", skills[0].name);
}

test "layered append order" {
    const allocator = std.testing.allocator;
    var tmp = std.testing.tmpDir(.{});
    defer tmp.cleanup();

    const tmp_path = try getTmpDirPath(allocator, &tmp);
    defer allocator.free(tmp_path);

    const user_dir = try std.fs.path.join(allocator, &.{ tmp_path, "user" });
    defer allocator.free(user_dir);
    const project_dir = try std.fs.path.join(allocator, &.{ tmp_path, "project" });
    defer allocator.free(project_dir);

    const user_skills = try std.fs.path.join(allocator, &.{ user_dir, "skills" });
    defer allocator.free(user_skills);
    const user_skill_dir = try std.fs.path.join(allocator, &.{ user_skills, "user_skill" });
    defer allocator.free(user_skill_dir);
    const u_md = try std.fs.path.join(allocator, &.{ user_skill_dir, "skill.md" });
    defer allocator.free(u_md);

    try std.Io.Dir.cwd().createDirPath(std.testing.io, user_dir);
    try std.Io.Dir.cwd().createDirPath(std.testing.io, user_skills);
    try std.Io.Dir.cwd().createDirPath(std.testing.io, user_skill_dir);
    const f1 = try std.Io.Dir.cwd().createFile(std.testing.io, u_md, .{});
    f1.close(std.testing.io);

    const proj_skills = try std.fs.path.join(allocator, &.{ project_dir, "skills" });
    defer allocator.free(proj_skills);
    const proj_skill_dir = try std.fs.path.join(allocator, &.{ proj_skills, "proj_skill" });
    defer allocator.free(proj_skill_dir);
    const p_md = try std.fs.path.join(allocator, &.{ proj_skill_dir, "skill.md" });
    defer allocator.free(p_md);

    try std.Io.Dir.cwd().createDirPath(std.testing.io, project_dir);
    try std.Io.Dir.cwd().createDirPath(std.testing.io, proj_skills);
    try std.Io.Dir.cwd().createDirPath(std.testing.io, proj_skill_dir);
    const f2 = try std.Io.Dir.cwd().createFile(std.testing.io, p_md, .{});
    f2.close(std.testing.io);

    const skills = try discoverLayered(std.testing.io, allocator, user_dir, project_dir, null);
    defer freeSkills(allocator, skills);

    try std.testing.expectEqual(@as(usize, 2), skills.len);
    try std.testing.expectEqualStrings("user_skill", skills[0].name);
    try std.testing.expect(skills[0].source == .user);
    try std.testing.expectEqualStrings("proj_skill", skills[1].name);
    try std.testing.expect(skills[1].source == .project);
}

test "finds skills with case-insensitive skill.md" {
    const allocator = std.testing.allocator;
    var tmp = std.testing.tmpDir(.{});
    defer tmp.cleanup();

    const tmp_path = try getTmpDirPath(allocator, &tmp);
    defer allocator.free(tmp_path);

    const skills_dir_path = try std.fs.path.join(allocator, &.{ tmp_path, "skills" });
    defer allocator.free(skills_dir_path);

    // Create skill dirs with different case variations
    const lower_dir = try std.fs.path.join(allocator, &.{ skills_dir_path, "lower" });
    defer allocator.free(lower_dir);
    const upper_dir = try std.fs.path.join(allocator, &.{ skills_dir_path, "upper" });
    defer allocator.free(upper_dir);
    const mixed_dir = try std.fs.path.join(allocator, &.{ skills_dir_path, "mixed" });
    defer allocator.free(mixed_dir);

    try std.Io.Dir.cwd().createDirPath(std.testing.io, skills_dir_path);
    try std.Io.Dir.cwd().createDirPath(std.testing.io, lower_dir);
    try std.Io.Dir.cwd().createDirPath(std.testing.io, upper_dir);
    try std.Io.Dir.cwd().createDirPath(std.testing.io, mixed_dir);

    // Create skill.md with different cases
    const lower_md = try std.fs.path.join(allocator, &.{ lower_dir, "skill.md" });
    defer allocator.free(lower_md);
    const upper_md = try std.fs.path.join(allocator, &.{ upper_dir, "SKILL.MD" });
    defer allocator.free(upper_md);
    const mixed_md = try std.fs.path.join(allocator, &.{ mixed_dir, "SkIlL.Md" });
    defer allocator.free(mixed_md);

    const f1 = try std.Io.Dir.cwd().createFile(std.testing.io, lower_md, .{});
    f1.close(std.testing.io);
    const f2 = try std.Io.Dir.cwd().createFile(std.testing.io, upper_md, .{});
    f2.close(std.testing.io);
    const f3 = try std.Io.Dir.cwd().createFile(std.testing.io, mixed_md, .{});
    f3.close(std.testing.io);

    const skills = try discoverIn(std.testing.io, allocator, tmp_path, .user);
    defer freeSkills(allocator, skills);

    try std.testing.expectEqual(@as(usize, 3), skills.len);
    // Skills are sorted alphabetically by name
    try std.testing.expectEqualStrings("lower", skills[0].name);
    try std.testing.expectEqualStrings("mixed", skills[1].name);
    try std.testing.expectEqualStrings("upper", skills[2].name);
    
    // Verify that the skill_md paths have the correct case variation
    try std.testing.expect(std.mem.endsWith(u8, skills[0].skill_md, "skill.md"));
    try std.testing.expect(std.mem.endsWith(u8, skills[1].skill_md, "SkIlL.Md"));
    try std.testing.expect(std.mem.endsWith(u8, skills[2].skill_md, "SKILL.MD"));
}

fn getTmpDirPath(allocator: std.mem.Allocator, tmp: *std.testing.TmpDir) ![]u8 {
    var buf: [std.Io.Dir.max_path_bytes]u8 = undefined;
    const ptr = std.c.getcwd(&buf, buf.len) orelse return error.CwdError;
    const cwd_path = std.mem.sliceTo(ptr, 0);
    return try std.fs.path.join(allocator, &.{ cwd_path, ".zig-cache", "tmp", &tmp.sub_path });
}

/// Find skill.md file with case-insensitive matching.
/// Returns the actual path to the file found, or error.FileNotFound if none exists.
fn findSkillMdCaseInsensitive(io: std.Io, dir: std.Io.Dir, skill_name: []const u8, allocator: std.mem.Allocator) ![]const u8 {
    const skill_dir_path = try std.fs.path.join(allocator, &.{ skill_name });
    defer allocator.free(skill_dir_path);

    var skill_dir = dir.openDir(io, skill_dir_path, .{ .iterate = true }) catch return error.FileNotFound;
    defer skill_dir.close(io);

    var it = skill_dir.iterate();
    while (try it.next(io)) |entry| {
        if (entry.kind != .file) continue;
        if (isSkillMdFilename(entry.name)) {
            return try std.fs.path.join(allocator, &.{ skill_dir_path, entry.name });
        }
    }
    return error.FileNotFound;
}

/// Check if filename matches skill.md (case-insensitive)
fn isSkillMdFilename(name: []const u8) bool {
    const target = "skill.md";
    if (name.len != target.len) return false;
    for (name, 0..) |c, i| {
        if (std.ascii.toLower(c) != target[i]) return false;
    }
    return true;
}
