use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{Value, json};

use crate::config::schema::Config;
use crate::session::orchestration::SessionPool;
use crate::store::session_store::SessionStore;

use super::traits::{Tool, ToolError, ToolResult, ToolSchema};

// ---------------------------------------------------------------------------
// spawn_agent
// ---------------------------------------------------------------------------

pub struct SpawnAgentTool {
    pub pool: Arc<SessionPool>,
    pub config: Arc<Config>,
    pub store: Arc<SessionStore>,
    pub project: PathBuf,
    pub parent_provider: String,
}

#[async_trait]
impl Tool for SpawnAgentTool {
    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "spawn_agent",
            description: "Spawn a child agent on any configured provider. Returns immediately \
                          with a session ID. The child runs asynchronously in its own git worktree.",
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

    async fn invoke(&self, args: Value) -> Result<ToolResult, ToolError> {
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

        let (provider, prov_name, model_name) = SessionPool::resolve_provider(
            &self.config,
            provider_name,
            model_override,
            &self.parent_provider,
        )
        .map_err(ToolError::ExecutionFailed)?;

        let profile = self
            .config
            .sessions
            .get(profile_name)
            .cloned()
            .unwrap_or_default();

        let tools = crate::tools::build_registry(
            &profile.tools.iter().map(|s| s.as_str()).collect::<Vec<_>>(),
        );

        let worktree = if use_worktree {
            self.pool.worktrees.as_ref().and_then(|mgr| {
                let child_id = format!("{}", uuid::Uuid::now_v7().simple());
                mgr.create(&child_id).ok()
            })
        } else {
            None
        };

        let id = self
            .pool
            .spawn(crate::session::orchestration::SpawnConfig {
                provider,
                provider_name: prov_name.clone(),
                model_name: model_name.clone(),
                profile,
                profile_name: profile_name.to_string(),
                prompt,
                tools,
                store: Arc::clone(&self.store),
                project: self.project.clone(),
                worktree: worktree.clone(),
                context_files,
            })
            .await;

        let result = json!({
            "session_id": id.0,
            "provider": prov_name,
            "model": model_name,
            "status": "queued",
            "worktree": worktree.as_ref().map(|w| w.path.to_string_lossy().to_string()),
            "branch": worktree.as_ref().map(|w| &w.branch),
        });

        Ok(ToolResult::success(result.to_string()))
    }
}

// ---------------------------------------------------------------------------
// check_agents
// ---------------------------------------------------------------------------

pub struct CheckAgentsTool {
    pub pool: Arc<SessionPool>,
}

#[async_trait]
impl Tool for CheckAgentsTool {
    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "check_agents",
            description: "Poll status of child agents. Returns status, active tool, tokens, and elapsed time.",
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

    async fn invoke(&self, args: Value) -> Result<ToolResult, ToolError> {
        let ids: Option<Vec<String>> = args["ids"].as_array().map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect()
        });

        let children = self.pool.check(ids.as_deref()).await;

        let total = children.len();
        let running = children
            .iter()
            .filter(|c| c.status == crate::session::orchestration::ChildStatus::Running)
            .count();
        let done = children
            .iter()
            .filter(|c| c.status == crate::session::orchestration::ChildStatus::Done)
            .count();
        let queued = children
            .iter()
            .filter(|c| c.status == crate::session::orchestration::ChildStatus::Queued)
            .count();
        let error = children
            .iter()
            .filter(|c| {
                matches!(
                    c.status,
                    crate::session::orchestration::ChildStatus::Error(_)
                )
            })
            .count();

        let result = json!({
            "children": children,
            "summary": {
                "total": total,
                "running": running,
                "done": done,
                "queued": queued,
                "error": error,
            }
        });

        Ok(ToolResult::success(result.to_string()))
    }
}

// ---------------------------------------------------------------------------
// collect_agent
// ---------------------------------------------------------------------------

pub struct CollectAgentTool {
    pub pool: Arc<SessionPool>,
}

#[async_trait]
impl Tool for CollectAgentTool {
    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "collect_agent",
            description: "Retrieve the final output from a completed child agent. Includes diff \
                          summary when the child ran in a worktree.",
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

    async fn invoke(&self, args: Value) -> Result<ToolResult, ToolError> {
        let session_id = args["session_id"]
            .as_str()
            .ok_or_else(|| ToolError::InvalidArgs("missing 'session_id'".into()))?;

        let info = self
            .pool
            .collect(session_id)
            .await
            .map_err(ToolError::ExecutionFailed)?;

        let mut result = serde_json::to_value(&info).unwrap_or_default();

        if let (Some(branch), Some(mgr)) = (&info.worktree_branch, &self.pool.worktrees) {
            let child_id = branch.strip_prefix("phoenix/agent/").unwrap_or(session_id);
            if let Ok(diff) = mgr.diff_summary(child_id, "HEAD") {
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
    pub pool: Arc<SessionPool>,
}

#[async_trait]
impl Tool for CancelAgentTool {
    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "cancel_agent",
            description: "Cancel a running or queued child agent. Cleans up its worktree.",
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

    async fn invoke(&self, args: Value) -> Result<ToolResult, ToolError> {
        let session_id = args["session_id"]
            .as_str()
            .ok_or_else(|| ToolError::InvalidArgs("missing 'session_id'".into()))?;

        self.pool
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
    pub pool: Arc<SessionPool>,
}

#[async_trait]
impl Tool for MergeAgentTool {
    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "merge_agent",
            description: "Merge a completed child's worktree branch back into the parent branch.",
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

    async fn invoke(&self, args: Value) -> Result<ToolResult, ToolError> {
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
            .pool
            .collect(session_id)
            .await
            .map_err(ToolError::ExecutionFailed)?;

        let branch = info
            .worktree_branch
            .as_deref()
            .ok_or_else(|| ToolError::ExecutionFailed("child has no worktree".into()))?;

        let child_id = branch.strip_prefix("phoenix/agent/").unwrap_or(session_id);

        let mgr =
            self.pool.worktrees.as_ref().ok_or_else(|| {
                ToolError::ExecutionFailed("worktree manager not available".into())
            })?;

        // Auto-commit any remaining changes
        let _ = mgr.auto_commit(child_id, &format!("phoenix: agent {child_id} — final"));

        let merge_result = mgr
            .merge(child_id, strategy, message, cleanup)
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
