use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use parking_lot::RwLock;
use serde_json::{Value, json};

use crate::config::Config;
use crate::session::orchestration::SessionPool;
use crate::store::session_store::SessionStore;

use super::traits::{InputRequester, Tool, ToolError, ToolRegistry, ToolResult, ToolSchema};

// ---------------------------------------------------------------------------
// Shared context for all orchestration tools
// ---------------------------------------------------------------------------

pub struct OrchestrationContext {
    pub pool: Arc<SessionPool>,
    pub config: Arc<RwLock<Config>>,
    pub store: Arc<SessionStore>,
    pub project: PathBuf,
    pub parent_provider: RwLock<String>,
    pub parent_tools: RwLock<ToolRegistry>,
}

pub fn register_orchestration_tools(registry: &mut ToolRegistry, ctx: Arc<OrchestrationContext>) {
    registry.register(Arc::new(SpawnAgentTool {
        ctx: Arc::clone(&ctx),
    }));
    registry.register(Arc::new(CheckAgentsTool {
        ctx: Arc::clone(&ctx),
    }));
    registry.register(Arc::new(CollectAgentTool {
        ctx: Arc::clone(&ctx),
    }));
    registry.register(Arc::new(CancelAgentTool {
        ctx: Arc::clone(&ctx),
    }));
    registry.register(Arc::new(MergeAgentTool {
        ctx: Arc::clone(&ctx),
    }));
}

// ---------------------------------------------------------------------------
// spawn_agent
// ---------------------------------------------------------------------------

pub struct SpawnAgentTool {
    pub ctx: Arc<OrchestrationContext>,
}

#[async_trait]
impl Tool for SpawnAgentTool {
    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "spawn_agent".into(),
            description: "Spawn a child agent on any configured provider. Returns immediately \
                          with a session ID. The child runs asynchronously in its own git worktree."
                .into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "prompt": {
                        "type": "string",
                        "description": "The task to give the child agent."
                    },
                    "provider": {
                        "type": "string",
                        "description": "Name of a configured provider profile. Defaults to the orchestrator's provider."
                    },
                    "model": {
                        "type": "string",
                        "description": "Model override. If omitted, uses the provider profile's default model."
                    },
                    "profile": {
                        "type": "string",
                        "description": "Session profile name (determines tools, system prompt). Defaults to 'default'."
                    },
                    "worktree": {
                        "type": "boolean",
                        "description": "Create an isolated git worktree. Default: true."
                    },
                    "context": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "File paths to pre-load into the child's conversation."
                    }
                },
                "required": ["prompt"]
            }),
        }
    }

    async fn invoke(
        &self,
        args: Value,
        _input: &dyn InputRequester,
    ) -> Result<ToolResult, ToolError> {
        let prompt = args["prompt"]
            .as_str()
            .ok_or_else(|| ToolError::InvalidArgs("missing 'prompt'".into()))?
            .to_string();

        let provider_name = args["provider"].as_str();
        let model_override = args["model"].as_str();
        let profile_name = args["profile"].as_str().unwrap_or("default");
        let use_worktree = args["worktree"].as_bool().unwrap_or(true);
        let context_files: Vec<String> = args["context"]
            .as_array()
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default();

        let config = self.ctx.config.read().clone();
        let parent_provider = self.ctx.parent_provider.read().clone();

        let effective_provider = provider_name
            .or(config.conductor.agent_provider.as_deref())
            .unwrap_or(&parent_provider);
        let effective_model = model_override.or(config.conductor.agent_model.as_deref());

        let (provider, prov_name, model_name) = SessionPool::resolve_provider(
            &config,
            Some(effective_provider),
            effective_model,
            &parent_provider,
        )
        .map_err(ToolError::ExecutionFailed)?;

        let profile = config
            .sessions
            .get(profile_name)
            .cloned()
            .unwrap_or_default();

        let tools = {
            let parent_tools = self.ctx.parent_tools.read();
            std::sync::Arc::new(parking_lot::RwLock::new(parent_tools.clone()))
        };

        let system_prompt_override = None;

        let worktree = if use_worktree {
            if let Some(mgr) = &self.ctx.pool.worktrees {
                let child_id = format!("{}", uuid::Uuid::now_v7().simple());
                mgr.create(&child_id).await.ok()
            } else {
                None
            }
        } else {
            None
        };

        let id = self
            .ctx
            .pool
            .spawn(crate::session::orchestration::SpawnParams {
                provider,
                provider_name: prov_name.clone(),
                model_name: model_name.clone(),
                profile,
                profile_name: profile_name.to_string(),
                prompt,
                tools,
                store: Arc::clone(&self.ctx.store),
                project: self.ctx.project.clone(),
                worktree: worktree.clone(),
                context_files,
                config,
                system_prompt_override,
            })
            .await;

        let task_short: String = args["prompt"]
            .as_str()
            .unwrap_or("")
            .chars()
            .take(60)
            .collect();

        Ok(ToolResult::success(format!(
            "Spawned [{model_name}] {task_short}\nsession_id: {}",
            id.0
        )))
    }
}

