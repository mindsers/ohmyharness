//! Settings, and omh’s own features.
//!
//! One rule serves four commands, and it is the reason these live together: a
//! write reaches the layer that already declares the key, else the key’s own
//! classification in `crate::key` decides, else `--local` or `--save` names a
//! file on purpose. What must not be committed is a property of the key
//! rather than of the command — fail-safe by enumeration, which is a real
//! trade and is recorded in `docs/design/decisions.md`.

use crate::adapter::{self, Adapter};
use crate::out;
use crate::profile::{Paths, Profile};
use crate::{auth, base, config, key, notice, report, selection, settings};
use anyhow::{Context, Result};
use std::process::Command;

/// Your defaults, against every key omh reads.
///
/// The unset half is the useful half. `key::KEYS` is a table in the binary, so
/// a settings file cannot show it, and until this existed the only way to
/// learn a key's name was to already know it.
pub(crate) fn show_settings(paths: &Paths, ctx: &out::Ctx) -> Result<()> {
    // Read straight from the file, not through `config::policy` — that
    // resolves a repo, and the template is not one of the layers it resolves
    // through. Asking it would report your template as empty.
    let mine = config::values(paths, config::Layer::Personal)?;

    ctx.say(&report::Settings {
        file: config::Layer::Personal.file(paths).display().to_string(),
        known: key::KEYS
            .iter()
            .map(|k| report::Known {
                key: k.name.to_string(),
                does: k.does.to_string(),
                value: mine.get(k.name).cloned(),
            })
            .collect(),
        // `[use]` and `[omh]` are seeded, so they are neither a default nor
        // unread. `config::values` renders a table as `[name]`.
        tables: mine
            .keys()
            .filter(|k| *k == &format!("[{}]", config::USE) || *k == &format!("[{}]", config::OMH))
            .cloned()
            .collect(),
        unread: mine
            .iter()
            .filter(|(k, _)| {
                key::describes(k).is_none()
                    && *k != &format!("[{}]", config::USE)
                    && *k != &format!("[{}]", config::OMH)
            })
            .map(|(k, v)| report::Setting {
                key: k.clone(),
                value: v.clone(),
                whose: None,
            })
            .collect(),
    });
    Ok(())
}

/// What is effective in this checkout, and which file decided it.
///
/// Where the reporting this design keeps promising actually surfaces. With a
/// curated list the useful question stops being "what is this set to" and
/// becomes "why is this skill not here", and that needs the selection, the
/// features and the settings in one place.
pub(crate) fn show_repo(cwd: &std::path::Path, ctx: &out::Ctx) -> Result<()> {
    let paths = Paths::discover(cwd)?;
    let manifest = base::Manifest::load_dir(&paths.base())?;
    let policy = settings::resolve(&paths, &manifest)?;

    let settings = config::policy(&paths)?
        .into_iter()
        .map(|s| report::Effective {
            key: s.key,
            value: s.value,
            layer: s.layer.to_string(),
            shadows: s.shadows.iter().map(|l| l.to_string()).collect(),
        })
        .collect();

    let mut names: Vec<&str> = manifest
        .entries
        .iter()
        .map(|e| e.feature.as_str())
        .collect();
    names.sort();
    names.dedup();
    let features = names
        .into_iter()
        .map(|feature| report::Feature {
            name: feature.to_string(),
            on: !policy.off.contains(feature),
        })
        .collect();

    let using = using_here(&paths, &manifest)?;

    ctx.say(&report::Repo {
        repo_id: paths.repo_name(),
        dir: paths.repo.join(".omh").display().to_string(),
        settings,
        features,
        using,
        notices: selection_notices(&paths, &manifest)?,
    });
    Ok(())
}

/// What this repo takes from your catalogue, per capability.
///
/// Lifted out of `show_repo` when `init` needed the same answer. Two commands
/// deriving it separately is two answers that are free to disagree, and the
/// one a person reads first is `init`'s.
pub(crate) fn using_here(paths: &Paths, manifest: &base::Manifest) -> Result<Vec<report::Using>> {
    let profile = Profile::resolve(paths);
    let policy = settings::resolve(paths, manifest)?;
    // What omh's own features bring, which `[use]` never names — a feature owns
    // its entries, so they are excluded from the selection. Reported beside the
    // selection rather than folded into it: they are here because a feature is
    // on, and `omh set <feature> off` is what takes them away.
    let owned = manifest.owns();
    let mut using = Vec::new();
    for cap in adapter::Capability::ALL {
        let entries = profile.entries(cap)?;
        let unselected = policy.selection.unselected(cap, &entries);
        let from_a_feature: Vec<String> = owned
            .get(&cap)
            .map(|names| {
                let mut on: Vec<String> = names
                    .iter()
                    .filter(|(_, feature)| !policy.off.contains(*feature))
                    .map(|(name, _)| name.clone())
                    .collect();
                on.sort();
                on
            })
            .unwrap_or_default();
        // `None` rather than a list identical to the catalogue's, because the
        // two are different states: one follows the catalogue as it grows and
        // the other is a list that happens to be complete today.
        //
        // Kept in the **declared** order, not `entries`' alphabetical one. For
        // `rules` that order is the whole feature — this page's own docs say
        // "the list is the order" — and building the line from the sorted
        // catalogue made this report the one place that contradicted it.
        // Filtered by what the catalogue actually holds, so a name nothing
        // answers to is reported as missing rather than listed as used.
        using.push(report::Using {
            capability: cap.to_string(),
            selected: policy.selection.order(cap).map(|order| {
                order
                    .iter()
                    .filter(|n| entries.iter().any(|e| e == *n))
                    .cloned()
                    .collect()
            }),
            unselected,
            from_a_feature,
        });
    }
    Ok(using)
}

