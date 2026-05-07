use std::collections::HashSet;
use std::path::{Path, PathBuf};

use crate::config::schema::{ProviderProfile, SessionProfile};
use crate::providers::model_info;
use crate::session::message::{Message, Role};

// ---------------------------------------------------------------------------
// Rules
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RuleSource {
    Global,
    Project,
}

#[derive(Debug, Clone)]
pub struct Rule {
    pub path: PathBuf,
    pub content: String,
    pub globs: Option<Vec<String>>,
    pub source: RuleSource,
}

// ---------------------------------------------------------------------------
// Agent context (AGENTS.md / CLAUDE.md)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct AgentContext {
    pub path: PathBuf,
    pub content: String,
    pub dir: PathBuf,
    pub is_root: bool,
}

// ---------------------------------------------------------------------------
// Context limits
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy)]
pub struct ContextLimits {
    pub context_window: u32,
    pub max_output: u32,
    pub threshold: f64,
}

pub struct CompactionResult {
    pub removed_count: usize,
    pub remaining_count: usize,
    pub was_compacted: bool,
}

// ---------------------------------------------------------------------------
// Context state (persists across loop iterations within a session)
// ---------------------------------------------------------------------------

#[derive(Default)]
pub struct ContextState {
    loaded_rules: HashSet<PathBuf>,
    loaded_agents: HashSet<PathBuf>,
    pub activated_skills: HashSet<String>,
    cached_rules: Option<Vec<Rule>>,
    cached_agents: Option<Vec<AgentContext>>,
    touched_files: HashSet<PathBuf>,
    last_scanned_idx: usize,
}

// ---------------------------------------------------------------------------
// Context result
// ---------------------------------------------------------------------------

pub struct ContextResult {
    pub system_prompt_suffix: String,
    pub newly_loaded: Vec<String>,
}

// ---------------------------------------------------------------------------
// Frontmatter parsing
// ---------------------------------------------------------------------------

pub fn parse_frontmatter(content: &str) -> (Option<Vec<String>>, String) {
    let trimmed = content.trim_start();
    if !trimmed.starts_with("---") {
        return (None, content.to_string());
    }

    let after_first = &trimmed[3..];
    let after_first = after_first.strip_prefix('\n').unwrap_or(after_first);

    let Some(end_idx) = after_first.find("\n---") else {
        return (None, content.to_string());
    };

    let frontmatter = &after_first[..end_idx];
    let body_start = end_idx + 4; // "\n---"
    let body = after_first[body_start..]
        .strip_prefix('\n')
        .unwrap_or(&after_first[body_start..]);

    let mut globs: Vec<String> = Vec::new();
    let mut in_paths = false;

    for line in frontmatter.lines() {
        let trimmed_line = line.trim();
        if trimmed_line.starts_with("paths:") {
            in_paths = true;
            continue;
        }
        if in_paths {
            if let Some(item) = trimmed_line.strip_prefix("- ") {
                let item = item.trim();
                let item = item
                    .strip_prefix('"')
                    .and_then(|s| s.strip_suffix('"'))
                    .or_else(|| item.strip_prefix('\'').and_then(|s| s.strip_suffix('\'')))
                    .unwrap_or(item);
                if !item.is_empty() {
                    globs.push(item.to_string());
                }
            } else if !trimmed_line.is_empty() {
                in_paths = false;
            }
        }
    }

    let result_globs = if globs.is_empty() { None } else { Some(globs) };
    (result_globs, body.to_string())
}

// ---------------------------------------------------------------------------
// Rule discovery
// ---------------------------------------------------------------------------

fn discover_rules_in(dir: &Path, source: RuleSource) -> Vec<Rule> {
    let rules_dir = dir.join("rules");
    let Ok(entries) = std::fs::read_dir(&rules_dir) else {
        return vec![];
    };
    let mut rules = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("md") {
            continue;
        }
        if !entry.file_type().is_ok_and(|ft| ft.is_file()) {
            continue;
        }
        let Ok(raw) = std::fs::read_to_string(&path) else {
            continue;
        };
        let (globs, content) = parse_frontmatter(&raw);
        rules.push(Rule {
            path,
            content,
            globs,
            source: source.clone(),
        });
    }
    rules.sort_by(|a, b| a.path.cmp(&b.path));
    rules
}

