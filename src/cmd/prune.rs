//! What omh has left behind, and removing what nothing claims.
//!
//! omh creates things that outlive the checkout that made them — cache
//! volumes, containers, networks, images, and the per-checkout state under
//! `~/.omh`. Every one of them is keyed by `repo_id`, a one-way digest of the
//! checkout's path, so until the checkout registry landed omh could see that
//! they existed and never whose they were.
//!
//! **The rule this module is built on:** a removal is licensed by evidence
//! that the owning checkout is *gone*, and by nothing else. "omh has no record
//! of this" is not that evidence — on the first run after the registry lands,
//! nothing on the machine has a record, and reading that as "deleted" would
//! take everything. The bucket for it is `kept_unknown`, and only a flag whose
//! name says `dangerously` reaches it.

use crate::profile::Attribution;

/// What kind of thing this is. The report groups by it, and each class is
/// removed a different way.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Class {
    Volume,
    Container,
    Network,
    Image,
    /// A directory under `~/.omh` keyed by `repo_id`.
    State,
    /// A `tmp.*` remnant of an aborted operation. Owned by no checkout by
    /// construction, so it needs no attribution at all.
    Temp,
}

impl Class {
    pub fn noun(&self) -> &'static str {
        match self {
            Class::Volume => "volume",
            Class::Container => "container",
            Class::Network => "network",
            Class::Image => "image",
            Class::State => "directory",
            Class::Temp => "leftover",
        }
    }
}

/// One thing omh left behind.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Item {
    pub class: Class,
    /// What to say, and for most classes what to remove.
    pub name: String,
    /// The `repo_id` this is keyed by, when it has one. `Temp` has none.
    pub id: Option<String>,
}

/// What a run would do, split by why.
///
/// Every bucket is reported on every run, including the empty ones: a bucket
/// omitted when empty reads as a fact, and "nothing omh could not attribute"
/// is a different claim from silence.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Plan {
    /// Provably safe: the owning checkout is recorded and gone, and nothing
    /// here holds work.
    pub remove: Vec<Item>,
    /// The owning checkout is still on this machine. Theirs to keep.
    pub kept_live: Vec<Item>,
    /// omh could not attribute it, with why.
    pub kept_unknown: Vec<(Item, String)>,
    /// Attributed and gone, but it holds work nobody has reviewed — or omh
    /// could not tell whether it does.
    pub kept_unsafe: Vec<(Item, String)>,
    /// A class omh could not enumerate. **Not the same as empty**, and the
    /// reason a run that could not look must not read as a tidy machine.
    pub could_not_list: Vec<String>,
}

/// Decide, over answers someone else went and got.
///
/// Pure: the attributor and the work-check are injected, because the part that
/// fails silently is this decision and the part that fails loudly is the
/// shelling out.
pub fn plan(
    items: Vec<Item>,
    could_not_list: Vec<String>,
    attribute: &dyn Fn(&str) -> Attribution,
    holds_work: &dyn Fn(&Item) -> Option<String>,
    include_unsafe: bool,
) -> Plan {
    let mut out = Plan {
        could_not_list,
        ..Plan::default()
    };
    for item in items {
        // A `tmp.*` remnant is owned by nobody by construction: it is the
        // debris of an operation that did not finish, and there is no checkout
        // it could belong to. The one class needing no attribution.
        let attribution = match &item.id {
            None => Attribution::Gone(std::path::PathBuf::new()),
            Some(id) => attribute(id),
        };
        match attribution {
            Attribution::Live(_) => out.kept_live.push(item),
            Attribution::Unknown(why) => out.kept_unknown.push((item, why)),
            Attribution::Gone(_) => match holds_work(&item) {
                Some(why) => out.kept_unsafe.push((item, why)),
                None => out.remove.push(item),
            },
        }
    }
    // Widened only here, and only ever by an explicit flag. Note this moves
    // whole buckets rather than re-deciding them: what made something unsafe
    // is still true, and is still what the prompt reads out.
    if include_unsafe {
        for (item, _) in std::mem::take(&mut out.kept_unknown) {
            out.remove.push(item);
        }
        for (item, _) in std::mem::take(&mut out.kept_unsafe) {
            out.remove.push(item);
        }
    }
    out
}