/// The advisory lines a selection wants to add, for the two reports that print
/// them.
pub(crate) fn selection_notices(paths: &Paths, manifest: &base::Manifest) -> Result<Vec<String>> {
    notice::selection(
        &Profile::resolve(paths),
        &settings::resolve(paths, manifest)?.selection,
        &crate::cmd::catalogue::catalogue_lists(paths)?,
    )
}

/// Where a key belongs when nothing else has an opinion — the registry alone.
///
/// **The default is the committed file**, which is the opposite of what the
/// repo-scoped write did before 0.7.0, and the reversal is the point: what runtime a project
/// wants, and how long its sessions idle, are facts about the project, and a
/// teammate cloning it should get them. Until this table existed that default
/// was unavailable, because *every* value went to the gitignored file and the
/// safety came from the destination rather than from knowing the key.
///
/// So an unknown key is committed too. That is deliberate and it is the whole
/// reason `key::KEYS` is guarded by a scan rather than kept by hand: a key the
/// code reads and the table has never heard of would otherwise be classified
/// by whoever typed it. `every_setting_omh_reads_is_a_key_omh_can_classify`
/// fails the build first.
pub(crate) fn key_layer(key: &str) -> config::Layer {
    // The default half of `rule`, called *by* `rule` rather than duplicated
    // beside it. Two spellings of one judgement is the shape that drifts
    // silently, so there is only one, and it cannot disagree with itself.
    key::describes(key).map_or(config::Layer::Shared, key::Key::default_layer)
}

/// Which repo files a write or a removal has to reach, and what decided.
///
/// One value because it is one decision, and **one rule for four commands** —
/// `omh set`, `omh unset`, `omh use`, `omh unuse`. They had three different
/// answers between them, and every difference was a place for a command to
/// report success over something it had not changed.
#[derive(Debug, Clone)]
pub(crate) struct Reach {
    layers: Vec<config::Layer>,
    why: Why,
}

/// What sent a write to its layers.
///
/// The COMMITTED warning reads this. It fires on a committed write, which was
/// rare and deliberate while a flag was the only way to reach one and is
/// nearly *every* write now that the committed file is the default. A sentence
/// that fires on almost every invocation is one people stop reading, and this
/// codebase has already watched that happen once, to this same sentence, for
/// the same reason: it could not tell `account` from `carry_in`.
#[derive(Debug, Clone, Copy)]
pub(crate) enum Why {
    /// `--local` or `--save`: the file was named on the command line.
    Named,
    /// The key already has a value here, so the write joins it rather than
    /// landing in a file that value outranks.
    JoinsAStandingValue,
    /// Nothing held it, and the registry classified the key. The
    /// classification itself is read back through `key::describes` where the
    /// warning needs it — carrying it here too would be a second copy of one
    /// fact, and the compiler correctly points out that nothing reads it.
    Registry,
    /// Nothing held it, and the registry has never heard of it.
    Unclassified,
}

impl Reach {
    /// A command line that named its own file. The only route to `Why::Named`,
    /// so the reason cannot disagree with the layers at a call site.
    pub(crate) fn named(layer: config::Layer) -> Self {
        Self {
            layers: vec![layer],
            why: Why::Named,
        }
    }

    /// Whether any file this reaches is one git carries.
    pub(crate) fn committed(&self) -> bool {
        self.layers.iter().any(config::Layer::is_committed)
    }

    /// The same decision, read for a removal.
    ///
    /// A *write* with nothing holding the key needs a destination, and rule 3
    /// supplies one. A removal needs the opposite answer: nothing held it, so
    /// there is nothing to remove — and naming the default layer anyway is how
    /// the dry run came to promise dropping a switch from a repo that had
    /// never switched anything, while the real run said the opposite two lines
    /// later. A named flag still means that file, because you asked.
    pub(crate) fn for_removal(mut self) -> Self {
        if matches!(self.why, Why::Registry | Why::Unclassified) {
            self.layers.clear();
        }
        self
    }
}

