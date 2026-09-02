//! The commands that answer rather than change: `doctor`, `why`, `info`,
//! `graph`.
//!
//! `doctor` is the only one that proves anything about an adapter — it
//! launches the real image with the real mounts and asks the harness what it
//! sees. The rest read what is already on disk.

use crate::adapter::Adapter;
use crate::out;
use crate::profile::{Paths, Profile};
use crate::session::{self, Session};
use crate::{
    adapter, auth, base, config, container, detect, doctor, editor, image, key, memory, persist,
    render, report, runtime, settings, stack, why,
};
use anyhow::{Context, Result};
use std::process::Command;

/// Serve the graph UI from the session and open it.
///
/// Started on demand rather than always: the port is reserved when the session
/// is created (it has to be), but a process nobody looks at is waste.
/// The graph UI, once per repo.
///
/// Not per session: every session's graph lives in one volume, so a per-session
/// server showed every other session's graph anyway. Matching the server's
/// scope to its data's scope removes the duplication, survives sessions coming
/// and going, and lets the container mount only the index.
pub(crate) fn graph(cwd: &std::path::Path, stop: bool, ctx: &out::Ctx) -> Result<()> {
    let paths = Paths::discover(cwd)?;
    let backend = runtime::select(&crate::runtime_preference(&paths), &|p| {
        runtime::installed(p)
    })?;
    let container = base::ui_container(&paths.repo_name());

    if stop {
        if !crate::cmd::harvest::must_know(
            image::container_running(backend.as_ref(), &container),
            "the graph",
            "stop it",
        )? {
            ctx.say(
                &report::Action::new("graph-not-running", "the graph is not running")
                    .data(serde_json::json!({ "running": false })),
            );
            return Ok(());
        }
        image::container_remove(backend.program(), &container)?;
        ctx.say(
            &report::Action::new("graph-stopped", "graph stopped; sessions keep running")
                .data(serde_json::json!({ "running": false })),
        );
        return Ok(());
    }

    let port = base::ui_port(&container);
    if !crate::cmd::harvest::must_know(
        image::container_running(backend.as_ref(), &container),
        "the graph",
        "start it",
    )? {
        // A stopped container of the same name blocks `run --name`.
        let _ = image::container_remove(backend.program(), &container);

        let names: Vec<String> = Adapter::load_dir(&paths.adapters())?
            .into_iter()
            .map(|a| a.name)
            .collect();
        let harness = detect::preferred_harness(&names, &|h| runtime::installed(h))
            .context("no adapters installed — run `omh init`")?;
        let adapter = Adapter::find(&paths.adapters(), &harness)?;
        let ca = image::ca_for(&paths)?;
        image::ensure(backend.program(), &adapter, ca.as_deref())?;

        let out = Command::new(backend.program())
            .args(base::ui_run_args(
                &image::tag_for(&adapter, ca.as_deref()),
                &container,
                &paths.cache_volume(),
                port,
            ))
            .output()?;
        if !out.status.success() {
            anyhow::bail!(
                "could not start the graph: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            );
        }
        std::thread::sleep(std::time::Duration::from_millis(1500));
    }

    let url = format!("http://127.0.0.1:{port}");
    ctx.say(
        &report::Action::new("graph-started", format!("graph at {url}"))
            .next("omh graph --stop")
            .data(serde_json::json!({ "url": url, "port": port, "running": true })),
    );
    ctx.hint("every session's graph for this repo, in one place");
    let _ = Command::new(if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    })
    .arg(&url)
    .status();
    Ok(())
}

