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

// ── omh sNN sync ────────────────────────────────────────────────────────────

/// What a sync brought over, and what it could not settle.
#[derive(Debug, Clone)]
pub struct Synced {
    pub id: String,
    pub base: String,
    /// Where the base now is.
    pub onto: String,
    /// How many commits arrived on it.
    pub moved: usize,
    /// Paths that need a decision, with markers in them.
    pub conflicted: Vec<String>,
    /// Whether there was uncommitted work to checkpoint first.
    pub checkpoint: bool,
    /// Why the agent will not be told about this at its next start, if it
    /// will not be.
    ///
    /// `None` is the ordinary answer. A `Some` is not a failed sync — the sync
    /// is done by the time this is known — but the user has to hear it, because
    /// the thing they were promised silently did not happen.
    pub note: Option<String>,
}

impl Report for Synced {
    fn human(&self, p: &out::Palette) -> String {
        let mut s = out::heading(
            p,
            &format!(
                "{} · {} commit{} from {}",
                self.id,
                self.moved,
                if self.moved == 1 { "" } else { "s" },
                self.base
            ),
        );
        s.push('\n');

        if self.conflicted.is_empty() {
            s.push_str(&out::nothing(p, "merged cleanly — nothing needs deciding"));
            return s;
        }
        // Named, every one, because each is a decision somebody has to make and
        // a count is not something you can act on.
        s.push_str(&format!(
            "  {}\n",
            p.paint(
                out::WARN,
                &format!(
                    "{} file{} resolving:",
                    self.conflicted.len(),
                    if self.conflicted.len() == 1 {
                        " needs"
                    } else {
                        "s need"
                    }
                )
            )
        ));
        for path in &self.conflicted {
            // The agent chose these names; git quotes what it must, and this
            // does not trust that.
            s.push_str(&format!("    {}\n", out::untrusted(path)));
        }
        s
    }

    fn json(&self) -> serde_json::Value {
        json!({
            "session": self.id,
            "base": self.base,
            "onto": self.onto,
            "moved": self.moved,
            "conflicted": self.conflicted,
            "checkpointed": self.checkpoint,
            "noted": self.note.is_none(),
        })
    }

    fn asides(&self) -> out::Asides {
        let mut asides = out::Asides::default();
        if let Some(why) = &self.note {
            // A warning rather than a hint, and it says what to do instead: the
            // commit is on disk either way, so nothing is lost that cannot be
            // read — only the sentence that would have said to go and read it.
            asides = asides.warn(format!(
                "the sandbox will not be told this happened — omh could not leave the note \
                 ({}). It will find `base moved to {}` in its own log: {why}",
                self.id, self.onto
            ));
        }
        if self.checkpoint {
            asides = asides.hint(format!(
                "  omh {} log             the checkpoint this can be undone from",
                self.id
            ));
        }
        if !self.conflicted.is_empty() {
            // The agent is the one holding the whole tree, and the sandbox is
            // where a conflict is safe to be wrong in. Say that rather than
            // leaving the user to open the files themselves.
            asides = asides.hint(format!(
                "  omh {} resume          the markers are in the sandbox, where fixing \
                 them cannot hurt you",
                self.id
            ));
        }
        asides
    }
}

/// Every session synced against trunk, in one report.
///
/// `omh s sync --all` moves trunk into each session in turn and stops at the
/// first that cannot go cleanly — a conflict to resolve, or an error. Stopping
/// is the point: a conflict wants a person, and pressing on would bury the one
/// that needs deciding under a wall of ones that did not.
#[derive(Debug, Clone)]
pub struct SyncedAll {
    /// The sessions brought forward cleanly, in order.
    pub done: Vec<Synced>,
    /// The session it stopped at, and why — a conflict or an error. `None`
    /// when every session synced cleanly.
    pub stopped: Option<SyncStop>,
    /// The sessions after the stop, not reached. Empty when nothing stopped.
    pub untouched: Vec<String>,
}

/// Why `sync --all` stopped.
#[derive(Debug, Clone)]
pub enum SyncStop {
    /// It synced, but the result needs a person: markers to resolve.
    Conflict(Synced),
    /// It could not sync — a running sandbox that must be stopped first, a git
    /// failure. Carries the session and the reason.
    Error { id: String, why: String },
}

impl Report for SyncedAll {
    fn human(&self, p: &out::Palette) -> String {
        let mut s = out::heading(
            p,
            &format!(
                "synced {} session{}",
                self.done.len(),
                if self.done.len() == 1 { "" } else { "s" }
            ),
        );
        s.push('\n');
        for synced in &self.done {
            s.push_str(&format!(
                "  {} · {} from {}\n",
                p.paint(out::NAME, &synced.id),
                match synced.moved {
                    1 => "1 commit".to_string(),
                    n => format!("{n} commits"),
                },
                synced.base
            ));
        }
        match &self.stopped {
            None => {}
            Some(SyncStop::Conflict(synced)) => s.push_str(&format!(
                "\n  {}\n",
                p.paint(
                    out::WARN,
                    &format!(
                        "stopped at {}: {} file{} to resolve",
                        synced.id,
                        synced.conflicted.len(),
                        if synced.conflicted.len() == 1 {
                            ""
                        } else {
                            "s"
                        }
                    )
                )
            )),
            Some(SyncStop::Error { id, why }) => s.push_str(&format!(
                "\n  {}\n",
                p.paint(
                    out::WARN,
                    &format!("stopped at {id}: {}", out::untrusted(why))
                )
            )),
        }
        if !self.untouched.is_empty() {
            s.push_str(&format!(
                "  {}\n",
                p.paint(
                    out::DIM,
                    &format!("not reached: {}", self.untouched.join(", "))
                )
            ));
        }
        s
    }

    fn json(&self) -> serde_json::Value {
        json!({
            "done": self.done.iter().map(Report::json).collect::<Vec<_>>(),
            "stopped": match &self.stopped {
                None => serde_json::Value::Null,
                Some(SyncStop::Conflict(synced)) => json!({
                    "session": synced.id,
                    "why": "conflict",
                    "conflicted": synced.conflicted,
                }),
                Some(SyncStop::Error { id, why }) => json!({
                    "session": id,
                    "why": "error",
                    "reason": why,
                }),
            },
            "untouched": self.untouched,
        })
    }

    fn asides(&self) -> out::Asides {
        let mut asides = out::Asides::default();
        match &self.stopped {
            Some(SyncStop::Conflict(synced)) => {
                asides = asides.hint(format!(
                    "  omh {} resume          resolve the markers in the sandbox, then \
                     `omh s sync --all` again",
                    synced.id
                ));
            }
            Some(SyncStop::Error { id, .. }) => {
                asides = asides.hint(format!(
                    "  omh {id} sync --down     sync this one, stopping its sandbox, then \
                     `omh s sync --all` again"
                ));
            }
            None => {}
        }
        asides
    }
}

// ── omh sNN log ─────────────────────────────────────────────────────────────

/// What the agent has committed inside the sandbox, and where the line is.
///
/// The command that changes how a session feels: until it existed you could
/// not tell the agent had been committing at all, and `--keep` opening a rebase
/// todo was the first sight of it.
#[derive(Debug)]
pub struct Log {
    pub id: String,
    pub read: crate::shadow::Checkpoints,
    /// How far the session's branch trails the base, or `None` when omh could
    /// not tell — which is not the same as zero and does not print as it.
    pub behind: Option<usize>,
    pub base: String,
    /// omh's own snapshots of the tree, one per turn — `None` unless asked
    /// for.
    ///
    /// Its own list, never merged into `read.commits`. Two reasons, both
    /// load-bearing: `diff <n>` and `--keep 1,3-4` index that list by number,
    /// so a snapshot in it would become selectable and then replayable; and
    /// the "yours from here" divider is an index into rendered rows, so an
    /// interleaved list would label rows as already on the branch that are
    /// not — the exact failure `cleanly_split` exists to prevent.
    pub turns: Option<Vec<crate::shadow::Turn>>,
}

impl Log {
    /// Checkpoints the next `--keep` would take.
    fn pending(&self) -> usize {
        self.read.commits.iter().filter(|c| !c.landed).count()
    }

    /// Whether one line can say which work is already the branch's.
    ///
    /// It can when the landed checkpoints are the oldest ones and nothing else
    /// — the shape a session has when `--keep` has simply been run once. A
    /// merge breaks it: `landed` means *ancestor of the replay point*, not
    /// *older*, so a landed commit can sit above an unlanded one. Then a
    /// divider does not merely fall in an awkward place, it **labels rows as
    /// already on the branch that are not** — about work `omh sNN rm` would
    /// destroy. So it is not drawn, and the numbers are named instead.
    fn cleanly_split(&self) -> bool {
        let pending = self.pending();
        self.read.commits[..self.read.commits.len() - pending]
            .iter()
            .all(|c| c.landed)
    }

    /// States in which the list is not the whole story.
    ///
    /// Both are states `harvest` refuses over. A log that showed neither would
    /// let a user read a clean review and then be refused by `--keep` citing
    /// work they were never shown.
    fn incomplete(&self) -> bool {
        self.read.unreachable > 0 || self.read.replay_point_lost
    }
}

/// A duration as a person reads it: `0s`, `12m`, `3h`, `9d`.
///
/// Pure, and given the seconds rather than reading a clock, so the rendering is
/// testable without one. Deliberately one unit — this is a column beside a
/// subject, and *2 hours 14 minutes ago* buys precision nobody is using to
/// decide whether to read a diff.
fn ago(seconds: u64) -> String {
    match seconds {
        s if s < 60 => format!("{s}s"),
        s if s < 60 * 60 => format!("{}m", s / 60),
        // Hours as far as two days, because *36h* still reads as "yesterday
        // evening" while `1d` has already thrown that away.
        s if s < 48 * 60 * 60 => format!("{}h", s / (60 * 60)),
        s => format!("{}d", s / (24 * 60 * 60)),
    }
}