/// The rule, given the flags as typed and where the thing already is.
///
/// 1. `--local` and `--save` are the user naming the file outright.
/// 2. Otherwise **every repo layer that already holds it**. Reaching one while
///    another still declares the key is how `omh unuse` reported success over
///    an entry it was still staging, and how `omh unset carry_in` left a map to
///    a credential in a committed file while saying it had removed it. Both
///    shipped. Reaching all of them ends the class rather than each instance.
/// 3. Otherwise the committed file — what runtime a project wants, and which
///    of omh's features it runs with, are facts about the project a teammate
///    cloning should get — **except** a key that can name a credential, which
///    goes to the gitignored one. That exception is `src/key.rs`, per key, and
///    it is the whole of what keeps a secret out of git now that committed is
///    the default.
///
/// An unknown key is committed too, deliberately: one the code reads and the
/// table has never heard of would otherwise be classified by whoever typed it,
/// and `every_setting_omh_reads_is_a_key_omh_can_classify` fails the build
/// before that can happen to one of omh's own.
pub(crate) fn rule(held: Vec<config::Layer>, key: &str, local: bool, save: bool) -> Reach {
    if local {
        return Reach::named(config::Layer::Local);
    }
    if save {
        return Reach::named(config::Layer::Shared);
    }
    if !held.is_empty() {
        return Reach {
            layers: held,
            why: Why::JoinsAStandingValue,
        };
    }
    // Through `key_layer`, not a second `map_or` beside it. Two spellings of
    // one judgement is the shape that drifts silently, and a guard reading the
    // other one is how the step where rule 2 lives went unguarded in #82.
    Reach {
        layers: vec![key_layer(key)],
        why: if key::describes(key).is_some() {
            Why::Registry
        } else {
            Why::Unclassified
        },
    }
}

/// The rule for a bare settings key.
pub(crate) fn reach(paths: &Paths, key: &str, local: bool, save: bool) -> Result<Reach> {
    Ok(rule(config::holding(paths, key)?, key, local, save))
}

/// The rule for a key inside a table — `[use]`'s capabilities, `[omh]`'s
/// features. Same three steps, one level in.
pub(crate) fn reach_in(
    paths: &Paths,
    table: &str,
    key: &str,
    local: bool,
    save: bool,
) -> Result<Reach> {
    Ok(rule(
        config::holding_in(paths, table, key)?,
        key,
        local,
        save,
    ))
}

/// Drop a feature switch, letting omh's own default return.
///
/// The mirror of `feature_switch`, and it exists for the reason `unset` was
/// just repaired: a feature lives in the `[omh]` table, so `omh unset
/// codegraph` reading bare keys found nothing and reported the feature absent
/// while `[omh] codegraph = false` sat in the file, still off.
pub(crate) fn feature_forget(
    paths: &Paths,
    feature: &str,
    reach: Reach,
    dry_run: bool,
    ctx: &out::Ctx,
) -> Result<()> {
    let reach = reach.for_removal();
    if dry_run {
        // Planned against the layers that actually hold the switch, so the
        // preview cannot promise a drop the real run then denies. It used to
        // iterate `declaring`, which always names the committed file, and said
        // "would drop …" over a feature nobody had ever switched.
        for layer in &reach.layers {
            ctx.say(
                &report::Action::new(
                    "feature-forget-planned",
                    format!("would drop the {feature} switch from the {layer} layer"),
                )
                .data(serde_json::json!({ "feature": feature, "layer": layer.to_string() })),
            );
        }
        if reach.layers.is_empty() {
            ctx.say(&report::Action::new(
                "feature-forget-planned",
                format!("{feature} is not switched here; nothing to drop"),
            ));
        }
        return Ok(());
    }
    // Collected rather than short-circuited. A `?` inside the loop abandoned
    // everything already dropped *and* the report that would have said so, so
    // a permission error on the second file left the first one silently
    // rewritten — in a committed file, which the user then finds in a diff.
    let mut dropped = Vec::new();
    let mut failed = None;
    for layer in &reach.layers {
        match config::forget_feature(paths, *layer, feature) {
            Ok(true) => dropped.push(*layer),
            Ok(false) => {}
            Err(e) => {
                failed = Some((*layer, e));
                break;
            }
        }
    }
    ctx.say(
        &report::Action::new(
            if dropped.is_empty() {
                "feature-unset-absent"
            } else {
                "feature-unset"
            },
            if dropped.is_empty() {
                format!("{feature} was not switched here")
            } else {
                format!("this checkout no longer switches {feature}")
            },
        )
        .data(serde_json::json!({
            "feature": feature,
            "dropped": dropped.iter().map(ToString::to_string).collect::<Vec<_>>(),
        })),
    );
    if let Some((layer, e)) = failed {
        return Err(e).with_context(|| format!("the {layer} layer still switches `{feature}`"));
    }
    // What is still deciding, if anything. `config::policy` cannot answer —
    // it skips tables by design, so `[omh]` is invisible to it — and the
    // personal layer is one no repo command writes but every read consults.
    say_what_still_switches(paths, feature, ctx)
}

