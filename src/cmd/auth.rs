//! `omh auth` — run a harness’s own login once, and keep what it wrote.
//!
//! Accounts are per harness, and which one a project uses is a project-level
//! setting, because that is how it actually varies. `crate::auth` owns the
//! capture; this drives it and reports.

use crate::adapter::{Adapter, ProviderFormat};
use crate::out;
use crate::profile::{Paths, Profile};
use crate::session::Session;
use crate::{auth, container, image, memory, persist, report, runtime};
use anyhow::{Context, Result};
use std::collections::BTreeMap;

struct NetworkCleanup<'a> {
    backend: &'a runtime::Backend,
    name: &'a str,
}

impl Drop for NetworkCleanup<'_> {
    fn drop(&mut self) {
        let _ = image::network_remove(self.backend, self.name);
    }
}

/// Capture an account: run the harness's own login in a sandbox, or with
/// `--import` copy the login this machine already holds.
pub(crate) fn auth_cmd(
    cwd: &std::path::Path,
    harness: &str,
    account: &str,
    import: bool,
    provider: Option<&std::path::Path>,
    ctx: &out::Ctx,
) -> Result<()> {
    let paths = Paths::discover(cwd)?;
    let adapter = Adapter::find(&paths.adapters(), harness)?;

    if adapter.creds.is_empty() {
        anyhow::bail!(
            "adapter {harness} declares no credential paths, so there is nothing to capture"
        );
    }

    auth::validate_name(account)?;
    // Before anything is touched: a refusal leaves the account as it was.
    let source = adapter.provider.as_ref().map(|p| p.import.as_str());
    if provider.is_some() && source.is_none() {
        anyhow::bail!(
            "{harness} takes no provider file — omh keeps one for codex and omp only, so far"
        );
    }
    let account_dir = auth::dir(&paths, &adapter, account);
    let already = auth::is_captured(&paths, &adapter, account);
    // Before `prepare`, which then lays a placeholder where a file was set
    // aside: the login mounts the account's own files, not a session's copy.
    for note in auth::scrub_account(&adapter, &account_dir)? {
        ctx.warn(&note);
    }
    auth::prepare(&adapter, &account_dir, "/home/agent")?;

    // The account's model provider, kept at its root where no session writes:
    // from the file named, or with `--import` from the host's own config.
    // Authored before any login, and independent of one.
    let home = dirs::home_dir().context("no home directory")?;
    let format = adapter.provider.as_ref().map(|p| p.format);
    let provided = match (provider, import, source, format) {
        (Some(file), _, Some(_), Some(format)) => {
            let raw = std::fs::read_to_string(file)
                .with_context(|| format!("reading {}", file.display()))?;
            let doc = author_provider(format, &raw, &account_dir)
                .with_context(|| format!("{}", file.display()))?;
            Some(doc.with_context(|| format!("{} defines no model provider", file.display()))?)
        }
        (None, true, Some(source), Some(format)) => {
            let host = crate::adapter::expand_host(source, &home, &paths.repo);
            match read_host_provider(format, &host)? {
                Some((host, raw)) => author_provider(format, &raw, &account_dir)
                    .with_context(|| format!("{}", host.display()))?,
                None => None,
            }
        }
        _ => None,
    };

    // `--import` copies what the host already holds and starts nothing: a
    // login that cannot finish in a sandbox is the reason it exists. With a
    // provider and no token to copy, the provider is the capture.
    let imported = if import {
        match auth::import(&adapter, &account_dir, &home) {
            Ok(from) => Some(from),
            Err(e) if provided.is_some() => {
                ctx.warn(&format!("{e:#} — the account holds its provider only"));
                None
            }
            Err(e) => return Err(e),
        }
    } else if provider.is_some() {
        None
    } else {
        login_in_sandbox(&paths, &adapter, account, &account_dir, already, ctx)?;
        None
    };
    let all = auth::accounts(&paths, &adapter)?;
    // What the files can and cannot settle. For a harness naming `token` files
    // an empty `unfilled` *is* the login; for one that keeps credentials
    // somewhere omh cannot stat it means only that nothing is obviously
    // missing, and saying "captured" there announced a login to users who had
    // opened the harness, run nothing and quit.
    let decided = auth::decided_by_files(&adapter);
    let mut action = if decided {
        report::Action::new(
            "account-captured",
            format!("`{account}` captured for {harness}"),
        )
    } else {
        report::Action::new(
            "account-recorded",
            format!("`{account}` recorded for {harness} — login not confirmed"),
        )
        .note(format!(
            "{harness} keeps its credentials where omh cannot read them, so only \
             {harness} can say whether the login took"
        ))
        .next(format!("omh doctor --harness {harness}"))
    };
    // A copy, not a link: the account and the host each hold their own login
    // from here on, and a later login on the host does not reach this one.
    if let Some(from) = &imported {
        let from: Vec<String> = from.iter().map(|p| p.display().to_string()).collect();
        action = action.note(format!("copied from {}", from.join(", ")));
    }
    // By name: the values can be tokens.
    let providers: Vec<String> = provided.iter().flatten().cloned().collect();
    if !providers.is_empty() {
        action = action.note(format!(
            "model provider {} kept in {}",
            providers.join(", "),
            account_dir
                .join(crate::provider::file(format.context(
                    "a provider was kept for a harness that takes none"
                )?))
                .display()
        ));
    }
    action = action.data(serde_json::json!({
        "harness": harness,
        "account": account,
        "reauthenticated": already,
        "credentials": account_dir.display().to_string(),
        "imported": imported,
        "providers": providers,
        "accounts": all,
    }));
    // Only once there is a choice to make. With one account the line is a
    // sentence about a decision nobody has.
    if all.len() > 1 {
        action = action
            .note(format!("accounts: {}", all.join(", ")))
            .next("omh set account <name>");
    }
    ctx.say(&action);
    Ok(())
}

