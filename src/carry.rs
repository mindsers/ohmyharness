//! Carry-in.
//!
//! A git worktree holds only tracked files, so an agent lands somewhere with no
//! `.env`, no certs, no local config — a checkout that cannot run the app it is
//! supposed to work on. `carry_in` names what to copy across.
//!
//! It is deliberately an allowlist, because **this is the only path by which a
//! secret reaches the agent**. Carrying everything gitignored would sweep in
//! every credential you happen to have lying around.
//!
//! Copy, not symlink: a symlink's target would have to resolve inside the
//! sandbox, which would mean mounting your main checkout — exposing the
//! uncommitted work the worktree model exists to protect.

use anyhow::{Context, Result};
use std::io::Write;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Copied,
    /// The source changed since it was carried.
    Refreshed,
    Unchanged,
    /// Listed but not present in the checkout.
    Missing,
    /// Listed, and git already tracks it.
    ///
    /// A misconfiguration rather than a hazard: `carry_in` exists for what the
    /// worktree does *not* get, and a tracked file is already there on the
    /// branch. Carrying one replaces it with whatever the checkout holds right
    /// now, which shows up forever as a modification nobody made in the session
    /// — and, until `commit` learned to refuse it, went out in the commit.
    AlreadyTracked,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Carried {
    pub path: String,
    pub action: Action,
}

/// A pattern names something inside the repo, and nothing else.
///
/// `carry_in` is read from a *committed* layer, so a malicious or careless
/// entry like `../../.ssh` would copy host secrets into a sandbox the agent
/// controls.
pub fn validate_pattern(pattern: &str) -> Result<()> {
    let p = pattern.trim();
    if p.is_empty() {
        anyhow::bail!("an empty carry_in entry names nothing");
    }
    if p.starts_with('/') || p.starts_with('~') {
        anyhow::bail!("carry_in is relative to the repository: `{pattern}`");
    }
    if Path::new(p)
        .components()
        .any(|c| matches!(c, std::path::Component::ParentDir))
    {
        anyhow::bail!("carry_in cannot reach outside the repository: `{pattern}`");
    }
    Ok(())
}

/// Copy the listed paths from the checkout into the worktree.
pub fn apply(repo: &Path, worktree: &Path, patterns: &[String]) -> Result<Vec<Carried>> {
    // Validate everything before copying anything: a list with one bad entry
    // should not half-apply.
    for pattern in patterns {
        validate_pattern(pattern)?;
    }

    let mut out = Vec::new();
    for pattern in patterns {
        let rel = pattern.trim().trim_end_matches('/');
        let dst = worktree.join(rel);
        let action = match source(repo, rel)? {
            None => Action::Missing,
            Some(_) if tracked(repo, rel) => Action::AlreadyTracked,
            Some(src) if src.is_dir() => copy_dir(worktree, &src, &dst)?,
            Some(src) => copy_file(worktree, &src, &dst)?,
        };
        out.push(Carried {
            path: pattern.clone(),
            action,
        });
    }

    exclude(worktree, patterns)?;
    Ok(out)
}

/// Where a carried path is in the checkout, if it is there — reached without
/// following a link at it or at any directory above it.
///
/// `validate_pattern` keeps a pattern lexically inside the repository, and
/// that is where it stopped: a link at a carried name, or above one, was
/// followed, and what it pointed at was copied into the agent's worktree. Risk
/// §4 says a teammate's commit cannot copy your `~/.ssh` into a sandbox; a
/// committed `secrets -> ~/.ssh` under a carried `secrets/id_rsa` did.
fn source(repo: &Path, rel: &str) -> Result<Option<PathBuf>> {
    let mut at = repo.to_path_buf();
    for part in Path::new(rel).components() {
        at.push(part);
        match std::fs::symlink_metadata(&at) {
            Ok(meta) if meta.file_type().is_symlink() => anyhow::bail!(
                "carry_in does not follow links: `{rel}` is, or is under, {} — a link. \
                 Carry what it points at by its own path, inside the repository",
                at.display()
            ),
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e).with_context(|| format!("reading {}", at.display())),
        }
    }
    Ok(Some(at))
}

