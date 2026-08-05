//! Profile resolution. Three layers, later winning:
//!
//!   1. `~/.omh/profile`        personal, every project
//!   2. `<repo>/.omh/profile`   project, committed, shared with the team
//!   3. `<repo>/.omh/local`     project, gitignored, yours alone
//!
//! Layer 2 is committed, so it must never hold a secret; that is what layer 3
//! and `carry_in` are for.
//!
//! Nothing here is ever copied into your home directory. Layers resolve to a
//! list of paths and the launcher bind-mounts them, which is why there is no
//! drift to fight and no daemon to run.

use crate::adapter::Capability;
use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

pub struct Paths {
    pub root: PathBuf,
    pub repo: PathBuf,
}

impl Paths {
    pub fn discover(cwd: &Path) -> Result<Self> {
        let home = dirs::home_dir().context("no home directory")?;
        Ok(Self {
            root: home.join(".omh"),
            repo: repo_root(cwd)?,
        })
    }

    pub fn adapters(&self) -> PathBuf {
        self.root.join("adapters")
    }

    pub fn editors(&self) -> PathBuf {
        self.root.join("editors")
    }

    pub fn creds(&self, harness: &str) -> PathBuf {
        self.root.join("creds").join(harness)
    }

    /// Outside the repo on purpose: nested worktrees make your IDE index every
    /// session's full copy of the codebase.
    pub fn worktrees(&self) -> PathBuf {
        self.root.join("worktrees").join(self.repo_id())
    }

    /// Per-launch staging. Keyed by repo as well as session and harness: two
    /// checkouts both on `s01` must not share a rendered profile.
    pub fn staging(&self, session: &str, harness: &str) -> PathBuf {
        self.root
            .join("run")
            .join(self.repo_id())
            .join(session)
            .join(harness)
    }

    /// A throwaway working directory, deliberately outside `worktrees/` so a
    /// login never appears in `omh s ls` as a session you could resume.
    pub fn scratch(&self, name: &str) -> PathBuf {
        self.root.join("scratch").join(self.repo_id()).join(name)
    }

    pub fn keys(&self) -> PathBuf {
        self.root.join("keys").join(self.repo_id())
    }

    /// Cache volume — keyed by repo, deliberately not by harness. This is what
    /// lets memory survive a harness switch.
    pub fn cache_volume(&self) -> String {
        format!("omh-cache-{}", self.repo_id())
    }

    pub fn network(&self) -> String {
        format!("omh-{}", self.repo_id())
    }

    pub fn container(&self, session: &str) -> String {
        format!("omh-{}-{session}", self.repo_id())
    }

    pub fn repo_name(&self) -> String {
        self.repo_id()
    }

    fn repo_id(&self) -> String {
        self.repo
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "repo".into())
    }
}

pub struct Profile {
    /// Existing layers, in application order.
    pub layers: Vec<PathBuf>,
}

impl Profile {
    pub fn resolve(paths: &Paths) -> Self {
        let layers = [
            paths.root.join("profile"),
            paths.repo.join(".omh/profile"),
            paths.repo.join(".omh/local"),
        ]
        .into_iter()
        .filter(|p| p.exists())
        .collect();
        Self { layers }
    }

    /// Every layer's copy of `cap`'s source, in application order. Missing
    /// layers are skipped, so an empty result means "nothing declared".
    pub fn sources(&self, cap: Capability) -> Vec<PathBuf> {
        self.layers
            .iter()
            .map(|l| l.join(cap.source()))
            .filter(|p| p.exists())
            .collect()
    }

    /// Capabilities the profile actually carries — used to report what an
    /// adapter had to drop.
    pub fn declared(&self) -> Vec<Capability> {
        Capability::ALL
            .into_iter()
            .filter(|c| !self.sources(*c).is_empty())
            .collect()
    }
}

