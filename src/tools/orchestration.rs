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
    pub agents: RwLock<Vec<crate::session::agents::AgentDefinition>>,
    pub plan_approved: std::sync::atomic::AtomicBool,
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
    registry.register(Arc::new(MergeAgentTool { ctx }));
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
                    },
                    "agent": {
                        "type": "string",
                        "description": "Name of a custom agent definition to use. Overrides system prompt, tools, and optionally provider/model with the agent's configuration."
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
        if !self
            .ctx
            .plan_approved
            .load(std::sync::atomic::Ordering::Relaxed)
        {
            return Ok(ToolResult::error(
                "Plan not approved. Present your plan to the user first, \
                 then wait for them to say \"go\" or \"approved\" before spawning agents.",
            ));
        }

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
        let agent_name = args["agent"].as_str();

        let agent_def = if let Some(name) = agent_name {
            let agents = self.ctx.agents.read();
            let def = crate::session::agents::find_agent(&agents, name)
                .ok_or_else(|| ToolError::InvalidArgs(format!("unknown custom agent: {name}")))?
                .clone();
            Some(def)
        } else {
            None
        };

        let config = self.ctx.config.read().clone();
        let parent_provider = self.ctx.parent_provider.read().clone();

        let effective_provider = provider_name
            .or(agent_def.as_ref().and_then(|d| d.provider.as_deref()))
            .or(config.conductor.agent_provider.as_deref())
            .unwrap_or(&parent_provider);
        let effective_model = model_override
            .or(agent_def.as_ref().and_then(|d| d.model.as_deref()))
            .or(config.conductor.agent_model.as_deref());

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
            let reg = if let Some(ref def) = agent_def {
                if def.tools.is_empty() {
                    parent_tools.clone()
                } else {
                    let mut reg = ToolRegistry::new();
                    for tool_name in &def.tools {
                        if let Some(tool) = parent_tools.get(tool_name) {
                            reg.register(tool);
                        } else if let Some(tool) = super::lookup(tool_name) {
                            reg.register(tool);
                        } else {
                            tracing::warn!(
                                "custom agent '{}': unknown tool '{}'",
                                def.name,
                                tool_name
                            );
                        }
                    }
                    reg
                }
            } else {
                parent_tools.clone()
            };
            std::sync::Arc::new(parking_lot::RwLock::new(reg))
        };

        let system_prompt_override = agent_def.as_ref().map(|d| d.system_prompt.clone());

        let worktree = if use_worktree {
            self.ctx.pool.worktrees.as_ref().and_then(|mgr| {
                let child_id = format!("{}", uuid::Uuid::now_v7().simple());
                mgr.create(&child_id).ok()
            })
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

        let agent_label = agent_name.unwrap_or("default");
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

        let total = children.len();
        let mut running = 0usize;
        let mut done = 0usize;

        for child in &children {
            match &child.status {
                crate::session::orchestration::ChildStatus::Running => running += 1,
                crate::session::orchestration::ChildStatus::Done => done += 1,
                _ => {}
            }
        }

        Ok(ToolResult::success(format!(
            "{total} agents: {running} running, {done} done"
        )))
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
        let _ = mgr.auto_commit(child_id, &format!("phx: agent {child_id} — final"));

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
