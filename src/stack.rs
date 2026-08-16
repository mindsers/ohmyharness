//! Stacks — what a project needs installed, as data rather than as Rust.
//!
//! A stack answers one question: *what does this project need in order to be
//! worked on?* If the agent has just changed something and wants to check it,
//! what tool does it reach for, and is that tool here?
//!
//! It is therefore not a set of hooks. Hooks are automation — when something
//! runs, on which events. Conflating the two produced the original wrong fix:
//! treating a missing compiler as a hook that should be suppressed, which hides
//! the symptom and leaves the environment as broken as it was. A human opening
//! a shell in that sandbox and typing `cargo test` gets the same error.
//!
//! These ship with omh — embedded at compile time by `build.rs`, refreshed into
//! `~/.omh/stacks` by every `init`, exactly as adapters and the base set are.
//! That is deliberate: a local edit fixing Elixir on one laptop leaves omh
//! broken for every other Elixir user, and removes the pressure that would have
//! produced a real fix. What moving them out of a `const` buys is a lower
//! barrier to *contributing* — a few lines of TOML rather than Rust — not a
//! lower barrier to diverging.
//!
//! Commands are the one thing still split. `detect::conventional` holds what
//! omh's conventional hooks run, in code, because a command belongs to a hook
//! and a `test =` key here would be a third copy of a string that already has
//! two homes. Until build-order item 7 ships those as catalogue hook files, the
//! two are tied together by `every_conventional_command_is_provisioned`, which
//! refuses a hook whose program no provide installs.

use anyhow::{Context, Result};
use serde::Deserialize;
use std::path::Path;

/// One ecosystem: how to tell a repo is one, and what such a repo needs.
///
/// No commands. A command belongs to a hook, and hooks already have two homes
/// with a defined precedence — `~/.omh/hooks/` for the ones you want
/// everywhere, `<repo>/.omh/hooks/` for the ones that belong to a project,
/// unioned by `render::merge_hooks` with the repo shadowing. A third copy here
/// would be the same string in a second place, free to disagree with the first.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Definition {
    pub name: String,
    /// The file whose presence says this repo is one of these.
    pub marker: String,
    #[serde(default, rename = "provide")]
    pub provides: Vec<Provide>,
}

/// One thing a stack puts in the image, and the case for it.
///
/// `needs` and `install` are deliberately separate fields. `install` is a
/// recipe; `needs` is the outcome to verify. That is not theoretical:
/// installing rustup produced a working `cargo` and still could not link
/// anything, because the image had no `cc`. The recipe succeeded and the
/// environment did not work. Kept paired *per provide*, the failure is
/// attributable — "the `linker` provide ran and `cc` still does not resolve" —
/// rather than a flat list of names with no idea which recipe owed them.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Provide {
    pub name: String,
    /// What must resolve in the sandbox once this has run. Never empty: a
    /// provide whose outcome is unstated is one nothing can verify.
    pub needs: Vec<String>,
    /// A shell predicate deciding whether this provide applies to this repo,
    /// evaluated **in the sandbox** with the repo mounted read-only. Absent
    /// means it always applies.
    #[serde(default)]
    pub when: Option<String>,
    /// How the image gets it. Absent means "the base image already ships
    /// this" — an assertion rather than a provision, which costs nothing to
    /// state and turns an unwritten assumption into a checked one.
    #[serde(default)]
    pub install: Option<String>,
    pub because: String,
    #[serde(default, rename = "measured")]
    pub measured: Vec<crate::base::Measured>,
}

/// Which of these stacks this repo is.
///
/// Marker presence and nothing cleverer. The finer question — *which variant*,
/// which package manager — is a provide's `when`, asked in the sandbox once a
/// stack is already in play. This one runs on the host on every launch, so it
/// stays a filename check: cheap enough that noticing a `package.json` that
/// appeared last week costs nothing.
///
/// Borrowed rather than cloned because the caller already owns the definitions
/// and a detected stack is a view of one, not a copy that could go stale.
pub fn detected<'a>(stacks: &'a [Definition], repo: &Path) -> Vec<&'a Definition> {
    stacks
        .iter()
        .filter(|s| repo.join(&s.marker).exists())
        .collect()
}

/// How a provide is named everywhere it is named: `[provision]` keys, the
/// probe's outcome names, the report. One speller, so those cannot drift.
///
/// Not the image tag — `image::stack_tag` hashes the recipe, which is what the
/// image actually contains, and two repos whose fired provides differ in name
/// but not in recipe should share a layer.
///
/// This spelling is part of a **committed file**, so it is pinned literally by
/// a test rather than only round-tripped: the opt-out somebody hand-writes is
/// a string in their `settings.toml`, and a speller that changed would leave
/// it matching nothing, in silence.
pub fn key(stack: &str, provide: &str) -> String {
    format!("{stack}/{provide}")
}

