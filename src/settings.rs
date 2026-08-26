//! `[omh]` — which of omh's own features are on here.
//!
//! One table for now. `[omh]` names **features**, never entries: `codegraph`
//! is the server, its four hooks and its section of the rules, and switching
//! half of it off produces a graph that quietly stops tracking the code. That
//! state is unrepresentable rather than warned about, which is what removed the
//! guard, the manifest field and the launch warning an earlier design needed.
//!
//! Disabling is not removal. A feature off here leaves your `mcp.json` exactly
//! as you have it; the server is dropped from the document this session is
//! given, and the next repo gets it back.
//!
//! Everything else in these files — `carry_in`, `idle_timeout`, and `[use]`
//! when it lands — is read by [`crate::config::policy`], which resolves the
//! same three paths with provenance. Two readers of one file rather than two
//! files: a setting and a feature switch are both something a repo decided, and
//! `policy.toml` was a fourth name for that idea living inside a directory whose
//! purpose was content.

use crate::base::Manifest;
use crate::profile::Paths;
use anyhow::{Context, Result};
use serde::Deserialize;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

/// Deliberately not `deny_unknown_fields`: this file holds settings too, and
/// `config::policy` is what reads them. Denying here would make a `carry_in`
/// beside `[omh]` an error in one reader and a value in the other.
///
/// That argument covers *scalars* and stops there. `[omh]` and `[mcp]` are the
/// complete set of tables either reader understands, so an unrecognised one is
/// read by nobody and reported by nothing — which is why `rest` is collected
/// and checked rather than ignored.
#[derive(Debug, Default, Deserialize)]
struct File {
    #[serde(default)]
    omh: BTreeMap<String, bool>,
    #[serde(default)]
    mcp: BTreeMap<String, ServerOverride>,
    /// What this repo uses from the catalogue. Named here as well as read by
    /// [`crate::selection`], or the guard below would refuse `[use]` as a table
    /// nobody reads — which it would be, since `config::policy` skips tables
    /// and this is the reader.
    #[serde(default, rename = "use")]
    uses: BTreeMap<String, Vec<String>>,
    /// What this repo resolved to — `"<stack>/<provide>" = true` for each
    /// provide that applied. Named here for the same reason `use` is: unnamed,
    /// the guard below would refuse it as a table nobody reads, and `init`
    /// would write a file omh could not then parse.
    #[serde(default)]
    provision: BTreeMap<String, bool>,
    #[serde(flatten)]
    rest: toml::Table,
}

/// What a repo may say about a catalogue server. Environment and nothing else:
/// a repo names entries from your catalogue, it does not define one, so there
/// is deliberately no `command` here to redeclare it with.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct ServerOverride {
    #[serde(default)]
    env: BTreeMap<String, String>,
}

/// The gitignored layer's filename, so `init` ignores exactly the file this
/// module reads.
///
/// It is not covered by the `local/` line `init` already writes — that ignores
/// the *directory* `.omh/local`, and this is a file beside it. Documenting a
/// tracked file as gitignored is how a machine-local override gets committed
/// to somebody's team repo.
pub const LOCAL: &str = "settings.local.toml";

/// This repo's, then this repo's gitignored — later winning.
///
/// `~/.omh/default.toml` is not here and is not a layer. It is the template
/// `omh init` seeds a new repo from, so a repo's behaviour is explained by
/// files inside the repo — which is what a teammate cloning it can see, and
/// what `omh repo` can account for without pointing at a file they do not have.
///
/// Read from `config::Layer::SETTINGS` rather than spelled again, so the file a
/// feature switch is read from and the file a setting is read from cannot drift
/// apart.
fn layers(paths: &Paths) -> [PathBuf; 2] {
    crate::config::Layer::SETTINGS.map(|l| l.file(paths))
}

