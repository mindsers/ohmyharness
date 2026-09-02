//! Getting `omh` itself into the sandbox.
//!
//! MCP servers are spawned by the harness, and the harness runs *inside* the
//! container — so the base set declaring `command = "omh"` only means anything
//! if an `omh` exists in there. The code graph sidesteps this by curling a
//! published release into the image; baking omh's own source into the base
//! image would rebuild it on every edit.
//!
//! So: cross-build once into `~/.omh/bin`, bind-mount it read-only. The build
//! is a `cargo build` inside a throwaway `rust` container, cached by a digest
//! of the sources, which means it reruns exactly when the code changes and not
//! otherwise.
//!
//! Releases publish static musl linux binaries now — the answer this was
//! written while waiting for — but nothing here fetches one, so a macOS omh
//! installed from a release finds no sources to build from and launches
//! without a memory server, saying so. Downloading the tarball matching its
//! own version is what closes that, and it is not done.

use anyhow::{bail, Context, Result};
use std::path::{Path, PathBuf};

/// Where the sandbox expects to find it. Not under the agent's home, because
/// it is a program rather than state, and `/usr/local/bin` is already on PATH.
pub const GUEST_BIN: &str = "/usr/local/bin/omh";

/// What has to happen before the server can start.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Delivery {
    /// The running binary is already a Linux binary for the right
    /// architecture: mount it and change nothing.
    HostBinary(PathBuf),
    /// A cross-build from an earlier launch, still good.
    Cached(PathBuf),
    /// Nothing usable yet.
    MustBuild(PathBuf),
}

/// Where a cross-built binary for `arch` lives.
///
/// Keyed by architecture, and that is not decoration: an x86 binary mounted
/// into an arm64 container fails with `exec format error`, which the harness
/// reports as the MCP server crashing — a cause nobody would guess from the
/// message.
pub fn cached_at(root: &Path, arch: &str) -> PathBuf {
    root.join("bin").join(format!("omh-linux-{arch}"))
}

/// Decide, without touching anything.
///
/// `exists` is injected rather than probed so the whole decision is a table
/// test — this is the part that can be wrong silently, and the shelling-out
/// part is the part that fails loudly.
pub fn plan_delivery(
    os: &str,
    arch: &str,
    current_exe: &Path,
    root: &Path,
    exists: &dyn Fn(&Path) -> bool,
) -> Delivery {
    if os == "linux" {
        return Delivery::HostBinary(current_exe.to_path_buf());
    }
    let cached = cached_at(root, arch);
    match exists(&cached) {
        true => Delivery::Cached(cached),
        false => Delivery::MustBuild(cached),
    }
}

/// The architecture a Linux container will run here, in Rust's target
/// vocabulary. Derived from the host's, because a container without an
/// explicit platform matches it.
pub fn target_arch(host_arch: &str) -> Result<&'static str> {
    match host_arch {
        "aarch64" | "arm64" => Ok("aarch64"),
        "x86_64" | "amd64" => Ok("x86_64"),
        other => bail!("no linux build target known for `{other}`"),
    }
}

/// The binary to mount, if there is one.
///
/// `None` rather than a path that does not exist: the caller mounts what this
/// returns, and docker turns a missing bind source into a directory.
///
/// Two of the ways this answers `None` are not "not built yet" but "omh cannot
/// tell": an unsupported host architecture, and a `current_exe` that will not
/// resolve. Both stay `None`, because a session without memory is still a
/// session — but both say so first. Discarding them silently leaves a session
/// with no memory server and no explanation, including under `omh doctor`,
/// which is the one command whose job is to notice.
pub fn available(paths: &crate::profile::Paths, ctx: &crate::out::Ctx) -> Option<PathBuf> {
    let arch = match target_arch(std::env::consts::ARCH) {
        Ok(a) => a,
        Err(e) => {
            ctx.warn(&format!("no memory server here — {e:#}"));
            return None;
        }
    };
    let exe = match std::env::current_exe() {
        Ok(p) => p,
        Err(e) => {
            ctx.warn(&format!(
                "no memory server here — cannot locate the running omh: {e}"
            ));
            return None;
        }
    };
    let plan = plan_delivery(
        std::env::consts::OS,
        arch,
        &exe,
        &paths.root,
        &|p: &Path| p.exists(),
    );
    match plan {
        Delivery::HostBinary(p) | Delivery::Cached(p) => Some(p),
        Delivery::MustBuild(_) => None,
    }
}