pub fn discover_rules(home: &Path, project: &Path) -> Vec<Rule> {
    let phoenix_home = home.join(".phoenix");
    let phoenix_project = project.join(".phoenix");
    let mut all = discover_rules_in(&phoenix_home, RuleSource::Global);
    all.extend(discover_rules_in(&phoenix_project, RuleSource::Project));
    all
}

// ---------------------------------------------------------------------------
// AGENTS.md / CLAUDE.md discovery
// ---------------------------------------------------------------------------

const SKIP_DIRS: &[&str] = &[
    ".git",
    "node_modules",
    "target",
    ".next",
    ".nuxt",
    "dist",
    "__pycache__",
    ".venv",
    "vendor",
];

const MAX_WALK_DEPTH: usize = 10;

fn walk_for_file(dir: &Path, filename: &str, project: &Path, depth: usize) -> Vec<AgentContext> {
    if depth > MAX_WALK_DEPTH {
        return vec![];
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return vec![];
    };

    let mut results = Vec::new();

    let target = dir.join(filename);
    if target.is_file()
        && let Ok(content) = std::fs::read_to_string(&target)
    {
        let is_root = dir == project;
        results.push(AgentContext {
            path: target,
            content,
            dir: dir.to_path_buf(),
            is_root,
        });
    }

    for entry in entries.flatten() {
        if !entry.file_type().is_ok_and(|ft| ft.is_dir()) {
            continue;
        }
        let name = entry.file_name();
        let name_str = name.to_string_lossy();
        if name_str.starts_with('.') || SKIP_DIRS.contains(&name_str.as_ref()) {
            continue;
        }
        results.extend(walk_for_file(&entry.path(), filename, project, depth + 1));
    }

    results
}

pub fn discover_agent_contexts(project: &Path) -> Vec<AgentContext> {
    let agents = walk_for_file(project, "AGENTS.md", project, 0);
    if !agents.is_empty() {
        return agents;
    }
    walk_for_file(project, "CLAUDE.md", project, 0)
}

// ---------------------------------------------------------------------------
// File path extraction from tool calls
// ---------------------------------------------------------------------------

pub fn extract_touched_files(messages: &[Message], start_idx: usize) -> HashSet<PathBuf> {
    let mut paths = HashSet::new();

    for msg in messages.iter().skip(start_idx) {
        if msg.role != Role::ToolCall {
            continue;
        }
        let Some(tc) = &msg.tool_call else {
            continue;
        };

        match tc.name.as_str() {
            "read" | "write" | "edit" => {
                if let Ok(args) = serde_json::from_str::<serde_json::Value>(&tc.args_json)
                    && let Some(fp) = args.get("file_path").and_then(|v| v.as_str())
                {
                    paths.insert(PathBuf::from(fp));
                }
            }
            "bash" => {
                if let Ok(args) = serde_json::from_str::<serde_json::Value>(&tc.args_json)
                    && let Some(cmd) = args.get("command").and_then(|v| v.as_str())
                {
                    for token in cmd.split_whitespace() {
                        if token.contains('/') && !token.starts_with('-') {
                            let clean = token.trim_matches(|c: char| {
                                c == '\'' || c == '"' || c == ';' || c == '|' || c == '>'
                            });
                            if !clean.is_empty() {
                                paths.insert(PathBuf::from(clean));
                            }
                        }
                    }
                }
            }
            _ => {}
        }
    }

    paths
}

// ---------------------------------------------------------------------------
// Glob matching
// ---------------------------------------------------------------------------

fn file_matches_globs(file: &Path, globs: &[String], project: &Path) -> bool {
    let relative = file.strip_prefix(project).unwrap_or(file);
    let rel_str = relative.to_string_lossy();

    for pattern in globs {
        if let Ok(pat) = glob::Pattern::new(pattern)
            && pat.matches(&rel_str)
        {
            return true;
        }
    }
    false
}

fn file_is_under(file: &Path, dir: &Path) -> bool {
    file.starts_with(dir)
}

// ---------------------------------------------------------------------------
// Context composition
// ---------------------------------------------------------------------------