/// Everything this repo decided: which of omh's features are off here, and what
/// it says about the environment of the servers it uses.
///
/// One pass for all of it, because it comes from the same files and a second
/// pass would be a second chance for two readers to disagree about which layer
/// won.
pub fn resolve(paths: &Paths, manifest: &Manifest) -> Result<RepoPolicy> {
    let mut state: BTreeMap<String, bool> = BTreeMap::new();
    let mut mcp_env: BTreeMap<String, BTreeMap<String, String>> = BTreeMap::new();
    let mut provision: BTreeMap<String, bool> = BTreeMap::new();
    let mut selection = crate::selection::Selection::owning(manifest.owns());
    for path in layers(paths) {
        let Some(raw) = read(&path)? else {
            continue;
        };
        let file: File =
            toml::from_str(&raw).with_context(|| format!("parsing {}", path.display()))?;
        // A table — or an array *of* tables, which `is_table()` answers `false`
        // for, so `[[omhh]]` slipped through and was read by nobody. A scalar
        // or a plain array falls through on purpose: `carry_in = [".env"]` is
        // a setting, and `config::policy` resolves those.
        let is_table_like = |v: &toml::Value| {
            v.is_table()
                || v.as_array()
                    .is_some_and(|a| a.iter().any(toml::Value::is_table))
        };
        for (key, value) in &file.rest {
            if is_table_like(value) {
                // `[toolchain]` is named specifically, because it was omh's own
                // table and somebody was *told* to write it. It falls into
                // `rest` now that the field is gone, so it is already refused —
                // but by a message that says only "read by nobody", which is
                // true and useless to the person who typed `cargo = "skip"`
                // because a release of omh asked them to.
                //
                // An error rather than a warning: the table decided whether a
                // hook ran, and a version that reads it and does nothing leaves
                // somebody believing their answer is in force while the hook
                // fires every turn.
                anyhow::ensure!(
                    key != "toolchain",
                    "{}: `[toolchain]` was removed. omh now provisions your \
                     stack's toolchain and measures the image it built, so a \
                     hook whose program is missing is held back by name without \
                     anything on file — and starts running again by itself once \
                     the sandbox has it. Delete the table. If what you want is a \
                     provide left out, say so with \
                     `[provision] \"<stack>/<name>\" = false`.",
                    path.display()
                );
                anyhow::bail!(
                    "{}: `[{key}]` is read by nobody. This file holds settings at the \
                     top level, `[omh]` for omh's own features, and `[mcp.<name>.env]` \
                     for a server's environment in this repo.",
                    path.display()
                );
            }
        }
        for (key, on) in file.omh {
            validate(&key, manifest, &path)?;
            state.insert(key, on);
        }
        // Variable by variable, so a later layer adding a token does not drop
        // the one an earlier layer set. A repo overriding a server's
        // environment; the catalogue's own env lives on the server in
        // `~/.omh/mcp.json`, which is the file that owns servers.
        for (name, over) in file.mcp {
            mcp_env.entry(name).or_default().extend(over.env);
        }
        // Wholesale per capability, which is the opposite rule and deliberately
        // so: an env override adds a variable, a selection *is* the list, and
        // merging one would make removal unexpressible.
        selection.apply(&file.uses, &path)?;
        // Key by key, like `mcp_env` and unlike `[use]`: an opt-out on one
        // laptop says nothing about the provides it did not mention, so
        // replacing the table wholesale would discard the team's resolution for
        // every other stack.
        provision.extend(file.provision);
    }
    let off: BTreeSet<String> = state
        .into_iter()
        .filter(|(_, on)| !on)
        .map(|(name, _)| name)
        .collect();
    let mut policy = RepoPolicy::switching_off(manifest, off);
    policy.mcp_env = mcp_env;
    policy.selection = selection;
    policy.provision = provision;
    Ok(policy)
}

