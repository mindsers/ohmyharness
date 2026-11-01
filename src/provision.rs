//! Which sandbox a repo runs, and what it is known to hold.
//!
//! The stack a checkout's files name, the provides it installs, the image tag
//! that follows from them, and what a probe of that image found — the answer
//! `omh init` records, every launch reads, and `omh doctor` and `omh upgrade`
//! re-ask. Apart from any one command, since each of them needs it.

use crate::adapter::Adapter;
use crate::out;
use crate::paths::Paths;
use crate::{base, config, facts, image, render, runtime, settings, stack};
use anyhow::Result;
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

/// Which provides applied, from what the predicates answered.
///
/// `None` when nothing was answered — the container never ran, the runtime
/// hiccuped, the image was missing. That case is not "nothing applies", and the
/// difference is destructive rather than academic: `stack::reconcile` drops
/// every `true` it is not told about, so recording an empty answer would erase
/// a repo's resolution and leave the next launch provisioning nothing.
///
/// A provide that could not answer is simply absent from the set, which is the
/// safe direction on a first look — it is not installed, so it is not recorded,
/// so its `needs` are not claimed and nothing reports a gap omh invented.
/// Installing on a coin-flip would be silent either way. On a later look it is
/// not the whole answer: `stack::reread` puts back the ones the table already
/// recorded, because a `true` dropped over a question nobody could answer is a
/// change nobody decided.
///
/// `asked` is how many provides there were to ask about, and it separates two
/// things an empty report cannot: **nothing to ask** is an answer, **nothing
/// answered** is silence. A repo that stops being a stack has no candidates and
/// runs no container, and that has to clear the resolution rather than preserve
/// it — otherwise `[provision]` keeps asserting `rust/toolchain = true` after
/// the `Cargo.toml` is gone, and the stack layer keeps installing a toolchain
/// nothing uses.
pub(crate) fn fired_from(
    asked: usize,
    answered: &[crate::probe::Outcome],
) -> Option<BTreeSet<String>> {
    if asked == 0 {
        return Some(BTreeSet::new());
    }
    // One line per provide, so fewer lines than provides is a report that did
    // not finish — not a report saying "no". Accepting the prefix would make
    // `reconcile` drop every `true` it was not told about and rewrite a
    // committed file without them. The now-deleted `[toolchain]` question had
    // this same shape and had to be fixed for it, where it only cost a spurious
    // question; here it deletes.
    if answered.len() != asked {
        return None;
    }
    Some(
        answered
            .iter()
            .filter(|o| stack::verdict(o) == stack::Verdict::Applies)
            .map(|o| o.name.clone())
            .collect(),
    )
}

/// Write what fired into the repo's **shared**, committed settings, and hand
/// back what the file now says.
///
/// A function rather than four lines inline, because the layer it names on both
/// sides is the whole of its correctness and inline it is reachable only
/// through a container. Both halves are load-bearing in opposite directions:
///
/// - **Read `Shared`.** `reconcile` writes what it is given, so reading the
///   merge would take a `false` from `settings.local.toml` — one laptop's *not
///   here* — and commit it for everybody who clones.
/// - **Write `Shared`.** The resolution is the repo's, and a teammate cloning
///   it is the reason it lives in a committed file at all. Written to `Local`
///   it would be re-derived, and re-asked, on every machine.
pub(crate) fn record_resolution(
    paths: &Paths,
    fired: &BTreeSet<String>,
) -> Result<BTreeMap<String, bool>> {
    let recorded = stack::reconcile(
        &config::read_provision(paths, config::Layer::Shared)?,
        fired,
    );
    config::write_provision(paths, config::Layer::Shared, &recorded)?;
    Ok(recorded)
}

