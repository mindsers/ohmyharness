//! What each command decided, before anyone has chosen how to read it.
//!
//! One value per command, with a [`Report`] impl that can put it either to a
//! person or to a program. The commands themselves keep the work and hand the
//! answer here; that is what makes the answer testable without a container, a
//! terminal, or a subprocess.
//!
//! ## Why the states are enums and not strings
//!
//! `work_state` used to return `String` — `"3 uncommitted"`, `"→ feat/a"`, or
//! `""`. That is a perfectly good column and a terrible API: a script asking
//! *does this session have unpushed work* has to parse English, and the empty
//! string means both **clean** and **omh could not tell**, which are the two
//! answers it is most dangerous to confuse. The enum keeps them apart, and the
//! human renderer is the only place the English lives.

use crate::out::{self, Cell, Report, Table};
use serde_json::json;

// ── things omh did ──────────────────────────────────────────────────────────

/// Something omh did, what it means, and what to type next.
///
/// Most of omh's output is not a listing — it is one sentence about one thing
/// that just happened. Thirty bespoke report types for thirty sentences would
/// be worse than the `println!`s they replace, so this is the shared shape.
///
/// It still carries **structured** fields rather than only the sentence.
/// `{"message": "removed session s01"}` makes a script parse English to learn
/// the id, which is the whole failure `--json` exists to avoid; `kind` and
/// `data` are what it actually reads.
#[derive(Debug, Clone)]
pub struct Action {
    /// A stable machine key — `session-removed`, `graph-stopped`. Never
    /// reworded for style; the sentence is where the wording lives.
    pub kind: &'static str,
    pub summary: String,
    /// Commands the user may want next, printed as given so they can be
    /// pasted. Dimmed, because they are an offer rather than an instruction.
    pub next: Vec<String>,
    /// Consequences worth knowing that are **not** commands.
    ///
    /// Kept apart from `next` on purpose. `next` is a promise that the line can
    /// be selected and pasted; mixing a sentence into it breaks that promise
    /// for every line, because the reader can no longer tell which is which
    /// without reading them all.
    pub notes: Vec<String>,
    /// The facts behind the sentence, merged into the JSON object.
    pub data: serde_json::Value,
}

impl Action {
    pub fn new(kind: &'static str, summary: impl Into<String>) -> Self {
        Self {
            kind,
            summary: summary.into(),
            next: Vec::new(),
            notes: Vec::new(),
            data: json!({}),
        }
    }

    pub fn next(mut self, command: impl Into<String>) -> Self {
        self.next.push(command.into());
        self
    }

    pub fn note(mut self, consequence: impl Into<String>) -> Self {
        self.notes.push(consequence.into());
        self
    }

    pub fn data(mut self, data: serde_json::Value) -> Self {
        self.data = data;
        self
    }
}

impl Report for Action {
    /// The summary and its consequences — not `next`.
    ///
    /// `notes` stay here because a consequence is part of what happened:
    /// *branch omh/s01 kept* is the answer to `omh s rm`, not advice about it.
    /// `next` is advice, so it leaves through [`Report::asides`] and lands on
    /// stderr.
    fn human(&self, _p: &out::Palette) -> String {
        let mut s = format!("{}\n", self.summary);
        for note in &self.notes {
            s.push_str(&format!("  {note}\n"));
        }
        s
    }

    /// Indented as they always were, so only the *stream* changed here.
    ///
    /// The two spaces group the commands under the summary they belong to, and
    /// a shell ignores leading whitespace on a pasted line — the property
    /// `a_suggested_command_survives_being_pasted` is really guarding is that
    /// nothing is inserted *within* the command.
    fn asides(&self) -> out::Asides {
        out::Asides {
            warnings: Vec::new(),
            hints: self.next.iter().map(|c| format!("  {c}")).collect(),
        }
    }

    fn json(&self) -> serde_json::Value {
        let mut o = json!({
            "action": self.kind,
            "message": self.summary,
        });
        if !self.next.is_empty() {
            o["next"] = json!(self.next);
        }
        if !self.notes.is_empty() {
            o["notes"] = json!(self.notes);
        }
        if let (Some(o), Some(extra)) = (o.as_object_mut(), self.data.as_object()) {
            for (k, v) in extra {
                o.insert(k.clone(), v.clone());
            }
        }
        o
    }
}

// ── omh s down ──────────────────────────────────────────────────────────────

/// What `omh s down` did to each session it was asked about.
///
/// A list rather than one `Action` per session, because with no id the command
/// is asked about *every* session — and saying each one separately emits a
/// JSON document per session, which is a parse error for the caller and
/// nothing at all for the exit code.
#[derive(Debug, Clone)]
pub struct Down {
    /// `(id, was it running and is it now stopped)`.
    pub sessions: Vec<(String, bool)>,
}

impl Report for Down {
    fn human(&self, p: &out::Palette) -> String {
        if self.sessions.is_empty() {
            return format!("{}\n", p.paint(out::DIM, "no sessions"));
        }
        let mut t = Table::new();
        for (id, stopped) in &self.sessions {
            t = t.row(vec![
                Cell::styled(id, out::NAME),
                if *stopped {
                    Cell::plain("stopped; worktree and branch survive")
                } else {
                    Cell::styled("was not running", out::DIM)
                },
            ]);
        }
        t.render(p)
    }

    fn json(&self) -> serde_json::Value {
        json!({
            "action": "sessions-down",
            "sessions": self.sessions.iter().map(|(id, stopped)| json!({
                "session": id,
                "stopped": stopped,
            })).collect::<Vec<_>>(),
        })
    }
}

// ── omh s ls ────────────────────────────────────────────────────────────────

/// Where a session is in the cycle, as one answer.
///
/// Ordered most-actionable first, and deliberately one answer rather than a
/// tally: `s ls` is read at a glance, and a session with uncommitted work needs
/// committing whatever else is also true of it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Work {
    /// git would not answer. **Never rendered as clean** — see the test.
    Unknown,
    Uncommitted(usize),
    ToPush(usize),
    /// Pushed, under the name it went out as.
    Published(String),
    /// Nothing to do.
    Clean,
}

impl Work {
    /// The column `omh s ls` has always printed. Wording preserved exactly:
    /// people grep this, and `tests/cli.rs` pins all four forms.
    pub fn human(&self) -> String {
        match self {
            Self::Unknown => "?".into(),
            Self::Uncommitted(n) => format!("{n} uncommitted"),
            Self::ToPush(n) => format!("{n} to push"),
            Self::Published(target) => format!("→ {target}"),
            Self::Clean => String::new(),
        }
    }

    /// The colour the state deserves at a glance: something to do is yellow,
    /// done is green, and *cannot tell* is not green.
    fn style(&self) -> anstyle::Style {
        match self {
            Self::Unknown => out::WARN,
            Self::Uncommitted(_) | Self::ToPush(_) => out::WARN,
            Self::Published(_) => out::OK,
            Self::Clean => out::DIM,
        }
    }

    fn json(&self) -> serde_json::Value {
        match self {
            Self::Unknown => json!({ "state": "unknown" }),
            Self::Uncommitted(n) => json!({ "state": "uncommitted", "count": n }),
            Self::ToPush(n) => json!({ "state": "unpushed", "count": n }),
            Self::Published(target) => json!({ "state": "published", "branch": target }),
            Self::Clean => json!({ "state": "clean" }),
        }
    }
}

/// One session, as `omh s ls` sees it.
#[derive(Debug, Clone)]
pub struct Session {
    pub id: String,
    pub label: String,
    pub running: bool,
    /// **`None` means nobody asked**, which is not `Some(Work::Clean)`.
    ///
    /// `omh ls` is the wide view and does not spend a git subprocess per
    /// session on a column it never prints. It filled this with `Work::Clean`
    /// once — harmless only because `Inventory` happened not to read the
    /// field, and one line away from reporting every session in `omh ls` as
    /// having nothing in it. `Work::Unknown` exists to keep *cannot tell*
    /// apart from *nothing to do*; `Option` keeps *did not ask* apart from
    /// both, and makes the mistake unspellable rather than merely absent.
    pub work: Option<Work>,
    /// Commits the base branch has that this session does not.
    /// **`None` means omh could not count**, which is not `Some(0)`.
    ///
    /// The same distinction `work` above keeps, for the same reason and in the
    /// same failure: a base that does not resolve in this checkout. The table
    /// renders both as a blank cell — a glance column has nowhere to put a
    /// question — but JSON says `null` rather than a number nobody took.
    pub behind: Option<usize>,
}

/// Every session in this checkout, and what earlier ones left behind.
#[derive(Debug, Clone)]
pub struct Sessions {
    pub sessions: Vec<Session>,
    pub base: String,
    /// Session ids with a container or a run directory but no worktree.
    pub leftovers: Vec<String>,
}

