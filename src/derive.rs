//! How *this* project spells its commands, derived from what it already
//! commits.
//!
//! The catalogue holds one hook per ecosystem — `cargo test` is what a rust
//! project runs. That answer is right for rust and wrong for node, where the
//! test command depends on which package manager the project uses and whether
//! it declared a `test` script at all. `npm test` in a repo with no such script
//! is a hook that fails on every turn, which is the failure this whole design
//! exists to remove.
//!
//! So for the ecosystems the catalogue cannot answer for, omh reads the files
//! the project already commits — a lockfile, a `package.json`, a `Makefile` —
//! and writes a hook into `<repo>/.omh/hooks/`. That is tier 1 of
//! `docs/design/adoption.md`'s table: knowable from the repo, on the host, for
//! free.
//!
//! ## This module executes nothing
//!
//! Not a subprocess, not a shell, not `make -qp`. A stack's `install` and
//! `when` run in a container precisely so a definition cannot execute on
//! somebody's laptop; a derivation that shelled out during `init` would put the
//! thing omh exists to avoid back into the one command everybody runs. Every
//! answer here comes from reading a file.
//!
//! ## Cannot tell is never a licence to act
//!
//! Every reader returns nothing rather than a guess: two lockfiles, a
//! `Taskfile` using a YAML feature the scanner does not understand, a
//! `package.json` that will not parse. A repo that gets no hook is a repo
//! somebody writes one for. A repo that gets the *wrong* hook is one where
//! every turn ends in a red mark nobody can explain, and the hook omh invented
//! is the last place they will look.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

/// A node package manager, which is a property of the **project** rather than
/// of the developer: the lockfile is committed, so the whole team runs the same
/// one or the lockfile is worthless.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Manager {
    Npm,
    Pnpm,
    Yarn,
    Bun,
}

impl Manager {
    /// Its name, which is also the program a hook needs on PATH.
    pub fn program(self) -> &'static str {
        match self {
            Self::Npm => "npm",
            Self::Pnpm => "pnpm",
            Self::Yarn => "yarn",
            Self::Bun => "bun",
        }
    }

    /// The lockfiles that identify it. More than one for bun, which changed
    /// format: `bun.lockb` is the binary lockfile, `bun.lock` the text one it
    /// gained later, and a repo may commit either.
    fn lockfiles(self) -> &'static [&'static str] {
        match self {
            Self::Npm => &["package-lock.json"],
            Self::Pnpm => &["pnpm-lock.yaml"],
            Self::Yarn => &["yarn.lock"],
            Self::Bun => &["bun.lock", "bun.lockb"],
        }
    }

    /// How it runs a script the project declared.
    ///
    /// **Always `run`.** `npm test` and `pnpm test` work without it, so the
    /// short form is tempting — and `bun test` is a different command
    /// altogether: bun's own test runner, which ignores `scripts.test`
    /// entirely and goes looking for `*.test.ts` files. A project whose `test`
    /// script runs vitest would silently get bun's runner instead, find
    /// nothing or find the wrong thing, and report success either way.
    ///
    /// One spelling that is correct for all four beats four spellings and a
    /// footnote.
    pub fn run(self, script: &str) -> String {
        format!("{} run {script}", self.program())
    }

    const ALL: [Manager; 4] = [Self::Npm, Self::Pnpm, Self::Yarn, Self::Bun];
}

/// Which package manager this repo uses, or `None` if nothing here says.
///
/// Three sources, in this order, and the order is the whole of it:
///
/// 1. **What the image was provisioned with.** `[provision]` records what
///    `init` resolved and the stack layer was built from, so it is the only
///    source that describes the sandbox the hook will run in. A hook spelled
///    for a manager the image does not have is the one outcome that is worse
///    than no hook.
/// 2. **A lockfile.** It records what was actually *used*, and it is committed,
///    so it is the team's answer rather than one laptop's.
/// 3. **`packageManager`.** A declaration of intent, which may be aspirational
///    — somebody adds the field and the lockfile stays as it was.
///
/// **Two lockfiles is `None`**, not a winner picked by order. A repo with both
/// a `yarn.lock` and a `pnpm-lock.yaml` is mid-migration or broken, and either
/// way omh does not know which one the team runs. Guessing produces a hook that
/// installs the wrong tree.
pub fn manager(repo: &Path, provision: &BTreeMap<String, bool>) -> Option<Manager> {
    // Only `true`. A `false` says *do not install this*, and reading an opt-out
    // as a selection would spell every hook for the one manager the image was
    // deliberately built without.
    let provisioned: Vec<Manager> = Manager::ALL
        .into_iter()
        .filter(|m| provision.get(&crate::stack::key("node", m.program())) == Some(&true))
        .collect();
    if let [only] = provisioned[..] {
        return Some(only);
    }
    // More than one provisioned is not a tie to break here: the repo's own
    // evidence decides between them below, and if that cannot either, nothing
    // does.

    let locked: Vec<Manager> = Manager::ALL
        .into_iter()
        .filter(|m| m.lockfiles().iter().any(|f| repo.join(f).exists()))
        .collect();
    match locked[..] {
        [only] => return Some(only),
        // Two lockfiles: the repo contains evidence of two managers having
        // actually been run, and no declaration outranks that. Unless exactly
        // one of them is in the image, which the block above already answered.
        [_, ..] => return None,
        [] => {}
    }

    // Nothing was run here yet, so intent is all there is — and it is a real
    // answer, since corepack reads exactly this field to decide what to
    // install.
    let raw = std::fs::read_to_string(repo.join("package.json")).ok()?;
    let parsed: serde_json::Value = serde_json::from_str(&raw).ok()?;
    let declared = parsed.get("packageManager")?.as_str()?;
    // `pnpm@9.0.0` — the version is corepack's business, not a hook's.
    let name = declared.split('@').next().unwrap_or_default();
    Manager::ALL.into_iter().find(|m| m.program() == name)
}

