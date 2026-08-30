use std::path::{Path, PathBuf};
use serde::{Deserialize, Serialize};
use crate::error::AcpError;

/// A created worktree: path on disk + branch name.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Worktree {
    pub path: PathBuf,
    pub branch: String,
}

/// Result of merging an agent branch back into main.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MergeResult {
    pub success: bool,
    pub conflicts: Vec<String>,
    pub repo_blocked: bool,
}

/// Manages git worktrees under `.tasker-worktrees/` for agent runs.
/// Constructed per-run from the card's `repo_path`, not ambient cwd.
pub struct WorktreeManager {
    repo_root: PathBuf,
    wt_root: PathBuf,
}

impl WorktreeManager {
    pub fn new(repo_root: &Path) -> Self {
        let wt_root = repo_root.join(".tasker-worktrees");
        Self { repo_root: repo_root.to_path_buf(), wt_root }
    }

    /// Idempotently append `.tasker-worktrees/` to `.git/info/exclude`.
    fn ensure_excluded(&self) -> Result<(), AcpError> {
        let exclude_path = self.repo_root.join(".git").join("info").join("exclude");
        if !exclude_path.exists() {
            return Ok(());
        }
        let content = std::fs::read_to_string(&exclude_path)
            .map_err(|e| AcpError::internal(format!("read info/exclude: {e}")))?;
        if content.lines().any(|l| l.trim() == ".tasker-worktrees/") {
            return Ok(());
        }
        let prefix = if content.is_empty() || content.ends_with('\n') { "" } else { "\n" };
        std::fs::write(&exclude_path, format!("{content}{prefix}.tasker-worktrees/\n"))
            .map_err(|e| AcpError::internal(format!("write info/exclude: {e}")))?;
        Ok(())
    }

    /// Detect an in-progress merge in the repo root.
    fn check_merge_in_progress(&self) -> Result<(), AcpError> {
        let merge_head = self.repo_root.join(".git").join("MERGE_HEAD");
        if merge_head.exists() {
            return Err(AcpError::conflict(
                "A merge is already in progress in the main worktree. Resolve it before starting an agent run. Remedy: acp_resolve_merge",
            ));
        }
        Ok(())
    }

    /// Sanitize a card id into a valid git refname component.
    fn sanitize_branch(card_id: &str) -> String {
        let cleaned: String = card_id
            .chars()
            .map(|c| if c.is_alphanumeric() || c == '-' || c == '_' { c } else { '-' })
            .collect();
        format!("agent/{cleaned}")
    }

    /// Create a worktree for `card_id` on branch `agent/<card_id>`.
    pub async fn create(&self, card_id: &str) -> Result<Worktree, AcpError> {
        self.ensure_excluded()?;
        self.check_merge_in_progress()?;
        let wt_path = self.wt_root.join(card_id);
        if wt_path.exists() {
            return Err(AcpError::conflict(format!(
                "Worktree directory already exists: {}. Remove it first. Remedy: acp_remove_worktree",
                wt_path.display()
            )));
        }
        let branch = Self::sanitize_branch(card_id);
        let output = tokio::process::Command::new("git")
            .arg("worktree").arg("add").arg("-b")
            .arg(&branch)
            .arg(&wt_path)
            .arg("HEAD")
            .current_dir(&self.repo_root)
            .output()
            .await
            .map_err(|e| AcpError::internal(format!("git worktree add: {e}")))?;
        if !output.status.success() {
            return Err(AcpError::internal(format!(
                "git worktree add failed: {}",
                String::from_utf8_lossy(&output.stderr)
            )));
        }
        Ok(Worktree { path: wt_path, branch })
    }

    /// Remove a worktree by card_id. Safe if already gone.
    /// Also deletes the agent branch.
    pub async fn remove(&self, card_id: &str) -> Result<(), AcpError> {
        let wt_path = self.wt_root.join(card_id);
        if wt_path.exists() {
            let _ = tokio::process::Command::new("git")
                .arg("worktree").arg("remove").arg("--force")
                .arg(&wt_path)
                .current_dir(&self.repo_root)
                .output()
                .await;
            let _ = tokio::process::Command::new("git")
                .arg("worktree").arg("prune")
                .current_dir(&self.repo_root)
                .output()
                .await;
        }
        // Delete the branch (safe if already gone).
        let branch = Self::sanitize_branch(card_id);
        let _ = tokio::process::Command::new("git")
            .arg("branch").arg("-D").arg(&branch)
            .current_dir(&self.repo_root)
            .output()
            .await;
        Ok(())
    }