/// The provider a harness config `raw` defines — Codex's tables, omp's
/// `models.yml` — resolved for a sandbox and written as the account's
/// provider file: the names of the providers it holds, or `None` when `raw`
/// defines none. Environment variables it reads are read here, once, from the
/// shell `omh auth` runs in.
fn author_provider(
    format: ProviderFormat,
    raw: &str,
    account_dir: &std::path::Path,
) -> Result<Option<Vec<String>>> {
    let env = |k: &str| std::env::var(k).ok();
    match format {
        ProviderFormat::CodexConfig => {
            let Some(doc) = crate::provider::from_codex_config(raw, &env)? else {
                return Ok(None);
            };
            crate::provider::write(account_dir, &doc)?;
            let names = doc
                .get("model_providers")
                .and_then(|p| p.as_table())
                .map(|p| p.keys().cloned().collect())
                .unwrap_or_default();
            Ok(Some(names))
        }
        ProviderFormat::OmpModels => {
            let Some(doc) = crate::provider::omp::from_models_config(raw, &env)? else {
                return Ok(None);
            };
            crate::provider::omp::write(account_dir, &doc)?;
            Ok(Some(crate::provider::omp::names(&doc)))
        }
    }
}

/// The host's own provider config, where the harness reads it — for omp, the
/// `models.yaml` it reads when `models.yml` is absent. `None` when there is
/// none.
fn read_host_provider(
    format: ProviderFormat,
    host: &std::path::Path,
) -> Result<Option<(std::path::PathBuf, String)>> {
    let mut candidates = vec![host.to_path_buf()];
    if format == ProviderFormat::OmpModels {
        candidates.push(host.with_extension("yaml"));
    }
    for at in candidates {
        match std::fs::read_to_string(&at) {
            Ok(raw) => return Ok(Some((at, raw))),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => return Err(e).with_context(|| format!("reading {}", at.display())),
        }
    }
    Ok(None)
}