/// Launch the real image with the real mounts and ask the harness's own paths
/// what they can see. Nothing in process can answer this: a green unit suite
/// proves omh mounts a path, never that anything reads it.
pub(crate) fn doctor_cmd(
    cwd: &std::path::Path,
    harness: Option<&str>,
    dry_run: bool,
    ctx: &out::Ctx,
) -> Result<()> {
    let paths = Paths::discover(cwd)?;
    let profile = Profile::resolve(&paths);
    let name = match harness {
        Some(h) => h.to_string(),
        None => {
            let names: Vec<String> = Adapter::load_dir(&paths.adapters())?
                .into_iter()
                .map(|a| a.name)
                .collect();
            detect::preferred_harness(&names, &|h| runtime::installed(h))
                .context("no adapters installed — run `omh init`")?
        }
    };
    let adapter = Adapter::find(&paths.adapters(), &name)?;

    // Credentials are the half no in-process test can reach: whether a token
    // saved here survives depends on how the runtime binds the path.
    // The same resolver `omh new` uses, asked the same way. It read `None` for
    // the explicit account and then swallowed the answer with `.unwrap_or(None)`
    // — so `-a work` went nowhere, and *ambiguous* became *no account*, against
    // a function whose own doc comment reads "Ambiguity is an error, never a
    // guess". Two accounts captured, and doctor reported credentials unchecked
    // in exactly the words it uses for a user who has none.
    //
    // That mattered more here than anywhere: credentials are the half no
    // in-process test can reach, so `credential_checks` below is the only
    // evidence a token survives the mount, and it was being skipped for anyone
    // with a second account. The remedy the resolver prints names `-a` — the
    // flag this was discarding.
    let configured = crate::policy_value(&paths, "account");
    let account = auth::resolve_for_launch(&paths, &adapter, configured.as_deref())?
        .map(|a| auth::dir(&paths, &name, &a));

    // Resolved once and used for both the checks and the plan below, so the
    // probe cannot check a session different from the one it launches.
    let (own, repo) = crate::cmd::session::resolved(&paths)?;
    // The one reading this command makes. `ensure_stack` below takes
    // `sandbox.ca`, and `ca_check` asserts against the same value.
    let ca = image::ca_for(&paths)?;

    // **Before any image work, and only when no root is set.** This is the one
    // problem doctor could never reach: behind a TLS-inspecting proxy the
    // build dies on an unknown issuer, so every guest-side check below is
    // unreachable and doctor's answer was a docker error. `build` names the
    // setting when that happens — but a build that was *cached* before the
    // proxy appeared succeeds, and then only the sessions fail. That is the
    // case this catches, and a cache has hidden a problem here for weeks
    // before.
    //
    // Only `Private` is reported. `Public` is the ordinary answer and a row
    // saying so is noise; `Unknown` means omh could not tell — offline, or not
    // macOS — and a check that guesses in that state is the cry-wolf this
    // whole three-valued shape exists to avoid.
    if ca.is_none() {
        let (verdict, hosts) = doctor::inspected_hosts();
        // **`Unknown` is not `Public` and must not look like it.** It was
        // silent, which made "omh could not measure this" identical at the
        // terminal to "omh measured it and it is fine" — and the reasons are
        // reachable: no `openssl`, no network, an `openssl` that ignores
        // `-CAfile`. A check that quietly did not run is the shape doctor
        // exists to eliminate, not to add.
        if let doctor::Inspection::Unknown(why) = &verdict {
            ctx.say(
                &report::Action::new(
                    "tls-inspection-unknown",
                    format!("could not check whether this network re-signs TLS: {why}"),
                )
                .data(serde_json::json!({ "reason": why })),
            );
        }
        if verdict == doctor::Inspection::Private {
            // Named, because a proxy that re-signs one host and not another is
            // a surprising thing to be told and the reader should be able to
            // check it rather than take it on faith.
            ctx.warn(&format!(
                "this network re-signs TLS for {} with a root your machine \
                 trusts and a container does not — a sandbox cannot verify \
                 what it fetches from {}. Set the corporate root:\n\n    \
                 security find-certificate -a -c \"Zscaler\" -p > ~/corp-root.pem\n    \
                 omh set --local ca_cert ~/corp-root.pem\n\n\
                 See docs/troubleshooting.md.",
                hosts.join(" and "),
                if hosts.len() == doctor::FETCHES.len() {
                    "the network"
                } else {
                    "there"
                }
            ));
        }
    }

    let mut sandbox = crate::cmd::init::sandbox(&paths, &adapter, &repo, ca)?;
    if let Ok(backend) = runtime::select(&crate::runtime_preference(&paths), &|p| {
        runtime::installed(p)
    }) {
        sandbox.top_up(
            &paths,
            backend.program(),
            &adapter,
            &profile.sources(adapter::Capability::Hooks)?,
            &own,
            &repo,
            ctx,
        )?;
    }
    let mut checks = doctor::checks(&profile, &adapter, &own, &repo, &sandbox.resolves)?;
    if account.is_some() {
        checks.extend(doctor::credential_checks(&adapter));
    }
    // The one claim about this image no test can settle: whether the root
    // omh embedded actually got into the store the toolchains read.
    checks.extend(doctor::ca_check(sandbox.ca.as_deref()));
    // Only if the resolved profile actually declares it: a check for a server
    // nobody configured would fail honestly and mean nothing.
    //
    // Read through `render::parse_layers` rather than `config::servers`, which
    // returns only each server's *command* — the arguments are what say which
    // directories it will look in, and those are the whole point of the check.
    let declared = render::parse_layers(&profile.sources(adapter::Capability::Mcp)?)?;
    // Not when this repo has switched the feature off: the server is left out
    // of the document on purpose, so checking for it is checking a claim omh
    // deliberately did not make.
    if let Some(server) = declared
        .get(memory::tools::SERVER_KEY)
        .filter(|_| !repo.disabled_servers.contains(memory::tools::SERVER_KEY))
    {
        checks.extend(doctor::memory_checks(server));
    }
    if checks.is_empty() {
        ctx.say(
            &report::Action::new(
                "doctor-nothing-to-check",
                "nothing to check: the profile is empty",
            )
            .data(serde_json::json!({ "harness": name, "checks": 0 })),
        );
        return Ok(());
    }

    let session = Session::scratch(paths.scratch("doctor"), "doctor".into());
    session.ensure(&paths.repo, "")?;

    let opts = container::Options {
        staging: container::Staging::Apply,
        // No dtach and no terminal: the probe's output has to be captured.
        persist: persist::Mode::None,
        tty: false,
        account_dir: account.clone(),
        memory_bin: memory::deliver::available(&paths, ctx),
        // The probe has to compose the same rules a launch would, or it proves
        // the harness reads a document nobody will be given.
        base: Some(session::default_branch(&paths.repo)),
        omh: own,
        repo,
        image: sandbox.tag.clone(),
        resolves: sandbox.resolves.clone(),
    };
    if let Some(account_dir) = &account {
        auth::prepare(&adapter, account_dir, auth::GUEST_HOME)?;
    }
    crate::cmd::session::say_selection(&paths, &profile, &opts.repo, ctx);
    let mut plan = container::plan(&paths, &profile, &adapter, &session, &[], opts)?;
    crate::cmd::session::say_rules(&plan, ctx);
    plan.argv = vec!["sh".into(), "-c".into(), doctor::probe_script(&checks)];

    let backend = runtime::select(&crate::runtime_preference(&paths), &|p| {
        runtime::installed(p)
    })?;
    plan.validate(&backend.caps())?;

    if dry_run {
        // The script itself, unwrapped: this output exists to be piped into a
        // shell or read line by line, and a report around it would have to be
        // stripped back off. `Probe` says so in one place instead of here.
        ctx.say(&report::Probe {
            script: doctor::probe_script(&checks),
            checks: checks.iter().map(|c| c.name.clone()).collect(),
        });
        return Ok(());
    }

    image::ensure_stack(
        backend.program(),
        &adapter,
        &sandbox.recipe(),
        // The sandbox's own reading. A fresh `ca_for` here would be a second
        // resolution, and `sandbox.tag` — which `opts.image` runs and
        // `ca_check` asserts against — came from the first.
        sandbox.ca.as_deref(),
        &paths.repo,
    )?;
    image::ensure_network(backend.program(), &plan.network)?;

    let account_name = account
        .as_ref()
        .map(|a| a.file_name().unwrap_or_default().to_string_lossy().into());
    ctx.progress(&match &account_name {
        Some(a) => format!("checking {name} in {} as {a}…", sandbox.tag),
        None => format!(
            "checking {name} in {} — no account, so credentials go unchecked…",
            sandbox.tag
        ),
    });

    let out = Command::new(backend.program())
        .args(backend.args(&plan))
        .output()?;
    let from_the_sandbox = doctor::parse(&String::from_utf8_lossy(&out.stdout));
    let _ = session.remove(&paths.repo, "", &paths.shadows()); // diagnostic: leave no session behind
                                                               // `with_context` would make the sandbox's stderr the *outer* error, so
                                                               // `out::problem` would print it as omh's own headline and demote omh's
                                                               // explanation to a cause — with an empty stderr rendering as a bare
                                                               // `omh:` and nothing after it. The sentence omh wrote stays first, and
                                                               // what the container said follows it, sanitised: it is not omh's text.
    let outcomes =
        crate::cmd::harvest::every_check(from_the_sandbox).map_err(
            |e| match crate::out::untrusted(String::from_utf8_lossy(&out.stderr).trim()) {
                said if said.is_empty() => e,
                said => anyhow::anyhow!("{e}\n{said}"),
            },
        )?;

    let report = report::Doctor {
        harness: name,
        tag: sandbox.tag.clone(),
        account: account_name,
        outcomes,
    };
    ctx.say(&report);
    if !report.passed() {
        anyhow::bail!(
            "{} of {} checks failed",
            report.failed(),
            report.outcomes.len()
        );
    }
    Ok(())
}