impl Report for Sessions {
    fn human(&self, p: &out::Palette) -> String {
        // No longer `&& leftovers.is_empty()`. With the leftovers on stderr,
        // that condition left stdout holding an empty table for the repo whose
        // sessions have all been removed badly — the one case where "no
        // sessions" is the answer and something is also wrong.
        if self.sessions.is_empty() {
            return format!("{}\n", p.paint(out::DIM, "no sessions"));
        }

        let mut table = Table::new();
        for s in &self.sessions {
            table = table.row(vec![
                Cell::styled(&s.id, out::NAME),
                Cell::plain(&s.label),
                if s.running {
                    Cell::styled("up", out::OK)
                } else {
                    Cell::styled("stopped", out::DIM)
                },
                match &s.work {
                    Some(work) => Cell::styled(work.human(), work.style()),
                    None => Cell::plain(""),
                },
                match s.behind {
                    Some(0) | None => Cell::plain(""),
                    Some(n) => Cell::styled(format!("({n} behind {})", self.base), out::DIM),
                },
            ]);
        }
        table.render(p)
    }

    /// The leftovers, which are not what `omh s ls` was asked for.
    ///
    /// Both lines used to be appended to the table above, so
    /// `omh s ls > sessions.txt` wrote them into the file — the exact case
    /// `docs/commands.md` promises they stay out of. They are still in `json`
    /// as `leftovers`, where a script wanted them all along.
    fn asides(&self) -> out::Asides {
        if self.leftovers.is_empty() {
            return out::Asides::default();
        }
        out::Asides::default()
            .warn(format!(
                "{} removed but left something behind: {}",
                if self.leftovers.len() == 1 {
                    "1 session was"
                } else {
                    "sessions were"
                },
                self.leftovers.join(", ")
            ))
            .hint("  clear each with  omh s rm <id>")
    }

    fn json(&self) -> serde_json::Value {
        json!({
            "base": self.base,
            "sessions": self.sessions.iter().map(|s| json!({
                "id": s.id,
                "label": s.label,
                "running": s.running,
                // `null` where nobody asked. A caller can tell that apart from
                // every answer, which is the whole reason for the `Option`.
                "work": s.work.as_ref().map(Work::json),
                "behind": s.behind,
            })).collect::<Vec<_>>(),
            "leftovers": self.leftovers,
        })
    }
}

// ── omh ls ──────────────────────────────────────────────────────────────────

/// A harness omh knows about here, and who it is logged in as.
#[derive(Debug, Clone)]
pub struct Harness {
    pub name: String,
    /// Empty means nobody has run `omh auth` for it yet.
    pub accounts: Vec<String>,
}

/// An editor omh could attach with, and whether this machine has it.
#[derive(Debug, Clone)]
pub struct Editor {
    pub name: String,
    pub installed: bool,
}

/// What `omh ls` found: harnesses, editors, sessions.
#[derive(Debug, Clone)]
pub struct Inventory {
    pub harnesses: Vec<Harness>,
    /// Where a harness would be added, for the message when there are none.
    pub adapters_dir: String,
    pub editors: Vec<Editor>,
    pub sessions: Vec<Session>,
    pub base: String,
}

impl Report for Inventory {
    fn human(&self, p: &out::Palette) -> String {
        let mut s = out::heading(p, "harnesses:");
        if self.harnesses.is_empty() {
            s.push_str(&out::nothing(
                p,
                &format!("none — add {}/<name>.toml", self.adapters_dir),
            ));
        } else {
            let mut t = Table::new();
            for h in &self.harnesses {
                t = t.row(vec![
                    Cell::styled(&h.name, out::NAME),
                    if h.accounts.is_empty() {
                        Cell::styled("not authed", out::DIM)
                    } else {
                        Cell::styled(h.accounts.join(", "), out::OK)
                    },
                ]);
            }
            s.push_str(&t.render(p));
        }

        // Editors are omitted entirely when there are none, rather than shown
        // empty: unlike a harness, an editor is optional, and a section headed
        // `editors:` with `(none)` under it reads as something missing.
        if !self.editors.is_empty() {
            s.push('\n');
            s.push_str(&out::heading(p, "editors:"));
            let mut t = Table::new();
            for e in &self.editors {
                t = t.row(vec![
                    Cell::styled(&e.name, out::NAME),
                    if e.installed {
                        Cell::styled("installed", out::OK)
                    } else {
                        Cell::styled("not installed", out::DIM)
                    },
                ]);
            }
            s.push_str(&t.render(p));
        }

        s.push('\n');
        s.push_str(&out::heading(p, "sessions:"));
        if self.sessions.is_empty() {
            s.push_str(&out::nothing(p, "none"));
        } else {
            let mut t = Table::new();
            for sess in &self.sessions {
                t = t.row(vec![
                    Cell::styled(&sess.id, out::NAME),
                    Cell::plain(&sess.label),
                    match sess.behind {
                        Some(0) | None => Cell::plain(""),
                        Some(n) => Cell::styled(format!("({n} behind {})", self.base), out::DIM),
                    },
                ]);
            }
            s.push_str(&t.render(p));
        }
        s
    }

    fn json(&self) -> serde_json::Value {
        json!({
            // Where a harness would be added. The human form only mentions it
            // in the empty-list message, which is exactly when it matters —
            // and omitting it here left a script diagnosing *why is nothing
            // installed* with no way to learn where to write the file.
            "adapters_dir": self.adapters_dir,
            "harnesses": self.harnesses.iter().map(|h| json!({
                "name": h.name,
                "accounts": h.accounts,
                "authed": !h.accounts.is_empty(),
            })).collect::<Vec<_>>(),
            "editors": self.editors.iter().map(|e| json!({
                "name": e.name,
                "installed": e.installed,
            })).collect::<Vec<_>>(),
            "sessions": self.sessions.iter().map(|s| json!({
                "id": s.id,
                "label": s.label,
                "behind": s.behind,
            })).collect::<Vec<_>>(),
            "base": self.base,
        })
    }
}

// ── omh doctor ──────────────────────────────────────────────────────────────

/// What a probe reported, and the circumstances it ran under.
///
/// The circumstances are fields rather than a printed header because they are
/// the first thing anybody asks about a failed check — *which image, whose
/// credentials* — and a header is exactly the part of the output that gets
/// lost when somebody pastes the failing lines into a bug report.
#[derive(Debug, Clone)]
pub struct Doctor {
    pub harness: String,
    pub tag: String,
    /// `None` means no account was staged, so credentials went unchecked.
    pub account: Option<String>,
    pub outcomes: Vec<crate::doctor::Outcome>,
}

impl Doctor {
    pub fn failed(&self) -> usize {
        self.outcomes.iter().filter(|o| !o.ok).count()
    }

    /// Through `doctor::passed`, which is the definition the command's exit
    /// code has always used — including its refusal to call an **empty** run a
    /// pass. Re-deriving it from `failed() == 0` here would quietly say yes to
    /// a probe that produced nothing.
    pub fn passed(&self) -> bool {
        crate::doctor::passed(&self.outcomes)
    }
}

impl Report for Doctor {
    fn human(&self, p: &out::Palette) -> String {
        let mut t = Table::new();
        for o in &self.outcomes {
            t = t.row(vec![
                if o.ok {
                    Cell::styled("✓", out::OK)
                } else {
                    Cell::styled("✗", out::BAD)
                },
                Cell::plain(&o.name),
                Cell::plain(&o.detail),
            ]);
        }
        let mut s = t.render(p);

        // Only the success line. A failure is reported by the command failing
        // — `out::problem` prints the tally in omh's error voice and the exit
        // status carries it — and saying it here too would print the same
        // sentence twice, once unstyled and once red, which reads like two
        // different problems.
        if self.passed() {
            s.push('\n');
            s.push_str(&format!(
                "  {}\n",
                p.paint(
                    out::OK,
                    &format!(
                        "all {} checks passed — {}'s adapter paths are verified",
                        self.outcomes.len(),
                        self.harness
                    )
                )
            ));
        }
        s
    }

    fn json(&self) -> serde_json::Value {
        json!({
            "harness": self.harness,
            "image": self.tag,
            "account": self.account,
            // `ok` is the verdict; the other two are tallies. Named apart on
            // purpose: `"passed": 3` beside a `passed()` that returns a bool
            // invites `jq -e '.passed'`, which is truthy on a run where two of
            // five checks failed — inverting the one thing this command exists
            // to tell you, in the format where the exit code is most likely to
            // have been thrown away.
            "ok": self.passed(),
            "passed_count": self.outcomes.len() - self.failed(),
            "failed_count": self.failed(),
            "checks": self.outcomes.iter().map(|o| json!({
                "name": o.name,
                "ok": o.ok,
                "detail": o.detail,
            })).collect::<Vec<_>>(),
        })
    }
}

// ── omh config ──────────────────────────────────────────────────────────────

