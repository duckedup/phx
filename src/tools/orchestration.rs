use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use parking_lot::RwLock;
use serde_json::{Value, json};

use crate::config::schema::Config;
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
            .spawn(crate::session::orchestration::SpawnConfig {
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

        let agent_label = "default";
        let wt_info = worktree
            .as_ref()
            .map(|w| format!(" on branch {}", w.branch))
            .unwrap_or_default();

        let output = format!(
            "◆ Agent spawned\n\
             \n\
               id       {}\n\
               agent    {}\n\
               model    {}/{}\n\
               status   queued{}\n",
            id.0, agent_label, prov_name, model_name, wt_info
        );

        Ok(ToolResult::success(output))
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

        let output = format!("check_agents\n{}", lines.join("\n"));
        Ok(ToolResult::success(output))
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

        let mut result = serde_json::to_value(&info).unwrap_or_default();

        if let (Some(branch), Some(mgr)) = (&info.worktree_branch, &self.ctx.pool.worktrees) {
            let child_id = branch.strip_prefix("phx/agent/").unwrap_or(session_id);
            if let Ok(diff) = mgr.diff_summary(child_id, "HEAD").await {
                result["worktree_diff"] = json!({
                    "branch": branch,
                    "files_changed": diff.files_changed,
                    "insertions": diff.insertions,
                    "deletions": diff.deletions,
                    "diff_summary": diff.summary,
                });
            }
        }

        Ok(ToolResult::success(result.to_string()))
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
        Ok(ToolResult::success(
            json!({
                "session_id": session_id,
                "cancelled": true,
                "reason": reason,
            })
            .to_string(),
        ))
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

        Ok(ToolResult::success(
            json!({
                "session_id": session_id,
                "merged": true,
                "strategy": args["strategy"].as_str().unwrap_or("squash"),
                "commit": merge_result.commit,
                "files_changed": merge_result.files_changed,
                "conflicts": merge_result.conflicts,
                "worktree_removed": cleanup,
            })
            .to_string(),
        ))
    }
}
