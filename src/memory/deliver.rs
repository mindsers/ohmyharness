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
//! A macOS omh installed from a release carries no sources to cross-build
//! from. It fetches instead: the static musl linux binary its own version
//! published, verified against that release's `SHA256SUMS` before it is
//! installed, and cached under `~/.omh/bin` keyed by version so the next
//! launch reuses it. `plan_delivery` decides between the running binary, the
//! cache, a cross-build (sources present) and a fetch (sources absent); a
//! fetch that cannot run, or a download that fails its checksum, installs
//! nothing and says why.

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
    /// Sources are here; cross-build it.
    MustBuild(PathBuf),
    /// No sources — a released build — so fetch the published linux binary
    /// this omh's own version publishes. Carries where to put it.
    MustFetch(PathBuf),
}

/// Where a cross-built binary for `arch` lives.
///
/// Keyed by architecture, and that is not decoration: an x86 binary mounted
/// into an arm64 container fails with `exec format error`, which the harness
/// reports as the MCP server crashing — a cause nobody would guess from the
/// message.
pub fn cached_at(root: &Path, arch: &str, version: &str) -> PathBuf {
    root.join("bin").join(format!("omh-linux-{arch}-{version}"))
}

/// Decide, without touching anything.
///
/// `exists` is injected rather than probed so the whole decision is a table
/// test — this is the part that can be wrong silently, and the shelling-out
/// part is the part that fails loudly.
pub fn plan_delivery(
    os: &str,
    arch: &str,
    version: &str,
    current_exe: &Path,
    root: &Path,
    has_sources: bool,
    exists: &dyn Fn(&Path) -> bool,
) -> Delivery {
    if os == "linux" {
        return Delivery::HostBinary(current_exe.to_path_buf());
    }
    let cached = cached_at(root, arch, version);
    if exists(&cached) {
        return Delivery::Cached(cached);
    }
    // Sources present means a dev tree: build. Absent means a released binary,
    // which cannot cross-build itself, so fetch the linux artifact this same
    // release published.
    match has_sources {
        true => Delivery::MustBuild(cached),
        false => Delivery::MustFetch(cached),
    }
}

/// Where the release for `version`/`arch` lives, and the sums beside it.
///
/// Matches `.github/workflows/release.yml`: the static musl target, a tarball
/// named for it holding `omh-<target>/omh`, and one `SHA256SUMS` for the
/// whole release. Static musl on purpose — a gnu build would need the guest's
/// libc, and the base image's is not omh's to assume.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Urls {
    pub tarball: String,
    pub sums: String,
    /// The tarball's basename, which is also its entry in `SHA256SUMS`.
    pub name: String,
}

pub fn fetch_urls(version: &str, arch: &str) -> Urls {
    let target = format!("{arch}-unknown-linux-musl");
    let name = format!("omh-{target}.tar.gz");
    let base = format!("https://github.com/mindsers/ohmyharness/releases/download/v{version}");
    Urls {
        tarball: format!("{base}/{name}"),
        sums: format!("{base}/SHA256SUMS"),
        name,
    }
}

/// The recorded sum for one artifact, out of a `SHA256SUMS` file.
///
/// `sha256sum` writes `<hex>  <name>` and prepends `./` under some shells;
/// a file fetched over HTTP may arrive with CRLF line endings. Neither is the
/// artifact's business, so both are tolerated — and the name must match
/// exactly, so a sum for another platform's tarball never stands in for this
/// one.
pub fn sum_for(sums: &str, name: &str) -> Option<String> {
    for line in sums.lines() {
        let line = line.trim_end_matches('\r');
        let (hex, file) = line.split_once("  ").or_else(|| line.split_once(' '))?;
        let file = file.trim_start_matches("./");
        if file == name {
            let hex = hex.trim();
            if hex.len() == 64 && hex.bytes().all(|b| b.is_ascii_hexdigit()) {
                return Some(hex.to_string());
            }
        }
    }
    None
}