impl Log {
    /// The snapshot view: omh's own commits, on their own, or a sentence
    /// saying there are none.
    ///
    /// A whole separate rendering rather than an extra column, because the two
    /// lists answer different questions. The agent's commits are what a
    /// harvest will take; these are what the tree looked like when each turn
    /// ended, and their numbers name nothing a command will accept.
    fn turns_human(&self, p: &out::Palette, turns: &[crate::shadow::Turn]) -> String {
        let mut s = out::heading(
            p,
            &format!(
                "{} · {} turn{}",
                self.id,
                turns.len(),
                if turns.len() == 1 { "" } else { "s" }
            ),
        );
        s.push('\n');
        if turns.is_empty() {
            s.push_str(&out::nothing(
                p,
                "no turns recorded — nothing has been photographed in this sandbox yet",
            ));
            return s;
        }
        let mut table = Table::new();
        for t in turns {
            let (files, churn) = match &t.touched {
                None => ("merge".to_string(), String::new()),
                Some(c) => (
                    format!("{} file{}", c.files, if c.files == 1 { "" } else { "s" }),
                    churn(c),
                ),
            };
            table = table.row(vec![
                // The spelling that gets the tree back, not a number `--keep`
                // would take.
                Cell::styled(format!("~{}", t.back), out::NAME),
                Cell::styled(t.age.map_or("?".into(), ago), out::DIM),
                // Shown, and constant for omh's own snapshots — which is what
                // makes anything the agent parked on this ref visible here.
                // `risks.md` 5b claims this view is the display case for that
                // hiding place, and without this column it was not.
                Cell::plain(out::untrusted(&t.subject)),
                Cell::styled(files, out::DIM),
                Cell::styled(churn, out::DIM),
            ]);
        }
        s.push_str(&table.render(p));
        s
    }
}

impl Report for Log {
    fn human(&self, p: &out::Palette) -> String {
        if let Some(turns) = &self.turns {
            return self.turns_human(p, turns);
        }
        let total = self.read.commits.len();
        let pending = self.pending();
        let mut head = format!(
            "{} · {total} checkpoint{}",
            self.id,
            if total == 1 { "" } else { "s" }
        );
        if pending > 0 {
            head.push_str(&format!(", {pending} not yours yet"));
        }
        // Three answers, three renderings. Silence for zero and silence for
        // *could not tell* would be the same rendering, which is the rule this
        // file states at the top: the empty string meaning both **clean** and
        // **omh could not tell** is the pair it is most dangerous to confuse.
        match self.behind {
            Some(0) => {}
            Some(behind) => head.push_str(&format!(" · {behind} behind {}", self.base)),
            None => head.push_str(&format!(" · how far behind {} is unknown", self.base)),
        }
        let mut s = out::heading(p, &head);
        s.push('\n');

        if self.read.commits.is_empty() {
            s.push_str(&out::nothing(
                p,
                "no checkpoints — the agent has not committed anything in this session",
            ));
        } else {
            // Right-aligned against the widest number, so the column reads as a
            // column of numbers rather than of text that happens to be digits.
            let width = total.to_string().len();
            let mut table = Table::new();
            // Newest first: the checkpoint you want is nearly always the one
            // that just happened. The *numbers* still count from the oldest —
            // they are what `diff` and `--keep` will take, and they have to
            // mean the same thing tomorrow.
            for c in self.read.commits.iter().rev() {
                let (files, churn) = match &c.touched {
                    // A merge is not measured, and *0 files* is a measurement.
                    None => ("merge".to_string(), String::new()),
                    Some(t) => (
                        format!("{} file{}", t.files, if t.files == 1 { "" } else { "s" }),
                        churn(t),
                    ),
                };
                table = table.row(vec![
                    Cell::styled(format!("{:>width$}", c.number), out::NAME),
                    // `?` rather than a guess. A date omh could not read must
                    // not borrow the confidence of *just now*.
                    Cell::styled(c.age.map_or("?".into(), ago), out::DIM),
                    // The agent's words, and the only untrusted value on the
                    // line — see `out::untrusted`.
                    Cell::plain(out::untrusted(&c.subject)),
                    Cell::styled(files, out::DIM),
                    Cell::styled(churn, out::DIM),
                ]);
            }
            let rendered = table.render(p);
            let mut lines: Vec<String> = rendered.lines().map(str::to_string).collect();
            if pending > 0 && pending < total && self.cleanly_split() {
                let widest = lines
                    .iter()
                    .map(|l| out::display_width(l))
                    .max()
                    .unwrap_or(0);
                let label = " yours from here ";
                // In characters throughout. `─` is three bytes, so sizing the
                // rule by `len()` and halving it with `split_at` lands inside
                // one and panics — which it did, on the first run of these
                // tests, rather than merely drawing a crooked line.
                let dashes = widest.saturating_sub(label.chars().count() + 2).max(4);
                let left = "─".repeat(dashes / 2);
                let right = "─".repeat(dashes - dashes / 2);
                lines.insert(
                    pending,
                    p.paint(out::DIM, &format!("  {left}{label}{right}")),
                );
            }
            s.push_str(&lines.join("\n"));
            s.push('\n');
        }

        // Always, including when it is zero: this is the work `--keep` sweeps
        // into a *Work in progress* commit, and the moment to see it is before
        // that happens rather than in the log afterwards.
        s.push('\n');
        s.push_str(&format!(
            "  {}\n",
            p.paint(
                out::DIM,
                &format!(
                    "uncommitted in the sandbox: {} file{}",
                    self.read.uncommitted,
                    if self.read.uncommitted == 1 { "" } else { "s" }
                )
            )
        ));
        s
    }

    fn json(&self) -> serde_json::Value {
        // The turns ride as their own key rather than replacing `checkpoints`,
        // so a script asking for one shape never silently gets the other.
        json!({
            "session": self.id,
            "base": self.base,
            "turns": self.turns.as_ref().map(|turns| {
                turns
                    .iter()
                    .map(|t| {
                        json!({
                            // `back`, not `number` — the two lists used to
                            // share a key name in one document, so a script
                            // could take a turn's number and hand it to
                            // `--keep`.
                            "back": t.back,
                            "ref": format!("{}~{}", crate::shadow::TURN_REF, t.back),
                            "subject": t.subject,
                            "age_seconds": t.age,
                            "files": t.touched.as_ref().map(|c| c.files),
                            "added": t.touched.as_ref().map(|c| c.added),
                            "removed": t.touched.as_ref().map(|c| c.removed),
                        })
                    })
                    .collect::<Vec<_>>()
            }),
            "behind": self.behind,
            "uncommitted": self.read.uncommitted,
            "unreachable": self.read.unreachable,
            "replay_point_lost": self.read.replay_point_lost,
            "pending": self.pending(),
            "checkpoints": self.read.commits.iter().rev().map(|c| json!({
                "number": c.number,
                "id": c.id,
                // Raw here: a program reading this is not a terminal, and a
                // subject with a replacement character in it is one it cannot
                // match against git's own output.
                "subject": c.subject,
                "age_seconds": c.age,
                "merge": c.touched.is_none(),
                "files": c.touched.as_ref().map(|t| t.files),
                "added": c.touched.as_ref().map(|t| t.added),
                "removed": c.touched.as_ref().map(|t| t.removed),
                "uncounted": c.touched.as_ref().map(|t| t.uncounted),
                "landed": c.landed,
            })).collect::<Vec<_>>(),
        })
    }

    fn asides(&self) -> out::Asides {
        // The snapshot view offers no *hints* — it has no numbers a command
        // takes, which is the point of keeping the lists apart. The warnings
        // below are a different thing: `unreachable` and a lost replay point
        // are facts about the session, and this file's own doc calls them
        // "states `harvest` refuses over" precisely so a user cannot read a
        // clean review and then be refused by `--keep` citing work they were
        // never shown. Suppressing them for anyone who habitually types
        // `--turns` was the same failure with a flag in front of it.
        let hints_are_meaningless_here = self.turns.is_some();
        let mut asides = out::Asides::default();
        if self.read.unreachable > 0 {
            asides = asides.warn(format!(
                "{} commit{} in this sandbox are on no branch it can reach, and are not \
                 listed above. `omh {} commit --keep` refuses until they are:\n  \
                 git --git-dir=<the sandbox repo> log --all --not HEAD",
                self.read.unreachable,
                if self.read.unreachable == 1 {
                    " "
                } else {
                    "s "
                },
                self.id
            ));
        }
        if self.read.replay_point_lost {
            asides = asides.warn(format!(
                "the last handover is no longer in this history — something rewound below \
                 it — so omh cannot tell which of these the branch already has. `omh {} \
                 commit --keep` refuses until that is resolved",
                self.id
            ));
        }
        if !self.cleanly_split() {
            let landed: Vec<String> = self
                .read
                .commits
                .iter()
                .filter(|c| c.landed)
                .map(|c| c.number.to_string())
                .collect();
            asides = asides.warn(format!(
                "no single line divides this list: {} already on the branch. Everything \
                 else is new",
                landed.join(", ")
            ));
        }
        // Written a step early once and caught by
        // `the_lines_omh_prints_are_lines_omh_accepts`: `diff` did not
        // take a number until the step after this one, and a hint is a promise
        // that the line can be selected and pasted. It arrived with the
        // argument it names.
        //
        // `--keep` is withheld for the same reason whenever omh already knows
        // it would be refused — those states are exactly the ones `harvest`
        // stops on.
        let mut offered: Vec<(String, String)> = Vec::new();
        if hints_are_meaningless_here {
            return asides;
        }
        if let Some(newest) = self.read.commits.last() {
            offered.push((
                format!("omh {} diff {}", self.id, newest.number),
                "read that one".into(),
            ));
        }
        if self.pending() > 0 && !self.incomplete() {
            offered.push((
                format!("omh {} commit --keep", self.id),
                format!(
                    "bring the {} new one{} onto the branch",
                    self.pending(),
                    if self.pending() == 1 { "" } else { "s" }
                ),
            ));
        }
        // Padded to the widest command rather than by hand. Two hints written
        // with counted spaces lined up until the session id changed width, and
        // the promise a hint makes is easier to believe from a column that is
        // actually a column.
        let widest = offered
            .iter()
            .map(|(cmd, _)| out::display_width(cmd))
            .max()
            .unwrap_or(0);
        offered.into_iter().fold(asides, |asides, (cmd, what)| {
            let pad = " ".repeat(widest - out::display_width(&cmd) + 4);
            asides.hint(format!("  {cmd}{pad}{what}"))
        })
    }
}

