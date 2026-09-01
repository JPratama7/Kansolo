use crate::error::AcpError;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Created worktree: path on disk + branch name + the repo's default
/// branch (resolved at creation, used for diff/merge targets).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Worktree {
    pub path: PathBuf,
    pub branch: String,
    pub default_branch: String,
}

/// Merge result for folding an agent branch back into main.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MergeResult {
    pub success: bool,
    pub conflicts: Vec<String>,
    pub repo_blocked: bool,
}

/// Manages git worktrees under `.tasker-worktrees/` for agent runs.
/// Built from the card's resolved repo path, not the ambient cwd.
pub struct WorktreeManager {
    repo_root: PathBuf,
    wt_root: PathBuf,
}

impl WorktreeManager {
    pub fn new(repo_root: &Path) -> Self {
        let wt_root = repo_root.join(".tasker-worktrees");
        Self {
            repo_root: repo_root.to_path_buf(),
            wt_root,
        }
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
        let prefix = if content.is_empty() || content.ends_with('\n') {
            ""
        } else {
            "\n"
        };
        std::fs::write(
            &exclude_path,
            format!("{content}{prefix}.tasker-worktrees/\n"),
        )
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
            .map(|c| {
                if c.is_alphanumeric() || c == '-' || c == '_' {
                    c
                } else {
                    '-'
                }
            })
            .collect();
        format!("agent/{cleaned}")
    }

    /// Validate `card_id` before using it in filesystem paths.
    /// Rejects path separators, `..`, and empty strings — prevents
    /// `wt_root.join(card_id)` from escaping the `.tasker-worktrees` sandbox.
    fn validate_card_id(card_id: &str) -> Result<(), AcpError> {
        if card_id.is_empty()
            || card_id.contains('/')
            || card_id.contains('\\')
            || card_id == ".."
            || card_id.contains("..")
        {
            return Err(AcpError::validation(format!(
                "invalid card_id: {card_id:?}"
            )));
        }
        Ok(())
    }

    /// Resolve the repo's default branch once. Tries, in order:
    ///   1. `git symbolic-ref refs/remotes/origin/HEAD` (origin's default)
    ///   2. `git symbolic-ref --short HEAD`            (current branch)
    ///   3. `"main"`                                    (last-resort fallback)
    async fn resolve_default_branch(&self) -> String {
        let try_symbolic = |args: &[&str]| -> Option<String> {
            // Synchronous spawn is fine: this is a quick local git call.
            let out = std::process::Command::new("git")
                .args(args)
                .current_dir(&self.repo_root)
                .output()
                .ok()?;
            if !out.status.success() {
                return None;
            }
            let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
            if s.is_empty() {
                None
            } else {
                Some(s)
            }
        };
        if let Some(full) = try_symbolic(&["symbolic-ref", "refs/remotes/origin/HEAD"]) {
            if let Some(b) = full.strip_prefix("refs/remotes/origin/") {
                return b.to_string();
            }
        }
        if let Some(b) = try_symbolic(&["symbolic-ref", "--short", "HEAD"]) {
            return b;
        }
        "main".to_string()
    }

    /// Create a worktree for `card_id` on branch `agent/<card_id>`.
    pub async fn create(&self, card_id: &str) -> Result<Worktree, AcpError> {
        Self::validate_card_id(card_id)?;
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
            .arg("worktree")
            .arg("add")
            .arg("-b")
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
        let default_branch = self.resolve_default_branch().await;
        Ok(Worktree {
            path: wt_path,
            branch,
            default_branch,
        })
    }

    /// Remove a worktree by card_id. Safe if already gone.
    /// Also deletes the agent branch.
    pub async fn remove(&self, card_id: &str) -> Result<(), AcpError> {
        Self::validate_card_id(card_id)?;
        let wt_path = self.wt_root.join(card_id);
        if wt_path.exists() {
            let _ = tokio::process::Command::new("git")
                .arg("worktree")
                .arg("remove")
                .arg("--force")
                .arg(&wt_path)
                .current_dir(&self.repo_root)
                .output()
                .await;
            let _ = tokio::process::Command::new("git")
                .arg("worktree")
                .arg("prune")
                .current_dir(&self.repo_root)
                .output()
                .await;
        }
        // Delete the branch (safe if already gone).
        let branch = Self::sanitize_branch(card_id);
        let _ = tokio::process::Command::new("git")
            .arg("branch")
            .arg("-D")
            .arg(&branch)
            .current_dir(&self.repo_root)
            .output()
            .await;
        Ok(())
    }