/// Stage the carried files for mounting, and say where each one landed.
///
/// The copy used to go straight into the worktree, which was right while `/work`
/// held no git. It does now, and `git clean -fdx` removes an untracked file
/// whether or not `info/exclude` names it — so the agent could delete the `.env`
/// its app needs and not get it back until the next launch.
///
/// A bind mount survives that, because a mountpoint cannot be **unlinked**. So
/// the copy is staged outside the worktree and mounted over a placeholder,
/// which leaves three properties the copy did not have: `git clean` reports
/// `Resource busy` and cleans everything else; the agent can still write to it,
/// because the mount is read-write; and what it writes lands on omh's copy
/// rather than on the file in the user's checkout.
///
/// Unlink is the whole of the guarantee, and two things scope it. `git clean`
/// exits **1** after the warning — it finishes the job and still reports
/// failure, so an agent running `git clean -fdx && …` or anything under
/// `set -e` sees its command fail. And `: > .env` truncates the file to zero
/// bytes, exit 0, straight through to the staged copy: the mount stops the file
/// being removed, not its contents being destroyed. That is inherent to being
/// writable, which is the point of it.
///
/// That last one is why this stages rather than mounting the source. Mounting
/// the checkout's own file read-write was measured letting an agent append to
/// the real `.env` — the exact reach outside the sandbox the worktree model
/// exists to prevent.
///
/// **Files only. A carried directory gets none of this.** `is_dir()` is skipped
/// below, so omh mounts nothing and the directory keeps `apply`'s plain copy —
/// measured, `git clean -fdx` prints `Removing certs/`, exits 0, and the
/// directory and everything in it is gone. Silently, and with a success code.
///
/// Mounting the directory instead would not have helped, which is why it is not
/// done: a directory mountpoint protects itself and not its contents, so
/// `clean` empties it first and only then reports `Resource busy` on the
/// directory — and because the mount is read-write, those deletions reach the
/// staged copy too. Protecting the contents means one mount per file, unbounded
/// in whatever the user chose to carry.
///
/// So a carried directory is exposed exactly as it was before this change. The
/// launcher says so at the point it carries one; this is the reason.
pub fn to_mount(repo: &Path, into: &Path, patterns: &[String]) -> Result<Vec<Staged>> {
    for pattern in patterns {
        validate_pattern(pattern)?;
    }
    let mut out: Vec<Staged> = Vec::new();
    for pattern in patterns {
        let rel = pattern.trim().trim_end_matches('/');
        // Absent and already-tracked are `apply`'s to report — it runs first and
        // says so to the user. Silently skipped here so the two cannot disagree
        // about what happened, only about what to do next. A link is refused,
        // as `apply` refuses it.
        let Some(src) = source(repo, rel)? else {
            continue;
        };
        if tracked(repo, rel) || src.is_dir() {
            continue;
        }
        // One entry, one mount. `carry_in = [".env", ".env/"]` both normalise to
        // the same `rel`, and docker refuses a plan with two mounts on one
        // destination — `Duplicate mount point: /work/.env`, a launch failure
        // naming a path the user never wrote. Redundant before this change,
        // fatal after it.
        if out.iter().any(|s: &Staged| s.rel == rel) {
            continue;
        }
        let at = into.join(rel);
        // The invariant `Staged` documents, checked where the value is made.
        debug_assert!(
            at.starts_with(into),
            "a staged file outside the staging directory is the checkout's own"
        );
        out.push(Staged {
            rel: rel.to_string(),
            host: at,
        });
    }
    Ok(out)
}

/// Put the bytes where `to_mount` said they would be.
///
/// Split from the decision because a dry run has to *name* the same mounts a
/// real launch would make without writing anything —
/// `skipped_staging_still_reports_the_real_mounts` asserts exactly that, and it
/// went red the moment a fixture had a `carry_in` entry. Deciding is reads of
/// the checkout and is safe either way; this is the half that is not.
pub fn materialise(repo: &Path, staged: &[Staged]) -> Result<()> {
    for item in staged {
        if let Some(parent) = item.host.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("making room for staged {}", item.rel))?;
        }
        let src = repo.join(&item.rel);
        std::fs::copy(&src, &item.host)
            .with_context(|| format!("staging {} to {}", src.display(), item.host.display()))?;
    }
    Ok(())
}

