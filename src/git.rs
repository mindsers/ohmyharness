//! Every git omh starts, named by whose tree it runs in.
//!
//! Host-side git over trees the agent writes is where a sandbox reaches the
//! host: a hook, a filter, a pager or an `fsmonitor` the agent named runs as
//! the user. The rule against that was held by naming — `session.rs` had
//! `NO_HOOKS` and its own runners, `shadow.rs` had `NEUTRALISED` and others,
//! and twenty-nine `Command::new("git")` each chose between them, or forgot
//! to, where they stood. Every spawn starts here now, from a builder that says
//! whose tree it is, and `no_git_starts_outside_the_git_module` keeps it so.
//!
//! The builders differ in what they believe, not in what they run:
//!
//! - [`users`] — the user's own checkout, or no repository at all. Their
//!   config and their hooks, as git would run them for them.
//! - [`in_worktree`] — a session's worktree: the user's repository, with
//!   files the agent wrote. No hooks: nobody asked omh to run them there.
//! - [`agents`] — a repository the agent writes, gitdir and all, or a scratch
//!   checkout of one. No setting in it that can execute or mislead.
//! - [`landing`] — commit and push in a worktree, which run the user's own
//!   hooks on purpose; `hooks` says whose those are.

use std::process::Command;

/// What a host-side git in a tree the agent writes is told about hooks: run
/// none. [`NO_HOOKS`] passes it alone and [`NEUTRALISED`] among the rest.
///
/// `/dev/null` rather than the empty value, which the agent's repositories
/// were once given. Both run nothing — measured against git 2.56.0 — but this
/// one names a place that cannot hold a hook, where `rev-parse --git-path
/// hooks` reads the empty value back as `./`.
const HOOKS_OFF: &str = "core.hooksPath=/dev/null";

/// [`HOOKS_OFF`], as arguments.
pub(crate) const NO_HOOKS: [&str; 2] = ["-c", HOOKS_OFF];

/// Config keys that turn `git` into "run whatever this repository says".
///
/// The gitdir is a read-write mount because the agent commits through it, and
/// the `config` inside it is **not**: `write_config` rewrites that file to a
/// ten-key allowlist on every launch and `container::plan` mounts it read-only
/// (#52). So these keys are omh's second line, not its first — an earlier
/// version of this comment called them all agent-writable, which stopped being
/// true when the mount landed and was still being quoted as a premise
/// afterwards.
///
/// What the agent *can* still write is everything else in the gitdir:
/// `refs/replace/`, `info/grafts`, `info/attributes`. Those are the ones with
/// no config key at all, and they are handled beside this list.
///
/// **These fire.** An earlier version of this comment said nothing host-side
/// read an existing shadow, and that stopped being true twice: `harvest`
/// fetches and replays those commits on the host as the user, and
/// `checkpoints` reads the whole history on every `omh sNN log`. A reader who
/// reached the old sentence first would have concluded the list was inert and
/// could be relaxed.
///
/// Two of them are not about execution at all. `core.useReplaceRefs` and the
/// `GIT_GRAFT_FILE` in `GUEST_ENV` are about the answer being *true*: both let
/// the agent rewrite what a read prints without touching a commit, and the one
/// command that exists to be believed about the agent's work is the one that
/// reads this repository.
pub(crate) const NEUTRALISED: [&str; 8] = [
    // `ensure` has carried this since the seed commit and said why: a global
    // `commit.gpgsign = true` fails with `gpg failed to sign the data`, over a
    // commit the user never made in a repository they cannot see. The harvest
    // then added three more commit sites and read none of that — the
    // checkpoint, the replant and the stamp all died for anyone who signs by
    // default. `--no-verify` covers hooks and nothing of signing.
    "commit.gpgsign=false",
    HOOKS_OFF,
    "core.pager=cat",
    "core.fsmonitor=",
    "core.sshCommand=",
    "protocol.file.allow=never",
    // `git diff` runs an external driver by default; the log family will not
    // without `--ext-diff`, which nothing here passes. Both are the agent's to
    // name — this config lives in a read-write mount — so the key is closed
    // rather than left to depend on which verb a future reader picks.
    "diff.external=",
    // A commit the agent replaced reads as whatever it replaced it with.
    // Measured 2026-08-23 against git 2.55.0, in a gitdir the agent owns:
    // `git replace -f <real> <forged>` and the log prints the forged subject
    // and the forged (empty) numstat beside the *real* commit id, with no
    // warning on either stream. `--no-replace-objects` and this key both stop
    // it; the key is here because this list is the mechanism every host-side
    // read already carries.
    //
    // This one is not about executing anything — it is about being believed.
    // `omh sNN log` exists to tell you what the agent did.
    "core.useReplaceRefs=false",
];

/// The user's own checkout, or no repository at all.
pub(crate) fn users() -> Command {
    Command::new("git")
}

/// A session's worktree: no hooks run.
pub(crate) fn in_worktree() -> Command {
    let mut git = Command::new("git");
    git.args(NO_HOOKS);
    git
}

/// [`in_worktree`] as a line for the user to type: what a refusal prints when
/// it hands a step in a session's worktree back to them. Printing a command is
/// running it by other means, and typed bare in that worktree `git` runs hooks
/// from a relative `core.hooksPath` among the agent's files. `rest` is the verb
/// and its arguments, as shell words.
pub(crate) fn typed_in_worktree(tree: &std::path::Path, rest: &str) -> String {
    format!(
        "git -c {HOOKS_OFF} -C {} {rest}",
        crate::quote::sh(&tree.to_string_lossy())
    )
}

/// A repository the agent writes, or a scratch checkout of one: every key in
/// [`NEUTRALISED`] closed. The caller adds what else the tree needs — the
/// environment that keeps a graft file out, a `--git-dir`.
pub(crate) fn agents() -> Command {
    let mut git = Command::new("git");
    git.args(NEUTRALISED.iter().flat_map(|kv| ["-c", kv]));
    git
}

/// Landing — commit and push — which runs the user's own hooks on purpose,
/// and only the ones `hooks` names (`session::Hooks`).
pub(crate) fn landing(hooks: &[&str]) -> Command {
    let mut git = Command::new("git");
    git.args(hooks);
    git
}