/// One setting, and which file decided it.
#[derive(Debug, Clone)]
pub struct Setting {
    pub key: String,
    pub value: String,
    /// `None` where the command shows a single layer, and saying so on every
    /// row would be noise.
    pub whose: Option<String>,
}

/// One capability's slice of the catalogue.
#[derive(Debug, Clone)]
pub struct Catalogue {
    pub capability: String,
    pub entries: Vec<String>,
}

/// What `omh config` shows: your defaults, and what you have to draw on.
#[derive(Debug, Clone)]
pub struct Config {
    pub defaults_file: String,
    pub settings: Vec<Setting>,
    pub catalogue_dir: String,
    pub catalogue: Vec<Catalogue>,
}

impl Report for Config {
    fn human(&self, p: &out::Palette) -> String {
        let mut s = format!(
            "{} {}\n",
            p.paint(out::HEAD, "your defaults"),
            p.paint(out::DIM, &self.defaults_file)
        );
        if self.settings.is_empty() {
            s.push_str(&out::nothing(p, "nothing set"));
        } else {
            let mut t = Table::new();
            for setting in &self.settings {
                t = t.row(vec![
                    Cell::styled(&setting.key, out::NAME),
                    Cell::plain(&setting.value),
                ]);
            }
            s.push_str(&t.render(p));
        }

        s.push('\n');
        s.push_str(&format!(
            "{} {}\n",
            p.paint(out::HEAD, "your catalogue"),
            p.paint(out::DIM, &self.catalogue_dir)
        ));
        let mut t = Table::new();
        for c in &self.catalogue {
            // The count as well as the names: a catalogue is a thing that
            // grows, and this is the number the unselected report talks about.
            t = t.row(vec![
                Cell::plain(&c.capability),
                Cell::styled(c.entries.len().to_string(), out::DIM),
                Cell::plain(c.entries.join(", ")),
            ]);
        }
        s.push_str(&t.render(p));
        s
    }

    fn json(&self) -> serde_json::Value {
        json!({
            "defaults_file": self.defaults_file,
            "settings": self.settings.iter().map(|s| json!({
                "key": s.key,
                "value": s.value,
                "whose": s.whose,
            })).collect::<Vec<_>>(),
            "catalogue_dir": self.catalogue_dir,
            "catalogue": self.catalogue.iter().map(|c| json!({
                "capability": c.capability,
                "count": c.entries.len(),
                "entries": c.entries,
            })).collect::<Vec<_>>(),
        })
    }
}

/// What `omh config mcp` shows: every server, and which layer decided it.
#[derive(Debug, Clone)]
pub struct Servers {
    pub servers: Vec<Setting>,
}

impl Report for Servers {
    fn human(&self, p: &out::Palette) -> String {
        let mut s = out::heading(p, "mcp:");
        if self.servers.is_empty() {
            s.push_str(&out::nothing(p, "nothing set"));
            return s;
        }
        let mut t = Table::new();
        for server in &self.servers {
            // Content says whose it is; a setting says which file decided it.
            t = t.row(vec![
                Cell::styled(&server.key, out::NAME),
                Cell::plain(&server.value),
                Cell::styled(
                    format!("← {}", server.whose.as_deref().unwrap_or("?")),
                    out::DIM,
                ),
            ]);
        }
        s.push_str(&t.render(p));
        s
    }

    fn json(&self) -> serde_json::Value {
        json!({
            "servers": self.servers.iter().map(|s| json!({
                "name": s.key,
                "command": s.value,
                "whose": s.whose,
            })).collect::<Vec<_>>(),
        })
    }
}

// ── omh repo ────────────────────────────────────────────────────────────────

/// One effective setting, and what it beat to get there.
#[derive(Debug, Clone)]
pub struct Effective {
    pub key: String,
    pub value: String,
    pub layer: String,
    /// Layers this one overrode. Provenance is the point: a three-layer merge
    /// you cannot trace is worse than no layering at all.
    pub shadows: Vec<String>,
}

/// One of omh's own features, and whether this repo switched it off.
#[derive(Debug, Clone)]
pub struct Feature {
    pub name: String,
    pub on: bool,
}

/// What this repo uses for one capability.
#[derive(Debug, Clone)]
pub struct Using {
    pub capability: String,
    /// `None` means *follow the catalogue* — which is a different state from a
    /// list that happens to name everything in it today, because one keeps up
    /// as the catalogue grows and the other does not.
    pub selected: Option<Vec<String>>,
    pub unselected: Vec<String>,
}

impl Using {
    fn summary(&self) -> String {
        match &self.selected {
            None => "everything".into(),
            Some(taken) if taken.is_empty() => "nothing".into(),
            Some(taken) => taken.join(", "),
        }
    }
}

/// What is effective in this checkout, and which file decided it.
#[derive(Debug, Clone)]
pub struct Repo {
    pub dir: String,
    pub settings: Vec<Effective>,
    pub features: Vec<Feature>,
    pub using: Vec<Using>,
    /// Advisory lines the selection wants to add.
    pub notices: Vec<String>,
}

impl Report for Repo {
    fn human(&self, p: &out::Palette) -> String {
        let mut s = format!(
            "{} {}\n",
            p.paint(out::HEAD, "this repo"),
            p.paint(out::DIM, &self.dir)
        );

        s.push('\n');
        s.push_str(&out::heading(p, "settings"));
        if self.settings.is_empty() {
            s.push_str(&out::nothing(p, "nothing set"));
        } else {
            let mut t = Table::new();
            for e in &self.settings {
                t = t.row(vec![
                    Cell::styled(&e.key, out::NAME),
                    Cell::plain(&e.value),
                    Cell::styled(
                        if e.shadows.is_empty() {
                            format!("← {}", e.layer)
                        } else {
                            format!("← {} (overrides {})", e.layer, e.shadows.join(", "))
                        },
                        out::DIM,
                    ),
                ]);
            }
            s.push_str(&t.render(p));
        }

        s.push('\n');
        s.push_str(&out::heading(p, "omh's features"));
        let mut t = Table::new();
        for f in &self.features {
            t = t.row(vec![
                Cell::plain(&f.name),
                if f.on {
                    Cell::styled("on", out::OK)
                } else {
                    Cell::styled("off here", out::WARN)
                },
            ]);
        }
        s.push_str(&t.render(p));

        s.push('\n');
        s.push_str(&out::heading(p, "using"));
        let mut t = Table::new();
        for u in &self.using {
            t = t.row(vec![
                Cell::plain(&u.capability),
                Cell::plain(u.summary()),
                if u.unselected.is_empty() {
                    Cell::plain("")
                } else {
                    Cell::styled(
                        format!(
                            "({} not selected: {})",
                            u.unselected.len(),
                            u.unselected.join(", ")
                        ),
                        out::DIM,
                    )
                },
            ]);
        }
        s.push_str(&t.render(p));

        for line in &self.notices {
            s.push('\n');
            s.push_str(&format!("{line}\n"));
        }
        s
    }

    fn json(&self) -> serde_json::Value {
        json!({
            "dir": self.dir,
            "settings": self.settings.iter().map(|e| json!({
                "key": e.key,
                "value": e.value,
                "layer": e.layer,
                "overrides": e.shadows,
            })).collect::<Vec<_>>(),
            "features": self.features.iter().map(|f| json!({
                "name": f.name,
                "on": f.on,
            })).collect::<Vec<_>>(),
            "using": self.using.iter().map(|u| json!({
                "capability": u.capability,
                // `null` is *follows the catalogue*, and an array is a list.
                // Collapsing the two into an array of today's names would tell
                // a script the selection is pinned when it is not.
                "selected": u.selected,
                "unselected": u.unselected,
            })).collect::<Vec<_>>(),
            "notices": self.notices,
        })
    }
}

// ── omh use --all ───────────────────────────────────────────────────────────

/// Every capability's list rewritten to match the catalogue.
#[derive(Debug, Clone)]
pub struct Resynced {
    /// The files written — one per repo layer that has a say.
    pub wrote: Vec<String>,
    /// `(capability, how many entries)`.
    pub counts: Vec<(String, usize)>,
}

impl Report for Resynced {
    fn human(&self, p: &out::Palette) -> String {
        let mut s = String::new();
        for path in &self.wrote {
            s.push_str(&format!("resynced to your catalogue — wrote → {path}\n"));
        }
        let mut t = Table::new();
        for (capability, count) in &self.counts {
            t = t.row(vec![
                Cell::plain(capability),
                Cell::styled(count.to_string(), out::DIM),
            ]);
        }
        s.push_str(&t.render(p));
        s
    }

    fn json(&self) -> serde_json::Value {
        json!({
            "action": "catalogue-resynced",
            "wrote": self.wrote,
            "capabilities": self.counts.iter().map(|(capability, count)| json!({
                "capability": capability,
                "count": count,
            })).collect::<Vec<_>>(),
        })
    }
}