    /// Diff between the repo's default branch and the agent branch for `card_id`.
    pub async fn diff_main(&self, card_id: &str) -> Result<String, AcpError> {
        Self::validate_card_id(card_id)?;
        let branch = Self::sanitize_branch(card_id);
        let default_branch = self.resolve_default_branch().await;
        let ref_spec = format!("{default_branch}...{branch}");
        let output = tokio::process::Command::new("git")
            .arg("diff")
            .arg(&ref_spec)
            .current_dir(&self.repo_root)
            .output()
            .await
            .map_err(|e| AcpError::internal(format!("git diff: {e}")))?;
        Ok(String::from_utf8_lossy(&output.stdout).to_string())
    }

    /// Merge the agent branch back into the repo's default branch. On
    /// conflict, leaves the merge in progress and returns the conflict list.
    pub async fn merge_branch(&self, card_id: &str) -> Result<MergeResult, AcpError> {
        Self::validate_card_id(card_id)?;
        self.check_merge_in_progress()?;
        let branch = Self::sanitize_branch(card_id);
        let default_branch = self.resolve_default_branch().await;
        // Check out the default branch first so the merge target is
        // deterministic, not whatever HEAD happens to be.
        let checkout = tokio::process::Command::new("git")
            .arg("checkout")
            .arg(&default_branch)
            .current_dir(&self.repo_root)
            .output()
            .await
            .map_err(|e| AcpError::internal(format!("git checkout: {e}")))?;
        if !checkout.status.success() {
            return Err(AcpError::internal(format!(
                "git checkout {default_branch} failed: {}",
                String::from_utf8_lossy(&checkout.stderr)
            )));
        }
        let output = tokio::process::Command::new("git")
            .arg("merge")
            .arg("--no-ff")
            .arg(&branch)
            .current_dir(&self.repo_root)
            .output()
            .await
            .map_err(|e| AcpError::internal(format!("git merge: {e}")))?;
        if output.status.success() {
            return Ok(MergeResult {
                success: true,
                conflicts: Vec::new(),
                repo_blocked: false,
            });
        }
        // Merge conflict — prefer `git diff --name-only --diff-filter=U` which
        // lists unmerged paths reliably (and handles spaces in paths). Fall
        // back to parsing merge output if that yields nothing.
        let conflicts = self.conflicted_files().await.unwrap_or_else(|| {
            let stderr = String::from_utf8_lossy(&output.stderr).to_string();
            let stdout = String::from_utf8_lossy(&output.stdout).to_string();
            extract_conflicts(&stdout, &stderr)
        });
        Ok(MergeResult {
            success: false,
            conflicts,
            repo_blocked: true,
        })
    }

    /// List unmerged paths via `git diff --name-only --diff-filter=U`.
    /// Returns None if the command fails or yields no paths (caller falls
    /// back to parsing merge output).
    async fn conflicted_files(&self) -> Option<Vec<String>> {
        let out = tokio::process::Command::new("git")
            .arg("diff")
            .arg("--name-only")
            .arg("--diff-filter=U")
            .arg("-z")
            .current_dir(&self.repo_root)
            .output()
            .await
            .ok()?;
        if !out.status.success() {
            return None;
        }
        let paths: Vec<String> = String::from_utf8_lossy(&out.stdout)
            .split('\0')
            .filter(|p| !p.is_empty())
            .map(|p| p.to_string())
            .collect();
        if paths.is_empty() {
            None
        } else {
            Some(paths)
        }
    }
}

