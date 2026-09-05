//! A session’s life: start it, attach to it, list what is running, stop it.
//!
//! A session is a container, a git worktree and a branch. `crate::container`
//! builds the plan, `crate::session` owns the worktree and `crate::runtime`
//! runs it; this decides which of those a typed command means, and reports
//! what happened.

use crate::adapter::{self, Adapter};
use crate::out;
use crate::profile::{Paths, Profile};
use crate::session::{self, Session};
use crate::{
    ask, auth, base, carry, config, container, detect, editor, idle, image, memory, notice,
    persist, render, report, runtime, settings, shadow, ssh, stack,
};
use anyhow::{Context, Result};
use std::process::Command;

/// The docker half of `container::reuse`: gather the three facts it decides on.
///
/// One exec, not two. Whether the container can be entered and what is running
/// inside it are the same question asked of the same command — and a container
/// that refuses the exec cannot answer the second, which is why an unreadable
/// probe short-circuits to "replace it" rather than to "nothing is running".
///
/// It short-circuits there only for the failure that means it, though. Every
/// other way the exec can fail is a question omh cannot answer, and answering
/// it wrongly costs an agent its turn — so those refuse.
pub(crate) fn reuse_decision(
    backend: &runtime::Backend,
    name: &str,
    plan: &container::Plan,
    session: &Session,
) -> Result<container::Reuse> {
    let probe = backend.exec_args(name, &image::probe_command(), false);
    container::decide(
        &session.id,
        image::container_probe(backend, &probe),
        || image::container_stamp(backend, name),
        plan,
    )
}

/// What became of a container already running under the session's name.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Disposition {
    /// It is this session. Exec into it.
    Attach,
    /// It was not this session, nothing was live inside it, and it is gone.
    /// The launch goes on to start a fresh one.
    Replaced,
}

/// Decide about a running container, and act on the one decision that acts.
///
/// The decision is `container::decide`'s; this is where it meets the runtime.
/// Split out of `session_up` so it can run against a scripted backend: the
/// `attach` defects of the 0.9.0 audit lived exactly here and had nowhere to be
/// tested, because the only way to reach this code was a real launch.
pub(crate) fn reuse_or_replace(
    backend: &runtime::Backend,
    name: &str,
    plan: &container::Plan,
    session: &Session,
    harness: &str,
    ctx: &out::Ctx,
) -> Result<Disposition> {
    match reuse_decision(backend, name, plan, session)? {
        container::Reuse::Attach => Ok(Disposition::Attach),
        container::Reuse::Blocked { live, changed } => anyhow::bail!(
            "session {id} is running {} and cannot be reused for this launch \
             ({})\n  stop it with        omh {id} down\n  \
             or start a fresh one  omh new {}",
            live.join(", "),
            changed.join(", "),
            harness,
            id = session.id,
        ),
        container::Reuse::Restart(why) => {
            // `warn`, not `progress`. This destroys a running container,
            // and `progress` is suppressed entirely under `--json` — so
            // the one destructive act in a launch was the one a script
            // could not see.
            //
            // An earlier draft justified that by "every other outcome of
            // this match reaches both formats: `Attach` returns, `Blocked`
            // bails", and neither half survives checking. `Attach` reaches
            // *neither* format — it returns silently and the next thing
            // said is `announce`, which is gated on `Format::Human`.
            // `Blocked` bails, which a script sees as exit 1 and prose on
            // stderr rather than as a field. The argument for `warn` does
            // not need them: a destructive act should be visible wherever
            // omh can speak at all.
            ctx.warn(&format!(
                "restarting the sandbox for {} — {}",
                session.label(),
                why.join(", ")
            ));
            // `?`, not `let _`. `container_remove` bails with docker's
            // own stderr and its comment explains why — and both callers
            // threw that away. The launch then failed further down against
            // the same sick daemon, and the user read `the container name
            // is already in use` with nothing connecting it to the restart
            // they were just told about.
            image::container_remove(backend, name)
                .with_context(|| format!("replacing the sandbox for {}", session.id))?;
            Ok(Disposition::Replaced)
        }
    }
}