/// Build it if it is not there yet. Called before launch, like `image::ensure`.
pub fn ensure(
    program: &str,
    paths: &crate::profile::Paths,
    crate_dir: &Path,
    ctx: &crate::out::Ctx,
) -> Result<PathBuf> {
    let arch = target_arch(std::env::consts::ARCH)?;
    let exe = std::env::current_exe().context("locating the running omh")?;
    match plan_delivery(
        std::env::consts::OS,
        arch,
        &exe,
        &paths.root,
        &|p: &Path| p.exists(),
    ) {
        Delivery::HostBinary(p) | Delivery::Cached(p) => Ok(p),
        Delivery::MustBuild(out) => {
            // Cross-building needs omh's own sources, which a released binary
            // does not carry. Said plainly rather than failing inside docker
            // with a message about a missing Cargo.toml.
            if !crate_dir.join("Cargo.toml").exists() {
                bail!(
                    "no omh sources at {} — a released build cannot cross-build \
                     itself, so the memory server needs a published linux binary",
                    crate_dir.display()
                );
            }
            // The proxy that made `ca_cert` necessary is in front of
            // crates.io too, and this container is not omh's image.
            let ca = crate::image::ca_path(paths)?;
            build(program, crate_dir, &out, arch, ca.as_deref(), ctx)?;
            Ok(out)
        }
    }
}

/// Cross-build `omh` for Linux, inside a container, into the cache.
///
/// The announcement goes through `Ctx`, not to the terminal: a bare `eprintln!`
/// carries no palette and is not suppressed under `--json`, which is the whole
/// of what `progress` is for. It said so on stderr while cargo owned the
/// terminal underneath it, which is the second half of the same problem.
pub fn build(
    program: &str,
    crate_dir: &Path,
    out: &Path,
    arch: &str,
    ca: Option<&Path>,
    ctx: &crate::out::Ctx,
) -> Result<()> {
    std::fs::create_dir_all(out.parent().context("cache has no parent")?)?;
    let target = format!("{arch}-unknown-linux-gnu");

    ctx.progress(&format!(
        "cross-building the memory server for linux/{arch} — first run only…"
    ));
    let mut child = std::process::Command::new(program)
        .args([
            "run",
            "--rm",
            "--platform",
            &format!("linux/{}", docker_arch(arch)),
        ])
        .arg("-v")
        .arg(format!("{}:/src:ro", crate_dir.display()))
        .arg("-v")
        .arg(format!("{}:/out", out.parent().unwrap().display()))
        // A named volume for the build cache, so a second build is incremental
        // rather than a second cold compile.
        //
        // **Both variables, and that is the whole of it.** `CARGO_HOME` alone
        // caches the registry and nothing else: the compiled artifacts live in
        // `target/`, inside a `--rm` container, so every interrupted or failed
        // build started again from the first crate. Measured before the fix —
        // three launches, three cold compiles from `proc-macro2` — under a
        // comment claiming the opposite.
        .args(["-v", "omh-selfbuild:/cargo"])
        .args(["-e", "CARGO_HOME=/cargo"])
        .args(["-e", "CARGO_TARGET_DIR=/cargo/target"])
        .args(["-w", "/build"])
        .args(ca_args(ca))
        .arg("rust:1-bookworm")
        .args([
            "sh",
            "-c",
            &build_script(&out.file_name().unwrap().to_string_lossy()),
        ])
        // Piped and relayed, for the same reason `image::build` does it: this
        // runs cargo against crates.io through whatever inspects this network,
        // and a bare "the cross-build failed" is the message `ca_cert` exists
        // to replace. Watching it still works — `relay` writes every line back
        // out — and it is a long build, so that matters.
        .stderr(std::process::Stdio::piped())
        .spawn()
        .with_context(|| format!("running {program} for the cross-build"))?;

    let log = match child.stderr.take() {
        Some(err) => crate::image::relay(err),
        None => String::new(),
    };
    let status = child
        .wait()
        .with_context(|| format!("waiting for {program} to finish the cross-build"))?;

    if !status.success() {
        // `ca.is_some()`, carried rather than derived — the caller resolved it
        // to build the `-v` mount three lines up, so unlike the image build
        // there was never a recipe to sniff.
        if let Some(why) = crate::image::why_the_build_failed(&log, ca.is_some()) {
            bail!("cross-building omh for linux/{arch} failed\n\n{why}");
        }
        bail!("cross-building omh for linux/{arch} failed");
    }
    let _ = target; // named for the error message above, not passed to cargo
    Ok(())
}