/// `+48 −12`, with each half dropped when it is zero.
///
/// A checkpoint that only adds lines reads `+48`, not `+48 −0`: the zero is
/// noise in a column scanned for size, and every added-only commit would carry
/// one.
///
/// `·N` is files git would not count lines for — never a blank, which is what
/// *changed nothing* looks like. A 200MB blob and a mode-bit change are not
/// the same event.
fn churn(t: &crate::shadow::Touched) -> String {
    let counted = match (t.added, t.removed) {
        (0, 0) => String::new(),
        (a, 0) => format!("+{a}"),
        (0, r) => format!("−{r}"),
        (a, r) => format!("+{a} −{r}"),
    };
    match (counted.is_empty(), t.uncounted) {
        (_, 0) => counted,
        (true, n) => format!("·{n}"),
        (false, n) => format!("{counted} ·{n}"),
    }
}

// ── omh s down ──────────────────────────────────────────────────────────────

/// What `omh s down` did to each session it was asked about.
///
/// A list rather than one `Action` per session, because with no id the command
/// is asked about *every* session — and saying each one separately emits a
/// JSON document per session, which is a parse error for the caller and
/// nothing at all for the exit code.
/// What happened to one session when `down` reached it.
///
/// Three outcomes rather than a `bool`, and the `bool` was the same collapse
/// this file's `running` column had: a session omh could not ask about was
/// warned on stderr and then left out of the report entirely, so `omh down`
/// printed **`no sessions`** — on stdout, the answer channel — over a daemon
/// it never reached, and `--json` returned `"sessions": []`. A missing row is
/// worse than a wrong one: a script iterating the list sees nothing at all.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Stopped {
    /// It was up, and is not now.
    Yes,
    /// It was already down.
    WasNotRunning,
    /// omh could not tell, so it did not try. Carries the runtime's reason.
    CouldNotTell(String),
}

#[derive(Debug, Clone)]
pub struct Down {
    pub sessions: Vec<(String, Stopped)>,
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
                match stopped {
                    Stopped::Yes => Cell::plain("stopped; worktree and branch survive"),
                    Stopped::WasNotRunning => Cell::styled("was not running", out::DIM),
                    Stopped::CouldNotTell(_) => {
                        Cell::styled("omh could not tell — left alone", out::WARN)
                    }
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
                // `null` rather than `false` for the one omh could not ask
                // about, with the reason beside it — the same three-valued
                // shape `omh s` gives `running`, for the same reason: a script
                // reading `stopped == false` over an unreachable runtime got a
                // fiction, and before that got no row at all.
                "stopped": match stopped {
                    Stopped::Yes => serde_json::Value::Bool(true),
                    Stopped::WasNotRunning => serde_json::Value::Bool(false),
                    Stopped::CouldNotTell(_) => serde_json::Value::Null,
                },
                "why": match stopped {
                    Stopped::CouldNotTell(why) => serde_json::Value::String(why.clone()),
                    _ => serde_json::Value::Null,
                },
            })).collect::<Vec<_>>(),
        })
    }
}

// ── omh s ────────────────────────────────────────────────────────────────

/// Where a session is in the cycle, as one answer.
///
/// Ordered most-actionable first, and deliberately one answer rather than a
/// tally: `omh s` is read at a glance, and a session with uncommitted work needs
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
    /// The column `omh s` has always printed. Wording preserved exactly:
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

/// One session, as `omh s` sees it.
#[derive(Debug, Clone)]
pub struct Session {
    pub id: String,
    pub label: String,
    /// Whether the sandbox is up. `None` means no runtime was found, so
    /// nobody asked; `Some(Running::Unknown)` means omh asked and the runtime
    /// would not answer.
    ///
    /// A bare `bool` until #63, which made a Docker daemon that is down render
    /// every live sandbox as `stopped` — in both formats, with nothing said.
    pub running: Option<crate::image::Running>,
    /// **`None` means nobody asked**, which is not `Some(Work::Clean)`.
    ///
    /// `omh info` is the wide view and does not spend a git subprocess per
    /// session on a column it never prints. It filled this with `Work::Clean`
    /// once — harmless only because `Inventory` happened not to read the
    /// field, and one line away from reporting every session in `omh info` as
    /// having nothing in it. `Work::Unknown` exists to keep *cannot tell*
    /// apart from *nothing to do*; `Option` keeps *did not ask* apart from
    /// both, and makes the mistake unspellable rather than merely absent.
    pub work: Option<Work>,
    /// Commits the base branch has that this session does not.
    /// **`None` means omh could not count**, which is not `Some(0)`.
    ///
    /// Not quite `work`'s `Option`, which keeps *nobody asked* apart from every
    /// answer and carries *cannot tell* one level in, as `Work::Unknown`. This
    /// column is always asked for, so it needs two states rather than three,
    /// and `None` is the failure — a base that does not resolve in this
    /// checkout, a branch deleted by hand, a worktree whose `.git` no longer
    /// leads anywhere.
    ///
    /// This used to say the table renders `None` and `Some(0)` alike "because
    /// a glance column has nowhere to put a question", which is how the defect
    /// came to be written down as the design. It has somewhere: `behind_cell`
    /// asks the question out loud. JSON has always said `null` rather than a
    /// number nobody took.
    pub behind: Option<usize>,
}

/// Files more than one session is changing.
///
/// The collision git will not mention until a merge, said while both sessions
/// are still open and either could be redirected. Two agents editing one file
/// in two sandboxes is the ordinary shape of parallel work — it is not an
/// error, and this does not say it is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Overlap {
    /// The sessions changing them, in the order `omh s` lists them.
    pub sessions: Vec<String>,
    pub paths: Vec<String>,
}

/// `s01 and s03`, `s01, s02 and s03` — a list as a person reads one.
fn spoken(names: &[String]) -> String {
    match names.split_last() {
        None => String::new(),
        Some((last, [])) => last.clone(),
        Some((last, rest)) => format!("{} and {last}", rest.join(", ")),
    }
}

/// Group the paths every pair-or-more of sessions share.
///
/// Grouped by *which* sessions rather than by path, so three files touched by
/// the same two sessions are one line rather than three. That is the sentence
/// a reader wants — "s01 and s03 both change these" — and it is also what
/// keeps the section short on a repo where two agents are working through the
/// same module.
///
/// Pure, and given the paths rather than reading git, so the grouping is a
/// table. What produces the paths is one `status --porcelain` per session that
/// `omh s` already runs for its uncommitted count and then throws away.
pub fn overlaps(changed: &[(String, Vec<String>)]) -> Vec<Overlap> {
    let mut who: std::collections::BTreeMap<&str, Vec<&str>> = Default::default();
    for (id, paths) in changed {
        for path in paths {
            let sessions = who.entry(path.as_str()).or_default();
            // A session that lists a path twice is not two sessions.
            if !sessions.contains(&id.as_str()) {
                sessions.push(id);
            }
        }
    }

    let mut grouped: std::collections::BTreeMap<Vec<&str>, Vec<&str>> = Default::default();
    for (path, sessions) in who {
        if sessions.len() > 1 {
            grouped.entry(sessions).or_default().push(path);
        }
    }
    grouped
        .into_iter()
        .map(|(sessions, paths)| Overlap {
            sessions: sessions.into_iter().map(str::to_string).collect(),
            paths: paths.into_iter().map(str::to_string).collect(),
        })
        .collect()
}

/// Whether the sandbox is up, as one cell — four answers, four renderings.
///
/// The column beside `behind`, with the same rule and the same history: a
/// `bool` rendered *stopped* for a container that was not running and for a
/// runtime that would not say, so a Docker daemon that is down showed every
/// live sandbox as stopped.
///
/// *Nobody asked* and *asked and could not tell* are kept apart too. They lead
/// different places — no runtime at all is a machine that cannot run sessions,
/// and a runtime that will not answer is one that usually can.
fn running_cell(running: &Option<crate::image::Running>) -> Cell {
    use crate::image::Running;
    match running {
        Some(Running::Yes) => Cell::styled("up", out::OK),
        Some(Running::No) => Cell::styled("stopped", out::DIM),
        Some(Running::Unknown(_)) => Cell::styled("up?", out::WARN),
        // An absence, because nobody asked — the treatment `work` gives the
        // same state, and for the same reason.
        //
        // This said `no runtime`: a statement about the machine that only one
        // of this field's two producers had established. `omh info` sets `None`
        // because asking costs a subprocess per session, on a machine that may
        // well have docker, and it was harmless only because that listing
        // never renders this column — one line from being false, which is
        // precisely what the `work` field's doc warns about. The machine-level
        // fact is now said once, by the caller that knows it, rather than
        // inferred here once per row.
        None => Cell::plain(""),
    }
}