/// The `[provision]` table to write, given what this repo already recorded and
/// what just fired.
///
/// Three rules, and the asymmetry between them is the whole design:
///
/// - **omh writes only `true`.** It records what applied; it never records a
///   refusal, because a refusal it invented would be indistinguishable in a
///   committed file from one somebody made.
/// - **A `false` is never touched.** It can only have been typed, so it is a
///   decision, and re-running `init` is not consent to discard it.
/// - **A `true` that no longer applies is removed**, not kept and not flipped.
///   The table describes what is true now, which is what makes re-running
///   `init` the honest fix for a `yarn.lock` swapped for a `pnpm-lock.yaml`.
///
/// Takes the **shared layer's own table**, never the three-layer resolution. A
/// `false` in `settings.local.toml` is one laptop's decision; reading it here
/// would copy it into the committed file and export it to the team.
pub fn reconcile(
    shared: &std::collections::BTreeMap<String, bool>,
    fired: &std::collections::BTreeSet<String>,
) -> std::collections::BTreeMap<String, bool> {
    let mut out = std::collections::BTreeMap::new();
    for (key, on) in shared {
        if !on {
            out.insert(key.clone(), false);
        }
    }
    for key in fired {
        out.entry(key.clone()).or_insert(true);
    }
    out
}

/// What a predicate answered.
///
/// Three-valued over a mechanism that is two-valued: a shell command yields one
/// exit code, and *false* and *broken* both come back non-zero. Collapsing them
/// would make a `jq` choking on malformed JSON indistinguishable from a repo
/// that simply is not a pnpm project — and *cannot tell is never a licence to
/// act* is the rule the rest of this codebase runs on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    /// Exit 0. This provide applies here.
    Applies,
    /// Exit 1. It does not.
    DoesNot,
    /// Anything above 1 — the predicate could not answer, with its code when
    /// that could be read. Reported and **not fired**: a provide omh skipped
    /// because it could not tell surfaces as its `needs` not resolving, which is
    /// loud — `init` reports it and every hook that names the program is held
    /// back by name — whereas installing on a coin-flip is silent.
    CouldNotAnswer(Option<i32>),
}

/// Read a probe outcome as a verdict.
///
/// The code is carried in `detail` rather than in a wider `Outcome`, so the one
/// wire format and the one parser serve both this and `doctor`'s checks. The
/// emitter below and this reader are round-tripped by the tests, so they cannot
/// drift into disagreeing about where the number is.
pub fn verdict(o: &crate::doctor::Outcome) -> Verdict {
    if o.ok {
        return Verdict::Applies;
    }
    match o
        .detail
        .split_whitespace()
        .next()
        .and_then(|c| c.parse().ok())
    {
        Some(1) => Verdict::DoesNot,
        code => Verdict::CouldNotAnswer(code),
    }
}

/// A shell script asking, for each provide, whether it applies to this repo.
///
/// Emits the same `ok|fail\t<key>\t<detail>` wire format as every other probe,
/// so `doctor::parse` reads it and there is one format and one parser.
///
/// The exit code leads `detail` because that is the only channel a two-valued
/// `Outcome` leaves for a three-valued answer — see [`Verdict`]. Each predicate
/// is its own `if`, so one that dies takes only its own line: a `set -e` or a
/// chain would let the first broken predicate silence every provide after it,
/// and a truncated report read as a complete one is a defect this codebase has
/// already paid for twice — see `main::fired_from`, which refuses a report
/// shorter than the question it asked.
pub fn predicate_script(candidates: &[(String, Option<&str>)]) -> String {
    let mut out = String::from("#!/bin/sh\n");
    for (key, when) in candidates {
        let k = crate::doctor::single_quote(key);
        match when {
            // No condition is not a failed condition.
            None => out.push_str(&format!("printf 'ok\\t%s\\tapplies\\n' {k}\n")),
            // `( … )` — a subshell, and load-bearing rather than tidy. A bare
            // `if exit 2; then` terminates the *script*, so every predicate
            // after it produces no line at all, and a truncated report read as
            // a complete one is the failure this design has already been fixed
            // for once. In a subshell the `exit` ends only that predicate and
            // the `if` sees its code.
            // `>/dev/null 2>&1` on the predicate itself, not just the subshell
            // for `exit`. Predicate output and omh's protocol share one stream,
            // and `doctor::parse` takes a well-formed line from anywhere in it —
            // so a predicate written without a redirect (`grep packageManager
            // package.json`) prints **repo-controlled text** into the channel.
            // `init` runs against repos you have just cloned, and a forged
            // `applies` line reaches a committed table and an image recipe that
            // runs as root. Every shipped predicate redirects; this stops that
            // being a matter of discipline.
            Some(pred) => out.push_str(&format!(
                "if ( {pred} ) >/dev/null 2>&1; then printf 'ok\\t%s\\tapplies\\n' {k}; \
                 else c=$?; if [ \"$c\" -eq 1 ]; then \
                 printf 'fail\\t%s\\t1 does not apply\\n' {k}; else \
                 printf 'fail\\t%s\\t%s could not answer\\n' {k} \"$c\"; fi; fi\n"
            )),
        }
    }
    out
}

