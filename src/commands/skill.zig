const std = @import("std");
const core = @import("phoenix_core");
const dispatcher = @import("dispatcher.zig");

/// Look up a skill by command name. Skills are stored in `config.skills` with
/// later layers (project, explicit) shadowing earlier ones (user) on duplicate
/// names — return the *last* match so explicit/project overrides win.
pub fn findSkill(config: *const core.Config, name: []const u8) ?core.Skill {
    var found: ?core.Skill = null;
    for (config.skills) |s| {
        if (std.mem.eql(u8, s.name, name)) found = s;
    }
    return found;
}

const max_skill_bytes: usize = 256 * 1024;

pub fn handle(
    ctx: dispatcher.DispatchCtx,
    skill: core.Skill,
    args: []const u8,
    out: std.mem.Allocator,
) !dispatcher.Result {
    // Read skill.md. Cap at 256 KiB.
    const body = std.Io.Dir.cwd().readFileAlloc(
        ctx.io,
        skill.skill_md,
        out,
        .limited(max_skill_bytes),
    ) catch |err| {
        const msg = try std.fmt.allocPrint(
            out,
            "skill {s}: failed to read {s}: {s}",
            .{ skill.name, skill.skill_md, @errorName(err) },
        );
        return .{ .err = msg };
    };

    const label = try std.fmt.allocPrint(out, "skill:{s}", .{skill.name});
    return .{
        .inject_context = .{
            .label = label,
            .body = body,
            .user_message = try out.dupe(u8, args),
        },
    };
}

// ---- Tests ----

fn tmpHome(allocator: std.mem.Allocator, tmp: *std.testing.TmpDir) ![]u8 {
    var buf: [std.Io.Dir.max_path_bytes]u8 = undefined;
    const ptr = std.c.getcwd(&buf, buf.len) orelse return error.CwdError;
    const cwd_path = std.mem.sliceTo(ptr, 0);
    return try std.fs.path.join(allocator, &.{ cwd_path, ".zig-cache", "tmp", &tmp.sub_path, "home" });
}

test "findSkill returns last match" {
    const allocator = std.testing.allocator;
    var tmp = std.testing.tmpDir(.{});
    defer tmp.cleanup();
    const home = try tmpHome(allocator, &tmp);
    defer allocator.free(home);

    // user_dir = home (Config.load sets user_dir = home/.phoenix)
    // proj_dir = home/project (Config.load sets project_dir = home/project/.phoenix)
    const proj_base = try std.fs.path.join(allocator, &.{ home, "project" });
    defer allocator.free(proj_base);

    // Skills discovered from user_dir/.phoenix/skills/ and project_dir/.phoenix/skills/
    const user_skills = try std.fs.path.join(allocator, &.{ home, ".phoenix", "skills", "research" });
    defer allocator.free(user_skills);
    const proj_skills = try std.fs.path.join(allocator, &.{ proj_base, ".phoenix", "skills", "research" });
    defer allocator.free(proj_skills);

    try std.Io.Dir.cwd().createDirPath(std.testing.io, user_skills);
    try std.Io.Dir.cwd().createDirPath(std.testing.io, proj_skills);

    const u_md = try std.fs.path.join(allocator, &.{ user_skills, "skill.md" });
    defer allocator.free(u_md);
    const p_md = try std.fs.path.join(allocator, &.{ proj_skills, "skill.md" });
    defer allocator.free(p_md);

    const f1 = try std.Io.Dir.cwd().createFile(std.testing.io, u_md, .{});
    f1.close(std.testing.io);
    try std.Io.Dir.cwd().writeFile(std.testing.io, .{ .sub_path = p_md, .data = "project content" });

    var cfg = try core.Config.load(allocator, std.testing.io, .{ .home = home, .cwd = proj_base });
    defer cfg.deinit();

    const found = findSkill(&cfg, "research");
    try std.testing.expect(found != null);
    try std.testing.expect(found.?.source == .project);
}

test "skill handle reads body and sets user_message" {
    const allocator = std.testing.allocator;
    var tmp = std.testing.tmpDir(.{});
    defer tmp.cleanup();
    const home = try tmpHome(allocator, &tmp);
    defer allocator.free(home);

    const skills_dir = try std.fs.path.join(allocator, &.{ home, "skills", "research" });
    defer allocator.free(skills_dir);
    try std.Io.Dir.cwd().createDirPath(std.testing.io, skills_dir);
    const md_path = try std.fs.path.join(allocator, &.{ skills_dir, "skill.md" });
    defer allocator.free(md_path);
    try std.Io.Dir.cwd().writeFile(std.testing.io, .{ .sub_path = md_path, .data = "research skill body" });

    const skill = core.Skill{
        .name = "research",
        .dir = skills_dir,
        .skill_md = md_path,
        .source = .user,
    };

    var cfg = try core.Config.load(allocator, std.testing.io, .{ .home = null });
    defer cfg.deinit();

    var out_arena = std.heap.ArenaAllocator.init(allocator);
    defer out_arena.deinit();

    const result = try handle(
        .{ .io = std.testing.io, .gpa = allocator, .home = null, .config = &cfg },
        skill,
        "my query",
        out_arena.allocator(),
    );

    switch (result) {
        .inject_context => |ctx| {
            try std.testing.expectEqualStrings("research skill body", ctx.body);
            try std.testing.expectEqualStrings("my query", ctx.user_message);
            try std.testing.expectEqualStrings("skill:research", ctx.label);
        },
        else => return error.TestUnexpectedResult,
    }
}