    /// List all worktrees.
    pub async fn list(&self) -> Result<Vec<Worktree>, AcpError> {
        let output = tokio::process::Command::new("git")
            .arg("worktree").arg("list").arg("--porcelain")
            .current_dir(&self.repo_root)
            .output()
            .await
            .map_err(|e| AcpError::internal(format!("git worktree list: {e}")))?;
        if !output.status.success() {
            return Err(AcpError::internal(format!(
                "git worktree list failed: {}", String::from_utf8_lossy(&output.stderr)
            )));
        }
        let text = String::from_utf8_lossy(&output.stdout);
        let mut worktrees = Vec::new();
        let mut current_path: Option<PathBuf> = None;
        let mut current_branch: Option<String> = None;
        for line in text.lines() {
            if line.is_empty() {
                if let (Some(p), Some(b)) = (current_path.take(), current_branch.take()) {
                    worktrees.push(Worktree { path: p, branch: b });
                }
                continue;
            }
            if let Some(path) = line.strip_prefix("worktree ") {
                current_path = Some(PathBuf::from(path));
            } else if let Some(branch) = line.strip_prefix("branch ") {
                current_branch = Some(branch.trim_start_matches("refs/heads/").to_string());
            }
        }
        if let (Some(p), Some(b)) = (current_path, current_branch) {
            worktrees.push(Worktree { path: p, branch: b });
        }
        Ok(worktrees)
    }

    /// Diff between main and the agent branch for `card_id`.
    pub async fn diff_main(&self, card_id: &str) -> Result<String, AcpError> {
        let branch = Self::sanitize_branch(card_id);
        let ref_spec = format!("main...{branch}");
        let output = tokio::process::Command::new("git")
            .arg("diff").arg(&ref_spec)
            .current_dir(&self.repo_root)
            .output()
            .await
            .map_err(|e| AcpError::internal(format!("git diff: {e}")))?;
        Ok(String::from_utf8_lossy(&output.stdout).to_string())
    }

    /// Merge the agent branch back into main. On conflict, leaves the
    /// merge in progress and returns the conflict list.
    pub async fn merge_branch(&self, card_id: &str) -> Result<MergeResult, AcpError> {
        self.check_merge_in_progress()?;
        let branch = Self::sanitize_branch(card_id);
        let output = tokio::process::Command::new("git")
            .arg("merge").arg("--no-ff").arg(&branch)
            .current_dir(&self.repo_root)
            .output()
            .await
            .map_err(|e| AcpError::internal(format!("git merge: {e}")))?;
        if output.status.success() {
            return Ok(MergeResult { success: true, conflicts: Vec::new(), repo_blocked: false });
        }
        // Merge conflict — parse conflicted files from stderr.
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        let conflicts = extract_conflicts(&stdout, &stderr);
        Ok(MergeResult { success: false, conflicts, repo_blocked: true })
    }
}