/// What the cross-build container needs to trust a corporate root, or nothing.
///
/// A function rather than inline `.args(..)` for `build_script`'s reason: this
/// container is upstream's `rust:1-bookworm`, not a recipe omh writes, so
/// there is no recipe text a test could read and no tag that would move. The
/// only way to assert what it is told is to build the arguments somewhere a
/// test can see them.
fn ca_args(ca: Option<&Path>) -> Vec<String> {
    let Some(at) = ca else {
        return Vec::new();
    };
    // Read-only, and at a fixed guest path: what the host calls it is the
    // user's business and what cargo is told to read is omh's.
    const GUEST: &str = "/omh-ca.crt";
    vec![
        "-v".into(),
        format!("{}:{GUEST}:ro", at.display()),
        "-e".into(),
        format!("CARGO_HTTP_CAINFO={GUEST}"),
        "-e".into(),
        format!("SSL_CERT_FILE={GUEST}"),
    ]
}

/// The shell the cross-build runs inside the container.
///
/// A function rather than a `format!` at the call site for `image::digest_command`'s
/// reason: a test can read what will actually run, and the thing that broke
/// here was not a decision the planner made but a literal in the command
/// nothing could see.
/// What the container needs is **derived**, not listed. A literal list is
/// exactly what went stale: `build.rs` was added to the repo and not to the
/// list, and `bundled::ALL` is the same set `build.rs` itself walks — so a
/// seventh shipped directory arrives here without anybody remembering it.
///
/// `set -e` and no `2>/dev/null`: a copy that cannot find a file says which,
/// and stops. Swallowing it is what turned one missing path into a compile
/// error five steps away that read as a bug in cargo.
fn build_script(out_name: &str) -> String {
    let mut sources = vec![
        "/src/src".to_string(),
        "/src/Cargo.toml".to_string(),
        "/src/Cargo.lock".to_string(),
        // Without it cargo runs no build script, sets no `OUT_DIR`, and
        // `bundled.rs`'s `include!` does not compile.
        "/src/build.rs".to_string(),
    ];
    // The data `build.rs` reads at compile time. It panics naming the
    // directory if one is unreadable, which is the right failure — but only
    // once the directory is there to be read.
    sources.extend(
        crate::bundled::ALL
            .iter()
            .map(|kind| format!("/src/{}", kind.dir())),
    );
    format!(
        "set -e; cp -r {} /build/; \
         cd /build && cargo build --release --locked \
         && cp \"$CARGO_TARGET_DIR\"/release/omh /out/{out_name}",
        sources.join(" ")
    )
}

fn docker_arch(arch: &str) -> &str {
    match arch {
        "aarch64" => "arm64",
        _ => "amd64",
    }
}

#[cfg(test)]
mod tests {

