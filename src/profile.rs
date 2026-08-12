//! One catalogue, and it is personal.
//!
//! ```text
//! ~/.omh/rules/ skills/ commands/ subagents/ hooks/ mcp.json
//! ```
//!
//! Content used to live in three layers of identical shape — `~/.omh/profile`,
//! `<repo>/.omh/profile`, `<repo>/.omh/local` — which meant "where is this
//! skill" had three answers, and `sources` was a union: a later layer could
//! shadow a same-named entry, but nothing could turn one off. The only lever
//! was not installing it globally, which is the opposite of a catalogue.
//!
//! **Hooks are the exception, and the reason is in the capability itself.** A
//! skill is a way *you* work and travels with you across repos. A hook binds to
//! a repo's own commands — `cargo test` here, `pnpm test` next door, one name
//! and two bodies — so a capability that is project-specific by nature has to be
//! declarable where the project is, or the catalogue fills with entries that are
//! only ever right in one place. So the rule is not "no content in the repo", it
//! is **content lives where its scope is**.
//!
//! What that costs, and it is real: a repo can no longer ship a skill, an MCP
//! server or a command to your teammates. What it still shares is its rules
//! file — which for the first time actually reaches the agent — its hooks, and
//! its settings. Recorded, not built: a catalogue entry could carry a `source`
//! and `omh sync` could fetch missing ones, which restores team sharing without
//! putting content back in the repo.
//!
//! Nothing here is ever copied into your home directory. A capability resolves
//! to a list of paths and the launcher bind-mounts them, which is why there is
//! no drift to fight and no daemon to run.

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

    /// The base set as shipped: what `init` seeds and what `omh why` explains.
    /// Versioned files, oldest kept, so an upgrade can eventually diff two.
    pub fn base(&self) -> PathBuf {
        self.root.join("base")
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
        self.runs().join(session).join(harness)
    }

    /// Per-repo run state: staged profiles, and the marker recording when each
    /// session was last used.
    pub fn runs(&self) -> PathBuf {
        self.root.join("run").join(self.repo_id())
    }

    /// A throwaway working directory, deliberately outside `worktrees/` so a
    /// login never appears in `omh s ls` as a session you could resume.
    pub fn scratch(&self, name: &str) -> PathBuf {
        self.root.join("scratch").join(self.repo_id()).join(name)
    }

    pub fn keys(&self) -> PathBuf {
        self.root.join("keys").join(self.repo_id())
    }

    /// The local note store — keyed by repo, and outside the checkout so it
    /// outlives the worktree that produced it. A session is a git worktree
    /// holding tracked files only, and `omh s rm` removes it with `--force`,
    /// so a gitignored store inside the repo would be both invisible to the
    /// sandbox and destroyed by session removal.
    ///
    /// The committed half of the store is not here: it is tracked, so it
    /// belongs in the repo, and it arrives in every worktree by itself.
    pub fn notes(&self) -> PathBuf {
        self.root.join("notes").join(self.repo_id())
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
    /// The catalogue — yours, every project.
    root: PathBuf,
    /// This checkout, which declares hooks and nothing else.
    repo: PathBuf,
}

impl Profile {
    pub fn resolve(paths: &Paths) -> Self {
        Self {
            root: paths.root.clone(),
            repo: paths.repo.clone(),
        }
    }

    /// Where `cap` is declared, in application order — so a later path wins.
    ///
    /// One entry for five of the six. Hooks get a second because they are the
    /// one capability with a repo tier, and the repo's come **last**: a project
    /// overrides your personal `format` hook with the one it actually needs,
    /// without either being renamed.
    ///
    /// Absent paths are skipped, so an empty result means "nothing declared" —
    /// and it stays a `Vec` rather than an `Option` because every caller
    /// downstream merges a list, and hooks would have needed the list anyway.
    pub fn sources(&self, cap: Capability) -> Vec<PathBuf> {
        let mut out = vec![self.root.join(cap.source())];
        if cap == Capability::Hooks {
            out.push(self.repo.join(".omh").join(cap.source()));
        }
        out.retain(|p| p.exists());
        out
    }

