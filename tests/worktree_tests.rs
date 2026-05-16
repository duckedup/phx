#![cfg(not(miri))]

use phx::worktree::{MergeStrategy, WorktreeManager};
use tempfile::TempDir;

fn init_test_repo() -> TempDir {
    let dir = TempDir::new().unwrap();
    let path = dir.path();
    let out = std::process::Command::new("git")
        .args(["init", "-b", "main"])
        .current_dir(path)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git init failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    std::process::Command::new("git")
        .args(["config", "user.email", "test@test.com"])
        .current_dir(path)
        .output()
        .unwrap();
    std::process::Command::new("git")
        .args(["config", "user.name", "Test"])
        .current_dir(path)
        .output()
        .unwrap();
    std::fs::write(path.join("README.md"), "# test").unwrap();
    std::process::Command::new("git")
        .args(["add", "."])
        .current_dir(path)
        .output()
        .unwrap();
    let out = std::process::Command::new("git")
        .args(["commit", "-m", "initial"])
        .current_dir(path)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git commit failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    dir
}

#[tokio::test]
async fn create_and_list_worktree() {
    let dir = init_test_repo();
    let mgr = WorktreeManager::new(dir.path().to_path_buf()).unwrap();

    let info = mgr.create("c-01").await.unwrap();
    assert!(info.path.exists());
    assert_eq!(info.branch, "phx/agent/c-01");
    assert_eq!(info.child_id, "c-01");

    let active = mgr.list().await.unwrap();
    assert_eq!(active.len(), 1);
    assert_eq!(active[0].child_id, "c-01");

    mgr.remove("c-01", true).await.unwrap();
    assert!(!info.path.exists());
    let active = mgr.list().await.unwrap();
    assert_eq!(active.len(), 0);
}

#[tokio::test]
async fn auto_commit_dirty_worktree() {
    let dir = init_test_repo();
    let mgr = WorktreeManager::new(dir.path().to_path_buf()).unwrap();

    let info = mgr.create("c-02").await.unwrap();

    let committed = mgr.auto_commit("c-02", "no-op").await.unwrap();
    assert!(!committed);

    std::fs::write(info.path.join("new_file.txt"), "hello").unwrap();
    let committed = mgr.auto_commit("c-02", "added file").await.unwrap();
    assert!(committed);

    mgr.remove("c-02", true).await.unwrap();
}

#[tokio::test]
async fn diff_summary_shows_changes() {
    let dir = init_test_repo();
    let mgr = WorktreeManager::new(dir.path().to_path_buf()).unwrap();

    let info = mgr.create("c-03").await.unwrap();
    std::fs::write(info.path.join("feature.rs"), "fn main() {}").unwrap();
    mgr.auto_commit("c-03", "add feature").await.unwrap();

    let diff = mgr.diff_summary("c-03", "main").await.unwrap();
    assert!(
        diff.files_changed >= 1,
        "expected at least 1 file changed, got {}. numstat may have failed silently.",
        diff.files_changed
    );
    assert!(
        diff.insertions > 0,
        "expected insertions > 0, got {}",
        diff.insertions
    );

    mgr.remove("c-03", true).await.unwrap();
}

#[tokio::test]
async fn merge_squash_and_cleanup() {
    let dir = init_test_repo();
    let mgr = WorktreeManager::new(dir.path().to_path_buf()).unwrap();

    let info = mgr.create("c-04").await.unwrap();
    std::fs::write(info.path.join("merged.txt"), "content").unwrap();
    mgr.auto_commit("c-04", "work").await.unwrap();

    let result = mgr
        .merge("c-04", MergeStrategy::Squash, Some("squash merge"), true)
        .await
        .unwrap();
    assert!(!result.commit.is_empty());
    assert!(result.conflicts.is_empty());
    assert!(!info.path.exists());
}

#[tokio::test]
async fn cleanup_all_removes_everything() {
    let dir = init_test_repo();
    let mgr = WorktreeManager::new(dir.path().to_path_buf()).unwrap();

    mgr.create("c-10").await.unwrap();
    mgr.create("c-11").await.unwrap();
    mgr.create("c-12").await.unwrap();

    let removed = mgr.cleanup_all().await.unwrap();
    assert_eq!(removed, 3);
    assert_eq!(mgr.list().await.unwrap().len(), 0);
}

#[test]
fn not_a_git_repo_fails() {
    let dir = TempDir::new().unwrap();
    let result = WorktreeManager::new(dir.path().to_path_buf());
    assert!(result.is_err());
}