    /// **The second command a user hits behind a proxy, and it said nothing.**
    ///
    /// `ca_cert` gets `omh init` working; the cross-build then runs cargo
    /// against crates.io inside `rust:1-bookworm`, through the same inspecting
    /// proxy, and failed with `cross-building omh for linux/… failed` — the
    /// bare message `image::why_the_build_failed` exists to replace, with
    /// nothing pointing back at the setting just fixed. `86c5f5b` mounted the
    /// certificate here and stopped short of diagnosing the failure.
    ///
    /// Driven through a shim standing in for docker, so it needs no container:
    /// the shim writes what a proxied cargo writes and exits non-zero.
    #[test]
    #[cfg(unix)]
    fn a_cross_build_that_died_on_a_certificate_names_the_setting() {
        use std::os::unix::fs::PermissionsExt;
        let d = tempfile::tempdir().unwrap();
        let shim = d.path().join("docker-shim");
        std::fs::write(
            &shim,
            "#!/bin/sh\necho 'error: the SSL certificate is invalid; class=Ssl (16)' >&2\nexit 1\n",
        )
        .unwrap();
        std::fs::set_permissions(&shim, std::fs::Permissions::from_mode(0o755)).unwrap();

        let out = d.path().join("cache").join("omh-memory");
        let crate_dir = d.path().join("crate");
        std::fs::create_dir_all(&crate_dir).unwrap();

        // No certificate set: the user is told one exists and how to set it.
        let e = build(
            &shim.display().to_string(),
            &crate_dir,
            &out,
            "arm64",
            None,
            &crate::out::Ctx::plain(),
        )
        .expect_err("the shim always fails");
        let e = format!("{e:#}");
        assert!(
            e.contains("ca_cert"),
            "the cross-build must name the setting too: {e}"
        );
        assert!(
            e.contains("omh set --local"),
            "and the spelling that works: {e}"
        );

        // A certificate already set: the other answer, not the same one again.
        let pem = d.path().join("corp.pem");
        std::fs::write(
            &pem,
            "-----BEGIN CERTIFICATE-----\nQUJD\n-----END CERTIFICATE-----\n",
        )
        .unwrap();
        let e = build(
            &shim.display().to_string(),
            &crate_dir,
            &out,
            "arm64",
            Some(&pem),
            &crate::out::Ctx::plain(),
        )
        .expect_err("the shim always fails");
        let e = format!("{e:#}");
        assert!(
            !e.contains("omh set --local"),
            "the certificate is mounted here; do not tell them to set it: {e}"
        );

        // And an ordinary failure is left ordinary.
        let plain = d.path().join("plain-shim");
        std::fs::write(
            &plain,
            "#!/bin/sh\necho 'no space left on device' >&2\nexit 1\n",
        )
        .unwrap();
        std::fs::set_permissions(&plain, std::fs::Permissions::from_mode(0o755)).unwrap();
        let e = format!(
            "{:#}",
            build(
                &plain.display().to_string(),
                &crate_dir,
                &out,
                "arm64",
                None,
                &crate::out::Ctx::plain(),
            )
            .expect_err("the shim always fails")
        );
        assert!(
            !e.contains("ca_cert"),
            "a disk-full cross-build is not a proxy problem: {e}"
        );
    }
    use super::*;

    const NEVER: &dyn Fn(&Path) -> bool = &|_: &Path| false;
    const ALWAYS: &dyn Fn(&Path) -> bool = &|_: &Path| true;