    /// Capabilities the profile actually carries.
    ///
    /// Not currently called outside tests: the launcher reports dropped
    /// capabilities from the adapter side instead. Kept because it is the
    /// profile-side half of that answer and `omh eject` will need it.
    #[allow(dead_code)]
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
        let paths = Paths {
            root: dir.path().join("home"),
            repo: dir.path().join("repo"),
        };
        for (layer, name, body) in layers {
            let base = match *layer {
                "catalogue" => paths.root.clone(),
                "project" => paths.repo.join(".omh"),
                // The three that are going away, so a test can say what no
                // longer reaches a session.
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

    /// Absent is normal: a fresh catalogue declares most capabilities not at
    /// all, and an empty result has to mean that rather than a path invented on
    /// its behalf — the launcher mounts what this returns.
    #[test]
    fn absent_capabilities_are_skipped_not_faked() {
        let f = fixture(&[("catalogue", "rules/tdd.md", "only")]);
        let profile = Profile::resolve(&f.paths);
        assert_eq!(profile.sources(Capability::Rules).len(), 1);
        assert!(profile.sources(Capability::Skills).is_empty());
    }

    #[test]
    fn declared_reports_only_present_capabilities() {
        let f = fixture(&[
            ("catalogue", "rules/tdd.md", "r"),
            ("catalogue", "mcp.json", "{}"),
            ("catalogue", "skills/x/SKILL.md", "s"),
        ]);
        let declared = Profile::resolve(&f.paths).declared();
        assert_eq!(
            declared,
            vec![Capability::Rules, Capability::Skills, Capability::Mcp]
        );
    }

    /// Content lives in one place.
    ///
    /// Three layers with identical shapes meant "where is this skill" had three
    /// answers, and `sources` was a union — a later layer could shadow a
    /// same-named entry but nothing could turn one off, so "these are my twelve
    /// MCP servers, this project uses three" was unsayable.
    #[test]
    fn a_capability_resolves_to_one_catalogue_path() {
        let f = fixture(&[("catalogue", "skills/mine/SKILL.md", "yours")]);
        assert_eq!(
            Profile::resolve(&f.paths).sources(Capability::Skills),
            vec![f.paths.root.join("skills")]
        );
    }

    /// A project names entries from your catalogue; it cannot declare one.
    ///
    /// The committed layer is what made a repo able to hand you a skill, an MCP
    /// server or a command — content that arrives by `git clone` and runs
    /// against your work. What a repo still shares is its rules file, its hooks,
    /// its selection and its policy.
    #[test]
    fn a_repo_cannot_declare_content_of_its_own() {
        let f = fixture(&[
            ("shared", "skills/theirs/SKILL.md", "the repo's"),
            ("local", "skills/secret/SKILL.md", "yours, here"),
            ("shared", "mcp.json", "{}"),
        ]);
        let profile = Profile::resolve(&f.paths);
        assert!(profile.sources(Capability::Skills).is_empty());
        assert!(profile.sources(Capability::Mcp).is_empty());
    }

    /// Hooks are the one capability with a repo tier, because they are the one
    /// whose scope is genuinely the repo: `cargo test` here, `pnpm test` next
    /// door, one name and two bodies.
    ///
    /// The repo's come last, so a project overrides your personal `format` hook
    /// with the one this project actually needs, without renaming anything.
    #[test]
    fn hooks_resolve_to_the_catalogue_then_the_repo() {
        let f = fixture(&[
            ("catalogue", "hooks/format.json", "yours"),
            ("project", "hooks/format.json", "this repo's"),
        ]);
        assert_eq!(
            Profile::resolve(&f.paths).sources(Capability::Hooks),
            vec![f.paths.root.join("hooks"), f.paths.repo.join(".omh/hooks")],
            "project last, so project wins"
        );
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
        let a = Paths {
            root: dir.path().into(),
            repo: dir.path().join("alpha"),
        };
        let b = Paths {
            root: dir.path().into(),
            repo: dir.path().join("beta"),
        };
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