/// The crate directory, only when it really holds *this* omh's sources.
///
/// A released binary is unpacked wherever the user put it, and
/// `CARGO_MANIFEST_DIR` is baked in at compile time — so on such a machine that
/// path may exist and be somebody else's project. A `Cargo.toml` naming a
/// different crate, or this crate at another version, is not sources omh may
/// cross-build itself from.
pub fn sources_at(dir: &Path, version: &str) -> Option<PathBuf> {
    let manifest = std::fs::read_to_string(dir.join("Cargo.toml")).ok()?;
    let table: toml::Table = manifest.parse().ok()?;
    let package = table.get("package")?.as_table()?;
    let named_omh = package.get("name").and_then(|v| v.as_str()) == Some("omh");
    let same_version = package.get("version").and_then(|v| v.as_str()) == Some(version);
    (named_omh && same_version).then(|| dir.to_path_buf())
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
        env!("CARGO_PKG_VERSION"),
        &exe,
        &paths.root,
        // `available` never builds or fetches, so whether sources are present
        // does not change its answer: anything not already cached is `None`.
        false,
        &|p: &Path| p.exists(),
    );
    match plan {
        Delivery::HostBinary(p) | Delivery::Cached(p) => Some(p),
        Delivery::MustBuild(_) | Delivery::MustFetch(_) => None,
    }
}

/// Build it if it is not there yet. Called before launch, like `image::ensure`.
pub fn ensure(
    program: &str,
    paths: &crate::profile::Paths,
    sources: Option<&Path>,
    fetch: &dyn Fn(&Urls, &Path) -> Result<()>,
    ctx: &crate::out::Ctx,
) -> Result<PathBuf> {
    let arch = target_arch(std::env::consts::ARCH)?;
    let exe = std::env::current_exe().context("locating the running omh")?;
    match plan_delivery(
        std::env::consts::OS,
        arch,
        env!("CARGO_PKG_VERSION"),
        &exe,
        &paths.root,
        sources.is_some(),
        &|p: &Path| p.exists(),
    ) {
        Delivery::HostBinary(p) | Delivery::Cached(p) => Ok(p),
        Delivery::MustBuild(out) => {
            let crate_dir = sources.expect("MustBuild is only chosen when sources are present");
            // The proxy that made `ca_cert` necessary is in front of
            // crates.io too, and this container is not omh's image.
            let root = crate::image::ca_for(paths)?;
            let ca = root.as_ref().map(crate::image::Root::path);
            build(program, crate_dir, &out, arch, ca, ctx)?;
            Ok(out)
        }
        Delivery::MustFetch(out) => {
            fetch_into(&out, arch, fetch, ctx)?;
            Ok(out)
        }
    }
}

/// Fetch, verify, and install the published linux binary for `arch`.
///
/// The download lands in a temp directory that drops when this returns; only a
/// verified binary is installed, and it goes in atomically by rename so a
/// half-written file is never mounted. The unversioned cache older omh wrote
/// is removed once the versioned one lands.
fn fetch_into(
    out: &Path,
    arch: &str,
    fetch: &dyn Fn(&Urls, &Path) -> Result<()>,
    ctx: &crate::out::Ctx,
) -> Result<()> {
    let version = env!("CARGO_PKG_VERSION");
    let urls = fetch_urls(version, arch);
    ctx.progress(&format!(
        "fetching the memory server for linux/{arch} v{version} — first run only…"
    ));
    let staging = tempfile::tempdir().context("staging the download")?;
    fetch(&urls, staging.path()).with_context(|| format!("fetching {}", urls.tarball))?;

    let tarball = staging.path().join(&urls.name);
    let sums = std::fs::read_to_string(staging.path().join("SHA256SUMS"))
        .context("reading the published SHA256SUMS")?;
    let want = sum_for(&sums, &urls.name).with_context(|| {
        format!(
            "no v{version} release publishes {} — SHA256SUMS names none",
            urls.name
        )
    })?;
    let got = sha256_of(&tarball)?;
    anyhow::ensure!(
        got == want,
        "the downloaded {} does not match its published checksum — refusing it",
        urls.name
    );

    // The tarball holds `omh-<target>/omh`; extract just that.
    let extracted = staging.path().join("unpacked");
    std::fs::create_dir_all(&extracted)?;
    let status = std::process::Command::new("tar")
        .args(["-xzf"])
        .arg(&tarball)
        .arg("-C")
        .arg(&extracted)
        .status()
        .context("running tar")?;
    anyhow::ensure!(status.success(), "unpacking {} failed", urls.name);
    let binary = std::fs::read_dir(&extracted)?
        .flatten()
        .map(|e| e.path().join("omh"))
        .find(|p| p.exists())
        .with_context(|| format!("{} held no omh binary", urls.name))?;

    std::fs::create_dir_all(out.parent().context("cache has no parent")?)?;
    let tmp = out.with_extension("partial");
    std::fs::copy(&binary, &tmp).context("installing the memory server")?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o755))?;
    }
    std::fs::rename(&tmp, out).context("installing the memory server")?;
    // The unversioned cache an older omh wrote is stale now.
    let _ = std::fs::remove_file(out.parent().unwrap().join(format!("omh-linux-{arch}")));
    Ok(())
}