/// The per-repo directories omh writes, and whether losing one can lose work.
///
/// Named here rather than discovered, so a class added to `Paths` without
/// being added here fails `every_class_omh_writes_appears_in_the_report`
/// instead of quietly never being reported.
pub const STATE_DIRS: &[(&str, bool)] = &[
    // (directory under ~/.omh, can losing it lose something nobody can rebuild)
    //
    // **`notes` is `true`, and driving the real binary is what moved it.** A
    // dry run put `notes/<gone checkout>` straight in the remove set: the
    // checkout is gone, so by the letter of the rule it is an orphan. But a
    // note is something a person wrote, not state omh derived — the store's own
    // doc says it outlives the session on purpose — and the difference between
    // "orphaned" and "worthless" is exactly the difference this flag exists to
    // respect. It goes only through the prompt, which names it.
    ("worktrees", true),
    ("shadow", true),
    ("notes", true),
    // Derived on every launch, and rebuilt without asking anyone.
    ("run", false),
    ("scratch", false),
    // An ssh key for a checkout that no longer exists is a credential left
    // lying around, so removing it is the safe direction rather than the risky
    // one. Nothing is lost that omh cannot mint again.
    ("keys", false),
];

/// Everything omh has left on this machine, and the classes it could not read.
///
/// Machine-wide on purpose: an orphan outlives the checkout that made it, so a
/// per-checkout listing structurally cannot see the ones that matter.
pub fn inventory(
    root: &std::path::Path,
    backend: Option<&dyn crate::runtime::Runtime>,
) -> (Vec<Item>, Vec<String>) {
    let mut items = Vec::new();
    let mut blind = Vec::new();

    for (dir, _) in STATE_DIRS {
        let at = root.join(dir);
        match std::fs::read_dir(&at) {
            // Never made is not a failure to look.
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => blind.push(format!("omh could not read {}: {e}", at.display())),
            Ok(entries) => {
                for entry in entries {
                    match entry {
                        Err(e) => blind.push(format!(
                            "an entry under {} could not be read ({e})",
                            at.display()
                        )),
                        Ok(e) => {
                            let name = e.file_name().to_string_lossy().into_owned();
                            let path = e.path().display().to_string();
                            // `tmp.*` is the debris of an operation that did
                            // not finish. It is keyed by nothing, so it is
                            // owned by nobody — an answer, not a gap.
                            if name.starts_with("tmp.") {
                                items.push(Item {
                                    class: Class::Temp,
                                    name: path,
                                    id: None,
                                });
                            } else {
                                items.push(Item {
                                    class: Class::State,
                                    name: path,
                                    id: Some(name),
                                });
                            }
                        }
                    }
                }
            }
        }
    }

    match backend {
        None => blind.push(
            "there is no container runtime to ask about volumes, containers or networks".into(),
        ),
        Some(b) => {
            list(
                b,
                b.volume_args(),
                "volumes",
                &mut blind,
                |n| {
                    n.strip_prefix("omh-cache-").map(|id| Item {
                        class: Class::Volume,
                        name: n.to_string(),
                        id: Some(id.to_string()),
                    })
                },
                &mut items,
            );
            list(
                b,
                b.running_all_args(),
                "containers",
                &mut blind,
                |n| {
                    id_in_container(n).map(|id| Item {
                        class: Class::Container,
                        name: n.to_string(),
                        id: Some(id),
                    })
                },
                &mut items,
            );
            images(root, b, &mut blind, &mut items);
            list(
                b,
                b.network_args(),
                "networks",
                &mut blind,
                |n| {
                    n.strip_prefix("omh-")
                        .filter(|r| !r.starts_with("cache-"))
                        .map(|id| Item {
                            class: Class::Network,
                            name: n.to_string(),
                            id: Some(id.to_string()),
                        })
                },
                &mut items,
            );
        }
    }
    (items, blind)
}