/// `omh why <thing>` — who put this here, and on what grounds.
///
/// Needs no container and no session: it is a pure function of the manifest and
/// the resolved profile, which is why it can answer even for something you have
/// removed.
pub(crate) fn why_cmd(cwd: &std::path::Path, thing: &str, ctx: &out::Ctx) -> Result<()> {
    let paths = Paths::discover(cwd)?;

    // A settings key, before the catalogue is consulted at all. `why` resolves
    // against MCP servers and hooks, and a key is neither, so every one of them
    // came back *nothing recorded under that name* — while three `--help`
    // strings and two documentation pages told people to ask here. `why.rs`
    // calls that its own failure mode; this closes it rather than deleting the
    // sentence, because the classification is what the whole layer rule rests
    // on and a person has no other way to read it.
    if let Some(k) = key::describes(thing) {
        ctx.say(&report::Why {
            thing: thing.to_string(),
            text: why_a_key(&paths, k),
        });
        return Ok(());
    }

    let manifest = base::Manifest::load_dir(&paths.base())?;

    // Servers and hooks are the same kind of thing here: installed, from a
    // layer, chosen by omh or by you.
    let mut installed = config::servers(&paths)?;
    installed.extend(config::hooks(&paths)?);

    // What omh ships, for deciding whether your copy has been changed. MCP
    // servers only: hooks and rules sections are generated at launch, so there
    // is nothing of yours to compare — a file of that name is a leftover, and
    // the `Generated` verdict names it as one rather than as your edit.
    let baselines: std::collections::BTreeMap<String, String> = manifest
        .entries
        .iter()
        .filter_map(|e| e.command.clone().map(|c| (e.name.clone(), c)))
        .collect();

    // Hooks that belong to a detected ecosystem are omh's opinion about that
    // ecosystem, not about this repo. Reported as neither the base set nor
    // yours, because claiming either would be false in a way this command
    // exists to prevent.
    //
    // The command travels with the name, so the claim is checkable: this reads
    // the hook that would actually ship rather than matching on a name anyone
    // could give a file. What changed with the catalogue is where the body
    // comes from — the file, not a `match` in Rust — and that a repo shadowing
    // the name is reported with *its* command, which is the honest answer.
    let mut derived = std::collections::BTreeMap::new();
    let stack_defs = stack::load_all(&paths.stacks(), &paths.repo_stacks())?;
    let detected = stack::detected(&stack_defs, &paths.repo);
    let (own, repo_policy) = crate::cmd::session::resolved(&paths)?;
    let merged = render::merge_hooks(
        &Profile::resolve(&paths).sources(adapter::Capability::Hooks)?,
        &own,
        &repo_policy,
    )?;
    for (name, hook) in &merged {
        let Some(stack) = hook.stack.as_deref() else {
            continue;
        };
        let Some(def) = detected.iter().find(|d| d.name == stack) else {
            continue;
        };
        derived.insert(
            name.clone(),
            why::Derived {
                from: format!("{}, detected from {}", def.name, def.marker),
                command: hook.does().to_string(),
                layer: config::Layer::Shared,
            },
        );
    }

    let source = manifest.source();
    let version = manifest.version.clone();
    let catalog = why::Catalog {
        off: settings::resolve(&paths, &manifest)?.off,
        manifest: &manifest,
        baselines,
        installed,
        derived,
    };
    ctx.say(&report::Why {
        thing: thing.to_string(),
        text: why::render_with_source(&catalog, &catalog.why(thing), &version, &source),
    });
    Ok(())
}

