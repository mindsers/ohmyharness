//! `omh auth` — run a harness’s own login once, and keep what it wrote.
//!
//! Accounts are per harness, and which one a project uses is a project-level
//! setting, because that is how it actually varies. `crate::auth` owns the
//! capture; this drives it and reports.

use crate::adapter::Adapter;
use crate::out;
use crate::profile::{Paths, Profile};
use crate::session::Session;
use crate::{auth, container, image, memory, persist, report, runtime};
use anyhow::Result;
use std::collections::BTreeMap;
use std::process::Command;

/// Run the harness's own login inside a sandbox, with this account's credential
/// files bind-mounted writable. There is no separate capture step: the login
/// writes straight through to the host.
pub(crate) fn auth_cmd(
    cwd: &std::path::Path,
    harness: &str,
    account: &str,
    ctx: &out::Ctx,
) -> Result<()> {
    let paths = Paths::discover(cwd)?;
    let profile = Profile::resolve(&paths);
    let adapter = Adapter::find(&paths.adapters(), harness)?;

    if adapter.creds.is_empty() {
        anyhow::bail!(
            "adapter {harness} declares no credential paths, so there is nothing to capture"
        );
    }

    auth::validate_name(account)?;
    let account_dir = auth::dir(&paths, harness, account);
    let already = auth::is_captured(&paths, &adapter, account);
    auth::prepare(&adapter, &account_dir, "/home/agent")?;

    let backend = runtime::select(&crate::runtime_preference(&paths), &|p| {
        runtime::installed(p)
    })?;
    let ca = image::ca_for(&paths)?;
    image::ensure(
        backend.program(),
        &adapter,
        ca.as_ref().map(image::Root::pem),
    )?;

    // A throwaway: logging in must not leave a branch behind.
    let session = Session::scratch(paths.scratch("auth"), "auth".into());
    session.ensure(&paths.repo, "")?;
    let (own, repo) = crate::cmd::session::resolved(&paths)?;

    let plan = container::plan(
        &paths,
        &profile,
        &adapter,
        &session,
        &[],
        container::Options {
            staging: container::Staging::Apply,
            persist: persist::Mode::None,
            tty: true,
            account_dir: Some(account_dir.clone()),
            memory_bin: memory::deliver::available(&paths, ctx),
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
            // the same in both images.
            image: image::tag_for(&adapter, ca.as_ref().map(image::Root::pem)),
            // So nothing has been measured about it here, and nothing is
            // suppressed. That is the safe direction — a login session running
            // one hook too many costs nothing, and this container exists for
            // the length of an OAuth redirect.
            resolves: BTreeMap::new(),
        },
    )?;
    plan.validate(&backend.caps())?;
    image::ensure_network(backend.program(), &plan.network)?;

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
    let status = Command::new(backend.program())
        .args(backend.args(&plan))
        .status()?;
    if let Err(e) = session.remove(&paths.repo, "", &paths.shadows()) {
        // A leftover `auth` worktree wins `session::current()` and silently
        // becomes the session the next launch runs in.
        ctx.warn(&format!("could not remove the auth worktree: {e}"));
    }

    // Host paths, not guest ones: the guest path names a container that has
    // already been torn down and that the user cannot inspect.
    let unfilled: Vec<std::path::PathBuf> =
        auth::unfilled(&adapter, &account_dir, auth::GUEST_HOME)
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
    let all = auth::accounts(&paths, &adapter);
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
    action = action.data(serde_json::json!({
        "harness": harness,
        "account": account,
        "reauthenticated": already,
        "credentials": account_dir.display().to_string(),
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