// ── omh import ──────────────────────────────────────────────────────────────

/// What happened to one thing an import looked at.
///
/// One vocabulary across all three importers — servers, catalogue entries,
/// hooks — because they are the same five outcomes wearing different words.
/// `omh mcp import` said `already identical` where `omh import skills` said
/// `already in your catalogue`, and a reader had to learn both to read either.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// Brought across.
    Took,
    /// Already there and the same; nothing to do.
    Kept,
    /// Already there and **different**. Never overwritten without `--force`.
    Conflict,
    /// Refused, with a reason — a bad name, an unreadable file.
    Skipped,
    /// omh could not translate it. Still in the harness's own file, still
    /// running there, which is the honest outcome — but somebody who was not
    /// told would think omh had taken everything.
    Left,
}

impl Verdict {
    /// The word, which carries the meaning on its own. A reader in a pipe, in
    /// CI, or without colour vision gets this and nothing else.
    ///
    /// **Words, not symbols.** `omh doctor` gets away with `✓`/`✗` because it
    /// has two outcomes and they are opposites. Five is past what a glyph can
    /// carry: `-` and `=` are one pixel apart in most terminal fonts, and the
    /// pair they would distinguish — *refused* from *already there* — is the
    /// one a reader most needs to tell apart. These words are also what the
    /// integration suite and anybody's shell alias already grep for.
    fn mark(&self) -> &'static str {
        match self {
            Self::Took => "imported",
            Self::Kept => "kept",
            Self::Conflict => "conflict",
            Self::Skipped => "skipped",
            Self::Left => "left",
        }
    }

    fn style(&self) -> anstyle::Style {
        match self {
            Self::Took => out::OK,
            Self::Kept => out::DIM,
            Self::Conflict | Self::Skipped => out::WARN,
            Self::Left => out::DIM,
        }
    }

    fn key(&self) -> &'static str {
        match self {
            Self::Took => "took",
            Self::Kept => "kept",
            Self::Conflict => "conflict",
            Self::Skipped => "skipped",
            Self::Left => "left",
        }
    }
}

/// One name, and what became of it.
#[derive(Debug, Clone)]
pub struct Considered {
    pub name: String,
    pub verdict: Verdict,
    /// Why — a conflict's advice, a skip's error. Empty where the verdict says
    /// it all.
    pub detail: String,
}

/// What an import brought across, and what it left alone.
#[derive(Debug, Clone, Default)]
pub struct Imported {
    /// What was read — a harness name, or `claude skills`.
    pub what: String,
    pub source: String,
    pub considered: Vec<Considered>,
    /// What omh calls the things it was looking for, for the empty message:
    /// "no servers found" reads better than "nothing found".
    pub noun: String,
    pub dry_run: bool,
    /// Where it landed, when anything did.
    pub wrote: Option<String>,
    /// Files a selection was written into, so the entries are not dead on
    /// arrival.
    pub selected_in: Vec<String>,
}

impl Imported {
    pub fn count(&self, verdict: Verdict) -> usize {
        self.considered
            .iter()
            .filter(|c| c.verdict == verdict)
            .count()
    }
}

impl Report for Imported {
    fn human(&self, p: &out::Palette) -> String {
        let mut s = format!(
            "import from {} {}\n",
            p.paint(out::HEAD, &self.what),
            p.paint(out::DIM, &format!("({})", self.source))
        );

        if self.considered.is_empty() {
            s.push_str(&out::nothing(p, &format!("no {} found", self.noun)));
            return s;
        }

        let mut t = Table::new();
        for c in &self.considered {
            t = t.row(vec![
                Cell::styled(c.verdict.mark(), c.verdict.style()),
                Cell::plain(&c.name),
                Cell::styled(&c.detail, out::DIM),
            ]);
        }
        s.push_str(&t.render(p));

        for path in &self.selected_in {
            s.push_str(&format!("  selected in {path}\n"));
        }

        if self.dry_run {
            s.push_str(&format!(
                "\n{}\n",
                p.paint(out::DIM, "--dry-run: nothing written")
            ));
        } else if let Some(path) = &self.wrote {
            s.push_str(&format!("\nwrote → {path}\n"));
        }
        s
    }

    fn json(&self) -> serde_json::Value {
        json!({
            "what": self.what,
            "source": self.source,
            "considered": self.considered.iter().map(|c| json!({
                "name": c.name,
                "verdict": c.verdict.key(),
                "detail": c.detail,
            })).collect::<Vec<_>>(),
            "took": self.count(Verdict::Took),
            "kept": self.count(Verdict::Kept),
            "conflicts": self.count(Verdict::Conflict),
            "skipped": self.count(Verdict::Skipped),
            "left": self.count(Verdict::Left),
            "dry_run": self.dry_run,
            "wrote": self.wrote,
            "selected_in": self.selected_in,
        })
    }
}

// ── omh init ────────────────────────────────────────────────────────────────

/// Everything `omh init` decided, in the order somebody reads it.
///
/// Assembled and reported once, rather than printed as it goes. The old
/// version interleaved outcomes with the work — an image line here, a
/// provision line four hundred lines later — which read fine on a terminal and
/// made `--json` impossible: a run would have emitted six unrelated objects,
/// none of them the answer to "what did init do".
///
/// Slow work still says so while it happens, through `Ctx::progress`, which is
/// stderr and human-only. Progress is not the report.
#[derive(Debug, Clone, Default)]
pub struct Init {
    /// How many questions were actually **put**, not how many were answered:
    /// a question declined was still a question asked, and the headline claims
    /// omh asked nothing.
    pub asked: usize,
    pub adapters: Vec<String>,
    pub editors: Vec<String>,
    pub harness: Option<String>,
    pub harness_on_host: bool,
    /// The base image, and the repo-specific one if a stack layer was built.
    pub image: Option<String>,
    pub stack_image: Option<String>,
    /// `(name, marker)` per detected stack.
    pub stacks: Vec<(String, String)>,
    pub provisioned: Vec<String>,
    /// Things that went wrong while provisioning without failing `init`.
    pub provision_problems: Vec<String>,
    /// `(hook, program it needs)` — written and travelling, but not running here.
    pub held_back: Vec<(String, String)>,
    pub importable: Vec<String>,
    pub memory: String,
    pub catalogue_dir: String,
    pub repo_dir: String,
    pub graph: Option<String>,
    pub base_set: String,
    pub rationale: Vec<(String, String)>,
    pub next_command: String,
}