/// The real fetch: `curl` the tarball and the sums into `dest`.
///
/// `curl` for the same reason the image build uses it — it honours the system
/// trust store the `ca_cert` machinery configures — and `-f` so an HTTP error
/// is a failure rather than a saved error page.
pub fn fetch(urls: &Urls, dest: &Path) -> Result<()> {
    for (url, name) in [
        (&urls.tarball, &urls.name),
        (&urls.sums, &"SHA256SUMS".to_string()),
    ] {
        let status = std::process::Command::new("curl")
            .args(["-fsSL", "-o"])
            .arg(dest.join(name.as_str()))
            .arg(url)
            .status()
            .context("running curl")?;
        anyhow::ensure!(status.success(), "downloading {url}");
    }
    Ok(())
}

/// SHA-256 of a file, through the platform's own tool.
///
/// `shasum -a 256` on macOS, `sha256sum` on Linux — one of the two is always
/// present, and shelling out avoids a hashing crate for the one place omh
/// needs one.
fn sha256_of(file: &Path) -> Result<String> {
    let (program, args): (&str, &[&str]) = if cfg!(target_os = "macos") {
        ("shasum", &["-a", "256"])
    } else {
        ("sha256sum", &[])
    };
    let out = std::process::Command::new(program)
        .args(args)
        .arg(file)
        .output()
        .with_context(|| format!("running {program}"))?;
    anyhow::ensure!(
        out.status.success(),
        "{program} failed on {}",
        file.display()
    );
    String::from_utf8_lossy(&out.stdout)
        .split_whitespace()
        .next()
        .map(str::to_string)
        .context("empty checksum output")
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
            "1.0.0",
            Path::new("/usr/bin/omh"),
            Path::new("/home/x/.omh"),
            false,
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
                "1.0.0",
                Path::new("/opt/homebrew/bin/omh"),
                Path::new("/home/x/.omh"),
                true,
                exists,
            );
            let chosen = match plan {
                Delivery::HostBinary(p)
                | Delivery::Cached(p)
                | Delivery::MustBuild(p)
                | Delivery::MustFetch(p) => p,
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
        let plan = |sources, exists| {
            plan_delivery(
                "macos",
                "aarch64",
                "1.0.0",
                Path::new("/bin/omh"),
                root,
                sources,
                exists,
            )
        };
        assert_eq!(
            plan(true, ALWAYS),
            Delivery::Cached(cached_at(root, "aarch64", "1.0.0"))
        );
        assert_eq!(
            plan(true, NEVER),
            Delivery::MustBuild(cached_at(root, "aarch64", "1.0.0"))
        );
        // No sources and nothing cached: fetch, not build.
        assert_eq!(
            plan(false, NEVER),
            Delivery::MustFetch(cached_at(root, "aarch64", "1.0.0"))
        );
    }

    /// A released omh — no sources — fetches the published linux binary its own
    /// version publishes, rather than trying to cross-build and failing.
    #[test]
    fn a_release_with_no_sources_fetches_the_published_linux_binary() {
        let root = Path::new("/home/x/.omh");
        let table = [
            ("linux", true, ALWAYS, "host"),
            ("macos", true, ALWAYS, "cached"),
            ("macos", false, ALWAYS, "cached"),
            ("macos", true, NEVER, "build"),
            ("macos", false, NEVER, "fetch"),
        ];
        for (os, sources, exists, want) in table {
            let got = match plan_delivery(
                os,
                "aarch64",
                "1.0.0",
                Path::new("/bin/omh"),
                root,
                sources,
                exists,
            ) {
                Delivery::HostBinary(_) => "host",
                Delivery::Cached(_) => "cached",
                Delivery::MustBuild(_) => "build",
                Delivery::MustFetch(_) => "fetch",
            };
            assert_eq!(got, want, "os={os} sources={sources}");
        }
    }

    /// A cached binary from another version is not reused — a memory server
    /// from the last release is not this release's.
    #[test]
    fn a_cached_binary_from_another_version_is_not_reused() {
        let root = Path::new("/home/x/.omh");
        assert_ne!(
            cached_at(root, "aarch64", "1.0.0"),
            cached_at(root, "aarch64", "1.0.1")
        );
        // The cache present is 1.0.0's; this omh is 1.0.1, so it does not count.
        let here = cached_at(root, "aarch64", "1.0.1");
        assert_eq!(
            plan_delivery(
                "macos",
                "aarch64",
                "1.0.1",
                Path::new("/bin/omh"),
                root,
                false,
                &|p: &Path| { p == cached_at(root, "aarch64", "1.0.0") }
            ),
            Delivery::MustFetch(here),
        );
    }

    /// The tarball omh asks for is the one the release publishes: the musl
    /// target, and one SHA256SUMS for the release.
    #[test]
    fn the_tarball_asked_for_is_the_one_the_release_publishes() {
        let urls = fetch_urls("0.9.0", "x86_64");
        assert_eq!(urls.name, "omh-x86_64-unknown-linux-musl.tar.gz");
        assert!(
            urls.tarball
                .ends_with("/download/v0.9.0/omh-x86_64-unknown-linux-musl.tar.gz"),
            "{}",
            urls.tarball
        );
        assert!(
            urls.sums.ends_with("/download/v0.9.0/SHA256SUMS"),
            "{}",
            urls.sums
        );
        assert_eq!(
            fetch_urls("0.9.0", "aarch64").name,
            "omh-aarch64-unknown-linux-musl.tar.gz"
        );
    }

    /// A checksum for another platform never stands in for this one, and the
    /// parser tolerates the `./` and CRLF a SHA256SUMS may carry.
    #[test]
    fn a_checksum_for_another_platform_never_stands_in_for_this_one() {
        let a = "a".repeat(64);
        let b = "b".repeat(64);
        let sums = format!(
            "{a}  ./omh-aarch64-unknown-linux-musl.tar.gz\r\n{b}  omh-x86_64-unknown-linux-musl.tar.gz\n"
        );
        assert_eq!(
            sum_for(&sums, "omh-aarch64-unknown-linux-musl.tar.gz").as_deref(),
            Some(a.as_str())
        );
        assert_eq!(
            sum_for(&sums, "omh-x86_64-unknown-linux-musl.tar.gz").as_deref(),
            Some(b.as_str())
        );
        assert_eq!(
            sum_for(&sums, "omh-riscv64-unknown-linux-musl.tar.gz"),
            None
        );
        assert_eq!(
            sum_for("short  omh.tar.gz", "omh.tar.gz"),
            None,
            "a non-sha value is not a sum"
        );
    }

    /// A baked source path that happens to exist is not trusted because it
    /// exists: it has to be this crate, at this version.
    #[test]
    fn a_baked_source_path_is_not_trusted_because_it_exists() {
        let d = tempfile::tempdir().unwrap();
        assert_eq!(
            sources_at(d.path(), "1.0.0"),
            None,
            "no Cargo.toml, no sources"
        );

        std::fs::write(
            d.path().join("Cargo.toml"),
            "[package]\nname = \"somethingelse\"\nversion = \"1.0.0\"\n",
        )
        .unwrap();
        assert_eq!(
            sources_at(d.path(), "1.0.0"),
            None,
            "another crate is not omh"
        );

        std::fs::write(
            d.path().join("Cargo.toml"),
            "[package]\nname = \"omh\"\nversion = \"0.0.1\"\n",
        )
        .unwrap();
        assert_eq!(
            sources_at(d.path(), "1.0.0"),
            None,
            "omh at another version is not these sources"
        );

        std::fs::write(
            d.path().join("Cargo.toml"),
            "[package]\nname = \"omh\"\nversion = \"1.0.0\"\n",
        )
        .unwrap();
        assert_eq!(sources_at(d.path(), "1.0.0").as_deref(), Some(d.path()));
    }

    /// The real crate directory is these sources at this version.
    #[test]
    fn the_running_crate_is_recognised_as_its_own_sources() {
        assert_eq!(
            sources_at(
                Path::new(env!("CARGO_MANIFEST_DIR")),
                env!("CARGO_PKG_VERSION")
            )
            .as_deref(),
            Some(Path::new(env!("CARGO_MANIFEST_DIR")))
        );
    }

    /// An x86 binary mounted into an arm64 container fails with `exec format
    /// error`, which the harness reports as the MCP server crashing — a cause
    /// nobody would guess from the message. One cache path per architecture is
    /// what stops the two ever being confused.
    #[test]
    fn each_architecture_caches_to_its_own_path() {
        let root = Path::new("/home/x/.omh");
        assert_ne!(
            cached_at(root, "aarch64", "1.0.0"),
            cached_at(root, "x86_64", "1.0.0")
        );
        for arch in ["aarch64", "x86_64"] {
            assert!(
                cached_at(root, arch, "1.0.0")
                    .to_string_lossy()
                    .contains(arch),
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

    /// A fetch that could not run says so, and installs nothing. The failure
    /// is the fetch's, reported plainly, not a silent empty cache.
    #[test]
    fn an_offline_fetch_says_so_rather_than_saying_no_sources() {
        // Only meaningful where a fetch would be attempted at all.
        if std::env::consts::OS == "linux" {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let paths = crate::profile::Paths {
            root: dir.path().join("home"),
            repo: dir.path().join("repo"),
        };
        let fetch = |_: &Urls, _: &Path| bail!("could not resolve github.com");
        let err = ensure("docker", &paths, None, &fetch, &crate::out::Ctx::plain()).unwrap_err();
        let said = format!("{err:#}");
        assert!(
            said.contains("could not resolve"),
            "the fetch's own reason: {said}"
        );
        let arch = target_arch(std::env::consts::ARCH).unwrap();
        assert!(
            !cached_at(&paths.root, arch, env!("CARGO_PKG_VERSION")).exists(),
            "nothing is installed on a failed fetch"
        );
    }

    /// A download whose checksum does not match the published one is refused,
    /// not installed.
    #[test]
    fn a_download_that_fails_its_checksum_is_refused() {
        if std::env::consts::OS == "linux" {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let paths = crate::profile::Paths {
            root: dir.path().join("home"),
            repo: dir.path().join("repo"),
        };
        let arch = target_arch(std::env::consts::ARCH).unwrap();
        let fetch = move |urls: &Urls, dest: &Path| {
            std::fs::write(dest.join(&urls.name), b"not the real binary")?;
            // A well-formed SHA256SUMS, but for a different content.
            std::fs::write(
                dest.join("SHA256SUMS"),
                format!("{}  {}\n", "0".repeat(64), urls.name),
            )?;
            Ok(())
        };
        let err = ensure("docker", &paths, None, &fetch, &crate::out::Ctx::plain()).unwrap_err();
        assert!(
            format!("{err:#}").contains("does not match"),
            "got: {err:#}"
        );
        assert!(!cached_at(&paths.root, arch, env!("CARGO_PKG_VERSION")).exists());
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