pub fn compute_context(
    rules: &[Rule],
    agents: &[AgentContext],
    touched_files: &HashSet<PathBuf>,
    project: &Path,
    state: &mut ContextState,
) -> ContextResult {
    let mut rule_sections = Vec::new();
    let mut agent_sections = Vec::new();
    let mut newly_loaded = Vec::new();

    for rule in rules {
        let active = match &rule.globs {
            None => true,
            Some(globs) => touched_files
                .iter()
                .any(|f| file_matches_globs(f, globs, project)),
        };
        if !active {
            continue;
        }

        let display = rule
            .path
            .strip_prefix(project)
            .map(|p| p.display().to_string())
            .unwrap_or_else(|_| rule.path.display().to_string());

        if state.loaded_rules.insert(rule.path.clone()) {
            newly_loaded.push(display.clone());
        }

        rule_sections.push(format!("# Rule: {display}\n{}", rule.content));
    }

    for agent in agents {
        let active = agent.is_root || touched_files.iter().any(|f| file_is_under(f, &agent.dir));
        if !active {
            continue;
        }

        let display = agent
            .path
            .strip_prefix(project)
            .map(|p| p.display().to_string())
            .unwrap_or_else(|_| agent.path.display().to_string());

        if state.loaded_agents.insert(agent.path.clone()) {
            newly_loaded.push(display.clone());
        }

        let label = if agent.is_root {
            format!(
                "# {} (root)",
                agent.path.file_name().unwrap_or_default().to_string_lossy()
            )
        } else {
            let rel_dir = agent.dir.strip_prefix(project).unwrap_or(&agent.dir);
            format!(
                "# {} ({})",
                agent.path.file_name().unwrap_or_default().to_string_lossy(),
                rel_dir.display()
            )
        };
        agent_sections.push(format!("{label}\n{}", agent.content));
    }

    let mut suffix = String::new();
    if !rule_sections.is_empty() {
        suffix.push_str("<rules>\n");
        suffix.push_str(&rule_sections.join("\n\n"));
        suffix.push_str("\n</rules>");
    }
    if !agent_sections.is_empty() {
        if !suffix.is_empty() {
            suffix.push_str("\n\n");
        }
        suffix.push_str("<agents-context>\n");
        suffix.push_str(&agent_sections.join("\n\n"));
        suffix.push_str("\n</agents-context>");
    }

    ContextResult {
        system_prompt_suffix: suffix,
        newly_loaded,
    }
}

// ---------------------------------------------------------------------------
// Top-level build_context
// ---------------------------------------------------------------------------

pub fn build_context(
    home: &Path,
    project: &Path,
    messages: &[Message],
    state: &mut ContextState,
    skills: &[crate::session::skills::Skill],
) -> ContextResult {
    if state.cached_rules.is_none() {
        state.cached_rules = Some(discover_rules(home, project));
    }
    if state.cached_agents.is_none() {
        state.cached_agents = Some(discover_agent_contexts(project));
    }

    let new_files = extract_touched_files(messages, state.last_scanned_idx);
    state.touched_files.extend(new_files);
    state.last_scanned_idx = messages.len();

    let rules = state.cached_rules.clone().unwrap_or_default();
    let agents = state.cached_agents.clone().unwrap_or_default();
    let touched = state.touched_files.clone();

    let mut result = compute_context(&rules, &agents, &touched, project, state);

    let catalog = crate::session::skills::build_skill_catalog(skills);
    if !catalog.is_empty() {
        if !result.system_prompt_suffix.is_empty() {
            result.system_prompt_suffix.push_str("\n\n");
        }
        result.system_prompt_suffix.push_str(&catalog);
    }

    result
}

// ---------------------------------------------------------------------------
// Context limits resolution
// ---------------------------------------------------------------------------