/// What omh knows about a settings key, as prose.
///
/// Says where an unadorned `omh set` would put it and whether git carries that
/// file, because the destination is the half a person cannot check any other
/// way — the classification is a table in the binary, not something a settings
/// file shows.
pub(crate) fn why_a_key(paths: &Paths, k: &key::Key) -> String {
    let layer = k.default_layer();
    let mut text = format!("`{}` is a setting omh reads.\n\n  {}\n", k.name, k.does);
    text.push_str(&match k.shape {
        key::Shape::Text => "  takes  one word or phrase\n".to_string(),
        key::Shape::Paths => "  takes  a TOML array of paths, e.g. [\".env\"]\n".to_string(),
        key::Shape::Path => "  takes  one path, e.g. /etc/ssl/certs/corp.pem\n".to_string(),
        key::Shape::Duration => "  takes  90s, 30m, 2h, 1d, or bare seconds\n".to_string(),
        key::Shape::Choice(all) => format!("  takes  one of {}\n", all.join(", ")),
    });
    text.push_str(&format!(
        "  kept   {} ({})\n",
        layer.file(paths).display(),
        crate::cmd::settings::tracked(layer)
    ));
    if k.secret == key::Secret::Yes {
        text.push_str(
            "\n  A value here can name a credential, which is why omh keeps it\n  out of the file git carries.\n",
        );
    }
    text
}