// ---------------------------------------------------------------------------
// check_agents
// ---------------------------------------------------------------------------

pub struct CheckAgentsTool {
    pub ctx: Arc<OrchestrationContext>,
}

#[async_trait]
impl Tool for CheckAgentsTool {
    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "check_agents".into(),
            description: "Poll status of child agents. Returns status, active tool, tokens, and elapsed time.".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "ids": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Filter to specific session IDs. Omit for all children."
                    }
                }
            }),
        }
    }

    async fn invoke(
        &self,
        args: Value,
        _input: &dyn InputRequester,
    ) -> Result<ToolResult, ToolError> {
        let ids: Option<Vec<String>> = args["ids"].as_array().map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect()
        });

        let children = match self.ctx.pool.try_check_filtered(ids.as_deref()) {
            Some(c) => c,
            None => self.ctx.pool.check(ids.as_deref()).await,
        };

        if children.is_empty() {
            return Ok(ToolResult::success("No agents."));
        }

        let mut lines = Vec::new();
        for child in &children {
            let status_str = match &child.status {
                crate::session::orchestration::ChildStatus::Queued => "queued",
                crate::session::orchestration::ChildStatus::Running => "working",
                crate::session::orchestration::ChildStatus::Done => "done",
                crate::session::orchestration::ChildStatus::Error(_) => "error",
                crate::session::orchestration::ChildStatus::Cancelled => "cancelled",
            };
            let elapsed = format!("{:.0}s", child.elapsed_s);
            let tool_info = child
                .active_tool
                .as_deref()
                .map(|t| format!(" ({t})"))
                .unwrap_or_default();
            lines.push(format!(
                "{}\t{}\t{}\t{}",
                child.task, status_str, elapsed, tool_info
            ));
        }

        Ok(ToolResult::success(lines.join("\n")))
    }
}

// ---------------------------------------------------------------------------
// collect_agent
// ---------------------------------------------------------------------------

pub struct CollectAgentTool {
    pub ctx: Arc<OrchestrationContext>,
}

#[async_trait]
impl Tool for CollectAgentTool {
    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "collect_agent".into(),
            description: "Retrieve the final output from a completed child agent. Includes diff \
                          summary when the child ran in a worktree."
                .into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "session_id": {
                        "type": "string",
                        "description": "ID of the child session to collect."
                    }
                },
                "required": ["session_id"]
            }),
        }
    }

    async fn invoke(
        &self,
        args: Value,
        _input: &dyn InputRequester,
    ) -> Result<ToolResult, ToolError> {
        let session_id = args["session_id"]
            .as_str()
            .ok_or_else(|| ToolError::InvalidArgs("missing 'session_id'".into()))?;

        let info = self
            .ctx
            .pool
            .collect(session_id)
            .await
            .map_err(ToolError::ExecutionFailed)?;

        let status_str = match &info.status {
            crate::session::orchestration::ChildStatus::Done => "done",
            crate::session::orchestration::ChildStatus::Error(e) => e.as_str(),
            crate::session::orchestration::ChildStatus::Cancelled => "cancelled",
            crate::session::orchestration::ChildStatus::Running => "running",
            crate::session::orchestration::ChildStatus::Queued => "queued",
        };

        let mut lines = vec![
            format!("Task:     {}", info.task),
            format!("Status:   {status_str}"),
            format!("Model:    {}/{}", info.provider, info.model),
            format!("Elapsed:  {:.1}s", info.elapsed_s),
        ];

        if let (Some(branch), Some(mgr)) = (&info.worktree_branch, &self.ctx.pool.worktrees) {
            let child_id = branch.strip_prefix("phx/agent/").unwrap_or(session_id);
            if let Ok(diff) = mgr.diff_summary(child_id, "HEAD").await {
                lines.push(String::new());
                lines.push(format!(
                    "Changes:  {} files (+{} −{})",
                    diff.files_changed, diff.insertions, diff.deletions
                ));
                lines.push(format!("Branch:   {branch}"));
                if !diff.summary.is_empty() {
                    lines.push(String::new());
                    lines.push(diff.summary);
                }
            }
        }

        if let Some(output) = &info.output
            && !output.is_empty()
        {
            lines.push(String::new());
            lines.push("Output:".to_string());
            lines.push(output.clone());
        }

        Ok(ToolResult::success(lines.join("\n")))
    }
}

