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

    // The agent's entire world. Never the host working tree.
    mounts.push(Mount {
        host: session.worktree.clone(),
        guest: crate::container_workdir().into(),
        read_only: false,
        file: false,
    });

    for cap in Capability::ALL {
        let sources = profile.sources(cap);
        if sources.is_empty() {
            continue;
        }
        let Some(binding) = adapter.supports(cap) else {
            dropped.push((cap, count_entries(&sources)));
            continue;
        };
        stage_capability(
            cap,
            binding,
            &sources,
            &stage,
            session,
            &mut mounts,
            staging,
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

    // The local note store, keyed by repo like the graph cache and for the
    // same reason: it must survive a container rebuild, a harness switch and
    // — unlike anything under /work — the removal of the session that wrote
    // it. Writable, because `remember` writes here.
    mounts.push(Mount {
        host: crate::memory::Layer::Local.dir(paths),
        guest: PathBuf::from(crate::memory::GUEST_LOCAL_NOTES),
        read_only: false,
        file: false,
    });

    // The memory server is `omh` itself, and the harness spawns MCP servers
    // inside the sandbox — so the base set's `command = "omh"` resolves to
    // nothing unless a binary is put there. Read-only: a program the agent
    // could rewrite is not a sandbox.
    //
    // Only when one exists. A bind mount of a missing host path makes docker
    // create a *directory*, and the failure then arrives as a permission error
    // about something nobody created.
    if let Some(bin) = opts.memory_bin.clone() {
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
        tty: opts.tty,
    })
}

fn stage_capability(
    cap: Capability,
    binding: &Binding,
    sources: &[PathBuf],
    stage: &Path,
    session: &Session,
    mounts: &mut Vec<Mount>,
    staging: Staging,
) -> Result<()> {
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

        // Rules ride along inside the worktree, so they need no mount of their
        // own — which is also why every harness's expected filename can point at
        // the same bytes.
        Render::Concat => {
            let merged = sources
                .iter()
                .map(|p| std::fs::read_to_string(p).unwrap_or_default())
                .collect::<Vec<_>>()
                .join("\n\n");
            for target in std::iter::once(&binding.path).chain(binding.also.iter()) {
                let rel = target
                    .strip_prefix("/work/")
                    .with_context(|| format!("`concat` target {target} must live under /work/"))?;
                if staging == Staging::Apply {
                    std::fs::write(session.worktree.join(rel), &merged)
                        .with_context(|| format!("staging {cap} at {rel}"))?;
                }
            }
        }

        // Everything else reshapes a merged canonical document.
        r => {
            let file = stage.join(format!("{cap}.rendered"));
            // Rendered even when skipped, so a dry run still surfaces a
            // malformed mcp.json instead of deferring it to launch.
            let rendered = crate::render::document(cap, r, sources)?;
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

    /// No longer branches on the host OS: which binary gets chosen is
    /// `deliver::plan_delivery`'s decision and is table-tested there. What is
    /// left here is what `plan` does with the answer.
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
    ///
    /// This ran on macOS only until the binary became an input. `plan` used to
    /// resolve it itself, and on Linux that resolves to the running
    /// executable — which exists by construction, so the state this guards
    /// could not be built and the assertion never held there.
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
    }

    #[test]
    fn rules_concatenate_into_every_declared_filename() {
        let fx = fixture();
        plan_for(&fx, "claude");
        for name in ["CLAUDE.md", "AGENTS.md"] {
            let body = std::fs::read_to_string(fx.session.worktree.join(name)).unwrap();
            assert_eq!(body, "personal rules\n\nshared rules", "{name}");
        }
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
            },
        )
        .unwrap();
        assert_eq!(p.argv, ["claude", "--resume", "abc"]);
    }

    /// Regression: `--dry-run` created a branch, a worktree, and wrote rules
    /// into it. A flag that exists to change nothing must change nothing.
    #[test]
    fn skipped_staging_writes_nothing() {
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
            },
        )
        .unwrap();

        assert!(!fx.paths.root.join("run").exists(), "no staging directory");
        for name in ["CLAUDE.md", "AGENTS.md"] {
            assert!(
                !fx.session.worktree.join(name).exists(),
                "{name} written during a dry run"
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
        };
        let p = plan(&fx.paths, &fx.profile, &adapter, &fx.session, &[], opts).unwrap();
        assert_eq!(p.argv, ["claude"]);
    }
}