/// Name the layer still switching a feature after a drop.
///
/// `omh unset codegraph` reported "follows omh's default" while
/// `~/.omh/settings.toml` held `[omh] codegraph = false`, which no repo
/// command reaches and every read honours. The message was a confident claim
/// about the effective state made by code that had looked at two layers of
/// three.
pub(crate) fn say_what_still_switches(paths: &Paths, feature: &str, ctx: &out::Ctx) -> Result<()> {
    for layer in config::Layer::SETTINGS {
        let doc = config::read_table(paths, layer, config::OMH)?;
        if let Some(on) = doc.get(feature) {
            ctx.warn(&format!(
                "`{feature}` is still switched {} by the {layer} layer — {}",
                if *on { "on" } else { "off" },
                layer.file(paths).display()
            ));
            return Ok(());
        }
    }
    Ok(())
}

/// Which of the two things `omh set <name>` is talking about.
///
/// A settings key is a bare key with a value you typed; one of omh's features
/// is a boolean in the `[omh]` table. Same command, different files' shapes,
/// and getting it wrong is silent in the worst direction: `omh set codegraph
/// off` used to write a top-level `codegraph = "off"`, warn that nothing reads
/// it, exit 0, and leave the feature on. A settings file that looks like it
/// says what you meant, beside a feature that ignored you.
///
/// **Three vocabularies, and only one pair is disjoint.** Settings keys do not
/// collide with feature names or entry names — `no_name_is_both_a_setting_and_a_feature`
/// reads `key::KEYS` and every shipped manifest and fails the build if they
/// ever do. Features and entries **overlap on purpose**: the manifest names a
/// feature after its principal entry, so `codegraph` and `memory` are each
/// both. That makes the order of the two manifest checks load-bearing, and
/// `a_feature_named_after_its_own_entry_resolves_as_the_feature` pins it —
/// read as an entry, `omh set codegraph off` would be refused with
/// `codegraph is part of the codegraph feature` and the feature would have no
/// spelling at all.
pub(crate) enum Names {
    /// In `key::KEYS`.
    ASetting,
    /// In the shipped manifest's feature column.
    AFeature,
    /// A base-set entry, which belongs to a feature without being one. Named
    /// separately because it is the interesting mistake: it is how somebody
    /// discovers the grouping without reading the manifest, and falling
    /// through to the key path would write `graph-rules = "off"` and report
    /// success over a rule that stayed on.
    AnEntryOf(String),
    /// One of your own catalogue entries, which `omh use`/`omh unuse` select
    /// and no settings key is named after.
    ///
    /// Carries *which*. The refusal has to name the capability, and a variant
    /// that cannot say it gets answered with whichever one was hardcoded —
    /// which is how this checked `skills` alone and told somebody with a rule
    /// of that name that nothing reads it.
    ACatalogueEntry(adapter::Capability),
    /// Neither. Written as a key and reported, the way it always was — a
    /// settings file is hand-editable and a key a newer omh reads must not be
    /// refused by this one.
    Neither,
}

pub(crate) fn names(paths: &Paths, name: &str, ctx: &out::Ctx) -> Names {
    if key::describes(name).is_some() {
        return Names::ASetting;
    }
    // **Reported and withdrawn, never fatal.** The manifest is consulted only
    // to *rule out* a feature name, and a home where `omh init` has not run has
    // none — which must not be how `omh unset <key>` starts failing, since that
    // is the command a person runs to get a secret out of git. Propagating the
    // error made an ordinary repo-local write depend on the health of a
    // directory the user never mentioned, and the `Neither` arm below promises
    // the opposite in as many words.
    let manifest = match base::Manifest::load_dir(&paths.base()) {
        Ok(m) => m,
        Err(e) => {
            ctx.warn(&format!(
                "could not read omh's base set, so `{name}` was not checked \
                 against omh's features — treating it as a setting\n  because {e:#}"
            ));
            return Names::Neither;
        }
    };
    // Features before entries, and that order **is** load-bearing: the
    // manifest names a feature after its principal entry, so `codegraph` and
    // `memory` are each both. Read as an entry, `omh set codegraph off` would
    // be refused with `codegraph is part of the codegraph feature` and the
    // feature would have no spelling at all.
    // `a_feature_named_after_its_own_entry_resolves_as_the_feature` pins it.
    if manifest.entries.iter().any(|e| e.feature == name) {
        return Names::AFeature;
    }
    if let Some(entry) = manifest.entry(name) {
        return Names::AnEntryOf(entry.feature.clone());
    }
    // A name from your own catalogue. The deleted feature-switch command
    // refused one by name — "a skill is not a feature" — and losing it would
    // have turned the refusal into a bare key nothing reads, which is a
    // quieter answer to a question somebody asked clearly.
    //
    // **Every capability, not just skills.** The first version checked
    // `Skills` alone, so a rule or a command of the same name got the dead key
    // instead — the same mistake one word over, which is what the refusal
    // exists to catch. `Capability::ALL` is the list that cannot rot.
    let profile = Profile::resolve(paths);
    for cap in adapter::Capability::ALL {
        // Not `.flatten()`. `Profile::entries` returns `Result` precisely so a
        // half-read catalogue is not reported as an empty one — its own
        // comment says so — and swallowing that here turns an unreadable
        // directory into a bare key written to a committed file.
        match profile.entries(cap) {
            Ok(entries) if entries.iter().any(|e| e == name) => {
                return Names::ACatalogueEntry(cap);
            }
            Ok(_) => {}
            Err(e) => ctx.warn(&format!(
                "could not read your {cap} catalogue, so `{name}` was not \
                 checked against it\n  because {e:#}"
            )),
        }
    }
    Names::Neither
}

