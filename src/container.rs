//! Launch plan: stage the resolved profile per capability, then bind-mount it
//! onto whatever paths the chosen harness happens to read.
//!
//! Docker cannot merge host directories onto one mount point, so layered
//! profiles are materialized into a per-launch staging directory first. Staging
//! writes to `~/.omh/run/`, never into the harness's real config location — the
//! harness only ever sees a read-only mount, so there is still nothing to drift
//! and nothing to clean up.

use crate::adapter::{expand, Adapter, Binding, Capability, Render};
use crate::profile::{Paths, Profile};
use crate::session::Session;
use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

#[derive(Debug)]
pub struct Plan {
    pub image: String,
    pub mounts: Vec<Mount>,
    pub env: Vec<(String, String)>,
    pub network: String,
    pub workdir: String,
    pub argv: Vec<String>,
    /// Capabilities the profile carries that this harness cannot express.
    pub dropped: Vec<(Capability, usize)>,
    /// What composing the project's rules turned up that the user should hear.
    pub rules: crate::rules::Report,
    /// Interactive harnesses need a terminal; a captured probe must not ask.
    pub tty: bool,
}

#[derive(Debug)]
pub struct Mount {
    pub host: PathBuf,
    pub guest: PathBuf,
    pub read_only: bool,
    /// A single file rather than a directory. Recorded at construction, never
    /// probed: under `Staging::Skip` the file does not exist yet, and a runtime
    /// capability check that changed answer between dry and real runs would be
    /// worse than no check.
    pub file: bool,
}

/// Home directory *inside* the container. Adapters template `$HOME` against it.
use crate::image::GUEST_HOME;

/// Where profile layer `i`'s copy of `cap` is mounted inside the container.
fn guest_layer(i: usize, cap: Capability) -> PathBuf {
    PathBuf::from(format!("/omh/layers/{i}/{}", cap.source()))
}

/// Whether `plan` may touch the filesystem. `--dry-run` must be able to show an
/// accurate plan without creating anything.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Staging {
    Apply,
    Skip,
}

/// Launch options that are not part of the profile.
#[derive(Debug, Clone)]
pub struct Options {
    pub staging: Staging,
    pub persist: crate::persist::Mode,
    pub tty: bool,
    /// Resolved credential account. `None` means no login is mounted.
    pub account_dir: Option<PathBuf>,
    /// Resolved memory-server binary. `None` means none is mounted, and that
    /// is the whole absent case.
    ///
    /// Resolved by the caller rather than probed here, for the reason the
    /// contributing doc gives: `plan()` is pure given a temp filesystem, and
    /// the one probe left in it could not be reached from a test. On Linux
    /// `deliver::available` returns the running executable, which exists by
    /// construction — so "the binary is missing" was unreachable there, and
    /// the guard against mounting a missing path only ran on macOS.
    pub memory_bin: Option<PathBuf>,
    /// The branch the project's own rules are read from when the worktree has
    /// none of its own. `None` asks git nothing.
    ///
    /// Resolved by the caller for the reason `memory_bin` is: `plan` stays pure
    /// given a temp filesystem, and a probe in here is a probe no test can
    /// reach. `Option` rather than an empty-string sentinel because
    /// `git show :AGENTS.md` is *valid* — `:path` is the index — so a sentinel
    /// that ever leaked past its guard would silently compose the staging area
    /// as the project's rules. `Option` makes that a compile error.
    pub base: Option<String>,
    /// What omh itself contributes — the hooks and rules sections the base
    /// manifest generates, with the features this repo switched off already
    /// removed.
    ///
    /// Resolved by the caller for the reason `base` and `memory_bin` are: the
    /// manifest lives on disk under `~/.omh/base` and reading it here would put
    /// a probe in a function whose purity is what makes it testable.
    pub omh: crate::base::Own,
}

pub fn plan(
    paths: &Paths,
    profile: &Profile,
    adapter: &Adapter,
    session: &Session,
    harness_args: &[String],
    opts: Options,
) -> Result<Plan> {
    let staging = opts.staging;
    let stage = paths.staging(&session.id, &adapter.name);
    let opts = &opts;
    let mut mounts = Vec::new();
    let mut dropped = Vec::new();

    // Composed before the capability loop because `place_destination` runs
    // inside it: it creates the empty placeholder at every declared name, and
    // composing afterwards would read omh's own file as the project's rules on
    // the very first launch.
    let (rules_doc, rules_report) = crate::rules::compose(
        paths,
        adapter,
        &session.worktree,
        opts.base.as_deref(),
        &opts.omh.sections,
    )?;

    // The agent's entire world. Never the host working tree.
    mounts.push(Mount {
        host: session.worktree.clone(),
        guest: crate::container_workdir().into(),
        read_only: false,
        file: false,
    });

    for cap in Capability::ALL {
        let sources = profile.sources(cap);
        // Every other capability is exactly what the profile carries. Rules are
        // not: the project's own `AGENTS.md` is composed in, and it is often the
        // only thing there — a fresh install has no rules layer of its own. So
        // asking the profile whether to stage threw the repo's conventions away
        // in the configuration a clone lands in, which is the bug this module
        // was written to fix.
        //
        // Hooks are the same story: omh's five are generated from the manifest
        // and belong to no layer at all, so a repo with no `hooks/` directory
        // still has them to stage.
        let carries_something = match cap {
            Capability::Rules => !rules_doc.trim().is_empty(),
            Capability::Hooks => !sources.is_empty() || !opts.omh.hooks.is_empty(),
            _ => !sources.is_empty(),
        };
        if !carries_something {
            continue;
        }
        let Some(binding) = adapter.supports(cap) else {
            // The composed document is one thing however many layers fed it, so
            // a rules-less harness drops at least the one it was handed — never
            // zero, which is what counting empty sources would have reported.
            // A harness with no hooks gives up omh's own as well as yours.
            let count = match cap {
                Capability::Rules => count_entries(&sources).max(1),
                // Layer files answering to a manifest name are never staged,
                // so counting them would report a harness giving up hooks it
                // was never going to run. An upgraded repo carries five.
                Capability::Hooks => {
                    count_named(&sources, |name| !opts.omh.reserved.contains(name))
                        + opts.omh.hooks.len()
                }
                _ => count_entries(&sources),
            };
            dropped.push((cap, count));
            continue;
        };
        stage_capability(
            cap,
            binding,
            &sources,
            &rules_doc,
            &opts.omh,
            Destination {
                stage: &stage,
                worktree: &session.worktree,
                staging,
            },
            &mut mounts,
        )?;
    }

    // The graph index, keyed by repo rather than harness — that is what lets
    // it survive a container rebuild and a switch from Claude Code to opencode.
    mounts.push(Mount {
        host: PathBuf::from(paths.cache_volume()),
        guest: PathBuf::from(crate::base::GRAPH_CACHE),
        read_only: false,
        file: false,
    });

    // A feature is all or nothing, and that has to reach the mounts rather
    // than stopping at the documents. With `memory` off the agent was still
    // given a writable store it is never told about and a server binary
    // nothing spawns — the half-configured state the design calls
    // unrepresentable.
    let memory_on = !opts
        .omh
        .disabled_servers
        .contains(crate::memory::tools::SERVER_KEY);

    // The local note store, keyed by repo like the graph cache and for the
    // same reason: it must survive a container rebuild, a harness switch and
    // — unlike anything under /work — the removal of the session that wrote
    // it. Writable, because `remember` writes here.
    if memory_on {
        mounts.push(Mount {
            host: crate::memory::Layer::Local.dir(paths),
            guest: PathBuf::from(crate::memory::GUEST_LOCAL_NOTES),
            read_only: false,
            file: false,
        });
    }

    // The memory server is `omh` itself, and the harness spawns MCP servers
    // inside the sandbox — so the base set's `command = "omh"` resolves to
    // nothing unless a binary is put there. Read-only: a program the agent
    // could rewrite is not a sandbox.
    //
    // Only when one exists. A bind mount of a missing host path makes docker
    // create a *directory*, and the failure then arrives as a permission error
    // about something nobody created.
    if let Some(bin) = opts.memory_bin.clone().filter(|_| memory_on) {
        mounts.push(Mount {
            host: bin,
            guest: PathBuf::from(crate::memory::deliver::GUEST_BIN),
            read_only: true,
            file: true,
        });
    }

    // Credentials mount at the paths the harness itself reads — anywhere else
    // and the session starts logged out no matter what was captured. Writable,
    // because OAuth tokens refresh in place.
    if let Some(account) = &opts.account_dir {
        for cred in crate::auth::mounts(adapter, account, GUEST_HOME) {
            mounts.push(Mount {
                host: cred.host,
                guest: cred.guest,
                read_only: false,
                file: cred.file,
            });
        }
    }

    Ok(Plan {
        image: crate::image::tag_for(adapter),
        mounts,
        env: vec![
            ("OMH_SESSION".into(), session.id.clone()),
            // Hooks run inside the sandbox and must name the project they
            // refresh; an env var keeps the hook file shared across sessions.
            (
                crate::base::PROJECT_ENV.into(),
                crate::base::project_name(&paths.repo_name(), &session.id),
            ),
        ],
        network: paths.network(),
        workdir: crate::container_workdir().into(),
        argv: crate::persist::wrap(
            opts.persist,
            &session.id,
            &adapter.name,
            std::iter::once(adapter.bin.clone())
                .chain(harness_args.iter().cloned())
                .collect(),
        ),
        dropped,
        rules: rules_report,
        tty: opts.tty,
    })
}