/// Arguments that evaluate predicates inside the sandbox, against the repo.
///
/// A **second** builder, deliberately not `image::probe_args`. That one is
/// mountless and a test asserts it, which is what stops a program probe
/// answering about the host. This one must see the checkout — so it takes the
/// mount `base::index_args` already established, for the reason recorded there:
/// *read-only, because a thing that reads code and can write into the checkout
/// is a sandbox hole for no benefit.*
///
/// Running them in the sandbox rather than on the host is the point. `install`
/// is arbitrary shell too, but it runs in a container as root and is contained
/// by construction; a predicate evaluated on the host would mean a stack file
/// executing shell on somebody's laptop during `init`.
pub fn predicate_args(tag: &str, repo: &Path, script: &str) -> Vec<String> {
    // Never the literal — `only_one_place_spells_the_container_workdir` counts
    // the spellings across the source, because asserting that two sides are
    // equal passes just as well when both hold the same hardcoded string.
    let workdir = crate::container_workdir();
    vec![
        "run".into(),
        "--rm".into(),
        "-v".into(),
        format!("{}:{workdir}:ro", repo.display()),
        "-w".into(),
        workdir.into(),
        tag.into(),
        "sh".into(),
        "-c".into(),
        script.into(),
    ]
}

/// What serde cannot say about a stack file.
///
/// Here rather than in the curation test, and the difference is the whole
/// point: that test reads `CARGO_MANIFEST_DIR/stacks`, so it proves things
/// about this source tree. `load_dir` is what reads `~/.omh/stacks`, and a
/// rule enforced only on the four files in this repo is not a rule about
/// stacks — it is a rule about these four files.
fn validate(def: &Definition, path: &Path) -> Result<()> {
    let at = path.display();

    // A name is half a `[provision]` key, and `key` joins the halves with `/`.
    // A stack `a/b` with provide `c` and a stack `a` with provide `b/c` mint one
    // key, so one person's `false` switches off somebody else's install.
    anyhow::ensure!(!def.name.trim().is_empty(), "{at}: the stack has no name");
    anyhow::ensure!(
        !def.name.contains('/'),
        "{at}: stack name `{}` contains `/`, which is what separates a stack \
         from a provide in a `[provision]` key",
        def.name
    );

    // `Path::join` **discards the base** when handed an absolute path, so an
    // absolute marker does not look inside the repo at all — it matches every
    // repository on the machine, each of which then runs this stack's `install`
    // as root. `..` climbs out of the checkout, and a blank marker joins to the
    // repo root, which always exists.
    //
    // Shipped stack files are reviewed; build-order item 6 puts one inside a
    // repo you have just cloned, so this closes before that opens.
    let marker = Path::new(&def.marker);
    anyhow::ensure!(
        !def.marker.trim().is_empty(),
        "{at}: the stack has no marker, and an empty one matches every repo"
    );
    anyhow::ensure!(
        marker.components().count() == 1
            && marker
                .components()
                .all(|c| matches!(c, std::path::Component::Normal(_))),
        "{at}: marker `{}` must be one filename inside the repo — an absolute \
         path matches every repo on this machine, and `..` leaves the checkout",
        def.marker
    );

    let mut seen = std::collections::BTreeSet::new();
    for p in &def.provides {
        anyhow::ensure!(
            !p.name.trim().is_empty(),
            "{at}: a provide has no name, so it cannot be keyed or reported"
        );
        anyhow::ensure!(
            !p.name.contains('/'),
            "{at}: provide name `{}` contains `/`",
            p.name
        );
        // Two provides of one name are one `[provision]` key and two installs,
        // and they destroy the attribution the `needs`/`install` pairing exists
        // for: "the `linker` provide ran and `cc` still does not resolve".
        anyhow::ensure!(
            seen.insert(p.name.as_str()),
            "{at}: two provides are called `{}`",
            p.name
        );
        anyhow::ensure!(
            !p.because.trim().is_empty(),
            "{at}: provide `{}` states no case — `omh why` has nothing to read",
            p.name
        );
        // Every entry here is handed to `command -v` by the sandbox probe. A
        // blank one, or one carrying arguments, resolves nowhere — so it reports a gap for a
        // toolchain the user has, and keeps reporting it. `detect::program`
        // returns `None` rather than guess for exactly this reason; a stack
        // file is the other door into the same mistake.
        anyhow::ensure!(
            !p.needs.is_empty(),
            "{at}: provide `{}` needs nothing, so nothing can verify it ran",
            p.name
        );
        for need in &p.needs {
            anyhow::ensure!(
                !need.trim().is_empty(),
                "{at}: provide `{}` has a blank `needs` entry",
                p.name
            );
            // Through `detect::program` rather than a local re-spelling of the
            // rule. That function is the codebase's answer to "is this word a
            // program name", it uses an allowlist for reasons its own tests
            // record, and a second implementation here accepted `$(which`,
            // `cargo|tee` and a leading space — the exact shapes it refuses.
            anyhow::ensure!(
                crate::detect::program(need) == Some(need.as_str()),
                "{at}: provide `{}` needs `{need}`, which is not a program name \
                 — `needs` is what must resolve on PATH, not a command to run",
                p.name
            );
        }
    }
    Ok(())
}