/// What this repo decided, resolved from its three settings files.
///
/// The counterpart to [`crate::base::Own`], which is what *omh* contributes.
/// Both reach `container::plan` from outside, and keeping them separate is what
/// stops "omh generated this" and "this repo asked for this" being answered by
/// one field lookup — `omh why`'s whole job is telling those apart.
/// `Default` is test-only, because [`crate::selection::Selection`]'s is: a
/// defaulted policy is one that thinks omh owns nothing, and the only callers
/// that want it are fixtures saying "this repo decided nothing".
#[cfg_attr(test, derive(Default))]
#[derive(Debug, Clone)]
pub struct RepoPolicy {
    /// Features switched off here. Nothing is uninstalled.
    pub off: BTreeSet<String>,
    /// Servers to drop from the rendered document even though `mcp.json` still
    /// lists them — the servers those switched-off features own. The file is
    /// left exactly as the user has it, and the next repo gets them back.
    pub disabled_servers: BTreeSet<String>,
    /// Per-repo MCP environment, by server name, from `[mcp.<name>.env]`.
    ///
    /// An override rather than a redeclaration, which is the whole point: a
    /// repo used to hold a token by copying the entire server entry into its
    /// own `mcp.json`, so a catalogue fix never reached it and the copy was
    /// invisible until it drifted.
    pub mcp_env: BTreeMap<String, BTreeMap<String, String>>,
    /// What this repo uses from the catalogue, from `[use]`.
    pub selection: crate::selection::Selection,
    /// Which provides apply here, from `[provision]` — the resolution `init`
    /// recorded so that launch re-evaluates no predicate.
    ///
    /// Keyed `"<stack>/<provide>"` by `stack::key`, which is pinned literally
    /// because this table is hand-edited. A `false` is a person's opt-out, for
    /// cost or because they supply the tool themselves; omh writes only `true`,
    /// so a `false` can only have been typed.
    ///
    /// What it changes is what goes into the image — `installs_for` drops the
    /// recipe, and `needs_of` stops claiming the provide owes anything, so
    /// nothing is reported as having failed to install when it was never
    /// installed.
    ///
    /// What it cannot do is make omh say something false about the sandbox. If
    /// a hook names the program, the probe asks about it through
    /// `render::hook_programs` regardless of this table, and a hook that cannot
    /// run is held back by name. This decides what omh *puts in* the image,
    /// never what omh *reports about* it.
    pub provision: BTreeMap<String, bool>,
}

impl RepoPolicy {
    /// The policy of a repo that switched off exactly these features.
    ///
    /// Public because the fixtures in `container` and `doctor` need it: a test
    /// that builds `disabled_servers` by hand is a second opinion about which
    /// servers a feature owns, and it can be wrong in the same direction as the
    /// code. One derivation, and `resolve` goes through it too.
    pub fn switching_off(manifest: &Manifest, off: BTreeSet<String>) -> Self {
        Self {
            disabled_servers: manifest
                .entries
                .iter()
                .filter(|e| e.kind == crate::base::Kind::Mcp && off.contains(&e.feature))
                .map(|e| e.name.clone())
                .collect(),
            off,
            mcp_env: BTreeMap::new(),
            // Empty of choices but *not* of what omh owns. A `Selection::default()`
            // here would let a fixture treat `codegraph` as an ordinary catalogue
            // entry, which is the one thing this type exists to prevent.
            selection: crate::selection::Selection::owning(manifest.owns()),
            provision: BTreeMap::new(),
        }
    }
}

/// A key has to be a feature. Checked where the value is minted, the rule
/// `memory::expand_key` states, so every reader inherits the one guard.
///
/// An entry name is the interesting error: it is how somebody discovers the
/// grouping without reading the manifest, and it is the request the design
/// deliberately refuses — `graph-first = false` would mean keeping the graph
/// and dropping one of the things that make it used, which is a bundle taken
/// apart rather than a setting.
fn validate(key: &str, manifest: &Manifest, path: &Path) -> Result<()> {
    let features: BTreeSet<&str> = manifest
        .entries
        .iter()
        .map(|e| e.feature.as_str())
        .collect();
    if features.contains(key) {
        return Ok(());
    }
    if let Some(entry) = manifest.entry(key) {
        anyhow::bail!(
            "{}: `{key}` is part of the `{}` feature, not a feature itself. \
             Write `{} = false` to switch off all of it — there is no way to \
             keep the rest and drop this one.",
            path.display(),
            entry.feature,
            entry.feature
        );
    }
    anyhow::bail!(
        "{}: `{key}` is not one of omh's features ({})",
        path.display(),
        features.into_iter().collect::<Vec<_>>().join(", ")
    )
}

