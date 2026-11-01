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
use crate::paths::Paths;
use anyhow::{Context, Result};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

pub struct Catalogue {
    /// The catalogue — yours, every project.
    root: PathBuf,
    /// This checkout, which declares hooks and nothing else.
    repo: PathBuf,
}

impl Catalogue {
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
                // omh's servers are part of omh, never of your catalogue.
                let omhs = crate::base::server_names()?;
                out.extend(
                    crate::render::parse_layers(&[source])?
                        .into_keys()
                        .filter(|name| !omhs.contains(name)),
                );
                continue;
            }
            out.extend(listed(&source, cap)?.into_iter().map(|(name, _)| name));
        }
        out.sort();
        out.dedup();
        Ok(out)
    }
}

/// Whether a file in `rules/` is a rule: a `.md`, in any case. The listing
/// and the composition both ask this, so what one offers the other composes —
/// `.MD` was listed and then skipped, the extension compared by case.
pub fn is_rule_file(path: &Path) -> bool {
    path.extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("md"))
}

/// One catalogue directory's entries, by name and by where each one is.
///
/// `entries`' reading of a directory, lifted out for the screen `omh settings`
/// draws, which has to open and remove what it lists — so a name it shows is
/// a name `omh info` shows, and the same files are skipped for the same reason.
pub fn listed(source: &Path, cap: Capability) -> Result<Vec<(String, PathBuf)>> {
    let mut out = Vec::new();
    let entries =
        std::fs::read_dir(source).with_context(|| format!("reading {}", source.display()))?;
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
        // A dotfile is skipped rather than reported, because it is not an
        // entry that went missing — it is a file that was never a catalogue
        // entry, and a launch that named it would be telling the user about
        // their operating system. Any other name the rule refuses — a line
        // break, a control character — is an entry somebody made, and every
        // launch refuses over it: skipped here, the listing never showed the
        // one thing stopping them.
        if entry.file_name().to_string_lossy().starts_with('.') {
            continue;
        }
        // A rule is a `.md` file, the one shape `rules` composes: a `.txt`
        // was listed and selectable and never composed, with nothing said.
        if cap == Capability::Rules && !is_rule_file(&entry.path()) {
            continue;
        }
        crate::selection::validate_entry_name(&name, cap, source)?;
        out.push((name, entry.path()));
    }
    out.sort();
    Ok(out)
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

pub(crate) fn applicable_hooks(
    names: Vec<String>,
    declared: &BTreeMap<String, Option<String>>,
    detected: &BTreeSet<String>,
) -> Vec<String> {
    names
        .into_iter()
        .filter(|n| match declared.get(n) {
            Some(Some(stack)) => detected.contains(stack),
            _ => true,
        })
        .collect()
}

/// The names a `[use]` list may hold for `cap`: what the catalogue and this
/// repo declare, minus omh's own, which `[omh]` governs and `[use]` refuses.
pub(crate) fn names(paths: &Paths, cap: Capability) -> Result<Vec<String>> {
    let names = names_unfiltered(paths, cap)?;
    if cap != Capability::Hooks {
        return Ok(names);
    }
    // Hooks alone can belong to an ecosystem, and omh now ships one set per
    // ecosystem. Offering a rust repo `go-test` would put every stack omh
    // knows into the list `init` writes and the launcher reports.
    let defs = crate::stack::load_all(&paths.stacks(), &paths.repo_stacks())?;
    let detected: BTreeSet<String> = crate::stack::detected(&defs, &paths.repo)
        .into_iter()
        .map(|d| d.name.clone())
        .collect();
    let catalogue = Catalogue::resolve(paths);
    let declared = crate::render::declared_stacks(&catalogue.sources(cap)?)?;
    Ok(applicable_hooks(names, &declared, &detected))
}

/// [`names`], before the hooks-only stack filter — every name the
/// catalogue holds for `cap`, whether or not this checkout's detected stacks
/// would offer it. `doctor::use_row` needs this half on its own: a `[use]`
/// name naming a hook a different stack owns is not the same fact as one
/// naming nothing in the catalogue at all, and reporting both as "not in the
/// catalogue" sends the reader to `omh info --repo`, which would show the
/// hook right there.
pub(crate) fn names_unfiltered(paths: &Paths, cap: Capability) -> Result<Vec<String>> {
    let manifest = crate::base::Manifest::load_dir(&paths.base())?;
    // Reserved, not merely owned: a name omh's server used to have is never
    // yours to list, and a list naming one is a list `[use]` refuses.
    let reserved = manifest.reserved();
    let catalogue = Catalogue::resolve(paths);
    Ok(catalogue
        .entries(cap)?
        .into_iter()
        .filter(|n| !reserved.get(&cap).is_some_and(|o| o.contains_key(n)))
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// **Skipped is for litter, not for an entry.** The listing dropped every
    /// name a `[use]` list cannot hold, which is right for `.DS_Store` and
    /// wrong for `lint<TAB>check.json`: the listing never showed it while every
    /// launch refused over it, and `omh doctor` called it broken. A dotfile is
    /// skipped; any other name the rule refuses is refused here too.
    #[test]
    fn a_listing_refuses_the_entry_a_launch_refuses_and_skips_only_litter() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(".DS_Store"), "").unwrap();
        std::fs::write(dir.path().join("fmt.json"), "{}").unwrap();
        let found = listed(dir.path(), Capability::Hooks).unwrap();
        assert_eq!(
            found.iter().map(|(n, _)| n.as_str()).collect::<Vec<_>>(),
            vec!["fmt"]
        );
        std::fs::write(dir.path().join("lint\tcheck.json"), "{}").unwrap();
        let err = format!(
            "{:#}",
            listed(dir.path(), Capability::Hooks)
                .expect_err("an entry a launch refuses was skipped")
        );
        assert!(err.contains("lint\\tcheck"), "{err}");
    }

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

        let result = Catalogue::resolve(&f.paths).sources(Capability::Skills);
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
        let catalogue = Catalogue::resolve(&f.paths);
        assert_eq!(catalogue.sources(Capability::Rules).unwrap().len(), 1);
        assert!(catalogue.sources(Capability::Skills).unwrap().is_empty());
    }

    /// A name omh mints has to be a name `[use]` can hold.
    ///
    /// `entries` is where `init` and `omh use --all` get the names they write,
    /// and `Selection::read_list` refuses one that begins with a dot — so a
    /// `.DS_Store` in `~/.omh/skills`, which Finder creates in any directory
    /// somebody opens, was written into `[use]` and then refused by every
    /// command that read the file afterwards. `omh info --repo`, `omh use`, and the
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
            Catalogue::resolve(&f.paths)
                .entries(Capability::Skills)
                .unwrap(),
            vec!["review-diff"],
            "a dotfile is not a skill, and naming it would poison the settings file"
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
            Catalogue::resolve(&f.paths)
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
        let catalogue = Catalogue::resolve(&f.paths);
        assert!(catalogue.sources(Capability::Skills).unwrap().is_empty());
        assert!(catalogue.sources(Capability::Mcp).unwrap().is_empty());
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
            Catalogue::resolve(&f.paths)
                .sources(Capability::Hooks)
                .unwrap(),
            vec![f.paths.root.join("hooks"), f.paths.repo.join(".omh/hooks")],
            "project last, so project wins"
        );
    }
}
