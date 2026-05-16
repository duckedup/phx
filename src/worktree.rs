use std::path::{Path, PathBuf};

#[derive(Debug, thiserror::Error)]
pub enum WorktreeError {
    #[error("not a git repository: {0}")]
    NotGitRepo(PathBuf),
    #[error("git error: {0}")]
    Git(String),
    #[error("worktree not found: {0}")]
    NotFound(String),
    #[error("merge conflict in {files:?}")]
    MergeConflict { files: Vec<String> },
}

#[derive(Debug, Clone)]
pub struct WorktreeInfo {
    pub path: PathBuf,
    pub branch: String,
    pub child_id: String,
}

#[derive(Debug, Clone, Copy)]
pub enum MergeStrategy {
    Squash,
    Rebase,
    Merge,
}

#[derive(Debug, Clone)]
pub struct DiffSummary {
    pub files_changed: usize,
    pub insertions: usize,
    pub deletions: usize,
    pub summary: String,
}

#[derive(Debug, Clone)]
pub struct MergeResult {
    pub commit: String,
    pub files_changed: usize,
    pub conflicts: Vec<String>,
}

#[derive(Clone)]
pub struct WorktreeManager {
    repo_root: PathBuf,
    worktree_base: PathBuf,
}

impl WorktreeManager {
    pub fn new(repo_root: PathBuf) -> Result<Self, WorktreeError> {
        let out = std::process::Command::new("git")
            .args(["rev-parse", "--git-dir"])
            .current_dir(&repo_root)
            .output()
            .map_err(|e| WorktreeError::Git(format!("failed to run git: {e}")))?;

        if !out.status.success() {
            return Err(WorktreeError::NotGitRepo(repo_root));
        }

        let project_name = repo_root
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("project");

        let worktree_base = repo_root
            .parent()
            .unwrap_or(Path::new("/tmp"))
            .join(format!(".phx-worktrees/{project_name}"));

        Ok(Self {
            repo_root,
            worktree_base,
        })
    }

    async fn git(&self, args: &[&str]) -> Result<String, WorktreeError> {
        let out = tokio::process::Command::new("git")
            .args(args)
            .current_dir(&self.repo_root)
            .output()
            .await
            .map_err(|e| WorktreeError::Git(format!("failed to run git: {e}")))?;

        if out.status.success() {
            Ok(String::from_utf8_lossy(&out.stdout).to_string())
        } else {
            Err(WorktreeError::Git(
                String::from_utf8_lossy(&out.stderr).trim().to_string(),
            ))
        }
    }

    async fn git_at(path: &Path, args: &[&str]) -> Result<String, WorktreeError> {
        let out = tokio::process::Command::new("git")
            .args(args)
            .current_dir(path)
            .output()
            .await
            .map_err(|e| WorktreeError::Git(format!("failed to run git: {e}")))?;

        if out.status.success() {
            Ok(String::from_utf8_lossy(&out.stdout).to_string())
        } else {
            Err(WorktreeError::Git(
                String::from_utf8_lossy(&out.stderr).trim().to_string(),
            ))
        }
    }

    fn branch_name(child_id: &str) -> String {
        format!("phx/agent/{child_id}")
    }

    fn worktree_path(&self, child_id: &str) -> PathBuf {
        self.worktree_base.join(child_id)
    }

