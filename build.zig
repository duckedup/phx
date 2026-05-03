const std = @import("std");

pub fn build(b: *std.Build) void {
    const target = b.standardTargetOptions(.{});
    const optimize = b.standardOptimizeOption(.{});

    const vaxis_dep = b.dependency("vaxis", .{
        .target = target,
        .optimize = optimize,
    });

    const core_mod = b.addModule("phoenix_core", .{
        .root_source_file = b.path("src/core/core.zig"),
        .target = target,
        .optimize = optimize,
    });

    // Shared test utilities for provider adapter tests
    const test_util_mod = b.createModule(.{
        .root_source_file = b.path("src/providers/test_util.zig"),
        .target = target,
        .optimize = optimize,
    });
    test_util_mod.addImport("phoenix_core", core_mod);

    // Provider adapter modules (imported by provider_registry.zig)
    const providers_claude = b.createModule(.{
        .root_source_file = b.path("src/providers/claude.zig"),
        .target = target,
        .optimize = optimize,
    });
    providers_claude.addImport("phoenix_core", core_mod);
    providers_claude.addImport("test_util", test_util_mod);
    core_mod.addImport("providers_claude", providers_claude);

    const providers_openai = b.createModule(.{
        .root_source_file = b.path("src/providers/openai.zig"),
        .target = target,
        .optimize = optimize,
    });
    providers_openai.addImport("phoenix_core", core_mod);
    providers_openai.addImport("test_util", test_util_mod);
    core_mod.addImport("providers_openai", providers_openai);

    const providers_ollama = b.createModule(.{
        .root_source_file = b.path("src/providers/ollama.zig"),
        .target = target,
        .optimize = optimize,
    });
    providers_ollama.addImport("phoenix_core", core_mod);
    providers_ollama.addImport("test_util", test_util_mod);
    core_mod.addImport("providers_ollama", providers_ollama);

    const providers_llamacpp = b.createModule(.{
        .root_source_file = b.path("src/providers/llamacpp.zig"),
        .target = target,
        .optimize = optimize,
    });
    providers_llamacpp.addImport("phoenix_core", core_mod);
    providers_llamacpp.addImport("providers_openai", providers_openai);
    providers_llamacpp.addImport("test_util", test_util_mod);
    core_mod.addImport("providers_llamacpp", providers_llamacpp);

    const providers_google = b.createModule(.{
        .root_source_file = b.path("src/providers/google.zig"),
        .target = target,
        .optimize = optimize,
    });
    providers_google.addImport("phoenix_core", core_mod);
    providers_google.addImport("test_util", test_util_mod);
    core_mod.addImport("providers_google", providers_google);

    const commands_mod = b.createModule(.{
        .root_source_file = b.path("src/commands/dispatcher.zig"),
        .target = target,
        .optimize = optimize,
    });
    commands_mod.addImport("phoenix_core", core_mod);

    const rpc_mod = b.createModule(.{
        .root_source_file = b.path("src/rpc/rpc.zig"),
        .target = target,
        .optimize = optimize,
    });
    rpc_mod.addImport("phoenix_core", core_mod);
    rpc_mod.addImport("commands", commands_mod);

    const exe_mod = b.createModule(.{
        .root_source_file = b.path("src/main.zig"),
        .target = target,
        .optimize = optimize,
    });
    exe_mod.addImport("vaxis", vaxis_dep.module("vaxis"));
    exe_mod.addImport("phoenix_core", core_mod);
    exe_mod.addImport("commands", commands_mod);
    exe_mod.addImport("rpc", rpc_mod);

    const exe = b.addExecutable(.{
        .name = "phoenix",
        .root_module = exe_mod,
    });
    b.installArtifact(exe);

    const run_cmd = b.addRunArtifact(exe);
    run_cmd.step.dependOn(b.getInstallStep());
    if (b.args) |args| {
        run_cmd.addArgs(args);
    }
    const run_step = b.step("run", "Run phoenix");
    run_step.dependOn(&run_cmd.step);

    const core_test_mod = b.createModule(.{
        .root_source_file = b.path("src/core/core.zig"),
        .target = target,
        .optimize = optimize,
    });

    // Shared test utilities for test build
    const test_test_util_mod = b.createModule(.{
        .root_source_file = b.path("src/providers/test_util.zig"),
        .target = target,
        .optimize = optimize,
    });
    test_test_util_mod.addImport("phoenix_core", core_test_mod);

    // Provider adapter modules for test build
    const test_providers_claude = b.createModule(.{
        .root_source_file = b.path("src/providers/claude.zig"),
        .target = target,
        .optimize = optimize,
    });
    test_providers_claude.addImport("phoenix_core", core_test_mod);
    test_providers_claude.addImport("test_util", test_test_util_mod);
    core_test_mod.addImport("providers_claude", test_providers_claude);

    const test_providers_openai = b.createModule(.{
        .root_source_file = b.path("src/providers/openai.zig"),
        .target = target,
        .optimize = optimize,
    });
    test_providers_openai.addImport("phoenix_core", core_test_mod);
    test_providers_openai.addImport("test_util", test_test_util_mod);
    core_test_mod.addImport("providers_openai", test_providers_openai);

    const test_providers_ollama = b.createModule(.{
        .root_source_file = b.path("src/providers/ollama.zig"),
        .target = target,
        .optimize = optimize,
    });
    test_providers_ollama.addImport("phoenix_core", core_test_mod);
    test_providers_ollama.addImport("test_util", test_test_util_mod);
    core_test_mod.addImport("providers_ollama", test_providers_ollama);

    const test_providers_llamacpp = b.createModule(.{
        .root_source_file = b.path("src/providers/llamacpp.zig"),
        .target = target,
        .optimize = optimize,
    });
    test_providers_llamacpp.addImport("phoenix_core", core_test_mod);
    test_providers_llamacpp.addImport("providers_openai", test_providers_openai);
    test_providers_llamacpp.addImport("test_util", test_test_util_mod);
    core_test_mod.addImport("providers_llamacpp", test_providers_llamacpp);

    const test_providers_google = b.createModule(.{
        .root_source_file = b.path("src/providers/google.zig"),
        .target = target,
        .optimize = optimize,
    });
    test_providers_google.addImport("phoenix_core", core_test_mod);
    test_providers_google.addImport("test_util", test_test_util_mod);
    core_test_mod.addImport("providers_google", test_providers_google);

    const core_tests = b.addTest(.{
        .root_module = core_test_mod,
    });
    const run_core_tests = b.addRunArtifact(core_tests);

    const test_commands_mod = b.createModule(.{
        .root_source_file = b.path("src/commands/dispatcher.zig"),
        .target = target,
        .optimize = optimize,
    });
    test_commands_mod.addImport("phoenix_core", core_test_mod);

    const test_rpc_mod = b.createModule(.{
        .root_source_file = b.path("src/rpc/rpc.zig"),
        .target = target,
        .optimize = optimize,
    });
    test_rpc_mod.addImport("phoenix_core", core_test_mod);
    test_rpc_mod.addImport("commands", test_commands_mod);

    const main_test_mod = b.createModule(.{
        .root_source_file = b.path("src/main.zig"),
        .target = target,
        .optimize = optimize,
    });
    main_test_mod.addImport("vaxis", vaxis_dep.module("vaxis"));
    main_test_mod.addImport("phoenix_core", core_mod);
    main_test_mod.addImport("commands", commands_mod);
    main_test_mod.addImport("rpc", rpc_mod);

    const main_tests = b.addTest(.{
        .root_module = main_test_mod,
    });
    const run_main_tests = b.addRunArtifact(main_tests);

    // Provider adapter test binaries (each module tested independently)
    const claude_tests = b.addTest(.{ .root_module = test_providers_claude });
    const run_claude_tests = b.addRunArtifact(claude_tests);

    const openai_tests = b.addTest(.{ .root_module = test_providers_openai });
    const run_openai_tests = b.addRunArtifact(openai_tests);

    const ollama_tests = b.addTest(.{ .root_module = test_providers_ollama });
    const run_ollama_tests = b.addRunArtifact(ollama_tests);

    const llamacpp_tests = b.addTest(.{ .root_module = test_providers_llamacpp });
    const run_llamacpp_tests = b.addRunArtifact(llamacpp_tests);

    const google_tests = b.addTest(.{ .root_module = test_providers_google });
    const run_google_tests = b.addRunArtifact(google_tests);

    const commands_tests = b.addTest(.{ .root_module = test_commands_mod });
    const run_commands_tests = b.addRunArtifact(commands_tests);

    const rpc_tests = b.addTest(.{ .root_module = test_rpc_mod });
    const run_rpc_tests = b.addRunArtifact(rpc_tests);

    const test_step = b.step("test", "Run tests");
    test_step.dependOn(&run_core_tests.step);
    test_step.dependOn(&run_main_tests.step);
    test_step.dependOn(&run_claude_tests.step);
    test_step.dependOn(&run_openai_tests.step);
    test_step.dependOn(&run_ollama_tests.step);
    test_step.dependOn(&run_llamacpp_tests.step);
    test_step.dependOn(&run_google_tests.step);
    test_step.dependOn(&run_commands_tests.step);
    test_step.dependOn(&run_rpc_tests.step);
}
