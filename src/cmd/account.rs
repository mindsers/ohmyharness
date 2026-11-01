//! `omh account` — your accounts and what each keeps; `login` runs a
//! harness’s own login once and keeps what it wrote; `forget` takes one kept
//! preference back.
//!
//! Accounts are per harness, and which one a project uses is a project-level
//! setting, because that is how it actually varies. `crate::auth` owns the
//! capture and `crate::prefs` what an account keeps; this drives them and
//! reports.

use crate::adapter::{Adapter, ProviderFormat};
use crate::catalogue::Catalogue;
use crate::out;
use crate::paths::Paths;
use crate::session::Session;
use crate::{auth, container, image, memory, persist, report, runtime};
use anyhow::{Context, Result};
use std::collections::BTreeMap;

/// Every harness's accounts, the one a launch here uses, and what each keeps.
pub(crate) fn list(cwd: &std::path::Path, ctx: &out::Ctx) -> Result<()> {
    let paths = Paths::discover(cwd)?;
    let mut harnesses = Vec::new();
    let mut unread = 0usize;
    for adapter in Adapter::load_dir(&paths.adapters())? {
        let names = auth::accounts(&paths, &adapter)?;
        let configured = auth::configured(&paths, &adapter)?;
        // `resolve`'s rule, without its refusals: the one named, if a login
        // answers to it, or the only one.
        let selected = match (&configured, names.as_slice()) {
            (Some(name), _) => names.iter().find(|n| *n == name).cloned(),
            (None, [only]) => Some(only.clone()),
            (None, _) => None,
        };
        let mut accounts = Vec::new();
        for name in names {
            // One account omh could not read is said in its own row: `?` here
            // stopped the listing, and every harness after it went unlisted.
            let account =
                match crate::prefs::kept_by_file(&adapter, &auth::dir(&paths, &adapter, &name)) {
                    Ok(kept) => report::Account {
                        name,
                        keys: kept.iter().flat_map(|k| k.keys.clone()).collect(),
                        kept: kept.into_iter().map(|k| (k.file, k.doc)).collect(),
                        unread: None,
                    },
                    Err(e) => {
                        unread += 1;
                        report::Account {
                            name,
                            keys: Vec::new(),
                            kept: Vec::new(),
                            unread: Some(format!("{e:#}")),
                        }
                    }
                };
            accounts.push(account);
        }
        harnesses.push(report::HarnessAccounts {
            harness: adapter.name.to_string(),
            configured,
            selected,
            accounts,
        });
    }
    ctx.say(&report::Accounts { harnesses });
    // Listed whole, and still not a clean answer.
    anyhow::ensure!(
        unread == 0,
        "omh could not read what {unread} account{} keep{} — named above",
        if unread == 1 { "" } else { "s" },
        if unread == 1 { "s" } else { "" }
    );
    Ok(())
}

/// Take one preference from what an account keeps: the account a launch
/// here uses, or the one named.
pub(crate) fn forget(
    cwd: &std::path::Path,
    harness: &str,
    key: &str,
    account: Option<&str>,
    dry_run: bool,
    ctx: &out::Ctx,
) -> Result<()> {
    let paths = Paths::discover(cwd)?;
    let adapter = Adapter::find(&paths.adapters(), harness)?;
    let account = match account {
        Some(name) => {
            auth::validate_name(name)?;
            anyhow::ensure!(
                auth::is_captured(&paths, &adapter, name),
                "no account `{name}` for {harness} — `omh account` lists them"
            );
            name.to_string()
        }
        None => {
            let configured = auth::configured(&paths, &adapter)?;
            auth::resolve(&paths, &adapter, configured.as_deref())?
        }
    };
    let dir = auth::dir(&paths, &adapter, &account);
    let forgot = crate::prefs::forget(&adapter, &dir, &account, key, dry_run)?;
    let said = format!("{harness}'s {} on account `{account}`", forgot.key);
    let mut action = report::Action::new(
        "preference-forgotten",
        match dry_run {
            true => format!("would forget {said}"),
            false => format!("forgot {said}"),
        },
    )
    .data(serde_json::json!({
        "harness": harness,
        "account": account,
        "key": forgot.key,
        "file": forgot.file,
        "was": forgot.was,
    }));
    if !dry_run {
        action = action.note("a session already started keeps its own copy");
    }
    ctx.say(&action);
    Ok(())
}