/// One carried file, staged and waiting to be mounted.
///
/// Unlike `Carried`, `Removed` and the rest of the reports in this crate, this
/// is not a record of something already decided — `host` *becomes* the source of
/// a read-write bind mount. "It is in the staging directory and never in the
/// checkout" is the whole of the security property here: a `host` pointing at
/// the checkout's own file mounts the user's real `.env` writable into the
/// sandbox, which is the reach this design exists to prevent.
///
/// Asserted where it is built rather than enforced by the type, which would
/// mean a fallible constructor for an invariant one function can uphold. If a
/// second producer ever appears, make the fields private.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Staged {
    /// Where it goes inside `/work`.
    pub rel: String,
    /// The staged copy, which is what gets mounted. Always under the staging
    /// directory this was asked to write into.
    pub host: PathBuf,
}

/// Whether git already has this path on the branch the session will start from.
///
/// Asked of the checkout rather than the worktree, because that is where the
/// `carry_in` entry is aimed and where the user can see the answer.
fn tracked(repo: &Path, rel: &str) -> bool {
    crate::git::users()
        .current_dir(repo)
        .args(["ls-files", "--error-unmatch", "--", rel])
        .output()
        .map(|out| out.status.success())
        .unwrap_or(false)
}

/// The checkout is the source of truth for carried files — they are yours, not
/// the agent's — so a changed source replaces the copy.
fn validate_destination(root: &Path, dst: &Path) -> Result<()> {
    let rel = dst
        .strip_prefix(root)
        .with_context(|| format!("{} is outside {}", dst.display(), root.display()))?;
    let mut at = root.to_path_buf();
    for component in rel.components() {
        at.push(component);
        match std::fs::symlink_metadata(&at) {
            Ok(meta) if meta.file_type().is_symlink() => {
                anyhow::bail!("carry_in destination {} is a symbolic link", at.display())
            }
            Ok(meta) if at != dst && !meta.is_dir() => {
                anyhow::bail!(
                    "carry_in destination parent {} is not a directory",
                    at.display()
                )
            }
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e).with_context(|| format!("inspecting {}", at.display())),
        }
    }
    Ok(())
}

fn copy_file(root: &Path, src: &Path, dst: &Path) -> Result<Action> {
    validate_destination(root, dst)?;
    let incoming = std::fs::read(src).with_context(|| format!("reading {}", src.display()))?;
    if std::fs::read(dst)
        .map(|existing| existing == incoming)
        .unwrap_or(false)
    {
        return Ok(Action::Unchanged);
    }
    let existed = std::fs::symlink_metadata(dst).is_ok();
    if let Some(parent) = dst.parent() {
        std::fs::create_dir_all(parent)?;
        validate_destination(root, parent)?;
        let mut part = tempfile::NamedTempFile::new_in(parent)?;
        part.write_all(&incoming)?;
        part.persist(dst)
            .map_err(|e| e.error)
            .with_context(|| format!("writing {}", dst.display()))?;
    }
    Ok(if existed {
        Action::Refreshed
    } else {
        Action::Copied
    })
}

fn copy_dir(root: &Path, src: &Path, dst: &Path) -> Result<Action> {
    validate_destination(root, dst)?;
    std::fs::create_dir_all(dst)?;
    let mut action = Action::Unchanged;
    for entry in std::fs::read_dir(src)? {
        // An entry that failed to read stops the copy rather than leaving it
        // short with nothing said.
        let entry = entry.with_context(|| format!("reading {}", src.display()))?;
        let from = entry.path();
        let to = dst.join(entry.file_name());
        // Not `is_dir`, which follows: a link inside a carried directory is
        // refused, as one at its name is.
        let kind = entry
            .file_type()
            .with_context(|| format!("reading {}", from.display()))?;
        anyhow::ensure!(
            !kind.is_symlink(),
            "carry_in does not follow links: {} is one",
            from.display()
        );
        let result = if kind.is_dir() {
            copy_dir(root, &from, &to)?
        } else {
            copy_file(root, &from, &to)?
        };
        // The directory as a whole is as changed as its most-changed member.
        if result != Action::Unchanged && action == Action::Unchanged {
            action = result;
        }
    }
    Ok(action)
}