/// The recipes to run, in the order the stack files gave them.
///
/// File order is install order — `corepack enable pnpm` needs the node the
/// provide above it asserted — so this walks the definitions rather than the
/// resolution, which is a map sorted by name and would silently reorder them.
///
/// A provide with no `install` contributes nothing: it asserts the base image
/// already ships something, so it changes neither the recipe nor the tag.
///
/// **`resolved` is the only input**, and that is the point. It is the
/// `[provision]` table as all three settings layers resolve it: `init` writes
/// what its predicates found, a person may write `false` to opt out, and every
/// launch afterwards reads the same table. A launch that re-derived this from
/// anything else — the predicates it cannot run, a set of provides that
/// "fired" — would build a different image from the one `init` reported, and
/// the disagreement would be invisible because both are plausible.
///
/// Only `true` provisions. Absent is not `false` and does not need to be: an
/// entry nobody recorded is one no predicate has said applies here.
pub(crate) fn installs_for<'a>(
    detected: &[&'a stack::Definition],
    resolved: &BTreeMap<String, bool>,
) -> Vec<&'a str> {
    detected
        .iter()
        .flat_map(|d| d.provides.iter().map(move |p| (d, p)))
        .filter(|(d, p)| resolved.get(&stack::key(&d.name, &p.name)) == Some(&true))
        .filter_map(|(_, p)| p.install.as_deref())
        .collect()
}

/// What the stacks this repo provisions said must resolve once they had run.
///
/// Only provides the resolution recorded `true`. A provide nobody recorded was
/// never installed, and a provide somebody opted out of was deliberately not
/// installed — reporting either as a failure would be a gap omh invented. The
/// consequence of an opt-out is not silenced by that: if a hook names the
/// program, it is probed anyway through `render::hook_programs`, and a hook
/// that cannot run is dropped by name.
///
/// This includes provides with **no `install`**, which is the point of letting
/// them exist: `stacks/node.toml`'s `runtime` asserts the base image already
/// ships `node` and `npm`, and the only way that assertion is worth writing is
/// if something checks it.
pub(crate) fn needs_of(
    detected: &[&stack::Definition],
    resolved: &BTreeMap<String, bool>,
) -> BTreeSet<String> {
    detected
        .iter()
        .flat_map(|d| d.provides.iter().map(move |p| (d, p)))
        .filter(|(d, p)| resolved.get(&stack::key(&d.name, &p.name)) == Some(&true))
        .flat_map(|(_, p)| p.needs.iter().cloned())
        .collect()
}

/// Everything worth asking one image about: what the stacks promised, and what
/// the hooks will actually run.
///
/// **The union, and it has to be.** The two lists answer different questions
/// and neither contains the other. A stack's `needs` is what provisioning owes
/// — the reading that catches rustup installing a `cargo` that cannot link.
/// A hook's program is what will be handed to a shell — and a hand-written
/// `shellcheck` hook is in no `needs` list, so a probe built from `needs` alone
/// ships it into a sandbox that cannot run it. That is the original
/// `cargo: not found` with a different program in it.
pub(crate) fn probe_targets(
    hook_dirs: &[PathBuf],
    own: &base::Own,
    repo: &settings::RepoPolicy,
    owed: &BTreeSet<String>,
) -> Result<BTreeSet<String>> {
    let mut wanted = render::hook_programs(hook_dirs, own, repo)?;
    wanted.extend(owed.iter().cloned());
    Ok(wanted)
}