// ---------------------------------------------------------------------------
// cancel_agent
// ---------------------------------------------------------------------------

pub struct CancelAgentTool {
    pub ctx: Arc<OrchestrationContext>,
}

#[async_trait]
impl Tool for CancelAgentTool {
    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "cancel_agent".into(),
            description: "Cancel a running or queued child agent. Cleans up its worktree.".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "session_id": {
                        "type": "string",
                        "description": "ID of the child session to cancel."
                    },
                    "reason": {
                        "type": "string",
                        "description": "Reason for cancellation."
                    }
                },
                "required": ["session_id"]
            }),
        }
    }

    async fn invoke(
        &self,
        args: Value,
        _input: &dyn InputRequester,
    ) -> Result<ToolResult, ToolError> {
        let session_id = args["session_id"]
            .as_str()
            .ok_or_else(|| ToolError::InvalidArgs("missing 'session_id'".into()))?;

        self.ctx
            .pool
            .cancel(session_id)
            .await
            .map_err(ToolError::ExecutionFailed)?;

        let reason = args["reason"]
            .as_str()
            .unwrap_or("cancelled by orchestrator");
        Ok(ToolResult::success(format!("Cancelled: {reason}")))
    }
}

// ---------------------------------------------------------------------------
// merge_agent
// ---------------------------------------------------------------------------

pub struct MergeAgentTool {
    pub ctx: Arc<OrchestrationContext>,
}

#[async_trait]
impl Tool for MergeAgentTool {
    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "merge_agent".into(),
            description: "Merge a completed child's worktree branch back into the parent branch."
                .into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "session_id": {
                        "type": "string",
                        "description": "ID of the completed child session to merge."
                    },
                    "strategy": {
                        "type": "string",
                        "enum": ["squash", "rebase", "merge"],
                        "description": "How to integrate changes. Default: squash."
                    },
                    "message": {
                        "type": "string",
                        "description": "Commit message. If omitted, auto-generated."
                    },
                    "cleanup": {
                        "type": "boolean",
                        "description": "Remove worktree and branch after merging. Default: true."
                    }
                },
                "required": ["session_id"]
            }),
        }
    }

    async fn invoke(
        &self,
        args: Value,
        _input: &dyn InputRequester,
    ) -> Result<ToolResult, ToolError> {
        let session_id = args["session_id"]
            .as_str()
            .ok_or_else(|| ToolError::InvalidArgs("missing 'session_id'".into()))?;

        let strategy = match args["strategy"].as_str().unwrap_or("squash") {
            "rebase" => crate::worktree::MergeStrategy::Rebase,
            "merge" => crate::worktree::MergeStrategy::Merge,
            _ => crate::worktree::MergeStrategy::Squash,
        };
        let message = args["message"].as_str();
        let cleanup = args["cleanup"].as_bool().unwrap_or(true);

        let info = self
            .ctx
            .pool
            .collect(session_id)
            .await
            .map_err(ToolError::ExecutionFailed)?;

        let branch = info
            .worktree_branch
            .as_deref()
            .ok_or_else(|| ToolError::ExecutionFailed("child has no worktree".into()))?;

        let child_id = branch.strip_prefix("phx/agent/").unwrap_or(session_id);

        let mgr =
            self.ctx.pool.worktrees.as_ref().ok_or_else(|| {
                ToolError::ExecutionFailed("worktree manager not available".into())
            })?;

        // Auto-commit any remaining changes
        let _ = mgr
            .auto_commit(child_id, &format!("phx: agent {child_id} — final"))
            .await;

        let merge_result = mgr
            .merge(child_id, strategy, message, cleanup)
            .await
            .map_err(|e| ToolError::ExecutionFailed(e.to_string()))?;

        let strategy_name = args["strategy"].as_str().unwrap_or("squash");
        let mut lines = vec![
            format!("Merged ({strategy_name})"),
            format!("Files:    {}", merge_result.files_changed),
        ];
        if !merge_result.commit.is_empty() {
            lines.push(format!("Commit:   {}", merge_result.commit));
        }
        if !merge_result.conflicts.is_empty() {
            lines.push(format!("Conflicts: {}", merge_result.conflicts.join(", ")));
        }
        if cleanup {
            lines.push("Worktree: cleaned up".to_string());
        }

        Ok(ToolResult::success(lines.join("\n")))
    }
}
