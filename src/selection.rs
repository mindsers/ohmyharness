//! What this repo uses from your catalogue.
//!
//! P3 gave content one home. That answered "where is this skill" and left the
//! other half open: everything in `~/.omh` reached every session, so *"these are
//! my twelve MCP servers, this project uses three"* was unsayable and the only
//! lever was uninstalling globally — the opposite of curating.
//!
//! ```toml
//! # <repo>/.omh/settings.toml
//! [use]
//! rules  = ["tdd", "commit-style"]   # and in this order
//! skills = ["review-diff"]
//! mcp    = ["*"]
//! ```
//!
//! **One mechanism: an allowlist.** No `exclude`, no `include`/`exclude` pair.
//! Removing something is deleting its name, and there is one place to look to
//! answer "is this on here".
//!
//! **Absent means everything; `[]` means nothing.** Those have to differ or the
//! design contradicts itself — a list that shrank to empty must mean what it
//! says, while a repo that never configured one gets the full catalogue, so
//! upgrading changes nothing and a new checkout is useful before it is
//! configured. `"*"` is "keep following the catalogue as it grows", said out
//! loud.
//!
//! **A feature is not selectable, in any capability.** `init` seeds
//! `manifest.servers()` into `~/.omh/mcp.json`, so `codegraph` and `memory` sit
//! there looking exactly like servers you added. Naming one in `[use].mcp`
//! without its hooks — or quietly omitting it — is a bundle taken apart, which
//! is the state `settings.rs` makes unrepresentable rather than warning about,
//! and the state that shipped broken once when the graph hooks kept firing
//! against a server dropped from the document. `[omh]` is the switch for those.

use crate::adapter::Capability;
use anyhow::{bail, Result};
use std::collections::BTreeMap;
use std::path::Path;

/// The wildcard, spelled the way a shell would.
const EVERYTHING: &str = "*";

/// What one capability's list says.
///
/// An enum rather than an `Option<Vec>` with a magic empty case, because
/// "everything" and "nothing" are the two ends of this design and reading them
/// off the length of a vector is how they get confused.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Chosen {
    /// No list, or `["*"]`. Follows the catalogue as it grows.
    All,
    /// Exactly these, in this order — which is load-bearing for `rules`, where
    /// the list is the composition order.
    These(Vec<String>),
}

/// What this repo selected, per capability, with the names omh owns held out.
#[derive(Debug, Clone, Default)]
pub struct Selection {
    chosen: BTreeMap<Capability, Chosen>,
    /// Names the manifest owns, each pointing at the feature it belongs to.
    /// Always allowed, never reported, never written by `init` — one place, so
    /// a mutation that drops the exemption goes red in the MCP, hook and rules
    /// paths at once.
    ///
    /// The feature rather than a bare set, because the useful half of refusing
    /// `[use] mcp = ["codegraph"]` is saying *which switch* does work.
    owned: Owned,
}

/// Capability to entry name to the feature that owns it.
pub type Owned = BTreeMap<Capability, BTreeMap<String, String>>;

impl Selection {
    /// An empty selection that knows what omh owns. Every capability follows
    /// the catalogue until a layer says otherwise.
    pub fn owning(owned: Owned) -> Self {
        Self {
            chosen: BTreeMap::new(),
            owned,
        }
    }