pub fn resolve_context_limits(
    model_name: &str,
    provider_profile: &ProviderProfile,
    session_profile: &SessionProfile,
) -> ContextLimits {
    let mut context_window: u32 = 128_000;

    for model in model_info::known_models() {
        if model.id == model_name {
            context_window = model.context_window;
            break;
        }
    }

    if let Some(cw) = provider_profile.context_window {
        context_window = cw;
    }
    if let Some(budget) = session_profile.token_budget {
        context_window = budget as u32;
    }

    let max_output = provider_profile.max_tokens.unwrap_or(16_384);

    let threshold = std::env::var("PHOENIX_COMPACT_THRESHOLD")
        .ok()
        .and_then(|v| v.parse::<f64>().ok())
        .unwrap_or(0.85)
        .clamp(0.1, 1.0);

    ContextLimits {
        context_window,
        max_output,
        threshold,
    }
}

// ---------------------------------------------------------------------------
// Token estimation
// ---------------------------------------------------------------------------

pub fn estimate_tokens(messages: &[Message], system_prompt: &str) -> u64 {
    let mut chars: u64 = system_prompt.len() as u64;
    for msg in messages {
        chars += msg.content.len() as u64;
        if let Some(tc) = &msg.tool_call {
            chars += tc.args_json.len() as u64;
            chars += tc.name.len() as u64;
        }
        if let Some(tr) = &msg.tool_result {
            chars += tr.output.len() as u64;
        }
    }
    chars / 4
}

// ---------------------------------------------------------------------------
// Compaction
// ---------------------------------------------------------------------------

pub fn should_compact(estimated_tokens: u64, limits: &ContextLimits) -> bool {
    let input_budget = limits.context_window.saturating_sub(limits.max_output);
    let threshold_tokens = (input_budget as f64 * limits.threshold) as u64;
    estimated_tokens > threshold_tokens
}

pub fn compact_messages(messages: &mut Vec<Message>, limits: &ContextLimits) -> CompactionResult {
    let original_count = messages.len();
    if original_count <= 4 {
        return CompactionResult {
            removed_count: 0,
            remaining_count: original_count,
            was_compacted: false,
        };
    }

    let target_budget =
        (limits.context_window.saturating_sub(limits.max_output) as f64 * 0.4) as u64;

    // Find system messages and first user message to preserve
    let mut preserve_head = 0;
    for (i, msg) in messages.iter().enumerate() {
        if msg.role == Role::System {
            preserve_head = i + 1;
        } else if msg.role == Role::User {
            preserve_head = i + 1;
            break;
        } else {
            break;
        }
    }
    preserve_head = preserve_head.max(1);

    // Count from the end to find how many messages fit in ~40% of budget
    let mut tail_tokens: u64 = 0;
    let mut preserve_tail_start = messages.len();
    for i in (preserve_head..messages.len()).rev() {
        let msg = &messages[i];
        let msg_tokens = (msg.content.len() as u64) / 4;
        let tc_tokens = msg
            .tool_call
            .as_ref()
            .map(|tc| (tc.args_json.len() + tc.name.len()) as u64 / 4)
            .unwrap_or(0);
        let tr_tokens = msg
            .tool_result
            .as_ref()
            .map(|tr| tr.output.len() as u64 / 4)
            .unwrap_or(0);
        let total = msg_tokens + tc_tokens + tr_tokens;

        if tail_tokens + total > target_budget {
            break;
        }
        tail_tokens += total;
        preserve_tail_start = i;
    }

    if preserve_tail_start <= preserve_head {
        return CompactionResult {
            removed_count: 0,
            remaining_count: original_count,
            was_compacted: false,
        };
    }

    let protected: Vec<&Message> = messages[preserve_head..preserve_tail_start]
        .iter()
        .filter(|m| m.content.contains("<skill_content"))
        .collect();

    let removed_count = (preserve_tail_start - preserve_head) - protected.len();

    let mut compacted = Vec::with_capacity(messages.len() - removed_count + 1);
    compacted.extend_from_slice(&messages[..preserve_head]);
    for msg in &protected {
        compacted.push((*msg).clone());
    }
    compacted.push(Message::system(format!(
        "[Earlier conversation compacted — {removed_count} messages removed to stay within context limits]"
    )));
    compacted.extend_from_slice(&messages[preserve_tail_start..]);

    let remaining_count = compacted.len();
    *messages = compacted;

    CompactionResult {
        removed_count,
        remaining_count,
        was_compacted: true,
    }
}

