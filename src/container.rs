//! Launch plan: stage the resolved profile per capability, then bind-mount it
//! onto whatever paths the chosen harness happens to read.
//!
//! Docker cannot merge host directories onto one mount point, so layered
//! profiles are materialized into a per-launch staging directory first. Staging
//! writes to `~/.omh/run/`, never into the harness's real config location — the
//! harness only ever sees a read-only mount, so there is still nothing to drift
//! and nothing to clean up.

use crate::adapter::{expand, Adapter, Capability, Render};
use crate::profile::{Paths, Profile};
use crate::session::Session;
use anyhow::{Context, Result};
use std::collections::BTreeMap;
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
    /// Hooks this harness *nearly* expressed — it has the capability but not
    /// the moment, tool or payload field they asked for. Separate from
    /// `dropped` because the granularity differs: a capability is given up
    /// whole, a hook individually, and reporting the second as the first would
    /// say a harness has no hooks when it has all but one.
    pub dropped_hooks: Vec<crate::hook::Dropped>,
    /// What composing the project's rules turned up that the user should hear.
    pub rules: crate::rules::Report,
    /// Interactive harnesses need a terminal; a captured probe must not ask.
    pub tty: bool,
}

/// What to do with the container already sitting under a session id.
#[derive(Debug)]
pub enum Reuse {
    /// It is the session being asked for. Exec into it.
    Attach,
    /// It is not, and replacing it loses nothing. The reasons are for the user.
    Restart(Vec<String>),
    /// It is not, but something is running inside that a restart would kill.
    Blocked {
        /// Harnesses still live in the session.
        live: Vec<String>,
        /// Why it needed replacing in the first place.
        changed: Vec<String>,
    },
}

/// Whether the container under a session id is the session being asked for.
///
/// Both failures this answers were live bugs, and they are one question: a
/// container is a plan materialized, and omh used to hand one back knowing only
/// that it was *running*. It execed into containers whose worktree had been
/// deleted, and into containers built for a different harness entirely.
///
/// Restarting is the remedy for both, because there is no lesser one — no
/// `exec` adds a mount or changes an image — and it is nearly free: the worktree
/// and branch live on the host and the graph in a volume. Nearly, not entirely.
/// It also kills whatever is running inside, so a session with a live harness is
/// reported rather than replaced. That is the same refusal-to-guess `idle`
/// makes: the cost of being wrong is an agent stopped mid-task.
/// What to do with a running container, given what the two probes said.
///
/// The decision, split from the two subprocesses that feed it, so that all of
/// it is a table. Everything destructive in a launch is downstream of this
/// function and none of it was reachable from a test: every launch case in
/// `tests/cli.rs` passes `--dry-run`, which returns before any of this, and
/// there is no fake runtime in the tree. Three reviewers asked for the same
/// thing and they were right — the guard belongs on the decision, not on the
/// two parsers below it.
///
/// The mutation this exists to kill: point `Probe::Unknown` at
/// `not_enterable()` instead of the refusal, and every test in the tree stayed
/// green while `docker rm -f` came back for a container omh could not read.
///
/// `stamped` is a closure because reading the stamp is a second subprocess and
/// only one branch needs it — and because a test then supplies it without a
/// runtime, which is the whole point.
pub fn decide(
    id: &str,
    probed: crate::image::Probe,
    stamped: impl FnOnce() -> crate::image::Stamp,
    plan: &Plan,
) -> Result<Reuse> {
    use crate::image::{Probe, Stamp};
    let listing = match probed {
        Probe::Listed(listing) => listing,
        // Both mean *replace it*, for opposite reasons: one cannot be entered
        // ever again, the other is not there to enter. Neither has anything
        // alive inside to lose, which is the only question that matters before
        // an `rm -f`.
        //
        // `Gone` was folded into the refusal below at first. That turned the
        // most ordinary race in the launch path — the sandbox's process
        // exiting between *is it running* and *can it be entered* — from
        // something that healed itself into a command the user ran twice.
        Probe::NotEnterable | Probe::Gone => return Ok(not_enterable()),
        // Refused rather than guessed, and this is the whole point of the
        // type. `Restart` means `rm -f` on a container confirmed running
        // earlier in the launch, and *the daemon blinked* is not a reason to
        // destroy somebody's turn. Attaching on the guess is no better — if
        // the container really is the broken kind, every command in it fails
        // the same way — but the user can tell the two apart from the message.
        Probe::Unknown(why) => anyhow::bail!(
            "omh could not tell whether {id}'s sandbox is still usable, so it will neither \
             attach to it nor replace it: {why}\n  \
             omh {id} claude        try again — a runtime that has come back will answer\n  \
             omh {id} down          stop it, and the next launch builds a fresh one"
        ),
    };
    // Refused for the same reason: `drift` reads an unreadable stamp as
    // *nothing about it can be verified*, which is a `Restart`, which is
    // `rm -f` on the container the probe just confirmed was alive and
    // enterable — with a reason omh invented.
    let stamp = match stamped() {
        Stamp::Read(stamp) => stamp,
        Stamp::Unknown(why) => anyhow::bail!(
            "omh could not read what {id}'s sandbox was built from, so it will neither \
             attach to it nor replace it: {why}\n  \
             omh {id} claude        try again — a runtime that has come back will answer\n  \
             omh {id} down          stop it, and the next launch builds a fresh one"
        ),
    };
    Ok(reuse(&stamp, plan, &crate::persist::live(id, &listing)))
}

/// A container that cannot be entered, or is not there to enter.
///
/// Its own constructor rather than a `false` first argument to `reuse`, which
/// made three of that function's four parameters meaningless and the state
/// `reuse(false, real_stamp, plan, &["claude"])` — a live harness discarded
/// and the container restarted anyway — something a caller could spell.
///
/// The wording lives beside the variant it fills rather than two files from
/// it, which is where the caller had to keep it.
pub fn not_enterable() -> Reuse {
    Reuse::Restart(vec!["it can no longer reach its worktree".into()])
}

pub fn reuse(
    stamp: &std::collections::BTreeMap<String, String>,
    plan: &Plan,
    live: &[String],
) -> Reuse {
    let changed = drift(&plan.labels(), stamp);
    if changed.is_empty() {
        return Reuse::Attach;
    }
    if !live.is_empty() {
        return Reuse::Blocked {
            live: live.to_vec(),
            changed,
        };
    }
    Reuse::Restart(changed)
}

