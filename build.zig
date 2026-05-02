const std = @import("std");

pub fn build(b: *std.Build) void {
    const target = b.standardTargetOptions(.{});
    const optimize = b.standardOptimizeOption(.{});

    // Dependencies
    const vaxis_dep = b.dependency("vaxis", .{
        .target = target,
        .optimize = optimize,
    });

    // Core library module
    const core_mod = b.addModule("phoenix_core", .{
        .root_source_file = b.path("src/core/core.zig"),
        .target = target,
        .optimize = optimize,
    });

    // Main executable
    const exe_mod = b.createModule(.{
        .root_source_file = b.path("src/main.zig"),
        .target = target,
        .optimize = optimize,
    });
    exe_mod.addImport("vaxis", vaxis_dep.module("vaxis"));
    exe_mod.addImport("phoenix_core", core_mod);

    const exe = b.addExecutable(.{
        .name = "phoenix",
        .root_module = exe_mod,
    });
    b.installArtifact(exe);

    // Run step
    const run_cmd = b.addRunArtifact(exe);
    run_cmd.step.dependOn(b.getInstallStep());
    if (b.args) |args| {
        run_cmd.addArgs(args);
    }
    const run_step = b.step("run", "Run phoenix");
    run_step.dependOn(&run_cmd.step);

    // Tests
    const core_tests = b.addTest(.{
        .root_module = b.createModule(.{
            .root_source_file = b.path("src/core/core.zig"),
            .target = target,
            .optimize = optimize,
        }),
    });
    const run_core_tests = b.addRunArtifact(core_tests);

    const main_tests = b.addTest(.{
        .root_module = b.createModule(.{
            .root_source_file = b.path("src/main.zig"),
            .target = target,
            .optimize = optimize,
        }),
    });
    main_tests.root_module.addImport("vaxis", vaxis_dep.module("vaxis"));
    main_tests.root_module.addImport("phoenix_core", core_mod);
    const run_main_tests = b.addRunArtifact(main_tests);

    const test_step = b.step("test", "Run tests");
    test_step.dependOn(&run_core_tests.step);
    test_step.dependOn(&run_main_tests.step);
}
