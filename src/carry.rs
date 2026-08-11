//! Carry-in.
//!
//! A git worktree holds only tracked files, so an agent lands somewhere with no
//! `.env`, no certs, no local config — a checkout that cannot run the app it is
//! supposed to work on. `carry_in` names what to copy across.
//!
//! It is deliberately an allowlist, because **this is the only path by which a
//! secret reaches the agent**. Carrying everything gitignored would sweep in
//! every credential you happen to have lying around.
//!
//! Copy, not symlink: a symlink's target would have to resolve inside the
//! sandbox, which would mean mounting your main checkout — exposing the
//! uncommitted work the worktree model exists to protect.

use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Copied,
    /// The source changed since it was carried.
    Refreshed,
    Unchanged,
    /// Listed but not present in the checkout.
    Missing,
    /// Listed, and git already tracks it.
    ///
    /// A misconfiguration rather than a hazard: `carry_in` exists for what the
    /// worktree does *not* get, and a tracked file is already there on the
    /// branch. Carrying one replaces it with whatever the checkout holds right
    /// now, which shows up forever as a modification nobody made in the session
    /// — and, until `commit` learned to refuse it, went out in the commit.
    AlreadyTracked,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Carried {
    pub path: String,
    pub action: Action,
}

/// A pattern names something inside the repo, and nothing else.
///
/// `carry_in` is read from a *committed* layer, so a malicious or careless
/// entry like `../../.ssh` would copy host secrets into a sandbox the agent
/// controls.
pub fn validate_pattern(pattern: &str) -> Result<()> {
    let p = pattern.trim();
    if p.is_empty() {
        anyhow::bail!("an empty carry_in entry names nothing");
    }
    if p.starts_with('/') || p.starts_with('~') {
        anyhow::bail!("carry_in is relative to the repository: `{pattern}`");
    }
    if Path::new(p)
        .components()
        .any(|c| matches!(c, std::path::Component::ParentDir))
    {
        anyhow::bail!("carry_in cannot reach outside the repository: `{pattern}`");
    }
    Ok(())
}

/// Copy the listed paths from the checkout into the worktree.
pub fn apply(repo: &Path, worktree: &Path, patterns: &[String]) -> Result<Vec<Carried>> {
    // Validate everything before copying anything: a list with one bad entry
    // should not half-apply.
    for pattern in patterns {
        validate_pattern(pattern)?;
    }

    let mut out = Vec::new();
    for pattern in patterns {
        let rel = pattern.trim().trim_end_matches('/');
        let src = repo.join(rel);
        let dst = worktree.join(rel);
        let action = if !src.exists() {
            Action::Missing
        } else if tracked(repo, rel) {
            Action::AlreadyTracked
        } else if src.is_dir() {
            copy_dir(&src, &dst)?
        } else {
            copy_file(&src, &dst)?
        };
        out.push(Carried {
            path: pattern.clone(),
            action,
        });
    }

    exclude(worktree, patterns)?;
    Ok(out)
}

/// Whether git already has this path on the branch the session will start from.
///
/// Asked of the checkout rather than the worktree, because that is where the
/// `carry_in` entry is aimed and where the user can see the answer.
fn tracked(repo: &Path, rel: &str) -> bool {
    std::process::Command::new("git")
        .current_dir(repo)
        .args(["ls-files", "--error-unmatch", "--", rel])
        .output()
        .map(|out| out.status.success())
        .unwrap_or(false)
}

/// The checkout is the source of truth for carried files — they are yours, not
/// the agent's — so a changed source replaces the copy.
fn copy_file(src: &Path, dst: &Path) -> Result<Action> {
    let incoming = std::fs::read(src).with_context(|| format!("reading {}", src.display()))?;
    if std::fs::read(dst)
        .map(|existing| existing == incoming)
        .unwrap_or(false)
    {
        return Ok(Action::Unchanged);
    }
    let existed = dst.exists();
    if let Some(parent) = dst.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(dst, incoming).with_context(|| format!("writing {}", dst.display()))?;
    Ok(if existed {
        Action::Refreshed
    } else {
        Action::Copied
    })
}

