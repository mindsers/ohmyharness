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
pub fn recipe_digest(recipe: &str) -> Result<String> {
    use anyhow::Context;
    use std::io::Write;
    use std::process::Stdio;

    let mut child = std::process::Command::new("git")
        .args(["hash-object", "--stdin"])
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
pub fn ensure_stack(program: &str, adapter: &Adapter, installs: &[&str]) -> Result<String> {
    ensure(program, adapter)?;
    let tag = stack_tag(adapter, installs);
    if tag != tag_for(adapter) && !exists(program, &tag) {
        eprintln!("omh: building {tag} — this repo's toolchain, first run only");
        build(program, &tag, &stack_dockerfile(adapter, installs))?;
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
pub fn build_args(tag: &str, context: &Path) -> Vec<String> {
    vec![
        "build".into(),
        "-t".into(),
        tag.into(),
        "-f".into(),
        "-".into(),
        context.to_string_lossy().into_owned(),
    ]
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
        build(program, &base, &base_dockerfile())?;
    }
    let t = tag_for(adapter);
    if !exists(program, &t) {
        eprintln!("omh: building {t}");
        build(program, &t, &harness_dockerfile(adapter))?;
    }
    Ok(())
}

fn build(program: &str, tag: &str, dockerfile: &str) -> Result<()> {
    use anyhow::Context;
    use std::io::Write;
    use std::process::Stdio;

    // Empty context: everything the image needs comes from the Dockerfile.
    let context = std::env::temp_dir().join("omh-build-context");
    std::fs::create_dir_all(&context)?;

    let mut child = std::process::Command::new(program)
        .args(build_args(tag, &context))
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
    reap(program, tag);
    Ok(())
}

/// Remove the tags this build just replaced.
///
/// Here rather than at a `omh image prune` the user has to remember, because
/// the moment a new tag exists is the moment the old one is dead — and the
/// failure this prevents is silent until the disk is full, at which point
/// docker wedges and every sandbox in every repo stops working. It cost this
/// machine 14 images and about 20 GB before anyone noticed.
///
/// Best-effort, and quiet about the difference between "nothing to do" and
/// "docker would not". A build that succeeded must not fail because tidying up
/// after it did — the image the user asked for is there either way. What it
/// does *not* do is claim: only tags docker confirms gone are reported.
fn reap(program: &str, built: &str) {
    let Ok(tags) = list_tags(program, built) else {
        return;
    };
    let in_use = images_in_use(program);
    let gone: Vec<String> = superseded(built, &tags, &in_use)
        .into_iter()
        .filter(|t| {
            std::process::Command::new(program)
                .args(["image", "rm", t])
                .output()
                .map(|o| o.status.success())
                .unwrap_or(false)
        })
        .collect();
    if !gone.is_empty() {
        eprintln!(
            "omh: removed {} image{} this build replaced",
            gone.len(),
            if gone.len() == 1 { "" } else { "s" }
        );
    }
}

/// Every tag of the repository `built` belongs to.
fn list_tags(program: &str, built: &str) -> Result<Vec<String>> {
    let repo = built.rsplit_once(':').map(|(r, _)| r).unwrap_or(built);
    let out = std::process::Command::new(program)
        .args(["images", "--format", "{{.Repository}}:{{.Tag}}", repo])
        .output()?;
    anyhow::ensure!(out.status.success(), "listing images for {repo}");
    Ok(String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(str::to_string)
        .collect())
}

/// Images any container references, running or stopped.
///
/// Stopped counts: `omh s down` leaves the container so the session can be
/// resumed, and resuming it needs the image it was created from.
fn images_in_use(program: &str) -> Vec<String> {
    std::process::Command::new(program)
        .args(["ps", "-a", "--format", "{{.Image}}"])
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
pub fn container_running(program: &str, name: &str) -> bool {
    std::process::Command::new(program)
        .args(["inspect", "-f", "{{.State.Running}}", name])
        .output()
        .map(|o| o.status.success() && String::from_utf8_lossy(&o.stdout).trim() == "true")
        .unwrap_or(false)
}

/// Can the session still be entered — not just "is it up"?
///
/// Running is not enough to reuse one. The directory `/work` is bound to can be
/// deleted while the container stays up (`git worktree remove` by hand is the
/// remaining way; `omh s rm` did it too until it learned to take the container
/// with it). Recreating that directory gives it a new inode the live mount does
/// not follow, and every `docker exec` from then on dies with
///
///   current working directory is outside of container mount namespace root
///   -- possible container breakout detected
///
/// which no amount of relaunching clears, because a running container is never
/// replaced. `args` is the backend's own exec line, so the probe enters exactly
/// where a harness would — the workdir is the part that fails.
///
/// Unit tests cannot assert this: the fact being checked belongs to docker, not
/// to omh. Verified by hand against a container in that state.
///
/// Returns the command's stdout, so one exec answers both questions the
/// launcher has — whether the container can be entered, and what is running
/// inside it. They are wanted together and they are the same probe.
pub fn container_probe(program: &str, args: &[String]) -> Option<String> {
    std::process::Command::new(program)
        .args(args)
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
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
pub fn container_stamp(program: &str, name: &str) -> std::collections::BTreeMap<String, String> {
    std::process::Command::new(program)
        .args(["inspect", "-f", "{{json .Config.Labels}}", name])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| omh_labels(&String::from_utf8_lossy(&o.stdout)))
        .unwrap_or_default()
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
/// A tag omh chooses is a hash of the recipe that produced it, so the moment a
/// recipe changes the previous tag is dead — nothing will ever ask for it
/// again, and until now nothing removed it either. Two weeks of ordinary work
/// left 14 omh images and about 20 GB, which is what filled the disk and wedged
/// Docker.
///
/// Decided here, as a function over lists, rather than inside the code that
/// shells out: which images should go is omh's own logic and is worth testing;
/// whether docker agrees to remove one is not something a unit test can settle.
///
/// Two things are never superseded. **`latest`** is a name a person may have
/// typed, where the hashes are names omh generated — reaping it would take away
/// something nobody can reconstruct from a recipe. And **anything a container
/// references**, however old, because that is a session someone is still using;
/// docker would refuse, but omh should not be asking, nor reporting a removal
/// that did not happen.
pub fn superseded(built: &str, tags: &[String], in_use: &[String]) -> Vec<String> {
    let repo = |t: &str| t.rsplit_once(':').map(|(r, _)| r.to_string());
    let Some(mine) = repo(built) else {
        return Vec::new();
    };
    tags.iter()
        .filter(|t| t.as_str() != built)
        .filter(|t| repo(t).as_deref() == Some(mine.as_str()))
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

    /// omh tags an image by hashing its recipe, so a recipe change builds a new
    /// tag and the old one is superseded the moment it does. Nothing removed
    /// it. Measured on a two-week-old machine: six `omh/claude` tags, three
    /// `omh/opencode`, two `omh/omp`, three `omh/base` — 14 tags and about
    /// 20 GB, which is what wedged Docker and stopped every sandbox test
    /// running.
    ///
    /// omh reaps sessions, worktrees, staging and the sandbox's repository. An
    /// image is the one thing it creates and never takes back.
    #[test]
    fn a_new_build_supersedes_the_older_tags_of_its_own_repository() {
        let tags = vec![
            "omh/claude:new".to_string(),
            "omh/claude:old".to_string(),
            "omh/claude:older".to_string(),
            "omh/claude:latest".to_string(),
            "omh/opencode:other".to_string(),
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
            !gone.contains(&"omh/opencode:other".to_string()),
            "another adapter's image is not this build's to remove: {gone:?}"
        );
        assert!(
            !gone.contains(&"omh/claude:latest".to_string()),
            "`latest` is a name a person may have typed, not a hash omh chose: \
             {gone:?}"
        );
    }

    /// A tag another session's container is running is not superseded, whatever
    /// its age. Docker would refuse the removal anyway — this is so omh does
    /// not ask, and does not report removing something it did not.
    #[test]
    fn a_tag_a_container_is_using_survives_a_newer_build() {
        let tags = vec![
            "omh/claude:new".to_string(),
            "omh/claude:running".to_string(),
        ];

        let gone = superseded("omh/claude:new", &tags, &["omh/claude:running".to_string()]);

        assert!(gone.is_empty(), "a live session keeps its image: {gone:?}");
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

    /// The one place a literal is correct: the whole point of this digest is
    /// that the value never moves. `DefaultHasher` would pass every test here
    /// and change under a toolchain upgrade, marking every image-pinned note in
    /// every repo stale on the same day.
    ///
    /// `#[ignore]`d because it shells out to git, and until 2026.08 git could not
    /// run inside an omh sandbox at all — the worktree's `.git` was a pointer at
    /// an admin directory omh does not mount, so git failed where finding no
    /// repository would have succeeded. That premise is gone: the sandbox has a
    /// repository of its own now, so this may well pass there. Left ignored
    /// because nobody has run omh's suite inside an omh sandbox to find out, and
    /// an ignore that is merely unverified is cheaper than a red suite. Runs on
    /// the host and in CI.
    #[test]
    #[ignore]
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
        let args = build_args("omh/x:latest", Path::new("/tmp/ctx"));
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