/// Every stack in a directory.
///
/// The `Adapter::load_dir` shape, not `Manifest::load_dir`'s: a directory of
/// stacks is a set, and a polyglot repo is genuinely several of them at once.
/// A missing directory is no stacks rather than an error, because a fresh
/// install has not seeded one yet and that is not a reason to refuse to work.
pub fn load_dir(dir: &Path) -> Result<Vec<Definition>> {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e).with_context(|| format!("reading {}", dir.display())),
    };

    let mut out = Vec::new();
    for entry in entries {
        let path = entry
            .with_context(|| format!("reading {}", dir.display()))?
            .path();
        // `.toml` and nothing else: a `.yours` backup is somebody's replaced
        // edit, kept on purpose by `install_bundled`, and reading it as a stack
        // would turn a saved file into a parse error on every command.
        if path.extension().is_none_or(|e| e != "toml") {
            continue;
        }
        let raw = std::fs::read_to_string(&path)
            .with_context(|| format!("reading {}", path.display()))?;
        let def: Definition =
            toml::from_str(&raw).with_context(|| format!("parsing {}", path.display()))?;
        validate(&def, &path)?;
        out.push(def);
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir_with(files: &[(&str, &str)]) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        for (name, body) in files {
            std::fs::write(dir.path().join(name), body).unwrap();
        }
        dir
    }

    const MINIMAL: &str = r#"
name   = "rust"
marker = "Cargo.toml"