/// Walk up looking for `.git`. The worktree model needs a real repo, so a
/// missing one is a hard error rather than a silent fallback to `cwd`.
pub fn repo_root(start: &Path) -> Result<PathBuf> {
    let mut cur = start.canonicalize().unwrap_or_else(|_| start.to_path_buf());
    loop {
        if cur.join(".git").exists() {
            return Ok(cur);
        }
        if !cur.pop() {
            anyhow::bail!(
                "{} is not inside a git repository\n\
                 omh isolates the agent on a worktree branch, which needs one.\n\
                 run `git init` first.",
                start.display()
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fixture {
        _dir: tempfile::TempDir,
        paths: Paths,
    }

    fn fixture(layers: &[(&str, &str, &str)]) -> Fixture {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths { root: dir.path().join("home"), repo: dir.path().join("repo") };
        for (layer, name, body) in layers {
            let base = match *layer {
                "personal" => paths.root.join("profile"),
                "shared" => paths.repo.join(".omh/profile"),
                "local" => paths.repo.join(".omh/local"),
                other => panic!("unknown layer {other}"),
            };
            let p = base.join(name);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, body).unwrap();
        }
        Fixture { _dir: dir, paths }
    }

    #[test]
    fn layers_apply_personal_then_shared_then_local() {
        let f = fixture(&[
            ("personal", "AGENTS.md", "one"),
            ("shared", "AGENTS.md", "two"),
            ("local", "AGENTS.md", "three"),
        ]);
        let sources = Profile::resolve(&f.paths).sources(Capability::Rules);
        let bodies: Vec<_> = sources.iter().map(|p| std::fs::read_to_string(p).unwrap()).collect();
        assert_eq!(bodies, ["one", "two", "three"], "local must apply last");
    }

    #[test]
    fn absent_layers_are_skipped_not_faked() {
        let f = fixture(&[("local", "AGENTS.md", "only")]);
        let profile = Profile::resolve(&f.paths);
        assert_eq!(profile.layers.len(), 1);
        assert_eq!(profile.sources(Capability::Rules).len(), 1);
        assert!(profile.sources(Capability::Skills).is_empty());
    }

    #[test]
    fn declared_reports_only_present_capabilities() {
        let f = fixture(&[
            ("personal", "AGENTS.md", "r"),
            ("shared", "mcp.json", "{}"),
            ("shared", "skills/x/SKILL.md", "s"),
        ]);
        let declared = Profile::resolve(&f.paths).declared();
        assert_eq!(declared, vec![Capability::Rules, Capability::Skills, Capability::Mcp]);
    }

    /// Worktrees live outside the repo so an IDE opened on the repo root does not
    /// index every session's full copy of the codebase.
    #[test]
    fn worktrees_live_outside_the_repo() {
        let f = fixture(&[]);
        assert!(!f.paths.worktrees().starts_with(&f.paths.repo));
        assert!(f.paths.worktrees().starts_with(&f.paths.root));
    }

    /// Keyed by repo, not harness — this is what lets memory survive a switch.
    #[test]
    fn cache_volume_is_harness_independent() {
        let f = fixture(&[]);
        assert_eq!(f.paths.cache_volume(), "omh-cache-repo");
    }

    #[test]
    fn missing_git_repo_is_a_hard_error() {
        let dir = tempfile::tempdir().unwrap();
        let err = repo_root(dir.path()).unwrap_err();
        assert!(err.to_string().contains("git init"), "got: {err}");
    }




    /// Regression: staging was keyed by session and harness only, so two repos
    /// both using session `s01` shared one rendered profile — repo A's MCP
    /// config could be mounted into repo B's sandbox.
    #[test]
    fn staging_is_keyed_by_repo() {
        let dir = tempfile::tempdir().unwrap();
        let a = Paths { root: dir.path().into(), repo: dir.path().join("alpha") };
        let b = Paths { root: dir.path().into(), repo: dir.path().join("beta") };
        assert_ne!(a.staging("s01", "claude"), b.staging("s01", "claude"));
    }

    #[test]
    fn staging_still_separates_sessions_and_harnesses() {
        let f = fixture(&[]);
        let p = &f.paths;
        assert_ne!(p.staging("s01", "claude"), p.staging("s02", "claude"));
        assert_ne!(p.staging("s01", "claude"), p.staging("s01", "opencode"));
    }
}