/// A value only omh can check: which captured login `account` names.
///
/// The account is one thing with one spelling — `omh auth <harness> -n work`
/// creates it, this selects it, and everything that launches or probes reads
/// the setting. The global `-a` that used to override it per invocation is
/// gone, so a typo here is now the only way to point a launch at credentials
/// that are not there, and it would otherwise surface as a failed login inside
/// a sandbox rather than as a wrong word on the command line.
///
/// Accounts live per harness — `~/.omh/creds/<harness>/<account>` — so this
/// has three answers rather than two, and the middle one is the common case:
/// captured for *some* harness is accepted, because "I only use claude" is
/// ordinary, and the harnesses are named because `work` is right until the day
/// you run `omh new opencode`.
pub(crate) fn no_account_that_no_login_answers_to(
    paths: &Paths,
    name: &str,
    ctx: &out::Ctx,
) -> Result<()> {
    let adapters = Adapter::load_dir(&paths.adapters()).unwrap_or_default();
    let mut has: Vec<String> = Vec::new();
    let mut all: Vec<String> = Vec::new();
    for adapter in &adapters {
        for account in auth::accounts(paths, adapter) {
            if account == name {
                has.push(adapter.name.clone());
            }
            all.push(format!("{} ({})", account, adapter.name));
        }
    }
    if !has.is_empty() {
        ctx.announce(&format!("`{name}` is captured for {}", has.join(", ")));
        return Ok(());
    }
    all.sort();
    all.dedup();
    anyhow::bail!(
        "no captured login called `{name}`.\n  {}",
        if all.is_empty() {
            "omh auth <harness> -n <name>   capture one first".to_string()
        } else {
            format!("captured: {}", all.join(", "))
        }
    );
}

/// `omh settings set` must not write a name omh already owns as a bare key.
///
/// `omh settings set codegraph off` wrote a top-level `codegraph = "off"`,
/// warned that nothing reads it, exited 0, and left the feature on. That is
/// the defect the fork exists to end, and `omh set` was routed through the
/// same reading while this door stayed open.
///
/// **Catalogue entries refuse here too.** They were let through on the reading
/// that this guard was about features, which is the function's old name and
/// not its job: `omh settings set myskill on` put `myskill = "on"` into
/// `default.toml` — the file **every new repo is seeded from** — while
/// `omh set myskill on` refused the same word. One name, two answers, and the
/// wrong one is the one that persists.
pub(crate) fn no_legacy_write_over_a_name_omh_owns(
    paths: &Paths,
    name: &str,
    ctx: &out::Ctx,
) -> Result<()> {
    match names(paths, name, ctx) {
        Names::AFeature => anyhow::bail!(
            "`{name}` is one of omh's features, not a setting — a bare key of \
             that name is read by nothing.\n  omh set {name} off"
        ),
        Names::AnEntryOf(feature) => Err(an_entry_is_not_a_feature(name, &feature)),
        Names::ACatalogueEntry(cap) => Err(a_catalogue_entry_is_not_a_setting(name, cap)),
        Names::ASetting | Names::Neither => Ok(()),
    }
}

/// A catalogue entry is selected, not set. Beside `an_entry_is_not_a_feature`
/// so the two refusals stay one shape.
pub(crate) fn a_catalogue_entry_is_not_a_setting(
    name: &str,
    cap: adapter::Capability,
) -> anyhow::Error {
    anyhow::anyhow!(
        "`{name}` is one of your {cap}, not a setting — what a project takes \
         from your catalogue is `omh use`.\n  omh use {cap} {name}\n  \
         omh unuse {cap} {name}"
    )
}

/// The grouping, said where somebody has just guessed at it.
///
/// Kept beside `names` so the two orders stay visible together.
pub(crate) fn an_entry_is_not_a_feature(name: &str, feature: &str) -> anyhow::Error {
    anyhow::anyhow!(
        "`{name}` is part of the `{feature}` feature, not a feature itself. \
         A feature is all or nothing.\n  omh set {feature} off"
    )
}

/// `on` or `off`, and nothing else.
///
/// Not `true`/`false`: the file holds a TOML boolean, but the command is a
/// switch and `omh set codegraph true` reads as somebody guessing at a
/// serialisation. Refused rather than accepted-and-normalised, because the
/// refusal is where the two words get taught.
pub(crate) fn on_or_off(feature: &str, value: &str) -> Result<bool> {
    match value {
        "on" => Ok(true),
        "off" => Ok(false),
        other => anyhow::bail!(
            "`{feature}` is one of omh's features, so it is on or off — \
             `{other}` is neither.\n  omh set {feature} off"
        ),
    }
}