/// The harness's own login, run in a throwaway sandbox with the account's
/// credential files bind-mounted writable. There is no separate capture step:
/// the login writes straight through to the host.
fn login_in_sandbox(
    paths: &Paths,
    adapter: &Adapter,
    account: &str,
    account_dir: &std::path::Path,
    already: bool,
    ctx: &out::Ctx,
) -> Result<()> {
    let harness = adapter.name.as_str();
    let profile = Profile::resolve(paths);
    let backend = runtime::select(&crate::runtime_preference(paths), &|p| {
        runtime::installed(p)
    })?;
    let ca = image::ca_for(paths)?;
    image::ensure(&backend, adapter, ca.as_ref().map(image::Root::pem))?;

    // A throwaway: logging in must not leave a branch behind.
    let session = Session::scratch(paths.scratch("auth"), "auth".into());
    session.ensure(&paths.repo, "")?;

    // The sandbox's host key, made here rather than left to the mount — the
    // reason is with the same call in `cmd::inspect`'s doctor, which hit it
    // first. In short: a bind mount whose host path does not exist is created
    // by the daemon, as root, on Linux.
    crate::ssh::ensure_host_key(&paths.keys())?;

    let (own, repo) = crate::cmd::session::resolved(paths)?;
    // The account's preferences, as a session has them: checked before
    // anything starts, then migrated and seeded — into the login's own copy,
    // under `scratch/` — before the login does.
    let prefs = crate::prefs::check(adapter, account_dir)?;

    let plan = container::plan(
        paths,
        &profile,
        adapter,
        &session,
        &[],
        container::Options {
            staging: container::Staging::Apply,
            persist: persist::Mode::None,
            tty: true,
            account_dir: Some(account_dir.to_path_buf()),
            launch: crate::container::Launch::Login,
            memory_bin: memory::deliver::available(paths, ctx),
            // Empty, like the base this scratch session was created with at
            // `session.ensure(&paths.repo, "")`: a login is not work on the
            // project, so there are no project rules to look up.
            base: None,
            omh: own,
            repo,
            // The harness image, not this repo's stack layer, for the reason
            // `base` is `None`: a login is not work on the project. Building a
            // toolchain to type a password would spend minutes on a container
            // that is thrown away, and the credential paths a login writes are
            // the same in both images. Over it, where the agent must own what
            // the host mounts, the owner layer: a login writes the account's
            // directory, which uid 1000 cannot write on such a host.
            image: image::for_host(
                &backend,
                &image::tag_for(adapter, ca.as_ref().map(image::Root::pem)),
                &image::Kind::Harness(adapter),
                true,
            )?,
            // So nothing has been measured about it here, and nothing is
            // suppressed. That is the safe direction — a login session running
            // one hook too many costs nothing, and this container exists for
            // the length of an OAuth redirect.
            resolves: BTreeMap::new(),
        },
    )?;
    plan.validate_for(&backend)?;
    let home = container::prefs_home(paths, &session, harness);
    if let Some(prefs) = &prefs {
        let running = || crate::cmd::session::sessions_mounting(&backend, account_dir);
        for note in prefs.apply(&running)? {
            ctx.warn(&note);
        }
        for note in prefs.seed(adapter, &home)? {
            ctx.warn(&note);
        }
    }
    auth::drop_carried(adapter, account_dir)?;
    image::ensure_network(&backend, &plan.network)?;
    let _network_cleanup = NetworkCleanup {
        backend: &backend,
        name: &plan.network,
    };

    // Progress, not the report: the login itself is what the user is here for,
    // and this is the sentence that tells them which window is about to open
    // and where the token will land. Under `--json` the same facts arrive as
    // fields on the outcome below.
    ctx.progress(&format!(
        "logging {harness} in as `{account}`{} — credentials → {}{}",
        if already { " (re-authenticating)" } else { "" },
        account_dir.display(),
        match &adapter.login {
            Some(hint) => format!("\nnext → {hint}"),
            None => String::new(),
        }
    ));
    let status = backend.command().args(backend.args(&plan)).status()?;
    // What the login chose among its preferences — a theme picked at a first
    // start — was chosen at the terminal, by the user: kept.
    match crate::prefs::keep(adapter, &home) {
        Ok(notes) => notes.iter().for_each(|n| ctx.warn(n)),
        Err(e) => ctx.warn(&format!("{e:#}")),
    }
    if let Err(e) = session.remove(&paths.repo, "", &paths.shadows()) {
        // A leftover `auth` worktree is a session `omh s` would list and offer
        // as a real one. Removed so the login leaves nothing behind.
        ctx.warn(&format!("could not remove the auth worktree: {e}"));
    }

    // Host paths, not guest ones: the guest path names a container that has
    // already been torn down and that the user cannot inspect.
    let unfilled: Vec<std::path::PathBuf> = auth::unfilled(adapter, account_dir, auth::GUEST_HOME)
        .iter()
        .map(|guest| {
            account_dir.join(
                guest
                    .strip_prefix(auth::GUEST_HOME)
                    .unwrap_or(guest.as_path()),
            )
        })
        .collect();
    auth::login_outcome(status.success(), &unfilled)
        .map_err(|e| e.context(format!("run `omh auth {harness} --name {account}` again")))?;
    Ok(())
}