    /// Apply one layer's `[use]` table. Later layers replace earlier ones **per
    /// capability**, wholesale.
    ///
    /// Not merged, and that is the whole point: merging an allowlist makes
    /// removal unexpressible, which is the defect this exists to fix. A personal
    /// `[use].skills` is "my default everywhere"; a repo naming `skills`
    /// replaces it outright and says so by naming it.
    pub fn apply(&mut self, table: &BTreeMap<String, Vec<String>>, whence: &Path) -> Result<()> {
        for (key, names) in table {
            let cap = Capability::from_key(key).ok_or_else(|| {
                anyhow::anyhow!(
                    "{}: `[use] {key}` is not a capability — expected {}",
                    whence.display(),
                    Capability::ALL
                        .iter()
                        .map(Capability::to_string)
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            })?;
            self.chosen.insert(cap, self.read_list(cap, names, whence)?);
        }
        Ok(())
    }

    fn read_list(&self, cap: Capability, names: &[String], whence: &Path) -> Result<Chosen> {
        if names.iter().any(|n| n == EVERYTHING) {
            // `["*", "tdd"]` is somebody expecting `*` to mean "and also".
            // Read as `All` it silently ignores the rest; read as a literal
            // name it selects a file nobody has. Neither is what was meant.
            if names.len() > 1 {
                bail!(
                    "{}: `[use] {cap}` names `*` beside {} other entr{} — `*` is the \
                     whole catalogue, so the rest cannot add to it. Drop the `*`, or \
                     drop the names.",
                    whence.display(),
                    names.len() - 1,
                    if names.len() == 2 { "y" } else { "ies" }
                );
            }
            return Ok(Chosen::All);
        }
        let mut out = Vec::with_capacity(names.len());
        for name in names {
            validate_entry_name(name, cap, whence)?;
            if let Some(feature) = self.feature_owning(cap, name) {
                bail!(
                    "{}: `[use] {cap}` names `{name}`, which is omh's — part of the \
                     `{feature}` feature. `[use]` names your entries; a feature is all \
                     or nothing and `omh repo disable {feature}` is its switch.",
                    whence.display()
                );
            }
            // Order is meaningful for `rules` and a duplicate would compose the
            // same section twice, so the first mention wins and the second is
            // simply not a second entry.
            if !out.contains(name) {
                out.push(name.clone());
            }
        }
        Ok(Chosen::These(out))
    }

    /// Does this repo use `name`?
    ///
    /// A name omh owns is always allowed, whatever the lists say. That single
    /// branch is what keeps `[use]` and `[omh]` governing different things.
    pub fn allows(&self, cap: Capability, name: &str) -> bool {
        if self.is_omhs(cap, name) {
            return true;
        }
        match self.chosen.get(&cap) {
            None | Some(Chosen::All) => true,
            Some(Chosen::These(names)) => names.iter().any(|n| n == name),
        }
    }

    /// The declared order for `cap`, when there is one. `None` means the
    /// catalogue's own order stands — which for rules is filename order, stated
    /// as the placeholder it always was.
    pub fn order(&self, cap: Capability) -> Option<&[String]> {
        match self.chosen.get(&cap) {
            Some(Chosen::These(names)) => Some(names),
            _ => None,
        }
    }

    /// Catalogue entries this repo did not name.
    ///
    /// The report that makes an expanded `[use]` safe: `init` writes the list
    /// once, so a skill added afterwards is off and the reason is invisible.
    pub fn unselected(&self, cap: Capability, available: &[String]) -> Vec<String> {
        match self.chosen.get(&cap) {
            Some(Chosen::These(_)) => available
                .iter()
                .filter(|n| !self.allows(cap, n))
                .cloned()
                .collect(),
            _ => Vec::new(),
        }
    }

    /// Names this repo selected that nothing answers to — a typo, or an entry
    /// removed from the catalogue while a repo still named it.
    pub fn missing(&self, cap: Capability, available: &[String]) -> Vec<String> {
        match self.chosen.get(&cap) {
            Some(Chosen::These(names)) => names
                .iter()
                .filter(|n| !available.iter().any(|a| a == *n) && !self.is_omhs(cap, n))
                .cloned()
                .collect(),
            _ => Vec::new(),
        }
    }

    /// Is this one of omh's own?
    pub fn is_omhs(&self, cap: Capability, name: &str) -> bool {
        self.feature_owning(cap, name).is_some()
    }

    /// The feature that owns `name`, if omh does.
    fn feature_owning(&self, cap: Capability, name: &str) -> Option<&str> {
        self.owned.get(&cap)?.get(name).map(String::as_str)
    }
}

/// A name is one entry in one directory, so it is a name and never a path.
///
/// Checked where a name is **minted** — here, in `omh use`, and in
/// `omh config edit` — rather than where it is joined to a directory, the rule
/// `memory::validate_key` and `carry::validate_pattern` already follow. Every
/// future caller inherits the guard instead of having to remember it, which is
/// the difference between a rule and a habit.
pub fn validate_entry_name(name: &str, cap: Capability, whence: &Path) -> Result<()> {
    // Stricter than `memory::validate_key`, which allows slash-separated slugs
    // because a note key is a hierarchy. An entry is one name in one directory,
    // so *any* separator is already wrong and there is nothing to split. A
    // leading `.` then covers `..` and every dotfile in one arm.
    let bad = name.is_empty()
        || name.starts_with('.')
        || name.contains('/')
        || name.contains('\\')
        || name.contains('\0');
    if bad {
        bail!(
            "{}: `{name}` is not a `{cap}` entry — an entry is a name in your \
             catalogue, never a path",
            whence.display()
        );
    }
    Ok(())
}

impl Capability {
    /// The word a `[use]` table and the CLI use for this capability.
    ///
    /// From `Display` rather than a second table: `mcp` is spelled `mcp.json`
    /// on disk and `mcp` everywhere a person types it, and two lists of six
    /// words is one list too many.
    pub fn from_key(key: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|c| c.to_string() == key)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn owned() -> Owned {
        let entry = |name: &str, feature: &str| (name.to_string(), feature.to_string());
        BTreeMap::from([
            (
                Capability::Mcp,
                BTreeMap::from([entry("codegraph", "codegraph"), entry("memory", "memory")]),
            ),
            (
                Capability::Hooks,
                BTreeMap::from([entry("graph-first", "codegraph")]),
            ),
        ])
    }