[[provide]]
name    = "toolchain"
needs   = ["cargo"]
because = "cargo is how a rust project is built and tested"
"#;

    fn shipped() -> Vec<Definition> {
        load_dir(Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/stacks")))
            .expect("the shipped stacks must load")
    }

    /// Every provide states its case, and any cost it claims is one somebody
    /// could have taken.
    ///
    /// Deliberately **weaker** than `every_base_set_entry_states_its_case` in
    /// one place and exactly as strong in another. `measured` is optional here,
    /// because a stack can be contributed by somebody who works in that
    /// ecosystem and has no way to run an image build — demanding a number
    /// would either block the contribution or invite an invented one, and the
    /// second is worse.
    ///
    /// What is *not* relaxed: a measurement that is present must be true. The
    /// base set shipped fabricated dates once — every `on` read `2026-08-04`,
    /// one day before this repository existed, typed rather than taken — and
    /// the rule that catches that is shared with the base set rather than
    /// spelled again here.
    #[test]
    fn every_provide_states_its_case() {
        let stacks = shipped();
        assert!(!stacks.is_empty(), "omh ships no stacks at all");

        for s in &stacks {
            assert!(!s.marker.trim().is_empty(), "{}: no marker", s.name);
            assert!(
                !s.provides.is_empty(),
                "{}: provides nothing, so detecting it does nothing",
                s.name
            );
            for p in &s.provides {
                let label = format!("{}/{}", s.name, p.name);
                assert!(!p.because.trim().is_empty(), "{label}: no `because`");
                // Without this there is nothing for the probe to check, so the
                // provide's claim to have worked can never be tested — which is
                // the difference between an environment and a hope.
                assert!(
                    !p.needs.is_empty(),
                    "{label}: needs nothing, so nothing can verify it ran"
                );
                crate::base::assert_measured_states_its_case(&label, &p.measured);
            }
        }
    }

    /// A stack is one whose marker is on disk — and it is that stack and not a
    /// neighbour.
    ///
    /// Iterated over every shipped definition rather than over this repo's own
    /// `Cargo.toml`: a rust-shaped implementation passes a rust-only guard, and
    /// three quarters of what detection does would go unexercised. That is not
    /// hypothetical — the comment `detect::KNOWN` carried said exactly this
    /// about the guard it replaced.
    #[test]
    fn a_repo_is_the_stack_whose_marker_it_holds() {
        let stacks = shipped();
        for s in &stacks {
            let d = tempfile::tempdir().unwrap();
            std::fs::write(d.path().join(&s.marker), "").unwrap();

            let found: Vec<&str> = detected(&stacks, d.path())
                .iter()
                .map(|f| f.name.as_str())
                .collect();
            assert_eq!(
                found,
                [s.name.as_str()],
                "a repo holding only {} is {} and nothing else",
                s.marker,
                s.name
            );
        }
    }

    /// Guessing a stack writes hooks that fail on every turn. Detecting nothing
    /// is the correct outcome for a repo omh does not recognise.
    #[test]
    fn no_marker_is_no_stack_rather_than_a_guess() {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join("README.md"), "hello").unwrap();
        assert!(detected(&shipped(), d.path()).is_empty());
    }

    /// A polyglot repo is genuinely several stacks at once — which is why
    /// `load_dir` returns a set rather than picking a winner.
    #[test]
    fn a_repo_can_be_more_than_one_stack() {
        let stacks = shipped();
        let d = tempfile::tempdir().unwrap();
        for s in stacks.iter().take(2) {
            std::fs::write(d.path().join(&s.marker), "").unwrap();
        }
        assert_eq!(detected(&stacks, d.path()).len(), 2);
    }

    /// A marker claimed twice is two stacks fighting over one repo, and which
    /// wins would come down to filename order.
    #[test]
    fn no_two_shipped_stacks_claim_the_same_name_or_marker() {
        let stacks = shipped();
        for (i, a) in stacks.iter().enumerate() {
            for b in &stacks[i + 1..] {
                assert_ne!(a.name, b.name, "two stacks called {}", a.name);
                assert_ne!(
                    a.marker, b.marker,
                    "{} and {} both claim {}",
                    a.name, b.name, a.marker
                );
            }
        }
    }

    // ── recording the resolution ────────────────────────────────────────────

    fn shared(entries: &[(&str, bool)]) -> std::collections::BTreeMap<String, bool> {
        entries.iter().map(|(k, v)| (k.to_string(), *v)).collect()
    }

    fn fired(keys: &[&str]) -> std::collections::BTreeSet<String> {
        keys.iter().map(|k| k.to_string()).collect()
    }

    /// The one spelling, asserted literally — because it is a **committed file
    /// format**, not an internal detail.
    ///
    /// Every other test here computes both sides through `key`, so the suite
    /// stays self-consistent under any spelling: `"{provide}/{stack}"` and
    /// `"{stack}:{provide}"` both pass. But somebody who writes
    /// `"rust/linker" = false` into `.omh/settings.toml` — the opt-out
    /// `[provision]` exists for — has a literal in a file omh does not own.
    /// Change the speller and that line stops matching anything and is
    /// silently ignored: the provide is installed anyway, exit 0, no
    /// diagnostic. `docs/configuration.md` and every repo already using it
    /// spell it this way.
    #[test]
    fn a_provision_key_is_stack_slash_provide() {
        assert_eq!(key("rust", "linker"), "rust/linker");
    }

    /// **Only a person writes `false`.** omh records what applied, so a `false`
    /// can only have been typed — it is a decision, and re-running `init` is not
    /// consent to discard it. The predicate may say pnpm applies every time;
    /// somebody wrote down that this repo supplies it another way.
    #[test]
    fn a_recorded_false_survives_re_resolution() {
        let out = reconcile(&shared(&[("node/pnpm", false)]), &fired(&["node/pnpm"]));
        assert_eq!(out.get("node/pnpm"), Some(&false));
    }

    #[test]
    fn a_newly_fired_provide_is_recorded_true() {
        let out = reconcile(&shared(&[]), &fired(&["rust/toolchain"]));
        assert_eq!(out.get("rust/toolchain"), Some(&true));
    }

    /// The resolution describes what is true **now**, so a provide that stopped
    /// applying loses its entry rather than keeping a stale `true`.
    ///
    /// This is what makes the drift story honest: swap a `yarn.lock` for a
    /// `pnpm-lock.yaml`, re-run `init`, and the yarn entry goes. Left behind, it
    /// would keep yarn in the image for ever and the file would describe a repo
    /// that no longer exists.
    #[test]
    fn a_provide_that_stopped_applying_loses_its_entry() {
        let out = reconcile(&shared(&[("node/yarn", true)]), &fired(&["node/pnpm"]));
        assert_eq!(
            out.get("node/yarn"),
            None,
            "the stale entry is gone: {out:?}"
        );
        assert_eq!(out.get("node/pnpm"), Some(&true));
    }

    /// A provide that did not fire is not written at all — not `false`, which
    /// would be omh recording a decision nobody made, in a committed file,
    /// where it then looks exactly like one somebody did make.
    #[test]
    fn nothing_is_invented_for_a_provide_that_never_fired() {
        let out = reconcile(&shared(&[]), &fired(&[]));
        assert!(out.is_empty(), "invented: {out:?}");
    }

    // ── predicates ──────────────────────────────────────────────────────────

    fn ask(candidates: &[(&str, Option<&str>)], cwd: &Path) -> Vec<(String, Verdict)> {
        let owned: Vec<(String, Option<&str>)> = candidates
            .iter()
            .map(|(k, w)| ((*k).to_string(), *w))
            .collect();
        let out = crate::doctor::run_probe_in(&predicate_script(&owned), cwd);
        crate::doctor::parse(&out)
            .iter()
            .map(|o| (o.name.clone(), verdict(o)))
            .collect()
    }

    /// Exit zero applies, exit one does not. The two ordinary answers, run
    /// through a real `/bin/sh` and parsed back through the shared wire format
    /// — because a predicate is a program, and asserting on the script's text
    /// would prove only that it mentions the right words.
    #[test]
    fn exit_zero_applies_and_exit_one_does_not() {
        let d = tempfile::tempdir().unwrap();
        let got = ask(&[("x/a", Some("true")), ("x/b", Some("false"))], d.path());

        assert_eq!(
            got,
            vec![
                ("x/a".to_string(), Verdict::Applies),
                ("x/b".to_string(), Verdict::DoesNot),
            ]
        );
    }

    /// The third answer, and the one a shell cannot give directly: a command
    /// yields **one** exit code, and "false" and "broken" both come back
    /// non-zero. Reading anything above 1 as *could not answer* is what lets a
    /// two-valued mechanism carry the three-valued rule the rest of omh runs
    /// on — *cannot tell is never a licence to act*.
    ///
    /// The code travels with the verdict so a stack author can fix their
    /// predicate; a bare "did not apply" would send them looking at the repo.
    #[test]
    fn an_exit_above_one_could_not_answer_and_says_with_what_code() {
        let d = tempfile::tempdir().unwrap();
        let got = ask(
            &[("x/misuse", Some("exit 2")), ("x/odd", Some("exit 7"))],
            d.path(),
        );

        assert_eq!(
            got,
            vec![
                ("x/misuse".to_string(), Verdict::CouldNotAnswer(Some(2))),
                ("x/odd".to_string(), Verdict::CouldNotAnswer(Some(7))),
            ]
        );
    }

    /// A predicate that ends the shell must end only itself.
    ///
    /// Found by writing the test above with `exit 2` and getting **no output at
    /// all**: a bare `if exit 2; then` terminates the script, so every provide
    /// after it goes unanswered. That is a truncated report read as a complete
    /// one — the failure `main::fired_from` refuses at the other end of this
    /// same wire — arriving through a stack file rather than through a dying
    /// container.
    #[test]
    fn a_predicate_that_ends_the_shell_does_not_silence_the_rest() {
        let d = tempfile::tempdir().unwrap();
        let got = ask(
            &[("x/dies", Some("exit 3")), ("x/after", Some("true"))],
            d.path(),
        );

        assert_eq!(
            got,
            vec![
                ("x/dies".to_string(), Verdict::CouldNotAnswer(Some(3))),
                ("x/after".to_string(), Verdict::Applies),
            ]
        );
    }

    /// No condition is not a failed condition. A provide that always applies —
    /// `rust/toolchain`, every apt recipe — must not need a `when = "true"`
    /// incantation to say so.
    #[test]
    fn a_provide_with_no_condition_applies() {
        let d = tempfile::tempdir().unwrap();
        let got = ask(&[("x/always", None)], d.path());
        assert_eq!(got, vec![("x/always".to_string(), Verdict::Applies)]);
    }

    /// Predicates run against the repo, so they are written relative to it.
    #[test]
    fn a_predicate_reads_the_repo_it_runs_in() {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join("pnpm-lock.yaml"), "").unwrap();
        let got = ask(
            &[
                ("node/pnpm", Some("test -f pnpm-lock.yaml")),
                ("node/yarn", Some("test -f yarn.lock")),
            ],
            d.path(),
        );

        assert_eq!(got[0].1, Verdict::Applies, "the lockfile is here");
        assert_eq!(got[1].1, Verdict::DoesNot, "and this one is not");
    }

    /// Every shipped predicate, against a repo that is that stack and a repo
    /// that is empty — asserting only that each gives one of the three answers
    /// and writes nothing. What it *should* answer for a given repo is the
    /// stack author's business; that it answers at all, in the protocol, is
    /// omh's.
    #[test]
    fn every_shipped_predicate_answers_in_the_protocol() {
        for def in shipped() {
            let candidates: Vec<(String, Option<&str>)> = def
                .provides
                .iter()
                .map(|p| (key(&def.name, &p.name), p.when.as_deref()))
                .collect();
            if candidates.is_empty() {
                continue;
            }

            for populated in [false, true] {
                let d = tempfile::tempdir().unwrap();
                if populated {
                    std::fs::write(d.path().join(&def.marker), "{}").unwrap();
                }
                let before = std::fs::read_dir(d.path()).unwrap().flatten().count();

                let out = crate::doctor::run_probe_in(&predicate_script(&candidates), d.path());
                let answered = crate::doctor::parse(&out);
                assert_eq!(
                    answered.len(),
                    candidates.len(),
                    "{} answered {} of {} predicates: {out}",
                    def.name,
                    answered.len(),
                    candidates.len()
                );

                let after = std::fs::read_dir(d.path()).unwrap().flatten().count();
                assert_eq!(
                    before, after,
                    "{}'s predicates wrote into the repo — they are mounted \
                     read-only in the sandbox, so this would fail there instead",
                    def.name
                );
            }
        }
    }

    /// Predicates must see the checkout, and must not be able to change it.
    ///
    /// The counterpart to `image::probe_args`' mountless guard, and the reason
    /// these are two builders rather than one with a flag: a program probe that
    /// could see the host would answer about the wrong machine, and a predicate
    /// that could not see the repo could not answer at all. Each has an
    /// invariant the other would violate.
    ///
    /// Read-only for the reason `base::index_args` already records: something
    /// that reads code and can write into the checkout is a sandbox hole for no
    /// benefit.
    #[test]
    fn predicates_see_the_repo_and_can_only_read_it() {
        let args = predicate_args("omh/x:latest", Path::new("/host/wt"), "#!/bin/sh\ntrue\n");

        let mounts: Vec<&String> = args
            .iter()
            .zip(args.iter().skip(1))
            .filter(|(f, _)| *f == "-v")
            .map(|(_, spec)| spec)
            .collect();
        assert_eq!(mounts.len(), 1, "exactly one mount: {args:?}");
        assert!(
            mounts[0].starts_with("/host/wt:") && mounts[0].ends_with(":ro"),
            "and it is the repo, read-only: {}",
            mounts[0]
        );
        assert!(
            args.windows(2)
                .any(|w| w[0] == "-w" && w[1] == crate::container_workdir()),
            "a predicate written `test -f pnpm-lock.yaml` needs the repo as its \
             working directory: {args:?}"
        );
        assert!(args.contains(&"--rm".to_string()), "{args:?}");
        assert_eq!(args.last().map(String::as_str), Some("#!/bin/sh\ntrue\n"));
    }

    /// A predicate's own output must never reach the report channel.
    ///
    /// Predicate stdout and omh's protocol share one stream, and `doctor::parse`
    /// accepts a well-formed line from anywhere in it. So a predicate written
    /// without a redirect — `grep packageManager package.json`, no `-q` — prints
    /// **repo-controlled text** into the protocol. `omh init` runs against
    /// repositories you have just cloned and not read, and a fabricated
    /// `applies` line is written into a committed `[provision]` table and into
    /// an image recipe that runs as root.
    ///
    /// The shipped predicates all redirect, but that is discipline; this makes
    /// it structural. `a_hostile_key_cannot_corrupt_the_run` covers a hostile
    /// key from a stack file — this covers hostile content from the repo.
    #[test]
    fn a_predicate_that_prints_cannot_fabricate_a_verdict() {
        let d = tempfile::tempdir().unwrap();
        // A predicate that echoes repo content, and a repo whose content is a
        // forged protocol line for a provide nobody asked about.
        let forged = "ok\trust/toolchain\tapplies";
        std::fs::write(d.path().join("package.json"), forged).unwrap();

        let got = ask(&[("node/pnpm", Some("cat package.json"))], d.path());

        assert_eq!(
            got.len(),
            1,
            "the repo's content became a verdict of its own: {got:?}"
        );
        assert_eq!(
            got[0].0, "node/pnpm",
            "and it is the one omh asked: {got:?}"
        );
    }

    /// A key reaches the script from a stack file, so it is not omh's to trust.
    /// Same rule, same reason, and the same shape of assertion as
    /// `doctor::a_program_name_with_a_quote_cannot_corrupt_the_probe`: the key
    /// comes back exactly as it went in, which no expansion survives.
    #[test]
    fn a_hostile_key_cannot_corrupt_the_run() {
        let d = tempfile::tempdir().unwrap();
        let hostile = "x/$(echo pwned)";
        let owned = vec![
            (hostile.to_string(), Some("true")),
            ("x/after".to_string(), Some("true")),
        ];
        let out = crate::doctor::run_probe_in(&predicate_script(&owned), d.path());

        assert!(
            !out.lines().any(|l| l.trim() == "pwned"),
            "a key was expanded as shell: {out}"
        );
        let answered = crate::doctor::parse(&out);
        assert!(
            answered.iter().any(|o| o.name == hostile),
            "the key came back changed: {answered:?}"
        );
        assert!(
            answered.iter().any(|o| o.name == "x/after"),
            "and one hostile key must not cost the rest: {answered:?}"
        );
    }

    /// A marker is one filename inside the repo, and nothing else.
    ///
    /// `Path::join` **discards the base when given an absolute path**, so
    /// `marker = "/etc/hostname"` does not look inside the repo — it matches
    /// every repository on the machine, and every one of them then runs that
    /// stack's `install` as root in an image build. `..` escapes the checkout
    /// the same way, and a blank marker joins to the repo root itself, which
    /// always exists.
    ///
    /// Today these files ship with omh and are reviewed. Build-order item 6
    /// puts one inside `<repo>/.omh/stacks`, in a repository you have just
    /// cloned and not read — so this closes before the milestone that opens it,
    /// not after.
    #[test]
    fn a_marker_that_is_not_one_filename_inside_the_repo_is_refused() {
        for (marker, why) in [
            ("/etc/hostname", "absolute — `join` throws the repo away"),
            ("../../etc/hostname", "climbs out of the checkout"),
            ("", "joins to the repo root, which always exists"),
            ("a/b", "is a path rather than a marker"),
        ] {
            let body = MINIMAL.replace("Cargo.toml", marker);
            let d = dir_with(&[("rust.toml", &body)]);
            let Err(e) = load_dir(d.path()) else {
                panic!("accepted a marker that {why}: {marker:?}");
            };
            let err = format!("{e:#}");
            assert!(err.contains("rust.toml"), "must name the file: {err}");
        }
    }

    /// A name is half a `[provision]` key, and `key` joins the two with `/`.
    /// A stack `a/b` with provide `c` and a stack `a` with provide `b/c` mint
    /// the same key — so one person's `false` silently switches off somebody
    /// else's install, and `reconcile` cannot express one without the other.
    #[test]
    fn a_name_that_would_collide_in_a_provision_key_is_refused() {
        for (field, value) in [
            ("name   = \"rust\"", "name   = \"ru/st\""),
            ("name   = \"rust\"", "name   = \"\""),
        ] {
            let body = MINIMAL.replacen(field, value, 1);
            let d = dir_with(&[("rust.toml", &body)]);
            assert!(
                load_dir(d.path()).is_err(),
                "accepted a stack name that cannot key a provide: {value}"
            );
        }
    }

    /// Two provides of one name are one `[provision]` key and two installs.
    /// The whole case for pairing `needs` with `install` per provide is
    /// attribution — *"the `linker` provide ran and `cc` still does not
    /// resolve"* — and two `toolchain`s make that sentence ambiguous.
    #[test]
    fn two_provides_cannot_share_a_name() {
        let body = format!(
            "{MINIMAL}\n[[provide]]\nname    = \"toolchain\"\nneeds   = [\"rustc\"]\nbecause = \"again\"\n"
        );
        let d = dir_with(&[("rust.toml", &body)]);
        let Err(e) = load_dir(d.path()) else {
            panic!("accepted two provides called `toolchain`");
        };
        assert!(format!("{e:#}").contains("toolchain"), "must name it");
    }

    /// A `needs` entry is a program name — what the sandbox probe looks for
    /// with `command -v`.
    /// A blank one, or one carrying arguments, resolves nowhere — so it reports
    /// a permanent gap for a toolchain the user has, which is the expensive
    /// failure direction and the one `detect::program` returns `None` to avoid.
    ///
    /// Checked in `load_dir` rather than in the curation test, because the
    /// curation test only ever reads this source tree. Once `~/.omh/stacks`
    /// exists and can be edited, `load_dir` is the only thing standing between
    /// a typo and a sandbox that reports a missing compiler for ever.
    ///
    /// The bad provide is always the **second** one, and where the list can
    /// hold two, the bad entry is the second entry. A fixture with one provide
    /// carrying one need cannot tell a validator that checks everything from
    /// one that checks only the first of each — and `stacks/node.toml` ships
    /// four provides, `stacks/rust.toml` a provide with four needs, so the
    /// difference is the difference between a guard and a decoration.
    #[test]
    fn a_needs_entry_that_is_not_a_program_name_is_refused() {
        for (needs, why) in [
            (r#"[""]"#, "blank"),
            (r#"["cargo test"]"#, "carries arguments"),
            ("[]", "empty, so nothing can verify the provide"),
            (r#"["cc", ""]"#, "blank, in second position"),
            (
                r#"["cc", "cargo test"]"#,
                "carries arguments, in second position",
            ),
        ] {
            let body = format!(
                "{MINIMAL}\n[[provide]]\nname    = \"linker\"\nneeds   = {needs}\n\
                 because = \"rustc emits objects and something has to link them\"\n"
            );
            let d = dir_with(&[("rust.toml", &body)]);
            let Err(e) = load_dir(d.path()) else {
                panic!("a {why} `needs` was accepted: {body}");
            };
            let err = format!("{e:#}");
            assert!(err.contains("rust.toml"), "must name the file: {err}");
            assert!(
                err.contains("linker"),
                "and the provide, so the fix is findable: {err}"
            );
        }
    }

    /// Stacks are a **set**, not a versioned document. `Manifest::load_dir`
    /// picks a single winner by version and is right to — there is one base
    /// set. There are many stacks, and a repo may be more than one of them, so
    /// this follows `Adapter::load_dir` instead: every file, sorted, all of
    /// them real.
    #[test]
    fn every_file_in_the_directory_is_a_stack() {
        let d = dir_with(&[
            ("zebra.toml", &MINIMAL.replace("rust", "zebra")),
            ("alpha.toml", &MINIMAL.replace("rust", "alpha")),
        ]);
        let found = load_dir(d.path()).unwrap();

        let names: Vec<&str> = found.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(
            names,
            ["alpha", "zebra"],
            "every file, in a stable order — a stack directory is not a contest"
        );
    }

    /// A fresh install, or somebody who deleted the directory. Adapters answer
    /// this with an empty list rather than an error, and a repo with no stacks
    /// is a repo omh can still set up.
    #[test]
    fn a_missing_directory_is_no_stacks_rather_than_an_error() {
        let d = tempfile::tempdir().unwrap();
        let found = load_dir(&d.path().join("nothing-here")).unwrap();
        assert!(found.is_empty(), "got {found:?}");
    }

    /// A misspelled key must not be silently a stack that provisions nothing.
    /// The failure it would otherwise cause is invisible until somebody's
    /// sandbox is missing a compiler, which is the whole failure this module
    /// exists to end. `Adapter` and `Manifest` both deny unknown fields for the
    /// same reason.
    ///
    /// The stray key is **added** rather than substituted for `marker`.
    /// Renaming one reads the same on the surface and tests something else
    /// entirely: serde answers `missing field `marker`` first, which satisfies
    /// every assertion below while `deny_unknown_fields` is never reached —
    /// deleting the attribute left this green. Keeping the file otherwise
    /// valid means only the unknown key can refuse it.
    #[test]
    fn a_key_omh_does_not_understand_is_refused_by_name() {
        let d = dir_with(&[("rust.toml", &format!("mark = \"Cargo.toml\"\n{MINIMAL}"))]);
        let err = format!("{:#}", load_dir(d.path()).unwrap_err());

        assert!(err.contains("mark"), "must name the key: {err}");
        assert!(err.contains("rust.toml"), "and the file: {err}");
    }

    /// Anything that is not a `.toml` is somebody's editor swap file, or a
    /// `.yours` backup `install_bundled` wrote when it replaced a managed file.
    /// Reading one as a stack would turn a saved edit into a parse error on
    /// every command.
    #[test]
    fn only_toml_files_are_read() {
        let d = dir_with(&[
            ("rust.toml", MINIMAL),
            ("rust.toml.yours", "this is not toml at all {{{"),
            ("notes.md", "nor is this"),
        ]);
        let found = load_dir(d.path()).unwrap();
        assert_eq!(found.len(), 1, "got {found:?}");
    }
}