fn copy_dir(src: &Path, dst: &Path) -> Result<Action> {
    std::fs::create_dir_all(dst)?;
    let mut action = Action::Unchanged;
    for entry in std::fs::read_dir(src)?.flatten() {
        let from = entry.path();
        let to = dst.join(entry.file_name());
        let result = if from.is_dir() {
            copy_dir(&from, &to)?
        } else {
            copy_file(&from, &to)?
        };
        // The directory as a whole is as changed as its most-changed member.
        if result != Action::Unchanged && action == Action::Unchanged {
            action = result;
        }
    }
    Ok(action)
}

/// Where a worktree's private ignore rules live.
///
/// Two traps here, both found empirically. `<worktree>/.git` is a *file*
/// pointing at the admin directory, not a directory — and git reads
/// `info/exclude` from the **common** git dir, not the per-worktree one, so
/// writing to `.git/worktrees/<id>/info/exclude` silently does nothing.
///
/// Consequence worth naming: this file is shared with the main checkout. It is
/// never committed, and carried paths are untracked there by definition, so the
/// effect is invisible — but it is not scoped to the worktree.
pub fn exclude_path(worktree: &Path) -> Option<PathBuf> {
    let out = std::process::Command::new("git")
        .current_dir(worktree)
        .args(["rev-parse", "--path-format=absolute", "--git-common-dir"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let dir = String::from_utf8_lossy(&out.stdout).trim().to_string();
    Some(PathBuf::from(dir).join("info/exclude"))
}

/// The filenames omh stages its rules onto, inside `/work`.
///
/// Named once because three things have to agree about them, in three modules:
/// the `concat` targets the adapters declare, `hide_staged_rules` here, and
/// `Session::commit`'s backstop. A harness whose rules file is named something
/// else needs adding to this list too.
pub const STAGED_RULES: [&str; 2] = ["CLAUDE.md", "AGENTS.md"];

/// Hide the rules filenames from the agent's `git status`.
///
/// omh mounts its rules rather than writing them, so there is usually nothing
/// here to hide. What this covers is the placeholder: a bind mount needs its
/// destination to exist, and a runtime that creates one inside `/work` leaves an
/// empty untracked file behind.
///
/// It cannot cover the tracked case at all — `info/exclude` is gitignore
/// semantics, silent about a file git already has — which is why the mount, not
/// this, is what keeps omh's staging out of the user's commit.
pub fn hide_staged_rules(worktree: &Path) -> Result<()> {
    exclude(worktree, &STAGED_RULES.map(String::from))
}

/// Keep carried files out of the agent's `git status`, so they never show up as
/// untracked noise or get committed onto the session branch.
fn exclude(worktree: &Path, patterns: &[String]) -> Result<()> {
    let Some(path) = exclude_path(worktree) else {
        // A scratch directory has no git at all; nothing to keep clean.
        return Ok(());
    };
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut body = std::fs::read_to_string(&path).unwrap_or_default();
    for pattern in patterns {
        let line = pattern.trim();
        if body.lines().any(|l| l.trim() == line) {
            continue;
        }
        if !body.is_empty() && !body.ends_with('\n') {
            body.push('\n');
        }
        body.push_str(line);
        body.push('\n');
    }
    std::fs::write(&path, body).with_context(|| format!("writing {}", path.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repo(files: &[(&str, &str)]) -> (tempfile::TempDir, PathBuf, PathBuf) {
        let d = tempfile::tempdir().unwrap();
        let repo = d.path().join("repo");
        let worktree = d.path().join("wt");
        std::fs::create_dir_all(repo.join(".git/info")).unwrap();
        std::fs::create_dir_all(&worktree).unwrap();
        for (name, body) in files {
            let p = repo.join(name);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, body).unwrap();
        }
        (d, repo, worktree)
    }

    fn carry(repo: &Path, wt: &Path, list: &[&str]) -> Vec<Carried> {
        let owned: Vec<String> = list.iter().map(|s| s.to_string()).collect();
        apply(repo, wt, &owned).unwrap()
    }

    // ── copying ─────────────────────────────────────────────────────────────

    #[test]
    fn a_listed_file_reaches_the_worktree() {
        let (_d, repo, wt) = repo(&[(".env.local", "SECRET=1")]);
        let out = carry(&repo, &wt, &[".env.local"]);

        assert_eq!(
            std::fs::read_to_string(wt.join(".env.local")).unwrap(),
            "SECRET=1"
        );
        assert_eq!(out[0].action, Action::Copied);
    }

    #[test]
    fn a_listed_directory_is_carried_whole() {
        let (_d, repo, wt) = repo(&[("certs/dev.pem", "cert"), ("certs/nested/ca.pem", "ca")]);
        carry(&repo, &wt, &["certs/"]);

        assert_eq!(
            std::fs::read_to_string(wt.join("certs/dev.pem")).unwrap(),
            "cert"
        );
        assert_eq!(
            std::fs::read_to_string(wt.join("certs/nested/ca.pem")).unwrap(),
            "ca"
        );
    }

    /// A `.env` you thought you were carrying and are not is exactly the
    /// failure that wastes an hour inside the sandbox.
    #[test]
    fn a_listed_path_that_does_not_exist_is_reported() {
        let (_d, repo, wt) = repo(&[]);
        let out = carry(&repo, &wt, &[".env.local"]);
        assert_eq!(out[0].action, Action::Missing);
        assert_eq!(out[0].path, ".env.local");
    }

    #[test]
    fn nothing_listed_carries_nothing() {
        let (_d, repo, wt) = repo(&[(".env", "x")]);
        assert!(carry(&repo, &wt, &[]).is_empty());
        assert!(!wt.join(".env").exists());
    }

    // ── re-running ──────────────────────────────────────────────────────────

    #[test]
    fn an_unchanged_file_is_not_copied_again() {
        let (_d, repo, wt) = repo(&[(".env", "A=1")]);
        carry(&repo, &wt, &[".env"]);
        let out = carry(&repo, &wt, &[".env"]);
        assert_eq!(out[0].action, Action::Unchanged);
    }

    /// The checkout is the source of truth for these files — they are yours,
    /// not the agent's.
    #[test]
    fn a_changed_source_refreshes_the_copy() {
        let (_d, repo, wt) = repo(&[(".env", "A=1")]);
        carry(&repo, &wt, &[".env"]);
        std::fs::write(repo.join(".env"), "A=2").unwrap();

        let out = carry(&repo, &wt, &[".env"]);
        assert_eq!(out[0].action, Action::Refreshed);
        assert_eq!(std::fs::read_to_string(wt.join(".env")).unwrap(), "A=2");
    }

    // ── the agent's git status ──────────────────────────────────────────────

    /// A **real** git worktree, because that is where this broke: `.git` in a
    /// worktree is a *file* pointing at the admin directory, not a directory,
    /// so writing `<worktree>/.git/info/exclude` silently does nothing.
    fn worktree_repo() -> (tempfile::TempDir, PathBuf, PathBuf) {
        let d = tempfile::tempdir().unwrap();
        let repo = d.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        let git = |args: &[&str]| {
            std::process::Command::new("git")
                .current_dir(&repo)
                .args(args)
                .output()
                .unwrap()
        };
        git(&["init", "-q", "-b", "main"]);
        git(&["config", "user.email", "t@e.com"]);
        git(&["config", "user.name", "t"]);
        git(&["commit", "-q", "--allow-empty", "-m", "root"]);
        let wt = d.path().join("wt");
        git(&[
            "worktree",
            "add",
            "-q",
            wt.to_str().unwrap(),
            "-b",
            "omh/s01",
        ]);
        std::fs::write(repo.join(".env.local"), "SECRET").unwrap();
        (d, repo, wt)
    }

    fn status(wt: &Path) -> String {
        let out = std::process::Command::new("git")
            .current_dir(wt)
            .args(["status", "--short"])
            .output()
            .unwrap();
        String::from_utf8_lossy(&out.stdout).into_owned()
    }

    /// `carry_in` is for what a worktree does not get. A tracked file is
    /// already on the branch, so listing one is a misconfiguration — and a
    /// quietly expensive one: the copy lands as a modification nobody made in
    /// the session, and `commit` then has to refuse it. Named at launch, where
    /// the entry can actually be fixed.
    #[test]
    fn a_carry_in_entry_git_already_tracks_is_reported_rather_than_copied() {
        let (_d, repo, wt) = worktree_repo();
        std::fs::write(repo.join("config.toml"), "PORT=3000\n").unwrap();
        for args in [vec!["add", "-A"], vec!["commit", "-q", "-m", "config"]] {
            std::process::Command::new("git")
                .current_dir(&repo)
                .args(&args)
                .output()
                .unwrap();
        }
        // The user's local edit, which is what carrying would copy across.
        std::fs::write(repo.join("config.toml"), "PORT=3000\nSECRET=hunter2\n").unwrap();

        let out = apply(&repo, &wt, &["config.toml".to_string()]).unwrap();

        assert_eq!(out[0].action, Action::AlreadyTracked);
        assert!(
            !wt.join("config.toml").exists(),
            "a tracked path is git's to deliver, and carrying it would have written \
             the checkout's uncommitted copy over whatever the branch holds"
        );
    }

    /// Carried files must not appear as untracked, or the agent commits your
    /// `.env` onto the session branch.
    #[test]
    fn carried_paths_are_hidden_from_the_agents_git_status() {
        let (_d, repo, wt) = worktree_repo();
        carry(&repo, &wt, &[".env.local"]);

        assert!(wt.join(".env.local").exists(), "carried");
        assert!(
            !status(&wt).contains(".env.local"),
            "must not be untracked noise:\n{}",
            status(&wt)
        );
    }

    #[test]
    fn excluding_twice_does_not_duplicate_entries() {
        let (_d, repo, wt) = worktree_repo();
        carry(&repo, &wt, &[".env.local"]);
        carry(&repo, &wt, &[".env.local"]);
        assert!(!status(&wt).contains(".env.local"));
    }

    #[test]
    fn an_existing_exclude_file_is_preserved() {
        let (_d, repo, wt) = worktree_repo();
        let path = exclude_path(&wt).unwrap();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "*.swp\n").unwrap();
        carry(&repo, &wt, &[".env.local"]);

        let body = std::fs::read_to_string(&path).unwrap();
        assert!(
            body.contains("*.swp"),
            "the user's rules must survive: {body}"
        );
    }

    /// The rules filenames stay out of `git status` whatever put them there —
    /// a mount placeholder, or a backend that had to fall back to writing.
    #[test]
    fn omhs_own_staged_files_are_hidden_too() {
        let (_d, _repo, wt) = worktree_repo();
        std::fs::write(wt.join("CLAUDE.md"), "rules").unwrap();
        std::fs::write(wt.join("AGENTS.md"), "rules").unwrap();

        hide_staged_rules(&wt).unwrap();
        let st = status(&wt);
        assert!(!st.contains("CLAUDE.md"), "got:\n{st}");
        assert!(!st.contains("AGENTS.md"), "got:\n{st}");
    }

    // ── escaping the repo ───────────────────────────────────────────────────

    #[test]
    fn ordinary_patterns_are_accepted() {
        for p in [".env", ".env.local", "certs/", "config/local.toml"] {
            validate_pattern(p).unwrap_or_else(|e| panic!("{p}: {e}"));
        }
    }

    /// `carry_in` lives in a committed layer, so a careless entry would copy
    /// host secrets into a sandbox the agent controls.
    #[test]
    fn a_pattern_cannot_escape_the_checkout() {
        for p in [
            "../secrets",
            "../../.ssh",
            "a/../../b",
            "/etc/passwd",
            "~/.ssh",
        ] {
            assert!(validate_pattern(p).is_err(), "`{p}` must be rejected");
        }
    }

    #[test]
    fn an_escaping_pattern_copies_nothing() {
        let (_d, repo, wt) = repo(&[]);
        let owned = vec!["../../.ssh".to_string()];
        assert!(
            apply(&repo, &wt, &owned).is_err(),
            "must refuse, not silently skip"
        );
    }
}