/// Where `file` is under `root`: the directory to walk from and the names
/// below it, or `None` for a file elsewhere. Decided by where its spelling
/// enters `root`, not by the spelling alone — a symlinked home spells
/// `~/.omh` another way, and that path went to a read that follows links —
/// and never by the file's directory resolved, which a link out of `root`
/// leads elsewhere. So each directory above it is resolved from the top down:
/// the first that is `root` is where the walk starts, and the names below it
/// are handed over unresolved, for the read to refuse a link at any of them.
/// The first that is inside `root` instead is a way in no walk from `root`
/// takes, and is refused. A directory omh cannot resolve is said, not taken
/// for one elsewhere; one that is not there holds nothing.
fn under_root(
    root: &std::path::Path,
    file: &std::path::Path,
) -> Result<Option<(std::path::PathBuf, std::path::PathBuf)>> {
    let at = std::path::absolute(file).with_context(|| format!("resolving {}", file.display()))?;
    if let Ok(rel) = at.strip_prefix(root) {
        return Ok(Some((root.to_path_buf(), rel.to_path_buf())));
    }
    let resolved = |dir: &std::path::Path| match std::fs::canonicalize(dir) {
        Ok(dir) => Ok(Some(dir)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e).with_context(|| format!("resolving {}", dir.display())),
    };
    let Some(canonical) = resolved(root)? else {
        return Ok(None);
    };
    let mut above: Vec<&std::path::Path> = at.ancestors().skip(1).collect();
    above.reverse();
    for dir in above {
        let Some(is) = resolved(dir)? else {
            return Ok(None);
        };
        if is == canonical {
            return Ok(Some((canonical, at.strip_prefix(dir)?.to_path_buf())));
        }
        anyhow::ensure!(
            !is.starts_with(&canonical),
            "{} reaches {} through a link at {}: name it by its path under {}",
            file.display(),
            root.display(),
            dir.display(),
            root.display()
        );
    }
    Ok(None)
}