/// What a running container no longer matches about the plan being asked for,
/// phrased for someone to act on rather than as a digest comparison.
///
/// Empty means it can be reused as-is. Anything else and it has to be replaced:
/// no `exec` adds a mount or changes an image, so there is no lesser remedy.
pub fn drift(
    expected: &[(String, String)],
    actual: &std::collections::BTreeMap<String, String>,
) -> Vec<String> {
    // A container omh started before it stamped anything. Reporting five
    // separate missing facts would bury the one thing worth saying.
    if actual.is_empty() {
        return vec!["it predates this check, so nothing about it can be verified".into()];
    }
    let mut out = Vec::new();
    for (key, want) in expected {
        let name = key.strip_prefix("omh.").unwrap_or(key);
        let Some(have) = actual.get(key) else {
            out.push(format!("{name} (not recorded)"));
            continue;
        };
        if have == want {
            continue;
        }
        // A list is reported as a count of what moved; printing fourteen mount
        // lines twice is not a message anybody reads.
        out.push(if want.contains('\n') || have.contains('\n') {
            let want_lines: std::collections::BTreeSet<&str> = want.lines().collect();
            let have_lines: std::collections::BTreeSet<&str> = have.lines().collect();
            format!(
                "{name} ({} added, {} removed)",
                want_lines.difference(&have_lines).count(),
                have_lines.difference(&want_lines).count()
            )
        } else {
            format!("{name} ({have} → {want})")
        });
    }
    out
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
    /// What this checkout decided — features off here, servers dropped with
    /// them, per-repo MCP environment.
    ///
    /// Beside `omh` rather than inside it. Both arrive resolved from outside for
    /// the same reason, which is what made one field tempting, but they answer
    /// opposite questions and `omh why` exists to keep those apart.
    pub repo: crate::settings::RepoPolicy,
    /// The image this session runs — the harness layer, or the stack layer
    /// built on top of it when this repo provisions anything.
    ///
    /// Resolved by the caller for the reason `omh`, `base` and `memory_bin`
    /// are, and for one more that is specific to it: deciding here would mean
    /// reading `[provision]` and the stack files from inside `plan`, so the
    /// choice of image would be reachable from no test — and *which image a
    /// session runs* is the single fact this whole design turns on. It was a
    /// hardcoded `image::tag_for(adapter)` for the whole of the first
    /// milestone, which built a stack layer nothing ever ran.
    pub image: String,
    /// What that image has been measured to contain, `{program: resolves}`.
    ///
    /// Read from `facts::Facts::about`, keyed on `image` above — so the two
    /// fields are answers about the same thing and a mismatch is impossible to
    /// spell here. A program absent from the map is one nobody probed, which
    /// suppresses nothing.
    pub resolves: BTreeMap<String, bool>,
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
    let mut dropped_hooks = Vec::new();

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
        &opts.repo.selection,
    )?;

    // The agent's entire world. Never the host working tree.
    mounts.push(Mount {
        host: session.worktree.clone(),
        guest: crate::container_workdir().into(),
        read_only: false,
        file: false,
    });

    // Built once, which is what the type is for. Constructed inside the loop it
    // rebuilt all seven values six times and bought nothing its own doc claimed.
    let stager = Stager {
        adapter,
        rules_doc: &rules_doc,
        own: &opts.omh,
        repo: &opts.repo,
        resolves: &opts.resolves,
        stage: &stage,
        worktree: &session.worktree,
        staging,
    };

    for cap in Capability::ALL {
        let sources = profile.sources(cap)?;
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
            // And a `claude-settings` document is no longer only hooks: it also
            // carries the approval without which the MCP document omh mounts is
            // listed and never loaded. Skipping it when a repo has turned every
            // hook off would turn that repo's MCP servers off with them, at a
            // distance, through a file neither setting mentions.
            Capability::Hooks => {
                !sources.is_empty()
                    || !opts.omh.hooks.is_empty()
                    || adapter
                        .supports(cap)
                        .is_some_and(|b| b.render == Render::ClaudeSettings)
            }
            _ => !sources.is_empty(),
        };
        if !carries_something {
            continue;
        }
        if adapter.supports(cap).is_none() {
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
        }
        dropped_hooks.extend(stager.stage(cap, &sources, &mut mounts)?);
    }

    // What `carry_in` names, mounted rather than copied.
    //
    // `carry::apply` has already put a copy in the worktree and told the user
    // what it did; this stages a second copy outside `/work` and mounts it over
    // the first. The mount is what the agent meets, and it is what survives
    // `git clean -fdx` — an untracked file is removed by `-x` whether or not
    // `info/exclude` names it, and a mountpoint cannot be unlinked.
    //
    // Read-write, because a carried file is config the agent may legitimately
    // edit, and from a staged copy so those edits cannot reach the checkout.
    //
    // Before the shadow block for consistency with the "derive the exclude list
    // from the mounts" rule that block states, and for nothing else. The
    // ordering is not load-bearing and this used to claim it was: the shadow
    // seeds `excluded` from the `carry_in` policy *first* and only then extends
    // it from the mounts, so a carried path is excluded either way — it just
    // ends up in the list twice. What the mount-derived entry does add is the
    // normalised form, so a pattern written `./.env` or `certs/` reaches git as
    // something it can match.
    // `branch.is_some()` as well as `Apply`, and the first is the one that
    // matters. `omh doctor` and `omh auth` borrow a writable `/work` for one
    // command through `Session::scratch`, and neither calls `carry_in` — that
    // runs in `run` and `attach` only. Keyed on `Apply` alone this staged a
    // plaintext copy of the user's `.env` and mounted it read-write into the
    // doctor probe and into the OAuth container, with nothing printed, because
    // `carry_in`'s whole reporting surface lives in a launcher neither reaches.
    // `omh doctor --dry-run` wrote the copy to disk on its way past.
    //
    // The same test the shadow block makes below, for a version of the same
    // reason: a session with no branch is not doing work, so it needs neither a
    // repository of its own nor anything of the user's to do it with.
    // Decided for every plan, written only for a real one. A dry run has to
    // name the mounts a launch would make — `skipped_staging_still_reports_the_
    // real_mounts` says so and went red when this staged and named in one step,
    // leaving `--dry-run` printing a plan missing the mount that holds the
    // user's secret. Deciding is reads of the checkout and is safe either way.
    let carried = if session.branch.is_some() {
        crate::carry::to_mount(
            &paths.repo,
            &stage.join("carried"),
            &crate::config::policy_list(paths, "carry_in"),
        )?
    } else {
        Vec::new()
    };
    if staging == Staging::Apply {
        crate::carry::materialise(&paths.repo, &carried)?;
    }
    for item in &carried {
        // A bind mount needs its destination to exist, and docker creates a
        // *directory* when it does not — the same reason the rules placeholders
        // are placed.
        //
        // A backstop with no reachable case today: `run` and `attach` both call
        // `carry_in` before this, so `apply` has always left the real file here,
        // and the `branch.is_some()` test above is what keeps the paths that do
        // not out. Kept for the reason `Session::commit` keeps its twin — the
        // guarantee is about what is on disk when docker looks, and that is not
        // this function's to assume.
        if staging == Staging::Apply {
            place_destination(&session.worktree.join(&item.rel))?;
        }
        mounts.push(Mount {
            host: item.host.clone(),
            guest: PathBuf::from(crate::container_workdir()).join(&item.rel),
            read_only: false,
            file: true,
        });
    }

    // The repository the sandbox is allowed to have.
    //
    // Seeded *after* the capability loop, not before, and that ordering is the
    // whole correctness of it. omh mounts its own documents over paths inside
    // `/work` — `.mcp.json` among them, which this repo tracks — and a shadow
    // seeded earlier captures the project's real file, then has omh's rendered
    // one mounted on top. The agent then opens on `M .mcp.json`, a change it
    // did not make; `git reset --hard` dies on `unable to unlink old
    // '.mcp.json': Resource busy`, because a bind mount cannot be unlinked, so
    // the one recovery this feature exists to give is the one thing that fails;
    // and `git add -A` reads *through* the mount, committing omh's rendered
    // document — credentials included — into the history a harvest replays.
    //
    // So the exclude list is built from the mounts themselves rather than
    // guessed at: whatever omh put inside `/work`, the sandbox's repository
    // does not track. Deriving it from `mounts` means a capability added later
    // is covered without anyone remembering to come back here.
    //
    // Only for a session that has a branch: a scratch session is `omh auth` or
    // `omh doctor` borrowing a writable `/work` for one command, with no work
    // to check point and nothing that could ever be harvested from it.
    if session.branch.is_some() {
        let shadow = crate::shadow::Shadow::new(&paths.shadows(), &session.id);

        let workdir = crate::container_workdir();
        let mut excluded = crate::config::policy_list(paths, "carry_in");
        excluded.extend(mounts.iter().filter_map(|m| {
            m.guest
                .strip_prefix(workdir)
                .ok()
                .map(|rel| rel.display().to_string())
                .filter(|rel| !rel.is_empty())
        }));

        // Both writes are `Apply`-only, for the reason the mode exists: a
        // `--dry-run` reports the plan a launch *would* carry out, and creating
        // a repository is not reporting. The mounts are named either way, so
        // what a dry run prints is still the truth about the launch.
        if staging == Staging::Apply {
            shadow.ensure(&session.worktree, &excluded)?;
        }

        mounts.push(Mount {
            host: shadow.gitdir.clone(),
            guest: crate::shadow::GUEST_GITDIR.into(),
            read_only: false,
            file: false,
        });

        // Staged rather than written into the worktree: `/work` is the user's
        // branch, and a `.git` file omh authored there would be a file the
        // worktree's own git has to explain. Here it exists only as something
        // to mount, and the mount is what the container sees.
        let pointer = stage.join("shadow-gitdir");
        if staging == Staging::Apply {
            if let Some(dir) = pointer.parent() {
                std::fs::create_dir_all(dir)?;
            }
            std::fs::write(&pointer, crate::shadow::pointer_file())?;
        }
        mounts.push(Mount {
            host: pointer,
            guest: format!("{workdir}/.git").into(),
            read_only: true,
            file: true,
        });

        // The push hook, mounted read-only over the copy `ensure` wrote.
        //
        // The gitdir has to be writable — the agent commits into it — so
        // everything in it is the agent's to delete, including the one hook
        // that speaks up when it reaches for a remote. Mounted, it cannot be
        // removed, rewritten or chmod-ed away. It can still be *bypassed*, by
        // `--no-verify` and by `core.hooksPath`; see `shadow::GUEST_PRE_PUSH`
        // for what that does and does not buy.
        let hook = stage.join("shadow-pre-push");
        if staging == Staging::Apply {
            std::fs::write(&hook, crate::shadow::pre_push_hook())?;
        }
        mounts.push(Mount {
            host: hook,
            guest: crate::shadow::GUEST_PRE_PUSH.into(),
            read_only: true,
            file: true,
        });

        // The config, mounted read-only over the copy `ensure` just wrote.
        //
        // Staged as a *copy of what is on disk* rather than composed here: git
        // records `repositoryformatversion` and `filemode` when it creates the
        // repository, and a file assembled from omh's idea of the keys would be
        // a repository git reads differently from the one it made. `ensure` has
        // already put it in the state omh wants; this carries those bytes in.
        let config = stage.join("shadow-config");
        if staging == Staging::Apply {
            std::fs::copy(shadow.gitdir.join("config"), &config)
                .with_context(|| format!("staging the sandbox's config for {}", session.id))?;
        }
        mounts.push(Mount {
            host: config,
            guest: crate::shadow::GUEST_CONFIG.into(),
            read_only: true,
            file: true,
        });
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
        .repo
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
        image: opts.image.clone(),
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
        dropped_hooks,
        rules: rules_report,
        tty: opts.tty,
    })
}

/// What staging needs that does not change from one capability to the next.
///
/// Seven values, invariant across the six-capability loop, built once before it
/// — which is what a struct with a method is for, and what this was not doing:
/// the literal was inside the loop, so every field was rebuilt six times and
/// the type bought nothing its doc claimed.
///
/// `Destination` used to hold the last three, on the argument that they keep
/// `stage_capability` under clippy's argument count. That function is this
/// method now, so the wrapper had one user and one reason, both gone. The part
/// of its comment worth keeping: the composed document is an input to one arm
/// rather than part of the destination, which is why `rules_doc` sits here as
/// itself and is not folded into anything.
struct Stager<'a> {
    adapter: &'a Adapter,
    rules_doc: &'a str,
    own: &'a crate::base::Own,
    repo: &'a crate::settings::RepoPolicy,
    /// What the session's image has been measured to contain. Beside `repo`
    /// rather than inside it: one is what this checkout decided, the other is
    /// what a container turned out to hold, and `omh why` exists to keep those
    /// apart.
    resolves: &'a BTreeMap<String, bool>,
    stage: &'a Path,
    worktree: &'a Path,
    staging: Staging,
}

impl Stager<'_> {
    fn stage(
        &self,
        cap: Capability,
        sources: &[PathBuf],
        mounts: &mut Vec<Mount>,
    ) -> Result<Vec<crate::hook::Dropped>> {
        let Stager {
            adapter,
            rules_doc,
            own,
            repo,
            resolves,
            stage,
            worktree,
            staging,
        } = *self;
        let mut dropped_hooks = Vec::new();
        // Looked up here rather than threaded in, so the tool vocabulary — which
        // lives on the adapter, not the binding — arrives with the adapter it
        // belongs to. `plan` has already established the capability is
        // supported; the error path exists because "the caller checked" is not
        // something the type system carries.
        let binding = adapter
            .supports(cap)
            .with_context(|| format!("{} declares no `{cap}` capability", adapter.name))?;
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
                    prune(&dst, cap, repo)?;
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
                        // The selection decides what the harness is *offered*.
                        // The layer behind these links is still mounted whole,
                        // so an unselected skill is not loaded but stays
                        // readable at a path an agent could go looking for —
                        // curation, not confinement. The boundary that matters
                        // is that `~/.omh` is mounted read-only and never
                        // writable; per-entry mounts would close this one and
                        // multiply the mount count by the size of the
                        // catalogue, for a threat omh does not claim to stop.
                        if !repo
                            .selection
                            .allows(cap, &crate::profile::entry_name(&entry.file_name()))
                        {
                            continue;
                        }
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
                    let rel = target.strip_prefix("/work/").with_context(|| {
                        format!("`concat` target {target} must live under /work/")
                    })?;
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
            _ => {
                let file = stage.join(format!("{cap}.rendered"));
                // Rendered even when skipped, so a dry run still surfaces a
                // malformed mcp.json instead of deferring it to launch.
                let rendered = crate::render::document(
                    cap,
                    binding,
                    sources,
                    own,
                    repo,
                    &adapter.tools,
                    resolves,
                )?;
                dropped_hooks.extend(rendered.dropped);
                if staging == Staging::Apply {
                    std::fs::create_dir_all(stage)?;
                    std::fs::write(&file, rendered.body)?;
                }
                let guest = expand(&binding.path, GUEST_HOME);
                // A target inside the worktree needs the mountpoint to exist
                // first, exactly as `concat` does. Everything above `/work` is
                // omh's own image, where the mountpoint is built in; `/work` is
                // the user's checkout, where a missing path makes the runtime
                // create a **directory** and report the failure later as a
                // permission error about something nobody created.
                //
                // Not an error when the target is elsewhere: `$HOME` paths are
                // the normal case and this is the exception they do not need.
                if staging == Staging::Apply {
                    if let Ok(rel) = guest.strip_prefix("/work/") {
                        place_destination(&worktree.join(rel))?;
                    }
                }
                mounts.push(Mount {
                    host: file,
                    guest,
                    read_only: true,
                    file: true,
                });
            }
        }
        Ok(dropped_hooks)
    }
}

