//! Images.
//!
//! Two layers: a base every session shares, and a thin per-harness layer that
//! runs the adapter's install command. The base deliberately satisfies the
//! `sbx` kit contract — non-root `agent` at UID 1000, passwordless sudo,
//! `/home/agent`, proxy env forwarding — so the same image works on either
//! backend and an sbx kit becomes a two-line file rather than a port.

use crate::adapter::Adapter;
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

pub fn base_dockerfile() -> String {
    // node:*-slim ships a `node` user already holding UID 1000, so rename it
    // rather than fighting it — sbx requires that UID to be `agent`.
    format!(
        r#"FROM node:22-bookworm-slim

RUN apt-get update && apt-get install -y --no-install-recommends \
      ca-certificates git ripgrep dtach sudo curl less jq procps openssh-server socat \
 && rm -rf /var/lib/apt/lists/*

RUN usermod -l agent -d /home/agent -m node \
 && groupmod -n agent node \
 && echo 'agent ALL=(ALL) NOPASSWD:ALL' > /etc/sudoers.d/agent \
 && chmod 0440 /etc/sudoers.d/agent

# Assert the sbx kit contract at build time rather than assuming it. If a future
# base image moves UID 1000, this fails here instead of failing mysteriously
# inside a sandbox.
RUN test "$(id -u agent)" = "1000" && test "$(getent passwd agent | cut -d: -f6)" = "/home/agent"

# The base set lives here, not in a harness layer: a code graph is
# harness-agnostic and every session should get the same one.
ARG TARGETARCH
RUN __GRAPH_INSTALL__

# Mount points the launcher expects to exist, owned by the unprivileged user.
# The graph cache is a volume; the image only needs the directory to exist and
# be owned by the agent, or docker creates it as root.
RUN mkdir -p /work /omh/sock /omh/cache /omh/layers /home/agent/.cache/codebase-memory-mcp \
 && chown -R agent:agent /work /omh /home/agent/.cache

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

/// Parents of every mount target under the agent's home.
///
/// `/home/agent` itself is excluded — the base image already owns it — and
/// `/work` is a mount omh controls rather than one the image prepares.
fn mount_parents(adapter: &Adapter) -> Vec<String> {
    const HOME: &str = "/home/agent";
    let mut dirs: Vec<String> = adapter
        .capabilities
        .values()
        .map(|b| b.path.clone())
        .chain(adapter.creds.iter().cloned())
        .chain(adapter.token.iter().cloned())
        .filter_map(|template| {
            let path = crate::adapter::expand(template.trim_end_matches('/'), HOME);
            // `/work` is a mount omh owns; only the home side needs preparing.
            if !path.starts_with(HOME) {
                return None;
            }
            path.parent().map(|p| p.display().to_string())
        })
        .filter(|d| d != HOME)
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
    Ok(())
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
        assert!(df.contains("/home/agent"), "home directory");
        assert!(df.contains("NOPASSWD"), "passwordless sudo");
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
                if !p.starts_with("/home/agent") {
                    continue;
                }
                let Some(parent) = p.parent().map(|x| x.display().to_string()) else {
                    continue;
                };
                if parent == "/home/agent" {
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
        for df in [base_dockerfile(), harness_dockerfile(&claude())] {
            let last_user = df
                .lines()
                .filter(|l| l.trim_start().starts_with("USER "))
                .next_back()
                .unwrap_or("");
            assert_eq!(last_user.trim(), "USER agent", "ended privileged:\n{df}");
        }
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
}
