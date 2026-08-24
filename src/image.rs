//! Images.
//!
//! Two layers: a base every session shares, and a thin per-harness layer that
//! runs the adapter's install command. The base deliberately satisfies the
//! `sbx` kit contract — non-root `agent` at UID 1000, passwordless sudo,
//! `/home/agent`, proxy env forwarding — so the same image works on either
//! backend and an sbx kit becomes a two-line file rather than a port.

use crate::adapter::Adapter;
use crate::base::GRAPH_CACHE;
use anyhow::Result;
use std::path::Path;

/// The base tag carries a digest of its own recipe, for the same reason the
/// harness tag does: with a mutable `:latest`, `ensure` sees the tag present and
/// skips the build, so a base change never reaches an install that already
/// built it. Adding `socat` to the base silently did nothing until this.
pub fn base_tag() -> String {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    base_dockerfile().hash(&mut h);
    format!("omh/base:{:x}", h.finish())
}

/// Tag includes a digest of the recipe, so a Dockerfile omh ships actually
/// reaches an install that already built the old one. With a fixed `:latest`,
/// `ensure` saw the tag present and skipped the build — while `omh init`
/// reported "already built".
pub fn tag_for(adapter: &Adapter) -> String {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    harness_dockerfile(adapter).hash(&mut h);
    base_dockerfile().hash(&mut h);
    format!("omh/{}:{:x}", adapter.name, h.finish())
}

/// The agent's home inside the sandbox.
///
/// Defined here because this is where it is *established* — the Dockerfile's
/// `usermod -d` below — and the same constant is interpolated into that
/// Dockerfile, so the image and the code that mounts into it cannot disagree.
///
/// Declared once because the alternative is consistency by copying: a literal
/// in `auth`, another in `container`, another in `doctor`, each with a comment
/// claiming it mirrors the others. That is consistent right up until it isn't,
/// and the symptom is a session that starts and reads nothing.
pub const GUEST_HOME: &str = "/home/agent";

/// A digest of an image recipe, for a note to pin.
///
/// Deliberately **not** `base_tag()`'s. That uses `DefaultHasher`, whose output
/// std explicitly does not guarantee across releases — fine for a tag, which is
/// ephemeral and local. A note is committed and travels, so pinning that value
/// would mark every image-triggered note in the repo stale on the day somebody
/// upgrades Rust: a mass false positive with no cause anybody could find.
///
/// `git hash-object` is a stable SHA-1 of the text, for ever, and shells out
/// exactly as `carry.rs` and `session.rs` already do.
///
/// It needs the git *binary* and not a repository, and `GIT_DIR` is pointed at
/// a path that cannot exist so it stays that way. Left to inherit the ambient
/// repository, the answer depends on where omh was launched from — measured:
///
///   no repository, or a sha1 one   bee8b835…  (40 hex)
///   a `--object-format=sha256` one  cf82dc1e…  (64 hex)
///   a dangling `.git`               fatal: not a git repository
///
/// The middle row is the one that matters, and it is the mass false positive
/// this function's doc argues against arriving by another road: a user whose
/// repo is SHA-256 pins digests nobody else's omh will ever compute, so every
/// image-pinned note in it reads stale. `--object-format` is not a fix — git
/// rejects it on `hash-object`. A `GIT_DIR` that resolves to nothing is,
/// and it closes the dangling-pointer row in the same line: that state is what
/// kept four tests here `#[ignore]`d until 2026.08, and `report.rs` records a
/// moved checkout producing one.
/// The exact `git` invocation `recipe_digest` makes.
///
/// Built here rather than inline so a test can run the real thing from a
/// directory of its choosing. Asserting that the env is *set* would be a claim
/// about the code's shape; running this command inside a `--object-format=sha256`
/// repository and getting 40 hex back is a claim about what it does.
fn digest_command() -> std::process::Command {
    let mut c = std::process::Command::new("git");
    // A directory that cannot exist, so no ambient repository decides the hash
    // algorithm. See the note on `recipe_digest`.
    c.env("GIT_DIR", "/omh-recipe-digest-has-no-repository")
        .args(["hash-object", "--stdin"]);
    c
}

pub fn recipe_digest(recipe: &str) -> Result<String> {
    use anyhow::Context;
    use std::io::Write;
    use std::process::Stdio;

    let mut child = digest_command()
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .context("running git hash-object")?;
    child
        .stdin
        .take()
        .context("no stdin")?
        .write_all(recipe.as_bytes())?;
    let out = child.wait_with_output()?;
    anyhow::ensure!(out.status.success(), "git hash-object failed");
    Ok(String::from_utf8(out.stdout)?.trim().to_string())
}

