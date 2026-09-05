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
    /// Never removable: an image is shared across checkouts, so a gone
    /// checkout is not grounds for removing one. `plan` refuses it explicitly
    /// rather than `inventory` declining to emit one, so the refusal is a
    /// decision a test can reach.
    Image,
    /// A directory under `~/.omh` keyed by `repo_id`.
    State,
}

// There is deliberately no `Temp` here. It classified anything named `tmp.*`
// as debris owned by nobody, which licensed skipping both the attribution
// table and the work check — and nothing in omh has ever created such a
// directory under a state dir. What it actually matched was the `repo_id` of a
// checkout `mktemp -d` had made, so a bare `omh prune` deleted a live
// checkout's worktrees and notes with no flag and no prompt.

impl Class {
    pub fn noun(&self) -> &'static str {
        match self {
            Class::Volume => "volume",
            Class::Container => "container",
            Class::Network => "network",
            Class::Image => "image",
            Class::State => "directory",
        }
    }
}

/// One thing omh left behind.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Item {
    pub class: Class,
    /// What to say, and for most classes what to remove.
    pub name: String,
    /// The `repo_id` this is keyed by. **Not an `Option`:** the one item that
    /// carried `None` was handed a fabricated `Attribution::Gone` and skipped
    /// the table entirely. Nothing omh cannot key is something it may decide
    /// about.
    pub id: String,
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
        // Asked of the attributor, always. There is no arm that decides
        // without it.
        match attribute(&item.id) {
            Attribution::Live(_) => out.kept_live.push(item),
            Attribution::Unknown(why) => out.kept_unknown.push((item, why)),
            // **An image is never removable, whoever built it.** It is
            // content-addressed and shared: the same `omh/claude:<hash>` serves
            // every checkout whose base resolves to it, and the `omh.repo`
            // label names only whoever got there first. Refused here rather
            // than by `inventory` declining to emit one, because an invariant
            // enforced by a producer's silence is one no test can supply the
            // input for — and `remove_one` has a working `image rm` arm one
            // line away.
            Attribution::Gone(_) if item.class == Class::Image => out.kept_unsafe.push((
                item,
                "an image is shared across checkouts, so a gone checkout is not grounds for \
                 removing one — that needs the in-use check `image::superseded` implements"
                    .to_string(),
            )),
            Attribution::Gone(_) => match holds_work(&item) {
                Some(why) => out.kept_unsafe.push((item, why)),
                None => out.remove.push(item),
            },
        }
    }
    if include_unsafe {
        out.widen();
    }
    out
}

impl Plan {
    /// Move what omh cannot vouch for into the removal set.
    ///
    /// A method rather than a second `plan` call over a second `inventory`.
    /// That shape produced two bugs at once: the report spliced `remove` from
    /// the widened plan onto the *narrow* plan's buckets, so destroyed items
    /// were printed under "left" and the user was advised to run the flag they
    /// had just run — and the second walk happened **after** the prompt, so a
    /// directory created while the question sat on screen was removed without
    /// ever appearing in it.
    ///
    /// Moving buckets in place means what is removed is exactly what was
    /// named, and the reasons stay with whatever is left.
    pub fn widen(&mut self) {
        for (item, _) in std::mem::take(&mut self.kept_unknown) {
            self.remove.push(item);
        }
        // **Images stay kept even here.** The flag widens what omh cannot
        // vouch for; it does not make a shared artifact belong to one
        // checkout. There is no question it could answer.
        let (images, rest): (Vec<_>, Vec<_>) = std::mem::take(&mut self.kept_unsafe)
            .into_iter()
            .partition(|(i, _)| i.class == Class::Image);
        self.kept_unsafe = images;
        for (item, _) in rest {
            self.remove.push(item);
        }
    }

    /// What the dangerous flag would reach, for the question to name.
    pub fn cannot_vouch_for(&self) -> impl Iterator<Item = &(Item, String)> {
        self.kept_unknown.iter().chain(
            self.kept_unsafe
                .iter()
                .filter(|(i, _)| i.class != Class::Image),
        )
    }
}