pub(crate) fn info(cwd: &std::path::Path, ctx: &out::Ctx) -> Result<()> {
    let paths = Paths::discover(cwd)?;
    let base = session::default_branch(&paths.repo);

    // What your catalogue holds. It belonged to the command 0.7.0 deleted, and that is
    // gone — `omh info` means *what you have here*, which a catalogue is.
    let profile = Profile::resolve(&paths);
    let mut catalogue = Vec::new();
    for cap in adapter::Capability::ALL {
        catalogue.push(report::Catalogue {
            capability: cap.to_string(),
            entries: profile.entries(cap)?,
        });
    }

    ctx.say(&report::Inventory {
        catalogue_dir: paths.root.display().to_string(),
        catalogue,
        harnesses: Adapter::load_dir(&paths.adapters())?
            .iter()
            .map(|a| report::Harness {
                name: a.name.clone(),
                accounts: auth::accounts(&paths, a),
            })
            .collect(),
        adapters_dir: paths.adapters().display().to_string(),
        editors: editor::Editor::load_dir(&paths.editors())?
            .iter()
            .map(|e| report::Editor {
                name: e.name.clone(),
                installed: runtime::installed(&e.bin),
            })
            .collect(),
        sessions: session::list(&paths.worktrees())
            .into_iter()
            .map(|id| {
                let sess = Session::new(&paths.worktrees(), id.clone());
                report::Session {
                    label: sess.label().to_string(),
                    // `omh info` is the wide view and does not ask git what
                    // state the work is in; `omh s` is the command for that, and
                    // asking here would cost a subprocess per session for a
                    // column this listing does not print. `None` says *not
                    // asked* — `Work::Clean` would be a claim, and a false one.
                    work: None,
                    // `None` for the same reason `work` is: this listing does
                    // not print the column and asking would cost a subprocess
                    // per session. `false` was a claim, and one omh had not
                    // checked.
                    running: None,
                    // Silently `.ok()` until #62 put a yellow question in
                    // this column: a surface that asks *how far behind?* and
                    // cannot say why is worse than one that never asked.
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
            .collect(),
        base,
    });
    Ok(())
}