/// Images that record the checkout they were built for.
///
/// Keyed by content hash rather than `repo_id`, so the usual route does not
/// reach them. Only stack images carry `omh.repo`; a base or harness image is
/// shared across checkouts and belongs to no single one, which is why omh does
/// not claim it belongs to any.
fn images(
    root: &std::path::Path,
    backend: &dyn crate::runtime::Runtime,
    blind: &mut Vec<String>,
    items: &mut Vec<Item>,
) {
    let Some(args) = backend.image_args() else {
        blind.push("omh has not measured how this runtime lists images".into());
        return;
    };
    let ids = match std::process::Command::new(backend.program())
        .args(&args)
        .output()
    {
        Err(e) => return blind.push(format!("omh could not list images: {e}")),
        Ok(out) if !out.status.success() => {
            return blind.push(format!(
                "omh could not list images: {}",
                crate::image::unreadable(&String::from_utf8_lossy(&out.stderr), &out.status)
            ))
        }
        Ok(out) => String::from_utf8_lossy(&out.stdout)
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty())
            .map(str::to_string)
            .collect::<Vec<_>>(),
    };
    if ids.is_empty() {
        return;
    }
    // One inspect for all of them: the label is the checkout path, and the
    // path is what `attribution_of` cannot use — so the id is recorded here
    // and the caller attributes by path instead.
    let mut inspect = vec![
        "image".to_string(),
        "inspect".into(),
        "--format".into(),
        "{{index .Config.Labels \"omh.repo\"}}\t{{index .RepoTags 0}}".into(),
    ];
    inspect.extend(ids);
    match std::process::Command::new(backend.program())
        .args(&inspect)
        .output()
    {
        Err(e) => blind.push(format!("omh could not inspect images: {e}")),
        Ok(out) if !out.status.success() => blind.push(format!(
            "omh could not inspect images: {}",
            crate::image::unreadable(&String::from_utf8_lossy(&out.stderr), &out.status)
        )),
        Ok(out) => {
            for line in String::from_utf8_lossy(&out.stdout).lines() {
                if let Some((repo, tag)) = line.trim().split_once('\t') {
                    if repo.is_empty() || tag.is_empty() {
                        continue;
                    }
                    // **The label is evidence, so record it.** Turning the
                    // path into an id and then looking the id up would throw
                    // away the one thing the label gave us. Written to the
                    // registry instead, which attributes this image *and*
                    // every volume, container, network and directory keyed by
                    // the same checkout — one label pays for all of them.
                    let path = std::path::Path::new(repo);
                    let id = crate::profile::id_for(path);
                    let _ = crate::profile::remember(root, &id, path);
                    items.push(Item {
                        class: Class::Image,
                        name: tag.to_string(),
                        id: Some(id),
                    });
                }
            }
        }
    }
}

/// The `repo_id` inside a container name.
///
/// `omh-<repo_id>-sNN` and `omh-graph-<repo_id>`. Written as a function
/// because getting it wrong silently attributes somebody else's container.
pub fn id_in_container(name: &str) -> Option<String> {
    let rest = name.strip_prefix("omh-")?;
    if let Some(graph) = rest.strip_prefix("graph-") {
        return Some(graph.to_string());
    }
    // A session suffix, and only a session suffix: `-s` followed by digits.
    let (id, tail) = rest.rsplit_once("-s")?;
    tail.chars()
        .all(|c| c.is_ascii_digit())
        .then(|| id.to_string())
        .filter(|id| !id.is_empty())
}

/// Run one listing, and record it as unreadable rather than empty when it
/// fails — the distinction the whole report rests on.
fn list(
    backend: &dyn crate::runtime::Runtime,
    args: Option<Vec<String>>,
    what: &str,
    blind: &mut Vec<String>,
    into: impl Fn(&str) -> Option<Item>,
    items: &mut Vec<Item>,
) {
    let Some(args) = args else {
        blind.push(format!(
            "omh has not measured how this runtime lists {what}"
        ));
        return;
    };
    match std::process::Command::new(backend.program())
        .args(&args)
        .output()
    {
        Err(e) => blind.push(format!("omh could not list {what}: {e}")),
        Ok(out) if !out.status.success() => blind.push(format!(
            "omh could not list {what}: {}",
            crate::image::unreadable(&String::from_utf8_lossy(&out.stderr), &out.status)
        )),
        Ok(out) => items.extend(
            String::from_utf8_lossy(&out.stdout)
                .lines()
                .map(str::trim)
                .filter(|n| !n.is_empty())
                .filter_map(&into),
        ),
    }
}