/// Say so when a write cannot be seen, because a layer above it already wins.
///
/// Unreachable without a flag: the rule reaches every repo layer that already
/// holds the key, so an unadorned write is never outranked. `--save` and
/// `--local` walk past that on purpose — you named the file, and honouring
/// that is right — so this is the case they leave behind. `omh set --save
/// idle_timeout 30m` over a standing local `15m` writes the committed file,
/// exits 0, and `omh info --repo` still says `15m`. Doing it silently was the bug.
pub(crate) fn say_if_shadowed(
    paths: &Paths,
    key: &str,
    wrote: &[config::Layer],
    ctx: &out::Ctx,
) -> Result<()> {
    let Some(winning) = config::policy(paths)?.into_iter().find(|s| s.key == key) else {
        return Ok(());
    };
    if wrote.contains(&winning.layer) {
        return Ok(());
    }
    ctx.warn(&format!(
        "`{key}` is still {} here — the {} layer sets it, and that outranks what was written",
        winning.value, winning.layer
    ));
    Ok(())
}

pub(crate) fn set(
    paths: &Paths,
    key: &str,
    value: &str,
    reach: Reach,
    dry_run: bool,
    ctx: &out::Ctx,
) -> Result<()> {
    // Written either way. A settings file is hand-editable and a key a newer
    // omh will read must not be refused by this one — but a key *this* omh
    // reads nothing from looks identical to one that took, and `carry_ins` is
    // a plausible thing to type. Named, not refused.
    match key::describes(key) {
        None => ctx.warn(&format!(
            "nothing in omh reads `{key}` — it is written, and it will sit there"
        )),
        // Written either way, for the same reason: a value a newer omh will
        // accept must not be refused by this one. But `persistence = tmux`
        // otherwise surfaces at the next launch, in a different command,
        // minutes later.
        Some(k) => {
            if let Some(quarrel) = key::quarrel(k, value) {
                ctx.warn(&format!("{quarrel} — written anyway"));
            }
        }
    }
    if dry_run {
        for layer in &reach.layers {
            ctx.say(
                &report::Action::new(
                    "setting-planned",
                    format!(
                        "would write {key} → {} ({})",
                        layer.file(paths).display(),
                        tracked(*layer)
                    ),
                )
                .data(serde_json::json!({
                    "key": key,
                    "value": value,
                    "layer": layer.to_string(),
                    "committed": layer.is_committed(),
                    "path": layer.file(paths).display().to_string(),
                })),
            );
        }
        return Ok(());
    }
    if reach.layers.contains(&config::Layer::Local) {
        crate::cmd::init::ensure_ignored(paths, ctx)?;
    }
    // Every layer the rule named, so a write cannot land under a value that
    // outranks it. That is the whole reason the rule returns a list.
    for layer in &reach.layers {
        let w = config::set(paths, key, value, *layer)?;
        // The one fact separating the safe destination from the dangerous one
        // used to be a five-character infix in an absolute path eighty columns
        // wide. `settings.toml` and `settings.local.toml` do not read as
        // opposites at a glance.
        ctx.say(
            &report::Action::new(
                "setting-written",
                format!("wrote → {} ({})", w.path.display(), tracked(w.layer)),
            )
            .data(serde_json::json!({
                "key": key,
                "value": value,
                "layer": w.layer.to_string(),
                "committed": w.committed,
                "path": w.path.display().to_string(),
            })),
        );
    }
    // The one mistake git makes unrecoverable. On stderr through `warn`, so it
    // survives `omh set … > log` — which is exactly the invocation a script
    // that is about to commit a secret would use.
    if reach.committed() {
        match reach.why {
            // Somebody named the file git carries.
            Why::Named => ctx.warn("the committed file is COMMITTED — never put a secret here"),
            // omh sent a key it has never heard of to a file git carries,
            // because that is what it does with an unclassified key. The line
            // above already said nothing reads it; this is the half of that
            // which retyping does not undo.
            Why::Unclassified => ctx.warn(&format!(
                "and the committed file is COMMITTED — `{key}` went into a file git carries"
            )),
            // The key was already there, or the registry judged the file safe
            // for it. Neither is news.
            Why::JoinsAStandingValue | Why::Registry => {}
        }
        // The general sentence fired for `account` — a name — exactly as it did
        // for `carry_in`, and a warning that cannot tell those apart is one
        // people learn to scroll past. Where omh knows the key reaches a
        // credential, it says so and says where the value would have gone.
        if let Some(k) = key::describes(key) {
            if k.secret == key::Secret::Yes {
                ctx.warn(&format!(
                    "  `{key}` is one of those — it belongs in {}",
                    k.default_layer().file(paths).display()
                ));
            }
        }
    }
    // Not for the template. "Outranks" is a claim about resolution, and the
    // template is not in it — saying a repo value beats your default is the
    // exact confusion this release removed, printed by the command that owns
    // the file.
    if reach.layers == [config::Layer::Personal] {
        return Ok(());
    }
    say_if_shadowed(paths, key, &reach.layers, ctx)
}