/// Ask the image about the programs nobody has asked it about yet, remember
/// the answers, and hand back everything known about it.
///
/// The cache is the reason a launch is not a container run: `Facts::unseen`
/// narrows the question to what has never been answered for this tag, and a
/// repo whose hooks and stacks have not changed asks nothing at all.
///
/// Never fatal. A runtime that will not start, an image that is not there, a
/// probe that says nothing — all of them leave the facts as they were, which
/// reads as *nobody has looked* and suppresses nothing. The alternative is a
/// diagnostic failure taking a launch down with it.
/// What a probe run amounts to: what it measured, or **why nobody could be
/// asked**.
///
/// Split out of `measure` because the reason is the whole value of the guard,
/// and a reason that only exists as an `eprintln!` inside a function that shells
/// out is a reason no test can see disappear.
///
/// A container that ran and **failed** is not an answer. Checking only the
/// `Err` arm — a runtime that would not start — was the shape `init`'s
/// predicate call already had to be fixed for: `docker run` failing because the
/// image is gone, the daemon is refusing, or the disk is full exits non-zero
/// with empty stdout, which parses to no outcomes and reads as *nothing was
/// measured*. Unmeasured suppresses nothing, so the direction is safe; the
/// silence is not. Without a reason the user gets a session with every hook
/// shipped into a sandbox nobody could ask about, and nothing said.
///
/// Stderr is trimmed to three lines. A runtime failing to pull or mount can
/// produce a page of it, and a diagnostic that buries the line above it in its
/// own output is one people learn to scroll past.
pub(crate) fn measured_or_reason(
    ok: bool,
    stdout: &str,
    stderr: &str,
) -> Result<Vec<crate::probe::Outcome>, String> {
    if !ok {
        let mut reason = String::from("could not ask the sandbox what it has");
        for line in stderr.lines().filter(|l| !l.trim().is_empty()).take(3) {
            reason.push_str("\n     ");
            reason.push_str(line);
        }
        return Err(reason);
    }
    Ok(crate::probe::parse(stdout))
}

/// What an image was found to have — and whether it was actually asked.
///
/// The two together, because apart they are indistinguishable at the far end:
/// a probe that could not run leaves the facts as they were, `has` comes back
/// as *nobody has looked*, and `render::held_back` reads an unmeasured program
/// as *not blocked*. For a launch that is right and deliberate — an
/// unmeasured program suppresses nothing, so nothing is dropped on a guess.
/// For `init`'s report it is a clean bill of health issued by a doctor who was
/// out, which is the exact sentence that report was rewritten to stop printing.
pub(crate) struct Measured {
    pub(crate) has: BTreeMap<String, bool>,
    /// Why the ask did not happen. `None` means these answers are answers.
    pub(crate) gave_up: Option<String>,
}

pub(crate) fn measure(
    backend: &runtime::Backend,
    paths: &Paths,
    tag: &str,
    wanted: &BTreeSet<String>,
    ctx: &out::Ctx,
) -> Result<Measured> {
    let mut facts = facts::Facts::load(paths);
    let unseen = facts.unseen(tag, wanted);
    if !unseen.is_empty() {
        let borrowed: Vec<&str> = unseen.iter().map(String::as_str).collect();
        let ran = backend.output(&image::probe_args(
            tag,
            &crate::probe::probe_programs(&borrowed),
        ));
        let outcomes = match ran {
            Ok(out) => measured_or_reason(
                out.status.success(),
                &String::from_utf8_lossy(&out.stdout),
                &String::from_utf8_lossy(&out.stderr),
            ),
            Err(e) => Err(format!("could not ask the sandbox what it has ({e})")),
        };
        let outcomes = match outcomes {
            Ok(outcomes) => outcomes,
            Err(reason) => {
                ctx.warn(&reason);
                // Still not fatal — see below — but no longer indistinguishable
                // from an image that was asked and had nothing.
                return Ok(Measured {
                    has: facts.about(tag),
                    gave_up: Some(reason.lines().next().unwrap_or_default().to_string()),
                });
            }
        };
        if !outcomes.is_empty() {
            facts.learn(tag, &outcomes);
            // Reported and swallowed, never fatal. This is a cache beside the
            // catalogue; a read-only home, a full disk or a `facts.json`
            // somebody replaced with a directory would otherwise abort every
            // `omh new`, `omh sNN resume`, `omh s attach`, `omh doctor` and `omh
            // init` on the machine — every caller of `top_up`, which is all of
            // them — a launch
            // killed by a file whose entire design premise is that losing it
            // degrades to "nobody has looked". `Facts::load` already treats the
            // read side this way and says why.
            if let Err(e) = facts.save(paths) {
                ctx.warn(&format!(
                    "measurements not cached ({e:#}) — the sandbox is asked again next time"
                ));
            }
        }
    }
    Ok(Measured {
        has: facts.about(tag),
        gave_up: None,
    })
}