/// How far a session trails its base, as one cell — three answers, three
/// renderings.
///
/// Shared by `omh s` and by the session list in `omh info`. Both were wrong
/// the same way — `Some(0) | None` rendered an empty cell, so *up to date* and
/// *omh could not count* were the same sight on the two surfaces where a user
/// picks which session to open.
///
/// They were never one thing that drifted, which an earlier draft of this
/// claimed: #35 wrote both copies in the same commit, and #46 changed both in
/// lockstep. The argument for sharing them is the ordinary one — two copies is
/// one more than can be checked — not a history of them coming apart.
///
/// `Log` keeps a third rendering of the same three answers, deliberately: it
/// builds a fragment of a heading rather than a `Cell`, and it was the copy
/// that was already right.
///
/// That pair is the one this file's own rule names as most dangerous to
/// confuse, and `log` carries a paragraph about it. A stale session that looks
/// current is how work gets done against code that moved.
///
/// `WARN` for the unanswered one, matching what the work column does with the
/// same uncertainty: it is not a worse number, it is the absence of one.
fn behind_cell(behind: Option<usize>, base: &str) -> Cell {
    match behind {
        Some(0) => Cell::plain(""),
        Some(n) => Cell::styled(format!("({n} behind {base})"), out::DIM),
        None => Cell::styled(format!("(how far behind {base}?)"), out::WARN),
    }
}

/// Every session in this checkout, and what earlier ones left behind.
#[derive(Debug, Clone)]
pub struct Sessions {
    pub sessions: Vec<Session>,
    pub base: String,
    /// Session ids with a container, a run directory or a sandbox repository
    /// but no worktree.
    pub leftovers: Vec<String>,
    /// Files more than one session is changing.
    pub overlaps: Vec<Overlap>,
    /// Sessions omh could not read, and so could not include above.
    ///
    /// Not the same as a session that collides with nobody, which is what an
    /// absence from `overlaps` otherwise means — and the whole section renders
    /// as nothing at all when it is empty, so a partial answer would be
    /// indistinguishable from a clean one.
    pub unreadable: Vec<String>,
    /// What the agent did and what it cost, plus its last check result, shown
    /// only when the view is scoped to one session. `None` for the wide
    /// listing, which does not read a transcript per row.
    pub focus: Option<Focus>,
    /// The `ssh` command that opens a shell in this session, when the view is
    /// scoped to one running session. `None` for the wide listing and for a
    /// stopped one: there is no shell to offer a session that is not up, and a
    /// dashboard of many is not the place to name one.
    pub shell: Option<String>,
}

/// The extra detail a scoped session view carries: what the agent did, and
/// whether the work passed this repo's checks.
#[derive(Debug, Clone)]
pub struct Focus {
    pub activity: Activity,
    pub check: Option<CheckState>,
}

/// What a session's transcript says the agent did.
#[derive(Debug, Clone)]
pub enum Activity {
    /// Read, and here is the summary.
    Read(crate::transcript::Summary),
    /// The harness declares no transcript, or none was written yet — not an
    /// empty session, a *not recorded* one.
    NotRecorded(String),
    /// A transcript omh opened and could make nothing of.
    Unreadable,
}

/// The last result of `omh sNN commit`'s checks, from `runs/<id>/check.json`.
#[derive(Debug, Clone)]
pub enum CheckState {
    Passed(usize),
    Failed(String),
    NotRun(String),
    /// `check.json` is there but omh could not read it — damaged, or written
    /// by a version whose shape this one does not know. Not the same as no
    /// check ever running, which shows no line at all.
    Unreadable,
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
                running_cell(&s.running),
                match &s.work {
                    Some(work) => Cell::styled(work.human(), work.style()),
                    None => Cell::plain(""),
                },
                behind_cell(s.behind, &self.base),
            ]);
        }
        let mut out = table.render(p);

        // Part of the answer, not an aside. `omh s > sessions.txt` is a
        // record of what is in flight, and two sessions editing one file is
        // the most consequential thing in it — a warning would put it on
        // stderr, where that file would not have it.
        for overlap in &self.overlaps {
            out.push_str(&format!(
                "\n  {}\n",
                p.paint(
                    // Not `DIM`. This is the line the section exists for, and
                    // the column beside it renders *omh could not tell* in
                    // `WARN` — de-emphasising the collision while highlighting
                    // the uncertainty says the wrong thing about which matters.
                    out::HEAD,
                    &format!(
                        "{} both change {}",
                        spoken(&overlap.sessions),
                        overlap.paths.join(", ")
                    )
                )
            ));
        }
        if let Some(focus) = &self.focus {
            out.push('\n');
            out.push_str(&focus_lines(p, focus));
        }

        // Said whether or not there are collisions above: with none, an
        // unreadable session is the only reason the section is empty, and that
        // is precisely when the silence would be read as "nothing collides".
        if !self.unreadable.is_empty() {
            out.push_str(&format!(
                "\n  {}\n",
                p.paint(
                    out::WARN,
                    &format!(
                        "omh could not read what {} {} changing, so anything above may be \
                         incomplete",
                        spoken(&self.unreadable),
                        match self.unreadable.len() {
                            1 => "is",
                            _ => "are",
                        }
                    )
                )
            ));
        }
        out
    }

    /// What to do next, and what omh could not answer — neither of which is what
    /// `omh s` was asked for.
    ///
    /// Both lines used to be appended to the table above, so
    /// `omh s > sessions.txt` wrote them into the file — the exact case
    /// `docs/commands.md` promises they stay out of. They are still in `json`
    /// as `leftovers`, where a script wanted them all along.
    fn asides(&self) -> out::Asides {
        let mut asides = out::Asides::default();

        // The number was in this table long before there was anything to do
        // about it. `sync` is that thing, and a dashboard that reports a
        // problem it can now name the answer to and does not is worse than one
        // that never had the answer.
        //
        // `Some(n)` with `n > 0` only. A zero has nothing to do and a `None`
        // is a question omh failed to answer — advising a merge on the
        // strength of a count that could not be taken is advice built on a
        // guess, and that row is already saying in the table that something is
        // wrong with reading it.
        // Not offered to a session omh could not ask about either: `sync`
        // refuses on *could not tell* whatever spelling it is given, so every
        // form of the suggestion is a line that fails when pasted. That row's
        // route is the runtime, and the warning below names it.
        let stale: Vec<&Session> = self
            .sessions
            .iter()
            .filter(|s| s.behind.is_some_and(|n| n > 0))
            .filter(|s| !matches!(s.running, Some(crate::image::Running::Unknown(_))))
            .collect();
        // The spelling that works for the session as it stands. `sync` refuses
        // on a running sandbox and names `--down` itself — so advising the bare
        // form for a row the table is printing `up` beside is advice that exits
        // non-zero when pasted, on the most common input this exists for: an
        // agent that has been running a while against trunk that moved.
        let offered: Vec<(String, &Session)> = stale
            .iter()
            .map(|s| {
                // `--down` for a sandbox that is up, because `sync` refuses on
                // a running session and the bare form would fail when pasted.
                //
                // A sandbox omh could not ask about gets neither spelling.
                // `--down` was offered at first, on the reasoning that it is
                // right either way — but `sync` refuses on *could not tell*
                // before it ever looks at `--down`, so that was a
                // paste-and-fail line in the one case the branch exists to
                // prevent. The route for that row is below, with the count it
                // could not take.
                let cmd = match s.running {
                    Some(crate::image::Running::Yes) => {
                        format!("omh {} sync --down", out::untrusted(&s.id))
                    }
                    _ => format!("omh {} sync", out::untrusted(&s.id)),
                };
                (cmd, *s)
            })
            .collect();
        // `display_width`, not `len`. The comment that used to sit here
        // justified the variable pad by ids that are not `sNN` — which is
        // exactly the case where counting bytes shears the column, and this
        // module says so where the function is defined.
        let widest = offered
            .iter()
            .map(|(cmd, _)| out::display_width(cmd))
            .max()
            .unwrap_or(0);
        for (cmd, s) in &offered {
            let pad = " ".repeat(widest - out::display_width(cmd) + 2);
            asides = asides.hint(format!(
                "  {cmd}{pad}bring {} in{}",
                self.base,
                match s.running {
                    Some(crate::image::Running::Yes) => ", stopping the sandbox first",
                    _ => ", merged on the host",
                }
            ));
        }

        // A row omh could not measure gets a route of its own rather than
        // silence. Withholding `sync` from it is right — a merge advised off a
        // count that failed is advice built on a guess — but silence next to
        // rows that each carry a next step reads as *this one is fine*, which
        // is the collapse this whole change is about, moved from the cell into
        // the advice.
        let unmeasured: Vec<&str> = self
            .sessions
            .iter()
            .filter(|s| s.behind.is_none())
            .map(|s| s.id.as_str())
            .collect();
        if let Some(first) = unmeasured.first() {
            asides = asides
                .warn(format!(
                    "omh could not measure {} against {} — {} may be working against code that \
                     moved, and `sync` is not offered over a count that failed",
                    spoken(&unmeasured.iter().map(|s| s.to_string()).collect::<Vec<_>>()),
                    self.base,
                    if unmeasured.len() == 1 { "it" } else { "they" }
                ))
                .hint(format!(
                    "  omh {} log   says why the count could not be taken",
                    out::untrusted(first)
                ));
        }

        // The shell into a focused, running session — the thing you reach for
        // right after `attach` closes, and which `attach` itself already
        // prints. A next action, so it stays out of a redirected listing.
        if let Some(shell) = &self.shell {
            asides = asides.hint(format!("  {shell}   open a shell in the session"));
        }

        if self.leftovers.is_empty() {
            return asides;
        }
        asides
            .warn(format!(
                "{} removed but left something behind: {}",
                if self.leftovers.len() == 1 {
                    "1 session was"
                } else {
                    "sessions were"
                },
                self.leftovers.join(", ")
            ))
            .hint("  clear each with  omh <id> rm")
    }

    fn json(&self) -> serde_json::Value {
        json!({
            "base": self.base,
            "sessions": self.sessions.iter().map(|s| json!({
                "id": s.id,
                "label": s.label,
                // Three values, not two: `true`, `false`, and `null` for a
                // question omh could not answer. A script reading `running ==
                // false` over an unreachable runtime got a fiction.
                "running": match &s.running {
                    Some(crate::image::Running::Yes) => serde_json::Value::Bool(true),
                    Some(crate::image::Running::No) => serde_json::Value::Bool(false),
                    Some(crate::image::Running::Unknown(_)) | None => serde_json::Value::Null,
                },
                // The reason, where the human table has a warning and a script
                // has nothing else — `--json` returns before asides, so this
                // field is the whole of what a caller gets. `null` here with
                // `running: null` above is *nobody asked*; a string is *asked
                // and the runtime would not say*, which the first version of
                // this rendered identically.
                "running_unknown": match &s.running {
                    Some(crate::image::Running::Unknown(why)) => {
                        serde_json::Value::String(why.clone())
                    }
                    _ => serde_json::Value::Null,
                },
                // `null` where nobody asked. A caller can tell that apart from
                // every answer, which is the whole reason for the `Option`.
                "work": s.work.as_ref().map(Work::json),
                "behind": s.behind,
            })).collect::<Vec<_>>(),
            "leftovers": self.leftovers,
            "overlaps": self.overlaps.iter().map(|o| json!({
                "sessions": o.sessions,
                "paths": o.paths,
            })).collect::<Vec<_>>(),
            "unreadable": self.unreadable,
            "shell": self.shell,
            "focus": self.focus.as_ref().map(focus_json),
        })
    }
}