/// Where staging writes, and whether it writes at all.
///
/// The three genuinely travel together — every render arm needs all of them —
/// and bundling them is what keeps `stage_capability` under the argument count
/// clippy accepts. The composed document is deliberately *not* in here: it is an
/// input to one arm, not part of the destination, and folding it in turned a
/// type into a parameter bag.
#[derive(Clone, Copy)]
struct Destination<'a> {
    stage: &'a Path,
    worktree: &'a Path,
    staging: Staging,
}

fn stage_capability(
    cap: Capability,
    binding: &Binding,
    sources: &[PathBuf],
    rules_doc: &str,
    own: &crate::base::Own,
    to: Destination<'_>,
    mounts: &mut Vec<Mount>,
) -> Result<()> {
    let Destination {
        stage,
        worktree,
        staging,
    } = to;
    match binding.render {
        // Union layers by entry name; later layers shadow earlier ones. Links
        // point at each layer's *guest* mount path, so they are intentionally
        // dangling on the host and correct inside the container. Mounting rather
        // than copying keeps content live: edit a skill on the host and the
        // running agent sees it.
        Render::Dir => {
            let dst = stage.join(cap.source());
            if staging == Staging::Apply {
                std::fs::create_dir_all(&dst)?;
            }
            for (i, src) in sources.iter().enumerate() {
                mounts.push(Mount {
                    host: src.clone(),
                    guest: guest_layer(i, cap),
                    read_only: true,
                    file: false,
                });
                if staging == Staging::Skip {
                    continue;
                }
                let Ok(entries) = std::fs::read_dir(src) else {
                    continue;
                };
                for entry in entries.flatten() {
                    let link = dst.join(entry.file_name());
                    let _ = std::fs::remove_file(&link);
                    symlink(&guest_layer(i, cap).join(entry.file_name()), &link)?;
                }
            }
            mounts.push(Mount {
                host: dst,
                guest: expand(&binding.path, GUEST_HOME),
                read_only: true,
                file: false,
            });
        }

        // Mounted read-only at each declared filename, which is why every
        // harness's expected name can point at the same bytes.
        //
        // Written into the worktree instead, omh's staging was indistinguishable
        // from the agent's work: a repo that tracks its own `CLAUDE.md` saw a
        // permanent modification nobody made, and `s commit` carried omh's rules
        // into the user's PR on top of the project's own conventions. A mount
        // leaves the file on disk as the branch has it, so git has nothing to
        // report. Read-only for the reason every other staged capability is: a
        // file the agent can rewrite is not a profile, it is a suggestion.
        Render::Concat => {
            // `rules::compose` owns the join, because the document is more than
            // the layers: the project's own file is read from the worktree
            // before this mount hides it, and each section is labelled with
            // where it came from.
            let merged = rules_doc;
            let file = stage.join(format!("{cap}.md"));
            if staging == Staging::Apply {
                std::fs::create_dir_all(stage)?;
                std::fs::write(&file, merged).with_context(|| format!("staging {cap}"))?;
            }
            for target in std::iter::once(&binding.path).chain(binding.also.iter()) {
                // Still `/work`-relative: the guest path is inside the worktree
                // mount, and a `concat` target anywhere else would put the rules
                // somewhere the harness does not read and nothing would say so.
                let rel = target
                    .strip_prefix("/work/")
                    .with_context(|| format!("`concat` target {target} must live under /work/"))?;
                if staging == Staging::Apply {
                    place_destination(&worktree.join(rel))?;
                }
                mounts.push(Mount {
                    host: file.clone(),
                    guest: PathBuf::from(target),
                    read_only: true,
                    file: true,
                });
            }
        }

        // Everything else reshapes a merged canonical document.
        r => {
            let file = stage.join(format!("{cap}.rendered"));
            // Rendered even when skipped, so a dry run still surfaces a
            // malformed mcp.json instead of deferring it to launch.
            let rendered = crate::render::document(cap, r, sources, own)?;
            if staging == Staging::Apply {
                std::fs::create_dir_all(stage)?;
                std::fs::write(&file, rendered)?;
            }
            mounts.push(Mount {
                host: file,
                guest: expand(&binding.path, GUEST_HOME),
                read_only: true,
                file: true,
            });
        }
    }
    Ok(())
}