/// Take out the links for entries this repo no longer uses.
///
/// The staging directory is keyed by session and harness, so the next launch
/// finds the last one's links still in it. Before `[use]` that was harmless: an
/// entry left the staged set only by being deleted from the catalogue, and the
/// leftover link then dangled into a layer that no longer held it. Selection
/// broke that — the layer behind the link is still mounted whole, deliberately
/// — so the link **resolved**, and `omh unuse` reported success while the agent
/// kept the entry forever.
///
/// Symlinks only, and only ones this selection excludes. The path is built by
/// joining a directory entry's own name to the directory it came from, so it
/// cannot escape; restricting to symlinks is what keeps this to removing things
/// omh put here.
fn prune(dst: &Path, cap: Capability, repo: &crate::settings::RepoPolicy) -> Result<()> {
    let entries = match std::fs::read_dir(dst) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e).with_context(|| format!("reading {}", dst.display())),
    };
    for entry in entries {
        // Not `.flatten()`: a `readdir` failing part-way through would leave a
        // stale link in place, which is the state this function exists to end.
        let entry = entry.with_context(|| format!("reading {}", dst.display()))?;
        if !entry.file_type().is_ok_and(|t| t.is_symlink()) {
            continue;
        }
        if repo
            .selection
            .allows(cap, &crate::profile::entry_name(&entry.file_name()))
        {
            continue;
        }
        std::fs::remove_file(entry.path())
            .with_context(|| format!("removing {}", entry.path().display()))?;
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

    /// What this container is made of, stamped onto it at launch.
    ///
    /// A `Plan` is a pure description; a container is one plan materialized.
    /// Everything here is fixed the moment `docker run` returns — no later
    /// `exec` can change the image, the mount set, the network or the
    /// environment. So a running container is the session you asked for only if
    /// these still match, and until this existed nothing asked: `omh opencode`
    /// against a session started by `omh claude` execed a binary that image does
    /// not contain, and `--account work` on one started as `personal` went on
    /// quietly using `personal`.
    ///
    /// `argv` and `tty` are deliberately absent. They belong to the *launch*,
    /// not to the container, and stamping them would rebuild the sandbox every
    /// time you passed a different flag to the harness.
    ///
    /// Values are verbatim rather than hashed, for two reasons. A digest can
    /// only say *that* something changed, and the line a user acts on has to say
    /// *what*. And the obvious hasher is the one `image::base_tag` uses —
    /// `DefaultHasher`, whose output std explicitly does not guarantee across
    /// releases. Fine for a tag, which is rebuilt anyway; here it would restart
    /// every running session on the day somebody upgrades Rust.
    pub fn labels(&self) -> Vec<(String, String)> {
        let mut mounts: Vec<String> = self
            .mounts
            .iter()
            .map(|m| {
                // Deliberately *not* docker's own `host:guest:ro` spelling. The
                // stamp travels on the same command line as the `-v` flags, and
                // a value that parses as a mount is one a reader — or a test
                // counting `:ro` — will mistake for one.
                format!(
                    "{} {} -> {}",
                    if m.read_only { "ro" } else { "rw" },
                    m.host.display(),
                    m.guest.display()
                )
            })
            .collect();
        mounts.sort();
        let mut env: Vec<String> = self.env.iter().map(|(k, v)| format!("{k}={v}")).collect();
        env.sort();
        // Sorted, because docker reports neither back in the order it was
        // given, and a stamp whose value depends on iteration order reads as
        // drift on the very next launch.
        //
        // Newline-separated: a mount is a pair of paths, and every other
        // plausible separator is a character a path may legally contain.
        vec![
            ("omh.image".into(), self.image.clone()),
            ("omh.network".into(), self.network.clone()),
            ("omh.workdir".into(), self.workdir.clone()),
            ("omh.mounts".into(), mounts.join("\n")),
            ("omh.env".into(), env.join("\n")),
        ]
    }

    /// One line, once, naming what this harness cannot do.
    ///
    /// Two granularities, because there are two kinds of loss. A capability is
    /// given up whole and counting is enough — nobody needs the names of nine
    /// skills. A hook is given up one at a time, and a count would be a lie
    /// dressed as a summary: "hooks: 0" while three are missing. So a dropped
    /// hook is named, with the word it asked for, because a hook that was never
    /// installed behaves exactly like one that has nothing to say.
    pub fn degradation(&self) -> Option<String> {
        let mut parts = Vec::new();
        if !self.dropped.is_empty() {
            let caps: Vec<_> = self
                .dropped
                .iter()
                .map(|(cap, n)| format!("{n} {cap}"))
                .collect();
            parts.push(format!("dropped {} (unsupported)", caps.join(", ")));
        }
        if !self.dropped_hooks.is_empty() {
            let hooks: Vec<_> = self.dropped_hooks.iter().map(|d| d.to_string()).collect();
            parts.push(format!("dropped hooks: {}", hooks.join(", ")));
        }
        (!parts.is_empty()).then(|| parts.join("; "))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ADAPTERS: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/adapters");
    const BASE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/base");

    /// What omh contributes and what this repo decided — the two inputs a plan
    /// takes from outside, resolved together because a fixture that built one
    /// without the other could say `codegraph` is off while its server is not.
    ///
    /// Called twice per `Options` literal, once for each half, which is why the
    /// manifest behind it is read once and leaked.
    fn decided() -> (crate::base::Own, crate::settings::RepoPolicy) {
        decided_with(Default::default())
    }

    /// The pair as a launch resolves it — from this fixture's settings files,
    /// through `settings::resolve`, exactly as `main::resolved` does.
    ///
    /// `decided()` builds a policy by hand, which is right for a case that is
    /// *about* a switched-off feature and wrong for everything else: a test that
    /// writes `[use]` into `.omh/settings.toml` and then hands `plan` a policy
    /// nobody read is asserting against its own struct literal.
    fn decided_from(fx: &Fx) -> (crate::base::Own, crate::settings::RepoPolicy) {
        let manifest = base_manifest();
        let repo = crate::settings::resolve(&fx.paths, manifest).unwrap();
        let installed = manifest.servers().into_keys().collect();
        (
            crate::base::own(manifest, &repo.off, &installed).unwrap(),
            repo,
        )
    }

    /// The shipped manifest, parsed once. Same reason `hook::tests::shipped`
    /// leaks its adapter: every fixture wants the bytes a launch would read,
    /// and re-parsing per call turned a fixture into a file-system benchmark.
    fn base_manifest() -> &'static crate::base::Manifest {
        static CELL: std::sync::OnceLock<crate::base::Manifest> = std::sync::OnceLock::new();
        CELL.get_or_init(|| crate::base::Manifest::load_dir(Path::new(BASE)).unwrap())
    }

    /// Every server the manifest names is treated as installed unless a case
    /// is about removal: `own` switches a feature off when its server is gone
    /// from the profile, and a fixture that declared none would silently
    /// disable everything.
    fn decided_with(
        off: std::collections::BTreeSet<String>,
    ) -> (crate::base::Own, crate::settings::RepoPolicy) {
        let manifest = crate::base::Manifest::load_dir(Path::new(BASE)).unwrap();
        let installed = manifest.servers().into_keys().collect();
        let own = crate::base::own(&manifest, &off, &installed).unwrap();
        // Through the same constructor `settings::resolve` uses, so a fixture
        // cannot hold a different opinion about which servers a feature owns.
        (
            own,
            crate::settings::RepoPolicy::switching_off(&manifest, off),
        )
    }

    struct Fx {
        _dir: tempfile::TempDir,
        paths: Paths,
        profile: Profile,
        session: Session,
    }

    /// Catalogue: rules, skills, subagents, mcp, one hook.
    /// The repo:  one hook, which is the only content a project may declare.
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

        let catalogue = &paths.root;
        write(catalogue.join("rules/tdd.md"), "personal rules");
        write(catalogue.join("skills/graphify/SKILL.md"), "graphify");
        write(catalogue.join("skills/review-diff/SKILL.md"), "review-diff");
        write(catalogue.join("subagents/explorer.md"), "explorer");
        write(
            catalogue.join("hooks/fmt.json"),
            r#"{"on":"turn-end","run":"fmt"}"#,
        );
        write(
            catalogue.join("mcp.json"),
            r#"{"mcpServers":{"m":{"command":"m"}}}"#,
        );

        // A carried file, because every invariant in this file is expressed
        // over the mount set and none of them could see a carried mount while
        // the fixture had no `carry_in`. Three guards that already existed —
        // the mount-destination one, the dry-run write one and the dry-run
        // parity one — went straight past the carried mount for that reason.
        std::fs::create_dir_all(paths.repo.join(".omh")).unwrap();
        std::fs::write(paths.repo.join(".env"), "SECRET=1\n").unwrap();
        std::fs::write(
            paths.repo.join(".omh/settings.toml"),
            "carry_in = [\".env\"]\n",
        )
        .unwrap();

        let session = Session::new(&paths.root.join("worktrees"), "s01".into());
        std::fs::create_dir_all(&session.worktree).unwrap();
        // Every real session worktree has one: `git worktree add` writes a
        // `.git` *file* naming the admin directory back in the checkout.
        //
        // The fixture needs it because `concat_destinations_exist_...` asserts
        // every `file` mount under `/work` has something on the host to land
        // on, and the sandbox's `.git` pointer is now one of those. What docker
        // does with a destination that is missing is the premise of that test
        // and is recorded there — `carry.rs` and `session.rs` do not fully
        // agree about it, and this comment is not the place to settle it.
        std::fs::write(
            session.worktree.join(".git"),
            "gitdir: /somewhere/in/the/checkout/.git/worktrees/s01\n",
        )
        .unwrap();

        let profile = Profile::resolve(&paths);
        Fx {
            _dir: dir,
            paths,
            profile,
            session,
        }
    }

    /// A plan against an adapter the caller built, rather than a shipped one.
    fn plan_with(
        fx: &Fx,
        adapter: &Adapter,
        decided: (crate::base::Own, crate::settings::RepoPolicy),
    ) -> Plan {
        let (own, repo) = decided;
        plan(
            &fx.paths,
            &fx.profile,
            adapter,
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
                repo,
                image: crate::image::tag_for(adapter),
                resolves: BTreeMap::new(),
            },
        )
        .unwrap()
    }

    /// **A measurement handed to `plan` reaches the hooks it renders.**
    ///
    /// `render`'s own tests prove `document` suppresses, and `container`'s
    /// fixtures all pass an empty map — so the wire between `Options.resolves`
    /// and the renderer is asserted nowhere. Replacing it with an empty map
    /// inside `plan` leaves the whole suite green, and the result is a session
    /// that ships every hook the image cannot run: the `cargo: not found`
    /// failure this milestone exists to remove, with the measurement taken,
    /// cached and then thrown away one call short of the renderer.
    #[test]
    fn a_measurement_reaches_the_hooks_a_plan_renders() {
        let fx = fixture();
        let adapter = Adapter::find(Path::new(ADAPTERS), "claude").unwrap();

        let with = |resolves: BTreeMap<String, bool>| {
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
                    memory_bin: None,
                    base: None,
                    omh: decided().0,
                    repo: decided().1,
                    image: crate::image::tag_for(&adapter),
                    resolves,
                },
            )
            .unwrap()
        };

        let measured_absent = with(BTreeMap::from([("fmt".to_string(), false)]));
        assert!(
            measured_absent
                .dropped_hooks
                .iter()
                .any(|d| d.name == "fmt"),
            "a hook whose program the image lacks must not reach the harness: {:?}",
            measured_absent.dropped_hooks
        );

        for known in [BTreeMap::from([("fmt".to_string(), true)]), BTreeMap::new()] {
            let p = with(known);
            assert!(
                !p.dropped_hooks.iter().any(|d| d.name == "fmt"),
                "measured present, and never measured, both leave a hook alone: {:?}",
                p.dropped_hooks
            );
        }
    }

    /// **A session runs the image the caller resolved**, and nothing else.
    ///
    /// The one fact this whole design turns on, and it was unguarded for a
    /// milestone: `plan` hardcoded `image::tag_for(adapter)`, so `init` built a
    /// stack layer that no session ever ran. A mutation sweep found it — every
    /// test passed with the layer replaced by the harness image, and with the
    /// harness image replaced by the base — because nothing asserted which
    /// image comes out.
    ///
    /// Pinned against a tag no adapter could produce, so the assertion cannot
    /// be satisfied by a derivation that happens to agree here: only passing
    /// `opts.image` through gives this answer.
    #[test]
    fn a_session_runs_the_image_the_caller_resolved() {
        let fx = fixture();
        let adapter = Adapter::find(Path::new(ADAPTERS), "claude").unwrap();
        let (own, repo) = decided();
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
                omh: own,
                repo,
                image: "omh/this-repos-toolchain:deadbeef".into(),
                resolves: BTreeMap::new(),
            },
        )
        .unwrap();

        assert_eq!(
            p.image, "omh/this-repos-toolchain:deadbeef",
            "the stack layer is built by `init` and must be the layer a session runs"
        );
        assert_ne!(
            p.image,
            crate::image::tag_for(&adapter),
            "a plan that re-derives the harness tag ignores what it was handed"
        );
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
        let (own, repo) = decided_from(fx);
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
                omh: own,
                repo,
                image: crate::image::tag_for(&adapter),
                resolves: BTreeMap::new(),
            },
        )
        .unwrap()
    }

    /// A repo with every hook off still gets the settings document, because
    /// that document is no longer only hooks: the approval that decides whether
    /// the mounted MCP document is *loaded* rides in it too.
    ///
    /// Coupled at a distance and therefore worth pinning — nothing about
    /// turning hooks off says anything about MCP, and the failure would be
    /// silent in both directions: servers listed, never loaded, every other
    /// check green.
    #[test]
    fn the_settings_document_ships_even_when_a_repo_runs_no_hooks() {
        let fx = fixture();
        // No hook layer *and* none of omh's own, which is the only arrangement
        // that used to skip this document. The fixture ships one, so it has to
        // go before the profile is resolved or this test passes without the
        // change it exists for.
        std::fs::remove_dir_all(fx.paths.root.join("hooks")).unwrap();
        let profile = Profile::resolve(&fx.paths);

        let adapter = Adapter::find(Path::new(ADAPTERS), "claude").unwrap();
        let (_, repo) = decided_from(&fx);
        let p = plan(
            &fx.paths,
            &profile,
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
                omh: Default::default(),
                repo,
                image: crate::image::tag_for(&adapter),
                resolves: BTreeMap::new(),
            },
        )
        .unwrap();

        let mount = p
            .mounts
            .iter()
            .find(|m| m.guest.ends_with(".claude/settings.json"))
            .expect("no hooks is not the same as no settings document");
        let doc: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&mount.host).unwrap()).unwrap();
        assert_eq!(
            doc["enableAllProjectMcpServers"], true,
            "the mcp document would be listed and never loaded: {doc}"
        );
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
            5,
            "worktree, graph cache, note store, the sandbox's own gitdir, and \
             the one carried file the fixture declares: {guests:?}"
        );
        assert!(
            guests.contains(&"/work/.env".to_string()),
            "a carried file is writable on purpose — it is config the agent may \
             edit, and the write lands on omh's copy rather than the checkout"
        );
        assert!(guests.contains(&"/work".to_string()));
        assert!(guests.iter().any(|g| g == crate::base::GRAPH_CACHE));
        assert!(guests.iter().any(|g| g == crate::memory::GUEST_LOCAL_NOTES));
        assert!(guests.iter().any(|g| g == crate::shadow::GUEST_GITDIR));
    }

    /// `omh doctor` and `omh auth` must not be handed the user's secrets.
    ///
    /// Both borrow a writable `/work` for one command through
    /// `Session::scratch`, and neither calls `carry_in` — that runs only in
    /// `run` and `attach`. So staging keyed on `Staging::Apply` alone put a
    /// plaintext copy of the user's `.env` into the doctor probe container and
    /// into the OAuth container `auth` gives network access to, with nothing
    /// printed: `carry_in`'s entire reporting surface is in the launcher it
    /// never reached. `omh doctor --dry-run` wrote the copy to disk too.
    ///
    /// `branch.is_some()` is the same test the shadow block uses twelve lines
    /// down, for a version of the same reason: a session with no branch is not
    /// doing work, so it needs neither a repository nor a credential.
    #[test]
    fn a_session_with_no_branch_is_handed_no_carried_secret() {
        let fx = fixture();
        std::fs::create_dir_all(fx.paths.repo.join(".omh")).unwrap();
        std::fs::write(fx.paths.repo.join(".env"), "SECRET=1\n").unwrap();
        std::fs::write(
            fx.paths.repo.join(".omh/settings.toml"),
            "carry_in = [\".env\"]\n",
        )
        .unwrap();

        let scratch = Session::scratch(fx.paths.scratch("doctor"), "doctor".into());
        std::fs::create_dir_all(&scratch.worktree).unwrap();
        let adapter = Adapter::find(Path::new(ADAPTERS), "claude").unwrap();
        let (own, repo) = decided_from(&fx);
        let p = plan(
            &fx.paths,
            &fx.profile,
            &adapter,
            &scratch,
            &[],
            Options {
                staging: Staging::Apply,
                persist: crate::persist::Mode::None,
                tty: true,
                account_dir: None,
                memory_bin: None,
                base: None,
                omh: own,
                repo,
                image: crate::image::tag_for(&adapter),
                resolves: BTreeMap::new(),
            },
        )
        .unwrap();

        assert!(
            !p.mounts
                .iter()
                .any(|m| m.guest.to_string_lossy() == "/work/.env"),
            "a borrowed /work must not be given the user's secrets: {:?}",
            p.mounts.iter().map(|m| &m.guest).collect::<Vec<_>>()
        );
        assert!(
            !fx.paths
                .staging("doctor", "claude")
                .join("carried")
                .exists(),
            "and nothing may write a plaintext copy of them to disk"
        );
    }

    /// A carried file arrives as a mount, not as a copy in the worktree.
    ///
    /// It was a copy, and that was fine while `/work` had no git in it. It does
    /// now, and `git clean -fdx` deletes an untracked file whether or not
    /// `info/exclude` names it — `-x` means "ignored files too". Measured in a
    /// container: the copy is removed, and the agent loses the `.env` the app
    /// needs for the rest of the session, since `carry_in` only runs at launch.
    ///
    /// A bind-mounted file survives because it is a *mountpoint*: `git clean`
    /// reports `failed to remove .env: Resource busy` and — the part that makes
    /// this the right fix rather than an adequate one — carries on cleaning
    /// everything else, so the agent's command still does its job.
    ///
    /// Read-write, so the agent can still edit what it was given, and sourced
    /// from a staged copy rather than from the checkout: mounting the user's own
    /// file read-write let an agent append to the real `.env`, which is the
    /// isolation this whole model exists for.
    #[test]
    fn a_carried_file_is_mounted_rather_than_copied_into_the_worktree() {
        let fx = fixture();
        std::fs::create_dir_all(fx.paths.repo.join(".omh")).unwrap();
        std::fs::write(fx.paths.repo.join(".env"), "SECRET=1\n").unwrap();
        std::fs::write(
            fx.paths.repo.join(".omh/settings.toml"),
            "carry_in = [\".env\"]\n",
        )
        .unwrap();

        let p = plan_for(&fx, "claude");

        let carried = p
            .mounts
            .iter()
            .find(|m| m.guest.to_string_lossy() == "/work/.env")
            .expect("a carried file has to be mounted into the worktree");
        assert!(carried.file, "a file, so the mountpoint resists unlink");
        assert!(
            !carried.read_only,
            "the agent has to be able to edit what it was handed"
        );
        assert_ne!(
            carried.host,
            fx.paths.repo.join(".env"),
            "mounting the checkout's own file lets the agent write to it"
        );
        assert_eq!(
            std::fs::read_to_string(&carried.host).expect("staged"),
            "SECRET=1\n",
            "and the staged copy has to be the file it stands in for"
        );
    }

    /// The agent commits into the shadow, so the gitdir is a read-write mount,
    /// so every file in it is a file the agent owns — including the `pre-push`
    /// hook that is the only thing standing between it and a remote it added
    /// itself. Measured: `rm` on a read-only mount gives `Resource busy`,
    /// overwriting and `chmod -x` give `Read-only file system`, and without the
    /// mount all three succeed.
    ///
    /// This does not make it a wall — `--no-verify` and `core.hooksPath` never
    /// read the file — and the doc on `GUEST_PRE_PUSH` says so. It makes the
    /// hook unable to *quietly* stop existing.
    #[test]
    fn the_sandbox_cannot_take_away_its_own_push_hook() {
        let fx = fixture();
        let p = plan_for(&fx, "claude");

        let hook = p
            .mounts
            .iter()
            .find(|m| m.guest.to_string_lossy() == crate::shadow::GUEST_PRE_PUSH)
            .expect("the push hook has to be mounted, not merely written");
        assert!(hook.read_only, "a hook the agent can rewrite is not a hook");
        assert!(hook.file, "a directory here would bury the gitdir's hooks");
        assert_eq!(
            std::fs::read_to_string(&hook.host).expect("staged"),
            crate::shadow::pre_push_hook(),
            "and it has to be the hook omh wrote, not an empty placeholder"
        );
    }

    /// The sandbox cannot rewrite the file that decides what its git does.
    ///
    /// Same shape as the push hook above and the same reason: the gitdir is
    /// read-write because the agent commits into it, so every file in it is the
    /// agent's — including the one holding `core.sshCommand`, `core.hooksPath`
    /// and the textconv drivers. `ensure` puts it back at each launch, which
    /// bounds how long a key survives; the mount is what stops it being set at
    /// all for the life of the container.
    #[test]
    fn the_sandbox_cannot_rewrite_the_config_that_decides_what_git_does() {
        let fx = fixture();
        let p = plan_for(&fx, "claude");

        let config = p
            .mounts
            .iter()
            .find(|m| m.guest.to_string_lossy() == crate::shadow::GUEST_CONFIG)
            .expect("the config has to be mounted, not merely written");
        assert!(
            config.read_only,
            "a config the agent can rewrite is not a config omh controls"
        );
        assert!(
            config.file,
            "a directory here would bury the gitdir's config"
        );

        let staged = std::fs::read_to_string(&config.host).expect("staged");
        let shadow = crate::shadow::Shadow::new(&fx.paths.shadows(), &fx.session.id);
        assert_eq!(
            staged,
            std::fs::read_to_string(shadow.gitdir.join("config")).expect("a seeded shadow"),
            "and it has to be the bytes git wrote, not a file omh composed"
        );
        assert!(
            staged.contains("sandbox@omh.invalid"),
            "including the identity the agent needs to commit at all: {staged}"
        );
    }

    /// omh mounts its own documents over paths inside `/work`, and the
    /// sandbox's repository must not track any of them. Seeded before the
    /// staging loop it captured the project's real `.mcp.json` — a file this
    /// repo tracks — and omh's rendered one was then mounted on top, which cost
    /// three things at once: the agent opened on `M .mcp.json`, a change it did
    /// not make; `git reset --hard` died on `unable to unlink old '.mcp.json':
    /// Resource busy`, because a bind mount cannot be unlinked, so the one
    /// recovery this feature exists to give was the thing that failed; and
    /// `git add -A` read through the mount and committed omh's rendered
    /// document, credentials and all.
    ///
    /// Asserted against the mount list rather than a fixed set of filenames, so
    /// a capability that starts staging something new is covered without anyone
    /// remembering this test exists.
    #[test]
    fn nothing_omh_mounts_into_the_worktree_is_tracked_by_the_sandbox() {
        let fx = fixture();
        let p = plan_for(&fx, "claude");

        let staged: Vec<String> = p
            .mounts
            .iter()
            .filter_map(|m| m.guest.strip_prefix("/work").ok())
            .map(|rel| rel.display().to_string())
            .filter(|rel| !rel.is_empty() && rel != ".git")
            .collect();
        assert!(
            !staged.is_empty(),
            "claude stages documents into /work; this test is vacuous otherwise"
        );

        let shadow = crate::shadow::Shadow::new(&fx.paths.shadows(), &fx.session.id);
        let exclude =
            std::fs::read_to_string(shadow.gitdir.join("info/exclude")).expect("a seeded shadow");
        for rel in staged {
            assert!(
                exclude.lines().any(|l| l == rel),
                "omh mounts {rel} into the worktree, so the sandbox's repository \
                 must not track it. exclude list was:\n{exclude}"
            );
        }
    }

    /// …including one that appears after the sandbox already exists.
    ///
    /// The sibling above proves the first launch derives the list from the
    /// mounts. This proves the second one does too, which is a different
    /// question: the repository is deliberately left as it is on relaunch, and
    /// the exclude list was left with it — so a document mounted later was one
    /// the sandbox had never heard of, and `git add -A` swept it in.
    ///
    /// Switching harness is the lever, because it is a first-class thing to do
    /// here — one session, `omh opencode` then `omh claude` — and because it
    /// moves the mount set and nothing else. A first version varied `carry_in`,
    /// which reaches the list twice over, once as policy and once as the mount
    /// it produces: it passed with the mount half of the derivation disabled
    /// outright, proving only that `carry_in` is copied through, which was
    /// never the defect.
    #[test]
    fn a_mount_added_after_the_first_launch_still_reaches_the_exclude_list() {
        let fx = fixture();
        let work_mounts = |p: &Plan| -> Vec<String> {
            p.mounts
                .iter()
                .filter_map(|m| m.guest.strip_prefix("/work").ok())
                .map(|rel| rel.display().to_string())
                .filter(|rel| !rel.is_empty() && rel != ".git")
                .collect()
        };

        let first = work_mounts(&plan_for(&fx, "opencode"));
        let second = work_mounts(&plan_for(&fx, "claude"));
        let arrived: Vec<&String> = second.iter().filter(|r| !first.contains(r)).collect();
        assert!(
            !arrived.is_empty(),
            "the harnesses have to differ in what they put in /work, or this is vacuous"
        );

        let shadow = crate::shadow::Shadow::new(&fx.paths.shadows(), &fx.session.id);
        let exclude =
            std::fs::read_to_string(shadow.gitdir.join("info/exclude")).expect("a seeded shadow");
        for rel in arrived {
            assert!(
                exclude.lines().any(|l| l == *rel),
                "{rel} was mounted by the second launch and the sandbox built by the \
                 first has never heard of it. exclude list was:\n{exclude}"
            );
        }
    }

    /// git is dead in the sandbox because `/work/.git` is a pointer into the
    /// user's checkout and the checkout is never mounted. The agent loses
    /// `status`, `diff`, `stash` and `reset --hard`, and the attached editor
    /// loses its source control panel — so omh gives it a repository of its
    /// own and points `/work/.git` at that instead.
    ///
    /// A *file* mount, and that is the whole trick: it shadows the pointer
    /// inside the container while the host's own file is never written, so
    /// `omh s diff`, `omh s commit` and `omh s push` go on working outside.
    #[test]
    fn the_sandbox_is_given_a_repository_of_its_own() {
        let fx = fixture();
        let p = plan_for(&fx, "claude");

        let gitdir = p
            .mounts
            .iter()
            .find(|m| m.guest.to_string_lossy() == crate::shadow::GUEST_GITDIR)
            .expect("the sandbox's gitdir has to be mounted");
        assert!(!gitdir.read_only, "the agent commits into it");
        assert!(!gitdir.file, "a gitdir is a directory");

        let pointer = p
            .mounts
            .iter()
            .find(|m| m.guest.to_string_lossy() == "/work/.git")
            .expect("`/work/.git` has to point at it");
        assert!(
            pointer.file,
            "a directory mount here would bury the worktree's own pointer \
             rather than shadow it"
        );
        assert!(pointer.read_only, "the pointer is omh's, not the agent's");
        // Against the gitdir mount's own guest path, not against
        // `pointer_file()` — comparing the shipped string to itself is a
        // tautology that stayed green while the pointer named `/nowhere`. These
        // are the two halves that have to agree: what the file says, and where
        // the repository it names is actually mounted.
        assert_eq!(
            std::fs::read_to_string(&pointer.host)
                .expect("the pointer is staged")
                .trim(),
            format!("gitdir: {}", gitdir.guest.display()),
            "the pointer must name the gitdir omh actually mounts"
        );
        assert_ne!(
            pointer.host,
            fx.session.worktree.join(".git"),
            "the host's own pointer must never be the thing mounted — writing \
             through it would corrupt the worktree's registration"
        );
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
                omh: decided().0,
                repo: decided().1,
                image: crate::image::tag_for(&adapter),
                resolves: BTreeMap::new(),
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

    /// The same mountpoint rule, for the documents the launcher *renders*
    /// rather than concatenates. `concat` placed its destinations and the
    /// generic arm did not, which was invisible for as long as every rendered
    /// target happened to sit in `$HOME` — where the image builds the
    /// mountpoint in and nothing has to place anything.
    ///
    /// The first `/work` target to arrive by that path would have had docker
    /// create a **directory** in the user's checkout and report it later as a
    /// permission error about something nobody created.
    #[test]
    fn a_rendered_document_bound_into_the_worktree_has_a_mountpoint_too() {
        let fx = fixture();
        let p = plan_for(&fx, "claude");

        let mcp = p
            .mounts
            .iter()
            .find(|m| m.guest.ends_with(".mcp.json"))
            .expect("claude is handed an mcp document");
        let Ok(rel) = mcp.guest.strip_prefix("/work") else {
            return; // an adapter whose harness reads it from $HOME needs none
        };
        let host = fx.session.worktree.join(rel);
        assert!(
            host.is_file(),
            "{} has nothing to mount onto: {} is missing",
            mcp.guest.display(),
            host.display()
        );
    }

    /// And it is a placeholder, never a rewrite: a repo that commits its own
    /// `.mcp.json` finds it byte-identical afterwards. The mount hides it for
    /// the length of the session — `omh mcp import` is how those servers come
    /// along — but hiding a file and truncating one look very different in
    /// somebody's diff.
    #[test]
    fn a_repos_own_mcp_document_survives_staging() {
        let fx = fixture();
        let own = fx.session.worktree.join(".mcp.json");
        std::fs::write(&own, r#"{"mcpServers":{"theirs":{"command":"t"}}}"#).unwrap();

        plan_for(&fx, "claude");

        assert_eq!(
            std::fs::read_to_string(&own).unwrap(),
            r#"{"mcpServers":{"theirs":{"command":"t"}}}"#
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

    /// What a `dir` capability actually offers the harness: the names in the
    /// staged directory it mounts, not the names in the catalogue behind it.
    fn staged_entries(p: &Plan, cap: Capability, guest_suffix: &str) -> Vec<String> {
        let mount = p
            .mounts
            .iter()
            .find(|m| m.guest.ends_with(guest_suffix))
            .unwrap_or_else(|| panic!("nothing staged for {cap}"));
        let mut names: Vec<String> = std::fs::read_dir(&mount.host)
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    /// A catalogue you cannot subtract from is not a catalogue.
    ///
    /// One place for content was P3's answer to "where is this skill". It left
    /// the other half unanswered: everything in it reaches every session, so
    /// "these are my twelve skills, this project uses two" was unsayable and the
    /// only lever was uninstalling globally — which is the opposite of curating.
    ///
    /// An allowlist, and one mechanism only. Removing something is deleting its
    /// name, and there is one place to look to answer "is this on here".
    #[test]
    fn a_repo_uses_the_catalogue_entries_it_named() {
        let fx = fixture();
        std::fs::create_dir_all(fx.paths.repo.join(".omh")).unwrap();
        std::fs::write(
            fx.paths.repo.join(".omh/settings.toml"),
            "[use]\nskills = [\"review-diff\"]\n",
        )
        .unwrap();

        let staged = staged_entries(
            &plan_for(&fx, "claude"),
            Capability::Skills,
            ".claude/skills",
        );
        assert_eq!(
            staged,
            vec!["review-diff"],
            "graphify is in the catalogue and was not named here"
        );
    }

    /// Write a `[use]` table into this repo's committed settings.
    fn selects(fx: &Fx, table: &str) {
        std::fs::create_dir_all(fx.paths.repo.join(".omh")).unwrap();
        std::fs::write(
            fx.paths.repo.join(".omh/settings.toml"),
            format!("[use]\n{table}"),
        )
        .unwrap();
    }

    /// The servers the harness is actually handed, out of the rendered document.
    fn staged_servers(p: &Plan) -> Vec<String> {
        let mount = p
            .mounts
            .iter()
            .find(|m| m.guest.ends_with(".mcp.json"))
            .expect("claude is handed its mcp document somewhere");
        let body = std::fs::read_to_string(&mount.host).unwrap();
        let doc: serde_json::Value = serde_json::from_str(&body).unwrap();
        let mut names: Vec<String> = doc["mcpServers"]
            .as_object()
            .expect("an object keyed by server name")
            .keys()
            .cloned()
            .collect();
        names.sort();
        names
    }

    /// A server this repo did not name is dropped from the document it is
    /// handed, and left in `mcp.json` exactly as you have it — the same
    /// distinction disabling a feature makes, for the same reason.
    #[test]
    fn a_server_this_repo_did_not_name_is_dropped_from_the_document() {
        let fx = fixture();
        std::fs::write(
            fx.paths.root.join("mcp.json"),
            r#"{"mcpServers":{"linear":{"command":"l"},"sentry":{"command":"s"}}}"#,
        )
        .unwrap();
        selects(&fx, "mcp = [\"linear\"]\n");

        assert_eq!(staged_servers(&plan_for(&fx, "claude")), vec!["linear"]);
        assert!(
            std::fs::read_to_string(fx.paths.root.join("mcp.json"))
                .unwrap()
                .contains("sentry"),
            "the catalogue is yours and is left as you have it"
        );
    }

    /// A hook this repo did not name never reaches the harness. Both tiers:
    /// the catalogue's and the repo's own `.omh/hooks/`.
    #[test]
    fn a_hook_this_repo_did_not_name_does_not_reach_the_harness() {
        let fx = fixture();
        std::fs::create_dir_all(fx.paths.repo.join(".omh/hooks")).unwrap();
        std::fs::write(
            fx.paths.repo.join(".omh/hooks/lint.json"),
            r#"{"on":"turn-end","run":"repo lint"}"#,
        )
        .unwrap();
        selects(&fx, "hooks = [\"lint\"]\n");

        let staged = staged_hooks(&plan_for(&fx, "claude"));
        assert!(staged.iter().any(|c| c == "repo lint"), "got: {staged:?}");
        assert!(
            !staged.iter().any(|c| c == "fmt"),
            "the catalogue's `fmt` was not named here: {staged:?}"
        );
    }

    /// `[use]` names *your* entries; a feature is `[omh]`'s business. With every
    /// list empty, omh's own must arrive whole — server, hooks and rules section
    /// together — or the table that is not allowed to take a feature apart has
    /// taken one apart.
    #[test]
    fn an_empty_selection_leaves_every_omh_feature_whole() {
        let fx = fixture();
        std::fs::write(
            fx.paths.root.join("mcp.json"),
            r#"{"mcpServers":{"codegraph":{"command":"c"},"memory":{"command":"omh"},
                              "linear":{"command":"l"}}}"#,
        )
        .unwrap();
        selects(
            &fx,
            "rules = []\nskills = []\nmcp = []\ncommands = []\nsubagents = []\nhooks = []\n",
        );

        let p = plan_for(&fx, "claude");
        assert_eq!(
            staged_servers(&p),
            vec!["codegraph", "memory"],
            "omh's own survive an empty list; `linear` is yours and does not"
        );
        let hooks = staged_hooks(&p);
        for (name, command) in own_commands() {
            assert!(
                hooks.contains(&command),
                "{name} is omh's and must still fire: {hooks:?}"
            );
        }
        assert!(
            composed_rules(&p).contains(crate::shadow::ARRANGEMENT),
            "and omh's rules sections are part of the same features"
        );
    }

    /// The list is the order — the thing P3 deferred, because ordering can only
    /// really come from a list somebody wrote. Rules build on each other: a
    /// general one followed by its exception reads differently reversed.
    #[test]
    fn the_use_list_is_the_rules_order() {
        let fx = fixture();
        let write = |name: &str, body: &str| {
            std::fs::write(fx.paths.root.join("rules").join(name), body).unwrap()
        };
        write("apple.md", "APPLE RULE");
        write("zebra.md", "ZEBRA RULE");
        selects(&fx, "rules = [\"zebra\", \"apple\"]\n");

        let body = composed_rules(&plan_for(&fx, "claude"));
        let zebra = body.find("ZEBRA RULE").expect("zebra composed");
        let apple = body.find("APPLE RULE").expect("apple composed");
        assert!(zebra < apple, "declared order, not filename order:\n{body}");
        assert!(
            !body.contains("personal rules"),
            "and `tdd` was not named, so it is not there:\n{body}"
        );
    }

    /// Without a list, filename order still stands. It is the fallback P3
    /// shipped and a repo that has not curated must not lose its rules.
    #[test]
    fn rules_fall_back_to_filename_order_without_a_list() {
        let fx = fixture();
        std::fs::write(fx.paths.root.join("rules/apple.md"), "APPLE RULE").unwrap();

        let body = composed_rules(&plan_for(&fx, "claude"));
        let apple = body.find("APPLE RULE").expect("apple composed");
        let tdd = body.find("personal rules").expect("tdd composed");
        assert!(apple < tdd, "alphabetical, and both there:\n{body}");
    }

    /// Every `dir` capability respects the selection, not just the one with a
    /// test. They share a loop, so hardcoding `Capability::Skills` in the filter
    /// left `commands` and `subagents` ignoring `[use]` entirely with the suite
    /// green.
    #[test]
    fn every_dir_capability_respects_the_selection() {
        let fx = fixture();
        let write = |p: PathBuf| {
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, "x").unwrap();
        };
        write(fx.paths.root.join("commands/ship.md"));
        write(fx.paths.root.join("commands/drop.md"));
        write(fx.paths.root.join("subagents/keep.md"));
        write(fx.paths.root.join("subagents/lose.md"));
        selects(
            &fx,
            "skills = [\"review-diff\"]\ncommands = [\"ship\"]\nsubagents = [\"keep\"]\n",
        );

        let p = plan_for(&fx, "claude");
        for (cap, guest, want) in [
            (Capability::Skills, ".claude/skills", "review-diff"),
            (Capability::Commands, ".claude/commands", "ship.md"),
            (Capability::Subagents, ".claude/agents", "keep.md"),
        ] {
            assert_eq!(
                staged_entries(&p, cap, guest),
                vec![want],
                "{cap} did not respect the list"
            );
        }
    }

    /// Deselecting something has to reach a session that already staged it.
    ///
    /// The staging directory is keyed by session and harness, so it is the same
    /// directory on the next launch, and nothing used to remove a link from it.
    /// Before `[use]` that was harmless: an entry left the staged set only by
    /// being deleted from the catalogue, and the leftover symlink then dangled
    /// into a layer that no longer had it. Selection breaks that — the layer
    /// behind the link is still mounted whole, deliberately — so the link
    /// **still resolves** and the agent keeps the entry the user just removed.
    ///
    /// Exit 0, a success line naming the file it wrote, and the thing is still
    /// there: the failure this project fears most, on the command whose entire
    /// job is removal.
    #[test]
    fn deselecting_an_entry_takes_it_out_of_a_directory_already_staged() {
        let fx = fixture();
        let staged = |p: &Plan| staged_entries(p, Capability::Skills, ".claude/skills");

        assert_eq!(
            staged(&plan_for(&fx, "claude")),
            vec!["graphify", "review-diff"],
            "both, before this repo says otherwise"
        );

        selects(&fx, "skills = [\"review-diff\"]\n");
        assert_eq!(
            staged(&plan_for(&fx, "claude")),
            vec!["review-diff"],
            "the link from the earlier launch has to go, or `unuse` removed nothing"
        );
    }

    /// And the pruning must not reach anything omh did not put there. The staged
    /// directory is omh's, but a mistake here deletes from a path built by
    /// joining a name to a directory, which is the shape worth being careful in.
    #[test]
    fn pruning_leaves_a_file_omh_did_not_stage() {
        let fx = fixture();
        let p = plan_for(&fx, "claude");
        let dst = p
            .mounts
            .iter()
            .find(|m| m.guest.ends_with(".claude/skills"))
            .expect("skills staged")
            .host
            .clone();
        std::fs::write(dst.join("notes.txt"), "mine").unwrap();

        selects(&fx, "skills = []\n");
        plan_for(&fx, "claude");
        assert!(
            dst.join("notes.txt").exists(),
            "only omh's own links are omh's to remove"
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

    /// omh's own hooks as *this harness* receives them.
    ///
    /// A hook is authored in omh's words and staged in Claude's, so the thing
    /// to look for in a settings document is the rendering — asserting the
    /// authored `run` string would pass against a harness that was handed
    /// nothing.
    fn own_commands() -> Vec<(&'static str, String)> {
        let adapter = Adapter::find(Path::new(ADAPTERS), "claude").unwrap();
        let binding = adapter
            .supports(Capability::Hooks)
            .expect("claude has hooks");
        crate::base::hooks()
            .into_iter()
            .map(
                |h| match crate::hook::render(h.name, &h.hook, binding, &adapter.tools).unwrap() {
                    crate::hook::Outcome::Rendered(r) => (h.name, r.command),
                    crate::hook::Outcome::Dropped(d) => panic!("claude cannot express {d}"),
                },
            )
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
        std::fs::remove_dir_all(fx.paths.root.join("hooks")).unwrap();

        let staged = staged_hooks(&plan_for(&fx, "claude"));
        for (name, command) in own_commands() {
            assert!(
                staged.contains(&command),
                "{name} must reach the harness with no hooks layer to read it: {staged:?}"
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
                omh: decided_with(["memory".to_string()].into()).0,
                repo: decided_with(["memory".to_string()].into()).1,
                image: crate::image::tag_for(&adapter),
                resolves: BTreeMap::new(),
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
            fx.paths.root.join("mcp.json"),
            r#"{"mcpServers":{"codegraph":{"command":"codebase-memory-mcp"}}}"#,
        )
        .unwrap();
        let (own, repo) = decided_with(["codegraph".to_string()].into());

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
                repo,
                image: crate::image::tag_for(&adapter),
                resolves: BTreeMap::new(),
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
                .root
                .join("mcp.json")
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
    /// A hook answering to a manifest name stops the launch, naming the file.
    ///
    /// P2 skipped these silently, which was right while the only such files
    /// were leftovers omh had seeded into `.omh/profile/hooks/` itself. Nothing
    /// reads that directory any more, so a file with one of these names is
    /// something somebody wrote on purpose — and a hook that is committed,
    /// reviewed, and quietly never runs is worse than one that refuses to
    /// start.
    ///
    /// It has to reach the *launch*, not just the renderer: the whole failure
    /// is a hook the user believes is installed.
    #[test]
    fn a_repo_hook_answering_to_a_manifest_name_stops_the_launch() {
        let fx = fixture();
        let hooks = fx.paths.repo.join(".omh/hooks");
        std::fs::create_dir_all(&hooks).unwrap();
        std::fs::write(
            hooks.join("graph-refresh.json"),
            r#"{"on":"turn-end","run":"my own indexer"}"#,
        )
        .unwrap();

        let adapter = Adapter::find(Path::new(ADAPTERS), "claude").unwrap();
        let err = plan(
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
                omh: decided().0,
                repo: decided().1,
                image: crate::image::tag_for(&adapter),
                resolves: BTreeMap::new(),
            },
        )
        .expect_err("a reserved name must not launch");
        let msg = format!("{err:#}");
        assert!(msg.contains("graph-refresh"), "name it: {msg}");
        assert!(msg.contains("settings.toml"), "and say the way out: {msg}");
    }

    /// What a hook may name and what the sandbox sets are one list.
    ///
    /// `hook::check_interpolation` refuses a `$` in `inject` that names
    /// anything omh does not bind — which is only true while `SANDBOX_VARS`
    /// and the env this function builds agree. Drift either way is silent: a
    /// variable set and not nameable is a refusal nobody can act on, and a
    /// variable nameable and not set expands to nothing in the middle of a
    /// sentence, which is the failure the check exists for.
    #[test]
    fn the_sandbox_sets_what_a_hook_may_name() {
        let fx = fixture();
        let plan = plan_for(&fx, "claude");
        let set: Vec<&str> = plan.env.iter().map(|(k, _)| k.as_str()).collect();

        for var in crate::hook::SANDBOX_VARS {
            assert!(
                set.contains(&var),
                "a hook may name ${var} and the sandbox does not set it: {set:?}"
            );
        }
        assert_eq!(
            set.len(),
            crate::hook::SANDBOX_VARS.len(),
            "and the sandbox sets nothing a hook is refused for naming: {set:?}"
        );
    }

    /// Switching a feature off does not hand you its names.
    ///
    /// `reserved` is built from every manifest hook whether or not its feature
    /// is on, and that is load-bearing in the *off* case specifically: with
    /// `codegraph` disabled there is no generated hook to win the merge, so a
    /// file called `graph-refresh.json` would simply be read and run — against
    /// a server that was dropped from the document. Disabling something and
    /// leaving it running is worse than not offering the switch.
    ///
    /// The existing guard covers the feature-*on* case, where the generated
    /// hook would have won anyway, so it stayed green with `reserved` narrowed
    /// to enabled features only.
    #[test]
    fn a_disabled_features_names_are_still_omhs() {
        let fx = fixture();
        let hooks = fx.paths.repo.join(".omh/hooks");
        std::fs::create_dir_all(&hooks).unwrap();
        std::fs::write(
            hooks.join("graph-refresh.json"),
            r#"{"on":"turn-end","run":"the disabled thing, still running"}"#,
        )
        .unwrap();

        let adapter = Adapter::find(Path::new(ADAPTERS), "claude").unwrap();
        let err = plan(
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
                omh: decided_with(["codegraph".to_string()].into()).0,
                repo: decided_with(["codegraph".to_string()].into()).1,
                image: crate::image::tag_for(&adapter),
                resolves: BTreeMap::new(),
            },
        )
        .expect_err("a manifest name is omh's whether the feature is on or off");
        assert!(format!("{err:#}").contains("graph-refresh"), "got: {err:#}");
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
                omh: decided().0,
                repo: decided().1,
                image: crate::image::tag_for(&adapter),
                resolves: BTreeMap::new(),
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
            ["graphify", "review-diff"],
            "every catalogue entry is linked by name"
        );

        // Every source a link can point into must actually be mounted.
        for i in 0..fx.profile.sources(Capability::Skills).unwrap().len() {
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

    /// A harness with no hooks at all.
    ///
    /// opencode used to be one, and the tests about giving up a whole
    /// capability used it. It grew a plugin system, so the *shipped* adapters
    /// now express all six between them — which is the capability floor reached,
    /// and leaves these guards without a subject. Synthesised rather than
    /// deleted: "a harness that cannot express a capability says so" is the
    /// invariant, not "opencode cannot express hooks".
    fn without_hooks() -> (tempfile::TempDir, Adapter) {
        let dir = tempfile::tempdir().unwrap();
        let real = std::fs::read_to_string(Path::new(ADAPTERS).join("claude.toml")).unwrap();
        let head = &real[..real.find("[capabilities.hooks]").expect("claude has hooks")];
        // `[tools]` goes with them: it is read by nobody without hooks, and the
        // adapter guard refuses a map nobody reads — correctly, and it is the
        // reason this cannot be a one-line edit.
        let tools = head.find("[tools]").expect("claude has a tool vocabulary");
        let after = head[tools..]
            .find("[capabilities.")
            .expect("a capability follows the vocabulary")
            + tools;
        std::fs::write(
            dir.path().join("plain.toml"),
            format!("{}{}", &head[..tools], &head[after..])
                .replace("name    = \"claude\"", "name    = \"plain\""),
        )
        .unwrap();
        let adapter = Adapter::find(dir.path(), "plain").unwrap();
        (dir, adapter)
    }

    #[test]
    fn unsupported_capabilities_are_reported_not_silently_dropped() {
        let fx = fixture();
        // All three shipped adapters express every capability — what the
        // capability floor asks for, reached by teaching omh to write a plugin
        // twice, since neither opencode nor omp has declarative hook config.
        for harness in ["claude", "omp", "opencode"] {
            assert!(
                plan_for(&fx, harness).dropped.is_empty(),
                "{harness} gives up no whole capability"
            );
        }
        // opencode still gives up four *hooks*, which is the finer grain
        // `dropped_hooks` exists for and a different statement: the capability
        // arrives, and four of the things in it cannot be said here. Every one
        // is an advisory injection, which is the shape opencode has no channel
        // for — including `git-note`, so on this harness an agent comes back
        // from a sync with no sentence about it.
        let oc = plan_for(&fx, "opencode");
        let named: Vec<&str> = oc.dropped_hooks.iter().map(|d| d.name.as_str()).collect();
        assert_eq!(
            named,
            ["git-note", "graph-first", "graph-orient", "graph-read"]
        );
        let msg = oc.degradation().expect("and they are said out loud");
        assert!(msg.contains("graph-read"), "by name: {msg}");

        // omp gives up the same four, and for reasons that are its own rather
        // than opencode's: it *has* a `session-start` moment, so `graph-orient`
        // is dropped for having no advisory channel there rather than for the
        // moment being absent. Pinned here because nothing else asserts which
        // hooks survive the new renderer's drop rules — a regression in them
        // stayed green everywhere else.
        let omp = plan_for(&fx, "omp");
        let named: Vec<&str> = omp.dropped_hooks.iter().map(|d| d.name.as_str()).collect();
        assert_eq!(
            named,
            ["git-note", "graph-first", "graph-orient", "graph-read"]
        );
        assert!(
            omp.dropped_hooks
                .iter()
                .any(|d| d.name == "graph-orient" && d.wanted.contains("session-start")),
            "the reason has to name the moment it asked for: {:?}",
            omp.dropped_hooks
        );

        let (_dir, plain) = without_hooks();
        let p = plan_with(&fx, &plain, decided());
        let dropped: Vec<_> = p.dropped.iter().map(|(c, _)| *c).collect();
        assert_eq!(dropped, vec![Capability::Hooks]);
        let msg = p.degradation().unwrap();
        assert!(msg.contains("hooks"), "got: {msg}");

        // What is given up includes omh's own, which come from the manifest
        // rather than from a layer. Counting only the profile's files would
        // report a harness dropping one hook while it drops six.
        let hooks = p
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

    /// A harness can have the hooks capability and still not have every moment
    /// in it, which is a granularity `dropped` cannot express: a count per
    /// capability says "hooks: 0" while three of them are missing.
    ///
    /// The failure this prevents is the quietest one there is. A hook that was
    /// never installed behaves exactly like a hook that is installed and has
    /// nothing to say — `graph-read` is silent on small files by design — so
    /// nothing about a session would ever reveal it.
    #[test]
    fn a_hook_this_harness_cannot_express_is_named_at_launch() {
        let fx = fixture();

        // Claude with no `before-tool`. Everything else it can still spell, so
        // this is a harness that keeps hooks and loses three of them.
        let dir = tempfile::tempdir().unwrap();
        let real = std::fs::read_to_string(Path::new(ADAPTERS).join("claude.toml")).unwrap();
        std::fs::write(
            dir.path().join("partial.toml"),
            real.replace("name    = \"claude\"", "name    = \"partial\"")
                .replace("before-tool   = \"PreToolUse\"", ""),
        )
        .unwrap();

        let adapter = Adapter::find(dir.path(), "partial").unwrap();
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
                omh: decided().0,
                repo: decided().1,
                image: crate::image::tag_for(&adapter),
                resolves: BTreeMap::new(),
            },
        )
        .unwrap();

        let named: Vec<_> = p.dropped_hooks.iter().map(|d| d.name.as_str()).collect();
        assert_eq!(named, ["graph-first", "graph-read"]);

        let msg = p
            .degradation()
            .expect("a dropped hook has to be said out loud");
        assert!(msg.contains("graph-read"), "by name: {msg}");
        assert!(msg.contains("before-tool"), "and what it wanted: {msg}");

        // The rest of the capability survives, which is the whole point of
        // dropping one hook rather than all of them.
        let staged = staged_hooks(&p);
        let (_, refresh) = own_commands()
            .into_iter()
            .find(|(name, _)| *name == "graph-refresh")
            .unwrap();
        assert!(
            staged.contains(&refresh),
            "turn-end still works: {staged:?}"
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
                    .join("hooks")
                    .join(format!("{}.json", hook.name)),
                r#"{"on":"turn-end","run":"seeded by an older omh"}"#,
            )
            .unwrap();
        }

        let (_dir, plain) = without_hooks();
        let p = plan_with(&fx, &plain, decided());
        let hooks = p
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
            body.contains("personal rules"),
            "the catalogue must be in there: {body}"
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
                omh: decided().0,
                repo: decided().1,
                image: crate::image::tag_for(&bad),
                resolves: BTreeMap::new(),
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
                omh: decided().0,
                repo: decided().1,
                image: crate::image::tag_for(&adapter),
                resolves: BTreeMap::new(),
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
        // Including the sandbox's repository, which lands under `shadow/`
        // rather than `run/` and so was outside everything this test looked at
        // — the `Apply` guard around `ensure` could be deleted and the suite
        // stayed green.
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
                omh: decided().0,
                repo: decided().1,
                image: crate::image::tag_for(&adapter),
                resolves: BTreeMap::new(),
            },
        )
        .unwrap();

        assert!(!fx.paths.root.join("run").exists(), "no staging directory");
        assert!(
            !fx.paths.shadows().exists(),
            "a dry run created the sandbox's repository at {}",
            fx.paths.shadows().display()
        );
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
                omh: decided().0,
                repo: decided().1,
                image: crate::image::tag_for(&adapter),
                resolves: BTreeMap::new(),
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
                omh: decided().0,
                repo: decided().1,
                image: crate::image::tag_for(&adapter),
                resolves: BTreeMap::new(),
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
            omh: decided().0,
            repo: decided().1,
            image: crate::image::tag_for(&adapter),
            resolves: BTreeMap::new(),
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
            omh: decided().0,
            repo: decided().1,
            image: crate::image::tag_for(&adapter),
            resolves: BTreeMap::new(),
        };
        let p = plan(&fx.paths, &fx.profile, &adapter, &fx.session, &[], opts).unwrap();
        assert_eq!(p.argv, ["claude"]);
    }

    // ── what a container was made of ────────────────────────────────────────

    fn plan_argv(fx: &Fx, harness: &str, argv: &[String]) -> Plan {
        let adapter = Adapter::find(Path::new(ADAPTERS), harness).unwrap();
        let (own, repo) = decided_from(fx);
        plan(
            &fx.paths,
            &fx.profile,
            &adapter,
            &fx.session,
            argv,
            Options {
                staging: Staging::Apply,
                persist: crate::persist::Mode::None,
                tty: true,
                account_dir: None,
                memory_bin: None,
                base: None,
                omh: own,
                repo,
                image: crate::image::tag_for(&adapter),
                resolves: BTreeMap::new(),
            },
        )
        .unwrap()
    }

    fn plan_account(fx: &Fx, harness: &str, account_dir: Option<PathBuf>) -> Plan {
        let adapter = Adapter::find(Path::new(ADAPTERS), harness).unwrap();
        let (own, repo) = decided_from(fx);
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
                account_dir,
                memory_bin: None,
                base: None,
                omh: own,
                repo,
                image: crate::image::tag_for(&adapter),
                resolves: BTreeMap::new(),
            },
        )
        .unwrap()
    }

    fn stamped(labels: &[(String, String)], key: &str) -> String {
        labels
            .iter()
            .find(|(k, _)| k == key)
            .unwrap_or_else(|| panic!("no {key} in {labels:?}"))
            .1
            .clone()
    }

    fn as_running(labels: &[(String, String)]) -> std::collections::BTreeMap<String, String> {
        labels.iter().cloned().collect()
    }

    /// The bug this exists for: a container is a materialization of *one* plan,
    /// and `session_up` handed a running one back without ever asking which
    /// plan that was. `omh opencode` against a session started by `omh claude`
    /// execed a binary the image does not contain — verified live — and
    /// `--account work` on a session started as `personal` silently went on
    /// using `personal`.
    #[test]
    fn the_stamp_distinguishes_the_harness_a_session_was_built_for() {
        let fx = fixture();
        let claude = plan_for(&fx, "claude").labels();
        let opencode = plan_for(&fx, "opencode").labels();
        assert_ne!(
            stamped(&claude, "omh.image"),
            stamped(&opencode, "omh.image"),
            "the image is per-harness, so the stamp must be too"
        );
    }

    /// The other half, and the reason `argv` is left out: relaunching the same
    /// harness with different arguments is the ordinary case. A stamp that moved
    /// would restart the container on every `claude --resume`.
    #[test]
    fn the_stamp_ignores_the_harness_command_line() {
        let fx = fixture();
        let plain = plan_argv(&fx, "claude", &[]);
        let resumed = plan_argv(&fx, "claude", &["--resume".into(), "x".into()]);
        assert_ne!(plain.argv, resumed.argv, "the fixture must differ at all");
        assert_eq!(plain.labels(), resumed.labels());
    }

    /// `run` already states the rule this enforces: "silently using the wrong
    /// account is expensive and invisible".
    #[test]
    fn switching_account_moves_the_stamp() {
        let fx = fixture();
        let personal = plan_account(&fx, "claude", Some(fx.paths.root.join("creds/personal")));
        let work = plan_account(&fx, "claude", Some(fx.paths.root.join("creds/work")));
        assert_ne!(personal.labels(), work.labels());
    }

    #[test]
    fn the_same_plan_stamps_the_same_way_twice() {
        let fx = fixture();
        let once = plan_for(&fx, "claude").labels();
        let twice = plan_for(&fx, "claude").labels();
        assert_eq!(once, twice, "an unstable stamp restarts every launch");
    }

    #[test]
    fn a_container_built_from_this_plan_has_not_drifted() {
        let fx = fixture();
        let want = plan_for(&fx, "claude").labels();
        assert!(drift(&want, &as_running(&want)).is_empty());
    }

    #[test]
    fn drift_names_the_fact_that_changed() {
        let fx = fixture();
        let want = plan_for(&fx, "opencode").labels();
        let have = as_running(&plan_for(&fx, "claude").labels());

        let found = drift(&want, &have);
        assert!(!found.is_empty(), "a harness switch is drift");
        let joined = found.join("; ");
        assert!(
            joined.contains("image"),
            "the reason has to be nameable, not a bare digest: {joined}"
        );
    }

    /// A container omh started before it stamped anything cannot be verified,
    /// and an unverifiable container is what this check exists to stop being
    /// trusted. Restarting one costs nothing — the worktree and branch are on
    /// the host.
    #[test]
    fn a_container_with_no_stamp_at_all_has_drifted() {
        let fx = fixture();
        let want = plan_for(&fx, "claude").labels();
        assert!(!drift(&want, &Default::default()).is_empty());
    }

    // ── deciding whether a running container is the one you asked for ───────

    fn stamp_of(p: &Plan) -> std::collections::BTreeMap<String, String> {
        p.labels().into_iter().collect()
    }

    #[test]
    fn a_container_matching_the_plan_is_handed_straight_back() {
        let fx = fixture();
        let p = plan_for(&fx, "claude");
        assert!(matches!(reuse(&stamp_of(&p), &p, &[]), Reuse::Attach));
    }

    /// Every branch of the one decision that can destroy a running container.
    ///
    /// This is the guard AGENTS.md asks for and the PR that introduced the
    /// type did not have. Everything destructive in a launch is downstream of
    /// `decide`, and none of it was reachable: `tests/cli.rs` launches with
    /// `--dry-run`, which returns first, and there is no fake runtime. So the
    /// parsers were tested and the thing they decide was not — three reviewers
    /// said so independently, and the mutation they each named (point
    /// `Probe::Unknown` at `not_enterable()`) left the whole tree green.
    #[test]
    fn nothing_a_probe_could_not_read_ends_in_a_container_being_destroyed() {
        use crate::image::{Probe, Stamp};
        let fx = fixture();
        let p = plan_for(&fx, "claude");
        let matching = || Stamp::Read(stamp_of(&p));
        let unread = || panic!("the stamp is not read when the probe already decided");

        // The two that mean *replace it*, and the one thing they have in
        // common: nothing alive inside to lose. Neither reads the stamp.
        for (probed, what) in [
            (Probe::NotEnterable, "a worktree replaced under it"),
            (Probe::Gone, "and a container that is not there at all"),
        ] {
            assert!(
                matches!(
                    decide("s01", probed, unread, &p).unwrap(),
                    Reuse::Restart(_)
                ),
                "{what} is replaced"
            );
        }

        // The refusal. Both halves matter: it does not restart, and it says
        // what the runtime said.
        let refused = decide(
            "s01",
            Probe::Unknown("daemon not reachable".into()),
            unread,
            &p,
        )
        .unwrap_err()
        .to_string();
        assert!(
            refused.contains("daemon not reachable"),
            "the runtime's own words reach the user: {refused}"
        );
        assert!(
            refused.contains("omh s01 down"),
            "and a way on that works without entering it: {refused}"
        );

        // A stamp omh cannot read is the same refusal, one call later — the
        // collapse that sat one line below the first fix.
        let refused = decide(
            "s01",
            Probe::Listed(String::new()),
            || Stamp::Unknown("answered with something omh could not read".into()),
            &p,
        )
        .unwrap_err()
        .to_string();
        assert!(
            refused.contains("could not read what s01's sandbox was built from"),
            "an unreadable stamp is not a container that predates the check: {refused}"
        );

        // And the ordinary path still works, both ways.
        assert!(
            matches!(
                decide("s01", Probe::Listed(String::new()), matching, &p).unwrap(),
                Reuse::Attach
            ),
            "a container matching the plan is handed back"
        );
        let stale = plan_for(&fx, "opencode");
        assert!(
            matches!(
                decide("s01", Probe::Listed(String::new()), matching, &stale).unwrap(),
                Reuse::Restart(_)
            ),
            "and one built for another harness is replaced"
        );
    }

    /// A live harness is never replaced, whatever else has drifted — and the
    /// listing that proves it is live comes from the probe.
    ///
    /// `Reuse::Blocked`'s own doc says the cost of being wrong is an agent
    /// stopped mid-task. That guarantee is only as good as the listing
    /// reaching this decision intact, which is why it is asserted here rather
    /// than only in `reuse`.
    #[test]
    fn a_live_harness_blocks_the_replacement_it_would_otherwise_get() {
        use crate::image::{Probe, Stamp};
        let fx = fixture();
        let built_as = plan_for(&fx, "claude");
        let asked_for = plan_for(&fx, "opencode");
        let stamp = || Stamp::Read(stamp_of(&built_as));

        let live = crate::persist::socket("s01", "claude");
        let listing = format!("{}\n", live.file_name().unwrap().to_string_lossy());

        let Reuse::Blocked { live, changed } =
            decide("s01", Probe::Listed(listing), stamp, &asked_for).unwrap()
        else {
            panic!("a session with a live harness is reported, not replaced");
        };
        assert_eq!(live, vec!["claude".to_string()]);
        assert!(
            !changed.is_empty(),
            "and it says what it would have changed"
        );
    }

    /// The first bug: `omh s rm` deleted the worktree and left the container
    /// up, and every exec afterwards died on a mount pointing at a directory
    /// that no longer existed. Nothing to compare — it simply cannot be used.
    ///
    /// A constructor rather than `reuse(false, …)`, which took three arguments
    /// it then ignored and let a caller spell `reuse(false, stamp, plan,
    /// &["claude"])` — a live harness discarded and the container replaced
    /// anyway, which is the outcome `Blocked` exists to prevent.
    #[test]
    fn a_container_that_cannot_be_entered_is_replaced_whatever_it_says() {
        assert!(matches!(not_enterable(), Reuse::Restart(_)));
        let Reuse::Restart(why) = not_enterable() else {
            panic!("not enterable is a restart");
        };
        assert!(
            why.iter().any(|w| w.contains("worktree")),
            "and says why, in the words the user reads: {why:?}"
        );
    }

    /// The second: `omh opencode` against a session started by `omh claude`.
    #[test]
    fn a_container_built_for_another_harness_is_replaced() {
        let fx = fixture();
        let want = plan_for(&fx, "opencode");
        let have = stamp_of(&plan_for(&fx, "claude"));
        let Reuse::Restart(why) = reuse(&have, &want, &[]) else {
            panic!("a harness switch needs a new container");
        };
        assert!(
            why.iter().any(|r| r.contains("image")),
            "the user has to be told what moved: {why:?}"
        );
    }

    /// Restarting is cheap — the worktree and branch are on the host — but it
    /// is not free: it kills whatever is running inside. A detached agent
    /// mid-task is exactly the thing `idle::expired` refuses to guess about.
    #[test]
    fn a_container_with_work_running_in_it_is_never_replaced_silently() {
        let fx = fixture();
        let want = plan_for(&fx, "opencode");
        let have = stamp_of(&plan_for(&fx, "claude"));
        let Reuse::Blocked { live, changed } = reuse(&have, &want, &["claude".to_string()]) else {
            panic!("a live harness must block the restart, not be run over");
        };
        assert_eq!(live, vec!["claude"]);
        assert!(
            !changed.is_empty(),
            "and it must still say why: {changed:?}"
        );
    }

    /// A live harness is only a reason to stop when something actually needs
    /// replacing. Relaunching the same harness into the same session is the
    /// ordinary case and must stay silent.
    #[test]
    fn a_live_harness_does_not_block_a_container_that_matches() {
        let fx = fixture();
        let p = plan_for(&fx, "claude");
        assert!(matches!(
            reuse(&stamp_of(&p), &p, &["claude".to_string()]),
            Reuse::Attach
        ));
    }
}