    pub async fn create(&self, child_id: &str) -> Result<WorktreeInfo, WorktreeError> {
        let branch = Self::branch_name(child_id);
        let path = self.worktree_path(child_id);

        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|e| WorktreeError::Git(format!("failed to create directory: {e}")))?;
        }

        let path_str = path.to_string_lossy();
        self.git(&["worktree", "add", "-b", &branch, &path_str, "HEAD"])
            .await?;

        Ok(WorktreeInfo {
            path,
            branch,
            child_id: child_id.to_string(),
        })
    }

    pub async fn remove(&self, child_id: &str, delete_branch: bool) -> Result<(), WorktreeError> {
        let path = self.worktree_path(child_id);
        let branch = Self::branch_name(child_id);

        if path.exists() {
            let path_str = path.to_string_lossy();
            let _ = self
                .git(&["worktree", "remove", "--force", &path_str])
                .await;
        }

        if delete_branch {
            let _ = self.git(&["branch", "-D", &branch]).await;
        }

        Ok(())
    }

    pub async fn auto_commit(&self, child_id: &str, message: &str) -> Result<bool, WorktreeError> {
        let path = self.worktree_path(child_id);
        if !path.exists() {
            return Err(WorktreeError::NotFound(child_id.to_string()));
        }

        let status = Self::git_at(&path, &["status", "--porcelain"]).await?;
        if status.trim().is_empty() {
            return Ok(false);
        }

        Self::git_at(&path, &["add", "-A"]).await?;
        Self::git_at(&path, &["commit", "-m", message]).await?;

        Ok(true)
    }

    pub async fn diff_summary(
        &self,
        child_id: &str,
        base_branch: &str,
    ) -> Result<DiffSummary, WorktreeError> {
        let branch = Self::branch_name(child_id);
        let range = format!("{base_branch}..{branch}");

        let path = self.worktree_path(child_id);
        let run_dir = if path.exists() {
            &path
        } else {
            &self.repo_root
        };

        let numstat = Self::git_at(run_dir, &["diff", "--numstat", &range])
            .await
            .unwrap_or_default();

        let mut files_changed = 0;
        let mut insertions = 0;
        let mut deletions = 0;
        for line in numstat.lines() {
            let parts: Vec<&str> = line.split('\t').collect();
            if parts.len() >= 2 {
                files_changed += 1;
                insertions += parts[0].parse::<usize>().unwrap_or(0);
                deletions += parts[1].parse::<usize>().unwrap_or(0);
            }
        }

        let summary = Self::git_at(run_dir, &["diff", "--stat", &range])
            .await
            .unwrap_or_default();

        Ok(DiffSummary {
            files_changed,
            insertions,
            deletions,
            summary,
        })
    }

    pub async fn merge(
        &self,
        child_id: &str,
        strategy: MergeStrategy,
        message: Option<&str>,
        cleanup: bool,
    ) -> Result<MergeResult, WorktreeError> {
        let branch = Self::branch_name(child_id);
        let auto_msg = format!("phx: merge agent {child_id}");
        let msg = message.unwrap_or(&auto_msg);

        let result = match strategy {
            MergeStrategy::Squash => match self.git(&["merge", "--squash", &branch]).await {
                Ok(_) => self.git(&["commit", "-m", msg]).await,
                Err(e) => Err(e),
            },
            MergeStrategy::Rebase => self.git(&["rebase", &branch]).await,
            MergeStrategy::Merge => self.git(&["merge", "--no-ff", "-m", msg, &branch]).await,
        };

        match result {
            Ok(_) => {
                let commit = self
                    .git(&["rev-parse", "--short", "HEAD"])
                    .await
                    .unwrap_or_default()
                    .trim()
                    .to_string();

                let diff = self
                    .git(&["diff", "--stat", "HEAD~1..HEAD"])
                    .await
                    .unwrap_or_default();
                let files_changed = diff.lines().count().saturating_sub(1);

                if cleanup {
                    let _ = self.remove(child_id, true).await;
                }

                Ok(MergeResult {
                    commit,
                    files_changed,
                    conflicts: vec![],
                })
            }
            Err(_) => {
                let conflict_output = self
                    .git(&["diff", "--name-only", "--diff-filter=U"])
                    .await
                    .unwrap_or_default();
                let conflicts: Vec<String> = conflict_output
                    .lines()
                    .filter(|l| !l.is_empty())
                    .map(|l| l.to_string())
                    .collect();

                if !conflicts.is_empty() {
                    let _ = self.git(&["merge", "--abort"]).await;
                    return Err(WorktreeError::MergeConflict { files: conflicts });
                }

                let _ = self.git(&["merge", "--abort"]).await;
                Err(WorktreeError::Git("merge failed".into()))
            }
        }
    }

    pub async fn list(&self) -> Result<Vec<WorktreeInfo>, WorktreeError> {
        let output = self.git(&["worktree", "list", "--porcelain"]).await?;
        let mut result = Vec::new();

        let mut current_path: Option<PathBuf> = None;
        let mut current_branch: Option<String> = None;

        for line in output.lines() {
            if let Some(path_str) = line.strip_prefix("worktree ") {
                current_path = Some(PathBuf::from(path_str));
            } else if let Some(branch_ref) = line.strip_prefix("branch refs/heads/") {
                current_branch = Some(branch_ref.to_string());
            } else if line.is_empty() {
                if let (Some(path), Some(branch)) = (current_path.take(), current_branch.take())
                    && let Some(child_id) = branch.strip_prefix("phx/agent/")
                {
                    result.push(WorktreeInfo {
                        path,
                        branch: branch.clone(),
                        child_id: child_id.to_string(),
                    });
                }
                current_path = None;
                current_branch = None;
            }
        }

        if let (Some(path), Some(branch)) = (current_path, current_branch)
            && let Some(child_id) = branch.strip_prefix("phx/agent/")
        {
            result.push(WorktreeInfo {
                path,
                branch: branch.clone(),
                child_id: child_id.to_string(),
            });
        }

        Ok(result)
    }

    pub async fn cleanup_all(&self) -> Result<usize, WorktreeError> {
        let active = self.list().await?;
        let count = active.len();
        for wt in &active {
            let _ = self.remove(&wt.child_id, true).await;
        }
        Ok(count)
    }
}