/// The scoped session's activity and check state, as lines under the table.
fn focus_lines(p: &out::Palette, focus: &Focus) -> String {
    let mut s = String::new();
    match &focus.activity {
        Activity::Read(summary) => {
            let tools: usize = summary.tools.values().sum();
            let cost = match summary.cost() {
                Some(c) => format!("${c:.2}"),
                None => "cost unknown".to_string(),
            };
            s.push_str(&format!(
                "  {}\n",
                p.paint(
                    out::HEAD,
                    &format!(
                        "{} turn{}, {} tool call{}, {} file{} touched · {cost}",
                        summary.turns,
                        if summary.turns == 1 { "" } else { "s" },
                        tools,
                        if tools == 1 { "" } else { "s" },
                        summary.files.len(),
                        if summary.files.len() == 1 { "" } else { "s" },
                    )
                )
            ));
            // Some turns read, some lines not: the totals are a floor, and
            // saying so is the difference between an honest partial and a
            // confident wrong total.
            if summary.unreadable > 0 {
                s.push_str(&format!(
                    "  {}\n",
                    p.paint(
                        out::WARN,
                        &format!(
                            "{} transcript line{} could not be read — the totals above are a floor",
                            summary.unreadable,
                            if summary.unreadable == 1 { "" } else { "s" }
                        )
                    )
                ));
            }
        }
        Activity::NotRecorded(why) => {
            s.push_str(&format!(
                "  {}\n",
                p.paint(out::DIM, &format!("activity not recorded — {why}"))
            ));
        }
        Activity::Unreadable => {
            s.push_str(&format!(
                "  {}\n",
                p.paint(out::WARN, "omh could not read this session's transcript")
            ));
        }
    }
    if let Some(check) = &focus.check {
        let (style, text) = match check {
            CheckState::Passed(n) => (out::NAME, format!("checks: passed ({n})")),
            CheckState::Failed(name) => (
                out::WARN,
                format!("checks: failed — {}", out::untrusted(name)),
            ),
            CheckState::NotRun(why) => (out::DIM, format!("checks: not run — {why}")),
            CheckState::Unreadable => (
                out::WARN,
                "checks: a result was recorded but omh could not read it".to_string(),
            ),
        };
        s.push_str(&format!("  {}\n", p.paint(style, &text)));
    }
    s
}

/// The scoped focus, as JSON.
fn focus_json(focus: &Focus) -> serde_json::Value {
    let activity = match &focus.activity {
        Activity::Read(summary) => json!({
            "state": "read",
            "turns": summary.turns,
            "tools": summary.tools,
            "files": summary.files.iter().collect::<Vec<_>>(),
            "cost": summary.cost(),
            "unreadable": summary.unreadable,
            "usage": summary.usage.iter().map(|(m, u)| (m.clone(), json!({
                "input": u.input, "output": u.output,
                "cache_read": u.cache_read, "cache_write": u.cache_write,
                "cost": u.cost,
            }))).collect::<serde_json::Map<_, _>>(),
        }),
        Activity::NotRecorded(why) => json!({ "state": "not-recorded", "why": why }),
        Activity::Unreadable => json!({ "state": "unreadable" }),
    };
    let check = focus.check.as_ref().map(|c| match c {
        CheckState::Passed(n) => json!({ "state": "passed", "count": n }),
        CheckState::Failed(name) => json!({ "state": "failed", "check": name }),
        CheckState::NotRun(why) => json!({ "state": "not-run", "why": why }),
        CheckState::Unreadable => json!({ "state": "unreadable" }),
    });
    json!({ "activity": activity, "check": check })
}

// ── omh info ────────────────────────────────────────────────────────────────

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

/// What `omh info` found: harnesses, editors, sessions, your catalogue.
#[derive(Debug, Clone)]
pub struct Inventory {
    pub harnesses: Vec<Harness>,
    /// Where a harness would be added, for the message when there are none.
    pub adapters_dir: String,
    pub editors: Vec<Editor>,
    pub sessions: Vec<Session>,
    pub base: String,
    /// Where your catalogue lives, so the listing below can be found on disk.
    pub catalogue_dir: String,
    /// What your catalogue holds, per capability.
    ///
    /// It lived under the command 0.7.0 deleted. `omh info` means
    /// *what you have here*, and a catalogue is exactly that — dropping it
    /// with the command would have lost the only listing of it.
    pub catalogue: Vec<Catalogue>,
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
                    behind_cell(sess.behind, &self.base),
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
            // A capability you have nothing in is nothing to report. On a fresh
            // install four of the six rows read `0` and taught the reader only
            // that omh has six capabilities, which is not what `omh info` was
            // asked. Same cut `init` makes, for the same reason.
            if c.entries.is_empty() {
                continue;
            }
            // The count as well as the names: a catalogue is a thing that
            // grows, and this is the number the unselected report talks about.
            t = t.row(vec![
                Cell::plain(&c.capability),
                Cell::styled(c.entries.len().to_string(), out::DIM),
                Cell::plain(c.entries.join(", ")),
            ]);
        }
        if self.catalogue.iter().all(|c| c.entries.is_empty()) {
            s.push_str(&out::nothing(p, "nothing in it yet"));
        }
        s.push_str(&t.render(p));
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
            "catalogue_dir": self.catalogue_dir,
            "catalogue": self.catalogue.iter().map(|c| json!({
                "capability": c.capability,
                "count": c.entries.len(),
                "entries": c.entries,
            })).collect::<Vec<_>>(),
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
/// What a doctor run was *about*, when it got as far as being about anything.
#[derive(Debug, Clone)]
pub struct DoctorSandbox {
    pub harness: String,
    pub tag: String,
}