impl Report for Init {
    fn human(&self, p: &out::Palette) -> String {
        // The headline is a claim about this run, so it has to be able to stop
        // being true. omh derives what it can and asks only what nothing could
        // derive; printing "asked nothing" after putting a question on screen
        // would break the promise the tagline is selling, in front of the
        // person who just watched it happen.
        let mut s = format!(
            "{}\n\n",
            p.paint(
                out::HEAD,
                &match self.asked {
                    0 => "omh init — decided, asked nothing".to_string(),
                    1 => "omh init — decided all but one question".to_string(),
                    n => format!("omh init — decided the rest; asked {n} questions"),
                }
            )
        );

        let mut t = Table::new();
        t = t.row(vec![
            Cell::plain("harnesses"),
            Cell::plain(format!(
                "{} ({})",
                self.adapters.len(),
                self.adapters.join(", ")
            )),
        ]);
        t = t.row(vec![
            Cell::plain("editors"),
            Cell::plain(format!(
                "{} ({})",
                self.editors.len(),
                self.editors.join(", ")
            )),
        ]);
        t = t.row(vec![
            Cell::plain("harness"),
            match &self.harness {
                Some(h) if self.harness_on_host => {
                    Cell::plain(format!("{h}  (found on your host)"))
                }
                Some(h) => Cell::plain(format!("{h}  (default; nothing detected on host)")),
                None => Cell::styled("none — no adapters available", out::WARN),
            },
        ]);
        if let Some(image) = &self.image {
            t = t.row(vec![Cell::plain("image"), Cell::plain(image)]);
        }
        if let Some(image) = &self.stack_image {
            t = t.row(vec![
                Cell::plain("image"),
                Cell::plain(format!("{image} (this repo's toolchain)")),
            ]);
        }
        if self.stacks.is_empty() {
            t = t.row(vec![
                Cell::plain("stack"),
                Cell::styled(
                    "none detected — write your test and format hooks into .omh/hooks/",
                    out::DIM,
                ),
            ]);
        }
        for (name, marker) in &self.stacks {
            // The marker and nothing else. What a stack's hooks run is the
            // hooks' business, and repeating a command here would be a second
            // copy free to disagree with the file that actually holds it.
            t = t.row(vec![
                Cell::plain("stack"),
                Cell::plain(format!("{name} (from {marker})")),
            ]);
        }
        for key in &self.provisioned {
            t = t.row(vec![Cell::plain("provision"), Cell::plain(key)]);
        }
        for problem in &self.provision_problems {
            t = t.row(vec![
                Cell::plain("provision"),
                Cell::styled(problem, out::WARN),
            ]);
        }
        // Named, with the evidence, because the alternative is the failure the
        // whole design replaces: a hook that runs on turn one and reports
        // `cargo: not found`, saying nothing about who decided to run cargo.
        for (name, wanted) in &self.held_back {
            t = t.row(vec![
                Cell::styled("held back", out::WARN),
                Cell::plain(format!("`{name}` needs {wanted}")),
            ]);
        }
        for line in &self.importable {
            t = t.row(vec![Cell::plain(""), Cell::plain(line)]);
        }
        t = t.row(vec![Cell::plain("memory"), Cell::plain(&self.memory)]);
        if let Some(graph) = &self.graph {
            t = t.row(vec![Cell::plain("graph"), Cell::plain(graph)]);
        }
        s.push_str(&t.render(p));

        // Derive, then confirm: a hypothesis worth correcting is not a
        // questionnaire.
        if self.stacks.len() > 1 {
            s.push_str(&format!(
                "\n  {} {} stacks detected; hooks were written for every command \
                 the sandbox can run.\n    drop the ones you do not want: .omh/hooks/\n",
                p.paint(out::WARN, "!"),
                self.stacks.len()
            ));
        }

        s.push('\n');
        let mut where_ = Table::new();
        where_ = where_.row(vec![
            Cell::plain("catalogue"),
            Cell::plain(&self.catalogue_dir),
        ]);
        where_ = where_.row(vec![
            Cell::plain("this repo"),
            Cell::plain(format!("{}  (committed)", self.repo_dir)),
        ]);
        s.push_str(&where_.render(p));

        s.push_str(&format!(
            "\n  {} ({})\n",
            p.paint(out::HEAD, "base set"),
            self.base_set
        ));
        let mut why = Table::new().indent(4);
        for (name, reason) in &self.rationale {
            why = why.row(vec![Cell::styled(name, out::NAME), Cell::plain(reason)]);
        }
        s.push_str(&why.render(p));

        // Named here because this is the moment somebody wonders what that is
        // and why it was installed without being asked.
        s.push_str(&format!(
            "\n  {}\n",
            p.paint(
                out::DIM,
                "omh why <name>  what it costs, what was considered instead, how to remove it"
            )
        ));
        s.push_str(&format!(
            "\n{}\n",
            p.paint(out::DIM, "not yet done: recall, cost accounting.")
        ));
        s.push_str(&format!("next: omh {}\n", self.next_command));
        s
    }

    fn json(&self) -> serde_json::Value {
        json!({
            "asked": self.asked,
            "adapters": self.adapters,
            "editors": self.editors,
            "harness": self.harness,
            "harness_on_host": self.harness_on_host,
            "image": self.image,
            "stack_image": self.stack_image,
            "stacks": self.stacks.iter().map(|(name, marker)| json!({
                "name": name,
                "marker": marker,
            })).collect::<Vec<_>>(),
            "provisioned": self.provisioned,
            "provision_problems": self.provision_problems,
            "held_back": self.held_back.iter().map(|(name, wanted)| json!({
                "hook": name,
                "needs": wanted,
            })).collect::<Vec<_>>(),
            "importable": self.importable,
            "memory": self.memory,
            "catalogue_dir": self.catalogue_dir,
            "repo_dir": self.repo_dir,
            "graph": self.graph,
            "base_set": self.base_set,
            "rationale": self.rationale.iter().map(|(name, why)| json!({
                "name": name,
                "why": why,
            })).collect::<Vec<_>>(),
            "next": self.next_command,
        })
    }
}

// ── omh memory stale ────────────────────────────────────────────────────────

/// One note, judged against whatever it said would outdate it.
#[derive(Debug, Clone)]
pub struct Judged {
    pub key: String,
    pub layer: String,
    pub recorded: String,
    pub age: Age,
    /// Why it is stale, or why omh could not tell.
    pub because: Option<String>,
}

/// How a note stands against the world it describes.
///
/// An enum rather than the heading string, and that is the point. This grouping
/// was a `match` on `memory::expiry::Verdict` precisely so the compiler owned
/// the mapping — its own comment records a verdict that once fell through a
/// `_` arm, was counted in no group, and vanished from the command. Splitting
/// the renderer into this module briefly reintroduced that: the producer
/// returned `"omh cannot tell"` and the renderer filtered on the same literal,
/// coupled by nothing but the spelling. Reword one side and the affected notes
/// match no group, are excluded from the fresh count, and disappear from the
/// human report while still appearing in the JSON.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Age {
    Stale,
    /// omh could not tell — never folded into `Fresh`.
    Unknown,
    /// Carries only its date; nothing said what would outdate it.
    NoTrigger,
    Fresh,
}

impl Age {
    /// The heading this sits under, or `None` for the one that is counted
    /// rather than listed.
    fn heading(&self) -> Option<&'static str> {
        match self {
            Self::Stale => Some("stale"),
            Self::Unknown => Some("omh cannot tell"),
            Self::NoTrigger => Some("no expiry — carries only its date"),
            Self::Fresh => None,
        }
    }

    /// The stable key a script reads. Headings are prose and may be reworded;
    /// these may not.
    fn key(&self) -> &'static str {
        match self {
            Self::Stale => "stale",
            Self::Unknown => "unknown",
            Self::NoTrigger => "no-trigger",
            Self::Fresh => "fresh",
        }
    }
}

/// The launch that would have happened.
///
/// `--dry-run` is the one place the runtime command line is the *product*, so
/// it goes to **stdout** where it can be redirected into a script, and it is
/// printed as one argv per line — the shape you can read, diff, and paste
/// behind a `docker` you are debugging.
#[derive(Debug, Clone)]
pub struct DryRun {
    pub status: String,
    pub worktree: String,
    /// The program and its arguments, program first.
    pub argv: Vec<String>,
}

impl Report for DryRun {
    fn human(&self, p: &out::Palette) -> String {
        let mut s = format!("{}\n", p.paint(out::HEAD, &self.status));
        s.push_str(&format!(
            "worktree {}\n\n",
            p.paint(out::DIM, &self.worktree)
        ));
        // Continued with `\` so the whole thing is one pasteable command, which
        // is the only reason anybody reads this output rather than `omh doctor`.
        s.push_str(&self.argv.join(" \\\n       "));
        s.push('\n');
        s
    }

    fn json(&self) -> serde_json::Value {
        json!({
            "status": self.status,
            "worktree": self.worktree,
            "argv": self.argv,
        })
    }
}

/// The shell `omh doctor --dry-run` would run in the sandbox.
///
/// Emitted **bare** in human form. The whole use of this output is
/// `omh doctor --dry-run | sh` or reading it beside the failure it explains,
/// and a heading or an indent would have to be stripped back off before either.
#[derive(Debug, Clone)]
pub struct Probe {
    pub script: String,
    pub checks: Vec<String>,
}

impl Report for Probe {
    fn human(&self, _p: &out::Palette) -> String {
        format!("{}\n", self.script)
    }

    fn json(&self) -> serde_json::Value {
        json!({
            "script": self.script,
            "checks": self.checks,
        })
    }
}

/// Why something is in your setup, as `omh why` explains it.
///
/// `why::render` already owns the prose — it is a page, not a table, and the
/// argument it makes has an order. This carries it through so that `--json`
/// gets the subject as a field rather than nothing at all.
#[derive(Debug, Clone)]
pub struct Why {
    pub thing: String,
    pub text: String,
}

impl Report for Why {
    fn human(&self, _p: &out::Palette) -> String {
        self.text.clone()
    }

    fn json(&self) -> serde_json::Value {
        json!({
            "thing": self.thing,
            "explanation": self.text,
        })
    }
}

/// A session's work against its base, as `git diff --stat` summarises it.
///
/// **A summary, not a patch.** `Session::diff` runs `--stat`, so this is the
/// file-and-line-count table and there is nothing here for `git apply` to
/// consume. The field is named for what it holds: calling it `patch` invited
/// `omh s diff --json | jq -r .patch | git apply`, which fails on every
/// session that has changed anything.
///
/// It reaches stdout **unchanged and unstyled** either way — omh adds only the
/// sentence for the empty case, because silence reads as breakage and the
/// useful thing to say is which comparison came up empty.
#[derive(Debug, Clone)]
pub struct Diff {
    pub label: String,
    pub base: String,
    pub summary: String,
}

impl Report for Diff {
    fn human(&self, p: &out::Palette) -> String {
        if self.summary.trim().is_empty() {
            return format!(
                "{}\n",
                p.paint(
                    out::DIM,
                    &format!("no changes on {} (against {})", self.label, self.base)
                )
            );
        }
        self.summary.clone()
    }