pub fn enforce_limits(
    messages: &mut Vec<Message>,
    system_prompt: &str,
    limits: &ContextLimits,
) -> CompactionResult {
    let estimated = estimate_tokens(messages, system_prompt);
    if should_compact(estimated, limits) {
        compact_messages(messages, limits)
    } else {
        CompactionResult {
            removed_count: 0,
            remaining_count: messages.len(),
            was_compacted: false,
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::message::ToolCall;
    use tempfile::tempdir;

    #[test]
    fn parse_frontmatter_no_frontmatter() {
        let content = "Just some markdown\n## Heading\nBody text.";
        let (globs, body) = parse_frontmatter(content);
        assert!(globs.is_none());
        assert_eq!(body, content);
    }

    #[test]
    fn parse_frontmatter_with_paths() {
        let content =
            "---\npaths:\n  - \"**/*.test.ts\"\n  - \"**/*.test.tsx\"\n---\nRule content here.";
        let (globs, body) = parse_frontmatter(content);
        let globs = globs.unwrap();
        assert_eq!(globs, vec!["**/*.test.ts", "**/*.test.tsx"]);
        assert_eq!(body, "Rule content here.");
    }

    #[test]
    fn parse_frontmatter_single_quotes() {
        let content = "---\npaths:\n  - '*.rs'\n---\nRust rules.";
        let (globs, body) = parse_frontmatter(content);
        let globs = globs.unwrap();
        assert_eq!(globs, vec!["*.rs"]);
        assert_eq!(body, "Rust rules.");
    }

    #[test]
    fn parse_frontmatter_no_paths_key() {
        let content = "---\ntitle: My Rule\n---\nContent.";
        let (globs, body) = parse_frontmatter(content);
        assert!(globs.is_none());
        assert_eq!(body, "Content.");
    }

    #[test]
    fn discover_rules_from_dirs() {
        let home = tempdir().unwrap();
        let project = tempdir().unwrap();

        let global_rules = home.path().join(".phoenix/rules");
        std::fs::create_dir_all(&global_rules).unwrap();
        std::fs::write(global_rules.join("style.md"), "Always use tabs.").unwrap();

        let project_rules = project.path().join(".phoenix/rules");
        std::fs::create_dir_all(&project_rules).unwrap();
        std::fs::write(
            project_rules.join("testing.md"),
            "---\npaths:\n  - \"**/*.test.ts\"\n---\nUse vitest.",
        )
        .unwrap();

        let rules = discover_rules(home.path(), project.path());
        assert_eq!(rules.len(), 2);
        assert_eq!(rules[0].source, RuleSource::Global);
        assert!(rules[0].globs.is_none());
        assert_eq!(rules[0].content, "Always use tabs.");
        assert_eq!(rules[1].source, RuleSource::Project);
        assert!(rules[1].globs.is_some());
    }

    #[test]
    fn discover_agents_md() {
        let project = tempdir().unwrap();
        std::fs::write(project.path().join("AGENTS.md"), "Root agent rules.").unwrap();
        let sub = project.path().join("src/components");
        std::fs::create_dir_all(&sub).unwrap();
        std::fs::write(sub.join("AGENTS.md"), "Component rules.").unwrap();

        let agents = discover_agent_contexts(project.path());
        assert_eq!(agents.len(), 2);

        let root = agents.iter().find(|a| a.is_root).unwrap();
        assert!(root.content.contains("Root agent"));

        let sub_agent = agents.iter().find(|a| !a.is_root).unwrap();
        assert!(sub_agent.content.contains("Component"));
    }

    #[test]
    fn claude_md_fallback() {
        let project = tempdir().unwrap();
        std::fs::write(project.path().join("CLAUDE.md"), "Claude fallback.").unwrap();

        let agents = discover_agent_contexts(project.path());
        assert_eq!(agents.len(), 1);
        assert!(agents[0].content.contains("Claude fallback"));
        assert!(agents[0].is_root);
    }

    #[test]
    fn agents_md_takes_precedence_over_claude_md() {
        let project = tempdir().unwrap();
        std::fs::write(project.path().join("AGENTS.md"), "Agent rules.").unwrap();
        std::fs::write(project.path().join("CLAUDE.md"), "Should not load.").unwrap();

        let agents = discover_agent_contexts(project.path());
        assert_eq!(agents.len(), 1);
        assert!(agents[0].content.contains("Agent rules"));
    }

    #[test]
    fn extract_touched_files_from_tool_calls() {
        let messages = vec![
            Message::user("read the config"),
            Message {
                role: Role::ToolCall,
                content: String::new(),
                tool_call: Some(ToolCall {
                    id: "1".into(),
                    name: "read".into(),
                    args_json: r#"{"file_path": "/project/src/config.ts"}"#.into(),
                }),
                tool_result: None,
            },
            Message {
                role: Role::ToolCall,
                content: String::new(),
                tool_call: Some(ToolCall {
                    id: "2".into(),
                    name: "edit".into(),
                    args_json: r#"{"file_path": "/project/src/utils.ts", "old_string": "a", "new_string": "b"}"#.into(),
                }),
                tool_result: None,
            },
        ];

        let files = extract_touched_files(&messages, 0);
        assert!(files.contains(&PathBuf::from("/project/src/config.ts")));
        assert!(files.contains(&PathBuf::from("/project/src/utils.ts")));
    }

    #[test]
    fn compute_context_glob_matching() {
        let project = PathBuf::from("/project");
        let rules = vec![
            Rule {
                path: PathBuf::from("/home/.phoenix/rules/style.md"),
                content: "Use tabs.".into(),
                globs: None,
                source: RuleSource::Global,
            },
            Rule {
                path: PathBuf::from("/project/.phoenix/rules/testing.md"),
                content: "Use vitest.".into(),
                globs: Some(vec!["**/*.test.ts".into()]),
                source: RuleSource::Project,
            },
        ];
        let agents = vec![AgentContext {
            path: PathBuf::from("/project/AGENTS.md"),
            content: "Root instructions.".into(),
            dir: PathBuf::from("/project"),
            is_root: true,
        }];

        let mut touched = HashSet::new();
        touched.insert(PathBuf::from("/project/src/main.ts"));

        let mut state = ContextState::default();
        let result = compute_context(&rules, &agents, &touched, &project, &mut state);

        assert!(result.system_prompt_suffix.contains("Use tabs."));
        assert!(!result.system_prompt_suffix.contains("Use vitest."));
        assert!(result.system_prompt_suffix.contains("Root instructions."));
        assert_eq!(result.newly_loaded.len(), 2); // style.md + AGENTS.md

        // Now touch a test file
        touched.insert(PathBuf::from("/project/src/foo.test.ts"));
        let result2 = compute_context(&rules, &agents, &touched, &project, &mut state);

        assert!(result2.system_prompt_suffix.contains("Use vitest."));
        assert_eq!(result2.newly_loaded.len(), 1); // only testing.md is new
    }

    #[test]
    fn subdirectory_agent_context_scoping() {
        let project = PathBuf::from("/project");
        let agents = vec![
            AgentContext {
                path: PathBuf::from("/project/AGENTS.md"),
                content: "Root.".into(),
                dir: PathBuf::from("/project"),
                is_root: true,
            },
            AgentContext {
                path: PathBuf::from("/project/src/components/AGENTS.md"),
                content: "Components.".into(),
                dir: PathBuf::from("/project/src/components"),
                is_root: false,
            },
        ];

        let mut touched = HashSet::new();
        touched.insert(PathBuf::from("/project/src/main.ts"));

        let mut state = ContextState::default();
        let result = compute_context(&[], &agents, &touched, &project, &mut state);

        assert!(result.system_prompt_suffix.contains("Root."));
        assert!(!result.system_prompt_suffix.contains("Components."));

        touched.insert(PathBuf::from("/project/src/components/Button.tsx"));
        let result2 = compute_context(&[], &agents, &touched, &project, &mut state);

        assert!(result2.system_prompt_suffix.contains("Components."));
        assert_eq!(result2.newly_loaded.len(), 1);
    }

    #[test]
    fn context_state_deduplication() {
        let project = PathBuf::from("/project");
        let rules = vec![Rule {
            path: PathBuf::from("/home/.phoenix/rules/style.md"),
            content: "Use tabs.".into(),
            globs: None,
            source: RuleSource::Global,
        }];

        let mut state = ContextState::default();
        let touched = HashSet::new();

        let r1 = compute_context(&rules, &[], &touched, &project, &mut state);
        assert_eq!(r1.newly_loaded.len(), 1);

        let r2 = compute_context(&rules, &[], &touched, &project, &mut state);
        assert_eq!(r2.newly_loaded.len(), 0);
        assert!(r2.system_prompt_suffix.contains("Use tabs."));
    }

    #[test]
    fn estimate_tokens_basic() {
        let messages = vec![Message::user("Hello, this is a test message.")];
        let tokens = estimate_tokens(&messages, "System prompt here.");
        assert!(tokens > 0);
        assert!(tokens < 100);
    }

    #[test]
    fn should_compact_logic() {
        let limits = ContextLimits {
            context_window: 200_000,
            max_output: 16_384,
            threshold: 0.85,
        };
        let input_budget = 200_000 - 16_384; // 183_616
        let threshold_tokens = (input_budget as f64 * 0.85) as u64; // ~156_073

        assert!(!should_compact(100_000, &limits));
        assert!(should_compact(160_000, &limits));
        assert!(should_compact(threshold_tokens + 1, &limits));
        assert!(!should_compact(threshold_tokens - 1, &limits));
    }

    #[test]
    fn compact_messages_preserves_head_and_tail() {
        // Use a tiny budget so compaction is forced even with short messages
        let limits = ContextLimits {
            context_window: 100,
            max_output: 20,
            threshold: 0.85,
        };

        let mut messages = vec![
            Message::system("System instructions"),
            Message::user("First question"),
            Message::assistant("First answer with some extra padding text to make it longer"),
            Message::user("Second question with more text here"),
            Message::assistant("Second answer with even more text here for tokens"),
            Message::user("Third question with lots of filler"),
            Message::assistant("Third answer with lots of filler text"),
            Message::user("Fourth question"),
            Message::assistant("Fourth answer - latest"),
        ];

        let result = compact_messages(&mut messages, &limits);
        assert!(result.was_compacted);
        assert!(result.removed_count > 0);

        // Head preserved: system + first user
        assert_eq!(messages[0].role, Role::System);
        assert!(messages[0].content.contains("System instructions"));
        assert_eq!(messages[1].role, Role::User);
        assert!(messages[1].content.contains("First question"));

        // Compaction marker present
        let has_marker = messages.iter().any(|m| m.content.contains("compacted"));
        assert!(has_marker);

        // Tail preserved: latest messages
        let last = messages.last().unwrap();
        assert!(last.content.contains("Fourth answer"));
    }

    #[test]
    fn compact_messages_too_few() {
        let limits = ContextLimits {
            context_window: 1000,
            max_output: 200,
            threshold: 0.85,
        };
        let mut messages = vec![Message::user("Hi"), Message::assistant("Hello")];
        let result = compact_messages(&mut messages, &limits);
        assert!(!result.was_compacted);
        assert_eq!(result.remaining_count, 2);
    }

    #[test]
    fn enforce_limits_no_compaction_needed() {
        let limits = ContextLimits {
            context_window: 200_000,
            max_output: 16_384,
            threshold: 0.85,
        };
        let mut messages = vec![Message::user("Short message")];
        let result = enforce_limits(&mut messages, "Short prompt", &limits);
        assert!(!result.was_compacted);
        assert_eq!(result.remaining_count, 1);
    }

    #[test]
    fn skips_hidden_and_node_modules() {
        let project = tempdir().unwrap();
        std::fs::write(project.path().join("AGENTS.md"), "Root.").unwrap();

        let hidden = project.path().join(".hidden");
        std::fs::create_dir_all(&hidden).unwrap();
        std::fs::write(hidden.join("AGENTS.md"), "Hidden.").unwrap();

        let nm = project.path().join("node_modules/pkg");
        std::fs::create_dir_all(&nm).unwrap();
        std::fs::write(nm.join("AGENTS.md"), "NodeModules.").unwrap();

        let agents = discover_agent_contexts(project.path());
        assert_eq!(agents.len(), 1);
        assert!(agents[0].is_root);
    }
}