/// Whether removing this could lose work nobody has reviewed.
///
/// Conservative on purpose: a worktree or a sandbox repository is treated as
/// work-bearing unless it is empty, because the alternative is omh deciding
/// that somebody's uncommitted afternoon was not important. What this produces
/// is not only a gate — it is the text of the question the dangerous flag
/// asks, so it has to say what is actually there.
pub fn holds_work(item: &Item) -> Option<String> {
    if item.class != Class::State {
        return None;
    }
    let path = std::path::Path::new(&item.name);
    let bearing = STATE_DIRS
        .iter()
        .any(|(dir, work)| *work && path.parent().is_some_and(|p| p.ends_with(dir)));
    if !bearing {
        return None;
    }
    match std::fs::read_dir(path) {
        Err(e) => Some(format!("omh could not read it to tell what it holds ({e})")),
        Ok(entries) => {
            let held: Vec<String> = entries
                .flatten()
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .collect();
            (!held.is_empty()).then(|| {
                format!(
                    "holds {} thing{} omh cannot vouch for ({})",
                    held.len(),
                    if held.len() == 1 { "" } else { "s" },
                    held.join(", ")
                )
            })
        }
    }
}

/// What the run did, or would do — every bucket, every time.
///
/// **No bucket is omitted when empty.** A report that prints only what it
/// found reads as an inventory; one that also prints "nothing omh could not
/// attribute" is telling you it looked. Those are different claims, and the
/// second is the one somebody deciding whether to trust the first needs.
pub fn render(plan: &Plan, went: &[(Item, Option<String>)], dry_run: bool) -> String {
    let mut out = String::new();
    let line = |out: &mut String, s: String| {
        out.push_str(&s);
        out.push('\n');
    };

    if dry_run {
        line(
            &mut out,
            format!(
                "would remove {} — nothing has been removed",
                count(&plan.remove)
            ),
        );
        for item in &plan.remove {
            line(
                &mut out,
                format!("  {:<10} {}", item.class.noun(), item.name),
            );
        }
    } else {
        let gone: Vec<&Item> = went
            .iter()
            .filter(|(_, why)| why.is_none())
            .map(|(i, _)| i)
            .collect();
        line(&mut out, format!("removed {}", count_ref(&gone)));
        for item in &gone {
            line(
                &mut out,
                format!("  {:<10} {}", item.class.noun(), item.name),
            );
        }
        // A removal that failed is news, not silence — the lesson from `rm`
        // reporting removals it had not performed.
        let stuck: Vec<&(Item, Option<String>)> =
            went.iter().filter(|(_, why)| why.is_some()).collect();
        if !stuck.is_empty() {
            line(&mut out, format!("would not go ({})", stuck.len()));
            for (item, why) in &stuck {
                line(
                    &mut out,
                    format!(
                        "  {:<10} {} — {}",
                        item.class.noun(),
                        item.name,
                        why.as_deref().unwrap_or("")
                    ),
                );
            }
        }
    }

    line(
        &mut out,
        format!(
            "left {}",
            count_ref(
                &plan
                    .kept_live
                    .iter()
                    .chain(plan.kept_unknown.iter().map(|(i, _)| i))
                    .chain(plan.kept_unsafe.iter().map(|(i, _)| i))
                    .collect::<Vec<_>>()
            )
        ),
    );
    line(
        &mut out,
        format!(
            "  {:<4} belong to checkouts still on this machine",
            plan.kept_live.len()
        ),
    );
    line(
        &mut out,
        format!("  {:<4} omh could not attribute", plan.kept_unknown.len()),
    );
    say_grouped(&mut out, &plan.kept_unknown);
    line(
        &mut out,
        format!("  {:<4} omh cannot vouch for", plan.kept_unsafe.len()),
    );
    say_grouped(&mut out, &plan.kept_unsafe);
    if !plan.kept_unknown.is_empty() || !plan.kept_unsafe.is_empty() {
        line(
            &mut out,
            "  those need `omh prune --dangerously-include-unsafe`, which names each one and asks first"
                .into(),
        );
    }

    // **Said even when nothing failed.** "every class was read" is the fact
    // that makes the numbers above mean anything.
    if plan.could_not_list.is_empty() {
        line(&mut out, "unread nothing — every class was listed".into());
    } else {
        line(
            &mut out,
            format!(
                "unread {} class{} omh could not list, so the counts above are a floor:",
                plan.could_not_list.len(),
                if plan.could_not_list.len() == 1 {
                    ""
                } else {
                    "es"
                }
            ),
        );
        for why in &plan.could_not_list {
            line(&mut out, format!("  {why}"));
        }
    }
    out
}