/// What this repo's sandbox is: the recipe its stacks provision, the image that
/// recipe produces, and what that image has been measured to contain.
///
/// The four fields are **one answer**, and holding them together is what makes
/// a mismatch hard to write: `tag` is derived from `installs`, `resolves` is
/// keyed on `tag`, and `owed` is what `installs` promised. Nothing outside
/// [`sandbox`] constructs one.
pub(crate) struct Sandbox {
    /// Owned, because the definitions they are read from do not outlive this.
    pub(crate) installs: Vec<String>,
    pub(crate) tag: String,
    pub(crate) resolves: BTreeMap<String, bool>,
    /// What the provides this repo installed said must resolve once they had.
    /// Carried here rather than re-derived, so the caller that tops the
    /// measurements up asks about the same list `init` reported on.
    pub(crate) owed: BTreeSet<String>,
    /// Why `resolves` is not an answer, when it is not.
    ///
    /// `None` until `top_up` has asked. A launch does not read this — an
    /// unmeasured program suppresses nothing either way — but a report that
    /// says what is held back has to be able to say *nothing was asked*, and
    /// `resolves` alone cannot: an empty map is what both outcomes look like.
    pub(crate) unmeasured: Option<String>,
    /// The corporate root `tag` was computed with, for the same reason
    /// `installs` is carried: the layer a launch builds and the tag it runs
    /// must come from one resolution. `session_up` read the setting a second
    /// time, which is the split its own doc comment warns about — a second read
    /// can differ from the first, and then omh builds one image and names
    /// another.
    pub(crate) ca: Option<crate::image::Root>,
}

impl Sandbox {
    /// The class of [`tag`](Self::tag), for an image built over it: a stack's
    /// when this repo installs something, the harness's own otherwise —
    /// `image::stack_tag` returns the harness's tag then.
    pub(crate) fn kind<'a>(
        &self,
        adapter: &'a Adapter,
        repo: &'a std::path::Path,
    ) -> image::Kind<'a> {
        if self.installs.is_empty() {
            image::Kind::Harness(adapter)
        } else {
            image::Kind::Stack(adapter, repo)
        }
    }

    pub(crate) fn recipe(&self) -> Vec<&str> {
        self.installs.iter().map(String::as_str).collect()
    }

    /// Ask this image about anything nobody has asked it yet, and keep the
    /// answers.
    ///
    /// Launch does this too, not only `init`. A hook added after the last
    /// `init` names a program no measurement covers, and an unmeasured program
    /// suppresses nothing — so without this the hook ships into a sandbox that
    /// may not have it and fails at turn one with `not found`, which is the
    /// failure this whole design starts from. The cache is what makes it
    /// affordable: a repo whose hooks and stacks have not changed asks nothing
    /// and starts no container.
    ///
    /// **Builds the image first**, and that ordering is the method's reason for
    /// existing rather than a detail inside it.
    ///
    /// `init` had it right and all three launch paths had it backwards: they
    /// measured, then built inside `session_up`. So the first launch after a
    /// recipe changed — a `[provision]` opt-out, a fresh clone of a repo whose
    /// resolution is committed, anything after `docker image prune` — probed a
    /// tag with no image behind it, learned nothing, and shipped every hook
    /// unsuppressed into a sandbox that did not have their programs. It healed
    /// on the *second* launch, which is precisely the broken-first-turn this
    /// design exists to remove.
    ///
    /// Fixing it in three call sites would have left the fourth caller to get
    /// it right. Here it cannot be got wrong: asking an image a question and
    /// making sure there is an image to ask are one operation.
    ///
    /// Failures inside `measure` are reported and swallowed — a runtime that
    /// will not start leaves the facts as they were, which reads as *nobody has
    /// looked* and suppresses nothing. The build is **not** swallowed: an image
    /// that will not build is the session, not a diagnostic about it.
    //
    // Eight arguments, one over clippy's default. Every one is a distinct
    // input this cannot derive — the paths, the runtime, the adapter, the hook
    // directories, both halves of the resolved settings, and where to report.
    // Bundling them into a struct only to unpack it here would move the list
    // rather than shorten it.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn top_up(
        &mut self,
        paths: &Paths,
        backend: &runtime::Backend,
        adapter: &Adapter,
        hook_dirs: &[PathBuf],
        own: &base::Own,
        repo: &settings::RepoPolicy,
        ctx: &out::Ctx,
    ) -> Result<()> {
        let recipe: Vec<String> = self.installs.clone();
        // The reading `self.tag` was computed from. This used to resolve the
        // setting again here, with a comment arguing that a parameter is one
        // more place to forget — but the value was already on `self`, and a
        // second read of a file that has moved builds a layer the tag beside
        // it does not name.
        let ca = self.ca.as_ref().map(crate::image::Root::pem);
        image::ensure_stack(
            backend,
            adapter,
            &recipe.iter().map(String::as_str).collect::<Vec<_>>(),
            ca,
            &paths.repo,
        )?;
        let wanted = probe_targets(hook_dirs, own, repo, &self.owed)?;
        let measured = measure(backend, paths, &self.tag, &wanted, ctx)?;
        self.resolves = measured.has;
        self.unmeasured = measured.gave_up;
        Ok(())
    }
}

