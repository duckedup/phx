use phoenix_plugin_sdk::skill;

const MAX_DIFF_BYTES: usize = 12_500; //Approx 5k tokens

skill! {
    name: "review",
    command: "review",
    description: "Review the current branch by diffing against main",
    execute(arguments) {
        let branch = if arguments.is_empty() {
            "main".to_string()
        } else {
            arguments.clone()
        };

        let git_log = run_command(
            "git",
            &["log".into(), "--oneline".into(), format!("{branch}..HEAD")],
        )
        .map(|o| o.stdout)
        .unwrap_or_default();

        let git_diff = run_command(
            "git",
            &["diff".into(), format!("{branch}...HEAD")],
        )
        .map(|o| o.stdout)
        .unwrap_or_default();

        let mut context = format!(
            "You are performing a code review of the current branch against `{branch}`.\n\
             \n\
             Review the changes for:\n\
             - **Correctness** — Logic errors, off-by-one, missing error handling.\n\
             - **Security** — Injection, credential exposure, unsafe input handling.\n\
             - **Style** — Naming, formatting, consistency with surrounding code.\n\
             - **Tests** — Are new code paths tested? Are edge cases covered?\n\
             - **Performance** — Unnecessary allocations, N+1 queries, blocking in async.\n\
             \n\
             Summarize your findings:\n\
             - List issues found (if any), ordered by severity.\n\
             - Call out anything that looks good or well-done.\n\
             - Provide a short overall verdict: approve, request changes, or comment.\n\
             \n\
             Keep your review concise and actionable. Focus on what matters — skip nitpicks unless asked."
        );

        if !git_log.is_empty() {
            context.push_str(&format!(
                "\n\n## Commits ({branch}..HEAD)\n\n```\n{git_log}```"
            ));
        }

        if !git_diff.is_empty() {
            if git_diff.len() <= MAX_DIFF_BYTES {
                context.push_str(&format!(
                    "\n\n## Diff ({branch}...HEAD)\n\n```diff\n{git_diff}```"
                ));
            } else {
                let diff_stat = run_command(
                    "git",
                    &["diff".into(), "--stat".into(), format!("{branch}...HEAD")],
                )
                .map(|o| o.stdout)
                .unwrap_or_default();

                let file_list = run_command(
                    "git",
                    &["diff".into(), "--name-only".into(), format!("{branch}...HEAD")],
                )
                .map(|o| o.stdout)
                .unwrap_or_default();

                context.push_str(&format!(
                    "\n\n## Changed files ({branch}...HEAD)\n\n\
                     The diff is too large to include inline ({} bytes). \
                     Use `read` and `bash` tools to inspect specific files.\n\n\
                     ```\n{diff_stat}```\n\n\
                     Files changed:\n",
                    git_diff.len()
                ));
                for file in file_list.lines() {
                    if !file.is_empty() {
                        context.push_str(&format!("- `{file}`\n"));
                    }
                }
                context.push_str(&format!(
                    "\nTo see the diff for a specific file, run: \
                     `git diff {branch}...HEAD -- <file>`"
                ));
            }
        }

        Ok(phoenix_plugin_sdk::SkillResult::with_toast(
            context,
            "Review mode activated.",
        ))
    }
}