/// The scripts a `package.json` declares.
///
/// Empty for a repo with none, one that will not parse, or one that is not
/// node. **A script that is not declared produces no hook** — `npm run test`
/// against a project without a `test` script exits non-zero every turn, which
/// is a red mark that teaches people to ignore red marks.
pub fn scripts(repo: &Path) -> BTreeSet<String> {
    let Ok(raw) = std::fs::read_to_string(repo.join("package.json")) else {
        return BTreeSet::new();
    };
    let Ok(parsed) = serde_json::from_str::<serde_json::Value>(&raw) else {
        return BTreeSet::new();
    };
    parsed
        .get("scripts")
        .and_then(serde_json::Value::as_object)
        .map(|s| s.keys().cloned().collect())
        .unwrap_or_default()
}

/// A task runner the project drives its own commands through.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Runner {
    Make,
    Just,
    Task,
}

impl Runner {
    pub fn program(self) -> &'static str {
        match self {
            Self::Make => "make",
            Self::Just => "just",
            Self::Task => "task",
        }
    }

    /// The filenames it answers to, in the order it looks for them.
    fn files(self) -> &'static [&'static str] {
        match self {
            Self::Make => &["Makefile", "makefile", "GNUmakefile"],
            Self::Just => &["justfile", "Justfile", ".justfile"],
            Self::Task => &["Taskfile.yml", "Taskfile.yaml"],
        }
    }

    pub fn run(self, target: &str) -> String {
        format!("{} {target}", self.program())
    }

    const ALL: [Runner; 3] = [Self::Make, Self::Just, Self::Task];
}

/// The runner this repo drives itself through, and the targets it declares.
///
/// `None` for no runner, more than one runner, or a file this module cannot
/// read **confidently**. The last is the interesting case and the reason these
/// scanners are hand-rolled: a `Taskfile` using YAML anchors or `includes:`
/// describes tasks that are not in the text, so a scanner that reported only
/// what it could see would be confidently incomplete. Reporting nothing is the
/// honest answer, and it costs a hook somebody can write by hand.
///
/// Two runners is the same cannot-tell as two lockfiles: a repo with a
/// `Makefile` and a `justfile` has an answer omh cannot read off the disk.
pub fn runner(repo: &Path) -> Option<(Runner, BTreeSet<String>)> {
    let mut found = Vec::new();
    for which in Runner::ALL {
        let Some(path) = which
            .files()
            .iter()
            .map(|f| repo.join(f))
            .find(|p| p.exists())
        else {
            continue;
        };
        let Ok(body) = std::fs::read_to_string(&path) else {
            // Unreadable is cannot-tell about *this* runner, and a repo that
            // also has another one is then unambiguous — which would be the
            // wrong conclusion, so it counts as present with no targets.
            found.push((which, BTreeSet::new()));
            continue;
        };
        found.push((which, which.targets(&body)));
    }
    match found.into_iter().collect::<Vec<_>>()[..] {
        [(which, ref targets)] if !targets.is_empty() => Some((which, targets.clone())),
        _ => None,
    }
}

impl Runner {
    /// The target names this file declares, as far as this can tell.
    fn targets(self, body: &str) -> BTreeSet<String> {
        match self {
            Self::Make | Self::Just => rule_heads(body),
            Self::Task => taskfile_tasks(body).unwrap_or_default(),
        }
    }
}