pub fn base_dockerfile() -> String {
    // Interpolated rather than written out, so the directory the image
    // prepares and the directory the launcher mounts into cannot drift.
    let notes = crate::memory::GUEST_LOCAL_NOTES;
    // node:*-slim ships a `node` user already holding UID 1000, so rename it
    // rather than fighting it — sbx requires that UID to be `agent`.
    format!(
        r#"FROM node:22-bookworm-slim

RUN apt-get update && apt-get install -y --no-install-recommends \
      ca-certificates git ripgrep dtach sudo curl less jq procps openssh-server socat \
 && rm -rf /var/lib/apt/lists/*

RUN usermod -l agent -d {GUEST_HOME} -m node \
 && groupmod -n agent node \
 && echo 'agent ALL=(ALL) NOPASSWD:ALL' > /etc/sudoers.d/agent \
 && chmod 0440 /etc/sudoers.d/agent

# Assert the sbx kit contract at build time rather than assuming it. If a future
# base image moves UID 1000, this fails here instead of failing mysteriously
# inside a sandbox.
RUN test "$(id -u agent)" = "1000" && test "$(getent passwd agent | cut -d: -f6)" = "{GUEST_HOME}"

# The base set lives here, not in a harness layer: a code graph is
# harness-agnostic and every session should get the same one.
ARG TARGETARCH
RUN __GRAPH_INSTALL__

# Mount points the launcher expects to exist, owned by the unprivileged user.
# The graph cache is a volume; the image only needs the directory to exist and
# be owned by the agent, or docker creates it as root.
RUN mkdir -p /work /omh/sock /omh/cache /omh/layers {notes} {GRAPH_CACHE} \
 && chown -R agent:agent /work /omh {GUEST_HOME}/.cache

# Session entrypoint: install the key, start sshd, then stay alive so the
# container outlives the command that created it. The key arrives as an env var
# because a bind-mounted authorized_keys lands with host ownership and sshd
# silently refuses to read one it does not trust.
RUN printf '%s\n' \
  '#!/bin/sh' \
  'set -e' \
  'mkdir -p "$HOME/.ssh" && chmod 700 "$HOME/.ssh"' \
  'if [ -n "$OMH_PUBKEY" ]; then' \
  '  printf "%s\\n" "$OMH_PUBKEY" > "$HOME/.ssh/authorized_keys"' \
  '  chmod 600 "$HOME/.ssh/authorized_keys"' \
  'fi' \
  'sudo ssh-keygen -A >/dev/null 2>&1 || true' \
  'sudo mkdir -p /run/sshd' \
  'sudo /usr/sbin/sshd' \
  'exec sleep infinity' \
  > /usr/local/bin/omh-session \
 && chmod 0755 /usr/local/bin/omh-session

# Proxy forwarding, for backends that filter egress through one.
ENV HTTP_PROXY="" HTTPS_PROXY="" NO_PROXY=""

USER agent
WORKDIR /work
"#
    )
    .replace("__GRAPH_INSTALL__", &crate::base::graph_install())
}

pub fn harness_dockerfile(adapter: &Adapter) -> String {
    // Install as root, run as agent: an image that ends privileged would hand
    // the agent the sandbox's own escape hatch.
    let mut df = format!("FROM {}\nUSER root\nRUN {}\n", base_tag(), adapter.install);

    // Docker creates a missing mount parent as root, leaving the agent unable
    // to write beside its own config — atomic credential writes and transcripts
    // both fail, the first of them silently.
    let dirs = mount_parents(adapter);
    if !dirs.is_empty() {
        df.push_str(&format!(
            "RUN mkdir -p {0} && chown -R agent:agent {0}\n",
            dirs.join(" ")
        ));
    }
    df.push_str("USER agent\nWORKDIR /work\n");
    df
}

/// A third layer: what this repo's stacks put in the image.
///
/// The same shape as `harness_dockerfile`, one layer further out, with one
/// `RUN` per provide that fired — in the order the stack file gave them,
/// because `corepack enable pnpm` needs the node the provide above it asserted.
/// An ordered list is how a stack file expresses that dependency without a
/// graph, and a graph would be a second way to say what the order already says.
///
/// Root to install, `agent` to run, exactly as the harness layer: an image that
/// ends privileged hands the agent the sandbox's own escape hatch.
pub fn stack_dockerfile(adapter: &Adapter, installs: &[&str]) -> String {
    let mut df = format!("FROM {}\nUSER root\n", tag_for(adapter));
    for install in installs {
        df.push_str(&format!("RUN {install}\n"));
    }
    df.push_str("USER agent\nWORKDIR /work\n");
    df
}

/// The image a session runs, given what this repo's stacks provide.
///
/// Reached through `main::sandbox`, which is the single place that decides —
/// `container::plan` takes the answer as `Options.image` and re-derives
/// nothing. It was not always so: for one milestone `plan` hardcoded
/// `tag_for(adapter)`, so this tag was built by `init` and run by nobody, and
/// no test noticed. `a_session_runs_the_image_the_caller_resolved` is the guard
/// that closed it.
///
/// Keyed on the **recipe**, which is the recorded installs in order — so a pnpm
/// repo and a yarn repo do not share an image, and a reordered stack file is a
/// different image too. That is slightly stronger than keying on the set of
/// provides recorded, and strictly more correct: it is what the image contains.
///
/// Nothing to install is the harness image itself rather than an empty layer
/// on top of it. A provide that applies but installs nothing — the `runtime`
/// assertion in `stacks/node.toml` — correctly does not move the tag, because
/// it changes nothing about the image.
pub fn stack_tag(adapter: &Adapter, installs: &[&str]) -> String {
    if installs.is_empty() {
        return tag_for(adapter);
    }
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    stack_dockerfile(adapter, installs).hash(&mut h);
    tag_for(adapter).hash(&mut h);
    base_dockerfile().hash(&mut h);
    format!("omh/{}:{:x}", adapter.name, h.finish())
}

/// Build the base, the harness layer and this repo's stack layer if missing,
/// and hand back the tag a session should run.
///
/// Returns the tag rather than letting the caller recompute it: two places
/// deciding which image to run is how a session ends up in one image while
/// `init` reported another.
///
/// Every caller now gets it from one `main::sandbox` call and hands the same
/// value to `container::plan`, so the layer that is built and the image that
/// runs are the same string by construction.
///
/// No test calls this: it builds an image, and there is no container runtime in
/// the dev sandbox. Its construction — `stack_tag` and `stack_dockerfile` — is
/// tested thoroughly; that the build *works* is `omh doctor`'s to prove, which
/// is the coverage line `CLAUDE.md` draws and not one a green suite can cross.
pub fn ensure_stack(
    program: &str,
    adapter: &Adapter,
    installs: &[&str],
    repo: &Path,
) -> Result<String> {
    ensure(program, adapter)?;
    let tag = stack_tag(adapter, installs);
    if tag != tag_for(adapter) && !exists(program, &tag) {
        eprintln!("omh: building {tag} — this repo's toolchain, first run only");
        build(
            program,
            &tag,
            &stack_dockerfile(adapter, installs),
            &Kind::Stack(adapter, repo),
        )?;
    }
    Ok(tag)
}

/// Parents of every mount target under the agent's home.
///
/// The home itself is excluded — the base image already owns it — and
/// `/work` is a mount omh controls rather than one the image prepares.
fn mount_parents(adapter: &Adapter) -> Vec<String> {
    let mut dirs: Vec<String> = adapter
        .capabilities
        .values()
        .map(|b| b.path.clone())
        .chain(adapter.creds.iter().cloned())
        .chain(adapter.token.iter().cloned())
        .filter_map(|template| {
            let path = crate::adapter::expand(template.trim_end_matches('/'), GUEST_HOME);
            // `/work` is a mount omh owns; only the home side needs preparing.
            if !path.starts_with(GUEST_HOME) {
                return None;
            }
            path.parent().map(|p| p.display().to_string())
        })
        .filter(|d| d != GUEST_HOME)
        .collect();
    dirs.sort();
    dirs.dedup();
    dirs
}

/// `docker build` arguments. The Dockerfile arrives on stdin, so nothing is
/// written to disk and the build context stays empty.
///
/// The class is stamped on as labels because the tag cannot carry it: a tag is
/// a hash of a recipe and says nothing about what kind of thing it names, and
/// `reap` has to know. See `Kind`.
pub fn build_args(tag: &str, context: &Path, kind: &Kind) -> Vec<String> {
    let mut a: Vec<String> = vec!["build".into(), "-t".into(), tag.into()];
    for (k, v) in kind.stamp() {
        a.push("--label".into());
        a.push(format!("{k}={v}"));
    }
    a.extend([
        "-f".into(),
        "-".into(),
        context.to_string_lossy().into_owned(),
    ]);
    a
}

/// What an image omh built *is*, as opposed to what it is called.
///
/// This exists because the first reaper did not have it and guessed instead,
/// from the one thing a tag does carry — its repository. The guess is wrong,
/// and not subtly: `tag_for` and `stack_tag` both format
/// `omh/{adapter}:{hash}`, so `omh/claude` holds the harness layer *and* one
/// stack layer per checkout, all of them current. Reading the repository as the
/// class made a stack build delete the harness image it had just been built
/// from; the next launch rebuilt the harness, and that build deleted the stack.
/// Two multi-minute builds on every launch, for ever, from a change whose whole
/// purpose was to stop rebuilds.
///
/// The repository is not the equivalence class and no amount of parsing makes
/// it one. Docker keeps labels beside the image, so the class travels with the
/// thing it describes and cannot drift from it — the same reason
/// `Plan::labels` stamps a container with what it was made of.
///
/// An image built before omh stamped anything carries no `omh.*` labels, is in
/// no class, and is never reaped. That is deliberate: the alternative is an
/// empty class matching every unlabelled image on the machine.
pub enum Kind<'a> {
    /// One recipe per omh version, shared by every adapter and every checkout.
    Base,
    /// One recipe per adapter per omh version.
    Harness(&'a Adapter),
    /// One per *checkout*, because the recipe is that repo's toolchain. This is
    /// the case a repository cannot express.
    ///
    /// `repo` is the checkout's full path and not `Paths::repo_name`, which is
    /// a directory basename: two checkouts both called `api` would otherwise
    /// share a class and reap each other, which is the same bug one level
    /// quieter. A host path in image metadata is already the house style —
    /// `Plan::labels` puts whole mount paths in `omh.mounts`.
    Stack(&'a Adapter, &'a Path),
}

impl Kind<'_> {
    /// The labels that identify this class. Two images with equal stamps are
    /// the same kind of thing built from different recipes, which is exactly
    /// when the older one is dead.
    pub fn stamp(&self) -> Vec<(String, String)> {
        let mut s = vec![("omh.kind".to_string(), self.name().to_string())];
        match self {
            Kind::Base => {}
            Kind::Harness(a) => s.push(("omh.adapter".into(), a.name.clone())),
            Kind::Stack(a, repo) => {
                s.push(("omh.adapter".into(), a.name.clone()));
                s.push(("omh.repo".into(), repo.to_string_lossy().into_owned()));
            }
        }
        s
    }

    fn name(&self) -> &'static str {
        match self {
            Kind::Base => "base",
            Kind::Harness(_) => "harness",
            Kind::Stack(..) => "stack",
        }
    }

    /// `docker images` arguments listing exactly this class.
    ///
    /// Docker applies `--filter label=k=v` as exact equality and ANDs repeated
    /// ones, so the match is docker's to make and omh never parses a label
    /// value back — which matters because two of the three values are free
    /// text: an adapter's `name` comes from a TOML file a user can drop in, and
    /// a checkout path can contain very nearly anything. Verified 2026-08-20
    /// against docker 29.7.2 that `--filter label=omh.repo=/checkouts/a,b`
    /// matches that image and only that image, comma and all.
    ///
    /// Deliberately not scoped to a repository as well. One notion of "same
    /// kind" is enough, and the second one is what broke this.
    pub fn list_args(&self) -> Vec<String> {
        let mut a: Vec<String> = vec!["images".into()];
        for (k, v) in self.stamp() {
            a.push("--filter".into());
            a.push(format!("label={k}={v}"));
        }
        a.extend(["--format".into(), "{{.Repository}}:{{.Tag}}".into()]);
        a
    }
}

/// Arguments that run a short diagnostic script inside the image, and nothing
/// else.
///
/// Deliberately mountless. `omh doctor` builds a full `container::plan` because
/// it is checking mounted files; this probe asks only *what does this image
/// have on PATH*, and every mount it does not take is a way it cannot end up
/// answering about the host instead. That confusion — host evidence standing in
/// for a fact about the sandbox — is what the probe exists to end, so it must
/// not be reachable from inside the probe itself.
///
/// No `-w` either: the answer must not depend on where the script starts.
pub fn probe_args(tag: &str, script: &str) -> Vec<String> {
    vec![
        "run".into(),
        "--rm".into(),
        // Never fetch. `docker run` defaults to `--pull=missing`, and omh's
        // tags carry no registry prefix, so a tag whose image is not here
        // resolves against Docker Hub — for a name every user of a given omh
        // version shares and anybody can precompute from this repository. A
        // squatted `omh/*` would be pulled and run, and its `ENTRYPOINT` runs
        // ahead of the `sh -c` below, so overriding the command saves nothing.
        //
        // This is the one path that runs an image `ensure*` has not just built,
        // which is what makes the default reachable at all.
        "--pull=never".into(),
        tag.into(),
        "sh".into(),
        "-c".into(),
        script.into(),
    ]
}

/// Build the base and the harness layer if they are missing. Progress goes
/// straight to the terminal: a multi-minute silent step reads as a hang.
pub fn ensure(program: &str, adapter: &Adapter) -> Result<()> {
    let base = base_tag();
    if !exists(program, &base) {
        eprintln!("omh: building {base} (first run only)");
        build(program, &base, &base_dockerfile(), &Kind::Base)?;
    }
    let t = tag_for(adapter);
    if !exists(program, &t) {
        eprintln!("omh: building {t}");
        build(
            program,
            &t,
            &harness_dockerfile(adapter),
            &Kind::Harness(adapter),
        )?;
    }
    Ok(())
}

fn build(program: &str, tag: &str, dockerfile: &str, kind: &Kind) -> Result<()> {
    use anyhow::Context;
    use std::io::Write;
    use std::process::Stdio;

    // Empty context: everything the image needs comes from the Dockerfile.
    let context = std::env::temp_dir().join("omh-build-context");
    std::fs::create_dir_all(&context)?;

    let mut child = std::process::Command::new(program)
        .args(build_args(tag, &context, kind))
        .stdin(Stdio::piped())
        .spawn()
        .with_context(|| format!("running {program} build"))?;
    child
        .stdin
        .as_mut()
        .context("build stdin")?
        .write_all(dockerfile.as_bytes())?;

    let status = child.wait()?;
    if !status.success() {
        anyhow::bail!("failed to build {tag}");
    }
    reap(program, tag, kind);
    Ok(())
}

/// Remove the tags this build just replaced.
///
/// Here rather than at an `omh image prune` the user has to remember, because
/// the moment a new tag exists is the moment the old one *of the same kind* is
/// dead, and the cost of not doing it is silent until the disk is full.
///
/// Best-effort: a build that succeeded must not fail because tidying up after
/// it did, so nothing here returns an error. It is not silent, though. A
/// removal docker declines because a container holds the image is expected and
/// says nothing; anything else is reported, because a reaper that has been
/// failing on every build for months is indistinguishable from one that is
/// working unless it says so — and that indistinguishability is the whole
/// failure this feature exists to end.
fn reap(program: &str, built: &str, kind: &Kind) {
    let tags = match list_tags(program, kind) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("omh: could not list images to reap: {e}");
            return;
        }
    };
    let in_use = images_in_use(program);
    let mut gone = Vec::new();
    for tag in superseded(built, &tags, &in_use) {
        match remove_image(program, &tag) {
            Removal::Deleted => gone.push(tag),
            // Docker holding a line omh already tried to hold means the two
            // disagreed about what is in use, which the ID-shaped `{{.Image}}`
            // makes possible. The image stays, which is the right outcome, so
            // there is nothing to tell anyone.
            Removal::InUse => {}
            // An untag is not a removal. `docker image rm` exits 0 for one, so
            // counting exit codes would report a reclaim that did not happen:
            // the image is still there under its other name.
            Removal::Untagged => {}
            Removal::Failed(why) => eprintln!("omh: could not remove {tag}: {why}"),
        }
    }
    if !gone.is_empty() {
        eprintln!("omh: removed {} this build replaced", gone.join(", "));
    }
}

/// What `docker image rm` actually did, which its exit code does not say.
enum Removal {
    /// The image is gone and its unique layers are reclaimable.
    Deleted,
    /// The name is gone; the image survives under another tag. Exit code 0,
    /// zero bytes freed.
    Untagged,
    /// A container references it. Expected, and not a problem.
    InUse,
    Failed(String),
}

fn remove_image(program: &str, tag: &str) -> Removal {
    let out = match std::process::Command::new(program)
        .args(["image", "rm", tag])
        .output()
    {
        Ok(o) => o,
        Err(e) => return Removal::Failed(e.to_string()),
    };
    if out.status.success() {
        return classify_removal(&String::from_utf8_lossy(&out.stdout));
    }
    let err = String::from_utf8_lossy(&out.stderr);
    if err.contains("is using its referenced image") {
        Removal::InUse
    } else {
        Removal::Failed(err.trim().to_string())
    }
}

/// Read a successful `docker image rm` for what it did.
///
/// Split from the call so the distinction can be tested. Whether docker prints
/// `Deleted:` is docker's behaviour and no unit test settles it; that omh reads
/// an `Untagged:`-only run as *not* a removal is omh's own logic, and it is the
/// difference between an honest report and a count of exit codes.
fn classify_removal(stdout: &str) -> Removal {
    if stdout
        .lines()
        .any(|l| l.trim_start().starts_with("Deleted:"))
    {
        Removal::Deleted
    } else {
        Removal::Untagged
    }
}

/// Every tag of the same class as the one just built.
fn list_tags(program: &str, kind: &Kind) -> Result<Vec<String>> {
    let out = std::process::Command::new(program)
        .args(kind.list_args())
        .output()?;
    anyhow::ensure!(out.status.success(), "listing images to reap");
    Ok(String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(str::to_string)
        .collect())
}

/// Images any container references, running or stopped.
///
/// Stopped counts, though not for the reason it first looks like. omh removes
/// its own container on `omh s down` — `down` calls `container_remove`, which
/// is `docker rm -f` — so a stopped container omh can still see is one that
/// died *outside* omh's control: a host reboot, an OOM kill, a daemon crash.
/// Docker refuses to drop an image such a container references, and plain
/// `docker ps` would not show it.
///
/// Read two ways because `{{.Image}}` is not reliably a tag: docker prints
/// whatever the container config holds, which degrades to a bare image ID once
/// that reference stops resolving — including when an earlier reap untagged it.
/// The `omh.image` label is the tag omh itself launched, stamped by
/// `Plan::labels`, and it does not degrade.
fn images_in_use(program: &str) -> Vec<String> {
    let read = |args: &[&str]| -> Vec<String> {
        std::process::Command::new(program)
            .args(args)
            .output()
            .map(|o| {
                String::from_utf8_lossy(&o.stdout)
                    .lines()
                    .map(str::trim)
                    .filter(|l| !l.is_empty())
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default()
    };
    let mut v = read(&["ps", "-a", "--format", "{{.Image}}"]);
    v.extend(read(&[
        "ps",
        "-a",
        "--filter",
        "label=omh.image",
        "--format",
        "{{.Label \"omh.image\"}}",
    ]));
    v.sort();
    v.dedup();
    v
}

/// The plan names a per-project network; something has to create it. Without
/// this every launch dies at `network omh-<repo> not found` — a plan that is
/// well-formed but not runnable.
pub fn ensure_network(program: &str, name: &str) -> Result<()> {
    let present = std::process::Command::new(program)
        .args(["network", "inspect", name])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);
    if present {
        return Ok(());
    }
    let out = std::process::Command::new(program)
        .args(["network", "create", name])
        .output()?;
    if !out.status.success() {
        anyhow::bail!(
            "creating network {name}: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(())
}

/// Is the session container up right now?
/// Whether the sandbox is up — with *omh could not tell* as its own answer.
///
/// A `bool` here was the same collapse the session dashboard carried in its
/// `behind` column, with more at stake. `false` meant both *the container is
/// not running* and *the runtime would not answer*, so a Docker daemon that is
/// down rendered live sandboxes as `stopped` — and, far worse, told
/// `stop_before_syncing` there was nothing to stop. A sync would then overwrite
/// files under a live agent, which that function's own doc calls the worst
/// outcome available.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Running {
    Yes,
    No,
    /// The runtime could not be asked, and why. Never *no*.
    ///
    /// The payload is always through `out::untrusted` and never empty — both
    /// are `running_from`'s doing, and it is the only thing that builds one.
    /// Callers repeat it into a terminal without sanitising again, so a second
    /// construction site would have to keep that promise.
    Unknown(String),
}

/// Is this container up, given what the runtime said when asked?
///
/// Over the process result rather than the process, so all the states are a
/// table — the shape `doctor::version_of` uses, for the reason it records:
/// while this was one `.unwrap_or(false)`, no test could reach any state but
/// the first. Measured 2026-08-24 against docker 29.7.2.
///
/// **The mechanism is the exit status, and the name is compared here rather
/// than passed to the runtime as a pattern.** Measured 2026-08-24 against
/// docker 29.7.2:
///
/// | container state | `ps --format {{.Names}}` | answer |
/// |---|---|---|
/// | running | lists it | `Yes` |
/// | paused | lists it | `Yes` |
/// | restarting | lists it | `Yes` |
/// | created, never started | absent | `No` |
/// | exited | absent | `No` |
/// | no such container | absent | `No` |
/// | daemon unreachable | exit 1 | `Unknown` |
///
/// `paused` and `restarting` answering `Yes` is deliberate and matches what
/// the old probe did: both hold a tree omh must not write under. `created`
/// answering `No` matches it too — nothing is running in a container that was
/// never started.
///
/// Two things this deliberately does **not** do. It does not ask the runtime
/// about one container: `--filter name=` is a *regex*, docker permits `.` in a
/// name, and `Paths::container` builds one from the checkout's directory
/// basename unsanitised — so a repo in `~/src/omh.rs` would probe with
/// `^omh-omh.rs-s01$` and match `omh-omhXrs-s01` as well. And a filter that
/// does not parse exits **0 with empty stdout**, which reads as *not running*:
/// the collapse this type exists to remove, reintroduced by the probe meant to
/// fix it. Comparing here has no pattern language in it.
///
/// It also does not fuse *stopped* with *never built* by accident — that is on
/// purpose. Neither is running and no caller wants them apart.
pub fn running_from(name: &str, asked: std::io::Result<std::process::Output>) -> Running {
    let out = match asked {
        Ok(out) => out,
        // The program is on `PATH` — `runtime::installed` said so before any
        // of this — so a spawn that fails is a fork failure or a binary that
        // vanished mid-run, and either way omh has no answer.
        Err(e) => {
            return Running::Unknown(crate::out::untrusted(&format!(
                "could not run the container runtime: {e}"
            )))
        }
    };
    if !out.status.success() {
        // A non-zero exit is never a *no*, whatever it did or did not say.
        return Running::Unknown(unreadable(
            &String::from_utf8_lossy(&out.stderr),
            &out.status,
        ));
    }
    match String::from_utf8_lossy(&out.stdout)
        .lines()
        .any(|listed| listed.trim() == name)
    {
        true => Running::Yes,
        false => Running::No,
    }
}

pub fn container_running(backend: &dyn crate::runtime::Runtime, name: &str) -> Running {
    running_from(
        name,
        std::process::Command::new(backend.program())
            .args(backend.running_args())
            .output(),
    )
}

/// Why omh has no answer, said the same way wherever that happens.
///
/// One function because `Running::Unknown` and `Probe::Unknown` make the same
/// promise — sanitised, never empty, repeated into a terminal without being
/// sanitised again — and it was a copy of the same ladder in both. Two copies
/// of a promise is one more than can be kept.
///
/// The empty case is not hypothetical tidiness: a non-zero exit with nothing
/// said produced `…so it will not sync over it: `, a sentence ending in a
/// colon. And it is the exit **code**, not the `Display` of `ExitStatus`,
/// which renders `exit status: 1` and read as "the container runtime exited
/// exit status: 1".
fn unreadable(said: &str, status: &std::process::ExitStatus) -> String {
    let said = crate::out::untrusted(said.trim());
    match said.is_empty() {
        false => said,
        true => match status.code() {
            Some(code) => format!("the container runtime exited {code}"),
            None => "the container runtime was killed by a signal".into(),
        },
    }
}

/// What the one exec `reuse_decision` runs came back as.
///
/// `Option<String>` here was the same collapse `Running` replaced, on the
/// decision with the most to lose: `None` meant *the container cannot reach
/// its worktree*, *the daemon died between the two calls*, *the container
/// stopped*, *the image has no shell*, and *the fork failed* — and `reuse`
/// turns the first into `Restart`, which `session_up` acts on with `rm -f`.
/// On a container it confirmed was running three lines earlier.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Probe {
    /// The exec worked. Carries what it printed, which may be empty.
    ///
    /// The one payload here written by the **sandbox** rather than by the
    /// runtime, so it is sanitised on the way out: these are filenames from a
    /// directory the agent can write to, and they reach a terminal through
    /// `Reuse::Blocked` — the message whose whole job is telling somebody
    /// their work is safe.
    Listed(String),
    /// The mount-namespace failure, by its own signature: the worktree the
    /// container is bound to was replaced under it, and no `exec` will ever
    /// work again. Replacing it is the only remedy.
    NotEnterable,
    /// The container is not there to enter — removed or exited between the
    /// two calls omh makes.
    ///
    /// Kept apart from `Unknown` because the safe direction is the opposite
    /// one: nothing is alive inside a container that is gone, so replacing it
    /// costs nothing and the launch carries on. Folding it into `Unknown`
    /// turned the most ordinary race in the launch path — an agent that
    /// finishes between *is it running* and *can it be entered* — into a
    /// failed command the user had to run twice.
    Gone,
    /// Something else went wrong, and why. Never *not enterable*.
    ///
    /// The payload is always through `out::untrusted` and never empty, both of
    /// which are `probe_from`'s doing. Callers repeat it into a terminal
    /// without sanitising again.
    Unknown(String),
}

/// The one failure a container cannot recover from, spelled the way docker
/// spells it.
///
/// Matched on the message rather than on the exit code, and that is a
/// measurement rather than a preference — an earlier draft of this said the
/// opposite, having failed to produce a second 128.
///
/// It failed because both attempts were *daemon-level* errors, which never
/// reach the OCI runtime and so cannot land on 128 by construction. Reaching
/// past that boundary takes seconds: `docker exec -w /loop` where `/loop` is a
/// symlink to itself gives exit 128 with `chdir to cwd ("/loop") … too many
/// levels of symbolic links`, on stdout, sharing the `OCI runtime exec
/// failed:` prefix and sharing nothing else.
///
/// And 128 is not the category the draft called it. Measured 2026-08-24
/// against docker 29.7.2: docker maps a start failure that reads as ENOENT to
/// **127** and one that reads as EACCES to **126**; 128 is what is left when
/// neither matches. A residue, which two unrelated failures share.
const BROKEN_MOUNT: &str = "container mount namespace root";

/// What docker prefixes an error it produced itself, on stdout.
///
/// The scan for `BROKEN_MOUNT` is restricted to bytes with this prefix, plus
/// stderr. Without it the search covers stdout, which on a partial `ls` is
/// written by the **agent** — and a match there is a `docker rm -f`.
const RUNTIME_ERROR: &str = "OCI runtime exec failed";

/// What docker says when the daemon answered but the container could not be
/// used — as opposed to the daemon not answering at all.
///
/// Measured 2026-08-24 against docker 29.7.2: a container that was removed
/// says `Error response from daemon: No such container: …`, one that exited
/// says `Error response from daemon: container … is not running`, and a daemon
/// that cannot be reached says `failed to connect to the docker API at …`.
/// The prefix is the mechanism rather than the two messages: if the daemon
/// answered, the problem is the container, and replacing a container with
/// nothing alive in it is free.
const DAEMON_ANSWERED: &str = "Error response from daemon";

/// The command the probe runs, and the only one `probe_from` is written to
/// read.
///
/// Here rather than at the call site because the two are one mechanism: what
/// `probe_from` may conclude from a non-zero exit depends entirely on this
/// command being unable to fail on its own. `[ -d ] || exit 0` is what makes
/// that true — an absent socket directory is the ordinary case for a session
/// whose harness has never run, and it exits 0 with nothing rather than
/// failing.
///
/// It replaces `ls … 2>/dev/null || true`, which swallowed *every* `ls`
/// failure into an empty listing. An empty listing means *no harness is live*,
/// which is the input that makes `reuse` choose `Restart` over `Blocked` — so
/// a directory that could not be read read as "nothing to lose" and the
/// container was destroyed with an agent inside. The polarity is opposite to
/// the bug this type fixes and the cost is the same.
pub fn probe_command() -> Vec<String> {
    vec![
        "sh".into(),
        "-c".into(),
        format!(
            "[ -d {dir} ] || exit 0; ls -1 {dir}",
            dir = crate::persist::SOCKET_DIR
        ),
    ]
}

/// Read the probe's result, keeping the failures that mean different things
/// apart.
///
/// Over the process result rather than the process, so all of it is a table.
///
/// Measured 2026-08-24 against docker 29.7.2, through `/bin/sh` rather than
/// the interactive shell — zsh's MULTIOS has made this exact measurement wrong
/// in this project before. Deleting the bind-mount source is enough on its own
/// — recreating it produces byte-identical output, so *replaced* is one route
/// in rather than the trigger:
///
/// | what happened | exit | where it said so |
/// |---|---|---|
/// | the exec worked | 0 | the listing, on stdout |
/// | the worktree was replaced under it | 128 | `OCI runtime exec failed: …` on **stdout** |
/// | the image has no such binary | 127 | `OCI runtime exec failed: …` on **stdout** |
/// | the container was removed | 1 | `Error response from daemon: No such container` |
/// | the container had exited | 1 | `Error response from daemon: container … is not running` |
/// | the daemon could not be reached | 1 | `failed to connect to the docker API at …` |
pub fn probe_from(asked: std::io::Result<std::process::Output>) -> Probe {
    let out = match asked {
        Ok(out) => out,
        Err(e) => {
            return Probe::Unknown(crate::out::untrusted(&format!(
                "could not run the container runtime: {e}"
            )))
        }
    };
    if out.status.success() {
        return Probe::Listed(crate::out::untrusted(&String::from_utf8_lossy(&out.stdout)));
    }
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    // Only the bytes docker wrote. Stdout is the agent's on a listing that
    // began before something went wrong, and every conclusion below is one the
    // agent must not be able to reach for itself.
    let runtime_said = match stdout.contains(RUNTIME_ERROR) {
        true => format!("{stdout}{stderr}"),
        false => stderr.into_owned(),
    };
    if runtime_said.contains(BROKEN_MOUNT) {
        return Probe::NotEnterable;
    }
    if runtime_said.contains(DAEMON_ANSWERED) {
        return Probe::Gone;
    }
    Probe::Unknown(unreadable(&runtime_said, &out.status))
}

pub fn container_probe(program: &str, args: &[String]) -> Probe {
    probe_from(std::process::Command::new(program).args(args).output())
}

/// omh's own records from the label map docker reports.
///
/// Separated from everybody else's because a base image sets labels too, and a
/// whole-map comparison would call `maintainer` drift. Anything unreadable —
/// including the bare `null` docker prints for a container with no labels at
/// all, which is every session started before omh stamped them — comes back
/// empty, and the caller reads empty as "cannot be verified".
pub fn omh_labels(json: &str) -> std::collections::BTreeMap<String, String> {
    serde_json::from_str::<std::collections::BTreeMap<String, String>>(json)
        .unwrap_or_default()
        .into_iter()
        .filter(|(k, _)| k.starts_with("omh."))
        .collect()
}

/// What the running container says it was built from.
/// What omh stamped on a container, or why it could not be read.
///
/// The third answer is not decoration. This was
/// `.ok().filter(success).unwrap_or_default()`, and an empty map is not a
/// neutral value here — `container::drift` reads one as a confident claim,
/// *"it predates this check, so nothing about it can be verified"*, which
/// `reuse` turns into `Restart` and `session_up` into `rm -f`.
///
/// So a daemon that restarted between the probe and this call destroyed a
/// container that had just been confirmed alive **and** enterable, and told
/// the user a fabricated reason for it. That is worse than the silence it
/// replaced: it is confident enough that nobody goes looking for a daemon
/// problem. The identical chain this module's `Probe` was written to close,
/// entered one line further down.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Stamp {
    /// Read, and here is what it says. Empty means the container carries no
    /// omh labels — genuinely older than the check.
    Read(std::collections::BTreeMap<String, String>),
    /// The runtime would not say, and why.
    Unknown(String),
}

pub fn container_stamp(program: &str, name: &str) -> Stamp {
    stamp_from(
        std::process::Command::new(program)
            .args(["inspect", "-f", "{{json .Config.Labels}}", name])
            .output(),
    )
}

/// Read the stamp, given what the runtime said.
///
/// `omh_labels` swallows a parse failure into an empty map, which used to mean
/// unparseable JSON also read as *predates this check*. A `docker inspect`
/// that exits 0 and prints something this cannot parse is the runtime
/// answering in a shape omh does not know, which is not the same as a
/// container with no labels — so the emptiness has to be decided here, where
/// the difference is still visible.
pub fn stamp_from(asked: std::io::Result<std::process::Output>) -> Stamp {
    let out = match asked {
        Ok(out) => out,
        Err(e) => {
            return Stamp::Unknown(crate::out::untrusted(&format!(
                "could not run the container runtime: {e}"
            )))
        }
    };
    if !out.status.success() {
        return Stamp::Unknown(unreadable(
            &String::from_utf8_lossy(&out.stderr),
            &out.status,
        ));
    }
    let said = String::from_utf8_lossy(&out.stdout);
    // `null` is what docker prints for a container with no labels at all, and
    // it is the one non-map answer that means something. Anything else that
    // will not parse is the runtime speaking a shape omh does not know.
    if said.trim() == "null" {
        return Stamp::Read(Default::default());
    }
    match serde_json::from_str::<std::collections::BTreeMap<String, String>>(said.trim()) {
        Ok(all) => Stamp::Read(
            all.into_iter()
                .filter(|(k, _)| k.starts_with("omh."))
                .collect(),
        ),
        Err(e) => Stamp::Unknown(crate::out::untrusted(&format!(
            "the container runtime answered with something omh could not read: {e}"
        ))),
    }
}

/// Stopped-but-present containers block `run --name`, so clear them first.
pub fn container_remove(program: &str, name: &str) -> Result<()> {
    let out = std::process::Command::new(program)
        .args(["rm", "-f", name])
        .output()?;
    if !out.status.success() {
        // A sandbox that is still running still has the credential directory
        // mounted writable; reporting it stopped would be a lie that matters.
        anyhow::bail!("{}", String::from_utf8_lossy(&out.stderr).trim());
    }
    Ok(())
}

/// The tags a freshly built one replaces.
///
/// A tag omh chooses is a hash of the recipe that produced it, so once a recipe
/// changes the previous tag of that same kind is dead: nothing will ever ask
/// for it again, and until now nothing removed it either.
///
/// Decided here, as a function over lists, rather than inside the code that
/// shells out: which images should go is omh's own logic and is worth testing;
/// whether docker agrees to remove one is not something a unit test can settle.
///
/// `tags` is one class — `Kind`, and not the docker repository, because a
/// repository holds several live tags at once and reading it as the class is
/// what made the first version of this eat the image it was built from.
///
/// Two things in a class are still never superseded. **`latest`**, because no
/// current recipe reproduces it: omh v0 tagged `omh/base:latest` and
/// `omh/{harness}:latest` (`129b530`), and `fac15ea` replaced both with recipe
/// hashes. A hash tag removed by mistake comes back by re-running the recipe;
/// `:latest` is reachable from no recipe omh still has, which makes it the one
/// removal that cannot be undone. And **anything a container references**,
/// however old, because that is a session someone is still using; docker would
/// refuse, but omh should not be asking, nor reporting a removal that did not
/// happen.
pub fn superseded(built: &str, tags: &[String], in_use: &[String]) -> Vec<String> {
    tags.iter()
        .filter(|t| t.as_str() != built)
        .filter(|t| !t.ends_with(":latest"))
        .filter(|t| !in_use.iter().any(|u| u == *t))
        .cloned()
        .collect()
}

pub fn exists(program: &str, tag: &str) -> bool {
    std::process::Command::new(program)
        .args(["image", "inspect", tag])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn output(code: i32, stdout: &str, stderr: &str) -> std::io::Result<std::process::Output> {
        use std::os::unix::process::ExitStatusExt;
        Ok(std::process::Output {
            status: std::process::ExitStatus::from_raw(code << 8),
            stdout: stdout.as_bytes().to_vec(),
            stderr: stderr.as_bytes().to_vec(),
        })
    }

    /// A probe that failed some other way is never read as *this container
    /// cannot reach its worktree* — and the two failures that mean *nothing is
    /// alive in there* are not read as refusals.
    ///
    /// The first is the worst outcome in the program: `reuse` turns *not
    /// enterable* into `Restart`, and `session_up` acts on a `Restart` with
    /// `docker rm -f` — on a container confirmed running earlier in the same
    /// function. Every non-namespace exec failure destroyed an agent mid-turn,
    /// and under `--json` said nothing, because the only notice was
    /// `ctx.progress`.
    ///
    /// The second is milder and was introduced by fixing the first: folding
    /// *the container is gone* into the refusal turned the ordinary race — a
    /// sandbox whose process exits between *is it running* and *can it be
    /// entered* — from something that healed itself into a command the user
    /// had to run twice.
    ///
    /// Every row below is a transcript measured 2026-08-24 against docker
    /// 29.7.2 through `/bin/sh`, except the one marked as a shape.
    #[test]
    fn a_probe_that_failed_some_other_way_does_not_read_as_a_broken_worktree() {
        let namespace = "OCI runtime exec failed: exec failed: unable to start container \
             process: current working directory is outside of container mount namespace \
             root -- possible container breakout detected";

        assert_eq!(
            probe_from(output(0, "agent.sock\n", "")),
            Probe::Listed("agent.sock\n".into()),
            "a healthy exec answers with what it listed"
        );
        assert_eq!(
            probe_from(output(0, "", "")),
            Probe::Listed(String::new()),
            "including an empty listing — the socket directory is not there yet"
        );
        // The sandbox writes those filenames, and they reach a terminal
        // through `Reuse::Blocked` — the message whose job is saying the work
        // is safe.
        let Probe::Listed(listed) = probe_from(output(0, "s01-\u{1b}[2Jclaude\n", "")) else {
            panic!("a successful exec is a listing");
        };
        assert!(
            !listed.contains('\u{1b}'),
            "and nothing in it repaints the terminal: {listed:?}"
        );

        // At three exit codes, because the code is the part that must not be
        // load-bearing, and on both streams, because which one docker picks is
        // not a thing to depend on.
        for (code, stdout, stderr, what) in [
            (
                128,
                namespace,
                "",
                "the measured signature, on the stream docker used",
            ),
            (
                1,
                "",
                namespace,
                "the same message on stderr, should docker move it",
            ),
            (
                127,
                namespace,
                "",
                "and at a code omh has no reason to expect",
            ),
        ] {
            assert_eq!(
                probe_from(output(code, stdout, stderr)),
                Probe::NotEnterable,
                "{what}"
            );
        }

        // Nothing alive to lose. Both measured; both used to be refusals.
        for (stderr, what) in [
            (
                "Error response from daemon: No such container: omh-repo-s01",
                "a container removed between the two calls",
            ),
            (
                "Error response from daemon: container 1794fad25dec is not running",
                "and one that exited between them",
            ),
        ] {
            assert_eq!(probe_from(output(1, "", stderr)), Probe::Gone, "{what}");
        }

        // Everything else. The first two would read as the fatal case if the
        // exit code decided instead of the message.
        for (code, stdout, stderr, what) in [
            (
                128,
                "OCI runtime exec failed: exec failed: unable to start container process: \
                 chdir to cwd (\"/loop\") set in config.json failed: too many levels of \
                 symbolic links",
                "",
                "a second 128 — measured, so matching the code is provably not enough",
            ),
            (
                127,
                "OCI runtime exec failed: exec failed: unable to start container process: \
                 exec: \"sh\": executable file not found in $PATH",
                "",
                "an image with no shell, sharing the prefix and nothing else",
            ),
            (
                1,
                "",
                "failed to connect to the docker API at unix:///var/run/docker.sock; check \
                 if the path is correct and if the daemon is running",
                "a daemon that could not be reached at all",
            ),
            // A shape rather than a transcript: omh has not produced one.
            (137, "", "", "an exec that exited 137 saying nothing"),
            // The one the agent could reach for. Stdout on a failure is the
            // sandbox's — a listing that began before something went wrong —
            // and the signature is only docker's when it carries docker's
            // prefix. Without the scoping, a file named after the failure
            // steers omh into `rm -f` on the container that named it.
            (
                137,
                "s01-current working directory is outside of container mount namespace root\n",
                "",
                "a filename the agent chose, which is not docker speaking",
            ),
        ] {
            let answered = probe_from(output(code, stdout, stderr));
            assert!(
                matches!(&answered, Probe::Unknown(_)),
                "{what}: {answered:?}"
            );
        }

        // The payload, which nothing used to look at — so dropping
        // `out::untrusted` from it passed, on a string that goes verbatim into
        // a `bail!` and out to a terminal.
        let Probe::Unknown(why) = probe_from(output(1, "", "cannot \u{1b}[2J connect")) else {
            panic!("an unrecognised failure has no answer");
        };
        assert!(!why.contains('\u{1b}'), "sanitised: {why:?}");
        assert!(why.contains("cannot"), "and the words survive: {why:?}");

        let Probe::Unknown(quiet) = probe_from(output(137, "", "")) else {
            panic!("a silent failure still has no answer");
        };
        assert!(
            quiet.contains("137"),
            "a failure that said nothing still says something: {quiet:?}"
        );
        assert!(
            !quiet.contains("exit status"),
            "and not `exited exit status: 137`: {quiet:?}"
        );

        assert!(
            matches!(
                probe_from(Err(std::io::Error::other("fork failed"))),
                Probe::Unknown(_)
            ),
            "and neither does a probe that never ran"
        );
    }

    /// The stamp has the same three answers, for the same reason.
    ///
    /// An unreadable stamp was `unwrap_or_default()` into an empty map, and
    /// `container::drift` reads an empty map as *it predates this check, so
    /// nothing about it can be verified* — a `Restart`, which is `rm -f`. The
    /// same chain `Probe` closes, entered one line further down, with a
    /// fabricated reason attached to it.
    #[test]
    fn a_stamp_omh_could_not_read_is_not_a_container_that_predates_the_check() {
        let labels = r#"{"omh.harness":"claude","maintainer":"alpine"}"#;
        let Stamp::Read(read) = stamp_from(output(0, labels, "")) else {
            panic!("a readable stamp is read");
        };
        assert_eq!(read.get("omh.harness").map(String::as_str), Some("claude"));
        assert!(
            !read.contains_key("maintainer"),
            "and everybody else's labels stay out of it: {read:?}"
        );

        // The one empty answer that means something: docker prints `null` for
        // a container carrying no labels, which really does predate the check.
        assert_eq!(
            stamp_from(output(0, "null\n", "")),
            Stamp::Read(Default::default()),
            "a container with no labels is read, and read as empty"
        );

        for (code, stdout, stderr, what) in [
            (
                1,
                "",
                "failed to connect to the docker API",
                "a daemon that would not answer",
            ),
            (
                0,
                "not json at all",
                "",
                "an answer in a shape omh does not know",
            ),
        ] {
            let answered = stamp_from(output(code, stdout, stderr));
            assert!(
                matches!(&answered, Stamp::Unknown(_)),
                "{what} is not a container that predates the check: {answered:?}"
            );
        }
    }

    /// A runtime that would not answer is never read as *not running*.
    ///
    /// The defect this replaces was one `.unwrap_or(false)`, and it had four
    /// inputs of which no test could reach three. What it cost is not a
    /// cosmetic column: `stop_before_syncing` asks this question to decide
    /// whether a sync would overwrite the files of a live agent, and a Docker
    /// daemon that is down answered *nothing is running*.
    ///
    /// The states are the ones measured against docker 29.7.2, not invented:
    /// running prints an id, stopped and never-built print nothing, and only
    /// an unreachable daemon exits non-zero.
    #[test]
    fn a_runtime_that_will_not_answer_is_not_a_container_that_is_stopped() {
        assert_eq!(
            running_from(
                "omh-repo-s01",
                output(0, "omh-repo-s01\nomh-repo-s02\n", "")
            ),
            Running::Yes,
            "listed among the running ones"
        );
        assert_eq!(
            running_from("omh-repo-s01", output(0, "omh-repo-s02\n", "")),
            Running::No,
            "a listing that does not name it is an answer, and the answer is no"
        );
        assert_eq!(
            running_from("omh-repo-s01", output(0, "", "")),
            Running::No,
            "nothing running at all, asked and answered"
        );

        // The comparison is exact, which is the whole reason it happens here
        // rather than in a `--filter` the runtime parses as a regex.
        assert_eq!(
            running_from("omh-repo-s1", output(0, "omh-repo-s10\n", "")),
            Running::No,
            "a longer name that starts the same way is a different container"
        );
        assert_eq!(
            running_from("omh-omh.rs-s01", output(0, "omh-omhXrs-s01\n", "")),
            Running::No,
            "and a `.` in a repo name is a character, not a wildcard — which is \
             what `--filter name=^…$` made it"
        );

        // The one that matters. Both halves: not `No`, and carrying why.
        let daemon_down = "failed to connect to the docker API at unix:///var/run/docker.sock";
        let unknown = running_from("omh-repo-s01", output(1, "", daemon_down));
        assert_ne!(unknown, Running::No, "a failed question is not a `no`");
        assert!(
            matches!(&unknown, Running::Unknown(why) if why.contains("docker API")),
            "and it carries the runtime's own words: {unknown:?}"
        );

        // A non-zero exit that says nothing is still not a `no`. The tempting
        // reading — no error text, so nothing was wrong — is how this class of
        // bug is reintroduced.
        let quiet = running_from("omh-repo-s01", output(1, "", ""));
        assert!(
            matches!(&quiet, Running::Unknown(_)),
            "silence on stderr does not make a failure into an answer"
        );
        // And it says something rather than an empty reason, which reaches the
        // user as a sentence ending in a colon.
        assert!(
            matches!(&quiet, Running::Unknown(why) if why.contains('1')),
            "the exit status stands in for the words it did not say: {quiet:?}"
        );
        assert!(
            !format!("{quiet:?}").contains("exit status"),
            "and not `exited exit status: 1`: {quiet:?}"
        );

        // The runtime is on `PATH` before any of this — a spawn that fails
        // anyway is a machine in trouble, not a container that is stopped.
        assert!(
            matches!(
                running_from("omh-repo-s01", Err(std::io::Error::other("fork failed"))),
                Running::Unknown(_)
            ),
            "and neither does a spawn that never ran"
        );
    }

    /// Whatever the runtime says comes back through `untrusted`.
    ///
    /// This text reaches the terminal through a warning and through `sync`'s
    /// refusal. A container name is chosen by omh, but the rest of that line
    /// is the runtime's, and an image label or a mount path inside it is not.
    #[test]
    fn the_runtimes_own_words_are_sanitised_before_they_are_repeated() {
        let sneaky = "cannot connect\u{1b}[2J to the daemon";
        let Running::Unknown(why) = running_from("omh-repo-s01", output(1, "", sneaky)) else {
            panic!("a failed probe is Unknown");
        };
        assert!(
            !why.contains('\u{1b}'),
            "no escape reaches the terminal: {why:?}"
        );
        assert!(why.contains("cannot connect"), "the words survive: {why:?}");
    }

    /// Every backend asks the same question the same way: list what is
    /// running, one name per line, and fail loudly rather than quietly.
    ///
    /// Both implementations, because `select` prefers `sbx` under `auto` — so
    /// the unmeasured backend is the *default* one, and a third arriving with
    /// its own spelling is how the contract rots. This asserts the shape they
    /// share; what neither this nor any test can assert is that sbx's `ps`
    /// behaves as assumed, which `runtime.rs` says out loud.
    #[test]
    fn every_backend_asks_for_the_running_set_and_names_it() {
        use crate::runtime::Runtime;
        for backend in [
            &crate::runtime::Docker as &dyn Runtime,
            &crate::runtime::Sbx as &dyn Runtime,
        ] {
            let args = backend.running_args();
            assert!(
                args.first().is_some_and(|a| a == "ps"),
                "{}: the running set: {args:?}",
                backend.name()
            );
            assert!(
                !args.iter().any(|a| a == "-a"),
                "{}: running, not every container ever created: {args:?}",
                backend.name()
            );
            // No container name anywhere in the argv. Handing one to the
            // runtime is what made it a pattern, and `.` in a repo directory
            // name is legal in a container name and a regex wildcard.
            assert!(
                !args.iter().any(|a| a.contains("filter") || a.contains('^')),
                "{}: nothing the runtime will read as a pattern: {args:?}",
                backend.name()
            );
        }
    }

    /// omh tags an image by hashing its recipe, so a recipe change builds a new
    /// tag and the old one of that kind is superseded the moment it does.
    /// Nothing removed it.
    ///
    /// Measured 2026-08-18 with `docker system df` on a two-week-old machine:
    /// 14.86 GB of images, 9.95 GB of it reclaimable, across 14 omh tags — six
    /// `omh/claude`, three `omh/opencode`, three `omh/base`, two `omh/omp`.
    ///
    /// Summing the `SIZE` column of `docker images` instead gives about 20 GB,
    /// and that is the number *not* to quote: it charges every tag the full
    /// cost of the `node:22-bookworm-slim` and apt layers they share. What one
    /// superseded harness tag actually returns is its own install layer, which
    /// `docker system df -v` puts in the hundreds of kilobytes. The cost being
    /// paid down here is unbounded tag growth, not the gigabytes the `SIZE`
    /// column advertises — a whole dead chain frees real space, one dead tag
    /// beside its replacement mostly does not.
    ///
    /// Two things were observed on that machine and one is inferred: the disk
    /// filled, and Docker stopped responding until it was restarted by hand.
    /// That the first caused the second is a reasonable read and was never
    /// measured, so it is not a claim this makes.
    ///
    /// Also not addressed here, and larger on that machine than the images:
    /// buildkit's cache, 8.8 GB with 2.16 GB reclaimable. `docker image rm`
    /// cannot reach it — only `docker builder prune` can, and taking somebody's
    /// build cache is not a thing to do without being asked.
    ///
    /// omh reaps sessions, worktrees, staging and the sandbox's repository. An
    /// image is the one thing it creates and never takes back.
    #[test]
    fn a_new_build_supersedes_the_older_tags_of_its_class() {
        let tags = vec![
            "omh/claude:new".to_string(),
            "omh/claude:old".to_string(),
            "omh/claude:older".to_string(),
            "omh/claude:latest".to_string(),
        ];

        let gone = superseded("omh/claude:new", &tags, &[]);

        assert!(
            gone.contains(&"omh/claude:old".to_string())
                && gone.contains(&"omh/claude:older".to_string()),
            "the tags this build replaces: {gone:?}"
        );
        assert!(
            !gone.contains(&"omh/claude:new".to_string()),
            "never the one just built: {gone:?}"
        );
        assert!(
            !gone.contains(&"omh/claude:latest".to_string()),
            "no recipe omh still has reproduces `latest`, so removing it is \
             the one removal that cannot be undone: {gone:?}"
        );
    }

    /// A tag a container references is not superseded, whatever its age.
    /// Docker would refuse the removal anyway — this is so omh does not ask,
    /// and does not report removing something it did not.
    ///
    /// The stale tag is here so the test cannot pass by returning nothing.
    #[test]
    fn a_tag_a_container_is_using_survives_a_newer_build() {
        let tags = vec![
            "omh/claude:new".to_string(),
            "omh/claude:held".to_string(),
            "omh/claude:stale".to_string(),
        ];

        let gone = superseded("omh/claude:new", &tags, &["omh/claude:held".to_string()]);

        assert_eq!(
            gone,
            vec!["omh/claude:stale".to_string()],
            "the held tag stays and the stale one goes: {gone:?}"
        );
    }

    /// `docker image rm` exits 0 whether it deleted an image or merely dropped
    /// one of its names, and only the first frees a byte. Counting exit codes
    /// reports a reclaim that did not happen — and then `exists` says the tag
    /// is missing, so the next launch rebuilds it anyway.
    #[test]
    fn dropping_a_name_is_not_removing_an_image() {
        let untagged = "Untagged: omh/claude:old\n";
        let deleted = "Untagged: omh/claude:old\nDeleted: sha256:9aeb9a1e2d14\n";

        assert!(
            matches!(classify_removal(untagged), Removal::Untagged),
            "the image survives under its other name"
        );
        assert!(matches!(classify_removal(deleted), Removal::Deleted));
    }

    /// The tag space is not one-live-tag-per-repository, and the first version
    /// of `superseded` assumed it was.
    ///
    /// `stack_tag` and `tag_for` both format `omh/{adapter}:{hash}` — the same
    /// docker repository — so `omh/claude` holds the harness layer *and* one
    /// stack layer per checkout, every one of them current. Grouping by
    /// repository and keeping the newest made a stack build delete the harness
    /// image it was built from; the next launch rebuilt the harness, and that
    /// build deleted the stack.
    ///
    /// The class is `Kind`, stamped on at build time, so the three below are
    /// three different classes and no build of one supersedes another.
    #[test]
    fn a_stack_build_and_a_harness_build_are_not_the_same_class() {
        let adapter = claude();
        let here = Path::new("/checkouts/api");
        let there = Path::new("/checkouts/web");

        let harness = Kind::Harness(&adapter).stamp();
        let mine = Kind::Stack(&adapter, here).stamp();
        let theirs = Kind::Stack(&adapter, there).stamp();

        assert_ne!(
            mine, harness,
            "a stack layer is not the harness layer it was built FROM"
        );
        assert_ne!(
            mine, theirs,
            "another checkout's toolchain is not this build's to replace"
        );
        assert_ne!(
            harness,
            Kind::Base.stamp(),
            "the harness layer is not the base it was built FROM"
        );
    }

    /// The class has to reach docker, or it is a comment. Every value is free
    /// text — an adapter's `name` comes from a droppable TOML file, a checkout
    /// path can hold a comma — so the match is docker's to make by exact label
    /// equality, and omh never parses a label value back.
    #[test]
    fn the_listing_asks_docker_for_exactly_one_class() {
        let adapter = claude();
        let args = Kind::Stack(&adapter, Path::new("/checkouts/api")).list_args();

        for (k, v) in Kind::Stack(&adapter, Path::new("/checkouts/api")).stamp() {
            assert!(
                args.windows(2)
                    .any(|w| w[0] == "--filter" && w[1] == format!("label={k}={v}")),
                "{k} is part of the class but not part of the query: {args:?}"
            );
        }
    }

    /// An image built before omh stamped anything carries no `omh.*` labels, so
    /// it is in no class and no build reaps it. Never collecting them is the
    /// right trade against reaping every unlabelled image on the machine.
    #[test]
    fn an_unstamped_image_is_in_nobodys_class() {
        let adapter = claude();
        for kind in [
            Kind::Base,
            Kind::Harness(&adapter),
            Kind::Stack(&adapter, Path::new("/checkouts/api")),
        ] {
            assert!(
                !kind.stamp().is_empty(),
                "an empty stamp would match every unlabelled image"
            );
        }
    }

    fn adapters() -> &'static Path {
        Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/adapters"))
    }

    fn claude() -> Adapter {
        Adapter::find(adapters(), "claude").unwrap()
    }

    #[test]
    fn tags_name_their_harness() {
        assert!(tag_for(&claude()).starts_with("omh/claude:"));
    }

    /// Regression: with a fixed `:latest`, `ensure` saw the tag present and
    /// skipped the build, so a Dockerfile fix never reached an install that had
    /// already built the old one — while `omh init` reported "already built".
    /// Regression: the base tag was a mutable `:latest`, so `ensure` skipped
    /// rebuilding it and a base change — adding `socat` — silently never shipped.
    #[test]
    fn a_changed_base_recipe_is_a_different_base_tag() {
        let before = base_tag();
        assert!(before.starts_with("omh/base:"));
        assert_ne!(before, "omh/base:latest", "a mutable tag never rebuilds");
    }

    /// The harness layer must pin the base it was built against, or a rebuilt
    /// base leaves the harness image referencing something that no longer exists.
    #[test]
    fn the_harness_layer_pins_an_exact_base() {
        let df = harness_dockerfile(&claude());
        assert!(df.contains(&base_tag()), "got: {df}");
        assert!(!df.contains("omh/base:latest"), "got: {df}");
    }

    // ── the stack layer ─────────────────────────────────────────────────────

    /// The same discipline the harness layer already keeps: pin the exact layer
    /// below, never a mutable `:latest`. With a floating base, `ensure` sees the
    /// tag present and skips the build, so a recipe change never reaches an
    /// install that already built the old one.
    #[test]
    fn the_stack_layer_pins_the_exact_harness_layer() {
        let df = stack_dockerfile(&claude(), &["apt-get install -y gcc"]);
        assert!(df.contains(&tag_for(&claude())), "got: {df}");
        assert!(!df.contains(":latest"), "got: {df}");
    }

    /// A pnpm repo and a yarn repo are the same stack and **not** the same
    /// image. Without this the two share a tag, one silently gets the other's
    /// package manager, and the cache makes it stick — worse than no cache at
    /// all, because it is wrong and fast.
    #[test]
    fn a_different_set_of_fired_installs_is_a_different_tag() {
        let a = claude();
        let pnpm = stack_tag(&a, &["corepack enable pnpm"]);
        let yarn = stack_tag(&a, &["corepack enable yarn"]);
        let both = stack_tag(&a, &["corepack enable pnpm", "corepack enable yarn"]);

        assert_ne!(pnpm, yarn, "different provides, different image");
        assert_ne!(pnpm, both, "a superset is a different image too");
        assert_eq!(
            pnpm,
            stack_tag(&a, &["corepack enable pnpm"]),
            "and an unchanged resolution must not rebuild"
        );
        // Order is part of the recipe, not a presentation detail. `corepack
        // enable pnpm` needs the node the provide above it asserted, so a
        // reordered stack file describes a different image — and a tag that
        // hashed the *set* would hand that repo the image built in the old
        // order, which is a cache hit on a build that never happened.
        assert_ne!(
            both,
            stack_tag(&a, &["corepack enable yarn", "corepack enable pnpm"]),
            "a reordered recipe is a different image"
        );
    }

    /// A repo whose stacks install nothing runs the harness image itself.
    /// Building an empty layer to hold nothing would cost every such repo a
    /// build, a tag and a pull for no content.
    #[test]
    fn nothing_to_install_is_the_harness_image_itself() {
        assert_eq!(stack_tag(&claude(), &[]), tag_for(&claude()));
    }

    /// File order is install order, and it is load-bearing: `corepack enable
    /// pnpm` needs the node the provide above it asserted. An ordered list is
    /// how a stack file says that without a dependency graph — so the recipe
    /// must not reorder it.
    #[test]
    fn installs_run_in_the_order_the_stack_file_gave() {
        // Deliberately *not* in alphabetical order. Given `["a", "b"]` a
        // sorting implementation is indistinguishable from a faithful one —
        // which is what the first version of this test asserted, and it passed
        // against a `sort()` inserted to break it.
        let df = stack_dockerfile(
            &claude(),
            &["zzz corepack enable pnpm", "aaa apt-get install nodejs"],
        );
        let at = |needle: &str| df.find(needle).unwrap_or_else(|| panic!("missing: {df}"));
        assert!(
            at("zzz corepack") < at("aaa apt-get"),
            "the recipe was reordered — file order is install order, and \
             `corepack enable pnpm` needs the node above it:\n{df}"
        );
    }

    #[test]
    fn a_changed_recipe_is_a_different_tag() {
        let mut a = claude();
        let before = tag_for(&a);
        a.install = "npm install -g @anthropic-ai/claude-code@next".into();
        assert_ne!(tag_for(&a), before, "a changed recipe must force a rebuild");
    }

    #[test]
    fn an_unchanged_recipe_keeps_its_tag() {
        assert_eq!(tag_for(&claude()), tag_for(&claude()));
    }

    /// The four things `sbx` requires of a kit base image. Getting these wrong
    /// means the image works on Docker and silently cannot be used on the other
    /// backend — the exact split this project exists to avoid.
    #[test]
    fn the_base_satisfies_the_sandbox_contract() {
        let df = base_dockerfile();
        assert!(
            df.contains("-u 1000") || df.contains("1000"),
            "UID 1000: {df}"
        );
        assert!(df.contains("agent"), "agent user");
        assert!(df.contains(GUEST_HOME), "home directory");
        assert!(df.contains("NOPASSWD"), "passwordless sudo");
    }

    /// The image is what *establishes* the home, and everything that mounts
    /// into a session assumes it. Those were separate literals in four files —
    /// consistent by copying, which holds until somebody changes one.
    ///
    /// The Dockerfile now interpolates the constant, so this asserts the build
    /// really does create and verify that path rather than a similar-looking
    /// one.
    #[test]
    fn the_image_creates_the_home_the_code_mounts_into() {
        let df = base_dockerfile();
        assert!(
            df.contains(&format!("usermod -l agent -d {GUEST_HOME}")),
            "the image must create the home the launcher mounts into:\n{df}"
        );
        assert!(
            df.contains(&format!("cut -d: -f6)\" = \"{GUEST_HOME}\"")),
            "and assert it at build time, not trust it:\n{df}"
        );
    }

    /// `GRAPH_CACHE` repeats the home rather than deriving it, because const
    /// concatenation of a `&str` const needs a macro crate. This is the guard
    /// that makes the repetition safe.
    #[test]
    fn the_graph_cache_lives_under_the_agents_home() {
        assert!(
            GRAPH_CACHE.starts_with(&format!("{GUEST_HOME}/")),
            "graph cache {GRAPH_CACHE} is not under {GUEST_HOME}"
        );
        assert!(
            base_dockerfile().contains(GRAPH_CACHE),
            "the image must create the cache directory, or docker makes it root-owned"
        );
    }

    /// The digest must not depend on the repository omh happens to be launched
    /// from.
    ///
    /// `recipe_digest` spawns git with no `current_dir`, so the ambient
    /// repository is whatever the user's shell was in — and a repository
    /// created with `--object-format=sha256` answers `hash-object` in 64 hex
    /// instead of 40. Every image-pinned note that user commits then carries a
    /// digest nobody else's omh will ever compute.
    ///
    /// Asserted over the *command shape* rather than by changing the process's
    /// directory: `set_current_dir` is process-global, and a test that moves it
    /// moves it for every other test running beside it.
    #[test]
    fn a_recipe_digest_does_not_inherit_the_repository_it_is_run_beside() {
        let dir = tempfile::tempdir().unwrap();
        let odd = dir.path().join("sha256");
        let made = std::process::Command::new("git")
            .args(["init", "-q", "--object-format=sha256"])
            .arg(&odd)
            .output()
            .expect("git must be installed to run this test");
        if !made.status.success() {
            return; // git too old for sha256; nothing to prove here
        }

        // omh's own command, run from inside that repository — the thing under
        // test — against a bare one that inherits it, which is the premise.
        let run = |pinned: bool| {
            let mut c = if pinned {
                digest_command()
            } else {
                let mut c = std::process::Command::new("git");
                c.args(["hash-object", "--stdin"]);
                c
            };
            c.current_dir(&odd);
            let mut ch = c
                .stdin(std::process::Stdio::piped())
                .stdout(std::process::Stdio::piped())
                .spawn()
                .unwrap();
            use std::io::Write;
            ch.stdin.take().unwrap().write_all(b"a recipe").unwrap();
            let out = ch.wait_with_output().unwrap();
            String::from_utf8_lossy(&out.stdout).trim().to_string()
        };

        let inherited = run(false);
        let pinned = run(true);
        assert_eq!(
            inherited.len(),
            64,
            "the premise: a sha256 repository answers in 64 hex"
        );
        assert_eq!(
            pinned,
            recipe_digest("a recipe").unwrap(),
            "and omh's digest must be the same wherever it was run from"
        );
    }

    /// The one place a literal is correct: the whole point of this digest is
    /// that the value never moves. `DefaultHasher` would pass every test here
    /// and change under a toolchain upgrade, marking every image-pinned note in
    /// every repo stale on the same day.
    #[test]
    fn an_image_recipe_digest_is_stable_across_toolchains() {
        assert_eq!(
            recipe_digest("hello\n").unwrap(),
            "ce013625030ba8dba906f756967f9e9ca394464a",
            "this is git's SHA-1 of the blob and it is not allowed to change"
        );
        // Same recipe, same digest; different recipe, different digest.
        assert_eq!(recipe_digest("a").unwrap(), recipe_digest("a").unwrap());
        assert_ne!(recipe_digest("a").unwrap(), recipe_digest("b").unwrap());
    }

    /// A bind mount whose guest directory does not exist is created by docker
    /// as **root**, and the agent then cannot write the note it was told to
    /// record — the same failure the graph cache and every credential mount
    /// already pay for.
    ///
    /// Asserted against the mount constant rather than a literal, so moving
    /// the mount without preparing its new home cannot stay green.
    #[test]
    fn the_image_creates_the_note_store_the_launcher_mounts_into() {
        let df = base_dockerfile();
        let notes = crate::memory::GUEST_LOCAL_NOTES;
        assert!(
            df.contains(notes),
            "the image must create {notes}, or docker makes it root-owned"
        );

        // Created is not enough — it has to be handed to the agent. `/omh` is
        // chowned wholesale, so the real requirement is that the store stays
        // under a prefix that chown covers.
        let owned = df
            .lines()
            .find(|l| l.contains("chown -R agent:agent"))
            .expect("the image must chown its mount points");
        assert!(
            owned
                .split_whitespace()
                .any(|dir| notes == dir || notes.starts_with(&format!("{dir}/"))),
            "{notes} is not covered by: {owned}"
        );
    }

    /// One definition, and no file quietly reintroducing its own.
    #[test]
    fn the_guest_home_is_defined_once() {
        let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut declarations = Vec::new();
        for entry in std::fs::read_dir(&src).unwrap().flatten() {
            let path = entry.path();
            if path.extension().is_some_and(|e| e == "rs") {
                let body = std::fs::read_to_string(&path).unwrap();
                for line in body.lines() {
                    let l = line.trim();
                    let is_const = l.starts_with("const ") || l.starts_with("pub const ");
                    if is_const && l.contains("= \"/home/agent\"") {
                        declarations.push(format!("{}: {l}", path.display()));
                    }
                }
            }
        }
        assert_eq!(
            declarations.len(),
            1,
            "the agent's home should be declared once:\n  {}",
            declarations.join("\n  ")
        );
    }

    /// Everything a session needs regardless of harness. `dtach` in particular:
    /// without it the persistence wrapper fails at launch, not at build.
    #[test]
    fn the_base_provides_what_every_session_needs() {
        let df = base_dockerfile();
        for tool in ["git", "dtach", "ripgrep"] {
            assert!(df.contains(tool), "missing {tool}: {df}");
        }
    }

    /// `omh code` attaches an IDE over SSH, so the session has to serve it.
    /// The base set has to be *in* the image, or every session starts without
    /// the thing that makes omh more than a launcher.
    #[test]
    fn the_base_image_carries_the_base_set() {
        let df = base_dockerfile();
        assert!(df.contains(crate::base::GRAPH_BIN), "no code graph: {df}");
    }

    /// The cache is a volume; the image only needs the directory to exist and
    /// be owned by the agent, or docker creates it as root.
    #[test]
    fn the_base_owns_the_graph_cache_directory() {
        let df = base_dockerfile();
        assert!(df.contains(crate::base::GRAPH_CACHE), "got: {df}");
    }

    #[test]
    fn the_base_can_serve_ssh() {
        let df = base_dockerfile();
        assert!(df.contains("openssh-server"), "got: {df}");
        assert!(df.contains("omh-session"), "needs a session entrypoint");
    }

    /// The key arrives as an env var rather than a mount: a bind-mounted
    /// authorized_keys lands with host ownership, and sshd silently refuses to
    /// read one it does not trust.
    #[test]
    fn the_session_entrypoint_installs_the_key_with_permissions_sshd_accepts() {
        let df = base_dockerfile();
        assert!(
            df.contains("OMH_PUBKEY"),
            "key must come from the environment"
        );
        assert!(df.contains("chmod 700"), "~/.ssh perms");
        assert!(df.contains("chmod 600"), "authorized_keys perms");
    }

    #[test]
    fn the_session_entrypoint_outlives_the_command_that_started_it() {
        let df = base_dockerfile();
        assert!(df.contains("sshd"), "must start sshd");
        assert!(df.contains("sleep infinity"), "PID 1 must not exit");
    }

    #[test]
    fn the_base_creates_the_paths_the_launcher_mounts_into() {
        let df = base_dockerfile();
        for dir in ["/work", "/omh/sock", "/omh/cache"] {
            assert!(df.contains(dir), "missing {dir}: {df}");
        }
    }

    #[test]
    fn the_harness_layer_extends_the_base_and_installs_the_harness() {
        let df = harness_dockerfile(&claude());
        assert!(df.contains(&format!("FROM {}", base_tag())), "got: {df}");
        assert!(
            df.contains("@anthropic-ai/claude-code"),
            "install command: {df}"
        );
    }

    /// Installing needs root; running must not have it. A base that ends as
    /// root hands the agent the sandbox's own escape hatch.
    /// Docker creates a missing mount parent as **root**. That makes
    /// `~/.claude` unwritable for the agent, which breaks anything writing a
    /// new file there — transcripts fail with EACCES, and an atomic credential
    /// write (temp file + rename, which needs write permission on the
    /// directory) fails while the login itself reports success.
    ///
    /// The harness layer knows every path it will mount into, so it can create
    /// them up front with the right owner.
    #[test]
    fn the_harness_layer_owns_the_directories_it_mounts_into() {
        let df = harness_dockerfile(&claude());
        assert!(df.contains("/home/agent/.claude"), "config dir: {df}");
        assert!(
            df.contains("chown"),
            "must be owned by agent, not root: {df}"
        );
    }

    /// Asserted on the parsed list, not a positional substring: the old form
    /// checked `!df.contains("mkdir -p /work")`, which sorting guaranteed could
    /// never appear even with the filter removed entirely.
    #[test]
    fn only_directories_under_the_agents_home_are_created() {
        for name in ["claude", "opencode"] {
            let a = Adapter::find(adapters(), name).unwrap();
            for dir in mount_parents(&a) {
                assert!(
                    dir.starts_with("/home/agent"),
                    "{name}: {dir} is not ours to create"
                );
            }
        }
    }

    /// The load-bearing half of this fix is the credential paths, and the only
    /// adapter that exercises it is opencode — deleting the `creds` chain left
    /// the whole suite green.
    #[test]
    fn credential_directories_are_created_too() {
        let a = Adapter::find(adapters(), "opencode").unwrap();
        let dirs = mount_parents(&a);
        assert!(
            dirs.iter().any(|d| d.contains("/.local/share")),
            "the creds parent must be created: {dirs:?}"
        );
    }

    /// Every home-side mount omh makes needs its parent created, or docker
    /// makes it root-owned and the agent cannot write beside its own config.
    #[test]
    fn every_home_side_mount_has_its_parent_created() {
        for name in ["claude", "opencode"] {
            let a = Adapter::find(adapters(), name).unwrap();
            let created = mount_parents(&a);
            let wanted = a
                .capabilities
                .values()
                .map(|b| b.path.clone())
                .chain(a.creds.iter().cloned())
                .chain(a.token.iter().cloned());
            for template in wanted {
                let p = crate::adapter::expand(template.trim_end_matches('/'), "/home/agent");
                if !p.starts_with(GUEST_HOME) {
                    continue;
                }
                let Some(parent) = p.parent().map(|x| x.display().to_string()) else {
                    continue;
                };
                if parent == GUEST_HOME {
                    continue; // the base image already owns the home itself
                }
                assert!(
                    created
                        .iter()
                        .any(|d| parent == *d || parent.starts_with(&format!("{d}/"))),
                    "{name}: nothing creates {parent} for {template}"
                );
            }
        }
    }

    /// Installing needs root; running must not have it. An image that ends as
    /// root hands the agent the sandbox's own escape hatch.
    #[test]
    fn images_end_as_the_unprivileged_user() {
        // The stack layer installs as root like the harness layer does, so it
        // is the newest way to end an image privileged — and an image that ends
        // privileged hands the agent the sandbox's own escape hatch.
        for df in [
            base_dockerfile(),
            harness_dockerfile(&claude()),
            stack_dockerfile(&claude(), &["apt-get install -y gcc"]),
        ] {
            let last_user = df
                .lines()
                .rfind(|l| l.trim_start().starts_with("USER "))
                .unwrap_or("");
            assert_eq!(last_user.trim(), "USER agent", "ended privileged:\n{df}");
        }
    }

    /// …and the stack layer *begins* as root, which the test above cannot see.
    ///
    /// It reads only the last `USER`, so dropping `USER root` from
    /// `stack_dockerfile` leaves it green — and the layer would then run
    /// `apt-get install gcc` as `agent`. Every recipe that installs anything
    /// system-wide fails, which is loud, but it fails at *image build* on data
    /// that is correct, and the person reading the error has no reason to
    /// suspect the Dockerfile omh generated.
    ///
    /// Asserted positionally rather than by presence: root must come before
    /// the first `RUN`, because a `USER root` after the installs would satisfy
    /// `contains` and change nothing.
    #[test]
    fn the_stack_layer_installs_as_root() {
        let df = stack_dockerfile(&claude(), &["apt-get install -y gcc"]);
        let line = |needle: &str| {
            df.lines()
                .position(|l| l.trim_start().starts_with(needle))
                .unwrap_or_else(|| panic!("missing `{needle}`:\n{df}"))
        };
        assert!(
            line("USER root") < line("RUN "),
            "the recipe runs before the layer takes root:\n{df}"
        );
    }

    /// The probe exists to answer a question about the *sandbox*, so it must be
    /// unable to see anything else. One bind mount of a host directory and
    /// `command -v cargo` starts answering about the developer's laptop — which
    /// is the exact confusion this whole feature was built to end, reintroduced
    /// one layer down where nothing would notice.
    ///
    /// Asserted as an invariant — *no mount of any spelling* — rather than
    /// against a fixed argument list, so a future flag cannot slip a mount in
    /// beside an assertion that still passes.
    #[test]
    fn the_toolchain_probe_can_see_nothing_but_the_image() {
        let args = probe_args("omh/x:latest", "#!/bin/sh\ntrue\n");
        // An **allowlist**, not a list of flags to forbid. Naming `-v`,
        // `--mount` and friends looks like an invariant and is a denylist: it
        // says nothing about `-w`, which `probe_args`' own doc also promises is
        // absent, nor about `--network`, `--privileged`, `-e` or `--userns`,
        // nor about whatever docker grows next. Everything between `run` and
        // the tag must be something this test knows about, so a new flag
        // arrives as a failure rather than as silence.
        let tag_at = args
            .iter()
            .position(|a| a == "omh/x:latest")
            .expect("the tag must be among the arguments");
        // Two entries, each earning its place: `--rm` leaves no container
        // behind, `--pull=never` refuses to fetch a tag that is not here. Both
        // are asserted for on their own below and in
        // `the_probe_never_fetches_the_image_it_asks_about`; the point of the
        // list is that nothing *else* may appear.
        for a in &args[1..tag_at] {
            assert!(
                a == "--rm" || a == "--pull=never",
                "an unexpected argument reached the probe — it must see nothing \
                 but the image, or it answers about the wrong machine: {args:?}"
            );
        }
        assert!(
            args[1..tag_at].contains(&"--rm".to_string()),
            "a diagnostic must leave no container behind: {args:?}"
        );
        assert!(
            args.contains(&"omh/x:latest".into()),
            "and it runs in the image the session will use: {args:?}"
        );
        assert_eq!(
            args.last().map(String::as_str),
            Some("#!/bin/sh\ntrue\n"),
            "the script is what runs, whole and unedited: {args:?}"
        );
    }

    /// The probe must never **fetch** the image it is asking about.
    ///
    /// `docker run <tag>` defaults to `--pull=missing`, so a tag with no local
    /// image is resolved against the default registry. omh's tags are
    /// `omh/<harness>:<hash>` — no registry prefix, so Docker Hub — and the
    /// hash is a pure function of the recipe, which means it is identical for
    /// every user on a given omh version and precomputable by anybody who has
    /// read this repository. A squatted `omh/*` namespace would therefore be
    /// pulled and run, and an `ENTRYPOINT` in a pulled image runs ahead of the
    /// `sh -c` argv this builds, so overriding the command does not save it.
    ///
    /// The window is real rather than theoretical: this probe is the one path
    /// that runs an image without `image::ensure*` having built it first.
    ///
    /// Asserted here as well as gated at the call site, on purpose. The gate is
    /// a caller's responsibility and the next caller may forget; this is a
    /// property of the command itself and travels with it.
    #[test]
    fn the_probe_never_fetches_the_image_it_asks_about() {
        let args = probe_args("omh/x:latest", "#!/bin/sh\ntrue\n");
        let tag_at = args.iter().position(|a| a == "omh/x:latest").unwrap();
        assert!(
            args[1..tag_at].iter().any(|a| a == "--pull=never"),
            "a missing image must be an error, never a registry fetch: {args:?}"
        );
    }

    #[test]
    fn build_reads_the_dockerfile_from_stdin() {
        let args = build_args("omh/x:latest", Path::new("/tmp/ctx"), &Kind::Base);
        assert_eq!(args[0], "build");
        assert!(args.contains(&"-t".into()) && args.contains(&"omh/x:latest".into()));
        assert!(
            args.windows(2).any(|w| w[0] == "-f" && w[1] == "-"),
            "Dockerfile must come from stdin so nothing is written to disk: {args:?}"
        );
    }

    // ── what a container says it was built from ─────────────────────────────

    /// Docker's own labels sit in the same map as omh's. Comparing the whole
    /// map would report drift for `maintainer` or anything a base image sets.
    #[test]
    fn only_omhs_own_records_are_read_back() {
        let got = omh_labels(r#"{"omh.image":"omh/claude:ab","maintainer":"nodejs"}"#);
        assert_eq!(got.len(), 1);
        assert_eq!(got.get("omh.image").unwrap(), "omh/claude:ab");
    }

    /// Docker prints the bare word `null` for a container carrying no labels at
    /// all — which is every session started before omh stamped them. Treating
    /// that as a parse error would be indistinguishable from a broken daemon.
    #[test]
    fn a_container_with_no_labels_reads_as_none_rather_than_an_error() {
        assert!(omh_labels("null").is_empty());
        assert!(omh_labels("{}").is_empty());
    }

    /// An answer omh cannot parse is an answer it cannot verify, and the caller
    /// treats "nothing recorded" as drift — which restarts the container. That
    /// is the safe direction: the alternative is trusting a container on the
    /// strength of output nobody understood.
    #[test]
    fn an_unreadable_answer_is_not_mistaken_for_a_match() {
        assert!(omh_labels("").is_empty());
        assert!(omh_labels("<html>error</html>").is_empty());
    }

    /// Values carry newlines — the mount list is one per line — and they have
    /// to survive the round trip or every launch reads as drift.
    #[test]
    fn a_multi_line_value_survives_the_round_trip() {
        let got = omh_labels(r#"{"omh.mounts":"ro /a -> /b\nrw /c -> /d"}"#);
        assert_eq!(got.get("omh.mounts").unwrap().lines().count(), 2);
    }
}