/// What this layer's file is, in the words a person needs at the moment of a
/// write.
///
/// Three answers, not two. `is_committed()` is a boolean and the template is
/// neither thing it distinguishes: it is not in a repo, so calling it
/// "gitignored" is true of nothing and answers a question nobody asked. What
/// matters about it is when it has an effect.
pub(crate) fn tracked(layer: config::Layer) -> &'static str {
    match layer {
        config::Layer::Shared => "committed",
        config::Layer::Local => "gitignored",
        config::Layer::Personal => "seeds new repos",
    }
}

/// Remove a setting from every layer that was asked for.
///
/// Reports per layer, and then says what still stands. Both halves earned
/// their place from the same defect: `omh unset carry_in` removed the
/// gitignored copy, said so, exited 0, and left a committed `carry_in` — a map
/// to a credential — effective. Removing from every declaring repo layer fixes
/// the case omh itself creates; the survivor line covers the rest, because an
/// unqualified `unset` deliberately does not reach into your personal file and
/// a person who cannot see why the value persists has been told nothing.
pub(crate) fn unset(
    paths: &Paths,
    key: &str,
    reach: Reach,
    dry_run: bool,
    ctx: &out::Ctx,
) -> Result<()> {
    let reach = reach.for_removal();
    if reach.layers.is_empty() {
        ctx.say(
            &report::Action::new("setting-absent", format!("{key} is not set in this repo"))
                .data(serde_json::json!({ "key": key, "removed": false })),
        );
        if dry_run {
            return Ok(());
        }
    }
    for layer in &reach.layers {
        if dry_run {
            ctx.say(
                &report::Action::new(
                    "setting-removal-planned",
                    format!(
                        "would drop {key} from the {layer} layer ({})",
                        tracked(*layer)
                    ),
                )
                .data(serde_json::json!({ "key": key, "layer": layer.to_string() })),
            );
            continue;
        }
        let removed = config::unset(paths, key, *layer)?;
        ctx.say(
            &report::Action::new(
                if removed {
                    "setting-removed"
                } else {
                    "setting-absent"
                },
                if removed {
                    format!("removed {key} from the {layer} layer")
                } else {
                    format!("{key} was not set in the {layer} layer")
                },
            )
            .data(serde_json::json!({
                "key": key,
                "layer": layer.to_string(),
                "removed": removed,
            })),
        );
    }
    if dry_run {
        return Ok(());
    }
    // What a person asked for was for the setting to stop applying. Anything
    // still supplying it is the answer to "why did nothing change" — and with
    // a named flag it is reachable, since `--save` and `--local` deliberately
    // touch one file. Without one the rule reached every repo layer, so the
    // only survivor left is a personal default, which no repo command writes.
    if let Some(still) = config::policy(paths)?.into_iter().find(|s| s.key == key) {
        ctx.warn(&format!(
            "`{key}` is still set in the {} layer — {}",
            still.layer,
            still.layer.file(paths).display()
        ));
        if still.layer.is_committed()
            && key::describes(key).is_some_and(|k| k.secret == key::Secret::Yes)
        {
            ctx.warn(&format!(
                "  and that file is COMMITTED — `omh unset --save {key}` drops it there"
            ));
        }
    }
    Ok(())
}

/// `$EDITOR` on your settings, or on one catalogue entry.
///
/// Once `$EDITOR` is spawned it is a full program running as you, and any fence
/// omh drew around it would be decorative — there is no trust boundary between
/// omh and the person whose home directory this is. The boundary that matters
/// is structural and already there: every catalogue directory a sandbox is given
/// is mounted **read-only**.
///
/// This used to say `~/.omh` is not mounted at all, which is simply false —
/// `container.rs` binds each catalogue source at `/omh/layers/<n>/<cap>` — and
/// it is the kind of claim a reader takes on trust. Read-only is the true
/// version and carries the same argument.
///
/// What does need a guard is the **name**, the moment this takes one and joins
/// it to a directory: `omh settings edit skills ../../../.ssh/id_rsa` is
/// traversal. Same rule and same function as `[use]` uses, because it is the
/// same act — a name being minted.
/// Takes the `Paths` its caller resolved, for the reason `mcp` does: the file
/// it opens is `~/.omh/default.toml` or a directory under `~/.omh`, and
/// `Paths::discover` made both unreachable outside a checkout.
pub(crate) fn edit(
    paths: &Paths,
    capability: Option<&str>,
    name: Option<&str>,
    layer: config::Layer,
) -> Result<()> {
    let file = match capability {
        None => layer.file(paths),
        Some(key) => {
            let cap = adapter::Capability::from_key(key).with_context(|| {
                format!(
                    "`{key}` is not a capability — expected {}",
                    capability_list()
                )
            })?;
            let dir = paths.root.join(cap.source());
            match name {
                None => dir,
                Some(name) => {
                    selection::validate_entry_name(name, cap, &dir)?;
                    dir.join(name)
                }
            }
        }
    };
    if let Some(parent) = file.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let editor = std::env::var("EDITOR").unwrap_or_else(|_| "vi".into());
    Command::new(editor).arg(&file).status()?;
    Ok(())
}