/// The per-repo directories omh writes, and whether losing one can lose work.
///
/// The names come from `profile::KEYED`, which is the canonical list and whose
/// own doc says adding a seventh should be "a change in one place". This adds
/// only the work flag. A second independent spelling was exactly the second
/// place that doc warns about, and
/// `every_directory_omh_keys_is_one_prune_considers` is what keeps them equal.
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
    backend: Option<&crate::runtime::Backend>,
    backfill: crate::profile::Backfill,
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
                            items.push(Item {
                                class: Class::State,
                                name: e.path().display().to_string(),
                                id: e.file_name().to_string_lossy().into_owned(),
                            });
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
                        id: id.to_string(),
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
                        id,
                    })
                },
                &mut items,
            );
            images(root, b, &mut blind, backfill);
            list(
                b,
                b.network_args(),
                "networks",
                &mut blind,
                |n| {
                    id_in_network(n).map(|id| Item {
                        class: Class::Network,
                        name: n.to_string(),
                        id,
                    })
                },
                &mut items,
            );
        }
    }
    (items, blind)
}

/// Harvest checkout paths from image labels. **Images are not pruned here.**
///
/// `omh.repo` records the checkout an image was *built for*, and that is a
/// backfill source worth having: recording it attributes that checkout's
/// volumes, containers, networks and directories, so one label pays for all of
/// them. Measured on one machine, it moved ten artifacts out of "could not
/// attribute".
///
/// **It is not grounds for removing the image.** An image is content-addressed
/// and shared: the same `omh/claude:<hash>` serves every checkout whose base
/// resolves to it, and the label names only whoever built it first. A dry run
/// while capturing the docs put `omh/claude:8eae0d5c1511fa89` — the image this
/// machine was actively running — in the remove set, because the tmp checkout
/// that first built it had been deleted. That is the mistake
/// `image::superseded` already refuses in its own doc: *anything a container
/// references, however old, because that is a session someone is still using*.
///
/// Pruning images needs the in-use check that function does, not this label.
/// Until that lands, omh does not remove images at all.
fn images(
    root: &std::path::Path,
    backend: &crate::runtime::Backend,
    blind: &mut Vec<String>,
    backfill: crate::profile::Backfill,
) {
    let Some(args) = backend.image_args() else {
        blind.push("omh has not measured how this runtime lists images".into());
        return;
    };
    let ids = match backend.output(&args) {
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
    match backend.output(&inspect) {
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
                    if backfill == crate::profile::Backfill::Record {
                        let _ = crate::profile::remember(root, &crate::profile::id_for(path), path);
                    }
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
    // **The session form first.** `graph-` was matched first, and a checkout
    // directory may legitimately be called `graph-something` — so
    // `omh-graph-tools-1a2b3c4d-s01` was read as the id `tools-1a2b3c4d-s01`,
    // which omh has no record of, and the live session was offered up for
    // removal.
    if let Some((id, tail)) = rest.rsplit_once("-s") {
        if !id.is_empty() && !tail.is_empty() && tail.chars().all(|c| c.is_ascii_digit()) {
            return Some(id.to_string());
        }
    }
    // The graph container, which carries no session suffix.
    rest.strip_prefix("graph-")
        .filter(|id| !id.is_empty())
        .map(str::to_string)
}

/// The `repo_id` inside a network name: `omh-<repo_id>-sNN` for a session's,
/// `omh-<repo_id>` for the per-repo one older versions made.
///
/// No `cache-` exclusion. There was one, guarding against `omh-cache-<id>` —
/// which is a **volume**, in a different namespace, and can never appear in a
/// network listing. What it actually did was drop the network of any checkout
/// whose directory is called `cache-something` into no bucket at all, while
/// the report still said every class had been listed.
pub fn id_in_network(name: &str) -> Option<String> {
    let rest = name.strip_prefix("omh-")?;
    // The session form first, as in `id_in_container`: a session's network is
    // named like its container, and read with the per-repo rule below it
    // would attribute to an id nobody has.
    if let Some((id, tail)) = rest.rsplit_once("-s") {
        if !id.is_empty() && !tail.is_empty() && tail.chars().all(|c| c.is_ascii_digit()) {
            return Some(id.to_string());
        }
    }
    // The per-repo network older versions created, one per checkout.
    Some(rest).filter(|id| !id.is_empty()).map(str::to_string)
}

/// Run one listing, and record it as unreadable rather than empty when it
/// fails — the distinction the whole report rests on.
fn list(
    backend: &crate::runtime::Backend,
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
    match backend.output(&args) {
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
pub fn render(
    plan: &Plan,
    went: &[(Item, Option<String>)],
    dry_run: bool,
    home: &std::path::Path,
) -> String {
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
                format!("  {:<10} {}", item.class.noun(), shorten(&item.name, home)),
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
                format!("  {:<10} {}", item.class.noun(), shorten(&item.name, home)),
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
    say_grouped(&mut out, &plan.kept_unknown, home);
    line(
        &mut out,
        format!("  {:<4} omh cannot vouch for", plan.kept_unsafe.len()),
    );
    say_grouped(&mut out, &plan.kept_unsafe, home);
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

/// A path as a person would write it: `~` for the home directory.
///
/// Compared by path component, not by string prefix — `/Users/younger` starts
/// with `/Users/you` and is somebody else entirely.
fn shorten(name: &str, home: &std::path::Path) -> String {
    std::path::Path::new(name)
        .strip_prefix(home)
        .map(|rest| format!("~/{}", rest.display()))
        .unwrap_or_else(|_| name.to_string())
}

/// Say a bucket without turning the report into a wall.
///
/// **Found by running it.** The first version printed one line per item with
/// its reason repeated, which on a machine with any history is hundreds of
/// identical sentences — unreadable, and it pushes every other line off the
/// screen. The leftovers row had already made and fixed this mistake; re-run
/// `omh prune` against a machine with orphans to see the shape it avoids.
///
/// Grouped by reason, because the reason is what differs and what a person
/// acts on. Items are named only while there are few enough to read; past
/// that the count is the fact, with a couple by name so the reader can
/// recognise the shape.
fn say_grouped(out: &mut String, items: &[(Item, String)], home: &std::path::Path) {
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
                out.push_str(&format!("         {} — {why}\n", shorten(&item.name, home)));
            }
        } else {
            out.push_str(&format!("         {} of them — {why}\n", group.len()));
            for item in group.iter().take(2) {
                out.push_str(&format!("           e.g. {}\n", shorten(&item.name, home)));
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

/// Remove one thing, and for a directory **ask whether it went**.
///
/// `rm` trusted `git worktree remove`'s exit code and reported removals it had
/// not performed, so a directory is re-asked with `still_there`.
///
/// **The runtime classes are not re-asked, and the doc used to claim they
/// were.** A volume, container or network is judged by the exit status of the
/// `rm` omh issued — which is the very thing the sentence above says is not the
/// answer. Re-asking means a second `volume ls` / `ps -a` / `network ls` per
/// item, and until that is written this reports what docker claimed rather than
/// what omh observed. Said plainly rather than left as a promise the code does
/// not keep.
pub fn remove_one(item: &Item, backend: Option<&crate::runtime::Backend>) -> Option<String> {
    let args: Vec<String> = match item.class {
        Class::State => {
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
    match backend.output(&args) {
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
    /// The home directory, so paths print the way a person writes them. The
    /// `--json` half keeps them absolute: a consumer wants the real path.
    pub home: std::path::PathBuf,
    pub plan: Plan,
    pub went: Vec<(Item, Option<String>)>,
    pub dry_run: bool,
}

impl crate::out::Report for Pruned {
    fn human(&self, _p: &crate::out::Palette) -> String {
        render(&self.plan, &self.went, self.dry_run, &self.home)
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
/// recorded and verified absent, and nothing there holds work — so there is no
/// `--yes` over it: a confirmation on something always safe is what teaches
/// people to confirm without reading, and then the one that matters is
/// answered the same way.
pub fn prune_cmd(
    cwd: &std::path::Path,
    dry_run: bool,
    include_unsafe: bool,
    interactive: crate::cmd::harvest::Interactive,
    ctx: &crate::out::Ctx,
    input: &mut dyn std::io::BufRead,
    out: &mut dyn std::io::Write,
) -> anyhow::Result<()> {
    let paths = crate::profile::Paths::discover(cwd)?;
    let backend = crate::runtime::select(&crate::runtime_preference(&paths), &|p| {
        crate::runtime::installed(p)
    })
    .ok();

    // **One walk.** The plan the question names is the plan that acts, so
    // nothing can appear between the two — a second inventory after the prompt
    // removed a directory created while the question was on screen.
    let backfill = if dry_run {
        crate::profile::Backfill::DoNot
    } else {
        crate::profile::Backfill::Record
    };
    let home = paths.root.parent().unwrap_or(&paths.root).to_path_buf();
    let (items, blind) = inventory(&paths.root, backend.as_ref(), backfill);
    let mut plan_ = plan(
        items,
        blind,
        &|id| crate::profile::attribution_of(&paths.root, id, backfill),
        &holds_work,
        false,
    );

    if include_unsafe && plan_.cannot_vouch_for().next().is_some() {
        if dry_run {
            // Nothing is removed, so there is nothing to consent to — and a
            // rehearsal that previews a *smaller* run than the real one is
            // worse than none. This used to report "would remove 0" for a
            // command that then removed everything.
            plan_.widen();
        } else {
            // **Somebody has to be there.** `Consent` is the house type for
            // this and `Interactive::of_stdin` is how every other destructive
            // path spells it. Without this, `yes | omh prune
            // --dangerously-include-unsafe` destroyed unreviewed commits while
            // the flag's own help promised it could not.
            let consent =
                crate::cmd::harvest::Consent::read(crate::cmd::harvest::Forced(false), interactive);
            anyhow::ensure!(
                consent == crate::cmd::harvest::Consent::MayAsk,
                "there is nobody to ask, and omh will not destroy what it cannot vouch for \
                 unasked. Run it from a terminal — there is deliberately no flag that skips \
                 this question, because nothing about a script makes the answer safer"
            );
            let mut question = String::from("omh cannot vouch for these. Each one, and why:\n\n");
            let mut asked = 0usize;
            for (item, why) in plan_.cannot_vouch_for() {
                asked += 1;
                question.push_str(&format!(
                    "  {:<10} {}\n    {why}\n",
                    item.class.noun(),
                    shorten(&item.name, &home)
                ));
            }
            question.push_str(&format!(
                "\nremove all {asked}? this destroys anything they hold"
            ));
            anyhow::ensure!(
                crate::ask::confirm(&question, input, out)?,
                "nothing was removed"
            );
            plan_.widen();
        }
    }

    let went: Vec<(Item, Option<String>)> = if dry_run {
        Vec::new()
    } else {
        plan_
            .remove
            .iter()
            .map(|i| (i.clone(), remove_one(i, backend.as_ref())))
            .collect()
    };

    let stuck = went.iter().any(|(_, why)| why.is_some());
    ctx.say(&Pruned {
        home,
        plan: plan_,
        went,
        dry_run,
    });
    // A removal that did not happen is a failure, the same as `rm`'s.
    anyhow::ensure!(!stuck, "some of what omh meant to remove is still there");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(class: Class, name: &str, id: &str) -> Item {
        Item {
            class,
            name: name.to_string(),
            id: id.to_string(),
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
            item(Class::Volume, "omh-cache-api-1a2b3c4d", "api-1a2b3c4d"),
            item(Class::Container, "omh-web-5e6f7a8b-s01", "web-5e6f7a8b"),
            item(Class::State, "/home/.omh/notes/wire", "wire"),
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
            item(Class::Volume, "gone", "gone-1a2b3c4d"),
            item(Class::Volume, "live", "live-5e6f7a8b"),
            item(Class::Volume, "unknown", "unk-9c0d1e2f"),
            item(Class::State, "holds-work", "work-3a4b5c6d"),
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

    /// Paths are shown the way a person writes them.
    ///
    /// The report prints one path per line and they are long; a screen of
    /// `/Users/<name>/.omh/...` is mostly prefix. It also made the docs
    /// dishonest: the captured block was hand-edited to `~/.omh/...` because
    /// that is what a reader expects, which is exactly the edit a captured
    /// block exists to prevent. Abbreviating for real makes the capture true.
    #[test]
    fn a_path_under_home_is_shown_as_home() {
        let home = std::path::Path::new("/Users/you");
        assert_eq!(shorten("/Users/you/.omh/run/web", home), "~/.omh/run/web");
        // Not a prefix match on the string: a sibling directory whose name
        // merely starts the same way is somebody else's.
        assert_eq!(
            shorten("/Users/younger/.omh/run/web", home),
            "/Users/younger/.omh/run/web"
        );
        assert_eq!(
            shorten("omh-cache-api-1a2b3c4d", home),
            "omh-cache-api-1a2b3c4d"
        );
    }

    /// Every directory a checkout's identity names is one `prune` considers.
    ///
    /// `profile::KEYED` is the canonical list and its doc asks for adding a
    /// seventh to be "a change in one place". `STATE_DIRS` re-spelled the same
    /// six independently, which made it a second place — and a directory
    /// missing from it is not reported, not attributed and never cleaned,
    /// silently.
    #[test]
    fn every_directory_omh_keys_is_one_prune_considers() {
        let considered: std::collections::BTreeSet<&str> =
            STATE_DIRS.iter().map(|(d, _)| *d).collect();
        let keyed: std::collections::BTreeSet<&str> =
            crate::profile::KEYED.iter().copied().collect();
        assert_eq!(
            considered, keyed,
            "prune must consider exactly the directories a checkout's identity names"
        );
    }

    /// A session's network is attributed to its checkout, not to a stranger
    /// whose id happens to be the session name.
    ///
    /// Networks are per session now — `omh-<repo_id>-sNN`, the container's own
    /// name — and reading that with the per-repo rule made `<repo_id>-s01`
    /// the id, which omh has no record of, so the live session's network was
    /// offered up for removal. The session form goes first, as it does in
    /// `id_in_container`, and the bare per-repo form stays readable because
    /// every checkout that ran an older omh still has one to prune.
    #[test]
    fn a_session_network_is_attributed_to_its_checkout_not_to_a_stranger() {
        assert_eq!(
            id_in_network("omh-tools-1a2b3c4d-s01").as_deref(),
            Some("tools-1a2b3c4d")
        );
        assert_eq!(
            id_in_network("omh-1a2b3c4d").as_deref(),
            Some("1a2b3c4d"),
            "the per-repo network older versions made is still somebody's"
        );
        assert_eq!(
            id_in_network("omh-graph-tools-1a2b3c4d-s12").as_deref(),
            Some("graph-tools-1a2b3c4d"),
            "a checkout directory may be called graph-something"
        );
    }

    /// The names omh keys things by, parsed back — including the ones that
    /// look like something else.
    ///
    /// Both spellings here misread a checkout whose *directory name* collides
    /// with omh's own prefixes, and both mistakes are silent:
    ///
    /// - `omh-graph-tools-1a2b3c4d-s01` is the session container of a checkout
    ///   called `graph-tools`. The graph branch matched first and returned
    ///   `tools-1a2b3c4d-s01` — an id omh has no record of, so the container
    ///   was offered for removal while the same run correctly attributed that
    ///   checkout's volume as live.
    /// - `omh-cache-server-1a2b3c4d` is the *network* of a checkout called
    ///   `cache-server`. A `cache-` exclusion — defending against a volume
    ///   name, in a different namespace — dropped it into no bucket at all,
    ///   while the report still said every class was listed.
    #[test]
    fn a_checkout_named_after_omhs_own_prefixes_is_still_read_correctly() {
        // Session containers: `omh-<repo_id>-sNN`.
        assert_eq!(
            id_in_container("omh-api-1a2b3c4d-s01").as_deref(),
            Some("api-1a2b3c4d")
        );
        assert_eq!(
            id_in_container("omh-graph-tools-1a2b3c4d-s01").as_deref(),
            Some("graph-tools-1a2b3c4d"),
            "a checkout may be called `graph-tools`; the session form is read first"
        );
        // The graph container: `omh-graph-<repo_id>`, no session suffix.
        assert_eq!(
            id_in_container("omh-graph-api-1a2b3c4d").as_deref(),
            Some("api-1a2b3c4d")
        );
        // Not omh's.
        assert_eq!(id_in_container("postgres").as_deref(), None);
        assert_eq!(id_in_container("omh-").as_deref(), None);

        // Networks: `omh-<repo_id>`, whatever the checkout is called.
        assert_eq!(
            id_in_network("omh-cache-server-1a2b3c4d").as_deref(),
            Some("cache-server-1a2b3c4d"),
            "a network is not a volume — `cache-` here is part of the checkout's name"
        );
        assert_eq!(id_in_network("bridge").as_deref(), None);
    }

    /// An image is never removed, however gone the checkout that built it.
    ///
    /// The invariant lived in the producer — `inventory` simply never emitted
    /// one — which made it both unguarded and untestable, while `remove_one`
    /// kept a working `image rm` arm one line away. It belongs in the decider,
    /// where a test can supply the input the producer refuses to.
    ///
    /// This is the `omh/claude:8eae0d5c1511fa89` incident exactly: the builder
    /// deleted, nothing held, the dangerous flag on. An image is
    /// content-addressed and shared, so the checkout that built it being gone
    /// says nothing about who is running it.
    #[test]
    fn an_image_is_never_removed_however_gone_its_builder_is() {
        let p = plan(
            vec![item(
                Class::Image,
                "omh/claude:8eae0d5c1511fa89",
                "tmp-1a2b3c4d",
            )],
            Vec::new(),
            &|_| Attribution::Gone("/gone/tmp".into()),
            &|_| None,
            true,
        );
        assert!(
            p.remove.is_empty(),
            "this is the image the machine is running: {:?}",
            p.remove
        );
        assert_eq!(p.kept_unsafe.len(), 1);
        assert!(
            p.kept_unsafe[0].1.contains("shared"),
            "and it says why, rather than looking like an oversight: {}",
            p.kept_unsafe[0].1
        );
    }

    /// **Every directory omh finds carries the id it is keyed by.**
    ///
    /// `inventory` used to classify anything named `tmp.*` as `Class::Temp`
    /// with no id, and `plan` handed those a fabricated `Attribution::Gone` —
    /// skipping the attribution table *and* the work check. That is the
    /// collapse this module exists to forbid, reached through a field default
    /// rather than a decision.
    ///
    /// It was not matching debris. A checkout directory made by `mktemp` is
    /// named `tmp.XXXX`, so its `repo_id` is `tmp.XXXX-<digest>` — and a live
    /// checkout's worktrees and notes were deleted by a bare `omh prune`, with
    /// no flag and no prompt. Nothing in omh has ever created a `tmp.*`
    /// directory under a state dir; `git log -S` finds the classifier and
    /// nothing else.
    #[test]
    fn a_directory_named_like_a_temp_file_is_still_attributed() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path().join(".omh");
        // The shape that bit: a checkout `mktemp -d` made, still alive.
        for dir in ["worktrees", "notes"] {
            std::fs::create_dir_all(root.join(dir).join("tmp.1s9QTT4H5v-cfd406d4")).unwrap();
        }

        let (items, blind) = inventory(&root, None, crate::profile::Backfill::DoNot);
        assert!(
            blind.iter().any(|b| b.contains("runtime")),
            "no runtime: {blind:?}"
        );
        assert_eq!(items.len(), 2, "both directories are found: {items:?}");
        for item in &items {
            assert_eq!(
                item.id, "tmp.1s9QTT4H5v-cfd406d4",
                "and each carries the id it is keyed by, so it can be attributed: {item:?}"
            );
        }

        // With the checkout alive, nothing goes — which is only reachable
        // because the id survived the walk.
        let p = plan(
            items,
            blind,
            &|_| Attribution::Live("/work/tmp.1s9QTT4H5v".into()),
            &holds_work,
            true,
        );
        assert!(
            p.remove.is_empty(),
            "a live checkout keeps its state: {:?}",
            p.remove
        );
    }

    /// What was removed is not also reported as left.
    ///
    /// `prune_cmd` spliced `remove` from the widened plan onto the buckets of
    /// the narrow one, so a confirmed run printed the destroyed items under
    /// "left" and advised running the flag it had just run.
    #[test]
    fn nothing_is_reported_both_removed_and_left() {
        let items = vec![
            item(Class::Volume, "unknown", "unk-9c0d1e2f"),
            item(Class::State, "/home/.omh/notes/gone", "work-3a4b5c6d"),
        ];
        let p = plan(
            items,
            Vec::new(),
            &|id| match id {
                "work-3a4b5c6d" => Attribution::Gone("/gone/wire".into()),
                _ => Attribution::Unknown("no record".into()),
            },
            &|i| {
                i.name
                    .ends_with("gone")
                    .then(|| "holds 3 commits".to_string())
            },
            true,
        );
        assert_eq!(p.remove.len(), 2, "both go, because the flag was given");
        assert!(
            p.kept_unknown.is_empty() && p.kept_unsafe.is_empty(),
            "and neither is still listed as left: {:?} {:?}",
            p.kept_unknown,
            p.kept_unsafe
        );
    }

    /// The flag widens the set and keeps the reasons.
    #[test]
    fn nothing_unsafe_goes_without_the_flag() {
        let items = vec![
            item(Class::Volume, "unknown", "unk-9c0d1e2f"),
            item(Class::State, "holds-work", "work-3a4b5c6d"),
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