/// Target names from a make- or just-shaped file.
///
/// Both put a rule's name at the start of a line, followed by a colon, with the
/// recipe indented under it. That shared shape is why one reader serves both.
///
/// What is deliberately *not* a target:
///
/// - an **indented** line, which is a recipe body
/// - `.PHONY`, `.DEFAULT_GOAL` and friends — make's own directives, which start
///   with a dot and would otherwise become hooks running `make .PHONY`
/// - a **variable assignment**: `CARGO := cargo` and `X ::= y` both contain a
///   colon at the top level, and `make CARGO` is not a thing
/// - a name that is not a plain word — `$(BINS):` is a target list computed at
///   run time, and omh cannot read what it expands to
fn rule_heads(body: &str) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    for line in body.lines() {
        if line.starts_with([' ', '\t']) || line.trim_start().starts_with('#') {
            continue;
        }
        let Some((heads, _)) = line.split_once(':') else {
            continue;
        };
        // `:=`, `::=` and `?=` are assignments; so is a bare `=` before any
        // colon, which `split_once` would not have reached.
        //
        // Indexed past the colon, not to it: `line[heads.len()..]` starts *at*
        // the separator, so it begins with `:` for every line in the file and
        // the check discarded all of them.
        let after = &line[heads.len() + 1..];
        // An assignment whose *value* contains a colon — `PATH_LIST = a:b` —
        // reaches here looking like a rule. It is told apart by the `=` sitting
        // where a second target name would be, which a just recipe's parameter
        // default (`build target="debug":`) never does: `heads.contains('=')`
        // cannot tell those two apart and refused every parameterised recipe.
        let mut words = heads.split_whitespace();
        let assignment = words.next().is_some_and(|w| w.ends_with('='))
            || words.next().is_some_and(|w| w.starts_with('='));
        if after.starts_with([':', '=']) || assignment || heads.trim().is_empty() {
            continue;
        }
        for head in heads.split_whitespace() {
            // just's recipes take parameters — `build target="debug":` — and
            // the name is the first word. Stopping at the first non-name word
            // covers that without knowing anything about just's grammar.
            if is_target_name(head) {
                out.insert(head.to_string());
            } else {
                break;
            }
        }
    }
    out
}

fn is_target_name(word: &str) -> bool {
    !word.is_empty()
        && word.starts_with(|c: char| c.is_ascii_alphanumeric() || c == '_')
        && word
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.' | '/'))
}

/// Task names from a `Taskfile.yml`, or `None` for a shape this cannot read.
///
/// Hand-rolled rather than a YAML dependency, in a tree that deliberately has
/// four. The trade that makes that honest is refusing loudly: this reads
/// exactly one shape — a block-mapping `tasks:` with each task a key indented
/// under it — and answers `None` for anything else.
///
/// The three refusals all mean *the tasks are not all in this text*:
///
/// - **`includes:`** pulls in another Taskfile entirely
/// - an **anchor, alias or merge key** (`&base`, `*base`, `<<:`) assembles a
///   task from somewhere else in the document
/// - a **flow mapping** (`tasks: {…}`) is a shape this does not parse at all
///
/// A partial answer would be worse than none: the hook omh wrote would run a
/// task whose real definition it never saw.
fn taskfile_tasks(body: &str) -> Option<BTreeSet<String>> {
    for line in body.lines() {
        let t = line.trim_start();
        if t.starts_with("includes:") || t.starts_with("<<:") {
            return None;
        }
        // An anchor or an alias, as a word rather than as any `&`/`*` — those
        // appear inside ordinary shell commands constantly (`a && b`, `rm *`).
        if t.split_whitespace()
            .any(|w| (w.starts_with('&') || w.starts_with('*')) && w.len() > 1)
        {
            return None;
        }
    }

    let mut out = BTreeSet::new();
    let mut inside = None;
    for line in body.lines() {
        if line.trim_end() == "tasks:" {
            inside = Some(());
            continue;
        }
        // `tasks:` with anything after it is a flow mapping or a reference, and
        // either way not the block this reads.
        if line.starts_with("tasks:") && line.trim_end() != "tasks:" {
            return None;
        }
        if inside.is_none() {
            continue;
        }
        if !line.starts_with([' ', '\t']) {
            if line.trim().is_empty() {
                continue;
            }
            break; // back to the top level: the `tasks:` block has ended.
        }
        let depth = line.len() - line.trim_start().len();
        let t = line.trim();
        // Exactly one level in, and a key. Deeper is a task's own body.
        if depth <= 2 && !t.starts_with('#') {
            if let Some((name, rest)) = t.split_once(':') {
                if rest.trim().is_empty() && is_target_name(name) {
                    out.insert(name.to_string());
                }
            }
        }
    }
    Some(out)
}

/// A hook omh worked out from the repo's own files, ready to be written into
/// `<repo>/.omh/hooks/`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Derived {
    pub name: String,
    pub hook: crate::hook::Hook,
    /// The file that justified it. `omh why` has to be able to say where every
    /// default came from, and "omh guessed" is not an answer.
    pub from: String,
}

/// The two moments omh derives for, and the script or target names that mean
/// them.
///
/// A closed list on purpose. Every name here is a convention strong enough that
/// a project using it means what omh thinks it means; `build`, `start` and
/// `dev` are not — running `npm run build` at the end of every turn is a
/// minute of wasted compute and a red mark whenever the tree is mid-edit.
const MOMENTS: [(crate::hook::Event, &str, &[&str]); 2] = [
    (crate::hook::Event::TurnEnd, "test", &["test"]),
    (crate::hook::Event::AfterTool, "format", &["format", "fmt"]),
];