    /// The cross-build copies in everything a build of omh needs.
    ///
    /// It did not, and had not since 2026-08-11. The copy list was written on
    /// 2026-08-10 (#3) naming `src`, `Cargo.toml` and `Cargo.lock`; `build.rs`
    /// arrived the next day (#7) and nothing updated it. Without the build
    /// script cargo sets no `OUT_DIR`, so `bundled.rs`'s `include!` fails to
    /// compile and the run dies in a wall of `cannot find Shipped in bundled`
    /// — five steps downstream of the missing file, because `2>/dev/null` on
    /// the `cp` swallowed the only message that named it.
    ///
    /// It went unseen for three weeks because a cached `omh-linux-aarch64`
    /// from 2026-08-08 predates the breakage: any machine holding one skips
    /// the build. A new macOS user has no cache, and this is the memory
    /// server's delivery — so it was the *first* `omh new` that failed.
    ///
    /// **Derived from `bundled::ALL` rather than listed.** A literal list is
    /// what broke, and a test asserting a literal list would have to be
    /// remembered in the same breath as the fix it guards. A seventh shipped
    /// directory now fails this test until the cross-build carries it.
    ///
    /// Every test in this module asserted `plan_delivery`'s decision — which
    /// mode, which cache path, which error. None read the command, and the
    /// defect was entirely inside it: the shape the 0.7.0 retrospective names,
    /// where the suite asserts what the code returns and the bug is in what
    /// the command does.
    #[test]
    fn the_cross_build_copies_in_everything_a_build_needs() {
        let script = build_script("omh-linux-aarch64");

        // Without this, no build script runs and `OUT_DIR` is never set.
        assert!(
            script.contains("/src/build.rs"),
            "the build script itself has to reach the container: {script}"
        );
        for kind in crate::bundled::ALL {
            assert!(
                script.contains(&format!("/src/{}", kind.dir())),
                "build.rs reads {}/ and panics without it: {script}",
                kind.dir()
            );
        }
        // The manifest, lockfile and sources were always there; asserted so a
        // rewrite of the copy cannot drop one while adding the others.
        for needed in ["/src/src", "/src/Cargo.toml", "/src/Cargo.lock"] {
            assert!(script.contains(needed), "{needed} is missing: {script}");
        }
    }

    /// A missing source is a message, not a compile error five steps later.
    ///
    /// `2>/dev/null` is why the real failure read as a compiler bug. The copy
    /// either succeeds or says which path it could not find, and `set -e`
    /// stops there rather than compiling a tree that is missing a file.
    #[test]
    fn the_cross_build_does_not_hide_a_failed_copy() {
        let script = build_script("omh-linux-aarch64");
        assert!(
            !script.contains("2>/dev/null"),
            "a swallowed cp error is what made this take a week to find: {script}"
        );
    }

    /// On Linux the running binary already *is* the thing the sandbox needs.
    /// Cross-building there would be a container spun up to produce a copy of
    /// the file that started it.
    #[test]
    fn on_linux_the_running_binary_is_mounted_as_it_is() {
        let plan = plan_delivery(
            "linux",
            "aarch64",
            Path::new("/usr/bin/omh"),
            Path::new("/home/x/.omh"),
            NEVER,
        );
        assert_eq!(plan, Delivery::HostBinary(PathBuf::from("/usr/bin/omh")));
    }

    /// A darwin binary cannot run in a Linux container, so the host's own
    /// executable is never a candidate there — however convenient.
    #[test]
    fn on_macos_the_hosts_own_binary_is_never_used() {
        for exists in [NEVER, ALWAYS] {
            let plan = plan_delivery(
                "macos",
                "aarch64",
                Path::new("/opt/homebrew/bin/omh"),
                Path::new("/home/x/.omh"),
                exists,
            );
            let chosen = match plan {
                Delivery::HostBinary(p) | Delivery::Cached(p) | Delivery::MustBuild(p) => p,
            };
            assert_ne!(
                chosen,
                PathBuf::from("/opt/homebrew/bin/omh"),
                "a darwin binary in a linux container is `exec format error`"
            );
        }
    }

    #[test]
    fn a_cross_build_is_reused_when_it_is_already_there() {
        let root = Path::new("/home/x/.omh");
        assert_eq!(
            plan_delivery("macos", "aarch64", Path::new("/bin/omh"), root, ALWAYS),
            Delivery::Cached(cached_at(root, "aarch64"))
        );
        assert_eq!(
            plan_delivery("macos", "aarch64", Path::new("/bin/omh"), root, NEVER),
            Delivery::MustBuild(cached_at(root, "aarch64"))
        );
    }

