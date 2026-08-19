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

    /// The stacks as shipped: what a project needs installed, and how the image
    /// gets it. Managed files, refreshed on every `init` like the adapters — a
    /// local edit that fixes one ecosystem leaves omh broken for everybody else
    /// using it, so the fix belongs upstream.
    pub fn stacks(&self) -> PathBuf {
        self.root.join("stacks")
    }

    /// Ecosystems **this repo** taught omh, for the one case a release never
    /// will: a proprietary internal toolchain. Written by `init` from an answer
    /// somebody typed, read beside the shipped ones, and unable to answer to a
    /// name omh ships.
    pub fn repo_stacks(&self) -> PathBuf {
        self.repo.join(".omh").join("stacks")
    }

    /// Files omh recognises as naming an ecosystem it cannot yet set up. A
    /// question, not an answer — see `stack::Marker`.
    pub fn markers(&self) -> PathBuf {
        self.root.join("markers")
    }

    /// The conventional hooks as shipped — `cargo test`, `gofmt -w .` — living
    /// in the catalogue beside the ones you write, because that is where their
    /// scope is: `cargo test` is what a rust project runs, not what *this* rust
    /// project runs.
    ///
    /// Managed like the stacks and the adapters, so a fix omh ships reaches
    /// somebody who ran `init` a year ago. A repo that needs its own spelling
    /// writes `<repo>/.omh/hooks/<name>.json`, which shadows this by the rule
    /// `merge_hooks` already applies.
    ///
    /// Deliberately the same directory `Capability::Hooks` sources, not a
    /// parallel one: a second place hooks can live is a second precedence rule
    /// to explain, and the shadowing already says what to do about a clash.
    pub fn hooks(&self) -> PathBuf {
        self.root.join(Capability::Hooks.source())
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

    /// Where the sandbox's own repositories live — one gitdir per session,
    /// plus the seed each was created from.
    ///
    /// A tree of its own rather than a sibling of the worktree it serves:
    /// `session::list` reports every directory under `worktrees()` as a
    /// session, so an `s01.git` beside `s01` would show up in `omh s ls` as a
    /// session you could resume.
    ///
    /// `next_id` would *not* be fooled — it parses the name after `s` as a
    /// number and `01.git` does not parse — but that is one enumerator getting
    /// lucky, not a reason to put them together.
    pub fn shadows(&self) -> PathBuf {
        self.root.join("shadow").join(self.repo_id())
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
    ///
    /// **Absent is not unreadable**, which `Path::exists()` cannot express: it
    /// answers `false` for a dangling symlink, an `EACCES` parent and an
    /// unmounted share alike. Read as "nothing declared" the launcher skips the
    /// capability, mounts nothing, reports nothing dropped and exits 0 — and
    /// `doctor` agrees, because it branches on the same empty list. With one
    /// catalogue that is every skill, rule, command, subagent and server at
    /// once, so `try_exists` and a `Result` rather than a silent `false`.
    pub fn sources(&self, cap: Capability) -> Result<Vec<PathBuf>> {
        let mut out = Vec::new();
        for path in self.candidates(cap) {
            if path
                .try_exists()
                .with_context(|| format!("reading {}", path.display()))?
            {
                out.push(path);
            }
        }
        Ok(out)
    }

    fn candidates(&self, cap: Capability) -> Vec<PathBuf> {
        let mut out = vec![self.root.join(cap.source())];
        if cap == Capability::Hooks {
            out.push(self.repo.join(".omh").join(cap.source()));
        }
        out
    }

    /// What this capability actually holds, by name.
    ///
    /// The names `[use]` selects from, `init` writes expanded, and the launcher
    /// reports as unselected — one function, because a name that is spelled one
    /// way by the report and another by the file that fixes it is worse than no
    /// report at all.
    ///
    /// `mcp.json` is the lone irregular case: a server is a record inside a file
    /// rather than a file in a directory, and it is read through the same parser
    /// the renderer uses rather than a second one that could disagree about what
    /// counts as a server.
    pub fn entries(&self, cap: Capability) -> Result<Vec<String>> {
        let mut out: Vec<String> = Vec::new();
        for source in self.sources(cap)? {
            if cap == Capability::Mcp {
                out.extend(crate::render::parse_layers(&[source])?.into_keys());
                continue;
            }
            let entries = std::fs::read_dir(&source)
                .with_context(|| format!("reading {}", source.display()))?;
            for entry in entries {
                // Not `.flatten()`, the rule this codebase follows for every
                // directory it lists: a `readdir` failing part-way through would
                // silently shorten the catalogue, and the report built from it
                // would say an entry is not selected because it was never seen.
                let entry = entry.with_context(|| format!("reading {}", source.display()))?;
                let name = entry_name(&entry.file_name());
                // A name omh mints has to be a name a `[use]` list can hold, or
                // `init` writes something every later read refuses. `.DS_Store`
                // is the one that actually happens: Finder creates it in any
                // directory somebody opens, and it bricked the repo.
                //
                // Skipped rather than reported, because it is not an entry that
                // went missing — it is a file that was never a catalogue entry,
                // and a launch that named it would be telling the user about
                // their operating system.
                if crate::selection::validate_entry_name(&name, cap, &source).is_err() {
                    continue;
                }
                out.push(name);
            }
        }
        out.sort();
        out.dedup();
        Ok(out)
    }

    /// Capabilities the profile actually carries.
    ///
    /// Not currently called outside tests: the launcher reports dropped
    /// capabilities from the adapter side instead. Kept because it is the
    /// profile-side half of that answer and `omh eject` will need it.
    #[allow(dead_code)]
    pub fn declared(&self) -> Result<Vec<Capability>> {
        let mut out = Vec::new();
        for cap in Capability::ALL {
            if !self.sources(cap)?.is_empty() {
                out.push(cap);
            }
        }
        Ok(out)
    }
}

/// The catalogue name of one directory entry.
///
/// A skill is a directory and a rule is a file, so the extension comes off and
/// nothing else does. One function rather than a `file_stem()` at each site,
/// because the name a `[use]` list matches, the name a launch reports as
/// unselected and the name `omh use` writes have to be the same string — three
/// spellings of "drop the extension" is three chances for a skill called
/// `review.diff` to be selectable under one name and reported under another.
pub fn entry_name(file_name: &std::ffi::OsStr) -> String {
    let path = Path::new(file_name);
    path.file_stem()
        .unwrap_or(file_name)
        .to_string_lossy()
        .into_owned()
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

    /// A catalogue omh cannot read is not a catalogue that declares nothing.
    ///
    /// `Path::exists()` answers `false` for *every* error — a dangling symlink
    /// into an unmounted volume, a parent directory created under `sudo`, a
    /// network share not up yet — so an unreadable catalogue resolved to "you
    /// declared none of this". The launcher then skips the capability, mounts
    /// nothing, adds nothing to `dropped`, and exits 0; `omh doctor` takes the
    /// same empty-sources branch and reports healthy. That is the closed loop
    /// `config::read_layer` was written about, and one catalogue makes it total
    /// rather than partial — before this there were three layers and one bad
    /// path degraded a third of the way.
    #[cfg(unix)]
    #[test]
    fn an_unreadable_catalogue_is_an_error_not_an_empty_one() {
        use std::os::unix::fs::PermissionsExt;
        let f = fixture(&[("catalogue", "skills/mine/SKILL.md", "s")]);
        // The parent unreadable, so `stat` on the child fails with EACCES
        // rather than ENOENT — a broken symlink or an absent mount reaches
        // `exists()` exactly the same way.
        std::fs::set_permissions(&f.paths.root, std::fs::Permissions::from_mode(0o000)).unwrap();

        let result = Profile::resolve(&f.paths).sources(Capability::Skills);
        std::fs::set_permissions(&f.paths.root, std::fs::Permissions::from_mode(0o755)).unwrap();

        let err = result.expect_err("unreadable must not read as undeclared");
        assert!(
            format!("{err:#}").contains("skills"),
            "must name the path: {err:#}"
        );
    }

    /// Absent is normal: a fresh catalogue declares most capabilities not at
    /// all, and an empty result has to mean that rather than a path invented on
    /// its behalf — the launcher mounts what this returns.
    #[test]
    fn absent_capabilities_are_skipped_not_faked() {
        let f = fixture(&[("catalogue", "rules/tdd.md", "only")]);
        let profile = Profile::resolve(&f.paths);
        assert_eq!(profile.sources(Capability::Rules).unwrap().len(), 1);
        assert!(profile.sources(Capability::Skills).unwrap().is_empty());
    }

    /// A name omh mints has to be a name `[use]` can hold.
    ///
    /// `entries` is where `init` and `omh use --all` get the names they write,
    /// and `Selection::read_list` refuses one that begins with a dot — so a
    /// `.DS_Store` in `~/.omh/skills`, which Finder creates in any directory
    /// somebody opens, was written into `[use]` and then refused by every
    /// command that read the file afterwards. `omh repo`, `omh use`, and the
    /// launch itself, all dead until the file was hand-edited.
    ///
    /// The rule this restores is `validate_entry_name`'s own: checked where a
    /// name is **minted**. This is the fourth mint point and the only one that
    /// was not on the list.
    #[test]
    fn a_name_omh_cannot_write_is_not_an_entry() {
        let f = fixture(&[
            ("catalogue", "skills/review-diff/SKILL.md", "s"),
            ("catalogue", "skills/.DS_Store", "junk"),
        ]);
        assert_eq!(
            Profile::resolve(&f.paths)
                .entries(Capability::Skills)
                .unwrap(),
            vec!["review-diff"],
            "a dotfile is not a skill, and naming it would poison the settings file"
        );
    }

    #[test]
    fn declared_reports_only_present_capabilities() {
        let f = fixture(&[
            ("catalogue", "rules/tdd.md", "r"),
            ("catalogue", "mcp.json", "{}"),
            ("catalogue", "skills/x/SKILL.md", "s"),
        ]);
        let declared = Profile::resolve(&f.paths).declared().unwrap();
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
            Profile::resolve(&f.paths)
                .sources(Capability::Skills)
                .unwrap(),
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
        assert!(profile.sources(Capability::Skills).unwrap().is_empty());
        assert!(profile.sources(Capability::Mcp).unwrap().is_empty());
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
            Profile::resolve(&f.paths)
                .sources(Capability::Hooks)
                .unwrap(),
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