/// Extract conflicted file paths from git merge output (fallback path).
/// Parses lines like `CONFLICT (content): Merge conflict in src/my file.rs`
/// by stripping the `CONFLICT (` prefix, splitting on `): `, then stripping
/// the leading `Merge conflict in ` description — so paths containing spaces
/// are preserved (the old code took the last space-separated token).
fn extract_conflicts(stdout: &str, stderr: &str) -> Vec<String> {
    let mut conflicts = Vec::new();
    for line in stdout.lines().chain(stderr.lines()) {
        if let Some(rest) = line.strip_prefix("CONFLICT (") {
            // rest: `content): Merge conflict in src/my file.rs`
            let Some((_kind, after)) = rest.split_once("): ") else {
                continue;
            };
            // after: `Merge conflict in src/my file.rs`
            if let Some(path) = after.strip_prefix("Merge conflict in ") {
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
        temp_git_repo_with_branch("main")
    }

    fn temp_git_repo_with_branch(branch: &str) -> tempfile_tempdir::TempDir {
        let dir = tempfile_tempdir::TempDir::new().unwrap();
        let out = Command::new("git")
            .arg("init")
            .arg("-b")
            .arg(branch)
            .current_dir(dir.path())
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "git init failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        Command::new("git")
            .arg("config")
            .arg("user.email")
            .arg("test@test.com")
            .current_dir(dir.path())
            .output()
            .unwrap();
        Command::new("git")
            .arg("config")
            .arg("user.name")
            .arg("Test")
            .current_dir(dir.path())
            .output()
            .unwrap();
        fs::write(dir.path().join("README.md"), "# Test\n").unwrap();
        Command::new("git")
            .arg("add")
            .arg(".")
            .current_dir(dir.path())
            .output()
            .unwrap();
        let out = Command::new("git")
            .arg("commit")
            .arg("-m")
            .arg("init")
            .current_dir(dir.path())
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "git commit failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        dir
    }

    mod tempfile_tempdir {
        pub struct TempDir {
            path: std::path::PathBuf,
        }
        impl TempDir {
            pub fn new() -> std::io::Result<Self> {
                let path =
                    std::env::temp_dir().join(format!("tasker-wt-test-{}", uuid::Uuid::new_v4()));
                std::fs::create_dir_all(&path)?;
                Ok(Self { path })
            }
            pub fn path(&self) -> &std::path::Path {
                &self.path
            }
        }
        impl Drop for TempDir {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.path);
            }
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
        let head_before = Command::new("git")
            .arg("rev-parse")
            .arg("HEAD")
            .current_dir(dir.path())
            .output()
            .unwrap();
        let mgr = WorktreeManager::new(dir.path());
        mgr.create("card456").await.unwrap();
        let head_after = Command::new("git")
            .arg("rev-parse")
            .arg("HEAD")
            .current_dir(dir.path())
            .output()
            .unwrap();
        assert_eq!(head_before.stdout, head_after.stdout);
        mgr.remove("card456").await.unwrap();
    }

    #[tokio::test]
    async fn diff_main_after_commit() {
        let dir = temp_git_repo();
        let mgr = WorktreeManager::new(dir.path());
        let wt = mgr.create("card789").await.unwrap();
        fs::write(wt.path.join("new.txt"), "content\n").unwrap();
        Command::new("git")
            .arg("add")
            .arg(".")
            .current_dir(&wt.path)
            .output()
            .unwrap();
        Command::new("git")
            .arg("commit")
            .arg("-m")
            .arg("change")
            .current_dir(&wt.path)
            .output()
            .unwrap();
        let diff = mgr.diff_main("card789").await.unwrap();
        assert!(!diff.is_empty());
        mgr.remove("card789").await.unwrap();
    }

    #[tokio::test]
    async fn crashed_dir_blocks_create() {
        let dir = temp_git_repo();
        let mgr = WorktreeManager::new(dir.path());
        let _wt = mgr.create("crash1").await.unwrap();
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
        Command::new("git")
            .arg("add")
            .arg(".")
            .current_dir(&wt.path)
            .output()
            .unwrap();
        Command::new("git")
            .arg("commit")
            .arg("-m")
            .arg("feature")
            .current_dir(&wt.path)
            .output()
            .unwrap();
        let head_before = Command::new("git")
            .arg("rev-parse")
            .arg("HEAD")
            .current_dir(dir.path())
            .output()
            .unwrap();
        let result = mgr.merge_branch("merge1").await.unwrap();
        assert!(result.success, "merge should succeed: {:?}", result);
        let head_after = Command::new("git")
            .arg("rev-parse")
            .arg("HEAD")
            .current_dir(dir.path())
            .output()
            .unwrap();
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
        Command::new("git")
            .arg("add")
            .arg(".")
            .current_dir(&wt.path)
            .output()
            .unwrap();
        Command::new("git")
            .arg("commit")
            .arg("-m")
            .arg("agent change")
            .current_dir(&wt.path)
            .output()
            .unwrap();
        // Conflicting change on main.
        fs::write(dir.path().join("README.md"), "# Main\n").unwrap();
        Command::new("git")
            .arg("add")
            .arg(".")
            .current_dir(dir.path())
            .output()
            .unwrap();
        Command::new("git")
            .arg("commit")
            .arg("-m")
            .arg("main change")
            .current_dir(dir.path())
            .output()
            .unwrap();
        let result = mgr.merge_branch("conflict1").await.unwrap();
        assert!(!result.success, "merge should conflict");
        assert!(!result.conflicts.is_empty(), "should have conflicts");
        assert!(result.repo_blocked, "repo should be blocked");
        // Abort the merge to clean up.
        Command::new("git")
            .arg("merge")
            .arg("--abort")
            .current_dir(dir.path())
            .output()
            .unwrap();
        mgr.remove("conflict1").await.unwrap();
    }

    #[test]
    fn extract_conflicts_preserves_spaces_in_path() {
        let stdout = "CONFLICT (content): Merge conflict in src/my file.rs\n";
        let conflicts = extract_conflicts(stdout, "");
        assert_eq!(conflicts, vec!["src/my file.rs".to_string()]);
    }

    #[test]
    fn extract_conflicts_ignores_non_conflict_lines() {
        let stdout = "Auto-merging foo.rs\nCONFLICT (content): Merge conflict in a/b c.rs\nUpdating abc..def\n";
        let conflicts = extract_conflicts(stdout, "");
        assert_eq!(conflicts, vec!["a/b c.rs".to_string()]);
    }

    #[tokio::test]
    async fn diff_and_merge_target_master_default_branch() {
        let dir = temp_git_repo_with_branch("master");
        let mgr = WorktreeManager::new(dir.path());
        let wt = mgr.create("mastercard").await.unwrap();
        assert_eq!(
            wt.default_branch, "master",
            "default branch should be master"
        );
        // Commit a change on the agent branch.
        fs::write(wt.path.join("feature.txt"), "new feature\n").unwrap();
        Command::new("git")
            .arg("add")
            .arg(".")
            .current_dir(&wt.path)
            .output()
            .unwrap();
        Command::new("git")
            .arg("commit")
            .arg("-m")
            .arg("feature")
            .current_dir(&wt.path)
            .output()
            .unwrap();
        // diff_main must diff against master, not main. If it targeted a
        // non-existent `main` branch the diff would be empty/error.
        let diff = mgr.diff_main("mastercard").await.unwrap();
        assert!(
            diff.contains("feature.txt"),
            "diff should include the new file"
        );
        // HEAD before merge must be on master.
        let head_before = Command::new("git")
            .arg("rev-parse")
            .arg("HEAD")
            .current_dir(dir.path())
            .output()
            .unwrap();
        let result = mgr.merge_branch("mastercard").await.unwrap();
        assert!(result.success, "merge should succeed: {:?}", result);
        let head_after = Command::new("git")
            .arg("rev-parse")
            .arg("HEAD")
            .current_dir(dir.path())
            .output()
            .unwrap();
        assert_ne!(
            head_before.stdout, head_after.stdout,
            "HEAD should advance on master"
        );
        // Confirm we are still on master after the merge.
        let branch = Command::new("git")
            .arg("symbolic-ref")
            .arg("--short")
            .arg("HEAD")
            .current_dir(dir.path())
            .output()
            .unwrap();
        assert_eq!(String::from_utf8_lossy(&branch.stdout).trim(), "master");
        mgr.remove("mastercard").await.unwrap();
    }

    #[test]
    fn validate_card_id_rejects_traversal() {
        assert!(WorktreeManager::validate_card_id("../foo").is_err());
        assert!(WorktreeManager::validate_card_id("a/../b").is_err());
        assert!(WorktreeManager::validate_card_id("a/b").is_err());
        assert!(WorktreeManager::validate_card_id("a\\b").is_err());
        assert!(WorktreeManager::validate_card_id("").is_err());
        assert!(WorktreeManager::validate_card_id("..").is_err());
    }

    #[test]
    fn validate_card_id_accepts_uuid() {
        assert!(WorktreeManager::validate_card_id("550e8400-e29b-41d4-a716-446655440000").is_ok());
        assert!(WorktreeManager::validate_card_id("simple-id").is_ok());
    }
}