#[derive(Debug, Clone)]
pub struct Doctor {
    /// The harness whose adapter paths were verified, and the image it ran in
    /// — **`None` when nothing ran in a sandbox at all**.
    ///
    /// These were `String`, and a host-only report filled them with prose:
    /// `harness: "the host"`, `tag: "no image — the runtime is missing"`. Two
    /// costs. The human renderer keyed its wording off `harness == "the host"`,
    /// a literal that had to stay in step across three files with nothing
    /// pinning it — rename one and the report silently claims adapter paths
    /// were verified again. And `--json` shipped an English sentence in
    /// `"image"`, the field every consumer reads as a tag, which is the
    /// parse-English failure this module's header argues against at length.
    pub sandbox: Option<DoctorSandbox>,
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
                        // A host-only report has verified no adapter path — it
                        // never reached a sandbox. Saying so anyway is the kind
                        // of claim `doctor` exists to stop making, and it is a
                        // `match` on a state rather than a string compare so a
                        // reworded literal cannot quietly restore it.
                        "all {} checks passed — {}",
                        self.outcomes.len(),
                        match &self.sandbox {
                            Some(s) => format!("{}'s adapter paths are verified", s.harness),
                            None => "the host answered; nothing was run in a sandbox".into(),
                        }
                    )
                )
            ));
        }
        s
    }

    fn json(&self) -> serde_json::Value {
        json!({
            // `null`, not a sentence. A consumer can test for absence; it
            // cannot parse "no image — the runtime is missing" out of a field
            // documented as a tag.
            "harness": self.sandbox.as_ref().map(|s| s.harness.clone()),
            "image": self.sandbox.as_ref().map(|s| s.tag.clone()),
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

// ── omh settings ────────────────────────────────────────────────────────────

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

/// One key omh reads, and what you have said about it.
#[derive(Debug, Clone)]
pub struct Known {
    pub key: String,
    /// What omh reads it for, from the registry.
    pub does: String,
    /// Your default, if you have one.
    pub value: Option<String>,
}

/// What `omh settings` shows: your defaults, against everything omh reads.
///
/// The unset keys are the point. The registry is a table in the binary, so a
/// settings file cannot show it, and until this command existed the only way
/// to learn a key's name was to already know it — or to guess at `omh why`.
#[derive(Debug, Clone)]
pub struct Settings {
    pub file: String,
    pub known: Vec<Known>,
    /// Tables in your file that `omh init` copies into a new repo — `[use]`
    /// and `[omh]`. Shown separately because they are neither a default you
    /// set nor something read by nothing, and reporting them as the latter was
    /// exactly backwards.
    pub tables: Vec<String>,
    /// Keys in your file that omh reads nothing from. Named rather than
    /// hidden: a typo looks exactly like a setting that took.
    pub unread: Vec<Setting>,
}

impl Report for Settings {
    fn human(&self, p: &out::Palette) -> String {
        let (set, unset): (Vec<&Known>, Vec<&Known>) =
            self.known.iter().partition(|k| k.value.is_some());

        let mut s = format!(
            "{} {}\n",
            p.paint(out::HEAD, "your defaults"),
            p.paint(out::DIM, &self.file)
        );
        if set.is_empty() {
            s.push_str(&out::nothing(p, "nothing set — omh's own defaults apply"));
        } else {
            let mut t = Table::new();
            for k in set {
                t = t.row(vec![
                    Cell::styled(&k.key, out::NAME),
                    Cell::plain(k.value.as_deref().unwrap_or_default()),
                ]);
            }
            s.push_str(&t.render(p));
        }

        if !unset.is_empty() {
            s.push('\n');
            s.push_str(&format!("{}\n", p.paint(out::HEAD, "omh also reads")));
            let mut t = Table::new();
            for k in unset {
                t = t.row(vec![
                    Cell::styled(&k.key, out::NAME),
                    Cell::styled(&k.does, out::DIM),
                ]);
            }
            s.push_str(&t.render(p));
        }

        if !self.tables.is_empty() {
            s.push('\n');
            s.push_str(&format!(
                "{}\n",
                p.paint(out::HEAD, "also seeded into a new repo")
            ));
            let mut t = Table::new();
            for name in &self.tables {
                t = t.row(vec![Cell::styled(name, out::NAME)]);
            }
            s.push_str(&t.render(p));
        }

        if !self.unread.is_empty() {
            s.push('\n');
            s.push_str(&format!(
                "{}\n",
                p.paint(out::HEAD, "set here, and read by nothing")
            ));
            let mut t = Table::new();
            for u in &self.unread {
                t = t.row(vec![Cell::styled(&u.key, out::NAME), Cell::plain(&u.value)]);
            }
            s.push_str(&t.render(p));
        }
        s
    }

    fn json(&self) -> serde_json::Value {
        json!({
            "file": self.file,
            "known": self.known.iter().map(|k| json!({
                "key": k.key,
                "does": k.does,
                "value": k.value,
            })).collect::<Vec<_>>(),
            "tables": self.tables,
            "unread": self.unread.iter().map(|u| json!({
                "key": u.key,
                "value": u.value,
            })).collect::<Vec<_>>(),
        })
    }
}

/// What `omh settings mcp` shows: every server, and which layer decided it.
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

// ── omh info --repo ─────────────────────────────────────────────────────────

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
    /// Empty whenever `selected` is `None` — a capability following the whole
    /// catalogue has nothing it left behind, and `Selection::unselected` says
    /// so. `Init`'s decision about which rows to print depends on it.
    pub unselected: Vec<String>,
    /// What a feature supplies here, which `[use]` never names.
    ///
    /// A feature owns its server, so `codegraph` and `memory` are excluded from
    /// the selection — and the row read `mcp  nothing` in a repo whose features
    /// block said `codegraph on` and `memory on` two lines above it, and whose
    /// `omh info` listed both in the catalogue. Three true statements, one of
    /// which was read as *there is no MCP here*.
    pub from_a_feature: Vec<String>,
}

impl Using {
    pub fn summary(&self) -> String {
        // What a feature brings is named beside what you chose, because it is
        // here either way and `[use]` never mentions it. Without this the row
        // read `mcp  nothing` in a repo whose features block said `codegraph
        // on` and `memory on` two lines above — three true statements, one of
        // which a reader takes as *there is no MCP here*.
        // A count, not the names. Naming them fixed `mcp  nothing` in a repo
        // running two servers, and cost the width of six hook names in the
        // `hooks` row — which every other row then pads out to, so
        // `skills  review-diff` was followed by a hundred spaces. The fact
        // worth carrying is *omh brings some here*; which ones is
        // `omh why <feature>`.
        let theirs = match self.from_a_feature.len() {
            0 => String::new(),
            n => format!("+{n} from omh's features"),
        };
        let yours = match &self.selected {
            None => "everything".to_string(),
            Some(taken) if taken.is_empty() => String::new(),
            Some(taken) => taken.join(", "),
        };
        match (yours.is_empty(), theirs.is_empty()) {
            (true, true) => "nothing".into(),
            (true, false) => theirs,
            (false, true) => yours,
            (false, false) => format!("{yours} · {theirs}"),
        }
    }
}

/// What is effective in this checkout, and which file decided it.
#[derive(Debug, Clone)]
pub struct Repo {
    pub dir: String,
    /// What omh keys this checkout's state by — the name in
    /// `~/.omh/worktrees/<id>`, in the cache volume, and in every container
    /// this checkout starts.
    ///
    /// Reported because risk 8d makes it a question people have: two projects
    /// called `api` are two ids now, and the way to see which container
    /// belongs to which checkout is to be told. It is also the only supported
    /// way to ask, which the CLI tests rely on rather than spelling the rule
    /// out a second time and watching the copy go stale.
    pub repo_id: String,
    pub settings: Vec<Effective>,
    pub features: Vec<Feature>,
    pub using: Vec<Using>,
    /// Advisory lines the selection wants to add.
    pub notices: Vec<String>,
}

impl Report for Repo {
    fn human(&self, p: &out::Palette) -> String {
        let mut s = format!(
            "{} {}\n{}\n",
            p.paint(out::HEAD, "this repo"),
            p.paint(out::DIM, &self.dir),
            // What omh keys this checkout's worktrees, sessions and containers
            // by. Printed because two checkouts of the same name are two ids
            // now, and `docker ps` is otherwise a list of names with no way
            // back to the project each belongs to.
            p.paint(out::DIM, &format!("  keyed as {}", self.repo_id))
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
                    // *More in your catalogue*, not *declined*. This is the
                    // unfiltered catalogue, so in a rust repo it counts
                    // `go-test` and `python-format` — hooks naming ecosystems
                    // this repo is not, which `omh use` refuses outright.
                    // Calling those "not selected" claims a decision where
                    // there was never a choice. `init` was corrected for the
                    // same fact and reads the same way.
                    // The count, not the names. Naming what a feature
                    // supplies made the middle column as wide as its longest
                    // row — `hooks` carries six of omh's own — and this column
                    // aligns to that, so `skills  review-diff` was followed by
                    // a hundred spaces before its parenthetical. `init` says
                    // this the same way, and what is applicable and unselected
                    // is named on its own line below.
                    Cell::styled(
                        format!("({} more in your catalogue)", u.unselected.len()),
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
            "repo_id": self.repo_id,
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
    /// Withheld, so the sentence is in the tense it happened in. The write was
    /// skipped under `--dry-run` and this line was not, which left the file
    /// untouched and the output reading `wrote →`.
    pub dry_run: bool,
    /// `(capability, how many entries)`.
    pub counts: Vec<(String, usize)>,
}

impl Report for Resynced {
    fn human(&self, p: &out::Palette) -> String {
        let mut s = String::new();
        for path in &self.wrote {
            s.push_str(&match self.dry_run {
                true => format!("would resync to your catalogue — would write → {path}\n"),
                false => format!("resynced to your catalogue — wrote → {path}\n"),
            });
        }
        let mut t = Table::new();
        for (capability, count) in self.counts.iter().filter(|(_, n)| *n > 0) {
            // Same cut as `omh info`'s catalogue block: a capability with
            // nothing in it is not what this command was asked about, and on a
            // fresh install it was four rows of `0` out of six.
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
/// `omh settings mcp import` said `already identical` where `omh import skills`
/// said
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

/// What the sandbox said about this repo's hooks — or why it was never asked.
///
/// One field rather than a `Vec` beside an `Option<String>`, because an empty
/// list and an unasked question render identically and only one of them is
/// good news: the measurement sits two `if let`s deep in `init` — a harness,
/// and a provisioning probe that answered in full — so a repo whose sandbox
/// could not be reached reported nothing held back while the launcher went on
/// holding hooks back.
///
/// **The default is `Unchecked`, and that is the point.** As two fields the
/// derived `Default` was `None` beside an empty `Vec`, which the renderer read
/// as *asked, and all clear* — so a branch that forgot to say anything got the
/// reassuring answer for free. That is the shape of the defect this replaced,
/// one level up.
///
/// Two fields could also say *not measured* and *here is what was measured* at
/// once. This cannot.
#[derive(Debug, Clone)]
pub enum Hooks {
    /// The sandbox answered. `(hook, program it needs)` — written and
    /// travelling, but not running here. Empty is a clean bill of health.
    Measured(Vec<(String, String)>),
    /// The question was never put, and why — written by the branch that
    /// skipped it, so it cannot describe a different one. It carried a single
    /// string set where it was declared, and told a repo with a harness, an
    /// image and a failed probe `not measured — no harness`.
    Unchecked(String),
}

impl Default for Hooks {
    fn default() -> Self {
        Self::Unchecked("the sandbox was not asked".to_string())
    }
}

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
    pub harness: Option<String>,
    pub harness_on_host: bool,
    /// The base image, and the repo-specific one if a stack layer was built.
    pub image: Option<String>,
    pub stack_image: Option<String>,
    /// `(name, marker)` per detected stack.
    pub stacks: Vec<(String, String)>,
    pub provisioned: Vec<String>,
    /// What went wrong without failing `init`.
    ///
    /// Provisioning's alone once, and named for it. The report's own reads —
    /// the catalogue, the selection — joined it when they stopped propagating:
    /// `init` reaches them having already built two images and written the
    /// repo, and turning that into exit 1 and one line of permission error
    /// reported none of the work it had actually done.
    pub problems: Vec<String>,
    pub hooks: Hooks,
    pub importable: Vec<String>,
    pub memory: String,
    pub catalogue_dir: String,
    pub repo_dir: String,
    pub graph: Option<String>,
    pub base_set: String,
    pub rationale: Vec<(String, String)>,
    /// What this repo takes from your catalogue, per capability.
    ///
    /// The same value `omh info --repo` reports, built the same way, because
    /// the question *what did init select here* and the question *what is
    /// selected here* have one answer and two commands that must not be able
    /// to disagree about it.
    ///
    /// **Selected, never installed.** Every capability but hooks is composed
    /// into each session from the catalogue rather than copied in — which is
    /// what lets a fix reach a repo that ran `init` a year ago — and MCP lives
    /// in `~/.omh/mcp.json`, which `init` writes to your *catalogue*, not
    /// here. Hooks are the one capability `init` writes into **this repo**,
    /// and only the ones it derives. Saying *installed* would describe work
    /// that does not happen.
    pub using: Vec<Using>,
    /// The advisory lines the selection wants to add, the same ones
    /// `omh info --repo` prints.
    ///
    /// Without them the two reports disagreed at exactly the place the rows
    /// above claim they cannot: a `[use]` entry naming something nothing
    /// answers to is filtered out of *both* `selected` and `unselected`, so
    /// when it was a capability's only entry the row vanished from `init` —
    /// while `omh info --repo`, one command later, warned about it by name.
    /// The derivation was shared; the reporting was not.
    pub notices: Vec<String>,
    /// `(run, does)`, in the order somebody does them — the names `json`
    /// publishes, so the pair has one set of names rather than three.
    pub next: Vec<(String, String)>,
    /// Settings copied out of `~/.omh/default.toml` into this repo's file.
    ///
    /// Reported because it is the one moment that template has any effect —
    /// nothing reads it at launch. A seed nobody is told about is
    /// indistinguishable from a default, and this repo now carries the values
    /// rather than inheriting them, which is a fact about a committed file.
    pub seeded: Vec<String>,
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

        // **No inventory.** `harnesses 3 (claude, omp, opencode)` and
        // `editors 4 (…)` opened this report and answered a question nobody
        // asks after `init`: they are facts about the machine, true before the
        // command ran and unchanged by it. `omh info` is where the machine is
        // described. What belongs here is what happened to *this repo*.
        let mut t = Table::new();
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
        if !self.seeded.is_empty() {
            t = t.row(vec![
                Cell::plain("settings"),
                Cell::plain(format!(
                    "{} seeded from your defaults ({})",
                    self.seeded.len(),
                    self.seeded.join(", ")
                )),
            ]);
        }
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
        // Selected, and what was left in the catalogue — the second half is
        // the one that is invisible otherwise, and it is the reason somebody
        // later asks why a skill they have is not running here.
        for using in &self.using {
            // A capability you have nothing in has nothing to report. On a
            // fresh install that is five rows of `0 selected` between the
            // stack and the provisioning, and a reader learns from them only
            // that omh has six capabilities — which is `omh info`'s answer,
            // not this one's.
            let mut said = match (&using.selected, using.unselected.is_empty()) {
                (Some(taken), true) if taken.is_empty() => continue,
                (None, _) => "everything in your catalogue".to_string(),
                (Some(taken), _) => format!("{} selected", taken.len()),
            };
            if !using.unselected.is_empty() {
                // *More in your catalogue*, not *declined*. This count is the
                // unfiltered catalogue — the same list `omh info --repo`
                // shows — so in a repo with no stack detected it counts
                // `go-test` and `python-format`, which this repo could never
                // have taken. Calling those "not here" told a first-time
                // reader they had turned down six things they never saw. What
                // narrows to the applicable set is `notices`, below, which is
                // why both are on this page.
                said.push_str(&format!(
                    "  ({} more in your catalogue)",
                    using.unselected.len()
                ));
            }
            t = t.row(vec![Cell::plain(&using.capability), Cell::plain(said)]);
        }
        for key in &self.provisioned {
            t = t.row(vec![Cell::plain("provision"), Cell::plain(key)]);
        }
        // `problem`, not `provision`. The list was provisioning's alone and
        // is now where every non-fatal failure `init` hits goes — a catalogue
        // it could not read is not a provisioning problem, and labelling it
        // one sends the reader to look at their stacks.
        for problem in &self.problems {
            t = t.row(vec![
                Cell::plain("problem"),
                Cell::styled(problem, out::WARN),
            ]);
        }
        // Named, with the evidence, because the alternative is the failure the
        // whole design replaces: a hook that runs on turn one and reports
        // `cargo: not found`, saying nothing about who decided to run cargo.
        match &self.hooks {
            Hooks::Measured(held) => {
                for (name, wanted) in held {
                    t = t.row(vec![
                        Cell::styled("held back", out::WARN),
                        Cell::plain(format!("`{name}` needs {wanted}")),
                    ]);
                }
            }
            Hooks::Unchecked(why) => {
                t = t.row(vec![
                    Cell::styled("held back", out::WARN),
                    Cell::plain(format!("not measured — {why}")),
                ]);
            }
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
        // Three lines rather than one, because the first thing after `init`
        // is not the only thing: the two that follow it are how you get back
        // to the session you are about to start, and a person who never sees
        // them starts a second one instead.
        for line in &self.notices {
            s.push_str(&format!("\n{line}\n"));
        }

        s.push_str(&format!("\n  {}\n", p.paint(out::HEAD, "next")));
        let mut next = Table::new().indent(4);
        for (line, does) in &self.next {
            next = next.row(vec![Cell::styled(line, out::NAME), Cell::plain(does)]);
        }
        s.push_str(&next.render(p));
        s
    }

    fn json(&self) -> serde_json::Value {
        json!({
            "asked": self.asked,
            "harness": self.harness,
            "harness_on_host": self.harness_on_host,
            "image": self.image,
            "stack_image": self.stack_image,
            "stacks": self.stacks.iter().map(|(name, marker)| json!({
                "name": name,
                "marker": marker,
            })).collect::<Vec<_>>(),
            "provisioned": self.provisioned,
            "problems": self.problems,
            // Both keys kept, and the enum matched to fill them: a script
            // reading `held_back == []` has to be able to tell the clean bill
            // from the unasked question, which is the same distinction the
            // human rows make.
            "held_back": match &self.hooks {
                Hooks::Measured(held) => held.iter().map(|(name, wanted)| json!({
                    "hook": name,
                    "needs": wanted,
                })).collect::<Vec<_>>(),
                Hooks::Unchecked(_) => Vec::new(),
            },
            "hooks_unchecked": match &self.hooks {
                Hooks::Measured(_) => serde_json::Value::Null,
                Hooks::Unchecked(why) => json!(why),
            },
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
            "using": self.using.iter().map(|u| json!({
                "capability": u.capability,
                "selected": u.selected,
                "unselected": u.unselected,
            })).collect::<Vec<_>>(),
            "notices": self.notices,
            "next": self.next.iter().map(|(line, does)| json!({
                "run": line,
                "does": does,
            })).collect::<Vec<_>>(),
            "seeded": self.seeded,
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
/// **The plan, not the command line.** This printed 55 lines of `docker run`
/// argv, one token per line, and nothing else — no image, no summary, nothing
/// a person could read. The argument was that the argv *is* the product,
/// pasteable behind a docker you are debugging, and that reader is real; they
/// are just not the reader `--dry-run` exists for. Somebody deciding whether to
/// let this tool near their repository asks *what will it do*, and got bind
/// mounts.
///
/// So the argv keeps its home in `--json`, whole and in order, which is where
/// a script was reading it from anyway — and the human form answers the
/// question.
#[derive(Debug, Clone)]
pub struct DryRun {
    pub status: String,
    pub worktree: String,
    pub image: String,
    pub network: String,
    /// `(capability, what it contributes)` — what the agent will be given.
    pub reads: Vec<(String, String)>,
    /// What the session can change, which is the short list that matters.
    pub writes: Vec<String>,
    /// The program and its arguments, program first.
    pub argv: Vec<String>,
}

impl Report for DryRun {
    fn human(&self, p: &out::Palette) -> String {
        let mut s = format!("{}\n", p.paint(out::HEAD, &self.status));
        let mut t = Table::new();
        t = t.row(vec![Cell::plain("image"), Cell::plain(&self.image)]);
        t = t.row(vec![Cell::plain("network"), Cell::plain(&self.network)]);
        t = t.row(vec![Cell::plain("worktree"), Cell::plain(&self.worktree)]);
        s.push_str(&t.render(p));

        if !self.reads.is_empty() {
            s.push_str(&format!(
                "\n  {}\n",
                p.paint(out::HEAD, "the agent is given")
            ));
            let mut r = Table::new().indent(4);
            for (what, detail) in &self.reads {
                r = r.row(vec![Cell::styled(what, out::NAME), Cell::plain(detail)]);
            }
            s.push_str(&r.render(p));
        }

        // The short list, and the reason this report exists. Everything omh
        // mounts is read-only but these, so naming them is naming the whole of
        // what a session can reach.
        s.push_str(&format!("\n  {}\n", p.paint(out::HEAD, "it can write")));
        let mut w = Table::new().indent(4);
        for line in &self.writes {
            w = w.row(vec![Cell::plain(line)]);
        }
        s.push_str(&w.render(p));

        s.push_str(&format!(
            "\n  {}\n",
            p.paint(out::DIM, "the runtime command line is in --json")
        ));
        s
    }

    fn json(&self) -> serde_json::Value {
        json!({
            "status": self.status,
            "worktree": self.worktree,
            "image": self.image,
            "network": self.network,
            "reads": self.reads.iter().map(|(what, detail)| json!({
                "what": what,
                "detail": detail,
            })).collect::<Vec<_>>(),
            "writes": self.writes,
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

/// A session's work against its base, or one checkpoint against its parent.
///
/// **The field is named for what it holds.** An earlier version of this always
/// ran `--stat` and said so here, with the reason: calling the field `patch`
/// invited `omh s diff --json | jq -r .patch | git apply`, which failed on
/// every session that had changed anything. Then `-p` arrived and put a real
/// patch in `summary`, which is the same footgun with the labels swapped — a
/// script could not tell which of the two it had been given, and the comment
/// arguing against that state was still sitting above the code producing it.
///
/// So the key follows the content: `summary` holds a `--stat`, `patch` holds a
/// patch, and exactly one of them is present. `jq -r .patch | git apply` now
/// works, for the same reason it used to fail.
///
/// The human rendering is sanitised — see [`human`](Diff::human) — and the
/// JSON is not, which is the split this file makes everywhere for the reason
/// `Log` states.
#[derive(Debug, Clone)]
pub struct Diff {
    /// What to call it in a sentence: `omh/s01`, or `s01 checkpoint 4`.
    pub label: String,
    /// The session id alone, so a script keying on it gets an id rather than a
    /// phrase.
    pub session: String,
    pub checkpoint: Option<usize>,
    pub base: String,
    pub what: crate::session::What,
    /// The `--stat` or the patch, as `what` says.
    pub body: String,
}

impl Diff {
    /// Whether there is anything here to read.
    ///
    /// The paged path asks before handing the terminal to git: an empty patch
    /// pages to a blank screen, which reads exactly like a broken pager and
    /// exactly like the refusal a detached worktree used to skip. The sentence
    /// below is the useful answer, and it only exists on the unpaged path.
    pub fn changed(&self) -> bool {
        !self.body.trim().is_empty()
    }
}

impl Report for Diff {
    /// Sanitised on the way out, because all of this was written inside the
    /// sandbox.
    ///
    /// `git show` prints the whole commit header and body — **author name and
    /// email, subject, and message** — and quotes none of it. Paths it does
    /// quote, by `core.quotePath`'s default. Measured: an ESC in a checkpoint
    /// subject reached omh's own output through this method. That is the same
    /// finding `log` acted on, arriving by a second route.
    ///
    /// A `--stat` survives this intact — measured: git's graph is spaces, `|`,
    /// `+` and `-`, with no tab and no other control character, so the
    /// alignment is byte-for-byte the same. A **patch** would not survive it,
    /// and does not reach here: a patch for a person is always paged, which is
    /// git writing to the terminal exactly as running git yourself would, and
    /// the only unpaged patch is the one a program asked for.
    fn human(&self, p: &out::Palette) -> String {
        if !self.changed() {
            return format!(
                "{}\n",
                p.paint(
                    out::DIM,
                    &format!("no changes on {} (against {})", self.label, self.base)
                )
            );
        }
        out::untrusted(&self.body)
    }

    fn json(&self) -> serde_json::Value {
        let mut v = json!({
            "session": self.session,
            "checkpoint": self.checkpoint,
            "base": self.base,
            "changed": self.changed(),
        });
        // Raw, and under the key that says what it is. A program is not a
        // terminal, and a subject with a replacement character in it is one it
        // cannot match against git's own output.
        let key = match self.what {
            crate::session::What::Summary => "summary",
            crate::session::What::Patch => "patch",
        };
        v[key] = json!(self.body);
        v
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

// ── omh s attach ────────────────────────────────────────────────────────────

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
mod tests;

/// What `omh eject` wrote, and what it could not.
///
/// The point of the report is the last line rather than the list: a command
/// whose purpose is *you can leave* has to end by saying that the files are
/// yours and omh is no longer in the path. Listing what it wrote is how that
/// claim is checkable.
#[derive(Debug, Clone)]
pub struct Ejected {
    pub harness: String,
    pub to: String,
    pub wrote: Vec<EjectedFile>,
    /// Capabilities this harness has no binding for. Named, because a reader
    /// comparing the output to their omh setup will otherwise assume omh lost
    /// something — an absent key means the harness cannot do that thing.
    pub dropped: Vec<String>,
    /// Files that still name a path only omh's sandbox has. Reported rather
    /// than rewritten: omh cannot know where you want your notes, and a guess
    /// written into a file you are about to depend on is worse than being
    /// told to look.
    pub sandboxed: Vec<String>,
    /// Sources omh could not read, so the capability came out incomplete or
    /// not at all. Distinct from `dropped`, which is the harness having no
    /// binding — that is omh working; this is omh unable to look.
    pub unreadable: Vec<String>,
    pub dry_run: bool,
}

/// What eject put at a path.
///
/// A `usize` with `1` meaning "a single document" was the first shape, and it
/// produced wrong output on the day it shipped: `copy_selected` returns the
/// number of *selected* entries, so a directory holding exactly one rendered
/// a blank cell — visually identical to `CLAUDE.md`. The type made the two
/// states unspellable-apart and the renderer duly conflated them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Wrote {
    /// One rendered document.
    Document,
    /// A directory, and how many entries reached it. Never zero: eject does
    /// not report a directory it wrote nothing to, and `NonZeroUsize` keeps
    /// that a property of the type rather than an `if` at the call site.
    Entries(std::num::NonZeroUsize),
}

#[derive(Debug, Clone)]
pub struct EjectedFile {
    pub capability: String,
    pub at: String,
    pub wrote: Wrote,
}

impl Report for Ejected {
    fn human(&self, p: &out::Palette) -> String {
        let mut s = format!(
            "{} {}\n",
            p.paint(out::HEAD, &format!("eject {}", self.harness)),
            p.paint(out::DIM, &self.to)
        );
        s.push('\n');

        if self.wrote.is_empty() {
            s.push_str(&out::nothing(p, "nothing to write"));
            return s;
        }

        let mut t = Table::new();
        for f in &self.wrote {
            t = t.row(vec![
                Cell::styled(&f.capability, out::NAME),
                Cell::plain(&f.at),
                Cell::styled(
                    &match &f.wrote {
                        Wrote::Document => String::new(),
                        Wrote::Entries(n) => format!(
                            "{n} {}",
                            match n.get() {
                                1 => "entry",
                                _ => "entries",
                            }
                        ),
                    },
                    out::DIM,
                ),
            ]);
        }
        s.push_str(&t.render(p));

        if !self.unreadable.is_empty() {
            s.push('\n');
            s.push_str(&out::warning(
                p,
                &format!(
                    "omh could not read these, so what came out is not all of it:\n    {}",
                    self.unreadable.join("\n    ")
                ),
            ));
        }

        if !self.dropped.is_empty() {
            s.push('\n');
            s.push_str(&out::hint(
                p,
                &format!(
                    "  {} has no binding for {} — nothing was lost, that harness \
                     cannot read it",
                    self.harness,
                    self.dropped.join(", ")
                ),
            ));
        }

        if !self.sandboxed.is_empty() {
            s.push('\n');
            s.push_str(&out::warning(
                p,
                &format!(
                    "these still name paths only omh's sandbox has — `/omh`, `/work`, \
                     `$OMH_*` — so they need editing before a harness reads them \
                     outside one:\n    {}",
                    self.sandboxed.join("\n    ")
                ),
            ));
        }

        s.push('\n');
        s.push_str(&out::hint(
            p,
            match self.dry_run {
                true => "  --dry-run: rendered, nothing written",
                // The whole reason the command exists, said plainly.
                false => "  these are yours now — omh is not in the path",
            },
        ));
        s
    }

    fn json(&self) -> serde_json::Value {
        json!({
            "harness": self.harness,
            "to": self.to,
            "wrote": self.wrote.iter().map(|f| json!({
                "capability": f.capability,
                "at": f.at,
                "entries": match &f.wrote {
                    Wrote::Document => 1,
                    Wrote::Entries(n) => n.get(),
                },
            })).collect::<Vec<_>>(),
            "dropped": self.dropped,
            "sandboxed": self.sandboxed,
            "unreadable": self.unreadable,
            "dry_run": self.dry_run,
        })
    }
}