/// Put an empty file where a mount is about to land, if nothing is there yet.
///
/// A bind mount needs its destination to exist, and for destinations inside
/// `/work` the runtime will not supply one: `/work` is the host worktree, so
/// docker resolves `/work/CLAUDE.md` back to a host path and refuses to create
/// a mountpoint "outside of rootfs". It creates the file on the host anyway on
/// its way out, which is what made the failure look intermittent — the first
/// launch of a session died, and the second found the leftover and worked.
///
/// `create_new`, never a write: a branch that carries its own `CLAUDE.md` must
/// find it byte-for-byte intact. The mount hides that file for the length of the
/// session; it does not replace it. The placeholder is kept out of the agent's
/// `git status` by `carry::hide_staged_rules`, which runs before this.
fn place_destination(path: &Path) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    match std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
    {
        Ok(_) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => Ok(()),
        Err(e) => Err(e).with_context(|| format!("placing {}", path.display())),
    }
}

/// Entries a harness would have been given, counting only the ones that pass
/// `keep`. Names are matched without their extension, the way a hook's
/// manifest name is written.
fn count_named(sources: &[PathBuf], keep: impl Fn(&str) -> bool) -> usize {
    sources
        .iter()
        .filter_map(|p| std::fs::read_dir(p).ok())
        .flat_map(|entries| entries.flatten())
        .filter(|e| {
            let name = e.file_name();
            let name = std::path::Path::new(&name);
            keep(
                &name
                    .file_stem()
                    .unwrap_or(name.as_os_str())
                    .to_string_lossy(),
            )
        })
        .count()
}

/// How much a harness is giving up, for the one-line degradation warning.
fn count_entries(sources: &[PathBuf]) -> usize {
    sources
        .iter()
        .map(|p| {
            std::fs::read_dir(p)
                .map(|e| e.flatten().count())
                .unwrap_or(1)
        })
        .sum()
}

#[cfg(unix)]
fn symlink(src: &Path, dst: &Path) -> Result<()> {
    std::os::unix::fs::symlink(src, dst)
        .with_context(|| format!("linking {} -> {}", dst.display(), src.display()))
}

impl Plan {
    /// Refuse a plan the chosen backend cannot honour. Starting a sandbox where
    /// the profile silently is not there is the worst available outcome.
    pub fn validate(&self, caps: &crate::runtime::Caps) -> Result<()> {
        let mut problems = Vec::new();
        for m in &self.mounts {
            if !caps.file_mounts && m.file {
                problems.push(format!("{} is a single-file mount", m.guest.display()));
            }
            if !caps.free_guest_paths && m.guest != m.host {
                problems.push(format!(
                    "{} would have to mount at its host path {}",
                    m.guest.display(),
                    m.host.display()
                ));
            }
        }
        if problems.is_empty() {
            return Ok(());
        }
        anyhow::bail!(
            "the selected runtime cannot honour this plan:\n  {}",
            problems.join("\n  ")
        )
    }

