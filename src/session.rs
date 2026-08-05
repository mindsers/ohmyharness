//! Sessions are git worktrees on their own branch.
//!
//! This is the part that makes the sandbox real. The container protects your
//! *host*; the worktree protects your *repo*. Your working tree is never
//! mounted, so an agent cannot touch uncommitted work or `main` — review is a
//! plain `git diff`.

use anyhow::{Context, Result};
use std::path::{Path, PathBuf};
use std::process::Command;

pub struct Session {
    pub id: String,
    pub branch: String,
    pub worktree: PathBuf,
}

impl Session {
    pub fn new(worktrees_dir: &Path, id: String) -> Self {
        Self {
            branch: format!("omh/{id}"),
            worktree: worktrees_dir.join(&id),
            id,
        }
    }

    /// Create the worktree if it does not exist yet. Idempotent, so relaunching
    /// into an existing session resumes it.
    pub fn ensure(&self, repo: &Path) -> Result<()> {
        if self.worktree.exists() {
            return Ok(());
        }
        std::fs::create_dir_all(self.worktree.parent().unwrap())?;
        let path = self.worktree.to_string_lossy().into_owned();

        // `omh rm` keeps branches on purpose, so a session id can outlive its
        // worktree. Reattach to the existing branch rather than failing — that
        // is what resuming a session means.
        let args: Vec<&str> = if self.branch_exists(repo) {
            vec!["worktree", "add", &path, &self.branch]
        } else {
            vec!["worktree", "add", "-b", &self.branch, &path]
        };
        git(repo, &args)
            .with_context(|| format!("creating worktree for session {}", self.id))?;
        Ok(())
    }

    fn branch_exists(&self, repo: &Path) -> bool {
        git(repo, &["rev-parse", "--verify", "--quiet", &self.branch]).is_ok()
    }

    pub fn remove(&self, repo: &Path) -> Result<()> {
        git(repo, &["worktree", "remove", "--force", &self.worktree.to_string_lossy()])?;
        // The branch outlives the worktree on purpose: removing a session must
        // not be able to destroy work that was never reviewed.
        Ok(())
    }

    pub fn diff(&self, repo: &Path, base: &str) -> Result<String> {
        git(repo, &["diff", "--stat", &format!("{base}...{}", self.branch)])
    }
}

/// Human-readable, monotonic session ids: `s01`, `s02`, ...
pub fn next_id(worktrees_dir: &Path) -> String {
    let used = std::fs::read_dir(worktrees_dir)
        .map(|entries| {
            entries
                .flatten()
                .filter_map(|e| {
                    e.file_name()
                        .to_string_lossy()
                        .strip_prefix('s')?
                        .parse::<u32>()
                        .ok()
                })
                .max()
                .unwrap_or(0)
        })
        .unwrap_or(0);
    format!("s{:02}", used + 1)
}

pub fn list(worktrees_dir: &Path) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(worktrees_dir) else {
        return Vec::new();
    };
    let mut out: Vec<_> = entries
        .flatten()
        .filter(|e| e.path().is_dir())
        .map(|e| e.file_name().to_string_lossy().to_string())
        .collect();
    out.sort();
    out
}

fn git(cwd: &Path, args: &[&str]) -> Result<String> {
    let out = Command::new("git")
        .current_dir(cwd)
        .args(args)
        .output()
        .context("running git")?;
    if !out.status.success() {
        anyhow::bail!("git {}: {}", args.join(" "), String::from_utf8_lossy(&out.stderr).trim());
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repo() -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("repo");
        std::fs::create_dir_all(&root).unwrap();
        for args in [
            vec!["init", "-q", "-b", "main"],
            vec!["config", "user.email", "t@example.com"],
            vec!["config", "user.name", "t"],
            vec!["commit", "-q", "--allow-empty", "-m", "root"],
        ] {
            git(&root, &args).unwrap();
        }
        (dir, root)
    }

    #[test]
    fn ensure_creates_worktree_on_its_own_branch() {
        let (d, root) = repo();
        let s = Session::new(&d.path().join("wt"), "s01".into());
        s.ensure(&root).unwrap();
        assert!(s.worktree.join(".git").exists());
        assert_eq!(s.branch, "omh/s01");
    }

    #[test]
    fn ensure_is_idempotent() {
        let (d, root) = repo();
        let s = Session::new(&d.path().join("wt"), "s01".into());
        s.ensure(&root).unwrap();
        s.ensure(&root).unwrap();
    }

    /// Regression: `rm` keeps branches on purpose so unreviewed work can never be
    /// destroyed — which made reusing a session id fail, because `ensure` always
    /// passed `-b`. Resuming a session must reattach to its existing branch.
    #[test]
    fn session_resumes_onto_a_surviving_branch() {
        let (d, root) = repo();
        let wt = d.path().join("wt");
        let s = Session::new(&wt, "s01".into());

        s.ensure(&root).unwrap();
        std::fs::write(s.worktree.join("work.txt"), "agent output").unwrap();
        git(&s.worktree, &["add", "-A"]).unwrap();
        git(&s.worktree, &["commit", "-q", "-m", "agent work"]).unwrap();

        s.remove(&root).unwrap();
        assert!(!s.worktree.exists(), "worktree gone");
        assert!(s.branch_exists(&root), "branch must survive rm");

        s.ensure(&root).unwrap();
        assert_eq!(
            std::fs::read_to_string(s.worktree.join("work.txt")).unwrap(),
            "agent output",
            "resuming must recover the branch's work, not start empty"
        );
    }

    #[test]
    fn diff_reports_against_base() {
        let (d, root) = repo();
        let s = Session::new(&d.path().join("wt"), "s01".into());
        s.ensure(&root).unwrap();
        std::fs::write(s.worktree.join("new.rs"), "fn main() {}").unwrap();
        git(&s.worktree, &["add", "-A"]).unwrap();
        git(&s.worktree, &["commit", "-q", "-m", "add"]).unwrap();
        assert!(s.diff(&root, "main").unwrap().contains("new.rs"));
    }

    #[test]
    fn ids_increment_and_list_in_order() {
        let d = tempfile::tempdir().unwrap();
        let wt = d.path().join("wt");
        std::fs::create_dir_all(&wt).unwrap();
        assert_eq!(next_id(&wt), "s01");
        std::fs::create_dir_all(wt.join("s01")).unwrap();
        std::fs::create_dir_all(wt.join("s02")).unwrap();
        assert_eq!(next_id(&wt), "s03");
        assert_eq!(list(&wt), ["s01", "s02"]);
    }

    #[test]
    fn git_failure_surfaces_stderr() {
        let (_d, root) = repo();
        let err = git(&root, &["rev-parse", "--verify", "nope"]).unwrap_err();
        assert!(err.to_string().contains("rev-parse"), "got: {err}");
    }
}