    fn selection(pairs: &[(&str, &[&str])]) -> Selection {
        let mut s = Selection::owning(owned());
        let table = pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.iter().map(|n| n.to_string()).collect()))
            .collect();
        s.apply(&table, Path::new("settings.toml")).unwrap();
        s
    }

    fn refused(pairs: &[(&str, &[&str])]) -> String {
        let mut s = Selection::owning(owned());
        let table = pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.iter().map(|n| n.to_string()).collect()))
            .collect();
        format!(
            "{:#}",
            s.apply(&table, Path::new("settings.toml"))
                .expect_err("should have been refused")
        )
    }

    /// The upgrade guard. A repo that never configured a selection has to get
    /// the whole catalogue, or shipping this feature turns every existing
    /// checkout off at once.
    #[test]
    fn a_repo_with_no_use_table_gets_the_whole_catalogue() {
        let s = Selection::owning(owned());
        for cap in Capability::ALL {
            assert!(s.allows(cap, "anything at all"), "{cap} should be open");
        }
        assert!(s.unselected(Capability::Skills, &["a".into()]).is_empty());
    }

    /// `[]` is not absence, and the difference is the whole design: removing
    /// something is deleting its name, so a list emptied by deleting the last
    /// one has to mean what it says.
    #[test]
    fn an_empty_list_selects_nothing_and_a_star_selects_everything() {
        let none = selection(&[("skills", &[])]);
        assert!(!none.allows(Capability::Skills, "review-diff"));

        let all = selection(&[("skills", &["*"])]);
        assert!(all.allows(Capability::Skills, "review-diff"));
        assert!(
            all.unselected(Capability::Skills, &["review-diff".into()])
                .is_empty(),
            "`*` follows the catalogue, so nothing is ever unselected under it"
        );
    }

    /// Merging would make removal unexpressible — a repo could add to your
    /// personal list and never take anything off it, which is the defect this
    /// module exists to fix wearing a different hat.
    #[test]
    fn a_later_layer_replaces_a_capabilitys_list_wholesale() {
        let mut s = Selection::owning(owned());
        let layer = |pairs: &[(&str, &[&str])]| -> BTreeMap<String, Vec<String>> {
            pairs
                .iter()
                .map(|(k, v)| (k.to_string(), v.iter().map(|n| n.to_string()).collect()))
                .collect()
        };
        s.apply(
            &layer(&[("skills", &["mine", "yours"]), ("rules", &["tdd"])]),
            Path::new("personal"),
        )
        .unwrap();
        s.apply(&layer(&[("skills", &["yours"])]), Path::new("repo"))
            .unwrap();

        assert!(
            !s.allows(Capability::Skills, "mine"),
            "replaced, not merged"
        );
        assert!(s.allows(Capability::Skills, "yours"));
        assert!(
            s.allows(Capability::Rules, "tdd"),
            "and a capability the later layer said nothing about is untouched"
        );
    }

    #[test]
    fn a_capability_key_outside_the_six_is_refused() {
        let err = refused(&[("plugins", &["x"])]);
        assert!(err.contains("plugins"), "must name it: {err}");
        assert!(err.contains("subagents"), "and list the six: {err}");
    }

    /// The traversal guard, at the place a name is minted. `omh config edit`
    /// joins one of these to a directory, and a guard that lives there instead
    /// is a guard every future caller has to remember.
    #[test]
    fn a_name_that_climbs_out_of_the_catalogue_is_refused() {
        for bad in ["../../../.ssh/id_rsa", "..", "a/b", "", ".hidden", "a\\b"] {
            let err = refused(&[("skills", &[bad])]);
            assert!(
                err.contains("never a path"),
                "{bad:?} slipped through: {err}"
            );
        }
    }

    /// `[use]` names *your* entries. omh's belong to `[omh]`, and the rule holds
    /// across every capability rather than for hooks alone: `init` seeds
    /// `codegraph` and `memory` into `~/.omh/mcp.json`, where they look exactly
    /// like servers you added.
    ///
    /// Selecting one without its hooks — or omitting it from a list that names
    /// the servers around it — is a feature taken apart, which is the one
    /// combination `settings.rs` refuses to let anybody express.
    #[test]
    fn a_name_omh_owns_is_not_selectable_in_any_capability() {
        for (cap, name) in [("mcp", "codegraph"), ("hooks", "graph-first")] {
            let err = refused(&[(cap, &[name])]);
            assert!(err.contains(name), "must name it: {err}");
            assert!(
                err.contains("omh repo disable"),
                "and point at the switch that does work: {err}"
            );
        }
    }

    /// And the other direction, which is the one that would fail silently: a
    /// list that simply omits omh's own must not turn them off.
    #[test]
    fn an_empty_selection_leaves_omhs_own_alone() {
        let s = selection(&[("mcp", &[]), ("hooks", &[])]);
        assert!(s.allows(Capability::Mcp, "codegraph"));
        assert!(s.allows(Capability::Mcp, "memory"));
        assert!(s.allows(Capability::Hooks, "graph-first"));
        assert!(
            !s.allows(Capability::Mcp, "linear"),
            "while yours are genuinely off"
        );
    }

    /// The list is the order — the thing P3 said it was deferring, since
    /// ordering can only really come from a list somebody wrote.
    #[test]
    fn the_declared_order_is_the_order() {
        let s = selection(&[("rules", &["zebra", "apple"])]);
        assert_eq!(s.order(Capability::Rules).unwrap(), ["zebra", "apple"]);
        assert!(
            s.order(Capability::Skills).is_none(),
            "a capability with no list has no opinion about order"
        );
    }

    /// A name written twice would compose a rules section twice. First mention
    /// wins, because that is the position somebody meant.
    #[test]
    fn a_name_repeated_is_still_one_entry() {
        let s = selection(&[("rules", &["tdd", "style", "tdd"])]);
        assert_eq!(s.order(Capability::Rules).unwrap(), ["tdd", "style"]);
    }

    /// `*` beside a name is somebody expecting it to mean "and also". Read as
    /// everything it silently ignores the rest; read as a literal name it
    /// selects a file nobody has.
    #[test]
    fn a_star_beside_a_name_is_refused() {
        let err = refused(&[("skills", &["*", "review-diff"])]);
        assert!(err.contains("whole catalogue"), "got: {err}");
    }

    #[test]
    fn unselected_names_what_the_catalogue_has_and_the_repo_did_not_take() {
        let s = selection(&[("skills", &["review-diff"])]);
        assert_eq!(
            s.unselected(
                Capability::Skills,
                &["graphify".into(), "review-diff".into()]
            ),
            vec!["graphify"]
        );
    }

    /// omh's own live in the catalogue's `mcp.json` and are governed elsewhere.
    /// Reporting them as unselected would send people to `omh use` for
    /// something `omh use` refuses to write.
    #[test]
    fn omhs_own_are_never_reported_as_unselected() {
        let s = selection(&[("mcp", &["linear"])]);
        assert!(s
            .unselected(
                Capability::Mcp,
                &["codegraph".into(), "memory".into(), "linear".into()]
            )
            .is_empty());
    }

    #[test]
    fn a_selected_name_nothing_answers_to_is_reported() {
        let s = selection(&[("skills", &["reveiw-diff"])]);
        assert_eq!(
            s.missing(Capability::Skills, &["review-diff".into()]),
            vec!["reveiw-diff"]
        );
        assert!(
            s.missing(Capability::Rules, &[]).is_empty(),
            "a capability with no list can name nothing missing"
        );
    }
}