    /// An x86 binary mounted into an arm64 container fails with `exec format
    /// error`, which the harness reports as the MCP server crashing — a cause
    /// nobody would guess from the message. One cache path per architecture is
    /// what stops the two ever being confused.
    #[test]
    fn each_architecture_caches_to_its_own_path() {
        let root = Path::new("/home/x/.omh");
        assert_ne!(cached_at(root, "aarch64"), cached_at(root, "x86_64"));
        for arch in ["aarch64", "x86_64"] {
            assert!(
                cached_at(root, arch).to_string_lossy().contains(arch),
                "the path must name the architecture it holds"
            );
        }
    }

    /// A host omh cannot guess a target for is a loud failure, not a build
    /// that produces something unrunnable.
    #[test]
    fn an_unknown_host_architecture_is_an_error_not_a_guess() {
        assert_eq!(target_arch("aarch64").unwrap(), "aarch64");
        assert_eq!(target_arch("arm64").unwrap(), "aarch64");
        assert_eq!(target_arch("x86_64").unwrap(), "x86_64");
        let err = target_arch("riscv64").unwrap_err().to_string();
        assert!(err.contains("riscv64"), "got: {err}");
    }

    /// A released omh carries no sources, so it cannot build its own Linux
    /// counterpart. That has to be a sentence somebody can act on, not a
    /// failure inside docker about a missing Cargo.toml.
    #[test]
    fn a_build_with_no_sources_says_what_is_actually_wrong() {
        let dir = tempfile::tempdir().unwrap();
        let paths = crate::profile::Paths {
            root: dir.path().join("home"),
            repo: dir.path().join("repo"),
        };
        // Only meaningful where a cross-build would be attempted at all.
        if std::env::consts::OS == "linux" {
            return;
        }
        let err = ensure("docker", &paths, dir.path(), &crate::out::Ctx::plain())
            .unwrap_err()
            .to_string();
        assert!(err.contains("published linux binary"), "got: {err}");
        assert!(
            err.contains(&dir.path().display().to_string()),
            "got: {err}"
        );
    }

    /// The guest path has to be somewhere already on PATH, or the base set's
    /// bare `command = "omh"` resolves to nothing.
    #[test]
    fn the_guest_path_is_on_the_default_path() {
        assert!(GUEST_BIN.starts_with("/usr/local/bin/"));
        assert!(
            !GUEST_BIN.starts_with(crate::image::GUEST_HOME),
            "a program is not state, and the home is mounted over"
        );
    }

    /// **The cross-build is the one container that is not omh's image.**
    ///
    /// `deliver::build` runs stock `rust:1-bookworm` and compiles omh against
    /// crates.io — over TLS, behind the same inspecting proxy `ca_cert` exists
    /// for, in a container that knows nothing about it. So the user who has
    /// just got `omh init` working hits the same unknown-issuer failure in a
    /// different command, with no entry in the troubleshooting page pointing
    /// back at the setting they already fixed.
    ///
    /// The certificate is *mounted* here rather than embedded: this image is
    /// upstream's, not a recipe omh writes, so there is nothing to bake it
    /// into and nothing whose tag would have to move.
    #[test]
    fn the_cross_build_is_given_the_corporate_root_too() {
        let none = ca_args(None);
        assert!(none.is_empty(), "nothing is added when nothing is set");

        let args = ca_args(Some(std::path::Path::new("/etc/ssl/corp.pem")));
        let joined = args.join(" ");
        assert!(
            joined.contains("/etc/ssl/corp.pem:/omh-ca.crt:ro"),
            "the certificate must be mounted, read-only: {joined}"
        );
        for var in ["CARGO_HTTP_CAINFO", "SSL_CERT_FILE"] {
            assert!(
                joined.contains(&format!("{var}=/omh-ca.crt")),
                "{var} is unset, so cargo still cannot reach crates.io: {joined}"
            );
        }
    }
}