    fn json(&self) -> serde_json::Value {
        json!({
            "session": self.label,
            "base": self.base,
            "changed": !self.summary.trim().is_empty(),
            "summary": self.summary,
        })
    }
}

/// Notes moved from local to team.
///
/// The human text comes from `memory::promote::report`, which already knows
/// how to say it — including the `git add :/` line, whose whole point is that
/// promoting moves a file and does **not** send anything to a teammate. This
/// adds the keys as data so a script does not have to parse `promoted X → Y`.
#[derive(Debug, Clone)]
pub struct Promoted {
    pub text: String,
    pub keys: Vec<String>,
}

impl Report for Promoted {
    fn human(&self, _p: &out::Palette) -> String {
        self.text.clone()
    }

    fn json(&self) -> serde_json::Value {
        json!({
            "action": "notes-promoted",
            "keys": self.keys,
            "message": self.text.trim_end(),
        })
    }
}

/// What the store looks like against the world it describes.
#[derive(Debug, Clone)]
pub struct Stale {
    pub judged: Vec<Judged>,
}

impl Stale {
    /// The groups, in the order they are read: what is wrong first. `Fresh` is
    /// absent because it is counted rather than listed.
    const GROUPS: [Age; 3] = [Age::Stale, Age::Unknown, Age::NoTrigger];

    pub fn count(&self, age: Age) -> usize {
        self.judged.iter().filter(|j| j.age == age).count()
    }

    fn fresh(&self) -> usize {
        self.count(Age::Fresh)
    }
}

impl Report for Stale {
    fn human(&self, p: &out::Palette) -> String {
        let mut s = String::new();
        let mut printed = false;
        for age in Self::GROUPS {
            let members: Vec<&Judged> = self.judged.iter().filter(|j| j.age == age).collect();
            if members.is_empty() {
                continue;
            }
            if printed {
                s.push('\n');
            }
            printed = true;
            s.push_str(&out::heading(
                p,
                &format!("{}:", age.heading().expect("GROUPS excludes Fresh")),
            ));

            let mut t = Table::new();
            for j in members {
                // Every line carries its date and its layer, exactly as
                // `recall` does: a note reported without those cannot be
                // judged.
                t = t.row(vec![
                    Cell::styled(&j.key, out::NAME),
                    Cell::plain(&j.layer),
                    Cell::plain(&j.recorded),
                    match &j.because {
                        Some(because) => Cell::styled(format!("— {because}"), out::DIM),
                        None => Cell::plain(""),
                    },
                ]);
            }
            s.push_str(&t.render(p));
        }

        let fresh = self.fresh();
        if !printed && fresh == 0 {
            s.push_str(&format!("{}\n", p.paint(out::DIM, "no notes yet")));
        } else if fresh > 0 {
            s.push_str(&format!(
                "\n{}\n",
                p.paint(out::OK, &format!("{fresh} still current"))
            ));
        }
        s
    }

    fn json(&self) -> serde_json::Value {
        json!({
            "notes": self.judged.iter().map(|j| json!({
                "key": j.key,
                "layer": j.layer,
                "recorded": j.recorded,
                "verdict": j.age.key(),
                "because": j.because,
            })).collect::<Vec<_>>(),
            "fresh": self.fresh(),
            "stale": self.count(Age::Stale),
            "unknown": self.count(Age::Unknown),
        })
    }
}

// ── omh attach ──────────────────────────────────────────────────────────────

/// A session that is up, and the ways in.
///
/// The URL and the `ssh` line are printed **even when an editor opened the
/// session successfully** — they are the answer to "how do I get back to this",
/// which is asked long after the window has been closed. Only suppressing them
/// on success would make the useful output the one you never see.
#[derive(Debug, Clone)]
pub struct Attached {
    pub session: String,
    pub url: String,
    pub alias: String,
    /// The editor that actually opened it, if one did.
    pub opened_in: Option<String>,
    /// Every editor omh knows here, and the command that would open this
    /// session in it.
    pub editors: Vec<(String, String)>,
}

impl Report for Attached {
    fn human(&self, p: &out::Palette) -> String {
        let mut s = match &self.opened_in {
            Some(name) => format!(
                "opening {} in {}\n",
                p.paint(out::NAME, &self.url),
                p.paint(out::HEAD, name)
            ),
            None => format!("session {} is up\n", p.paint(out::NAME, &self.session)),
        };

        s.push('\n');
        s.push_str(&format!("  {}\n", self.url));
        s.push_str(&format!("  ssh {}\n", self.alias));

        if self.opened_in.is_none() && !self.editors.is_empty() {
            s.push('\n');
            let mut t = Table::new();
            for (name, command) in &self.editors {
                t = t.row(vec![
                    Cell::styled(name, out::NAME),
                    Cell::styled(command, out::DIM),
                ]);
            }
            s.push_str(&t.render(p));
        }
        s
    }

    fn json(&self) -> serde_json::Value {
        json!({
            "session": self.session,
            "url": self.url,
            "ssh_alias": self.alias,
            "opened_in": self.opened_in,
            "editors": self.editors.iter().map(|(name, command)| json!({
                "name": name,
                "command": command,
            })).collect::<Vec<_>>(),
        })
    }
}

// ── omh memory ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct Notes {
    pub notes: Vec<crate::memory::Note>,
}

impl Report for Notes {
    fn human(&self, p: &out::Palette) -> String {
        if self.notes.is_empty() {
            return format!(
                "{}\n",
                p.paint(
                    out::DIM,
                    "no notes yet — the store fills as work surprises the agent"
                )
            );
        }
        crate::memory::render_list(&self.notes)
    }

    fn json(&self) -> serde_json::Value {
        json!({
            "notes": self.notes.iter().map(|n| json!({
                "key": n.key,
                "kind": n.kind.to_string(),
                "layer": n.layer.to_string(),
                "source": n.source,
                "recorded": n.recorded,
                "invalidated_by": n.invalidated_by,
                "path": n.path.display().to_string(),
            })).collect::<Vec<_>>(),
        })
    }
}

/// What the store-quality meter found.
///
/// Grouped by rule as well as listed flat, because the count per rule is the
/// signal and the individual lines are how you act on it.
#[derive(Debug, Clone)]
pub struct Lint {
    pub violations: Vec<crate::memory::Violation>,
    /// How many violations each rule accounts for, in the rules' own order —
    /// which is stable between runs, unlike a sort by count, so two lint
    /// outputs can be diffed against each other.
    pub tally: std::collections::BTreeMap<crate::memory::Rule, usize>,
}

impl Lint {
    /// Through `memory::refused` rather than counted again here. Two notions
    /// of *what the schema refuses* is how the exit code and the report start
    /// disagreeing — and the exit code is the half nobody reads until CI is
    /// already green on a store that should have failed.
    pub fn refused(&self) -> usize {
        crate::memory::refused(&self.violations)
    }
}

impl Report for Lint {
    fn human(&self, p: &out::Palette) -> String {
        if self.violations.is_empty() {
            return format!("{}\n", p.paint(out::OK, "no violations"));
        }

        let mut t = Table::new().indent(0);
        for v in &self.violations {
            let refused = matches!(v.rule.severity(), crate::memory::Severity::Refused);
            t = t.row(vec![
                // The severity word carries the distinction, and the colour
                // only repeats it — `omh memory lint` is read in CI, where
                // there is no colour at all.
                if refused {
                    Cell::styled("refused", out::BAD)
                } else {
                    Cell::styled("warning", out::WARN)
                },
                Cell::plain(v.layer.to_string()),
                Cell::plain(&v.detail),
            ]);
        }
        let mut s = t.render(p);

        s.push('\n');
        let mut counts = Table::new();
        for (rule, count) in &self.tally {
            counts = counts.row(vec![
                Cell::styled(count.to_string(), out::HEAD),
                Cell::plain(format!("{rule:?}")),
            ]);
        }
        s.push_str(&counts.render(p));
        s
    }

