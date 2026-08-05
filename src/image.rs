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

pub const BASE_TAG: &str = "omh/base:latest";

pub fn tag(harness: &str) -> String {
    format!("omh/{harness}:latest")
}

pub fn base_dockerfile() -> String {
    // node:*-slim ships a `node` user already holding UID 1000, so rename it
    // rather than fighting it — sbx requires that UID to be `agent`.
    r#"FROM node:22-bookworm-slim

RUN apt-get update && apt-get install -y --no-install-recommends \
      ca-certificates git ripgrep dtach sudo curl less jq procps openssh-server \
 && rm -rf /var/lib/apt/lists/*

RUN usermod -l agent -d /home/agent -m node \
 && groupmod -n agent node \
 && echo 'agent ALL=(ALL) NOPASSWD:ALL' > /etc/sudoers.d/agent \
 && chmod 0440 /etc/sudoers.d/agent

# Assert the sbx kit contract at build time rather than assuming it. If a future
# base image moves UID 1000, this fails here instead of failing mysteriously
# inside a sandbox.
RUN test "$(id -u agent)" = "1000" && test "$(getent passwd agent | cut -d: -f6)" = "/home/agent"

# Mount points the launcher expects to exist, owned by the unprivileged user.
RUN mkdir -p /work /omh/sock /omh/cache /omh/layers \
 && chown -R agent:agent /work /omh

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
    .to_string()
}

pub fn harness_dockerfile(adapter: &Adapter) -> String {
    // Install as root, run as agent: an image that ends privileged would hand
    // the agent the sandbox's own escape hatch.
    let mut df = format!("FROM {BASE_TAG}\nUSER root\nRUN {}\n", adapter.install);

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

/// Every directory under the agent's home that omh will mount into.
fn mount_parents(adapter: &Adapter) -> Vec<String> {
    const HOME: &str = "/home/agent";
    let mut dirs: Vec<String> = adapter
        .capabilities
        .values()
        .map(|b| b.path.clone())
        .chain(adapter.creds.iter().cloned())
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
    if !exists(program, BASE_TAG) {
        eprintln!("omh: building {BASE_TAG} (first run only)");
        build(program, BASE_TAG, &base_dockerfile())?;
    }
    let t = tag(&adapter.name);
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
pub fn container_remove(program: &str, name: &str) {
    let _ = std::process::Command::new(program)
        .args(["rm", "-f", name])
        .output();
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

    fn claude() -> Adapter {
        Adapter::find(
            Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/adapters")),
            "claude",
        )
        .unwrap()
    }

    #[test]
    fn tags_are_derived_from_the_harness_name() {
        assert_eq!(tag("claude"), "omh/claude:latest");
        assert_eq!(tag("opencode"), "omh/opencode:latest");
    }

    /// The four things `sbx` requires of a kit base image. Getting these wrong
    /// means the image works on Docker and silently cannot be used on the other
    /// backend — the exact split this project exists to avoid.
    #[test]
    fn the_base_satisfies_the_sandbox_contract() {
        let df = base_dockerfile();
        assert!(df.contains("-u 1000") || df.contains("1000"), "UID 1000: {df}");
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
        assert!(df.contains("OMH_PUBKEY"), "key must come from the environment");
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
        assert!(df.contains(&format!("FROM {BASE_TAG}")), "got: {df}");
        assert!(df.contains("@anthropic-ai/claude-code"), "install command: {df}");
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
        assert!(df.contains("chown"), "must be owned by agent, not root: {df}");
    }

    #[test]
    fn only_directories_under_the_agents_home_are_created() {
        let df = harness_dockerfile(&claude());
        // rules live in the worktree, which is a mount omh controls
        assert!(!df.contains("mkdir -p /work"), "not ours to create: {df}");
    }

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