/// What this repo's own files justify, for the ecosystems nothing already
/// covers.
///
/// **Derivation fills gaps; it never competes.** `covered` is the set of
/// ecosystem names the catalogue already ships hooks for — `{"rust", "go",
/// "python"}` as omh stands. A rust repo therefore derives nothing from its
/// `Makefile`, because `rust-test` already runs its suite and deriving anyway
/// would run it twice at the end of every turn.
///
/// Keyed on the **ecosystem**, not on the moment, and the difference is not
/// subtle: omh's own `graph-refresh` runs at turn-end in every repo, so a
/// moment-shaped notion of coverage would mark turn-end covered everywhere and
/// derive nothing, ever, for anybody.
///
/// Node is the case this exists for. The catalogue ships no node hook because
/// `npm test` is only a real command if the project declared a `test` script,
/// and which manager runs it is a property of the repo rather than of the
/// ecosystem. A polyglot repo that is both rust and node correctly gets both
/// hooks: two suites, two commands, and running one does not run the other.
///
/// A **runner** answers for the whole project rather than for an ecosystem, so
/// it applies only where no ecosystem hook does — a C project with a
/// `Makefile`, not a rust project that also has one.
///
/// **A runner outranks a script.** A `Makefile` with a `test` target in a repo
/// that also has `scripts.test` is somebody having written the wrapper on
/// purpose, and the wrapper is the entry point they maintain.
pub fn hooks(
    repo: &Path,
    provision: &BTreeMap<String, bool>,
    covered: &BTreeSet<String>,
) -> Vec<Derived> {
    let mut out = Vec::new();
    let runner = runner(repo).filter(|_| covered.is_empty());
    let manager = manager(repo, provision).filter(|_| !covered.contains("node"));
    let scripts = scripts(repo);

    for (on, moment, names) in MOMENTS {
        let found = runner
            .as_ref()
            .and_then(|(which, targets)| {
                names
                    .iter()
                    .find(|n| targets.contains(**n))
                    .map(|n| (which.program(), which.run(n), None))
            })
            .or_else(|| {
                let m = manager?;
                let n = names.iter().find(|n| scripts.contains(**n))?;
                Some((m.program(), m.run(n), Some("node")))
            });
        let Some((source, command, stack)) = found else {
            continue;
        };
        out.push(Derived {
            name: format!("{source}-{moment}"),
            hook: crate::hook::Hook {
                on,
                stack: stack.map(str::to_string),
                // Formatting belongs to the edit that made it necessary;
                // testing belongs to the end of the turn, when there is
                // something whole to test.
                tools: match on {
                    crate::hook::Event::AfterTool => vec![crate::hook::Tool::Edit],
                    _ => Vec::new(),
                },
                when: None,
                action: crate::hook::Action::Run(command),
            },
            from: source_of(&runner, manager, moment),
        });
    }
    out
}