/// Where a worktree's private ignore rules live.
///
/// Two traps here, both found empirically. `<worktree>/.git` is a *file*
/// pointing at the admin directory, not a directory — and git reads
/// `info/exclude` from the **common** git dir, not the per-worktree one, so
/// writing to `.git/worktrees/<id>/info/exclude` silently does nothing.
///
/// Consequence worth naming: this file is **your checkout's** as much as the
/// worktree's, so whatever omh writes here your own `git status` obeys too. It
/// holds the `carry_in` entries — files you keep out of git anyway, which is
/// why you carry them — and nothing else of omh's. It used to hold the names of
/// omh's mount placeholders as well, and after one launch an `AGENTS.md` you
/// created in your checkout was invisible to git; the host reads in
/// `session.rs` leave those out themselves now.
pub fn exclude_path(worktree: &Path) -> Option<PathBuf> {
    let out = crate::git::in_worktree()
        .current_dir(worktree)
        .args(["rev-parse", "--path-format=absolute", "--git-common-dir"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let dir = String::from_utf8_lossy(&out.stdout).trim().to_string();
    Some(PathBuf::from(dir).join("info/exclude"))
}

/// The filenames omh stages its rules onto, inside `/work`.
///
/// Named once because three things have to agree about them, in three modules:
/// the `concat` targets the adapters declare, `hidden_in_the_worktree` here,
/// and `Session::commit`'s backstop.
pub const STAGED_RULES: [&str; 2] = ["CLAUDE.md", "AGENTS.md"];

/// Every name a launch may leave in the worktree as a mount destination.
///
/// A bind mount needs its destination to exist, and docker will not create one
/// inside `/work`, so `container::place_destination` puts an empty file there
/// before every launch. Those files are omh's, and every host read that asks
/// the worktree what changed leaves them out: `Session::changed`, the staging
/// behind `diff` and `commit`, `rm`'s question and `materialise`.
///
/// **Checked against the mounts, not remembered.** This was `STAGED_RULES`
/// alone — two names kept by hand under a doc comment asking the next person to
/// remember — and `.mcp.json`, mounted into `/work` exactly like the rules
/// files, was never added. So on a session where the agent had done nothing,
/// `omh s` reported `1 uncommitted`, `omh sNN diff` showed `.mcp.json | 0` as
/// the session's work, and `omh sNN commit` put omh's own plumbing onto the
/// user's branch, where `omh sNN push` would publish it.
/// `every_placeholder_a_launch_leaves_in_the_worktree_is_one_omh_leaves_out`
/// walks the plan's `/work` mounts and fails if any name is missing here, so
/// the list cannot fall behind the mounts the way it did.
///
/// **At the worktree's root, and only while untracked** — what
/// `place_destination` makes, since it creates a file and never writes one. A
/// repo carrying its own `.mcp.json` keeps it: the mount hides it for the
/// length of the session and nothing else. An `AGENTS.md` the agent writes for
/// one package is its work. These used to be patterns in `info/exclude`, which
/// matched at every depth and, being your checkout's file too, hid your own.
pub fn hidden_in_the_worktree() -> Vec<&'static str> {
    STAGED_RULES.into_iter().chain([".mcp.json"]).collect()
}

/// Keep carried files out of every host-side `git status` of the worktree, so
/// they never show up as the session's work or get committed onto its branch.
///
/// The sandbox's own repository has its own list, built from the mounts; this
/// one is what `omh s`, `diff` and `commit` read on the host.
///
/// **Lines added, never a file rewritten.** It is your file, and it read as
/// UTF-8 and was written back whole: one byte outside UTF-8 read as an empty
/// file, and the write replaced every rule in it with omh's. So the bytes are
/// read as bytes, only a missing file is an empty one, and a launch with
/// nothing to add writes nothing.
fn exclude(worktree: &Path, patterns: &[String]) -> Result<()> {
    let Some(path) = exclude_path(worktree) else {
        // A scratch directory has no git at all; nothing to keep clean.
        return Ok(());
    };
    let body = match std::fs::read(&path) {
        Ok(body) => body,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(e) => return Err(e).with_context(|| format!("reading {}", path.display())),
    };
    let missing = missing_lines(&body, patterns);
    if missing.is_empty() {
        return Ok(());
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .with_context(|| format!("opening {}", path.display()))?;
    file.write_all(&missing)
        .with_context(|| format!("writing {}", path.display()))?;
    Ok(())
}

/// What `exclude` appends: each pattern the file does not already hold as a
/// line of its own, newline-terminated — after a newline of its own first when
/// the file's last line has none, so a pattern is never glued onto yours.
fn missing_lines(body: &[u8], patterns: &[String]) -> Vec<u8> {
    let held = |line: &str| {
        body.split(|b| *b == b'\n')
            .any(|l| l.trim_ascii() == line.as_bytes())
    };
    let mut out: Vec<u8> = Vec::new();
    for pattern in patterns {
        let line = pattern.trim();
        let already_added = out.split(|b| *b == b'\n').any(|l| l == line.as_bytes());
        if line.is_empty() || held(line) || already_added {
            continue;
        }
        if out.is_empty() && !body.is_empty() && !body.ends_with(b"\n") {
            out.push(b'\n');
        }
        out.extend_from_slice(line.as_bytes());
        out.push(b'\n');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The source side follows no link either. `carry_in` is validated
    /// against escaping the repository — risk §4: a teammate's commit must
    /// not be able to copy your `~/.ssh` into a sandbox — and the check was
    /// lexical only: a link at a carried name, or above it, was followed, and
    /// what it pointed at was copied into the agent's worktree. Refused, and
    /// nothing copied.
    #[cfg(unix)]
    #[test]
    fn a_carried_source_that_is_or_passes_through_a_link_is_refused() {
        use std::os::unix::fs::symlink;
        for carried in ["certs/", "secrets/id_rsa", "conf/"] {
            let dir = tempfile::tempdir().unwrap();
            let repo = dir.path().join("repo");
            let worktree = dir.path().join("worktree");
            let ssh = dir.path().join("ssh");
            std::fs::create_dir_all(&repo).unwrap();
            std::fs::create_dir_all(&worktree).unwrap();
            std::fs::create_dir_all(&ssh).unwrap();
            std::fs::write(ssh.join("id_rsa"), "PRIVATE KEY\n").unwrap();
            symlink(&ssh, repo.join("certs")).unwrap();
            symlink(&ssh, repo.join("secrets")).unwrap();
            // A real directory, holding a link: refused inside as at the top.
            std::fs::create_dir_all(repo.join("conf")).unwrap();
            symlink(ssh.join("id_rsa"), repo.join("conf/key")).unwrap();

            let result = apply(&repo, &worktree, &[carried.into()]);
            assert!(result.is_err(), "{carried}: a linked source was carried");
            let copied = walk(&worktree)
                .into_iter()
                .any(|p| std::fs::read_to_string(&p).is_ok_and(|b| b.contains("PRIVATE KEY")));
            assert!(!copied, "{carried}: the link's target reached the worktree");
        }
    }

    /// Every file under `dir`.
    fn walk(dir: &Path) -> Vec<PathBuf> {
        let mut out = Vec::new();
        let Ok(entries) = std::fs::read_dir(dir) else {
            return out;
        };
        for entry in entries {
            let p = entry.unwrap().path();
            if p.is_dir() {
                out.extend(walk(&p));
            } else {
                out.push(p);
            }
        }
        out
    }

    #[cfg(unix)]
    #[test]
    fn a_carried_file_does_not_follow_a_destination_symlink() {
        use std::os::unix::fs::symlink;

        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().join("repo");
        let worktree = dir.path().join("worktree");
        let unrelated = dir.path().join("unrelated");
        std::fs::create_dir_all(&repo).unwrap();
        std::fs::create_dir_all(&worktree).unwrap();
        std::fs::write(repo.join(".env"), "new\n").unwrap();
        std::fs::write(&unrelated, "keep\n").unwrap();
        symlink(&unrelated, worktree.join(".env")).unwrap();

        let result = apply(&repo, &worktree, &[".env".into()]);

        assert!(result.is_err(), "a linked destination must be refused");
        assert_eq!(std::fs::read_to_string(unrelated).unwrap(), "keep\n");
    }

    #[cfg(unix)]
    #[test]
    fn a_carried_file_does_not_traverse_a_destination_symlinked_parent() {
        use std::os::unix::fs::symlink;

        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().join("repo");
        let worktree = dir.path().join("worktree");
        let unrelated = dir.path().join("unrelated");
        std::fs::create_dir_all(repo.join("config")).unwrap();
        std::fs::create_dir_all(&worktree).unwrap();
        std::fs::create_dir_all(&unrelated).unwrap();
        std::fs::write(repo.join("config/app"), "new\n").unwrap();
        symlink(&unrelated, worktree.join("config")).unwrap();

        let result = apply(&repo, &worktree, &["config/app".into()]);

        assert!(result.is_err(), "a linked parent must be refused");
        assert!(!unrelated.join("app").exists());
    }

    /// The three skips guard `materialise`'s `fs::copy`, which fails on a
    /// missing source and on a directory. Drop either and the error propagates
    /// out of `plan` and no session launches at all — for a `carry_in` naming a
    /// file the user does not currently have, or a directory, which is what
    /// omh's own generated settings file suggests (`certs/`).
    ///
    /// Both cases were graceful and *reported* before mounting existed:
    /// `Action::Missing` says "not in this checkout", and a directory gets
    /// copied. One line in a skip condition is what keeps that true.
    #[test]
    fn nothing_is_staged_for_a_path_that_cannot_be_copied() {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().join("repo");
        let into = dir.path().join("staged");
        std::fs::create_dir_all(repo.join("certs")).unwrap();
        std::fs::write(repo.join("certs/ca.pem"), "cert\n").unwrap();

        let staged =
            to_mount(&repo, &into, &["gone.env".to_string(), "certs".to_string()]).unwrap();

        assert!(
            staged.is_empty(),
            "a missing file and a directory are both left to `apply`: {staged:?}"
        );
        // and the half that turns a skip into an outage
        materialise(&repo, &staged).expect("nothing to copy is not a failure");
    }

    /// `validate_pattern` runs here as well as in `apply`, and the duplication
    /// is load-bearing rather than redundant: the two are reached from
    /// different callers, and this module's own doc calls `carry_in` "the only
    /// path by which a secret reaches the agent". Deleting the loop left the
    /// whole suite green.
    #[test]
    fn a_pattern_that_escapes_the_repo_is_refused_before_anything_is_staged() {
        let dir = tempfile::tempdir().unwrap();
        assert!(to_mount(
            dir.path(),
            &dir.path().join("staged"),
            &["../../.ssh/id_rsa".to_string()]
        )
        .is_err());
    }

    /// One entry, one mount. Two patterns that normalise to the same path made
    /// docker refuse the launch with `Duplicate mount point: /work/.env`, a
    /// failure naming a path the user never typed.
    #[test]
    fn two_patterns_for_one_path_produce_one_mount() {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        std::fs::write(repo.join(".env"), "S=1\n").unwrap();

        let staged = to_mount(
            &repo,
            &dir.path().join("staged"),
            &[".env".to_string(), ".env/".to_string()],
        )
        .unwrap();

        assert_eq!(staged.len(), 1, "{staged:?}");
    }

    fn repo(files: &[(&str, &str)]) -> (tempfile::TempDir, PathBuf, PathBuf) {
        let d = tempfile::tempdir().unwrap();
        let repo = d.path().join("repo");
        let worktree = d.path().join("wt");
        std::fs::create_dir_all(repo.join(".git/info")).unwrap();
        std::fs::create_dir_all(&worktree).unwrap();
        for (name, body) in files {
            let p = repo.join(name);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, body).unwrap();
        }
        (d, repo, worktree)
    }

    fn carry(repo: &Path, wt: &Path, list: &[&str]) -> Vec<Carried> {
        let owned: Vec<String> = list.iter().map(|s| s.to_string()).collect();
        apply(repo, wt, &owned).unwrap()
    }

    // ── copying ─────────────────────────────────────────────────────────────

    #[test]
    fn a_listed_file_reaches_the_worktree() {
        let (_d, repo, wt) = repo(&[(".env.local", "SECRET=1")]);
        let out = carry(&repo, &wt, &[".env.local"]);

        assert_eq!(
            std::fs::read_to_string(wt.join(".env.local")).unwrap(),
            "SECRET=1"
        );
        assert_eq!(out[0].action, Action::Copied);
    }

    #[test]
    fn a_listed_directory_is_carried_whole() {
        let (_d, repo, wt) = repo(&[("certs/dev.pem", "cert"), ("certs/nested/ca.pem", "ca")]);
        carry(&repo, &wt, &["certs/"]);

        assert_eq!(
            std::fs::read_to_string(wt.join("certs/dev.pem")).unwrap(),
            "cert"
        );
        assert_eq!(
            std::fs::read_to_string(wt.join("certs/nested/ca.pem")).unwrap(),
            "ca"
        );
    }

    /// A `.env` you thought you were carrying and are not is exactly the
    /// failure that wastes an hour inside the sandbox.
    #[test]
    fn a_listed_path_that_does_not_exist_is_reported() {
        let (_d, repo, wt) = repo(&[]);
        let out = carry(&repo, &wt, &[".env.local"]);
        assert_eq!(out[0].action, Action::Missing);
        assert_eq!(out[0].path, ".env.local");
    }

    #[test]
    fn nothing_listed_carries_nothing() {
        let (_d, repo, wt) = repo(&[(".env", "x")]);
        assert!(carry(&repo, &wt, &[]).is_empty());
        assert!(!wt.join(".env").exists());
    }

    // ── re-running ──────────────────────────────────────────────────────────

    #[test]
    fn an_unchanged_file_is_not_copied_again() {
        let (_d, repo, wt) = repo(&[(".env", "A=1")]);
        carry(&repo, &wt, &[".env"]);
        let out = carry(&repo, &wt, &[".env"]);
        assert_eq!(out[0].action, Action::Unchanged);
    }

    /// The checkout is the source of truth for these files — they are yours,
    /// not the agent's.
    #[test]
    fn a_changed_source_refreshes_the_copy() {
        let (_d, repo, wt) = repo(&[(".env", "A=1")]);
        carry(&repo, &wt, &[".env"]);
        std::fs::write(repo.join(".env"), "A=2").unwrap();

        let out = carry(&repo, &wt, &[".env"]);
        assert_eq!(out[0].action, Action::Refreshed);
        assert_eq!(std::fs::read_to_string(wt.join(".env")).unwrap(), "A=2");
    }

    // ── the agent's git status ──────────────────────────────────────────────

    /// A **real** git worktree, because that is where this broke: `.git` in a
    /// worktree is a *file* pointing at the admin directory, not a directory,
    /// so writing `<worktree>/.git/info/exclude` silently does nothing.
    fn worktree_repo() -> (tempfile::TempDir, PathBuf, PathBuf) {
        let d = tempfile::tempdir().unwrap();
        let repo = d.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        let git = |args: &[&str]| {
            std::process::Command::new("git")
                .current_dir(&repo)
                .args(args)
                .output()
                .unwrap()
        };
        git(&["init", "-q", "-b", "main"]);
        git(&["config", "user.email", "t@e.com"]);
        git(&["config", "user.name", "t"]);
        git(&["commit", "-q", "--allow-empty", "-m", "root"]);
        let wt = d.path().join("wt");
        git(&[
            "worktree",
            "add",
            "-q",
            wt.to_str().unwrap(),
            "-b",
            "omh/s01",
        ]);
        std::fs::write(repo.join(".env.local"), "SECRET").unwrap();
        (d, repo, wt)
    }

    fn status(wt: &Path) -> String {
        let out = std::process::Command::new("git")
            .current_dir(wt)
            .args(["status", "--short"])
            .output()
            .unwrap();
        String::from_utf8_lossy(&out.stdout).into_owned()
    }

    /// `carry_in` is for what a worktree does not get. A tracked file is
    /// already on the branch, so listing one is a misconfiguration — and a
    /// quietly expensive one: the copy lands as a modification nobody made in
    /// the session, and `commit` then has to refuse it. Named at launch, where
    /// the entry can actually be fixed.
    #[test]
    fn a_carry_in_entry_git_already_tracks_is_reported_rather_than_copied() {
        let (_d, repo, wt) = worktree_repo();
        std::fs::write(repo.join("config.toml"), "PORT=3000\n").unwrap();
        for args in [vec!["add", "-A"], vec!["commit", "-q", "-m", "config"]] {
            std::process::Command::new("git")
                .current_dir(&repo)
                .args(&args)
                .output()
                .unwrap();
        }
        // The user's local edit, which is what carrying would copy across.
        std::fs::write(repo.join("config.toml"), "PORT=3000\nSECRET=hunter2\n").unwrap();

        let out = apply(&repo, &wt, &["config.toml".to_string()]).unwrap();

        assert_eq!(out[0].action, Action::AlreadyTracked);
        assert!(
            !wt.join("config.toml").exists(),
            "a tracked path is git's to deliver, and carrying it would have written \
             the checkout's uncommitted copy over whatever the branch holds"
        );
    }

    /// Carried files must not appear as untracked, or the agent commits your
    /// `.env` onto the session branch.
    #[test]
    fn carried_paths_are_hidden_from_the_agents_git_status() {
        let (_d, repo, wt) = worktree_repo();
        carry(&repo, &wt, &[".env.local"]);

        assert!(wt.join(".env.local").exists(), "carried");
        assert!(
            !status(&wt).contains(".env.local"),
            "must not be untracked noise:\n{}",
            status(&wt)
        );
    }

    #[test]
    fn excluding_twice_does_not_duplicate_entries() {
        let (_d, repo, wt) = worktree_repo();
        carry(&repo, &wt, &[".env.local"]);
        carry(&repo, &wt, &[".env.local"]);
        assert!(!status(&wt).contains(".env.local"));
    }

    #[test]
    fn an_existing_exclude_file_is_preserved() {
        let (_d, repo, wt) = worktree_repo();
        let path = exclude_path(&wt).unwrap();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "*.swp\n").unwrap();
        carry(&repo, &wt, &[".env.local"]);

        let body = std::fs::read_to_string(&path).unwrap();
        assert!(
            body.contains("*.swp"),
            "the user's rules must survive: {body}"
        );
    }

    // omh's own mount placeholders are not this file's to hide any more: it is
    // your checkout's exclude file too, and hiding them here hid your own
    // `AGENTS.md`. The host reads leave them out themselves, which
    // `an_untouched_session_holds_nothing_but_omhs_placeholders` holds; that
    // this file gets only the carried entries is
    // `a_launch_changes_nothing_about_what_your_checkout_ignores`.

    // ── escaping the repo ───────────────────────────────────────────────────

    #[test]
    fn ordinary_patterns_are_accepted() {
        for p in [".env", ".env.local", "certs/", "config/local.toml"] {
            validate_pattern(p).unwrap_or_else(|e| panic!("{p}: {e}"));
        }
    }

    /// `carry_in` lives in a committed layer, so a careless entry would copy
    /// host secrets into a sandbox the agent controls.
    #[test]
    fn a_pattern_cannot_escape_the_checkout() {
        for p in [
            "../secrets",
            "../../.ssh",
            "a/../../b",
            "/etc/passwd",
            "~/.ssh",
        ] {
            assert!(validate_pattern(p).is_err(), "`{p}` must be rejected");
        }
    }

    #[test]
    fn an_escaping_pattern_copies_nothing() {
        let (_d, repo, wt) = repo(&[]);
        let owned = vec!["../../.ssh".to_string()];
        assert!(
            apply(&repo, &wt, &owned).is_err(),
            "must refuse, not silently skip"
        );
    }
}