    /// One line, once, naming what this harness cannot do.
    pub fn degradation(&self) -> Option<String> {
        if self.dropped.is_empty() {
            return None;
        }
        let parts: Vec<_> = self
            .dropped
            .iter()
            .map(|(cap, n)| format!("{n} {cap}"))
            .collect();
        Some(format!("dropped {} (unsupported)", parts.join(", ")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ADAPTERS: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/adapters");
    const BASE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/base");

    /// What omh contributes, from the manifest this repo ships. Every plan
    /// built here gets the real thing rather than an empty stand-in, because
    /// the hooks and rules sections are part of what a launch *is*.
    fn own() -> crate::base::Own {
        own_with(&Default::default())
    }

    /// Every server the manifest names is treated as installed unless a case
    /// is about removal: `own` switches a feature off when its server is gone
    /// from the profile, and a fixture that declared none would silently
    /// disable everything.
    fn own_with(off: &std::collections::BTreeSet<String>) -> crate::base::Own {
        let manifest = crate::base::Manifest::load_dir(Path::new(BASE)).unwrap();
        let installed = manifest.servers().into_keys().collect();
        crate::base::own(&manifest, off, &installed).unwrap()
    }

    struct Fx {
        _dir: tempfile::TempDir,
        paths: Paths,
        profile: Profile,
        session: Session,
    }

    /// Personal layer: skills(graphify), rules, hooks, subagents.
    /// Shared layer:   skills(shared, graphify-override), rules, mcp.
    fn fixture() -> Fx {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths {
            root: dir.path().join("home"),
            repo: dir.path().join("repo"),
        };
        let write = |p: PathBuf, body: &str| {
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, body).unwrap();
        };

        let personal = paths.root.join("profile");
        write(personal.join("AGENTS.md"), "personal rules");
        write(
            personal.join("skills/graphify/SKILL.md"),
            "personal graphify",
        );
        write(personal.join("subagents/explorer.md"), "explorer");
        write(
            personal.join("hooks/fmt.json"),
            r#"{"event":"Stop","command":"fmt"}"#,
        );

        let shared = paths.repo.join(".omh/profile");
        write(shared.join("AGENTS.md"), "shared rules");
        write(shared.join("skills/graphify/SKILL.md"), "shared graphify");
        write(shared.join("skills/only-shared/SKILL.md"), "only shared");
        write(
            shared.join("mcp.json"),
            r#"{"mcpServers":{"m":{"command":"m"}}}"#,
        );

        let session = Session::new(&paths.root.join("worktrees"), "s01".into());
        std::fs::create_dir_all(&session.worktree).unwrap();

        let profile = Profile::resolve(&paths);
        Fx {
            _dir: dir,
            paths,
            profile,
            session,
        }
    }

    fn plan_for(fx: &Fx, harness: &str) -> Plan {
        plan_with_memory_bin(fx, harness, None)
    }

    /// The memory binary is an input rather than something `plan` probes, so
    /// both its presence and its absence are reachable from a test on any
    /// platform. Before that it was resolved inside `plan`, and on Linux it
    /// resolved to the running executable — so the absent case could not be
    /// constructed there at all.
    fn plan_with_memory_bin(fx: &Fx, harness: &str, memory_bin: Option<PathBuf>) -> Plan {
        let adapter = Adapter::find(Path::new(ADAPTERS), harness).unwrap();
        plan(
            &fx.paths,
            &fx.profile,
            &adapter,
            &fx.session,
            &[],
            Options {
                staging: Staging::Apply,
                persist: crate::persist::Mode::None,
                tty: true,
                account_dir: None,
                memory_bin,
                base: None,
                omh: own(),
            },
        )
        .unwrap()
    }

    /// The security contract. The worktree is writable because that is the
    /// work; the graph cache because it is an index omh owns; credentials
    /// because OAuth tokens refresh in place and
    /// a read-only mount would discard every refreshed token. Nothing else is,
    /// and a stray `rw` beyond those two is the difference between a sandbox
    /// and a suggestion.
    #[test]
    fn nothing_beyond_the_worktree_and_credentials_is_writable() {
        let fx = fixture();
        let p = plan_for(&fx, "claude");
        let writable: Vec<_> = p.mounts.iter().filter(|m| !m.read_only).collect();
        let guests: Vec<String> = writable
            .iter()
            .map(|m| m.guest.display().to_string())
            .collect();
        assert_eq!(
            guests.len(),
            3,
            "worktree, graph cache and the note store only: {guests:?}"
        );
        assert!(guests.contains(&"/work".to_string()));
        assert!(guests.iter().any(|g| g == crate::base::GRAPH_CACHE));
        assert!(guests.iter().any(|g| g == crate::memory::GUEST_LOCAL_NOTES));
    }

    /// The local note store is the one thing outside the worktree the agent
    /// may write, and it is writable because `remember` writes there. It is
    /// mounted from `~/.omh` rather than the checkout, which is what keeps
    /// `host_working_tree_is_never_mounted` true — assert both together, so a
    /// future mount cannot satisfy one by breaking the other.
    #[test]
    fn the_note_store_is_mounted_from_omhs_own_directory_never_the_checkout() {
        let fx = fixture();
        let p = plan_for(&fx, "claude");
        let notes = p
            .mounts
            .iter()
            .find(|m| m.guest == Path::new(crate::memory::GUEST_LOCAL_NOTES))
            .expect("the local note store must reach the sandbox");

        assert!(!notes.read_only, "`remember` writes there");
        assert!(
            notes.host.starts_with(&fx.paths.root),
            "the store belongs to omh: {}",
            notes.host.display()
        );
        assert!(
            !notes.host.starts_with(&fx.paths.repo),
            "a store inside the checkout dies with the worktree: {}",
            notes.host.display()
        );
    }

    fn fake_server_binary(fx: &Fx) -> std::path::PathBuf {
        let arch = crate::memory::deliver::target_arch(std::env::consts::ARCH).unwrap();
        let at = crate::memory::deliver::cached_at(&fx.paths.root, arch);
        std::fs::create_dir_all(at.parent().unwrap()).unwrap();
        std::fs::write(&at, b"#!/bin/sh\n").unwrap();
        at
    }

    /// The base set declares `command = "omh"`, and the harness spawns MCP
    /// servers *inside* the container — so that resolves to nothing unless a
    /// binary is put there. Without this the server is configured, advertised
    /// by `omh why`, and silently absent.
    #[test]
    fn the_memory_server_binary_reaches_the_sandbox() {
        let fx = fixture();
        let host = fake_server_binary(&fx);
        let p = plan_with_memory_bin(&fx, "claude", Some(host.clone()));

        let mount = p
            .mounts
            .iter()
            .find(|m| m.guest == Path::new(crate::memory::deliver::GUEST_BIN))
            .expect("the server binary must be mounted");

        assert_eq!(mount.host, host);
        assert!(
            mount.read_only,
            "a program the agent could rewrite is not a sandbox"
        );
        assert!(mount.file, "one file, not the directory around it");
    }

    /// A bind mount of a host path that does not exist makes docker create a
    /// **directory** there, and the harness then reports a permission error
    /// about something nobody created. Absent is absent.
    #[test]
    fn a_missing_server_binary_is_left_out_rather_than_mounted_as_a_directory() {
        let fx = fixture();
        // deliberately absent
        let p = plan_with_memory_bin(&fx, "claude", None);
        assert!(
            !p.mounts
                .iter()
                .any(|m| m.guest == Path::new(crate::memory::deliver::GUEST_BIN)),
            "nothing is mounted where nothing exists"
        );
    }

    /// Two sessions on one repo share a store: it is memory for the repo, not
    /// for the session that happened to record it. Keyed the same way the
    /// graph cache is, and for the same reason.
    #[test]
    fn every_session_of_one_repo_sees_the_same_notes() {
        let fx = fixture();
        let host_of = |p: &Plan| {
            p.mounts
                .iter()
                .find(|m| m.guest == Path::new(crate::memory::GUEST_LOCAL_NOTES))
                .map(|m| m.host.clone())
                .expect("note store mount")
        };
        assert_eq!(
            host_of(&plan_for(&fx, "claude")),
            host_of(&plan_for(&fx, "opencode")),
            "a harness switch must not change which notes exist"
        );
    }

    fn plan_with_account(fx: &Fx, account: &std::path::Path) -> Plan {
        let adapter = Adapter::find(Path::new(ADAPTERS), "claude").unwrap();
        plan(
            &fx.paths,
            &fx.profile,
            &adapter,
            &fx.session,
            &[],
            Options {
                staging: Staging::Skip,
                persist: crate::persist::Mode::None,
                tty: true,
                account_dir: Some(account.to_path_buf()),
                memory_bin: None,
                base: None,
                omh: own(),
            },
        )
        .unwrap()
    }

    /// Regression: credentials mounted at `$HOME/.omh-creds`, which no harness
    /// reads — so every session started logged out no matter what was captured.
    #[test]
    fn credentials_mount_where_the_harness_actually_looks() {
        let fx = fixture();
        let p = plan_with_account(&fx, Path::new("/host/creds/claude/work"));
        let cred = p
            .mounts
            .iter()
            .find(|m| m.guest.ends_with(".claude"))
            .expect("credential mount");
        assert_eq!(cred.guest, Path::new("/home/agent/.claude"));
        assert!(cred.host.starts_with("/host/creds/claude/work"));
    }

    /// OAuth tokens are rewritten as they refresh. Read-only here means every
    /// session silently throws away its new token and re-authenticates.
    #[test]
    fn credentials_are_writable_so_refreshed_tokens_survive() {
        let fx = fixture();
        let p = plan_with_account(&fx, Path::new("/host/creds/claude/work"));
        let cred = p
            .mounts
            .iter()
            .find(|m| m.guest.to_string_lossy().ends_with(".claude.json"))
            .unwrap();
        assert!(!cred.read_only, "token refresh must persist");
        assert!(cred.file, "a single file, not a directory");
    }

    #[test]
    fn no_account_means_no_credential_mounts() {
        let fx = fixture();
        let p = plan_for(&fx, "claude");
        assert!(!p
            .mounts
            .iter()
            .any(|m| m.guest.to_string_lossy().ends_with(".claude.json")));
    }

    /// Keyed by repo, not by harness: a graph rebuilt on every switch would
    /// make the switch expensive and the index perpetually cold.
    #[test]
    fn the_graph_cache_is_shared_across_harnesses() {
        let fx = fixture();
        let for_claude = plan_for(&fx, "claude");
        let for_opencode = plan_for(&fx, "opencode");
        let cache = |p: &Plan| {
            p.mounts
                .iter()
                .find(|m| m.guest == Path::new(crate::base::GRAPH_CACHE))
                .map(|m| m.host.display().to_string())
                .expect("graph cache mount")
        };
        assert_eq!(cache(&for_claude), cache(&for_opencode));
    }

    #[test]
    fn host_working_tree_is_never_mounted() {
        let fx = fixture();
        let p = plan_for(&fx, "claude");
        for m in &p.mounts {
            assert!(
                !m.host.starts_with(fx.paths.repo.join("src")),
                "must not expose the host checkout: {}",
                m.host.display()
            );
        }
    }

    /// Regression: launching died before the harness started, with
    /// `create mountpoint for /work/AGENTS.md mount: mountpoint
    /// "/run/host_virtiofs/.../AGENTS.md" is outside of rootfs`.
    ///
    /// omh mounts its rules onto `/work/CLAUDE.md`, inside the worktree mount,
    /// and left creating that destination to the runtime. Docker Desktop will
    /// not: `/work` is the host worktree over virtiofs, so runc resolves the
    /// destination to a path outside the container's rootfs and refuses. It
    /// creates the empty file on the host on its way out, which is why the
    /// second launch of a session always worked and only the first one failed —
    /// the bug hid behind its own leftovers.
    ///
    /// So omh has to place the destination itself, before docker sees the plan.
    #[test]
    fn concat_destinations_exist_in_the_worktree_before_anything_mounts_onto_them() {
        let fx = fixture();
        let p = plan_for(&fx, "claude");

        let targets: Vec<_> = p
            .mounts
            .iter()
            .filter(|m| m.file && m.guest.starts_with("/work"))
            .collect();
        assert!(!targets.is_empty(), "claude stages rules into /work");

        for m in targets {
            let rel = m.guest.strip_prefix("/work").unwrap();
            let host = fx.session.worktree.join(rel);
            assert!(
                host.is_file(),
                "{} has nothing to mount onto: {} is missing",
                m.guest.display(),
                host.display()
            );
        }
    }

    /// The placeholder exists only so the mount has somewhere to land. A repo
    /// that keeps its own `CLAUDE.md` on the branch must find it untouched — the
    /// read-only mount hides it for the length of the session, and truncating it
    /// would show up in the user's diff as a deletion nobody made.
    #[test]
    fn a_repos_own_rules_file_survives_staging() {
        let fx = fixture();
        let own = fx.session.worktree.join("CLAUDE.md");
        std::fs::write(&own, "the project's own rules").unwrap();

        plan_for(&fx, "claude");

        assert_eq!(
            std::fs::read_to_string(&own).unwrap(),
            "the project's own rules"
        );
    }

    /// Read the document staged for the `rules` capability, as the agent gets it.
    fn composed_rules(p: &Plan) -> String {
        let mount = p
            .mounts
            .iter()
            .find(|m| m.guest == Path::new("/work/CLAUDE.md"))
            .expect("claude stages rules onto /work/CLAUDE.md");
        std::fs::read_to_string(&mount.host).unwrap()
    }

    /// Regression: the capability loop asked the *profile* whether there were
    /// rules, and skipped everything when the answer was no.
    ///
    /// `Profile::sources` only ever looks at the three layer directories, so a
    /// user with no `AGENTS.md` of their own — a fresh install, which is most
    /// of them — took the `sources.is_empty()` branch and never staged or
    /// mounted anything. The composed document existed and was thrown away, so
    /// the repo's own rules went nowhere: the exact bug this module was written
    /// to fix, surviving in the configuration a clone lands in.
    ///
    /// Worse than silent — `plan.rules` was still returned, so the launcher
    /// could report "composed CLAUDE.md" about a document nobody was given.
    #[test]
    fn the_project_alone_is_reason_enough_to_mount_rules() {
        let fx = fixture();
        for layer in ["profile", ".omh/profile", ".omh/local"] {
            let _ = std::fs::remove_file(fx.paths.root.join(layer).join("AGENTS.md"));
            let _ = std::fs::remove_file(fx.paths.repo.join(layer).join("AGENTS.md"));
        }
        std::fs::write(fx.session.worktree.join("AGENTS.md"), "ONLY THE PROJECT").unwrap();
        // `Profile` caches which layers exist, so re-resolve after removing.
        let fx = Fx {
            profile: Profile::resolve(&fx.paths),
            ..fx
        };

        let p = plan_for(&fx, "claude");

        assert!(
            composed_rules(&p).contains("ONLY THE PROJECT"),
            "a repo's own rules must reach the agent even when the profile has none"
        );
    }

    /// The bug: surviving on disk is not the same as reaching the agent.
    ///
    /// `a_repos_own_rules_file_survives_staging` proves the file is intact for
    /// the user's diff, and that was mistaken for the whole obligation. The
    /// read-only mount still hides it for the length of the session, so a repo
    /// that writes down its own conventions runs an agent that has never read
    /// them — omh replaced the project's rules with its own instead of adding to
    /// them.
    #[test]
    fn the_repos_own_rules_reach_the_agent() {
        let fx = fixture();
        std::fs::write(
            fx.session.worktree.join("AGENTS.md"),
            "always run cargo fmt before finishing",
        )
        .unwrap();

        let body = composed_rules(&plan_for(&fx, "claude"));

        assert!(
            body.contains("always run cargo fmt before finishing"),
            "the project's own rules must reach the agent, got:\n{body}"
        );
    }

    /// Every hook command the harness would actually run, read back out of the
    /// rendered document. Parsed rather than grepped: the commands are shell
    /// with quotes in them, and a substring check against JSON compares
    /// escaped text with unescaped and fails on hooks that are present.
    fn staged_hooks(p: &Plan) -> Vec<String> {
        let mount = p
            .mounts
            .iter()
            .find(|m| m.guest.ends_with(".claude/settings.json"))
            .expect("claude stages hooks into ~/.claude/settings.json");
        let body = std::fs::read_to_string(&mount.host).unwrap();
        let doc: serde_json::Value = serde_json::from_str(&body).unwrap();
        doc["hooks"]
            .as_object()
            .expect("an object keyed by event")
            .values()
            .flat_map(|matchers| matchers.as_array().unwrap())
            .flat_map(|m| m["hooks"].as_array().unwrap())
            .map(|h| h["command"].as_str().unwrap().to_string())
            .collect()
    }

    /// omh's own hooks are generated from the base manifest, so a profile with
    /// no `hooks/` directory anywhere still gets them.
    ///
    /// This is the configuration a fresh clone lands in, and the same shape as
    /// the bug that hid a repo's rules: asking the profile whether to stage
    /// answers "nothing here" and skips the whole capability, when what omh
    /// contributes does not come from the profile at all.
    #[test]
    fn omhs_hooks_reach_a_profile_with_no_hooks_layer() {
        let fx = fixture();
        std::fs::remove_dir_all(fx.paths.root.join("profile/hooks")).unwrap();
        // `Profile` caches which layers exist, so re-resolve after removing.
        let fx = Fx {
            profile: Profile::resolve(&fx.paths),
            ..fx
        };

        let staged = staged_hooks(&plan_for(&fx, "claude"));
        for hook in crate::base::hooks() {
            assert!(
                staged.contains(&hook.command),
                "{} must reach the harness with no hooks layer to read it: {staged:?}",
                hook.name
            );
        }
    }

    /// The other half of `omhs_hooks_reach_a_profile_with_no_hooks_layer`, and
    /// it was missing: nothing asserted omh's rules sections reach the agent
    /// through a plan at all.
    ///
    /// The only section assertion here was a negative one — that a disabled
    /// feature's section is absent — so handing `rules::compose` an empty
    /// slice left the whole suite green while every session lost the git
    /// notice, the note protocol and the graph orientation.
    #[test]
    fn omhs_sections_reach_the_agent_through_the_plan() {
        let fx = fixture();
        let composed = composed_rules(&plan_for(&fx, "claude"));
        for section in crate::base::sections() {
            assert!(
                composed.contains(section.body.trim_end()),
                "{} must reach the agent: {composed}",
                section.name
            );
        }
    }

    /// All or nothing has to reach the mounts, not stop at the document.
    ///
    /// With `memory = false` the server was dropped and the note rules with
    /// it, while the writable note store and the `omh` binary were still
    /// mounted — the agent given a store it is not told about and a server
    /// binary nothing spawns. That is exactly the half-configured state three
    /// doc comments call unrepresentable.
    #[test]
    fn a_disabled_feature_takes_its_mounts_too() {
        let fx = fixture();
        let bin = fake_server_binary(&fx);
        let adapter = Adapter::find(Path::new(ADAPTERS), "claude").unwrap();
        let p = plan(
            &fx.paths,
            &fx.profile,
            &adapter,
            &fx.session,
            &[],
            Options {
                staging: Staging::Apply,
                persist: crate::persist::Mode::None,
                tty: true,
                account_dir: None,
                memory_bin: Some(bin),
                base: None,
                omh: own_with(&["memory".to_string()].into()),
            },
        )
        .unwrap();

        let guests: Vec<String> = p
            .mounts
            .iter()
            .map(|m| m.guest.display().to_string())
            .collect();
        assert!(
            !guests.iter().any(|g| g == crate::memory::GUEST_LOCAL_NOTES),
            "no note store: {guests:?}"
        );
        assert!(
            !guests
                .iter()
                .any(|g| g == crate::memory::deliver::GUEST_BIN),
            "and no server binary: {guests:?}"
        );
    }

    /// A feature off in this repo takes its server, its hooks and its section
    /// of the rules together.
    ///
    /// All three or none: `codegraph` on with `graph-refresh` off is a graph
    /// that quietly stops tracking the code, which is the one combination that
    /// manufactures confident wrong answers. Nothing is uninstalled — the
    /// server is still in `mcp.json`, and the next repo gets it.
    #[test]
    fn a_disabled_feature_takes_its_server_its_hooks_and_its_rules() {
        let fx = fixture();
        std::fs::write(
            fx.paths.repo.join(".omh/profile/mcp.json"),
            r#"{"mcpServers":{"codegraph":{"command":"codebase-memory-mcp"}}}"#,
        )
        .unwrap();
        let own = own_with(&["codegraph".to_string()].into());

        let adapter = Adapter::find(Path::new(ADAPTERS), "claude").unwrap();
        let p = plan(
            &fx.paths,
            &fx.profile,
            &adapter,
            &fx.session,
            &[],
            Options {
                staging: Staging::Apply,
                persist: crate::persist::Mode::None,
                tty: true,
                account_dir: None,
                memory_bin: None,
                base: None,
                omh: own,
            },
        )
        .unwrap();

        let hooks = staged_hooks(&p);
        assert!(
            !hooks.iter().any(|c| c.contains("codebase-memory-mcp")),
            "no graph hooks: {hooks:?}"
        );
        let mcp = p
            .mounts
            .iter()
            .find(|m| m.guest.ends_with(".mcp.json"))
            .map(|m| std::fs::read_to_string(&m.host).unwrap())
            .expect("claude stages mcp");
        assert!(
            !mcp.contains("codegraph"),
            "the server is dropped from the document, not from your file: {mcp}"
        );
        assert!(
            fx.paths
                .repo
                .join(".omh/profile/mcp.json")
                .metadata()
                .is_ok_and(|m| m.len() > 0),
            "your mcp.json is left exactly as you have it"
        );
        assert!(
            !composed_rules(&p).contains("This repo is indexed as a graph"),
            "no graph section"
        );
    }

    /// Switching a feature off has to take the leftovers with it.
    ///
    /// Found by running `omh doctor` with `[omh] codegraph = false`, not by
    /// the suite: generation dropped the four graph hooks and the seeded files
    /// of the same name were still sitting in the profile, so the graph hooks
    /// went on firing against a server that had been removed from the
    /// document. Disabling that leaves the disabled thing running is worse
    /// than not offering it.
    ///
    /// The rule is that a manifest name is omh's, on or off. Nothing in a
    /// layer answers to one.
    #[test]
    fn switching_a_feature_off_silences_the_files_it_seeded() {
        let fx = fixture();
        for hook in crate::base::hooks() {
            std::fs::write(
                fx.paths
                    .root
                    .join("profile/hooks")
                    .join(format!("{}.json", hook.name)),
                format!(
                    r#"{{"event":"{}","matcher":"{}","command":"{}"}}"#,
                    hook.event, hook.matcher, hook.command
                ),
            )
            .unwrap();
        }
        let own = own_with(&["codegraph".to_string()].into());

        let adapter = Adapter::find(Path::new(ADAPTERS), "claude").unwrap();
        let p = plan(
            &fx.paths,
            &fx.profile,
            &adapter,
            &fx.session,
            &[],
            Options {
                staging: Staging::Apply,
                persist: crate::persist::Mode::None,
                tty: true,
                account_dir: None,
                memory_bin: None,
                base: None,
                omh: own,
            },
        )
        .unwrap();

        let hooks = staged_hooks(&p);
        assert!(
            !hooks
                .iter()
                .any(|c| c.contains("codebase-memory-mcp") || c.contains("OMH_GRAPH_PROJECT")),
            "no graph hook may survive as a seeded file: {hooks:?}"
        );
        assert!(
            hooks
                .iter()
                .any(|c| c.contains("fatal") || c.contains("git")),
            "git-notice is a different feature and stays on: {hooks:?}"
        );
    }

    /// A hook file carrying a manifest name is a leftover: `init` seeded these
    /// before they were generated, and every repo initialised then still has
    /// five of them. omh's own must win, or the fix it ships never arrives —
    /// `git-unavailable` was already rewritten once, and every profile written
    /// before that carries the version that misses the multi-line scripts
    /// agents most often emit.
    #[test]
    fn a_seeded_copy_no_longer_decides_what_runs() {
        let fx = fixture();
        std::fs::write(
            fx.paths.root.join("profile/hooks/graph-refresh.json"),
            r#"{"event":"Stop","command":"the version from an older omh"}"#,
        )
        .unwrap();

        let staged = staged_hooks(&plan_for(&fx, "claude"));
        let ships = crate::base::hooks()
            .into_iter()
            .find(|h| h.name == "graph-refresh")
            .expect("graph-refresh is in the base set");
        assert!(
            staged.contains(&ships.command),
            "omh's own hook must be what runs: {staged:?}"
        );
        assert!(
            !staged.iter().any(|c| c == "the version from an older omh"),
            "the leftover file must not decide: {staged:?}"
        );
    }

    /// `--dry-run` prints the plan and writes nothing, placeholders included.
    #[test]
    fn a_dry_run_leaves_no_placeholder_behind() {
        let fx = fixture();
        let adapter = Adapter::find(Path::new(ADAPTERS), "claude").unwrap();
        plan(
            &fx.paths,
            &fx.profile,
            &adapter,
            &fx.session,
            &[],
            Options {
                staging: Staging::Skip,
                persist: crate::persist::Mode::None,
                tty: true,
                account_dir: None,
                memory_bin: None,
                base: None,
                omh: own(),
            },
        )
        .unwrap();

        for name in crate::carry::STAGED_RULES {
            assert!(
                !fx.session.worktree.join(name).exists(),
                "a dry run created {name}"
            );
        }
    }

    /// Regression: staged links pointed at host paths, which do not exist inside
    /// the container, so every skill silently vanished.
    #[test]
    fn staged_links_target_guest_paths() {
        let fx = fixture();
        let p = plan_for(&fx, "claude");
        let staged = p
            .mounts
            .iter()
            .find(|m| m.guest.ends_with(".claude/skills"))
            .expect("skills mount");

        let mut names: Vec<String> = Vec::new();
        for entry in std::fs::read_dir(&staged.host).unwrap().flatten() {
            let target = std::fs::read_link(entry.path()).unwrap();
            assert!(
                target.starts_with("/omh/layers"),
                "link must resolve inside the container, got {}",
                target.display()
            );
            names.push(entry.file_name().to_string_lossy().into_owned());
        }
        names.sort();
        assert_eq!(
            names,
            ["graphify", "only-shared"],
            "layers union by entry name"
        );

        // Every layer a link can point into must actually be mounted.
        for i in 0..fx.profile.sources(Capability::Skills).len() {
            assert!(
                p.mounts
                    .iter()
                    .any(|m| m.guest == guest_layer(i, Capability::Skills)),
                "layer {i} skills must be mounted for its links to resolve"
            );
        }
    }

    /// Regression: staging was keyed by session only, so launching a second
    /// harness overwrote the MCP config the first one had mounted.
    #[test]
    fn harnesses_do_not_share_staging() {
        let fx = fixture();
        let claude = plan_for(&fx, "claude");
        let opencode = plan_for(&fx, "opencode");

        let mcp_of = |p: &Plan| {
            let m = p
                .mounts
                .iter()
                .find(|m| {
                    m.guest.to_string_lossy().contains("mcp") || m.guest.ends_with("opencode.json")
                })
                .expect("mcp mount");
            std::fs::read_to_string(&m.host).unwrap()
        };

        let c = mcp_of(&claude);
        let o = mcp_of(&opencode);
        assert!(c.contains("mcpServers"), "claude schema: {c}");
        assert!(o.contains("\"mcp\""), "opencode schema: {o}");
        assert!(
            !c.contains("opencode.ai"),
            "claude config must not be clobbered by opencode"
        );
    }

    #[test]
    fn unsupported_capabilities_are_reported_not_silently_dropped() {
        let fx = fixture();
        assert!(plan_for(&fx, "claude").degradation().is_none());

        let oc = plan_for(&fx, "opencode");
        let dropped: Vec<_> = oc.dropped.iter().map(|(c, _)| *c).collect();
        assert_eq!(dropped, vec![Capability::Subagents, Capability::Hooks]);
        let msg = oc.degradation().unwrap();
        assert!(
            msg.contains("subagents") && msg.contains("hooks"),
            "got: {msg}"
        );

        // What is given up includes omh's own, which come from the manifest
        // rather than from a layer. Counting only the profile's files would
        // report a harness dropping one hook while it drops six.
        let hooks = oc
            .dropped
            .iter()
            .find(|(c, _)| *c == Capability::Hooks)
            .map(|(_, n)| *n)
            .unwrap();
        assert_eq!(
            hooks,
            1 + crate::base::hooks().len(),
            "the fixture's own hook plus omh's: {msg}"
        );
    }

    /// An upgraded repo carries the five seeded files, none of which is ever
    /// staged. Counting them told a user they were giving up eleven hooks
    /// where they give up six — a wrong number presented as a measurement,
    /// which is the one thing this repo's own docs will not do.
    #[test]
    fn what_a_harness_gives_up_does_not_count_files_that_were_never_staged() {
        let fx = fixture();
        for hook in crate::base::hooks() {
            std::fs::write(
                fx.paths
                    .root
                    .join("profile/hooks")
                    .join(format!("{}.json", hook.name)),
                r#"{"event":"Stop","command":"seeded by an older omh"}"#,
            )
            .unwrap();
        }

        let oc = plan_for(&fx, "opencode");
        let hooks = oc
            .dropped
            .iter()
            .find(|(c, _)| *c == Capability::Hooks)
            .map(|(_, n)| *n)
            .unwrap();
        assert_eq!(
            hooks,
            1 + crate::base::hooks().len(),
            "the leftovers are inert and must not be counted"
        );
    }

    /// Every declared filename gets the same bytes, and gets them as a mount.
    ///
    /// Writing them into the worktree instead put omh's staging where git could
    /// see it: a repo that tracks its own `CLAUDE.md` — normal for one whose
    /// users run agent harnesses — showed a permanent modification nobody made,
    /// and `s commit` published omh's rules over the project's conventions. A
    /// mount leaves the file on disk exactly as the branch has it.
    ///
    /// What the worktree does get is an empty file to mount onto, because docker
    /// will not create one there — see `place_destination`. So the invariant is
    /// about the bytes, not the file's existence: omh's rules must never be
    /// what is on disk.
    /// "Both names, one document" is asserted on the mount's **host path**, not
    /// by reading the two files and comparing them. `Concat` stages one file and
    /// points every target at it, so comparing the bytes back was two reads of
    /// one path — an assertion no mutation could fail.
    #[test]
    fn rules_reach_every_declared_filename_without_writing_them_into_the_worktree() {
        let fx = fixture();
        let p = plan_for(&fx, "claude");

        let mut hosts = Vec::new();
        for name in ["CLAUDE.md", "AGENTS.md"] {
            let guest = PathBuf::from("/work").join(name);
            let m = p
                .mounts
                .iter()
                .find(|m| m.guest == guest)
                .unwrap_or_else(|| panic!("no mount for {name}: {:?}", p.mounts));
            assert!(m.file, "{name} is one file, not a directory");
            assert!(m.read_only, "a rules file the agent can rewrite is not one");
            hosts.push(m.host.clone());
            assert_eq!(
                std::fs::read_to_string(fx.session.worktree.join(name)).unwrap(),
                "",
                "{name} in the worktree must stay empty — the rules arrive by mount"
            );
        }
        assert_eq!(hosts[0], hosts[1], "both names, one staged document");
        assert!(
            !hosts[0].starts_with(&fx.session.worktree),
            "the document is staged outside the worktree, not in it: {}",
            hosts[0].display()
        );
        let body = std::fs::read_to_string(&hosts[0]).unwrap();
        assert!(
            body.contains("personal rules") && body.contains("shared rules"),
            "the layers must be in there: {body}"
        );
    }

    #[test]
    fn concat_outside_the_worktree_is_rejected() {
        let fx = fixture();
        let bad: Adapter = toml::from_str(
            r#"
            name = "bad"
            bin = "bad"
            install = "x"
            [capabilities.rules]
            path = "/etc/AGENTS.md"
            render = "concat"
            "#,
        )
        .unwrap();
        let err = plan(
            &fx.paths,
            &fx.profile,
            &bad,
            &fx.session,
            &[],
            Options {
                staging: Staging::Apply,
                persist: crate::persist::Mode::None,
                tty: true,
                account_dir: None,
                memory_bin: None,
                base: None,
                omh: own(),
            },
        )
        .unwrap_err();
        assert!(format!("{err:#}").contains("/work/"), "got: {err:#}");
    }

    #[test]
    fn docker_args_carry_the_plan_faithfully() {
        use crate::runtime::Runtime;
        let fx = fixture();
        let p = plan_for(&fx, "claude");
        let a = crate::runtime::Docker.args(&p);
        let joined = a.join(" ");

        assert!(joined.contains("--network omh-repo"));
        assert!(joined.contains("-w /work"));
        assert!(joined.contains("OMH_SESSION=s01"));
        assert_eq!(a.last().unwrap(), "claude", "harness argv comes last");
        assert_eq!(
            a.iter().filter(|s| *s == "-v").count(),
            p.mounts.len(),
            "every mount reaches the command line"
        );
        assert_eq!(
            joined.matches(":ro").count(),
            p.mounts.iter().filter(|m| m.read_only).count()
        );
    }

    #[test]
    fn harness_args_are_forwarded() {
        let fx = fixture();
        let adapter = Adapter::find(Path::new(ADAPTERS), "claude").unwrap();
        let args = ["--resume".to_string(), "abc".to_string()];
        let p = plan(
            &fx.paths,
            &fx.profile,
            &adapter,
            &fx.session,
            &args,
            Options {
                staging: Staging::Apply,
                persist: crate::persist::Mode::None,
                tty: true,
                account_dir: None,
                memory_bin: None,
                base: None,
                omh: own(),
            },
        )
        .unwrap();
        assert_eq!(p.argv, ["claude", "--resume", "abc"]);
    }

    /// Regression: `--dry-run` created a branch, a worktree, and wrote rules
    /// into it. A flag that exists to change nothing must change nothing.
    ///
    /// The rules moved from the worktree into the staging directory, so the
    /// worktree check that used to carry this test now passes for a reason
    /// unrelated to dry-run — it is the *staged* file that has to stay unwritten,
    /// while the mount describing it still appears in the plan. A dry run is
    /// only useful if what it prints is what would run.
    #[test]
    fn skipped_staging_writes_nothing() {
        let fx = fixture();
        let adapter = Adapter::find(Path::new(ADAPTERS), "claude").unwrap();
        let p = plan(
            &fx.paths,
            &fx.profile,
            &adapter,
            &fx.session,
            &[],
            Options {
                staging: Staging::Skip,
                persist: crate::persist::Mode::None,
                tty: true,
                account_dir: None,
                memory_bin: None,
                base: None,
                omh: own(),
            },
        )
        .unwrap();

        assert!(!fx.paths.root.join("run").exists(), "no staging directory");
        for name in ["CLAUDE.md", "AGENTS.md"] {
            let guest = PathBuf::from("/work").join(name);
            let m = p
                .mounts
                .iter()
                .find(|m| m.guest == guest)
                .unwrap_or_else(|| panic!("the plan must still describe {name}"));
            assert!(
                !m.host.exists(),
                "{name} staged during a dry run: {}",
                m.host.display()
            );
            assert!(
                !fx.session.worktree.join(name).exists(),
                "{name} written into the worktree during a dry run"
            );
        }
    }

    /// A dry run is only useful if the plan it prints is the plan that would run.
    #[test]
    fn skipped_staging_still_reports_the_real_mounts() {
        let fx = fixture();
        let adapter = Adapter::find(Path::new(ADAPTERS), "claude").unwrap();
        let dry = plan(
            &fx.paths,
            &fx.profile,
            &adapter,
            &fx.session,
            &[],
            Options {
                staging: Staging::Skip,
                persist: crate::persist::Mode::None,
                tty: true,
                account_dir: None,
                memory_bin: None,
                base: None,
                omh: own(),
            },
        )
        .unwrap();
        let wet = plan(
            &fx.paths,
            &fx.profile,
            &adapter,
            &fx.session,
            &[],
            Options {
                staging: Staging::Apply,
                persist: crate::persist::Mode::None,
                tty: true,
                account_dir: None,
                memory_bin: None,
                base: None,
                omh: own(),
            },
        )
        .unwrap();

        let paths_of = |p: &Plan| -> Vec<String> {
            p.mounts
                .iter()
                .map(|m| format!("{}:{}", m.host.display(), m.guest.display()))
                .collect()
        };
        assert_eq!(paths_of(&dry), paths_of(&wet));
        assert_eq!(dry.argv, wet.argv);
    }

    /// The launch path must carry persistence, or a closed lid still kills the
    /// agent no matter how long-lived the sandbox is.
    #[test]
    fn the_planned_command_survives_losing_the_terminal() {
        let fx = fixture();
        let adapter = Adapter::find(Path::new(ADAPTERS), "claude").unwrap();
        let opts = Options {
            staging: Staging::Skip,
            persist: crate::persist::Mode::Dtach,
            tty: true,
            account_dir: None,
            memory_bin: None,
            base: None,
            omh: own(),
        };
        let p = plan(&fx.paths, &fx.profile, &adapter, &fx.session, &[], opts).unwrap();

        assert_eq!(p.argv[0], "dtach");
        assert_eq!(p.argv.last().unwrap(), "claude");
    }

    #[test]
    fn persistence_can_be_turned_off() {
        let fx = fixture();
        let adapter = Adapter::find(Path::new(ADAPTERS), "claude").unwrap();
        let opts = Options {
            staging: Staging::Skip,
            persist: crate::persist::Mode::None,
            tty: true,
            account_dir: None,
            memory_bin: None,
            base: None,
            omh: own(),
        };
        let p = plan(&fx.paths, &fx.profile, &adapter, &fx.session, &[], opts).unwrap();
        assert_eq!(p.argv, ["claude"]);
    }
}