fn source_of(
    runner: &Option<(Runner, BTreeSet<String>)>,
    manager: Option<Manager>,
    moment: &str,
) -> String {
    match (runner, manager) {
        (Some((which, _)), _) => format!("a `{moment}` target in this repo's {:?}", which),
        (None, Some(m)) => format!(
            "the `{moment}` script in package.json, run with {}",
            m.program()
        ),
        (None, None) => "this repo".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repo(files: &[(&str, &str)]) -> (tempfile::TempDir, std::path::PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("repo");
        std::fs::create_dir_all(&root).unwrap();
        for (name, body) in files {
            let p = root.join(name);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, body).unwrap();
        }
        (dir, root)
    }

    fn provisioned(keys: &[&str]) -> BTreeMap<String, bool> {
        keys.iter().map(|k| ((*k).to_string(), true)).collect()
    }

    /// A lockfile is committed and records what was actually run, so it is the
    /// team's answer about the project rather than one developer's habit.
    #[test]
    fn a_lockfile_names_the_package_manager() {
        for (lock, want) in [
            ("pnpm-lock.yaml", Manager::Pnpm),
            ("yarn.lock", Manager::Yarn),
            ("bun.lock", Manager::Bun),
            ("bun.lockb", Manager::Bun),
            ("package-lock.json", Manager::Npm),
        ] {
            let (_d, r) = repo(&[("package.json", "{}"), (lock, "")]);
            assert_eq!(manager(&r, &BTreeMap::new()), Some(want), "for {lock}");
        }
    }

    /// The lockfile beats the declaration, because one records what happened
    /// and the other records what somebody meant. A `packageManager: "pnpm@9"`
    /// added to a repo whose `yarn.lock` is still there is aspiration; the
    /// yarn.lock is what `install` actually reads.
    #[test]
    fn a_lockfile_beats_a_declaration() {
        let (_d, r) = repo(&[
            ("package.json", r#"{"packageManager":"pnpm@9.0.0"}"#),
            ("yarn.lock", ""),
        ]);
        assert_eq!(manager(&r, &BTreeMap::new()), Some(Manager::Yarn));
    }

    /// With no lockfile, the declaration is all there is — and it is a real
    /// answer, since corepack reads exactly this field.
    #[test]
    fn a_declaration_answers_when_no_lockfile_does() {
        for (field, want) in [
            ("pnpm@9.0.0", Manager::Pnpm),
            ("yarn@4.1.0", Manager::Yarn),
            ("bun@1.1.0", Manager::Bun),
            ("npm@10.5.0", Manager::Npm),
        ] {
            let (_d, r) = repo(&[(
                "package.json",
                &format!(r#"{{"packageManager":"{field}"}}"#),
            )]);
            assert_eq!(manager(&r, &BTreeMap::new()), Some(want), "for {field}");
        }
    }

    /// **Two lockfiles is cannot-tell**, and cannot-tell writes no hook.
    ///
    /// A repo with both is mid-migration or broken. Picking by order would give
    /// a stable, confident, wrong answer — and the hook it produced would
    /// install a second dependency tree on every turn.
    #[test]
    fn two_lockfiles_answer_nothing() {
        let (_d, r) = repo(&[
            ("package.json", "{}"),
            ("yarn.lock", ""),
            ("pnpm-lock.yaml", ""),
        ]);
        assert_eq!(manager(&r, &BTreeMap::new()), None);

        // And not even a `packageManager` breaks the tie: it says what somebody
        // intends, and what makes this ambiguous is that the repo contains
        // evidence of two things having actually been run.
        let (_d, r) = repo(&[
            ("package.json", r#"{"packageManager":"pnpm@9.0.0"}"#),
            ("yarn.lock", ""),
            ("pnpm-lock.yaml", ""),
        ]);
        assert_eq!(manager(&r, &BTreeMap::new()), None);
    }

    /// **What the image was built with outranks both**, because it is the only
    /// source that describes the machine the hook will run on.
    ///
    /// `[provision]` is what `init` resolved and `image::stack_tag` keyed the
    /// layer on, so it is not a fourth opinion about the repo — it is the
    /// sandbox. A hook spelled `yarn run test` against an image that
    /// provisioned pnpm fails on every turn with `yarn: not found`, which is
    /// the exact failure this design opens by describing.
    #[test]
    fn what_the_image_has_outranks_what_the_repo_says() {
        let (_d, r) = repo(&[("package.json", "{}"), ("yarn.lock", "")]);
        assert_eq!(
            manager(&r, &provisioned(&["node/pnpm"])),
            Some(Manager::Pnpm),
            "the sandbox has pnpm, so a yarn hook could not run in it"
        );

        // Including where the repo alone could not have answered at all.
        let (_d, r) = repo(&[("package.json", "{}"), ("yarn.lock", ""), ("bun.lock", "")]);
        assert_eq!(
            manager(&r, &provisioned(&["node/bun"])),
            Some(Manager::Bun),
            "two lockfiles, but only one of them is in the image"
        );
    }

    /// An opt-out is not a selection. `[provision] "node/pnpm" = false` says
    /// *do not install this*, and reading it as *use this* would spell every
    /// hook for the one manager the image deliberately lacks.
    #[test]
    fn a_provision_opt_out_never_names_the_manager() {
        let (_d, r) = repo(&[("package.json", "{}"), ("yarn.lock", "")]);
        let declined: BTreeMap<String, bool> =
            [("node/pnpm".to_string(), false)].into_iter().collect();
        assert_eq!(manager(&r, &declined), Some(Manager::Yarn));
    }

    /// A repo that is not node at all, and one whose `package.json` will not
    /// parse. Both are cannot-tell; neither is npm-by-default.
    #[test]
    fn nothing_here_is_nothing_rather_than_a_default() {
        let (_d, r) = repo(&[("README.md", "hello")]);
        assert_eq!(manager(&r, &BTreeMap::new()), None);

        let (_d, r) = repo(&[("package.json", "{ this is not json")]);
        assert_eq!(
            manager(&r, &BTreeMap::new()),
            None,
            "a file omh cannot read says nothing, and nothing is not npm"
        );
    }

    /// `bun test` is bun's own test runner and ignores `scripts.test`
    /// completely — it goes looking for `*.test.ts` instead. A project whose
    /// test script runs vitest would get bun's runner, find nothing, and pass.
    ///
    /// `run` is correct for all four, so there is one rule rather than three
    /// plus an exception somebody has to remember.
    #[test]
    fn a_script_is_always_run_never_invoked_directly() {
        for m in Manager::ALL {
            let spelled = m.run("test");
            assert_eq!(
                spelled,
                format!("{} run test", m.program()),
                "every manager runs a script the same way"
            );
        }
        assert_eq!(
            Manager::Bun.run("test"),
            "bun run test",
            "`bun test` would silently ignore the project's own test script"
        );
    }

    // ── task runners ────────────────────────────────────────────────────────

    /// The commonest shape: a target at the start of a line, its recipe
    /// indented under it.
    #[test]
    fn a_makefile_declares_its_targets() {
        let (_d, r) = repo(&[(
            "Makefile",
            ".PHONY: test fmt\n\
             CARGO := cargo\n\
             test:\n\
             \t$(CARGO) test\n\
             fmt lint:\n\
             \t$(CARGO) fmt\n",
        )]);
        let (which, targets) = runner(&r).expect("a Makefile is a runner");
        assert_eq!(which, Runner::Make);
        assert_eq!(
            targets,
            ["test", "fmt", "lint"]
                .map(String::from)
                .into_iter()
                .collect(),
            "two targets on one line are two targets; `.PHONY` is not one, and \
             neither is a `:=` assignment"
        );
        assert_eq!(which.run("test"), "make test");
    }

    /// just's recipes take parameters, and the name is the first word.
    #[test]
    fn a_justfile_declares_its_recipes() {
        let (_d, r) = repo(&[(
            "justfile",
            "# a comment\n\
             export RUST_LOG := \"debug\"\n\
             test:\n\
             \tcargo test\n\
             build target=\"debug\":\n\
             \tcargo build\n",
        )]);
        let (which, targets) = runner(&r).expect("a justfile is a runner");
        assert_eq!(which, Runner::Just);
        assert_eq!(
            targets,
            ["test", "build"].map(String::from).into_iter().collect()
        );
        assert_eq!(which.run("build"), "just build");
    }

    /// The Taskfile scanner reads exactly one shape and refuses everything
    /// else, which is the trade that lets it exist without a YAML parser in a
    /// dependency tree that deliberately has four.
    #[test]
    fn a_taskfile_declares_its_tasks() {
        let (_d, r) = repo(&[(
            "Taskfile.yml",
            "version: '3'\n\
             \n\
             tasks:\n\
             \x20 test:\n\
             \x20   cmds:\n\
             \x20     - go test ./...\n\
             \x20 lint:\n\
             \x20   cmds:\n\
             \x20     - golangci-lint run\n",
        )]);
        let (which, targets) = runner(&r).expect("a Taskfile is a runner");
        assert_eq!(which, Runner::Task);
        assert_eq!(
            targets,
            ["test", "lint"].map(String::from).into_iter().collect()
        );
    }

    /// **A YAML feature the scanner does not understand means no answer**, not
    /// a partial one.
    ///
    /// An `includes:` pulls in tasks that are not in this text at all; an
    /// anchor or a merge key means a task's real shape is assembled from
    /// somewhere else. A scanner that reported the tasks it could see would be
    /// confidently incomplete — and the failure would be a hook running a task
    /// that turned out to be overridden.
    #[test]
    fn a_taskfile_this_cannot_read_confidently_answers_nothing() {
        for (why, body) in [
            (
                "includes pull in tasks that are not in this file",
                "version: '3'\nincludes:\n  docs: ./docs/Taskfile.yml\ntasks:\n  test:\n    cmds:\n      - echo\n",
            ),
            (
                "an anchor means a task is assembled elsewhere",
                "version: '3'\nx-base: &base\n  silent: true\ntasks:\n  test:\n    <<: *base\n    cmds:\n      - echo\n",
            ),
            (
                "a flow mapping is a shape this does not read",
                "version: '3'\ntasks: {test: {cmds: [echo]}}\n",
            ),
        ] {
            let (_d, r) = repo(&[("Taskfile.yml", body)]);
            assert_eq!(runner(&r), None, "{why}");
        }
    }

    /// A variable whose **value** contains a colon looks exactly like a rule,
    /// and a just recipe's parameter default contains an `=` exactly like an
    /// assignment. Telling them apart is what the `=`'s *position* is for.
    ///
    /// Found by reasoning rather than by a failure, so it is pinned here: the
    /// obvious spelling — reject any head containing `=` — refused every
    /// parameterised just recipe, and the obvious fix for that accepts every
    /// path-valued make variable as a target called `PATH_LIST`.
    #[test]
    fn an_assignment_is_not_a_target_however_its_value_is_spelled() {
        let (_d, r) = repo(&[(
            "Makefile",
            "PATH_LIST = src:tests\n\
             OTHER= a:b\n\
             real:\n\
             \techo\n",
        )]);
        assert_eq!(
            runner(&r).expect("a Makefile is a runner").1,
            ["real"].map(String::from).into_iter().collect(),
            "a colon in a variable's value does not make it a rule"
        );
    }

    // ── what a repo justifies ───────────────────────────────────────────────

    fn ran(d: &[Derived], name: &str) -> Option<String> {
        d.iter()
            .find(|h| h.name == name)
            .map(|h| match &h.hook.action {
                crate::hook::Action::Run(c) => c.clone(),
                other => panic!("a derived hook runs a command, not {other:?}"),
            })
    }

    /// The case this module exists for. The catalogue ships no node hook,
    /// because which manager runs the script is the repo's business and
    /// whether there *is* a script is too.
    #[test]
    fn a_node_repo_gets_the_command_its_own_files_describe() {
        let (_d, r) = repo(&[
            (
                "package.json",
                r#"{"scripts":{"test":"vitest run","fmt":"prettier -w ."}}"#,
            ),
            ("pnpm-lock.yaml", ""),
        ]);
        let got = hooks(&r, &BTreeMap::new(), &BTreeSet::new());

        assert_eq!(ran(&got, "pnpm-test").as_deref(), Some("pnpm run test"));
        assert_eq!(ran(&got, "pnpm-format").as_deref(), Some("pnpm run fmt"));
        let test = got.iter().find(|h| h.name == "pnpm-test").unwrap();
        assert_eq!(test.hook.on, crate::hook::Event::TurnEnd);
        assert_eq!(
            test.hook.stack.as_deref(),
            Some("node"),
            "so the drift report notices when the package.json goes"
        );
        assert!(
            !test.from.is_empty(),
            "`omh why` has to be able to say where this came from"
        );

        let fmt = got.iter().find(|h| h.name == "pnpm-format").unwrap();
        assert_eq!(fmt.hook.on, crate::hook::Event::AfterTool);
        assert_eq!(
            fmt.hook.tools,
            vec![crate::hook::Tool::Edit],
            "formatting belongs to the edit that made it necessary"
        );
    }

    /// **A script that is not declared produces no hook**, restated where it
    /// bites: `npm run test` in a project with no `test` script fails every
    /// turn, and omh would have written that hook itself.
    #[test]
    fn an_undeclared_script_produces_no_hook() {
        let (_d, r) = repo(&[
            ("package.json", r#"{"scripts":{"build":"tsc"}}"#),
            ("pnpm-lock.yaml", ""),
        ]);
        assert_eq!(
            hooks(&r, &BTreeMap::new(), &BTreeSet::new()),
            Vec::new(),
            "a project with only a `build` script gets nothing — `build` is not \
             a moment omh has an opinion about"
        );
    }

    /// **Derivation fills gaps; it never competes.** A rust repo is already
    /// covered by the catalogue, so its `Makefile` produces nothing —
    /// otherwise every turn would run the suite twice.
    #[test]
    fn nothing_is_derived_for_an_ecosystem_already_covered() {
        let (_d, r) = repo(&[("Makefile", "test:\n\techo\nfmt:\n\techo\n")]);
        let rust = ["rust".to_string()].into_iter().collect();
        assert_eq!(
            hooks(&r, &BTreeMap::new(), &rust),
            Vec::new(),
            "a runner answers for the whole project, so it applies only where \
             no ecosystem hook does"
        );

        // A project no ecosystem hook covers is exactly what a runner is for.
        let got = hooks(&r, &BTreeMap::new(), &BTreeSet::new());
        assert_eq!(ran(&got, "make-test").as_deref(), Some("make test"));
        assert_eq!(ran(&got, "make-format").as_deref(), Some("make fmt"));
    }

    /// **No text from the repo reaches the command.** A script name is matched
    /// against omh's own list and the *list's* spelling is what gets written.
    ///
    /// This is what makes deriving a hook from somebody's `package.json` safe
    /// at all. A hook body is a shell command by construction, so a derivation
    /// that interpolated a repo-supplied key would let a `package.json` choose
    /// what runs at the end of every turn — a hook the user never wrote, in a
    /// file omh put there, attributed to omh. Cloning a repo would be enough.
    ///
    /// The same holds for a runner's targets, which are read out of a
    /// `Makefile` with no more validation than a name pattern.
    #[test]
    fn no_text_from_the_repo_reaches_a_derived_command() {
        // Every command omh could possibly derive: a program it names, a
        // moment it names, and nothing else. Asserted as membership of this
        // closed set rather than as the absence of one payload — a denylist
        // says nothing about the syntax it failed to think of, and this is
        // `detect::is_program_name`'s argument one file over.
        let vocabulary: BTreeSet<String> = MOMENTS
            .iter()
            .flat_map(|(_, _, names)| names.iter())
            .flat_map(|n| {
                Manager::ALL
                    .into_iter()
                    .map(|m| m.run(n))
                    .chain(Runner::ALL.into_iter().map(|w| w.run(n)))
            })
            .collect();

        // The hostile keys sort *before* the real one, so an implementation
        // that used the repo's spelling picks them rather than being saved by
        // `BTreeSet` ordering — which is what made the first version of this
        // test pass against exactly the mutation it was written for.
        let hostile = r#"{"scripts":{"test":"vitest run",
            "a\"; rm -rf $HOME; #":"x", "a-test":"y"}}"#;
        let (_d, r) = repo(&[("package.json", hostile), ("pnpm-lock.yaml", "")]);
        let node = hooks(&r, &BTreeMap::new(), &BTreeSet::new());
        assert_eq!(ran(&node, "pnpm-test").as_deref(), Some("pnpm run test"));

        // And the same through a Makefile, whose targets are read out of the
        // repo with no more validation than a name pattern.
        let (_d, r) = repo(&[(
            "Makefile",
            "a\"; rm -rf $HOME:\n\techo\na-test:\n\techo\ntest:\n\techo\n",
        )]);
        let make = hooks(&r, &BTreeMap::new(), &BTreeSet::new());
        assert_eq!(ran(&make, "make-test").as_deref(), Some("make test"));

        for d in node.iter().chain(make.iter()) {
            let crate::hook::Action::Run(command) = &d.hook.action else {
                panic!("a derived hook runs a command");
            };
            assert!(
                vocabulary.contains(command),
                "a command omh could not have spelled itself: {command}"
            );
        }
    }

    /// What is written is what the launcher reads back.
    ///
    /// `Hook` serialises through `Raw`, and `init` writes these files with
    /// `serde_json`, never `format!`. A hand-built JSON string was how the
    /// stack hooks used to be written, and its own test comment admitted a
    /// command containing a quote would produce a file nothing could read.
    #[test]
    fn a_derived_hook_survives_being_written_and_read_back() {
        let (_d, r) = repo(&[
            (
                "package.json",
                r#"{"scripts":{"test":"vitest run","fmt":"prettier -w ."}}"#,
            ),
            ("pnpm-lock.yaml", ""),
        ]);
        for d in hooks(&r, &BTreeMap::new(), &BTreeSet::new()) {
            let written = serde_json::to_string_pretty(&d.hook).unwrap();
            assert_eq!(
                crate::hook::Hook::parse(&written, &d.name).expect("must parse back"),
                d.hook,
                "what init writes is what the launcher reads"
            );
        }
    }

    /// Coverage applies to the **scripts** path too, not only to runners.
    ///
    /// Node is uncovered today because the catalogue ships no node hook — but
    /// that is a fact about the catalogue, not a property of this module. The
    /// day somebody contributes `hooks/node-test.json`, derivation has to stop
    /// or every node repo runs its suite twice; and the filter that does it
    /// survived being deleted, because every test here had an empty `covered`
    /// or a `covered` naming something else.
    #[test]
    fn an_ecosystem_the_catalogue_covers_derives_nothing_from_its_scripts() {
        let (_d, r) = repo(&[
            ("package.json", r#"{"scripts":{"test":"vitest run"}}"#),
            ("pnpm-lock.yaml", ""),
        ]);
        let node = ["node".to_string()].into_iter().collect();
        assert_eq!(
            hooks(&r, &BTreeMap::new(), &node),
            Vec::new(),
            "a catalogue hook for node would already run this suite"
        );
    }

    /// **Coverage is by ecosystem, never by moment**, and that is not a detail.
    ///
    /// omh's own `graph-refresh` runs at turn-end in *every* repo. A
    /// moment-shaped notion of coverage would therefore mark turn-end covered
    /// everywhere and derive a test hook for nobody, ever — silently, since an
    /// empty list is also what a repo with nothing to derive gets.
    #[test]
    fn a_hook_at_the_same_moment_does_not_cover_an_ecosystem() {
        let (_d, r) = repo(&[
            ("package.json", r#"{"scripts":{"test":"vitest run"}}"#),
            ("pnpm-lock.yaml", ""),
        ]);
        // rust is covered; node is not, and both would run at turn-end.
        let rust = ["rust".to_string()].into_iter().collect();
        let got = hooks(&r, &BTreeMap::new(), &rust);
        assert_eq!(
            ran(&got, "pnpm-test").as_deref(),
            Some("pnpm run test"),
            "a polyglot repo runs both suites: {got:?}"
        );
    }

    /// **A runner outranks a script.** Somebody who wrote a `Makefile` wrapping
    /// their own `npm test` maintains the wrapper, and that is the entry point
    /// they expect to be used.
    #[test]
    fn a_runner_outranks_a_script() {
        let (_d, r) = repo(&[
            ("package.json", r#"{"scripts":{"test":"vitest run"}}"#),
            ("pnpm-lock.yaml", ""),
            ("Makefile", "test:\n\tpnpm run test --reporter dot\n"),
        ]);
        let got = hooks(&r, &BTreeMap::new(), &BTreeSet::new());
        assert_eq!(ran(&got, "make-test").as_deref(), Some("make test"));
        assert!(
            ran(&got, "pnpm-test").is_none(),
            "one moment, one hook: {got:?}"
        );
    }

    /// A manager omh cannot name produces no hook, however many scripts the
    /// project declares. There is no spelling of `run test` that works without
    /// knowing which program to put in front of it.
    #[test]
    fn a_script_with_no_manager_to_run_it_produces_no_hook() {
        let (_d, r) = repo(&[
            ("package.json", r#"{"scripts":{"test":"vitest run"}}"#),
            ("yarn.lock", ""),
            ("pnpm-lock.yaml", ""),
        ]);
        assert_eq!(hooks(&r, &BTreeMap::new(), &BTreeSet::new()), Vec::new());
    }

    /// Two runners is the same cannot-tell as two lockfiles: the repo has an
    /// answer and omh cannot read which one off the disk.
    #[test]
    fn two_runners_answer_nothing() {
        let (_d, r) = repo(&[
            ("Makefile", "test:\n\techo\n"),
            ("justfile", "test:\n\techo\n"),
        ]);
        assert_eq!(runner(&r), None);
    }

    /// A runner file with nothing omh recognises in it is not a runner. An
    /// empty target list would otherwise read as "this repo has a Makefile",
    /// which is true and useless — every caller wants a target.
    #[test]
    fn a_runner_with_no_readable_targets_is_no_runner() {
        let (_d, r) = repo(&[("Makefile", "CARGO := cargo\n# nothing here\n")]);
        assert_eq!(runner(&r), None);
    }
}