/// Capture an account: run the harness's own login in a sandbox, or with
/// `--import` copy the login this machine already holds.
pub(crate) fn login(
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
            // Under `~/.omh`, it is read the way omh reads what a session
            // could have written: a name at a time from `~/.omh` down, never
            // through a link at any of them, and capped. It was
            // `read_to_string`, which a session could steer with a link swapped
            // in after the refusal that offered the file was printed — at the
            // file, or at any directory above it. Anywhere else, it is yours,
            // read as you named it.
            let under = under_root(&paths.root, file)?;
            let named = under.is_some();
            let raw = match under {
                Some((root, rel)) => {
                    let rel = rel
                        .to_str()
                        .with_context(|| format!("{} is not UTF-8", file.display()))?;
                    let raw = crate::held::read_under(&root, rel, 1 << 20)
                        .with_context(|| format!("reading {}", file.display()))?;
                    anyhow::ensure!(!raw.is_empty(), "{} is empty, or not there", file.display());
                    raw
                }
                None => std::fs::read_to_string(file)
                    .with_context(|| format!("reading {}", file.display()))?,
            };
            // A file under `~/.omh` — an account's own models file, which a
            // launch's refusal offers to keep, or anything in a session's
            // reach — could be a session's, so nothing is read from the
            // environment for it, and a provider that would read it is
            // refused.
            //
            // Where it was named decides as much as where it leads: a link
            // under `~/.omh` is a session's to swap between the read above and
            // any look at its target. And a path omh cannot resolve is not
            // taken for one outside.
            let leads = match (
                std::fs::canonicalize(file),
                std::fs::canonicalize(&paths.root),
            ) {
                (Ok(file), Ok(root)) => file.starts_with(root),
                _ => true,
            };
            let ours = named || leads;
            let env = |k: &str| std::env::var(k).ok();
            let doc = match ours {
                false => author_provider(format, &raw, &account_dir, &env),
                true => author_provider(format, &raw, &account_dir, &|_| None).with_context(|| {
                    format!(
                        "it is under {}, which a session can write, so nothing in it is read \
                         from your environment — check it, copy it out, and name the copy",
                        paths.root.display()
                    )
                }),
            }
            .with_context(|| format!("{}", file.display()))?;
            let doc =
                doc.with_context(|| format!("{} defines no model provider", file.display()))?;
            // Kept, and said where it came from: a successful adoption of a
            // file a session could have written was silent.
            if ours {
                ctx.warn(&format!(
                    "kept the providers in {} — it is under {}, where a session could have \
                     written it: check them with `omh account` before you launch",
                    file.display(),
                    paths.root.display()
                ));
            }
            Some(doc)
        }
        (None, true, Some(source), Some(format)) => {
            let host = crate::adapter::expand_host(source, &home, &paths.repo);
            let env = |k: &str| std::env::var(k).ok();
            match read_host_provider(format, &host)? {
                Some((host, raw)) => author_provider(format, &raw, &account_dir, &env)
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
    // A provider kept with no login — `--provider`, or an import that found
    // only providers to bring — is the capture: there is no login to confirm.
    let decided = auth::decided_by_files(&adapter) || (provided.is_some() && imported.is_none());
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
/// defines none. Environment variables it reads are read here, once, from
/// `env`.
fn author_provider(
    format: ProviderFormat,
    raw: &str,
    account_dir: &std::path::Path,
    env: &dyn Fn(&str) -> Option<String>,
) -> Result<Option<Vec<String>>> {
    match format {
        ProviderFormat::CodexConfig => {
            let Some(doc) = crate::provider::codex::from_codex_config(raw, env)? else {
                return Ok(None);
            };
            crate::provider::codex::write(account_dir, &doc)?;
            Ok(Some(crate::provider::codex::names(&doc)))
        }
        ProviderFormat::OmpModels => {
            let Some(doc) = crate::provider::omp::from_models_config(raw, env)? else {
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
    match format {
        ProviderFormat::OmpModels => candidates.push(host.with_extension("yaml")),
        ProviderFormat::CodexConfig => {}
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
    let catalogue = Catalogue::resolve(paths);
    let backend = runtime::select(&crate::runtime_preference(paths)?, &|p| {
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

    let (own, repo) = crate::settings::resolved(paths)?;
    // The account's preferences, as a session has them: checked before
    // anything starts, then migrated and seeded — into the login's own copy,
    // under `scratch/` — before the login does.
    let prefs = crate::prefs::check(adapter, account_dir)?;

    let plan = container::plan(
        paths,
        &catalogue,
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
            recipe: crate::memory::expiry::Recipe::of(ca.as_ref()),
        },
    )?;
    plan.validate_for(&backend)?;
    let home = container::prefs_home(paths, &session, harness);
    if let Some(prefs) = &prefs {
        let running = || crate::cmd::session::sessions_mounting(&backend, account_dir);
        for note in prefs.apply(&running)? {
            ctx.warn(&note);
        }
        // A login mounts the account's own files, so nothing is a copy.
        for note in prefs.seed(adapter, &home, &BTreeMap::new(), false)? {
            ctx.warn(&note);
        }
    }
    auth::drop_carried(adapter, account_dir)?;
    image::ensure_network(&backend, &plan.network)?;
    let _network_cleanup = image::NetworkCleanup {
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
    let unfilled: Vec<std::path::PathBuf> =
        auth::unfilled(adapter, account_dir, crate::guest::GUEST_HOME)
            .iter()
            .map(|guest| {
                account_dir.join(
                    guest
                        .strip_prefix(crate::guest::GUEST_HOME)
                        .unwrap_or(guest.as_path()),
                )
            })
            .collect();
    auth::login_outcome(status.success(), &unfilled).map_err(|e| {
        e.context(format!(
            "run `omh account login {harness} --name {account}` again"
        ))
    })?;
    Ok(())
}