/// Say a bucket without turning the report into a wall.
///
/// **Measured by running it.** The first version printed one line per item
/// with its reason repeated — 496 identical sentences on this machine, which
/// is unreadable and pushes everything else off the screen. It is the same
/// mistake the leftovers row already made and fixed, in a new place.
///
/// Grouped by reason, because the reason is what differs and what a person
/// acts on. Items are named only while there are few enough to read; past
/// that the count is the fact, with a couple by name so the reader can
/// recognise the shape.
fn say_grouped(out: &mut String, items: &[(Item, String)]) {
    const NAMES_UP_TO: usize = 8;
    let mut by_reason: Vec<(&str, Vec<&Item>)> = Vec::new();
    for (item, why) in items {
        match by_reason.iter_mut().find(|(r, _)| *r == why.as_str()) {
            Some((_, group)) => group.push(item),
            None => by_reason.push((why.as_str(), vec![item])),
        }
    }
    for (why, group) in by_reason {
        if group.len() <= NAMES_UP_TO {
            for item in &group {
                out.push_str(&format!("         {} — {why}\n", item.name));
            }
        } else {
            out.push_str(&format!("         {} of them — {why}\n", group.len()));
            for item in group.iter().take(2) {
                out.push_str(&format!("           e.g. {}\n", item.name));
            }
        }
    }
}

fn count(items: &[Item]) -> String {
    format!(
        "{} thing{}",
        items.len(),
        if items.len() == 1 { "" } else { "s" }
    )
}

fn count_ref(items: &[&Item]) -> String {
    format!(
        "{} thing{}",
        items.len(),
        if items.len() == 1 { "" } else { "s" }
    )
}

/// Remove one thing, then **ask whether it went**.
///
/// The exit code is not the answer: `rm` trusted `git worktree remove`'s and
/// reported removals it had not performed. Every class is re-asked here, and
/// what the report prints is what omh observed.
pub fn remove_one(item: &Item, backend: Option<&dyn crate::runtime::Runtime>) -> Option<String> {
    let args: Vec<String> = match item.class {
        Class::State | Class::Temp => {
            let path = std::path::Path::new(&item.name);
            let why = std::fs::remove_dir_all(path).err();
            return match crate::session::still_there(path) {
                Ok(false) => None,
                Err(e) => Some(format!("omh could not tell whether it is still there: {e}")),
                Ok(true) => Some(match why {
                    Some(e) => format!("{e}"),
                    None => "it is still there".to_string(),
                }),
            };
        }
        Class::Volume => vec!["volume".into(), "rm".into(), item.name.clone()],
        Class::Container => vec!["rm".into(), "-f".into(), item.name.clone()],
        Class::Network => vec!["network".into(), "rm".into(), item.name.clone()],
        Class::Image => vec!["image".into(), "rm".into(), item.name.clone()],
    };
    let Some(backend) = backend else {
        return Some("there is no container runtime to remove it with".into());
    };
    match std::process::Command::new(backend.program())
        .args(&args)
        .output()
    {
        Err(e) => Some(format!("{e}")),
        Ok(out) if !out.status.success() => Some(crate::image::unreadable(
            &String::from_utf8_lossy(&out.stderr),
            &out.status,
        )),
        Ok(_) => None,
    }
}

/// The run, as one value both audiences read.
///
/// A `Report` rather than a printed string, because `--json` has to carry the
/// same structure. `leftovers` warned to stderr and a `--json` consumer saw no
/// trace of it — a warning on stderr is not a report.
pub struct Pruned {
    pub plan: Plan,
    pub went: Vec<(Item, Option<String>)>,
    pub dry_run: bool,
}

impl crate::out::Report for Pruned {
    fn human(&self, _p: &crate::out::Palette) -> String {
        render(&self.plan, &self.went, self.dry_run)
    }