/// Extract conflicted file paths from git merge output.
fn extract_conflicts(stdout: &str, stderr: &str) -> Vec<String> {
    let mut conflicts = Vec::new();
    for line in stdout.lines().chain(stderr.lines()) {
        if line.starts_with("CONFLICT (") {
            if let Some(path) = line.rsplit(' ').next() {
                conflicts.push(path.to_string());
            }
        }
    }
    conflicts
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::process::Command;

    fn temp_git_repo() -> tempfile_tempdir::TempDir {
        let dir = tempfile_tempdir::TempDir::new().unwrap();
        let out = Command::new("git").arg("init").arg("-b").arg("main")
            .current_dir(dir.path()).output().unwrap();
        assert!(out.status.success(), "git init failed: {}", String::from_utf8_lossy(&out.stderr));
        Command::new("git").arg("config").arg("user.email").arg("test@test.com")
            .current_dir(dir.path()).output().unwrap();
        Command::new("git").arg("config").arg("user.name").arg("Test")
            .current_dir(dir.path()).output().unwrap();
        fs::write(dir.path().join("README.md"), "# Test\n").unwrap();
        Command::new("git").arg("add").arg(".")
            .current_dir(dir.path()).output().unwrap();
        let out = Command::new("git").arg("commit").arg("-m").arg("init")
            .current_dir(dir.path()).output().unwrap();
        assert!(out.status.success(), "git commit failed: {}", String::from_utf8_lossy(&out.stderr));
        dir
    }

    mod tempfile_tempdir {
        pub struct TempDir { path: std::path::PathBuf }
        impl TempDir {
            pub fn new() -> std::io::Result<Self> {
                let path = std::env::temp_dir().join(format!("tasker-wt-test-{}", uuid::Uuid::new_v4()));
                std::fs::create_dir_all(&path)?;
                Ok(Self { path })
            }
            pub fn path(&self) -> &std::path::Path { &self.path }
        }
        impl Drop for TempDir {
            fn drop(&mut self) { let _ = std::fs::remove_dir_all(&self.path); }
        }
    }

    #[tokio::test]
    async fn create_and_remove() {
        let dir = temp_git_repo();
        let mgr = WorktreeManager::new(dir.path());
        let wt = mgr.create("card123").await.unwrap();
        assert!(wt.path.exists());
        assert_eq!(wt.branch, "agent/card123");
        mgr.remove("card123").await.unwrap();
        assert!(!wt.path.exists());
    }

    #[tokio::test]
    async fn head_unchanged_after_create() {
        let dir = temp_git_repo();
        let head_before = Command::new("git").arg("rev-parse").arg("HEAD")
            .current_dir(dir.path()).output().unwrap();
        let mgr = WorktreeManager::new(dir.path());
        mgr.create("card456").await.unwrap();
        let head_after = Command::new("git").arg("rev-parse").arg("HEAD")
            .current_dir(dir.path()).output().unwrap();
        assert_eq!(head_before.stdout, head_after.stdout);
        mgr.remove("card456").await.unwrap();
    }

    #[tokio::test]
    async fn diff_main_after_commit() {
        let dir = temp_git_repo();
        let mgr = WorktreeManager::new(dir.path());
        let wt = mgr.create("card789").await.unwrap();
        fs::write(wt.path.join("new.txt"), "content\n").unwrap();
        Command::new("git").arg("add").arg(".")
            .current_dir(&wt.path).output().unwrap();
        Command::new("git").arg("commit").arg("-m").arg("change")
            .current_dir(&wt.path).output().unwrap();
        let diff = mgr.diff_main("card789").await.unwrap();
        assert!(!diff.is_empty());
        mgr.remove("card789").await.unwrap();
    }

    #[tokio::test]
    async fn crashed_dir_blocks_create() {
        let dir = temp_git_repo();
        let mgr = WorktreeManager::new(dir.path());
        let wt = mgr.create("crash1").await.unwrap();
        // Don't remove — simulate crashed run.
        let result = mgr.create("crash1").await;
        assert!(result.is_err());
        mgr.remove("crash1").await.unwrap();
        // Now create should succeed.
        mgr.create("crash1").await.unwrap();
        mgr.remove("crash1").await.unwrap();
    }

    #[tokio::test]
    async fn info_exclude_idempotent() {
        let dir = temp_git_repo();
        let mgr = WorktreeManager::new(dir.path());
        mgr.ensure_excluded().unwrap();
        let content1 = fs::read_to_string(dir.path().join(".git/info/exclude")).unwrap();
        assert!(content1.contains(".tasker-worktrees/"));
        mgr.ensure_excluded().unwrap();
        let content2 = fs::read_to_string(dir.path().join(".git/info/exclude")).unwrap();
        assert_eq!(content1.matches(".tasker-worktrees/").count(), 1);
        assert_eq!(content2, content1);
    }

    #[tokio::test]
    async fn merge_branch_advances_head() {
        let dir = temp_git_repo();
        let mgr = WorktreeManager::new(dir.path());
        let wt = mgr.create("merge1").await.unwrap();
        fs::write(wt.path.join("feature.txt"), "new feature\n").unwrap();
        Command::new("git").arg("add").arg(".")
            .current_dir(&wt.path).output().unwrap();
        Command::new("git").arg("commit").arg("-m").arg("feature")
            .current_dir(&wt.path).output().unwrap();
        let head_before = Command::new("git").arg("rev-parse").arg("HEAD")
            .current_dir(dir.path()).output().unwrap();
        let result = mgr.merge_branch("merge1").await.unwrap();
        assert!(result.success, "merge should succeed: {:?}", result);
        let head_after = Command::new("git").arg("rev-parse").arg("HEAD")
            .current_dir(dir.path()).output().unwrap();
        assert_ne!(head_before.stdout, head_after.stdout, "HEAD should advance");
        mgr.remove("merge1").await.unwrap();
    }

    #[tokio::test]
    async fn merge_conflict_detected() {
        let dir = temp_git_repo();
        let mgr = WorktreeManager::new(dir.path());
        let wt = mgr.create("conflict1").await.unwrap();
        // Commit conflicting change on agent branch.
        fs::write(wt.path.join("README.md"), "# Agent\n").unwrap();
        Command::new("git").arg("add").arg(".")
            .current_dir(&wt.path).output().unwrap();
        Command::new("git").arg("commit").arg("-m").arg("agent change")
            .current_dir(&wt.path).output().unwrap();
        // Conflicting change on main.
        fs::write(dir.path().join("README.md"), "# Main\n").unwrap();
        Command::new("git").arg("add").arg(".")
            .current_dir(dir.path()).output().unwrap();
        Command::new("git").arg("commit").arg("-m").arg("main change")
            .current_dir(dir.path()).output().unwrap();
        let result = mgr.merge_branch("conflict1").await.unwrap();
        assert!(!result.success, "merge should conflict");
        assert!(!result.conflicts.is_empty(), "should have conflicts");
        assert!(result.repo_blocked, "repo should be blocked");
        // Abort the merge to clean up.
        Command::new("git").arg("merge").arg("--abort")
            .current_dir(dir.path()).output().unwrap();
        mgr.remove("conflict1").await.unwrap();
    }
}