/// Work out which image this repo runs, and what is already known about it.
///
/// **One function, because these are one answer.** For the whole of the first
/// milestone `init` built a stack layer and `container::plan` hardcoded
/// `image::tag_for(adapter)`, so the layer was built by one command and run by
/// none — and nothing was wrong with either half on its own. Two places
/// deciding which image a session runs is the shape of that bug, so there is
/// one place, and the measurements come back keyed on the tag it returned.
///
/// Fatal when the stacks will not load, which is the opposite of `say_hooks`'
/// answer to the same directory and is right for the opposite reason. There, an
/// unreadable directory costs a report. Here it decides *which sandbox you get*:
/// falling back to the harness image would launch a session with no toolchain
/// in it, silently, which is the failure this whole design starts from.
///
/// Reads the cache but never the container. Asking the image anything is
/// [`Sandbox::top_up`], which builds it first — so this stays cheap enough to
/// call on every launch path before anything has been decided.
pub(crate) fn sandbox(
    paths: &Paths,
    adapter: &Adapter,
    repo: &settings::RepoPolicy,
    ca: Option<crate::image::Root>,
) -> Result<Sandbox> {
    let defs = stack::load_all(&paths.stacks(), &paths.repo_stacks())?;
    sandbox_from(paths, adapter, repo, ca, &defs)
}

/// [`sandbox`], over stack definitions the caller already holds.
///
/// For `omh upgrade --dry-run`, which describes the sandbox a real run would
/// build — from the stacks that run would have refreshed onto disk first, and
/// which a dry run reads out of the binary instead.
pub(crate) fn sandbox_from(
    paths: &Paths,
    adapter: &Adapter,
    repo: &settings::RepoPolicy,
    ca: Option<crate::image::Root>,
    defs: &[stack::Definition],
) -> Result<Sandbox> {
    let detected = stack::detected(defs, &paths.repo);
    let installs: Vec<String> = installs_for(&detected, &repo.provision)
        .into_iter()
        .map(str::to_string)
        .collect();
    // Once, and carried — but resolved by the *caller*, not here. Every later
    // question about this image (the tag, the layer that gets built, the
    // digest a note pins) is answered from that one read, so a PEM edited
    // between two of them cannot produce two different images. Resolving here
    // instead made this the second read for any caller that had already needed
    // the certificate for its harness image, which `init` does.
    let tag = image::stack_tag(
        adapter,
        &installs.iter().map(String::as_str).collect::<Vec<_>>(),
        ca.as_ref().map(image::Root::pem),
    );
    let resolves = facts::Facts::load(paths).about(&tag);
    let owed = needs_of(&detected, &repo.provision);
    Ok(Sandbox {
        installs,
        tag,
        resolves,
        owed,
        // Nothing has been asked yet — this reads the cache and never the
        // container. `top_up` is what turns it into an answer.
        unmeasured: Some("the sandbox has not been asked".into()),
        ca,
    })
}