    fn json(&self) -> serde_json::Value {
        let one = |i: &Item| serde_json::json!({ "class": i.class.noun(), "name": i.name, "repo_id": i.id });
        let withwhy = |(i, w): &(Item, String)| serde_json::json!({ "class": i.class.noun(), "name": i.name, "repo_id": i.id, "why": w });
        serde_json::json!({
            "dry_run": self.dry_run,
            "removed": self.went.iter().filter(|(_, w)| w.is_none()).map(|(i, _)| one(i)).collect::<Vec<_>>(),
            "would_not_go": self.went.iter().filter(|(_, w)| w.is_some())
                .map(|(i, w)| serde_json::json!({ "class": i.class.noun(), "name": i.name, "why": w }))
                .collect::<Vec<_>>(),
            "would_remove": self.dry_run.then(|| self.plan.remove.iter().map(one).collect::<Vec<_>>()),
            "kept_live": self.plan.kept_live.len(),
            "kept_unknown": self.plan.kept_unknown.iter().map(withwhy).collect::<Vec<_>>(),
            "kept_unsafe": self.plan.kept_unsafe.iter().map(withwhy).collect::<Vec<_>>(),
            // Named `could_not_list` and always present, because a consumer
            // reading `kept_*` needs to know whether the counts are complete.
            "could_not_list": self.plan.could_not_list,
        })
    }
}