/// Absent is not unreadable. `config::read_layer` records what conflating them
/// cost: a `chmod 000` file reported as "not declared", advice that could not
/// help, and a closed loop exiting 0.
fn read(path: &Path) -> Result<Option<String>> {
    match std::fs::read_to_string(path) {
        Ok(raw) => Ok(Some(raw)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e).with_context(|| format!("reading {}", path.display())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const BASE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/base");

    fn fixture() -> (tempfile::TempDir, Paths, Manifest) {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths {
            root: dir.path().join("home"),
            repo: dir.path().join("repo"),
        };
        for d in [&paths.root, &paths.repo.join(".omh")] {
            std::fs::create_dir_all(d).unwrap();
        }
        let manifest = Manifest::load_dir(Path::new(BASE)).unwrap();
        (dir, paths, manifest)
    }

    fn write(path: PathBuf, body: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, body).unwrap();
    }

    /// `init` ignores a filename; this module reads a path. They have to be
    /// the same file, and `.omh/.gitignore`'s existing `local/` line does not
    /// cover it — that is a directory, this is a file beside it. A tracked
    /// `settings.local.toml` is a machine-local override committed to a team
    /// repo.
    #[test]
    fn the_gitignored_layer_is_the_file_init_ignores() {
        let (_d, paths, _m) = fixture();
        let last = layers(&paths).last().unwrap().clone();
        assert_eq!(last.file_name().unwrap().to_string_lossy(), LOCAL);
        assert!(last.starts_with(paths.repo.join(".omh")));
    }

    #[test]
    fn a_repo_with_no_settings_has_everything_on() {
        let (_d, paths, m) = fixture();
        assert!(resolve(&paths, &m).unwrap().off.is_empty());
    }

    #[test]
    fn a_feature_named_false_is_off() {
        let (_d, paths, m) = fixture();
        write(
            paths.repo.join(".omh/settings.toml"),
            "[omh]\ncodegraph = false\n",
        );
        assert_eq!(
            resolve(&paths, &m).unwrap().off,
            BTreeSet::from(["codegraph".to_string()])
        );
    }

    /// A machine-wide preference and a one-repo exception both have to be
    /// expressible, or the layering is decoration.
    #[test]
    fn a_later_layer_wins() {
        let (_d, paths, m) = fixture();
        write(paths.root.join("settings.toml"), "[omh]\nmemory = false\n");
        write(
            paths.repo.join(".omh/settings.local.toml"),
            "[omh]\nmemory = true\n",
        );
        assert!(
            resolve(&paths, &m).unwrap().off.is_empty(),
            "this repo turned it back on"
        );
    }

    /// The state "graph on, refresher off" has to be unrepresentable rather
    /// than warned about — a graph that quietly stops tracking the code is the
    /// one combination that manufactures confident wrong answers.
    ///
    /// The error names the feature, which is also how somebody discovers the
    /// grouping without reading the manifest.
    #[test]
    fn a_hook_name_where_a_feature_belongs_names_the_feature() {
        let (_d, paths, m) = fixture();
        write(
            paths.repo.join(".omh/settings.toml"),
            "[omh]\ngraph-first = false\n",
        );
        // Not "mentions codegraph": the unknown-key error lists every feature
        // and would satisfy that while saying nothing about the grouping. The
        // guard is that this key is *part of* something, and which.
        let err = resolve(&paths, &m).unwrap_err().to_string();
        assert!(
            err.contains("`graph-first` is part of the `codegraph` feature"),
            "must say what it belongs to: {err}"
        );
        assert!(
            err.contains("codegraph = false"),
            "and what to write instead: {err}"
        );
    }

    #[test]
    fn an_unknown_feature_lists_the_features() {
        let (_d, paths, m) = fixture();
        write(
            paths.repo.join(".omh/settings.toml"),
            "[omh]\nteleport = false\n",
        );
        let err = resolve(&paths, &m).unwrap_err().to_string();
        assert!(err.contains("teleport"), "got: {err}");
        assert!(err.contains("codegraph") && err.contains("memory"), "{err}");
    }

    /// A repo says what a catalogue server's environment should be here, and
    /// nothing more — there is no `command`, so a repo cannot define a server
    /// by pretending to configure one.
    #[test]
    fn a_repo_overrides_a_servers_env_and_only_that() {
        let (_d, paths, m) = fixture();
        write(
            paths.repo.join(".omh/settings.local.toml"),
            "[mcp.linear.env]\nLINEAR_API_KEY = \"secret\"\n",
        );
        let r = resolve(&paths, &m).unwrap();
        assert_eq!(r.mcp_env["linear"]["LINEAR_API_KEY"], "secret");

        write(
            paths.repo.join(".omh/settings.local.toml"),
            "[mcp.linear]\ncommand = \"mine\"\n",
        );
        let err = format!("{:#}", resolve(&paths, &m).unwrap_err());
        assert!(err.contains("command"), "must name the key: {err}");
    }

    /// Variable by variable, so a machine-wide token and a per-repo region are
    /// both expressible — merging entry by entry would make the later layer
    /// silently drop the earlier one's variables.
    #[test]
    fn env_overrides_merge_variable_by_variable() {
        let (_d, paths, m) = fixture();
        write(
            paths.repo.join(".omh/settings.toml"),
            "[mcp.linear.env]\nTOKEN = \"t\"\n",
        );
        write(
            paths.repo.join(".omh/settings.local.toml"),
            "[mcp.linear.env]\nREGION = \"eu\"\n",
        );
        let env = &resolve(&paths, &m).unwrap().mcp_env["linear"];
        assert_eq!(env["TOKEN"], "t");
        assert_eq!(env["REGION"], "eu");
    }

    /// `[use]` is read from all three files, and a later one replaces a
    /// capability's list outright.
    ///
    /// The unit tests for this live in `selection.rs` and call `apply` directly
    /// with invented paths, so guarding the *resolve* with a layer check —
    /// making a personal `[use]` read by nobody, or a local one unable to
    /// override the committed list — left the whole suite green. Both are
    /// stated behaviours: a personal list is your default everywhere, and a
    /// local one is what `omh use` now has to write through.
    #[test]
    fn use_is_read_from_every_layer_and_the_last_one_wins() {
        let (_d, paths, m) = fixture();
        write(
            paths.root.join("settings.toml"),
            "[use]\nskills = [\"mine\"]\nrules = [\"tdd\"]\n",
        );
        write(
            paths.repo.join(".omh/settings.toml"),
            "[use]\nskills = [\"ours\"]\n",
        );
        let s = resolve(&paths, &m).unwrap().selection;
        assert!(s.allows(crate::adapter::Capability::Skills, "ours"));
        assert!(
            !s.allows(crate::adapter::Capability::Skills, "mine"),
            "the repo replaced the personal list rather than adding to it"
        );
        assert!(
            s.allows(crate::adapter::Capability::Rules, "tdd"),
            "and a capability only the personal layer named still stands"
        );

        write(
            paths.repo.join(".omh/settings.local.toml"),
            "[use]\nskills = [\"just-here\"]\n",
        );
        let s = resolve(&paths, &m).unwrap().selection;
        assert!(s.allows(crate::adapter::Capability::Skills, "just-here"));
        assert!(
            !s.allows(crate::adapter::Capability::Skills, "ours"),
            "the gitignored layer has the last word, which is why omh use writes it too"
        );
    }

    // ── the provisioning resolution ─────────────────────────────────────────

    /// What a repo resolved to: which stacks apply, which provides fired.
    ///
    /// Written by `init` after evaluating each provide's `when` in the sandbox,
    /// and read on every launch so that evaluation happens once rather than
    /// standing between somebody and their agent every session.
    ///
    /// **This reader lands before anything writes the table.** `resolve` refuses
    /// any table nobody reads — so `init` writing `[provision]` first would
    /// produce a file omh itself could not parse, and every later command would
    /// fail on it.
    #[test]
    fn a_provision_resolution_is_read_from_the_repo() {
        let (_d, paths, m) = fixture();
        write(
            paths.repo.join(".omh/settings.toml"),
            "[provision]\n\"rust/toolchain\" = true\n\"node/pnpm\" = false\n",
        );
        let policy = resolve(&paths, &m).unwrap();
        assert_eq!(policy.provision.get("rust/toolchain"), Some(&true));
        assert_eq!(policy.provision.get("node/pnpm"), Some(&false));
    }

    /// The ordering constraint, made executable.
    ///
    /// `resolve` refuses any table nobody reads, and that refusal is by design —
    /// a `[mcpp]` holding somebody's token reaches nothing, silently, which is
    /// the shape both this module and `config` exist to prevent. The cost is
    /// that a table omh *writes* must be readable **first**: reversed, `init`
    /// records `[provision]` and every later omh command dies parsing a file omh
    /// produced.
    ///
    /// Nothing else states that dependency, and somebody deleting the field as
    /// unused would find the suite green and the failure a release away.
    #[test]
    fn a_provision_table_is_not_a_table_nobody_reads() {
        let (_d, paths, m) = fixture();
        write(
            paths.repo.join(".omh/settings.toml"),
            "carry_in = []\n\n[provision]\n\"rust/toolchain\" = true\n",
        );
        assert!(
            resolve(&paths, &m).is_ok(),
            "omh must be able to read back the table it writes"
        );
    }

    /// *"Not on this laptop"* is a machine's decision, not the team's — so the
    /// gitignored layer has to be able to answer for itself, and win.
    #[test]
    fn a_later_layer_wins_a_provision_entry() {
        let (_d, paths, m) = fixture();
        write(
            paths.repo.join(".omh/settings.toml"),
            "[provision]\n\"rust/linker\" = true\n",
        );
        write(
            paths.repo.join(".omh/settings.local.toml"),
            "[provision]\n\"rust/linker\" = false\n",
        );
        assert_eq!(
            resolve(&paths, &m).unwrap().provision.get("rust/linker"),
            Some(&false)
        );
    }

    /// Key by key, like `[toolchain]` and `[mcp]` — and deliberately **not**
    /// like `[use]`, which replaces wholesale. A laptop that opts out of one
    /// provide has said nothing about the others, and taking its table as the
    /// whole answer would silently discard the team's resolution for every
    /// stack it did not mention.
    #[test]
    fn provision_entries_merge_key_by_key() {
        let (_d, paths, m) = fixture();
        write(
            paths.repo.join(".omh/settings.toml"),
            "[provision]\n\"go/toolchain\" = true\n\"rust/toolchain\" = true\n",
        );
        write(
            paths.repo.join(".omh/settings.local.toml"),
            "[provision]\n\"rust/linker\" = false\n",
        );

        let p = resolve(&paths, &m).unwrap().provision;
        assert_eq!(
            p.get("go/toolchain"),
            Some(&true),
            "personal survives: {p:?}"
        );
        assert_eq!(p.get("rust/toolchain"), Some(&true), "shared too: {p:?}");
        assert_eq!(p.get("rust/linker"), Some(&false), "and local: {p:?}");
    }

    /// A value omh cannot read is refused rather than dropped. Dropped, a
    /// typo'd opt-out silently becomes an opt-in, and the provide it was meant
    /// to exclude is installed anyway — with nothing said.
    #[test]
    fn a_provision_value_omh_cannot_read_is_refused_by_name() {
        let (_d, paths, m) = fixture();
        write(
            paths.repo.join(".omh/settings.toml"),
            "[provision]\n\"rust/linker\" = \"no\"\n",
        );
        let err = format!("{:#}", resolve(&paths, &m).unwrap_err());
        assert!(err.contains("rust/linker"), "must name the key: {err}");
        assert!(err.contains("settings.toml"), "and the file: {err}");
    }

    // ── the table that was removed ──────────────────────────────────────────

    /// `[toolchain]` is **gone**, and a repo that still has one is told so by
    /// name rather than having it quietly ignored.
    ///
    /// This is the one breakage this change accepts, so it is a stated
    /// behaviour with a test rather than a side effect of a field being
    /// deleted from a struct. The alternative — falling through to `rest` and
    /// producing whatever the generic guard happens to say — is how an accepted
    /// breakage becomes an accident nobody chose.
    ///
    /// It has to be an **error**, not a warning. The table's whole purpose was
    /// deciding whether a hook runs; a version that reads it and does nothing
    /// leaves somebody believing their `cargo = "skip"` is in force while the
    /// hook fires every turn.
    ///
    /// And the message has to name the way forward. Somebody wrote `skip`
    /// because their sandbox lacked a tool; the answer now is that omh
    /// provisions it, and if they want it left out that is a `[provision]`
    /// entry. An error that only says *stop* sends them to the source.
    #[test]
    fn a_repo_that_still_has_a_toolchain_table_is_told_so() {
        let (_d, paths, m) = fixture();
        write(
            paths.repo.join(".omh/settings.toml"),
            "[toolchain]\ncargo = \"skip\"\n",
        );
        let err = format!("{:#}", resolve(&paths, &m).unwrap_err());

        assert!(err.contains("toolchain"), "must name the table: {err}");
        assert!(err.contains("settings.toml"), "and the file: {err}");
        assert!(
            err.contains("provision"),
            "and what replaced it, or the only way forward is reading omh's \
             source: {err}"
        );
    }

    /// A table nobody reads is refused by name.
    ///
    /// `deny_unknown_fields` came off `File` so a `carry_in` beside `[omh]`
    /// would not be an error in one reader and a value in the other — right
    /// for scalars, which `config::policy` does read. It does not extend to
    /// tables: `[omh]` and `[mcp]` are the complete set either reader
    /// understands, so `[omhh]` or `[mcpp]` is read by nobody and reported by
    /// nothing. A token that reaches nothing, silently, is the shape both
    /// modules exist to refuse.
    #[test]
    fn a_table_nobody_reads_is_refused_by_name() {
        let (_d, paths, m) = fixture();
        write(
            paths.repo.join(".omh/settings.toml"),
            "[omhh]\ncodegraph = false\n",
        );
        let err = format!("{:#}", resolve(&paths, &m).unwrap_err());
        assert!(err.contains("omhh"), "must name the table: {err}");
        assert!(err.contains("settings.toml"), "and the file: {err}");

        // And a scalar still is not: `config::policy` reads those.
        write(
            paths.repo.join(".omh/settings.toml"),
            "carry_in = [\".env\"]\n",
        );
        assert!(
            resolve(&paths, &m).is_ok(),
            "a setting is not an unknown key"
        );
    }

    /// Absent is not unreadable — the `config::read_layer` lesson, which cost a
    /// closed loop that exited 0.
    #[cfg(unix)]
    #[test]
    fn an_unreadable_settings_file_is_an_error_not_an_absent_one() {
        use std::os::unix::fs::PermissionsExt;
        let (_d, paths, m) = fixture();
        let path = paths.repo.join(".omh/settings.toml");
        write(path.clone(), "[omh]\ncodegraph = false\n");
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o000)).unwrap();

        let err = resolve(&paths, &m).unwrap_err().to_string();
        assert!(err.contains("settings.toml"), "must name the file: {err}");
    }
}