/// Start the session container: clear a stopped one under the name, then run.
///
/// The clear is best-effort — there is usually nothing there — and the run is
/// not: a runtime that refuses to start the container is the launch failing,
/// said with the runtime's own reason.
pub(crate) fn start(
    backend: &runtime::Backend,
    plan: &container::Plan,
    name: &str,
    port: u16,
    pubkey: &str,
    session_id: &str,
) -> Result<()> {
    let _ = image::container_remove(backend, name); // a stopped one blocks --name
    let args = backend.up_args(plan, name, port, pubkey);
    let out = backend.output(&args)?;
    if !out.status.success() {
        anyhow::bail!(
            "starting session {session_id}: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(())
}

/// Bring a session's sandbox up if it is not already. A session is a *running
/// container*, not a launch — that is what lets an editor attach to the same
/// place the agent is working.
// One more than clippy's seven, for the backend. Bundling it with `paths` and
// `profile` into a launch context is the shape Phase 3c's `Resolved` takes;
// until then the parameter list is the honest one.
#[allow(clippy::too_many_arguments)]
pub(crate) fn session_up(
    backend: &runtime::Backend,
    paths: &Paths,
    profile: &Profile,
    adapter: &Adapter,
    session: &Session,
    opts: container::Options,
    // The resolution behind `opts.image` — the recipe *and* the certificate the
    // tag was computed with. Handed in whole rather than derived here, so
    // everything about the layer a launch builds comes from one
    // `crate::cmd::init::sandbox()` call and cannot name a different image than
    // it builds. That split is what let `init` build a layer no launch ever
    // ran, and taking the two apart into separate arguments is how it reopened
    // — this PR read `ca_cert` a second time here — so they arrive together.
    sandbox: &crate::cmd::init::Sandbox,
    ctx: &out::Ctx,
) -> Result<String> {
    let name = paths.container(&session.id);
    let running = crate::cmd::harvest::must_know(
        image::container_running(backend, &name),
        &session.id,
        "start or reuse it",
    )?;

    // Before planning, because the plan mounts the memory server only if a
    // binary exists. Degraded rather than fatal: a session without memory is
    // still a session, and refusing to launch over it would be the tail
    // wagging the dog — the same rule as a capability a harness cannot express.
    //
    // `ensure` is also what *resolves* the path, rather than the caller's
    // earlier `available()`. That ordering is the whole point and it is easy to
    // lose: on a first launch `available()` answers `None` because the binary
    // has not been cross-built yet, `ensure` then builds it, and a plan holding
    // the earlier answer mounts nothing — after printing that it was building
    // the very thing it goes on to ignore. Owning the field here means no
    // caller can sample it too early.
    let mut opts = opts;
    match memory::deliver::ensure(
        backend.program(),
        paths,
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")),
        ctx,
    ) {
        Ok(bin) => opts.memory_bin = Some(bin),
        Err(e) => {
            ctx.warn(&format!("memory server unavailable — {e:#}"));
            opts.memory_bin = None;
        }
    }

    // The account must reach *this* plan: this is the container that actually
    // runs. Building it without credentials is how every session started
    // logged out while `--dry-run` advertised the mounts.
    say_selection(paths, profile, &opts.repo, ctx);
    let plan = container::plan(paths, profile, adapter, session, &[], opts)?;
    plan.validate(&backend.caps())?;

    // The plan is built before this rather than after, because the plan *is*
    // the question: a running container is only this session if it was made
    // from the same one. Cheap — `ensure` above is a path check once the binary
    // is cached, and the staging the plan performs happens every launch anyway.
    if running {
        if let Disposition::Attach =
            reuse_or_replace(backend, &name, &plan, session, &adapter.name, ctx)?
        {
            return Ok(name);
        }
    }

    say_rules(&plan, ctx);
    image::ensure_stack(
        backend,
        adapter,
        &sandbox.recipe(),
        sandbox.ca.as_ref().map(crate::image::Root::pem),
        &paths.repo,
    )?;
    image::ensure_network(backend, &plan.network)?;

    let key = ssh::ensure_key(&paths.keys())?;
    let pubkey = std::fs::read_to_string(key.with_extension("pub"))?;
    let port = ssh::port(&paths.repo_name(), &session.id);

    start(backend, &plan, &name, port, pubkey.trim(), &session.id)?;
    // The session's worktree is not the checkout indexed at init — it holds
    // whatever the agent has since written. Index it now; the Stop hook keeps
    // it current from here.
    let project = base::project_name(&paths.repo_name(), &session.id);
    let _ = Command::new(backend.program())
        .args(backend.exec_args(
            &name,
            &[
                base::GRAPH_BIN.into(),
                "cli".into(),
                "index_repository".into(),
                "--repo-path".into(),
                crate::container_workdir().into(),
                "--name".into(),
                project,
                "--mode".into(),
                "fast".into(),
            ],
            false,
        ))
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();

    Ok(name)
}

/// Which harness `attach` rejoins as, from the session's own record.
///
/// `Harness(n)` is the session's word and is taken as is — a launch already
/// wrote it, and building any other image restarts the container on the wrong
/// harness. `NeverRecorded` is a session from before the record existed, or
/// one `omh s attach` itself made for an editor and never ran a harness in;
/// the host preference is the honest default there, since there is nothing to
/// contradict. `CouldNotTell` is damage — a marker present and unreadable —
/// and is refused rather than papered over with the host default, pointing at
/// `resume`, which can be told the harness outright.
fn harness_for_attach(
    recorded: session::Ran,
    installed: &[String],
    on_host: &dyn Fn(&str) -> bool,
) -> Result<String> {
    match recorded {
        session::Ran::Harness(name) => Ok(name),
        session::Ran::NeverRecorded => detect::preferred_harness(installed, on_host)
            .context("no adapters installed — run `omh init`"),
        session::Ran::CouldNotTell(why) => anyhow::bail!(
            "omh recorded a harness for this session and cannot read it back: {why}\n  \
             omh <id> resume <harness>   rejoin it as that, which rewrites the record"
        ),
    }
}

pub(crate) fn attach(
    cwd: &std::path::Path,
    id: Option<&str>,
    chosen: Option<&str>,
    ctx: &out::Ctx,
) -> Result<()> {
    let paths = Paths::discover(cwd)?;
    let profile = Profile::resolve(&paths);
    let names: Vec<String> = Adapter::load_dir(&paths.adapters())?
        .into_iter()
        .map(|a| a.name)
        .collect();

    // The session, before anything is built for it. `attach` used to pick the
    // *host's* preferred harness here and build that image — so attaching from
    // a machine that prefers claude to a session opencode built stopped the
    // opencode container and started a claude one over the same worktree. The
    // recorded harness is the session's; the host preference is for `omh new`.
    let session = crate::cmd::harvest::existing_session(&paths, id)?;
    let harness = harness_for_attach(
        session::harness_of(&paths.runs(), &session.id),
        &names,
        &|h| runtime::installed(h),
    )?;
    let adapter = Adapter::find(&paths.adapters(), &harness)?;
    let (own, repo) = resolved(&paths)?;
    let ca = crate::image::ca_for(&paths)?;
    let mut sandbox = crate::cmd::init::sandbox(&paths, &adapter, &repo, ca)?;
    // Selected once, here, and handed down: `session_up` used to select its
    // own, so a launch chose a runtime twice and a test could inject neither.
    let backend = runtime::select(&crate::runtime_preference(&paths), &|p| {
        runtime::installed(p)
    })?;
    sandbox.top_up(
        &paths,
        &backend,
        &adapter,
        &profile.sources(adapter::Capability::Hooks)?,
        &own,
        &repo,
        ctx,
    )?;

    std::fs::create_dir_all(paths.worktrees())?;
    // Through `existing_session`, like every other verb under `omh s`.
    //
    // It used to be `session::pick`, which returns a named id **unchecked**,
    // and then `ensure` created the worktree — so `omh s42 attach` built an
    // empty session off the base branch and opened an editor on it, exit 0,
    // reporting `session s42 is up`: the sentence a real rejoin prints. A typo
    // became a session. The comment that stood here claimed the opposite, that
    // attaching to a session which does not exist "is not a thing anyone asks
    // for" — true, and the code did it anyway.
    //
    // `attach` became a session verb in 0.7.0 and did not join the discipline
    // of the group it moved into. `existing_session` is that discipline, and
    // is resolved above so the harness this session ran is known before its
    // image is built.
    session.ensure(&paths.repo, &session::default_branch(&paths.repo))?;
    carry_in(&paths, &session, ctx)?;
    let _ = idle::touch(&paths.runs(), &session.id);

    let configured = crate::policy_value(&paths, "account");
    let account = auth::resolve_for_launch(&paths, &adapter, configured.as_deref())?
        .map(|a| auth::dir(&paths, &adapter.name, &a));
    if let Some(account_dir) = &account {
        auth::prepare(&adapter, account_dir, auth::GUEST_HOME)?;
    }

    // Said here, because `attach` is the one launch path that never said it.
    // `run` carries the drop list in its status line, built from the plan it
    // makes itself; `session_up` builds its own plan and discards it, so
    // `omh s attach` staged a hooks document with hooks removed and reported
    // nothing — and this is the path where it matters most, for the reason
    // `say_selection` gives: it is how you rejoin a session whose setup you
    // have since changed.
    //
    // Through `render::held_back`, so the wording and the set are `init`'s.
    for d in render::held_back(
        &profile.sources(adapter::Capability::Hooks)?,
        &own,
        &repo,
        &sandbox.resolves,
    )? {
        ctx.warn(&format!("`{}` needs {} — held back", d.name, d.wanted));
    }

    session_up(
        &backend,
        &paths,
        &profile,
        &adapter,
        &session,
        container::Options {
            staging: container::Staging::Apply,
            persist: persist::Mode::None,
            tty: false,
            account_dir: account,
            memory_bin: memory::deliver::available(&paths, ctx),
            base: Some(session::default_branch(&paths.repo)),
            omh: own,
            repo,
            image: sandbox.tag.clone(),
            resolves: sandbox.resolves.clone(),
        },
        &sandbox,
        ctx,
    )?;

    // The integration point is a managed SSH config include, not an IDE plugin —
    // that is what keeps every editor working without omh knowing about any.
    let home = dirs::home_dir().context("no home directory")?;
    let alias = ssh::host_alias(&paths.repo_name(), &session.id);
    let key = ssh::ensure_key(&paths.keys())?;
    let blocks: Vec<String> = session::list(&paths.worktrees())
        .into_iter()
        .map(|s| {
            ssh::config_block(
                &ssh::host_alias(&paths.repo_name(), &s),
                ssh::port(&paths.repo_name(), &s),
                &key,
            )
        })
        .collect();
    ssh::write_hosts(&home.join(".ssh/config.d/omh"), &blocks)?;
    ssh::ensure_include(&home.join(".ssh/config"))?;

    let fallback = std::env::var("OMH_EDITOR")
        .or_else(|_| std::env::var("EDITOR"))
        .ok()
        .and_then(|e| {
            let base = std::path::Path::new(&e)
                .file_name()?
                .to_string_lossy()
                .into_owned();
            Some(base)
        });
    let wanted = chosen.map(str::to_string).or(fallback);
    let ed = wanted
        .as_deref()
        .and_then(|n| editor::Editor::find(&paths.editors(), n));

    let editors: Vec<(String, String)> = editor::Editor::load_dir(&paths.editors())?
        .into_iter()
        .map(|e| (e.name.clone(), e.command(&alias).join(" ")))
        .collect();

    // Which editor, if any, actually got a window open. Everything else about
    // the report is the same either way — the URL and the `ssh` line are how
    // you rejoin this session tomorrow, whether or not something opened today.
    let opened_in = match ed {
        // An editor that is not installed is not an error — the URL is still a
        // good answer, and launching nothing silently would not be.
        Some(ed) if runtime::installed(&ed.bin) => {
            let cmd = ed.command(&alias);
            let ok = Command::new(&cmd[0])
                .args(&cmd[1..])
                .status()
                .map(|s| s.success());
            if matches!(ok, Ok(true)) {
                Some(ed.name.clone())
            } else {
                // Remote launches fail for ordinary reasons — missing
                // extension, handshake refused. Saying nothing leaves the user
                // waiting for a window that will never open.
                ctx.warn(&format!("{} did not open the session", ed.name));
                None
            }
        }
        other => {
            if let Some(ed) = other {
                ctx.warn(&format!("`{}` is not installed on this machine", ed.bin));
            } else if let Some(w) = &wanted {
                ctx.warn(&format!("no editor named `{w}` — see `omh info`"));
            }
            None
        }
    };

    ctx.say(&report::Attached {
        session: session.id.clone(),
        url: ssh::url(&alias),
        alias,
        opened_in,
        editors,
    });
    Ok(())
}

/// Stop sessions nobody has used for longer than `policy.idle_timeout`.
///
/// N sessions is N containers — the sprawl `docs/design/risks.md` names. Only
/// the container stops; the worktree and branch survive, so relaunching resumes
/// exactly where you left off.
///
/// Best-effort by design: this runs on the way to starting a session, and a
/// failure to reap must never stop you working.
pub(crate) fn reap_idle(paths: &Paths, launching: &str, ctx: &out::Ctx) {
    let Some(raw) = crate::policy_value(paths, "idle_timeout") else {
        return;
    };
    let Some(timeout) = idle::parse_duration(&raw) else {
        // Say so rather than ignoring silently — a setting that resolves with
        // provenance and then does nothing is exactly what this feature was.
        ctx.warn(&format!(
            "ignoring idle_timeout `{raw}` — expected a duration like 30m, 2h, 90s"
        ));
        return;
    };
    let Ok(backend) = runtime::select(&crate::runtime_preference(paths), &|p| {
        runtime::installed(p)
    }) else {
        return;
    };

    let running: Vec<(String, Option<std::time::SystemTime>)> = session::list(&paths.worktrees())
        .into_iter()
        .filter(|id| {
            crate::cmd::harvest::reapable(&image::container_running(&backend, &paths.container(id)))
        })
        .map(|id| {
            let last = idle::last_used(&paths.runs(), &id);
            (id, last)
        })
        .collect();

    // The liveness probe: a container `exec` listing the dtach sockets, read
    // into Working / Idle / CouldNotTell. Run only for sessions the clock has
    // already flagged, and only Idle is reaped — a live agent or an
    // unanswerable probe keeps its container.
    let live = |id: &str| {
        let probe = backend.exec_args(&paths.container(id), &image::probe_command(), false);
        idle::live_from(id, &image::container_probe(&backend, &probe))
    };
    for id in idle::expired(
        &running,
        timeout,
        std::time::SystemTime::now(),
        launching,
        &live,
    ) {
        match image::container_remove(&backend, &paths.container(&id)) {
            Ok(()) => ctx.progress(&format!(
                "stopped {id} — idle over {raw} (worktree and branch survive)"
            )),
            Err(e) => ctx.warn(&format!("could not stop idle session {id}: {e}")),
        }
    }
}

pub(crate) fn down(
    cwd: &std::path::Path,
    id: Option<&str>,
    all: bool,
    terminal: bool,
    ctx: &out::Ctx,
) -> Result<()> {
    let paths = Paths::discover(cwd)?;
    let backend = runtime::select(&crate::runtime_preference(&paths), &|p| {
        runtime::installed(p)
    })?;
    let ids = match id {
        Some(i) => vec![i.to_string()],
        None => {
            let every = session::list(&paths.worktrees());
            // Nothing to stop is not a question worth asking.
            if !all && !every.is_empty() {
                // On stderr, like every other prompt here: the answer channel
                // belongs to the report, and `omh s down > log` must not eat
                // the question.
                let agreed = terminal
                    && ask::confirm(
                        &format!("stop every sandbox? {} — {}", every.len(), every.join(", ")),
                        &mut std::io::stdin().lock(),
                        &mut std::io::stderr(),
                    )?;
                anyhow::ensure!(
                    agreed,
                    "nothing stopped:\n  omh s01 down       that one\n  omh s down --all   every one of them"
                );
            }
            every
        }
    };
    // Collected, then said once: with no id this is asked about every session,
    // and one `say` per session is one JSON document per session.
    let mut sessions = Vec::new();
    let mut stuck = 0usize;
    let mut unasked = 0usize;
    for i in &ids {
        let name = paths.container(i);
        match image::container_running(&backend, &name) {
            image::Running::No => {
                sessions.push((i.clone(), report::Stopped::WasNotRunning));
                continue;
            }
            // A row in the report, not a warning and a gap. Skipping the push
            // meant `down` over an unreachable daemon printed `no sessions` on
            // **stdout** and `"sessions": []` in JSON — a false all-clear on
            // the answer channel, which is the thing this whole change is
            // about, one report struct over.
            //
            // Counted apart from `stuck` too: omh never asked this one to
            // stop, so *would not stop* is a claim it cannot make.
            image::Running::Unknown(why) => {
                unasked += 1;
                ctx.warn(&format!("could not tell whether {i} is running: {why}"));
                sessions.push((i.clone(), report::Stopped::CouldNotTell(why)));
                continue;
            }
            image::Running::Yes => {}
        }
        match image::container_remove(&backend, &name) {
            Ok(()) => sessions.push((i.clone(), report::Stopped::Yes)),
            // Reported and carried on rather than returned: one container that
            // will not go must not hide the ones that did. It still decides
            // the exit code below — a caller whose JSON says nothing stopped
            // needs the status to agree.
            Err(e) => {
                stuck += 1;
                ctx.warn(&format!("{i} is still running: {e:#}"));
            }
        }
    }
    ctx.say(&report::Down { sessions });
    // Both are failures and they are different sentences. The old text called
    // a session omh never asked about one that "would not stop".
    anyhow::ensure!(
        stuck == 0 && unasked == 0,
        "{}",
        [
            (stuck, "would not stop"),
            (unasked, "could not be asked — the runtime did not answer"),
        ]
        .iter()
        .filter(|(n, _)| *n > 0)
        .map(|(n, what)| format!("{n} session{} {what}", if *n == 1 { "" } else { "s" }))
        .collect::<Vec<_>>()
        .join("; ")
    );
    Ok(())
}

pub(crate) fn sessions_ls(cwd: &std::path::Path, only: Option<&str>, ctx: &out::Ctx) -> Result<()> {
    let paths = Paths::discover(cwd)?;
    // Validated before anything is read, so an id nothing created fails the
    // way it fails for every other verb rather than listing nothing and
    // looking like an answer.
    if let Some(id) = only {
        crate::cmd::harvest::existing_session(&paths, Some(id))?;
    }
    // Said once, about the machine, rather than once per row. The `running`
    // column renders a `None` as an absence — nobody asked — and *why* nobody
    // asked is one fact, not N.
    let backend = match runtime::select(&crate::runtime_preference(&paths), &|p| {
        runtime::installed(p)
    }) {
        Ok(backend) => Some(backend),
        Err(e) => {
            ctx.warn(&format!("omh cannot say which sandboxes are up: {e:#}"));
            None
        }
    };
    let base = session::default_branch(&paths.repo);

    // What each session is changing, asked **once** and used twice. The count
    // `work_state` renders and the paths the overlap section names are the
    // same answer: read separately they were two subprocesses per session and,
    // worse, two snapshots — a live agent writes between them, so one listing
    // could report `0 uncommitted` beside a collision on a file that session
    // had just stashed.
    let mut changed: Vec<(String, Vec<String>)> = Vec::new();
    let mut unreadable: Vec<String> = Vec::new();
    let sessions: Vec<report::Session> = session::list(&paths.worktrees())
        .into_iter()
        .map(|id| {
            let sess = Session::new(&paths.worktrees(), id.clone());
            let touched = sess.changed();
            match &touched {
                Ok(touched) => changed.push((id.clone(), touched.clone())),
                // Carried, not dropped. A session omh cannot read contributes
                // no paths, so it silently belongs to no collision — and *no
                // overlap line* is exactly how "no collisions" is rendered.
                // `work_state` renders the same failure as `Work::Unknown` one
                // column over; the two must not disagree about whether it is
                // worth mentioning.
                Err(_) => unreadable.push(id.clone()),
            }
            report::Session {
                // `None` for *nobody asked*, which is what no backend at all
                // means, and `Unknown` inside it for *asked and could not
                // tell*. The same two-level shape `work` uses one column over,
                // and for the same reason it gives: the mistake becomes
                // unspellable rather than merely absent.
                running: backend.as_ref().map(|b| {
                    let asked = image::container_running(b, &paths.container(&id));
                    // Say why, which the first version of this did not — it
                    // built the reason, carried it through two layers and
                    // dropped it, while both docs promised it reached stderr.
                    // The same defect as the `.ok()` thirteen lines below,
                    // introduced in the change that fixed that one.
                    if let image::Running::Unknown(why) = &asked {
                        ctx.warn(&format!(
                            "could not tell whether {id}'s sandbox is running: {why}"
                        ));
                    }
                    asked
                }),
                label: sess.label().to_string(),
                work: Some(work_state(
                    &sess,
                    &paths.repo,
                    &base,
                    touched.as_ref().ok().map(Vec::len),
                )),
                // Not `.ok()`. The dashboard renders a failed count as a
                // question rather than as zero, which is only half the rule —
                // the other half is that the reason exists and git already
                // said it. `changed()` ten lines up pushes to `unreadable` for
                // the same class of failure, and `log` prints git's own words;
                // this was the one that threw them away.
                behind: match sess.behind(&paths.repo, &base) {
                    Ok(n) => Some(n),
                    Err(e) => {
                        ctx.warn(&format!(
                            "could not tell how far behind {base} {id} is: {e:#}"
                        ));
                        None
                    }
                },
                id,
            }
        })
        .collect();

    // Every session is read even when one is asked for, and that is not
    // waste: a collision is a fact about *two* sessions, so the paths of the
    // others are what make "s01 and s03 both change src/render.rs" sayable at
    // all. What narrows is the display.
    //
    // (An earlier note claimed focusing would be cheaper. It is not, for this
    // reason.)
    let overlaps = report::overlaps(&changed);
    let (sessions, overlaps) = match only {
        None => (sessions, overlaps),
        Some(id) => {
            let rows: Vec<_> = sessions.into_iter().filter(|s| s.id == id).collect();
            // `existing_session` said this id was there, and the filter says
            // it is not. The two ask different questions — one whether the
            // path exists, the other whether it is a directory — and between
            // them sit every subprocess this function runs, so a worktree
            // removed in another terminal lands here too. Rendering it would
            // print `no sessions`: exit 0, and byte-identical to a clean
            // checkout. Refusing says which of the two we could not reconcile.
            anyhow::ensure!(
                !rows.is_empty(),
                "{id} was there when omh looked and is not there now — \
                 removed while this ran? `omh s` lists what is left"
            );
            (
                rows,
                // Kept when they name this session, dropped otherwise — a
                // collision between two other sessions is not this one's
                // business, and one involving it is the most useful line on
                // the screen.
                overlaps
                    .into_iter()
                    .filter(|o| o.sessions.iter().any(|s| s == id))
                    .collect(),
            )
        }
    };

    ctx.say(&report::Sessions {
        sessions,
        // Not swept when one session was asked for. A leftover is an id with
        // no worktree, and the focused id was proved to have one, so a
        // focused sweep can only ever turn up other people's — guaranteed
        // off-topic rather than merely usually. The overlap section is the
        // opposite case and stays: a collision *is* a fact about this
        // session. Skipping it also saves the sweep's `ps` and its walks.
        leftovers: match only {
            // The list half; `omh s` already had the reason on stderr.
            None => leftovers(&paths, backend.as_ref(), ctx).0,
            Some(_) => Vec::new(),
        },
        overlaps,
        // Deliberately *not* narrowed. A session omh could not read is why
        // the overlap answer above may be short a line, and that is a fact
        // about the focused session even though the id named is not.
        unreadable,
        base,
    });
    Ok(())
}

/// Session ids with a container, a run directory or a sandbox repository but
/// no worktree.
///
/// Invisible until now, and not merely untidy: an orphan container holds a
/// session id, and the next session to take that id used to exec straight into
/// it. That was the mount-namespace failure. `s rm` cleans up after itself now,
/// so this reports what older versions left — and anything a hand
/// `git worktree remove` strands from here on.
///
/// A run directory counts only when it carries the marker `idle::touch` writes.
/// `omh doctor` and `omh auth` stage into the same tree under their own names,
/// and neither is a session anybody could resume or would want reported.
pub(crate) fn leftovers(
    paths: &Paths,
    backend: Option<&runtime::Backend>,
    ctx: &out::Ctx,
) -> (Vec<String>, Option<String>) {
    // **Why omh could not look, when it could not.** The warning goes to
    // stderr, which `omh s` wants — but `omh doctor` puts this in a report, and
    // a row that says "none" because nothing was listed is the collapse the
    // whole leftovers row exists to avoid.
    let mut unchecked: Option<String> = None;
    let live = session::list(&paths.worktrees());
    // A sandbox repository with no worktree — [risks](docs/design/risks.md) 8c.
    // The most valuable orphan of the three: a container is re-creatable and a
    // run directory holds a timestamp, while this holds every commit the agent
    // made and nothing points at it. `omh <id> rm` clears it, and since #58
    // says what it would take with it first.
    // `NotFound` is the ordinary "no sandbox has ever been built here". Any
    // other failure is omh being unable to look, and an empty `leftovers`
    // prints *nothing at all* — byte for byte what a clean checkout prints. Of
    // the three orphans this hunts, the repository is the one that holds work.
    let mut found: Vec<String> = match std::fs::read_dir(paths.shadows()) {
        Ok(entries) => entries
            .flatten()
            .filter_map(|e| {
                let name = e.file_name().to_string_lossy().into_owned();
                name.strip_suffix(".git").map(str::to_string)
            })
            .collect(),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(e) => {
            ctx.warn(&format!(
                "omh could not read {}, so orphaned sandbox repositories went unchecked: {e}",
                paths.shadows().display()
            ));
            Vec::new()
        }
    };
    found.extend(
        std::fs::read_dir(paths.runs())
            .into_iter()
            .flatten()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|id| idle::last_used(&paths.runs(), id).is_some()),
    );

    if let Some(backend) = backend {
        let prefix = paths.container("");
        // **Could not look is not "none".** This swallowed its failure, so a
        // daemon that was down reported *fewer* leftovers rather than saying
        // it had not looked — the same collapse `Running` exists to prevent,
        // in the function whose whole job is to notice what is left behind.
        match backend.output(&["ps", "-a", "--format", "{{.Names}}"]) {
            Ok(out) if out.status.success() => found.extend(
                String::from_utf8_lossy(&out.stdout)
                    .lines()
                    .filter_map(|n| n.trim().strip_prefix(&prefix))
                    .map(str::to_string),
            ),
            Ok(out) => {
                let why =
                    crate::image::unreadable(&String::from_utf8_lossy(&out.stderr), &out.status);
                ctx.warn(&format!(
                    "omh could not list containers, so orphaned sandboxes went \
                     unchecked: {why}"
                ));
                unchecked = Some(why);
            }
            Err(e) => {
                ctx.warn(&format!(
                    "omh could not list containers, so orphaned sandboxes went \
                     unchecked: {e}"
                ));
                unchecked = Some(e.to_string());
            }
        }
    }

    found.retain(|id| !live.contains(id));
    found.sort();
    found.dedup();
    (found, unchecked)
}

/// Where a session is in the cycle, phrased as the next thing to do about it.
///
/// Ordered most-actionable first, and deliberately one answer rather than a
/// tally: `omh s` is read at a glance, and a session with uncommitted work needs
/// committing whatever else is also true of it.
pub(crate) fn work_state(
    session: &Session,
    repo: &std::path::Path,
    base: &str,
    uncommitted: Option<usize>,
) -> report::Work {
    use report::Work;

    // A git that cannot answer is never rendered as an answer. Every accessor
    // below runs through the worktree's `.git` pointer, which goes stale when a
    // checkout moves and is already handled as a real case by `Session::remove`
    // — and a blank column reads as "nothing here" for a session that may be
    // holding a day of work the user is about to `s rm`.
    // The count comes from the caller, which already asked. `None` is the same
    // failure this used to discover for itself.
    let (uncommitted, unpushed) = match (uncommitted, session.unpushed()) {
        (Some(uncommitted), Ok(unpushed)) => (uncommitted, unpushed),
        _ => return Work::Unknown,
    };

    if let n @ 1.. = uncommitted {
        return Work::Uncommitted(n);
    }
    match unpushed {
        Some(n @ 1..) => Work::ToPush(n),
        // Nothing origin does not already have. Report the name it went out
        // under, which is what you would look for in a list of PRs — `omh/s01`
        // is not a name anybody searches for.
        Some(_) => match session.published_as() {
            Ok(Some(target)) => Work::Published(target),
            Ok(None) => Work::Clean,
            Err(_) => Work::Unknown,
        },
        // Never pushed, which is not the same as nothing to push: this is the
        // state the loop passes through every time, between `s commit` and the
        // first `s push`. Measured against the base branch instead, because a
        // blank here reads as a session nobody touched.
        None => match session.commits(repo, base) {
            Ok(0) => Work::Clean,
            Ok(n) => Work::ToPush(n),
            // The same rule as the accessors above: a git that cannot answer is
            // never rendered as an answer. `Clean` here would read as a session
            // holding nothing, over a branch nobody can count.
            Err(_) => Work::Unknown,
        },
    }
}

/// Say what composing the project's rules turned up, if anything.
///
/// Called from every path that builds a plan, not just `run`: `attach` and
/// `doctor` compose the same document, and a fallback announced on one path in
/// three is the same silence the notice exists to break. Only when there is
/// something to say — a line printed every launch is a line nobody reads.
pub(crate) fn say_rules(plan: &container::Plan, ctx: &out::Ctx) {
    for notice in plan.rules.notices() {
        ctx.warn(&notice.to_string());
    }
}

/// What the launcher noticed about this repo's hooks: which ones it has, which
/// are new or changed, and where detection and the directory disagree.
///
/// Reported on every launch including a dry run — a dry run is exactly when you
/// want to be told what a launch would hand your agent. The returned `Record`
/// is the *other* half: committing it is what spends the "new or changed"
/// call-out, so only a session that actually started may do it.
///
/// Never fatal. A repo whose hook drift cannot be computed is still a repo you
/// can work in; and an unreadable hooks directory stops the launch anyway, in
/// `render::merge_hooks`, which is where it should.
pub(crate) fn say_hooks(paths: &Paths, ctx: &out::Ctx) -> Option<notice::Record> {
    // An unreadable stacks directory is the same class of non-fatal as the rest
    // of this function: it costs the drift report, not the session. Reported
    // and withdrawn, never defaulted to empty — `notice::hooks` reads "no
    // definitions" as "no stack answers to that name", so an empty list does
    // not weaken the report, it inverts it and prints the inversion in omh's
    // own voice.
    let defs = match stack::load_all(&paths.stacks(), &paths.repo_stacks()) {
        Ok(defs) => defs,
        Err(e) => {
            ctx.warn(&format!(
                "could not read your stacks, so this repo's hooks went unchecked — {e:#}"
            ));
            return None;
        }
    };
    // The same withdrawal for the same reason: which ecosystems are covered and
    // what each hook file claims both come from reading the hook directories,
    // and a report built on half of that is a wrong report rather than a
    // shorter one.
    // Same withdrawal for the same reason: what each hook file claims comes
    // from reading the hook directories, and a drift report built on half of
    // that is a wrong report rather than a shorter one.
    let dirs = match Profile::resolve(paths).sources(adapter::Capability::Hooks) {
        Ok(dirs) => dirs,
        Err(e) => {
            ctx.warn(&format!("could not read your hooks — {e:#}"));
            return None;
        }
    };
    let declared = match render::declared_stacks(&dirs) {
        Ok(declared) => declared,
        Err(e) => {
            ctx.warn(&format!(
                "could not read your hooks, so drift went unchecked — {e:#}"
            ));
            return None;
        }
    };
    let detected = stack::detected(&defs, &paths.repo);
    match notice::hooks(paths, &detected, &declared) {
        Ok((notices, record)) => {
            for notice in notices {
                ctx.warn(&notice.to_string());
            }
            Some(record)
        }
        Err(e) => {
            ctx.warn(&format!("could not check this repo's hooks — {e:#}"));
            None
        }
    }
}

/// Say what this repo is not using from your catalogue, and what it named that
/// nothing answers to.
///
/// Called from **every path that builds a plan**, which is the rule `say_rules`
/// states and this broke on arrival: it was wired into `run` alone, so `attach`
/// and `doctor` composed the same profile and said nothing. `attach` is the path
/// where it matters most — it is how you rejoin a session that staged the
/// selection you have since changed.
///
/// Beside `say_hooks` and on the same terms otherwise: reported on every launch
/// including a dry run, never fatal. A selection omh cannot compute is not a
/// reason to refuse a session — and it cannot be one, because the report exists
/// to cover a silence rather than to guard anything.
pub(crate) fn say_selection(
    paths: &Paths,
    profile: &Profile,
    repo: &settings::RepoPolicy,
    ctx: &out::Ctx,
) {
    // Resolved here rather than inside `notice`: which ecosystems this repo is
    // takes the stack definitions and the checkout, and a report module that
    // read those would be deciding what it is meant to describe.
    let applicable = match crate::cmd::catalogue::catalogue_lists(paths) {
        Ok(lists) => lists,
        Err(e) => {
            ctx.warn(&format!("could not check what this repo uses — {e:#}"));
            return;
        }
    };
    match notice::selection(profile, &repo.selection, &applicable) {
        Ok(notices) => {
            for notice in notices {
                ctx.warn(&notice.to_string());
            }
        }
        Err(e) => ctx.warn(&format!("could not check what this repo uses — {e:#}")),
    }
}

/// Mark this repo's hooks as seen, now that a session is actually running.
///
/// Deliberately after the container is up rather than beside the report. The
/// snapshot is what makes "new or changed" fire exactly once, so writing it
/// from a launch that then died — Docker not running, an image that would not
/// build — spent the one notification about somebody else's executable content
/// changing under you, and the retry was silent. A dry run never gets here at
/// all, which is the other half of the same rule.
pub(crate) fn remember_hooks(record: Option<notice::Record>, ctx: &out::Ctx) {
    if let Some(record) = record {
        if let Err(e) = record.commit() {
            // The check succeeded and its notices are already printed; only the
            // bookkeeping failed. Saying "could not check" would send the user
            // looking at their hooks instead of at `~/.omh/run`.
            ctx.warn(&format!("this repo's hooks were not recorded — {e:#}"));
        }
    }
}

/// Copy the checkout's untracked essentials into a worktree, and say what
/// happened — a `.env` you thought you were carrying and are not is exactly the
/// failure that wastes an hour inside the sandbox.
pub(crate) fn carry_in(paths: &Paths, session: &Session, ctx: &out::Ctx) -> Result<()> {
    // The rules themselves are mounted, not written here — this covers the
    // empty placeholder each mount lands on, and any backend that cannot mount
    // a single file. It must run before `plan` places those placeholders.
    carry::hide_staged_rules(&session.worktree)?;

    let patterns = config::policy_list(paths, "carry_in");
    if patterns.is_empty() {
        return Ok(());
    }
    for item in carry::apply(&paths.repo, &session.worktree, &patterns)? {
        match item.action {
            // What was carried is progress, not a warning: it is the launcher
            // saying what it did, and it happens on every normal launch.
            // A directory is carried and *not* protected: `stage_for_mount`
            // mounts files only, so `git clean -fdx` in the sandbox removes a
            // carried directory and everything in it — silently, and with a
            // success code. Said here because `carried certs/` and `carried
            // .env` are otherwise the same sentence for two different fates,
            // and only one of them survives the agent tidying up.
            carry::Action::Copied | carry::Action::Refreshed
                if paths.repo.join(item.path.trim_end_matches('/')).is_dir() =>
            {
                ctx.warn(&format!(
                    "carried {} — a directory, so `git clean` in the sandbox can \
                     still remove it. Carrying the files individually keeps them.",
                    item.path
                ));
            }
            carry::Action::Copied => ctx.progress(&format!("carried {}", item.path)),
            carry::Action::Refreshed => ctx.progress(&format!("refreshed {}", item.path)),
            // The mistake, named where it is made rather than three commands
            // later at `s commit`. `carry_in` is for what a worktree does not
            // get; a tracked file is already on the branch.
            carry::Action::AlreadyTracked => ctx.warn(&format!(
                "carry_in lists {} — git already tracks it, so the worktree has it \
                 already. Not carried; drop it with `omh set carry_in`.",
                item.path
            )),
            carry::Action::Missing => ctx.warn(&format!(
                "carry_in lists {} — not in this checkout",
                item.path
            )),
            carry::Action::Unchanged => {}
        }
    }

    // Said at launch as well as at harvest, because this is the moment the
    // user can still do something about it — carry the file under a name the
    // scan can read, or accept that its path is its only protection. By
    // harvest the commits already exist.
    //
    // `Missing` is dropped here and only here: `Action::Missing` above says
    // the same thing about the same path a few lines earlier, and one gap
    // reported twice in one launch reads as two problems.
    let unscannable: Vec<_> = shadow::unscannable(&paths.repo, &patterns)?
        .into_iter()
        .filter(|u| u.why != shadow::Unreadable::Missing)
        .collect();
    crate::cmd::harvest::say_what_went_unscanned(&unscannable, ctx);
    Ok(())
}

pub(crate) fn run(
    cwd: &std::path::Path,
    argv: &[String],
    // Named individually rather than handed the whole `Cli`: a function
    // holding one can read `cli.session` where the dispatch scan cannot see
    // it, which is how `run` came to do it in the first place.
    //
    // One parameter, not two. `Start` carries the id when there is one, so
    // there is no second argument that could disagree with it.
    start: session::Start<'_>,
    dry_run: bool,
    ctx: &out::Ctx,
) -> Result<()> {
    let paths = Paths::discover(cwd)?;
    let name = &argv[0];
    let harness = name.clone();

    let adapter =
        Adapter::find(&paths.adapters(), name).map_err(|e| crate::unknown_tool(&paths, name, e))?;
    let profile = Profile::resolve(&paths);

    // A dry run must leave no trace: no branch, no worktree, no staged files.
    // Which identity this session runs as. Ambiguity is an error rather than a
    // guess: silently using the wrong account is expensive and invisible.
    let configured = crate::policy_value(&paths, "account");
    let account = auth::resolve_for_launch(&paths, &adapter, configured.as_deref())?
        .map(|a| auth::dir(&paths, name, &a));
    if let Some(account_dir) = &account {
        // The mountpoints have to exist before docker binds over them.
        auth::prepare(&adapter, account_dir, auth::GUEST_HOME)?;
    }

    // Always the trunk, never wherever HEAD happens to be: a session started on
    // a feature branch produces a diff against the wrong baseline. You attach to
    // a session, not to a branch — choosing a base was a knob nobody needed.
    //
    // Resolved before the options rather than beside the session, because the
    // plan needs it too: it is where the project's own rules come from when the
    // worktree has none of its own.
    let base = session::default_branch(&paths.repo);
    let (own, repo) = resolved(&paths)?;
    let ca = crate::image::ca_for(&paths)?;
    let mut sandbox = crate::cmd::init::sandbox(&paths, &adapter, &repo, ca)?;
    // Not on a dry run, which promises to leave no trace: topping up starts a
    // container and writes `~/.omh/facts.json`. What is already cached is used,
    // so the plan it prints is the plan a real launch would build from the same
    // knowledge.
    // Selected once, here, and handed down — to `top_up`, to the plan's
    // validation, and to `session_up`, which used to select its own. A dry run
    // selects too: the plan it prints is validated against the backend's
    // capabilities and spelled in the backend's argv.
    let backend = runtime::select(&crate::runtime_preference(&paths), &|p| {
        runtime::installed(p)
    })?;
    if !dry_run {
        sandbox.top_up(
            &paths,
            &backend,
            &adapter,
            &profile.sources(adapter::Capability::Hooks)?,
            &own,
            &repo,
            ctx,
        )?;
    }

    let opts = container::Options {
        // A dry run must leave no trace: no branch, no worktree, no staged files.
        staging: if dry_run {
            container::Staging::Skip
        } else {
            container::Staging::Apply
        },
        persist: crate::policy_value(&paths, "persistence")
            .as_deref()
            .unwrap_or("dtach")
            .parse()?,
        tty: true,
        account_dir: account,
        memory_bin: memory::deliver::available(&paths, ctx),
        base: Some(base.clone()),
        omh: own,
        repo,
        image: sandbox.tag.clone(),
        resolves: sandbox.resolves.clone(),
    };

    std::fs::create_dir_all(paths.worktrees())?;
    if let session::Start::Named(explicit) = start {
        session::validate_id(explicit)?;
    }
    let id = session::pick(&paths.worktrees(), start);
    let session = Session::new(&paths.worktrees(), id);
    if opts.staging == container::Staging::Apply {
        session.ensure(&paths.repo, &base)?;
        carry_in(&paths, &session, ctx)?;
        // Reap before starting another container, and record that this one is
        // in use so it is not reaped by the next launch.
        reap_idle(&paths, &session.id, ctx);
        let _ = idle::touch(&paths.runs(), &session.id);
    }

    let plan = container::plan(
        &paths,
        &profile,
        &adapter,
        &session,
        &argv[1..],
        opts.clone(),
    )?;

    plan.validate(&backend.caps())?;

    say_rules(&plan, ctx);
    say_selection(&paths, &profile, &opts.repo, ctx);
    let hooks_seen = say_hooks(&paths, ctx);

    // Without the `omh: ` prefix, which `Ctx` now owns: the launch line is a
    // diagnostic and goes through the same voice as every other one, so it
    // paints and prefixes the same way.
    let status_line = match plan.degradation() {
        Some(d) => format!("{} on {} — {d}", adapter.name, session.label()),
        None => format!("{} on {}", adapter.name, session.label()),
    };

    if dry_run {
        // What the agent is given, read off the plan's own mounts — so the
        // report cannot describe a launch other than the one that would happen.
        let mut reads: Vec<(String, String)> = Vec::new();
        if let Some(name) = &plan.rules.composed {
            reads.push((
                "rules".to_string(),
                format!("composed with this project's {name}"),
            ));
        }
        // The names, not a count of mounts. Counting mounts said `skills 2
        // mounted` on a repo using no skills at all — each capability takes a
        // layer mount and a rendered one, so the number measured omh's
        // plumbing rather than anything the agent gets. This is the same
        // answer `omh info --repo` gives, from the same function.
        for u in
            crate::cmd::settings::using_here(&paths, &base::Manifest::load_dir(&paths.base())?)?
        {
            let summary = u.summary();
            if summary != "nothing" {
                reads.push((u.capability, summary));
            }
        }
        for (cap, n) in &plan.dropped {
            reads.push((
                cap.to_string(),
                format!("{n} dropped — {} cannot take them", adapter.name),
            ));
        }

        // Everything omh mounts is read-only but these, so naming them names
        // the whole of what a session can reach. The worktree *is* `/work`, so
        // it is named once, by the path you would go and look at.
        let workdir = crate::container_workdir();
        let writes: Vec<String> = std::iter::once(format!(
            "{}  — this session's worktree, as {workdir}",
            session.worktree.display()
        ))
        .chain(
            plan.mounts
                .iter()
                .filter(|m| !m.read_only && m.guest != std::path::Path::new(workdir))
                .map(|m| m.guest.display().to_string()),
        )
        .collect();

        ctx.say(&report::DryRun {
            status: status_line,
            worktree: session.worktree.display().to_string(),
            image: plan.image.clone(),
            network: plan.network.clone(),
            reads,
            writes,
            argv: std::iter::once(backend.program().to_string())
                .chain(backend.args(&plan))
                .collect(),
        });
        return Ok(());
    }

    // The session is a running container. Exec into it rather than starting a
    // throwaway, so MCP daemons stay warm and `omh s attach` has something to
    // attach to.
    //
    // "Many harnesses take turns inhabiting it" is what this comment used to
    // claim, and it was not true: an image is built per harness, so the second
    // harness execed a binary the image does not contain. `session_up` restarts
    // on that mismatch now — a few seconds, not instant. Making it instant again
    // means one image carrying every installed harness.
    let name = session_up(
        &backend,
        &paths,
        &profile,
        &adapter,
        &session,
        container::Options {
            tty: false,
            ..opts.clone()
        },
        &sandbox,
        ctx,
    )?;
    // The container is up, so the launch happened and the call-out is spent.
    //
    // The harness record is written here rather than beside `last-used`, and
    // that is the difference between a note and a claim. Written earlier, a
    // launch that failed at `runtime::select`, at `plan.validate`, or on a
    // session already running something else still rewrote it — so
    // `omh s01 resume opencode` that never started anything left s01 recorded
    // as opencode, and the next bare `resume` rejoined a claude worktree as
    // opencode. That is the exact harm the refusal for an unrecorded session
    // exists to prevent, produced by the thing meant to prevent it.
    //
    // A dry run returns before this, so it still leaves no trace.
    if let Err(e) = session::remember_harness(&paths.runs(), &session.id, &harness) {
        ctx.warn(&format!(
            "could not record that {} ran {harness}: {e} — `omh {} resume` \
             will not be able to rejoin it",
            session.id, session.id
        ));
    }
    remember_hooks(hooks_seen, ctx);
    ctx.announce(&status_line);
    let status = Command::new(backend.program())
        .args(backend.exec_args(&name, &plan.argv, true))
        .status()?;
    // `omh s01 diff`, not `omh diff`. There is no top-level `diff` — the name
    // is not a command, so it comes
    // back as ``unknown harness `diff` ``. This line has been wrong since it
    // was written, in two different ways: it named a positional that the
    // session prefix has since deleted, so `the_lines_omh_prints_are_lines_
    // omh_accepts` now reads it, and would have caught both.
    ctx.hint(&format!("\nreview with  omh {} diff", session.id));
    std::process::exit(status.code().unwrap_or(1));
}

/// The two things a launch needs from outside the plan: what omh contributes,
/// and what this repo decided.
///
/// Resolved by the caller of `container::plan` rather than inside it, the rule
/// `memory_bin` and `base` already follow — the manifest is a file, and a probe
/// inside `plan` is a probe no test can reach.
///
/// Returned as a pair rather than merged. They arrive together and travel
/// together, which is exactly what made one struct tempting, but "omh generated
/// this" and "this repo asked for this" are the two answers `omh why` exists to
/// keep apart — and a type that holds both cannot help blurring them.
pub(crate) fn resolved(paths: &Paths) -> Result<(base::Own, settings::RepoPolicy)> {
    let manifest = base::Manifest::load_dir(&paths.base())?;
    let repo = settings::resolve(paths, &manifest)?;
    // What the catalogue still declares, so removing a server takes its feature
    // with it. `omh settings mcp rm codegraph` edits `mcp.json` and nothing
    // else, so this read is where that instruction is kept or broken.
    let installed = config::servers(paths)?.into_iter().map(|s| s.key).collect();
    Ok((base::own(&manifest, &repo.off, &installed)?, repo))
}

pub(crate) fn rm(
    cwd: &std::path::Path,
    id: &str,
    consent: crate::cmd::harvest::Consent,
    ctx: &out::Ctx,
) -> Result<()> {
    session::validate_id(id)?;
    let paths = Paths::discover(cwd)?;
    let session = Session::new(&paths.worktrees(), id.to_string());

    // Before anything is taken down, because everything below this line is
    // irreversible and the first of them is the container.
    //
    // The last piece of [risks](docs/design/risks.md) 2c. The branch survives a
    // removal and the worktree's files were on disk until this ran — but the
    // agent's own commits live only in the sandbox's repository, and `reap`
    // deletes it. After a `reset --hard` in the sandbox those were the only
    // copies there ever were. omh could not ask this question until `log`
    // learned to count them.
    // Not `.unwrap_or(0)`, and not `?` either. A count omh could not take is
    // not a count of none — but it is also not a reason `--force` cannot get
    // past, which is what `?` made it: an orphaned directory that is not a
    // repository at all could no longer be cleaned up by the one command that
    // exists to clean it up.
    //
    // So it joins `crate::cmd::harvest::AtStake::Unknown`, which has had exactly this shape and
    // exactly this escape since #58.
    let snapshots =
        match shadow::Shadow::new(&paths.shadows(), &session.id).turns(&session.worktree) {
            Ok(None) => crate::cmd::harvest::Snapshots::None,
            Ok(Some(n)) => crate::cmd::harvest::Snapshots::Kept(n),
            Err(e) => crate::cmd::harvest::Snapshots::Unreadable(format!("{e:#}")),
        };
    if let Some(note) = crate::cmd::harvest::may_remove(
        &paths,
        &session,
        snapshots,
        consent,
        &mut std::io::stdin().lock(),
        &mut std::io::stderr(),
    )? {
        ctx.warn(note.trim());
    }

    // Drop the graph with the code it describes, while the container is still
    // around to do it. Otherwise the index outlives the worktree forever.
    //
    // Then take the container itself down. A session is the container *and* the
    // worktree, and removing only the worktree leaves a half that can never be
    // reached again: the bind mount still points at the deleted directory, the
    // next launch recreates it at a new inode the mount does not follow, and
    // `session_up` — seeing a container that is up — execs into it and gets
    // "current working directory is outside of container mount namespace root"
    // for every command from then on. Nothing else ever removes it.
    // What actually happened, part by part. The report below is built from
    // these rather than asserting a list of four removals it never checked —
    // which is what it did, including in runs whose own warnings said the
    // opposite two lines earlier.
    let mut went: Vec<String> = Vec::new();
    let mut unreached: Vec<String> = Vec::new();
    match runtime::select(&crate::runtime_preference(&paths), &|p| {
        runtime::installed(p)
    }) {
        Err(e) => {
            // Was `if let Ok(_)`, so this fell through in silence and the
            // report went on to claim the container had gone.
            ctx.warn(&format!(
                "no container runtime omh can use, so {id}'s container and graph \
                 entry were left alone: {e:#}"
            ));
            unreached.push("the container and its graph entry".to_string());
        }
        Ok(backend) => {
            let name = paths.container(id);
            // Best-effort already — `let _` below says so — and *could not tell*
            // joins *not running* because the exec would fail against a runtime
            // that will not answer anyway. Failing a removal over a tidy-up nobody
            // asked for would be the tail wagging the dog.
            //
            // An earlier version of this comment claimed the graph entry is
            // dropped on the next launch that reuses the id. It is not:
            // `drop_graph_command` has exactly one caller, this one. What is
            // skipped here is skipped for good, so it is said rather than
            // swallowed.
            let up = image::container_running(&backend, &name);
            if let image::Running::Unknown(why) = &up {
                ctx.warn(&format!(
                    "could not tell whether {id}'s sandbox was up, so its graph entry \
                 was left behind: {why}"
                ));
                unreached.push("its graph entry".to_string());
            }
            if matches!(up, image::Running::Yes) {
                let project = base::project_name(&paths.repo_name(), id);
                let _ = Command::new(backend.program())
                    .args(backend.exec_args(&name, &base::drop_graph_command(&project), false))
                    .output();
                went.push("its graph entry".to_string());
            }
            // Best-effort: a container that was never started has nothing to
            // remove, and that must not stop the worktree from going.
            // Warned rather than swallowed, and the line above already warns
            // about the weaker failure — not being able to *tell* whether it was
            // up. A removal that fails leaves a live container bound to a worktree
            // this function is about to delete, which manufactures exactly the
            // unenterable state `Probe::NotEnterable` exists for. This function's
            // own doc names `omh s rm` as the historical cause of it.
            match image::container_remove(&backend, &name) {
                Ok(()) => went.push("the container".to_string()),
                Err(e) => {
                    ctx.warn(&format!(
                        "{id}'s container would not stop, and its worktree is going: it is left \
                     running against a directory that will not be there. `docker rm -f {name}` \
                     clears it — {e:#}"
                    ));
                    unreached.push("the container".to_string());
                }
            }
        }
    }

    // The third thing a session owns. Staging is re-rendered on every launch so
    // leaving it costs nothing that breaks — but the `last-used` marker beside
    // it is what says a session ran here, and a marker with no session behind it
    // is how `omh s` learns to report a leftover that is not there any more.
    match std::fs::remove_dir_all(paths.runs().join(id)) {
        Ok(()) => went.push("the run directory".to_string()),
        // Never made, or already gone: not a removal that failed.
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => unreached.push(format!("the run directory ({e})")),
    }

    // The branch is reported honestly rather than always claimed as kept: one
    // that never received a commit preserves nothing, and saying otherwise
    // trains people to ignore a namespace filling with dead refs.
    let base = session::default_branch(&paths.repo);
    let done = session.remove(&paths.repo, &base, &paths.shadows())?;
    let (removed, worktree) = (done.branch, done.worktree);
    match &done.shadow {
        session::Gone::Yes => went.push("the sandbox repository".to_string()),
        session::Gone::No(why) => unreached.push(format!("the sandbox repository ({why})")),
    }
    // **A worktree still on disk makes "removed session sNN" false**, and the
    // command used to print both — a warning saying it is there, then a
    // success line saying the session is gone, then exit 0. That is the shape
    // this whole change exists to remove, so the outcome is reported once and
    // the incomplete case is a failure rather than a footnote to a success.
    if let session::Gone::No(why) = &worktree {
        // **What happened to the branch, not an assumption about it.** The
        // first version said "the branch is untouched", and `remove` drops a
        // commitless one *before* it returns — so for exactly the sessions
        // that had nothing to review, the sentence was false.
        let branch = match removed {
            session::Removed::BranchKept(_) => {
                format!("the branch omh/{id} is still there.\n")
            }
            session::Removed::BranchDropped => String::new(),
            session::Removed::NoBranch => String::new(),
        };
        // Only what was observed. The first version asserted four removals
        // flatly and checked none of them, so a run with no usable runtime —
        // or one whose own warnings said the container would not stop —
        // printed those warnings and then claimed it had gone anyway.
        // The branch is its own sentence: `BranchKept` did *not* go, so
        // folding it into the "what went" list would be a new false claim in
        // the message written to remove one.
        if removed == session::Removed::BranchDropped {
            went.push(format!("the branch omh/{id}, which held no commits"));
        }
        let done = if went.is_empty() {
            String::new()
        } else {
            format!("what went: {}\n", went.join(", "))
        };
        let missed = if unreached.is_empty() {
            String::new()
        } else {
            format!("what omh could not reach: {}\n", unreached.join(", "))
        };
        anyhow::bail!(
            "{id} is partly removed — its worktree is still there:\n  \
             {at}\n  \
             {why}\n\
             {done}{missed}{branch}  \
             omh {id} rm              run it again once the directory is free",
            at = session.worktree.display()
        );
    }

    let action = match removed {
        session::Removed::BranchKept(n) => {
            // Two ways to be kept, and they are not the same news. A branch
            // kept because it holds three commits is an invitation to review
            // them; one kept because omh could not tell what it holds is a
            // question, and saying "3 commits" for it would be an invention.
            //
            // The count comes back from `remove` rather than being asked
            // again: it is the number that *made* the decision, and a second
            // call could answer differently and narrate a decision nobody took.
            //
            // The review command changes with it. What stops omh counting is a
            // range end that does not resolve, and for a branch this session is
            // standing on that is the base — so a line beginning `<base>..`
            // would fail in the user's hands for the reason they are being
            // shown it.
            let (kept, review) = match n {
                Some(n) => (
                    format!(
                        "kept ({n} {} to review)",
                        if n == 1 { "commit" } else { "commits" }
                    ),
                    format!("git log {base}..omh/{id}"),
                ),
                None => (
                    format!("kept — omh could not count it against {base}"),
                    format!("git log omh/{id}"),
                ),
            };
            report::Action::new(
                "session-removed",
                format!("removed session {id}; branch omh/{id} {kept}"),
            )
            .next(review)
            .next(format!("git branch -D omh/{id}"))
            .data(serde_json::json!({
                "session": id,
                "branch": format!("omh/{id}"),
                "branch_kept": true,
                "commits": n,
            }))
        }
        session::Removed::BranchDropped => report::Action::new(
            "session-removed",
            format!("removed session {id}; branch omh/{id} dropped (no commits)"),
        )
        .data(serde_json::json!({
            "session": id,
            "branch": format!("omh/{id}"),
            "branch_kept": false,
            "commits": 0,
        })),
        session::Removed::NoBranch => {
            report::Action::new("session-removed", format!("removed session {id}"))
                .data(serde_json::json!({ "session": id, "branch_kept": false }))
        }
    };
    ctx.say(&action);

    // The review moment rides on something already happening rather than a
    // ritual nobody performs. Best-effort on purpose: a store omh cannot read
    // is a reason to say nothing, never a reason to leave a session that
    // cannot be removed.
    //
    // A nudge is advice, so it goes through `hint`: on stderr, and absent
    // under `--json`, where a sentence about reviewing notes is noise in a
    // stream something else is parsing.
    if let Ok(notes) = memory::load(&paths) {
        if let Some(line) = memory::session_nudge(&notes, id) {
            ctx.hint(&line);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::Ran;

    /// The harness `attach` rejoins as, decided from the record alone.
    #[test]
    fn attach_rejoins_the_recorded_harness_and_refuses_a_damaged_record() {
        let names = vec!["claude".to_string(), "opencode".to_string()];
        let none = |_: &str| false;

        assert_eq!(
            harness_for_attach(Ran::Harness("opencode".into()), &names, &none).unwrap(),
            "opencode",
            "the session's own record is taken as is, not the host preference"
        );
        // NeverRecorded falls back to the host preference — first installed,
        // else first named.
        assert_eq!(
            harness_for_attach(Ran::NeverRecorded, &names, &none).unwrap(),
            "claude",
            "a session with no record uses the host default"
        );
        assert_eq!(
            harness_for_attach(Ran::NeverRecorded, &names, &|h| h == "opencode").unwrap(),
            "opencode",
            "and prefers one that is actually installed"
        );
        let refused = harness_for_attach(Ran::CouldNotTell("empty".into()), &names, &none)
            .expect_err("a record it cannot read is refused, not guessed at");
        assert!(
            refused.to_string().contains("resume"),
            "and points at the command that can be told the harness: {refused}"
        );
        // No adapters at all is the only NeverRecorded failure.
        assert!(
            harness_for_attach(Ran::NeverRecorded, &[], &none).is_err(),
            "with nothing installed there is no default to fall back to"
        );
    }

    // ── the launch path, without a container runtime ─────────────────────────
    //
    // These run `session_up`'s decisions against `Backend::scripted`. Before
    // the seam existed the two `attach` defects of the 0.9.0 audit — a live
    // session restarted because the host preferred another harness, and a
    // container the daemon would not describe replaced on a guess — had no
    // place to be written down; each is here now, red against the code that
    // had the defect.

    use crate::container::{sample_plan, Reuse};
    use crate::runtime::{answered, Backend, Docker};

    const NAME: &str = "omh-repo-s01";

    fn s01() -> Session {
        Session {
            id: "s01".into(),
            branch: Some("omh/s01".into()),
            worktree: std::path::PathBuf::from("/host/worktree"),
        }
    }

    /// What `docker inspect -f '{{json .Config.Labels}}'` prints for a
    /// container launched from this plan.
    fn stamped_from(plan: &container::Plan) -> String {
        let labels: std::collections::BTreeMap<String, String> =
            plan.labels().into_iter().collect();
        serde_json::to_string(&labels).unwrap()
    }

    fn removed(log: &[Vec<String>]) -> bool {
        log.iter()
            .any(|argv| argv.starts_with(&["rm".to_string(), "-f".to_string()]))
    }

    #[test]
    fn attaching_to_a_session_rejoins_the_harness_it_ran() {
        let plan = sample_plan();
        let (backend, log) = Backend::scripted(
            Box::new(Docker),
            vec![
                (vec!["exec"], answered(0, "s01-claude\n", "")),
                (vec!["inspect"], answered(0, &stamped_from(&plan), "")),
            ],
        );
        let went =
            reuse_or_replace(&backend, NAME, &plan, &s01(), "claude", &out::Ctx::plain()).unwrap();
        assert_eq!(went, Disposition::Attach);
        assert!(
            !removed(&log.borrow()),
            "a session that matches its plan is joined, never removed: {:?}",
            log.borrow()
        );
    }

    #[test]
    fn a_container_the_runtime_will_not_describe_is_never_replaced() {
        let plan = sample_plan();
        let (backend, log) = Backend::scripted(
            Box::new(Docker),
            vec![
                (vec!["exec"], answered(0, "s01-claude\n", "")),
                (
                    vec!["inspect"],
                    answered(
                        1,
                        "",
                        "Cannot connect to the Docker daemon at unix:///var/run/docker.sock",
                    ),
                ),
            ],
        );
        let refused = reuse_or_replace(&backend, NAME, &plan, &s01(), "claude", &out::Ctx::plain())
            .expect_err("an unreadable stamp is a refusal, not a restart");
        assert!(
            refused
                .to_string()
                .contains("neither attach to it nor replace it"),
            "{refused}"
        );
        assert!(
            !removed(&log.borrow()),
            "and nothing was removed: {:?}",
            log.borrow()
        );
    }

    #[test]
    fn a_session_running_another_harness_is_reported_not_restarted() {
        let plan = sample_plan();
        let mut other = sample_plan();
        other.image = "omh/opencode:latest".into();
        let (backend, log) = Backend::scripted(
            Box::new(Docker),
            vec![
                (vec!["exec"], answered(0, "s01-opencode\n", "")),
                (vec!["inspect"], answered(0, &stamped_from(&other), "")),
            ],
        );
        let refused = reuse_or_replace(&backend, NAME, &plan, &s01(), "claude", &out::Ctx::plain())
            .expect_err("a live harness blocks the restart");
        let said = refused.to_string();
        assert!(said.contains("opencode"), "names what is live: {said}");
        assert!(said.contains("omh s01 down"), "and how to stop it: {said}");
        assert!(
            !removed(&log.borrow()),
            "nothing live is ever removed: {:?}",
            log.borrow()
        );
    }

    #[test]
    fn a_drifted_sandbox_with_nothing_live_in_it_is_replaced() {
        let plan = sample_plan();
        let mut other = sample_plan();
        other.image = "omh/claude:older".into();
        let (backend, log) = Backend::scripted(
            Box::new(Docker),
            vec![
                (vec!["exec"], answered(0, "", "")),
                (vec!["inspect"], answered(0, &stamped_from(&other), "")),
                (vec!["rm", "-f", NAME], answered(0, "", "")),
            ],
        );
        let went =
            reuse_or_replace(&backend, NAME, &plan, &s01(), "claude", &out::Ctx::plain()).unwrap();
        assert_eq!(went, Disposition::Replaced);
        assert!(
            removed(&log.borrow()),
            "the stale container is removed: {:?}",
            log.borrow()
        );
        // And the decision this rides on is still the one `container::decide`
        // makes — the seam did not grow its own.
        assert!(matches!(
            reuse_decision(&backend, NAME, &plan, &s01()).unwrap(),
            Reuse::Restart(_)
        ));
    }

    #[test]
    fn a_stopped_container_is_cleared_before_the_new_one_is_named() {
        let plan = sample_plan();
        let (backend, log) = Backend::scripted(
            Box::new(Docker),
            vec![
                (vec!["rm", "-f", NAME], answered(1, "", "No such container")),
                (vec!["run"], answered(0, "deadbeef\n", "")),
            ],
        );
        start(&backend, &plan, NAME, 50022, "ssh-ed25519 AAAA omh", "s01").unwrap();
        let log = log.borrow();
        let cleared = log
            .iter()
            .position(|a| a.starts_with(&["rm".to_string(), "-f".to_string(), NAME.to_string()]));
        let named = log.iter().position(|a| {
            a.first().map(String::as_str) == Some("run")
                && a.windows(2).any(|w| w[0] == "--name" && w[1] == NAME)
        });
        assert!(
            cleared.is_some() && named.is_some() && cleared < named,
            "a stopped container under the name blocks `run --name`, so it is cleared first: {log:?}"
        );
    }

    #[test]
    fn a_run_the_runtime_refuses_names_the_session_and_repeats_the_reason() {
        let plan = sample_plan();
        let (backend, _) = Backend::scripted(
            Box::new(Docker),
            vec![
                (vec!["rm", "-f", NAME], answered(0, "", "")),
                (
                    vec!["run"],
                    answered(125, "", "docker: port is already allocated"),
                ),
            ],
        );
        let err = start(&backend, &plan, NAME, 50022, "ssh-ed25519 AAAA omh", "s01").unwrap_err();
        let said = err.to_string();
        assert!(
            said.contains("s01") && said.contains("port is already allocated"),
            "{said}"
        );
    }
}