/// `omh prune`.
///
/// Bare, it removes what is provably safe and reports what went and what is
/// left. The safe set is safe *by construction* — the owning checkout is
/// recorded and verified absent, and nothing in it holds work — so there is no
/// `--yes` over it: a confirmation on something always safe is what teaches
/// people to confirm without reading, and then the one that matters is
/// answered the same way.
pub fn prune_cmd(
    cwd: &std::path::Path,
    dry_run: bool,
    include_unsafe: bool,
    ctx: &crate::out::Ctx,
    input: &mut dyn std::io::BufRead,
    out: &mut dyn std::io::Write,
) -> anyhow::Result<()> {
    let paths = crate::profile::Paths::discover(cwd)?;
    let backend = crate::runtime::select(&crate::runtime_preference(&paths), &|p| {
        crate::runtime::installed(p)
    })
    .ok();
    let (items, blind) = inventory(&paths.root, backend.as_deref());

    // Decided without the flag first, so the question can name what the flag
    // would reach. Deciding with it and then asking would be asking about a
    // set the user cannot see.
    let shown = plan(
        items,
        blind,
        &|id| crate::profile::attribution_of(&paths.root, id),
        &holds_work,
        false,
    );

    let mut agreed = false;
    if include_unsafe
        && !dry_run
        && !(shown.kept_unknown.is_empty() && shown.kept_unsafe.is_empty())
    {
        let mut question = String::from("omh cannot vouch for these. Each one, and why:\n\n");
        for (item, why) in shown.kept_unknown.iter().chain(shown.kept_unsafe.iter()) {
            question.push_str(&format!(
                "  {:<10} {}\n    {why}\n",
                item.class.noun(),
                item.name
            ));
        }
        question.push_str(&format!(
            "\nremove all {}? this destroys anything they hold",
            shown.kept_unknown.len() + shown.kept_unsafe.len()
        ));
        agreed = crate::ask::confirm(&question, input, out)?;
        anyhow::ensure!(agreed, "nothing was removed");
    }

    let acting = plan(
        inventory(&paths.root, backend.as_deref()).0,
        Vec::new(),
        &|id| crate::profile::attribution_of(&paths.root, id),
        &holds_work,
        agreed,
    );
    let final_plan = Plan {
        remove: acting.remove,
        ..shown
    };

    let went: Vec<(Item, Option<String>)> = if dry_run {
        Vec::new()
    } else {
        final_plan
            .remove
            .iter()
            .map(|i| (i.clone(), remove_one(i, backend.as_deref())))
            .collect()
    };

    ctx.say(&Pruned {
        plan: final_plan,
        went: went.clone(),
        dry_run,
    });
    // A removal that did not happen is a failure, the same as `rm`'s.
    anyhow::ensure!(
        went.iter().all(|(_, why)| why.is_none()),
        "some of what omh meant to remove is still there"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(class: Class, name: &str, id: Option<&str>) -> Item {
        Item {
            class,
            name: name.to_string(),
            id: id.map(str::to_string),
        }
    }

    /// **The first run after this lands removes nothing.**
    ///
    /// No checkout on any machine has a record yet, so every id answers
    /// `Unknown`. If that ever reads as "the checkout was deleted", the first
    /// `omh prune` anybody types takes every artifact they have. This is the
    /// test that turns red before somebody's disk does.
    #[test]
    fn prune_removes_nothing_when_no_checkout_has_a_record() {
        let items = vec![
            item(
                Class::Volume,
                "omh-cache-api-1a2b3c4d",
                Some("api-1a2b3c4d"),
            ),
            item(
                Class::Container,
                "omh-web-5e6f7a8b-s01",
                Some("web-5e6f7a8b"),
            ),
            item(Class::State, "/home/.omh/notes/wire", Some("wire")),
        ];
        let plan = plan(
            items,
            Vec::new(),
            &|_| Attribution::Unknown("omh has no record of this checkout".into()),
            &|_| None,
            false,
        );
        assert!(
            plan.remove.is_empty(),
            "nothing may go on evidence omh does not have: {:?}",
            plan.remove
        );
        assert_eq!(
            plan.kept_unknown.len(),
            3,
            "and all three are accounted for"
        );
    }

    /// Exactly the gone one, and nothing beside it.
    #[test]
    fn prune_removes_exactly_the_gone_one() {
        let items = vec![
            item(Class::Volume, "gone", Some("gone-1a2b3c4d")),
            item(Class::Volume, "live", Some("live-5e6f7a8b")),
            item(Class::Volume, "unknown", Some("unk-9c0d1e2f")),
            item(Class::State, "holds-work", Some("work-3a4b5c6d")),
        ];
        let plan = plan(
            items,
            Vec::new(),
            &|id| match id {
                "gone-1a2b3c4d" => Attribution::Gone("/gone/api".into()),
                "live-5e6f7a8b" => Attribution::Live("/here/web".into()),
                "work-3a4b5c6d" => Attribution::Gone("/gone/wire".into()),
                _ => Attribution::Unknown("no record".into()),
            },
            &|i| (i.name == "holds-work").then(|| "holds 3 commits no branch has".to_string()),
            false,
        );
        assert_eq!(plan.remove.len(), 1);
        assert_eq!(plan.remove[0].name, "gone");
        assert_eq!(plan.kept_live.len(), 1);
        assert_eq!(plan.kept_unknown.len(), 1);
        assert_eq!(plan.kept_unsafe.len(), 1, "gone, but it holds work");
        assert!(
            plan.kept_unsafe[0].1.contains("3 commits"),
            "and the reason is kept, because it is what the question reads out"
        );
    }

    /// A remnant of an aborted operation belongs to nobody, and that is an
    /// answer rather than a gap.
    #[test]
    fn an_aborted_operations_remnant_needs_no_attribution() {
        let plan = plan(
            vec![item(
                Class::Temp,
                "/home/.omh/worktrees/tmp.1s9QTT4H5v",
                None,
            )],
            Vec::new(),
            // Nothing may be asked of the attributor: there is no id.
            &|_| panic!("a temp remnant has no checkout to attribute"),
            &|_| None,
            false,
        );
        assert_eq!(plan.remove.len(), 1);
    }

    /// A class omh could not enumerate is a gap in the report, never a tidy
    /// machine.
    #[test]
    fn a_class_omh_could_not_list_is_said_rather_than_assumed_empty() {
        let plan = plan(
            Vec::new(),
            vec!["omh could not list volumes: the daemon is not answering".into()],
            &|_| Attribution::Unknown("no record".into()),
            &|_| None,
            false,
        );
        assert!(plan.remove.is_empty());
        assert_eq!(plan.could_not_list.len(), 1, "and it is carried, not lost");
    }

    /// The flag widens the set and keeps the reasons.
    #[test]
    fn nothing_unsafe_goes_without_the_flag() {
        let items = vec![
            item(Class::Volume, "unknown", Some("unk-9c0d1e2f")),
            item(Class::State, "holds-work", Some("work-3a4b5c6d")),
        ];
        let attribute = |id: &str| match id {
            "work-3a4b5c6d" => Attribution::Gone("/gone/wire".into()),
            _ => Attribution::Unknown("no record".into()),
        };
        let holds = |i: &Item| (i.name == "holds-work").then(|| "holds 3 commits".to_string());

        let without = plan(items.clone(), Vec::new(), &attribute, &holds, false);
        assert!(without.remove.is_empty(), "neither goes on its own");

        let with = plan(items, Vec::new(), &attribute, &holds, true);
        assert_eq!(with.remove.len(), 2, "and both go only when asked for");
        assert!(with.kept_unknown.is_empty() && with.kept_unsafe.is_empty());
    }
}