pub(crate) fn capability_list() -> String {
    adapter::Capability::ALL
        .iter()
        .map(adapter::Capability::to_string)
        .collect::<Vec<_>>()
        .join(", ")
}

/// Switch one of omh's features on or off in this checkout.
///
/// Reached from `omh set <feature> on|off`, which since 0.7.0 is the only
/// caller — `enable`/`disable` were the other two and are retired. It writes
/// the `[omh]` table rather than a bare key, which is the whole reason `names`
/// exists: a feature written as a settings key is read by nothing, and looks
/// in the file exactly like one that took.
///
/// The layers come from the same rule every other write uses, so a switch
/// cannot land under a value that outranks it — `omh use` and `omh unuse`
/// answer the same question about `[use]`.
pub(crate) fn feature_switch(
    paths: &Paths,
    feature: &str,
    on: bool,
    reach: Reach,
    dry_run: bool,
    ctx: &out::Ctx,
) -> Result<()> {
    let manifest = base::Manifest::load_dir(&paths.base())?;
    let features: std::collections::BTreeSet<&str> = manifest
        .entries
        .iter()
        .map(|e| e.feature.as_str())
        .collect();
    // **Unreachable through today's only caller, and kept.** `names` reads
    // this same manifest from this same path and returns `AFeature` only when
    // it holds the name, so `omh set` cannot arrive here with a word this
    // check would refuse. What it stops is the *next* caller: without it, a
    // route that skipped `names` would write `[omh] nosuchfeature = true`,
    // which is a file that looks like it took and a feature that never
    // existed — the defect this whole fork was built to end, one door over.
    if !features.contains(feature) {
        // The entry-name case is the interesting error: it is how somebody
        // discovers the grouping without reading the manifest.
        if let Some(entry) = manifest.entry(feature) {
            return Err(an_entry_is_not_a_feature(feature, &entry.feature));
        }
        anyhow::bail!(
            "`{feature}` is not one of omh's features ({}). \
             A catalogue entry of yours is `omh use`/`omh unuse`.",
            features.into_iter().collect::<Vec<_>>().join(", ")
        );
    }
    if dry_run {
        // `--dry-run` used to be accepted and discarded here, so
        // `omh --dry-run set codegraph off` wrote a committed file and printed
        // `wrote →`. Its sibling arm in the same match honoured the flag,
        // which is how somebody learns the wrong lesson about this one.
        for layer in &reach.layers {
            ctx.say(
                &report::Action::new(
                    "feature-switch-planned",
                    format!(
                        "would switch {feature} {} in the {layer} layer ({})",
                        if on { "on" } else { "off" },
                        tracked(*layer)
                    ),
                )
                .data(serde_json::json!({
                    "feature": feature,
                    "on": on,
                    "layer": layer.to_string(),
                    "committed": layer.is_committed(),
                    "path": layer.file(paths).display().to_string(),
                })),
            );
        }
        return Ok(());
    }
    if reach.layers.contains(&config::Layer::Local) {
        crate::cmd::init::ensure_ignored(paths, ctx)?;
    }
    let mut written = Vec::new();
    for layer in &reach.layers {
        written.push(config::write_feature(paths, *layer, feature, on)?);
    }
    let wrote = crate::cmd::catalogue::written_paths(&written);
    let mut action = report::Action::new(
        if on { "feature-on" } else { "feature-off" },
        format!("{feature} is {} here", if on { "on" } else { "off" }),
    )
    .data(serde_json::json!({
        "feature": feature,
        "on": on,
        "paths": wrote,
    }));
    if !on {
        action = action.note("nothing was uninstalled; the next repo gets it back");
    }
    for w in &written {
        action = action.note(format!(
            "wrote → {} ({})",
            w.path.display(),
            tracked(w.layer)
        ));
    }
    ctx.say(&action);
    // The sibling arm says so; this one said nothing at all, on either stream,
    // while writing a file git carries.
    //
    // No `say_what_still_switches` here, deliberately: after a switch, what is
    // still switching it is what you just typed. That sentence answers "why did
    // nothing change", which is a question a *removal* raises and a write does
    // not.
    // **No COMMITTED warning here.** `Why` was built to keep that sentence
    // rare, and its own doc gives the reason: it is nearly every write now that
    // the committed file is the default, and a sentence that fires on almost
    // every invocation is one people stop reading — which this codebase has
    // already watched happen once, to this same sentence.
    //
    // `set <key> <value>` got the narrowing and this arm did not, so it fired
    // on every feature switch. A switch is *always* a committed write, and
    // committing it is the point: what a repo does with omh's features is a
    // fact about the repo, and a teammate cloning it should get the same one.
    // The line below already says so, in the voice of an outcome rather than a
    // warning about a secret.
    Ok(())
}