    fn json(&self) -> serde_json::Value {
        json!({
            "violations": self.violations.iter().map(|v| json!({
                "key": v.key,
                "layer": v.layer.to_string(),
                "rule": format!("{:?}", v.rule),
                "severity": match v.rule.severity() {
                    crate::memory::Severity::Refused => "refused",
                    crate::memory::Severity::Warning => "warning",
                },
                "detail": v.detail,
            })).collect::<Vec<_>>(),
            "tally": self.tally.iter().map(|(rule, count)| json!({
                "rule": format!("{rule:?}"),
                "count": count,
            })).collect::<Vec<_>>(),
            "refused": self.refused(),
            "warnings": self.violations.len() - self.refused(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::out::{emit, Format, Palette};

    fn session(id: &str, work: Work) -> Session {
        Session {
            id: id.into(),
            label: "claude".into(),
            running: false,
            work: Some(work),
            behind: Some(0),
        }
    }

    fn sessions(rows: Vec<Session>) -> Sessions {
        Sessions {
            sessions: rows,
            base: "main".into(),
            leftovers: vec![],
        }
    }

    /// **A session omh cannot read is never rendered as clean**, in either
    /// format.
    ///
    /// The worktree's `.git` is a pointer at an absolute path, and a checkout
    /// that moves leaves it dangling. Every accessor then fails, and the
    /// tempting default — treat a failed count as zero — renders a session
    /// holding a day of work exactly like one with nothing in it. That is the
    /// state in which someone runs `s rm`.
    ///
    /// `tests/cli.rs` already guards the human column through the binary. This
    /// guards **JSON too**, which that test cannot see and which is the format
    /// where the confusion is worse: a script comparing `count == 0` gets no
    /// hint that the count is a fiction, whereas a person at least sees a blank
    /// and wonders.
    #[test]
    fn a_session_omh_cannot_read_is_clean_in_neither_format() {
        let report = sessions(vec![session("s01", Work::Unknown)]);

        let human = emit(&report, Format::Human, &Palette::plain());
        assert!(
            human.contains('?'),
            "omh cannot tell, and must say so rather than imply clean — got {human:?}"
        );

        let machine = report.json();
        let state = &machine["sessions"][0]["work"]["state"];
        assert_eq!(
            state, "unknown",
            "and a script must be able to tell not-known from nothing-to-do"
        );
        assert_ne!(state, "clean");
        assert!(
            machine["sessions"][0]["work"].get("count").is_none(),
            "an unknown state carries no count, because inventing 0 is the bug"
        );
    }

    /// The two renderers describe the same list.
    ///
    /// The whole reason a command returns a value instead of printing: there is
    /// one list, and both methods walk it. A person and a script that disagreed
    /// about how many sessions exist would be a bug nobody could reproduce,
    /// because each would be looking at only one of the two.
    #[test]
    fn a_person_and_a_script_are_told_about_the_same_sessions() {
        let report = sessions(vec![
            session("s01", Work::Uncommitted(3)),
            session("s02", Work::Published("feat/a".into())),
        ]);

        let human = emit(&report, Format::Human, &Palette::plain());
        let machine = report.json();

        assert_eq!(machine["sessions"].as_array().unwrap().len(), 2);
        for id in ["s01", "s02"] {
            assert!(human.contains(id), "{id} is missing from {human:?}");
        }
        assert_eq!(
            human.lines().filter(|l| !l.trim().is_empty()).count(),
            2,
            "one line per session, and no more — got {human:?}"
        );
    }

    /// Every wording `tests/cli.rs` pins, in one place.
    ///
    /// These strings are a contract: they are what the integration suite greps
    /// for through the real binary, and what a user's own `grep` in a shell
    /// alias is looking at. Restating them as a unit test means a change to the
    /// wording fails here — where the diff is one line and the reason is
    /// obvious — rather than in a test that has to build a git repo first.
    #[test]
    fn the_column_says_what_it_has_always_said() {
        assert_eq!(Work::Uncommitted(1).human(), "1 uncommitted");
        assert_eq!(Work::ToPush(1).human(), "1 to push");
        assert_eq!(Work::Published("feat/a".into()).human(), "→ feat/a");
        assert_eq!(Work::Unknown.human(), "?");
        assert_eq!(Work::Clean.human(), "", "clean is the quiet one");
    }

    fn inventory(harnesses: Vec<Harness>) -> Inventory {
        Inventory {
            harnesses,
            adapters_dir: "/home/u/.omh/adapters".into(),
            editors: vec![],
            sessions: vec![],
            base: "main".into(),
        }
    }

    /// **A harness nobody has authed is listed, not hidden.**
    ///
    /// The reason to run `omh ls` at all is usually "why can't it log in", and
    /// the answer is a harness with no accounts. Filtering the empty case out
    /// of either format — the tempting `if !accounts.is_empty()` — deletes the
    /// one row the user came to see, and leaves a list that looks complete.
    #[test]
    fn a_harness_with_no_account_is_the_row_you_came_to_read() {
        let report = inventory(vec![
            Harness {
                name: "claude".into(),
                accounts: vec![],
            },
            Harness {
                name: "opencode".into(),
                accounts: vec!["work".into()],
            },
        ]);

        let human = emit(&report, Format::Human, &Palette::plain());
        assert!(
            human.contains("claude") && human.contains("not authed"),
            "the un-authed harness is named and its state given — got {human:?}"
        );

        let machine = report.json();
        assert_eq!(
            machine["harnesses"].as_array().unwrap().len(),
            2,
            "both harnesses reach a script, authed or not"
        );
        assert_eq!(machine["harnesses"][0]["authed"], false);
        assert_eq!(machine["harnesses"][1]["authed"], true);
    }

    /// Every section a person is shown is a key a script can read.
    ///
    /// The two renderers are written by hand and separately, which is where
    /// they drift: a section added to `human` and forgotten in `json` leaves
    /// `--json` quietly less useful than the default, and nothing fails.
    #[test]
    fn no_section_reaches_a_person_without_also_reaching_a_script() {
        let report = Inventory {
            harnesses: vec![Harness {
                name: "claude".into(),
                accounts: vec![],
            }],
            editors: vec![Editor {
                name: "vscode".into(),
                installed: true,
            }],
            sessions: vec![session("s01", Work::Clean)],
            ..inventory(vec![])
        };

        let human = emit(&report, Format::Human, &Palette::plain());
        let machine = report.json();
        for section in ["harnesses", "editors", "sessions"] {
            assert!(
                human.contains(&format!("{section}:")),
                "{section} is missing from the human report — got {human:?}"
            );
            assert!(
                machine[section].as_array().is_some_and(|a| !a.is_empty()),
                "{section} is missing from the machine report — got {machine}"
            );
        }
    }

    fn check(name: &str, ok: bool) -> crate::doctor::Outcome {
        crate::doctor::Outcome {
            name: name.into(),
            ok,
            detail: if ok { "resolves" } else { "missing" }.into(),
        }
    }

    /// **Colour is never the only thing carrying the answer.**
    ///
    /// `omh doctor` is read on a pipe, in CI logs, by users with `NO_COLOR`
    /// set, and by the roughly one in twelve men who cannot tell this
    /// particular red from this particular green. Every one of those readers
    /// gets `Palette::plain`, and if the pass/fail distinction lived in the
    /// style alone they would get a list of identical-looking lines — from the
    /// one command whose entire purpose is to say which thing is broken.
    ///
    /// So the mark is a character first and a colour second, and this asserts
    /// the character, with the palette deliberately switched off.
    #[test]
    fn a_failed_check_is_legible_with_no_colour_at_all() {
        let report = Doctor {
            harness: "claude".into(),
            tag: "omh/claude:abc".into(),
            account: None,
            outcomes: vec![check("rules", true), check("mcp", false)],
        };

        let human = emit(&report, Format::Human, &Palette::plain());
        assert!(
            !human.contains('\x1b'),
            "the premise: this reader has no colour at all"
        );

        let mcp = human
            .lines()
            .find(|l| l.contains("mcp"))
            .expect("the failing check is listed");
        let rules = human
            .lines()
            .find(|l| l.contains("rules"))
            .expect("the passing check is listed");
        assert_ne!(
            mcp.chars().find(|c| !c.is_whitespace()),
            rules.chars().find(|c| !c.is_whitespace()),
            "pass and fail must differ by more than colour — got {human:?}"
        );
        assert!(
            !human.contains("checks passed"),
            "and a run with a failure in it never claims success — got {human:?}"
        );
    }

    /// The tally and the list cannot disagree, and **the verdict is a bool**.
    ///
    /// The counts are derived, not stored, so a script can trust them against
    /// `checks` — the alternative is two numbers maintained by hand that drift
    /// the first time a check is added on one path only.
    ///
    /// The `ok` half is the guard that matters. A field called `passed`
    /// holding `1` reads as *this passed* to `jq -e '.passed'` and to
    /// `if data["passed"]:`, and it is truthy on a run where two of five
    /// checks failed — the exact inversion of what `omh doctor` is for, in the
    /// format where the exit code has most likely been discarded.
    #[test]
    fn the_tally_is_the_list_counted_and_the_verdict_is_not_a_tally() {
        let report = Doctor {
            harness: "claude".into(),
            tag: "t".into(),
            account: Some("work".into()),
            outcomes: vec![check("a", true), check("b", false), check("c", false)],
        };
        let machine = report.json();
        assert_eq!(machine["failed_count"], 2);
        assert_eq!(machine["passed_count"], 1);
        assert_eq!(
            machine["passed_count"].as_u64().unwrap() + machine["failed_count"].as_u64().unwrap(),
            machine["checks"].as_array().unwrap().len() as u64
        );
        assert_eq!(
            machine["ok"],
            json!(false),
            "the verdict is a bool, and a run with failures in it is false"
        );
        assert!(
            machine["ok"].is_boolean(),
            "never a count — a truthy number here says `passed` about a failed run"
        );
        assert_eq!(
            machine["account"], "work",
            "and whose credentials were checked is on the record, not in a header"
        );
    }

    /// An empty probe is not a pass, in the machine format either.
    ///
    /// `doctor::passed` refuses to call an empty run a success, and `ok` goes
    /// through it. Deriving the verdict as `failed_count == 0` here instead
    /// would report `true` for a probe that produced nothing at all — the
    /// state a broken sandbox leaves behind.
    #[test]
    fn a_probe_that_produced_nothing_is_not_reported_as_a_pass() {
        let empty = Doctor {
            harness: "claude".into(),
            tag: "t".into(),
            account: None,
            outcomes: vec![],
        };
        assert_eq!(empty.failed(), 0, "nothing failed, because nothing ran");
        assert_eq!(
            empty.json()["ok"],
            json!(false),
            "and that is still not a pass"
        );
    }

    /// **Following the catalogue is not the same as listing everything in it.**
    ///
    /// A capability with no selection tracks the catalogue as it grows; a
    /// selection that happens to name all of today's entries does not. They
    /// look identical the moment you print them as a list of names, and they
    /// diverge the first time somebody adds a skill — one repo gets it, the
    /// other silently does not, and `omh repo` said the same thing about both.
    ///
    /// So the human form says `everything`, and the machine form says `null`
    /// rather than an array.
    #[test]
    fn following_the_catalogue_is_not_a_list_that_happens_to_be_complete() {
        let report = Repo {
            dir: "/r/.omh".into(),
            settings: vec![],
            features: vec![],
            using: vec![
                Using {
                    capability: "rules".into(),
                    selected: None,
                    unselected: vec![],
                },
                Using {
                    capability: "skills".into(),
                    selected: Some(vec!["a".into(), "b".into()]),
                    unselected: vec![],
                },
            ],
            notices: vec![],
        };

        let human = emit(&report, Format::Human, &Palette::plain());
        assert!(
            human.contains("everything"),
            "an unpinned capability says so in words — got {human:?}"
        );

        let machine = report.json();
        assert!(
            machine["using"][0]["selected"].is_null(),
            "and as null to a script, not as an array of today's names"
        );
        assert_eq!(
            machine["using"][1]["selected"],
            json!(["a", "b"]),
            "while a real selection is the list it is"
        );
    }

    /// A setting says which layer won **and** what it beat.
    ///
    /// `omh repo` exists because of the three-layer merge, and the question it
    /// is opened to answer is "why is this value this". A row that gives the
    /// winner and drops the losers answers the easy half.
    #[test]
    fn an_overridden_setting_names_what_it_overrode() {
        let report = Repo {
            dir: "/r/.omh".into(),
            settings: vec![Effective {
                key: "account".into(),
                value: "work".into(),
                layer: "local".into(),
                shadows: vec!["shared".into(), "personal".into()],
            }],
            features: vec![],
            using: vec![],
            notices: vec![],
        };

        let human = emit(&report, Format::Human, &Palette::plain());
        for part in ["account", "work", "local", "shared", "personal"] {
            assert!(
                human.contains(part),
                "{part} is missing from the provenance — got {human:?}"
            );
        }
        assert_eq!(
            report.json()["settings"][0]["overrides"],
            json!(["shared", "personal"])
        );
    }

    /// **A script reads fields, never the sentence.**
    ///
    /// The lazy `--json` is `{"message": "removed session s01; branch kept"}`,
    /// which is the human string in a JSON wrapper: to learn the id, a caller
    /// has to match English that we reword whenever it reads badly. Every
    /// `Action` therefore carries a stable `kind` and its facts as fields, and
    /// this is what stops the next one being added with prose alone.
    #[test]
    fn an_action_gives_a_script_fields_and_not_a_sentence_to_parse() {
        let action = Action::new(
            "session-removed",
            "removed session s01; branch omh/s01 kept",
        )
        .next("git log main..omh/s01")
        .data(json!({ "session": "s01", "branch_kept": true, "commits": 3 }));

        let machine = action.json();
        assert_eq!(machine["action"], "session-removed");
        assert_eq!(
            machine["session"], "s01",
            "the id is a field, not something to regex out of the message"
        );
        assert_eq!(machine["branch_kept"], true);
        assert_eq!(machine["commits"], 3);

        let human = emit(&action, Format::Human, &Palette::plain());
        assert!(human.starts_with("removed session s01"));
        assert!(
            !human.contains("git log main..omh/s01"),
            "the next step is not part of the answer — it would land in a \
             redirected stdout — got {human:?}"
        );
        let hints = action.asides().hints;
        assert_eq!(
            hints.iter().map(|h| h.trim()).collect::<Vec<_>>(),
            vec!["git log main..omh/s01"],
            "it is still offered to the person, on stderr"
        );
    }

    /// The suggested command is reproduced exactly, so it can be pasted.
    ///
    /// A hint that has been re-wrapped, re-quoted or prefixed with a bullet is
    /// a hint that fails when pasted, and the user blames the command rather
    /// than the formatting. Indentation is the only decoration allowed.
    ///
    /// The second half is the one worth having, and it is here because the
    /// first draft of `memory rm` got it wrong: an English consequence was
    /// passed to `next`, which claims every line under it is runnable. One
    /// prose line in that list makes the reader check all of them, so the two
    /// have separate fields and separate keys in JSON.
    #[test]
    fn a_suggested_command_survives_being_pasted() {
        let action = Action::new("x", "done")
            .next("omh s rm s01")
            .note("teammates keep it until you commit the deletion");

        let hints = action.asides().hints;
        assert!(
            hints.iter().any(|l| l.trim() == "omh s rm s01"),
            "the command is handed over verbatim — got {hints:?}"
        );
        let human = emit(&action, Format::Human, &Palette::plain());
        assert!(
            human.contains("teammates keep it"),
            "the consequence is the answer and stays on stdout — got {human:?}"
        );

        let machine = action.json();
        assert_eq!(
            machine["next"],
            json!(["omh s rm s01"]),
            "`next` is runnable commands and nothing else"
        );
        assert_eq!(
            machine["notes"],
            json!(["teammates keep it until you commit the deletion"]),
            "and prose has its own key, so a script can run one and show the other"
        );
    }

    /// **What omh could not import is reported, never dropped.**
    ///
    /// The two quiet outcomes are the ones that matter. A hook omh cannot
    /// translate is still in the harness's own file and still running there;
    /// a skill refused for reaching outside itself was refused for a reason
    /// somebody needs to hear. Both are easy to filter out of a report — they
    /// are the boring rows — and both leave the user believing omh took
    /// everything.
    ///
    /// The words are asserted because they are a contract: `tests/cli.rs`
    /// greps them through the real binary, and so does anybody's shell alias.
    #[test]
    fn what_omh_would_not_take_is_named_and_not_merely_absent() {
        let report = Imported {
            what: "claude hooks".into(),
            source: "/h/settings.json".into(),
            considered: vec![
                Considered {
                    name: "fmt".into(),
                    verdict: Verdict::Took,
                    detail: "runs on save".into(),
                },
                Considered {
                    name: "sneaky".into(),
                    verdict: Verdict::Skipped,
                    detail: "is a symlink".into(),
                },
                Considered {
                    name: "PreToolUse[0]".into(),
                    verdict: Verdict::Left,
                    detail: "a handler with `if`, which omh cannot express".into(),
                },
            ],
            noun: "hooks".into(),
            ..Default::default()
        };

        let human = emit(&report, Format::Human, &Palette::plain());
        for (word, why) in [
            ("skipped", "is a symlink"),
            ("left", "which omh cannot express"),
        ] {
            assert!(
                human.contains(word) && human.contains(why),
                "{word} and its reason must both survive — got {human:?}"
            );
        }

        let machine = report.json();
        assert_eq!(machine["took"], 1);
        assert_eq!(machine["skipped"], 1);
        assert_eq!(machine["left"], 1);
        assert_eq!(
            machine["considered"].as_array().unwrap().len(),
            3,
            "and every name is in the list, whatever became of it"
        );
    }

    /// An empty list says so, rather than printing nothing at all.
    ///
    /// A command that exits 0 having written nothing is indistinguishable from
    /// one that crashed before it got started.
    #[test]
    fn nothing_to_report_is_still_something_to_say() {
        let human = emit(&sessions(vec![]), Format::Human, &Palette::plain());
        assert_eq!(human.trim(), "no sessions");

        let machine = sessions(vec![]).json();
        assert_eq!(
            machine["sessions"].as_array().unwrap().len(),
            0,
            "and the machine format is an empty list, not a missing key"
        );
    }
}
