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
        })
    }
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

/// What `omh info` found: harnesses, editors, sessions.
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
                    behind_cell(sess.behind, &self.base),
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
            "unread": self.unread.iter().map(|u| json!({
                "key": u.key,
                "value": u.value,
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
/// `omh config mcp import` said `already identical` where `omh import skills`
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

/// What omh has not built yet, printed on `init`'s last line.
///
/// A constant rather than a literal in the middle of `human` because it is a
/// claim about the whole product that nothing else re-checks, and it is
/// exactly the kind of line that gets typed once and left. It was: `recall`
/// shipped with the memory server and this sentence went on calling it undone.
const NOT_YET_DONE: &str = "not yet done: cost accounting.";

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
    /// Settings copied out of `~/.omh/settings.toml` into this repo's file.
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
        s.push_str(&format!("\n{}\n", p.paint(out::DIM, NOT_YET_DONE)));
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
    /// `init` does not call unfinished something the same screen installed.
    ///
    /// `omh init` prints the base set it just seeded — including *"memory —
    /// what a session learned outlives it"*, which is the MCP server answering
    /// `recall` and `remember` — and then printed *"not yet done: recall"* four
    /// lines below it. A new user's first screen contradicted itself, and the
    /// sentence was a literal buried in `human` that nothing read again.
    ///
    /// The tool names are the pair `doctor::memory_checks` expects the server
    /// to speak, so this fails if the sentence ever re-names a shipped one.
    #[test]
    fn init_does_not_call_unfinished_a_capability_that_ships() {
        for tool in ["recall", "remember"] {
            assert!(
                !super::NOT_YET_DONE.contains(tool),
                "`{}` ships — the memory server answers it and `omh doctor` \
                 checks that it does — but init still reports it as undone: {}",
                tool,
                super::NOT_YET_DONE
            );
        }
    }

    /// A sync names every file that needs a decision, and counts them in a
    /// sentence rather than in a template.
    ///
    /// The count is not decoration: a clean sync and a sync with one conflict
    /// are the same command with opposite next steps. And `1 file need
    /// resolving` shipped in the first draft of this — caught by printing the
    /// thing rather than by reading the code that builds it.
    #[test]
    fn a_sync_names_what_needs_deciding_and_says_so_in_english() {
        let synced = |conflicted: Vec<String>, moved: usize| super::Synced {
            id: "s01".into(),
            base: "main".into(),
            onto: "abc1234".into(),
            moved,
            conflicted,
            checkpoint: true,
            note: None,
        };
        let p = crate::out::Palette::plain();

        let clean = synced(vec![], 3).human(&p);
        assert!(
            clean.contains("3 commits from main"),
            "what arrived, and from where: {clean}"
        );
        assert!(
            clean.contains("nothing needs deciding"),
            "and that there is nothing to do: {clean}"
        );

        let one = synced(vec!["src/tap.rs".into()], 1).human(&p);
        assert!(
            one.contains("1 commit from main"),
            "one commit, not `1 commits`: {one}"
        );
        assert!(
            one.contains("1 file needs resolving"),
            "and one file needs it, rather than need it: {one}"
        );
        assert!(one.contains("src/tap.rs"), "named: {one}");

        let two = synced(vec!["a.rs".into(), "b.rs".into()], 2).human(&p);
        assert!(
            two.contains("2 files need resolving"),
            "and two of them need it: {two}"
        );
        assert!(
            two.contains("a.rs") && two.contains("b.rs"),
            "every one named — a count is not something you can act on: {two}"
        );

        // A sync that could not leave its note is still a sync that happened.
        // The user hears about it once, as a warning, and `--json` carries the
        // same fact as a field — a bare `eprint` reaches neither a script nor
        // a test, which is why this is on the report at all.
        let quiet = super::Synced {
            note: Some("Permission denied (os error 13)".into()),
            ..synced(vec![], 2)
        };
        assert!(
            quiet.human(&p).contains("2 commits from main"),
            "the sync is reported as the success it was: {}",
            quiet.human(&p)
        );
        let said = quiet.asides().warnings.join(" ");
        assert!(
            said.contains("Permission denied"),
            "with the reason, not just the fact: {said}"
        );
        assert!(
            said.contains("base moved to"),
            "and what the agent will find instead: {said}"
        );
        assert_eq!(quiet.json()["noted"], serde_json::json!(false));
        assert_eq!(synced(vec![], 2).json()["noted"], serde_json::json!(true));
    }

    /// `--turns` is its own view, and shares nothing with the numbered list.
    ///
    /// The separation is the whole design and it fails silently if it slips.
    /// `diff <n>` and `--keep 1,3-4` index `read.commits` by number, so a
    /// snapshot appended there becomes selectable and then replayable onto the
    /// user's branch — omh's own commit, replanted as the agent's work. And
    /// the divider is `lines.insert(pending, …)` over rendered rows, so an
    /// interleaved list labels rows as already on the branch that are not.
    ///
    /// Neither failure shows up as an error. Both look like a log that reads a
    /// little oddly.
    #[test]
    fn the_turn_view_never_borrows_the_numbers_that_land_work() {
        let snapshot = |back: usize| crate::shadow::Turn {
            back,
            subject: "turn end".into(),
            age: Some(60),
            touched: Some(crate::shadow::Touched {
                files: 2,
                added: 8,
                removed: 1,
                uncounted: 0,
            }),
        };
        let mut log = a_log();
        let plain = out::Palette::plain();
        let commits = log.read.commits.clone();

        log.turns = Some(vec![snapshot(0), snapshot(1)]);
        let printed = log.human(&plain);

        assert!(printed.contains("2 turns"), "the turn count: {printed}");
        // The identifier is the ref spelling, not a number — so there is no
        // number here for `--keep` to accept from the wrong list. `~0` is the
        // newest, and it is the first row.
        assert!(
            printed.contains("~0") && printed.contains("~1"),
            "each row is the spelling that gets that tree back: {printed}"
        );
        let rows: Vec<&str> = printed.lines().filter(|l| l.contains('~')).collect();
        assert!(
            rows.first().is_some_and(|r| r.contains("~0")),
            "newest first: {rows:?}"
        );
        for c in &commits {
            assert!(
                !printed.contains(&c.subject),
                "and not one of the agent's own subjects: {printed}"
            );
        }
        assert!(
            !printed.contains("yours from here"),
            "no divider, because nothing here is going anywhere: {printed}"
        );
        assert!(
            !printed.contains("not yours yet"),
            "and no pending count, which counts a different list: {printed}"
        );
        assert!(
            log.asides().hints.is_empty(),
            "nothing to offer: there are no numbers here a command takes: {:?}",
            log.asides()
        );
        // …but the warnings are about the session, not about which list is
        // being rendered. Suppressing them meant a user who habitually types
        // `--turns` never learned their replay point was lost.
        let mut lost = a_log();
        lost.read.replay_point_lost = true;
        lost.turns = Some(vec![snapshot(0)]);
        assert!(
            lost.asides()
                .warnings
                .iter()
                .any(|w| w.contains("the last handover is no longer")),
            "a session-level warning still reaches the turn view: {:?}",
            lost.asides()
        );

        // The two lists reach JSON under different keys, so a script asking
        // for one can never be handed the other.
        let doc = log.json();
        assert_eq!(doc["turns"].as_array().map(Vec::len), Some(2));
        // No `number` key on a turn — the two lists shared that name in one
        // document, so a script could read a turn's number and hand it to
        // `--keep`.
        assert!(
            doc["turns"][0]["number"].is_null(),
            "a turn carries no number: {doc}"
        );
        assert_eq!(
            doc["turns"][0]["ref"],
            serde_json::json!("refs/omh/turn~0"),
            "it carries the spelling that works instead: {doc}"
        );
        assert_eq!(
            doc["checkpoints"].as_array().map(Vec::len),
            Some(commits.len()),
            "and the agent's own list is untouched: {doc}"
        );

        // Without the flag nothing about turns appears at all.
        log.turns = None;
        assert_eq!(log.json()["turns"], serde_json::Value::Null);
        assert!(log.human(&plain).contains("not yours yet"));
    }

    /// Three sessions and two files are one sentence a person can read.
    ///
    /// Both separators are the identity with two sessions and one path, which
    /// is what the end-to-end test has — so `s01, s02 and s03` and
    /// `src/base.rs, src/render.rs`, the whole reason `spoken` exists, were
    /// asserted nowhere. This renders rather than groups.
    #[test]
    fn three_sessions_and_two_files_read_as_one_sentence() {
        let mut listing = sessions(vec![session("s01", Work::Uncommitted(1))]);
        listing.overlaps = vec![Overlap {
            sessions: vec!["s01".into(), "s02".into(), "s03".into()],
            paths: vec!["src/base.rs".into(), "src/render.rs".into()],
        }];

        let said = listing.human(&out::Palette::plain());
        assert!(
            said.contains("s01, s02 and s03 both change src/base.rs, src/render.rs"),
            "a list as a person reads one: {said}"
        );
    }

    /// A session omh could not read is said, because its absence from the
    /// section above means the opposite.
    #[test]
    fn a_session_omh_could_not_read_is_not_a_session_that_collides_with_nobody() {
        let mut listing = sessions(vec![session("s01", Work::Unknown)]);
        listing.unreadable = vec!["s02".into()];

        let said = listing.human(&out::Palette::plain());
        assert!(
            said.contains("could not read what s02 is changing"),
            "named, and in the singular: {said}"
        );
        assert!(
            said.contains("incomplete"),
            "and what that means for the lines above it: {said}"
        );
        assert_eq!(
            listing.json()["unreadable"],
            json!(["s02"]),
            "a script reading `overlaps: []` has to be able to tell a partial \
             answer from a clean one"
        );

        // …and nothing is said when there is nothing to say.
        let quiet = sessions(vec![session("s01", Work::Uncommitted(1))]);
        assert!(!quiet
            .human(&out::Palette::plain())
            .contains("could not read"));
    }

    /// Two sessions changing one file is the collision git will not mention
    /// until a merge.
    #[test]
    fn a_file_two_sessions_are_both_changing_is_named_with_both() {
        let changed = |pairs: &[(&str, &[&str])]| -> Vec<(String, Vec<String>)> {
            pairs
                .iter()
                .map(|(id, paths)| {
                    (
                        id.to_string(),
                        paths.iter().map(|p| p.to_string()).collect(),
                    )
                })
                .collect()
        };

        assert_eq!(
            overlaps(&changed(&[
                ("s01", &["src/render.rs", "src/base.rs", "only-mine.rs"]),
                ("s02", &["elsewhere.rs"]),
                ("s03", &["src/render.rs", "src/base.rs"]),
            ])),
            vec![Overlap {
                sessions: vec!["s01".into(), "s03".into()],
                paths: vec!["src/base.rs".into(), "src/render.rs".into()],
            }],
            "one line for the pair, not one per file — and nothing about the files \
             only one session has"
        );

        assert!(
            overlaps(&changed(&[("s01", &["a.rs"]), ("s02", &["b.rs"])])).is_empty(),
            "sessions working on different things collide with nobody"
        );
        assert!(
            overlaps(&changed(&[("s01", &["a.rs", "a.rs"])])).is_empty(),
            "and a session is never in collision with itself"
        );

        // Three sessions on one file, and a different pair on another: two
        // groups, each naming exactly who is in it.
        let three = overlaps(&changed(&[
            ("s01", &["shared.rs", "pair.rs"]),
            ("s02", &["shared.rs"]),
            ("s03", &["shared.rs", "pair.rs"]),
        ]));
        assert_eq!(three.len(), 2, "grouped by who, not by file: {three:?}");
        assert!(three
            .iter()
            .any(|o| o.sessions.len() == 3 && o.paths == ["shared.rs"]));
        assert!(three
            .iter()
            .any(|o| o.sessions == ["s01", "s03"] && o.paths == ["pair.rs"]));

        // One pair is one line whatever order the sessions arrive in, and the
        // order kept is `omh s`'s. The grouping key is the session list, so a
        // pair that varied would split into two lines about the same two
        // sessions.
        let reversed = overlaps(&changed(&[
            ("s03", &["x.rs", "y.rs"]),
            ("s01", &["y.rs", "x.rs"]),
        ]));
        assert_eq!(reversed.len(), 1, "one pair, one line: {reversed:?}");
        assert_eq!(reversed[0].sessions, ["s03", "s01"], "in listing order");
    }

    fn checkpoint(number: usize, subject: &str, landed: bool) -> crate::shadow::Checkpoint {
        crate::shadow::Checkpoint {
            number,
            id: format!("{number:0>7}c"),
            subject: subject.to_string(),
            age: Some(number as u64 * 600),
            touched: Some(crate::shadow::Touched {
                files: number,
                added: number * 10,
                removed: number,
                uncounted: 0,
            }),
            landed,
        }
    }

    fn a_log() -> Log {
        Log {
            turns: None,
            id: "s01".into(),
            read: crate::shadow::Checkpoints {
                commits: vec![
                    checkpoint(1, "Rename shadow to sandbox repo", true),
                    checkpoint(2, "Fix typo", true),
                    checkpoint(3, "Add the failing test first", false),
                    checkpoint(4, "Extract the tap guard", false),
                ],
                uncommitted: 2,
                ..Default::default()
            },
            behind: Some(2),
            base: "main".into(),
        }
    }

    /// The line is the whole point of the list: above it is work the branch has
    /// never seen, below it is work `--keep` has already handed over. Newest
    /// first, because the checkpoint you want to read is almost always the one
    /// that just happened.
    #[test]
    fn the_log_draws_the_line_where_the_next_harvest_starts() {
        let printed = a_log().human(&out::Palette::plain());
        let lines: Vec<&str> = printed.lines().collect();
        let at = |needle: &str| {
            lines
                .iter()
                .position(|l| l.contains(needle))
                .unwrap_or_else(|| panic!("no line for {needle}: {printed}"))
        };

        assert!(
            at("Extract the tap guard") < at("Add the failing test first"),
            "newest first: {printed}"
        );
        assert!(
            at("Add the failing test first") < at("yours from here"),
            "unharvested work is above the line: {printed}"
        );
        assert!(
            at("yours from here") < at("Fix typo"),
            "and what the branch already has is below it: {printed}"
        );
    }

    /// The count in the header is the one the user acts on, and it comes from
    /// the flags rather than from where the line was drawn.
    ///
    /// A history with a merge in it can put a landed commit above an unlanded
    /// one — `landed` means *ancestor of the replay point*, not *older*. One
    /// divider cannot express that, so the line becomes approximate. The count
    /// must not.
    #[test]
    fn the_count_is_exact_even_where_one_line_cannot_say_it() {
        let mut log = a_log();
        // oldest → newest: landed, not, landed, not
        log.read.commits[1].landed = false;
        log.read.commits[2].landed = true;
        let printed = log.human(&out::Palette::plain());

        assert!(
            printed.contains("2 not yours yet"),
            "two are not the branch's, wherever the line falls: {printed}"
        );
        assert_eq!(log.json()["pending"], json!(2));
    }

    /// A session with nothing landed yet has no line to draw, and a divider
    /// over the whole list would say the opposite of what it means.
    #[test]
    fn a_log_with_nothing_handed_over_yet_has_no_line_to_draw() {
        let mut log = a_log();
        log.read.commits.iter_mut().for_each(|c| c.landed = false);
        let printed = log.human(&out::Palette::plain());
        assert!(
            !printed.contains("yours from here"),
            "nothing is the branch's yet, so there is no line: {printed}"
        );
        assert!(
            printed.contains("Fix typo"),
            "every checkpoint is still listed: {printed}"
        );
    }

    /// The subject is the agent's own words, arriving from a gitdir the agent
    /// writes. Printed raw, one `\x1b[2K` repaints the line omh just wrote about
    /// whether the user's work is safe.
    #[test]
    fn a_subject_the_agent_wrote_cannot_repaint_the_log() {
        let mut log = a_log();
        // Every control character, not only ESC: `\r` repaints a line just as
        // well, and `untrusted` maps the whole class rather than one member.
        log.read.commits[3].subject = "Fix \u{1b}[2K\rand \u{8}nothing at all".into();
        let printed = log.human(&out::Palette::plain());
        assert!(
            !printed.chars().any(|c| c.is_control() && c != '\n'),
            "no control character survives into omh's own output: {printed:?}"
        );
        assert!(
            printed.contains("nothing at all"),
            "the words still arrive: {printed}"
        );
    }

    /// A sandbox that has committed nothing says so, rather than printing a
    /// header over an empty table — the answer *is* "nothing yet", and a user
    /// who sees column titles reads it as a listing that failed.
    #[test]
    fn a_sandbox_that_has_committed_nothing_says_so() {
        let mut log = a_log();
        log.read.commits.clear();
        log.read.uncommitted = 3;
        let printed = log.human(&out::Palette::plain());
        assert!(printed.contains("no checkpoints"), "it says so: {printed}");
        assert!(
            printed.contains('3'),
            "and still reports the work that is there: {printed}"
        );
    }

    /// A next step is not the answer, so it goes where every other next step
    /// goes — `omh s01 log > review.txt` must not capture advice.
    #[test]
    fn what_to_type_next_is_an_aside_and_not_the_log() {
        let log = a_log();
        let printed = log.human(&out::Palette::plain());
        let hints = log.asides().hints.join("\n");

        assert!(
            hints.contains("omh s01 commit --keep"),
            "the harvest is offered: {hints}"
        );
        // …and the newest checkpoint, now that `diff` takes a number. That one
        // line is read out of the tree and parsed by
        // `the_lines_omh_prints_are_lines_omh_accepts`; the `--keep`
        // line above is not, because that scan skips anything ending in a flag
        // and says why. Hence this assertion, which covers what the scan
        // cannot.
        assert!(
            hints.contains("omh s01 diff 4"),
            "the newest checkpoint is offered by number: {hints}"
        );
        assert!(
            !printed.contains("--keep"),
            "but not in the answer: {printed}"
        );
    }

    /// A script reads numbers, not a table. The number is what `diff` and
    /// `--keep` take, so it is the field that has to be there.
    #[test]
    fn a_program_reading_the_log_gets_the_numbers_not_the_english() {
        let v = a_log().json();
        let checkpoints = v["checkpoints"].as_array().expect("a list");
        assert_eq!(checkpoints.len(), 4);
        assert_eq!(
            checkpoints[0]["number"],
            json!(4),
            "newest first, as printed"
        );
        assert_eq!(checkpoints[0]["landed"], json!(false));
        assert_eq!(checkpoints[3]["number"], json!(1));
        assert_eq!(checkpoints[3]["landed"], json!(true));
        assert_eq!(v["pending"], json!(2), "what --keep would take");
        assert_eq!(v["uncommitted"], json!(2));
        assert_eq!(v["behind"], json!(2));
    }

    /// `behind` has three answers and one of them is *omh could not tell*.
    ///
    /// The enum note at the top of this file is about exactly this. The first
    /// version of this test asserted that the word *behind* was absent when
    /// omh could not count — which `Some(0)` also satisfies, so it passed while
    /// the two answers rendered identically. The invariant is that they differ,
    /// and it has to be written as a comparison to say so.
    #[test]
    fn a_count_omh_could_not_take_does_not_print_as_zero() {
        let render = |behind| {
            let mut log = a_log();
            log.behind = behind;
            log.human(&out::Palette::plain())
        };

        assert!(
            render(Some(2)).contains("2 behind main"),
            "a count omh could take is reported"
        );
        assert_ne!(
            render(None),
            render(Some(0)),
            "an unanswered question and a zero are the two answers it is most \
             dangerous to confuse"
        );
        assert!(
            !render(Some(0)).contains("behind"),
            "nothing to say when the session is level with its base"
        );
        assert_eq!(a_log().json()["behind"], json!(2));
        let mut unknown = a_log();
        unknown.behind = None;
        assert_eq!(unknown.json()["behind"], json!(null));
    }

    /// A session with everything already handed over.
    ///
    /// The mirror of the all-new case, and three separate `> 0` guards live
    /// here: the header would read *0 not yours yet*, a divider would be
    /// inserted above the whole table claiming everything below it is the
    /// branch's, and the aside would offer to bring *0 new ones* over.
    #[test]
    fn a_session_with_nothing_left_to_hand_over_offers_nothing() {
        let mut log = a_log();
        log.read.commits.iter_mut().for_each(|c| c.landed = true);
        let printed = log.human(&out::Palette::plain());

        assert!(
            !printed.contains("not yours yet"),
            "there is no work the branch has not seen: {printed}"
        );
        assert!(
            !printed.contains("yours from here"),
            "and no line to draw, since everything is below it: {printed}"
        );
        // The checkpoints are still readable — that is not what changed. What
        // is gone is the offer to hand anything over.
        assert!(
            !log.asides().hints.join("\n").contains("--keep"),
            "nothing left to bring onto the branch: {:?}",
            log.asides().hints
        );
        assert_eq!(log.json()["pending"], json!(0));
    }

    /// When one line would mislabel, no line is drawn and the numbers are
    /// named instead.
    ///
    /// Below the divider is *labelled* `yours from here`. Under an interleaved
    /// history those rows are affirmatively wrong — the reader is told work is
    /// already on the branch when it is not, about commits `omh sNN rm` would
    /// destroy. An ordering imperfection would be tolerable; a wrong label is
    /// not.
    #[test]
    fn a_history_one_line_cannot_divide_gets_no_line() {
        let mut log = a_log();
        // oldest → newest: landed, not, landed, not
        log.read.commits[1].landed = false;
        log.read.commits[2].landed = true;
        let printed = log.human(&out::Palette::plain());
        let warnings = log.asides().warnings.join("\n");

        assert!(
            !printed.contains("yours from here"),
            "no line can say this: {printed}"
        );
        assert!(
            warnings.contains('1') && warnings.contains('3'),
            "so the numbers already on the branch are named: {warnings}"
        );
        assert!(
            printed.contains("2 not yours yet"),
            "and the count stays exact: {printed}"
        );
    }

    /// A merge reports as a merge, and an uncountable file as uncounted.
    ///
    /// Both are *omh did not measure this*, and the rendering they must never
    /// share is the one that means *nothing changed*.
    #[test]
    fn what_omh_did_not_measure_never_renders_as_nothing() {
        let mut log = a_log();
        log.read.commits[3].touched = None;
        log.read.commits[2].touched = Some(crate::shadow::Touched {
            files: 2,
            added: 0,
            removed: 0,
            uncounted: 2,
        });
        let printed = log.human(&out::Palette::plain());
        let line = |needle: &str| {
            printed
                .lines()
                .find(|l| l.contains(needle))
                .unwrap_or_else(|| panic!("no row for {needle}: {printed}"))
                .to_string()
        };

        assert!(
            line("Extract the tap guard").contains("merge"),
            "a merge says so rather than reporting 0 files: {}",
            line("Extract the tap guard")
        );
        assert!(
            !line("Extract the tap guard").contains("0 file"),
            "and never claims a measurement it did not take"
        );
        assert!(
            line("Add the failing test first").contains('·'),
            "two files git would not count are marked, not blank: {}",
            line("Add the failing test first")
        );
        assert_eq!(log.json()["checkpoints"][0]["merge"], json!(true));
        assert_eq!(log.json()["checkpoints"][0]["files"], json!(null));
        assert_eq!(log.json()["checkpoints"][1]["uncounted"], json!(2));
    }

    /// A date omh could not read is a question mark, not *just now*.
    #[test]
    fn a_checkpoint_omh_could_not_date_does_not_read_as_just_committed() {
        let mut log = a_log();
        log.read.commits[3].age = None;
        let printed = log.human(&out::Palette::plain());
        let row = printed
            .lines()
            .find(|l| l.contains("Extract the tap guard"))
            .unwrap();

        assert!(row.contains('?'), "the age is unknown and says so: {row}");
        assert!(
            !row.contains("0s"),
            "not the strongest possible claim as a fallback for having none: {row}"
        );
        assert_eq!(log.json()["checkpoints"][0]["age_seconds"], json!(null));
    }

    /// Two states make the list incomplete, and both are refusals waiting to
    /// happen. Neither may be offered a `--keep` — a hint is a promise the
    /// line can be pasted.
    #[test]
    fn work_the_log_cannot_show_is_said_and_the_harvest_is_not_offered() {
        for (label, wreck) in [
            (
                "commits on a branch it wandered off",
                (|log: &mut Log| log.read.unreachable = 3) as fn(&mut Log),
            ),
            ("a lost replay point", |log: &mut Log| {
                log.read.replay_point_lost = true
            }),
        ] {
            let mut log = a_log();
            wreck(&mut log);
            let warnings = log.asides().warnings.join("\n");

            assert!(
                !warnings.is_empty(),
                "{label} has to reach the reader: {warnings}"
            );
            assert!(
                !log.asides().hints.join("\n").contains("--keep"),
                "--keep is not offered when omh knows it would be refused ({label}): {:?}",
                log.asides().hints
            );
        }
        assert_eq!(a_log().json()["unreachable"], json!(0));
        assert_eq!(a_log().json()["replay_point_lost"], json!(false));
    }

    /// The JSON is a contract, and the fields nobody asserts are the ones that
    /// drift.
    #[test]
    fn the_json_carries_every_field_a_script_reads() {
        let mut log = a_log();
        log.read.commits[3].subject = "Fix \u{1b}[31m things".into();
        let v = log.json();
        let newest = &v["checkpoints"][0];

        assert_eq!(v["session"], json!("s01"));
        assert_eq!(v["base"], json!("main"));
        assert_eq!(v["uncommitted"], json!(2));
        assert_eq!(newest["number"], json!(4));
        assert_eq!(newest["files"], json!(4));
        assert_eq!(newest["added"], json!(40), "added is not removed");
        assert_eq!(newest["removed"], json!(4), "and removed is not added");
        assert_eq!(newest["age_seconds"], json!(2400));
        assert!(newest["id"].as_str().is_some_and(|id| !id.is_empty()));
        // Deliberately raw, and the asymmetry with `human` is the point: a
        // program is not a terminal, and a subject with a replacement
        // character in it is one it cannot match against git's own output.
        assert!(
            newest["subject"].as_str().unwrap().contains('\u{1b}'),
            "the escape survives into JSON: {newest}"
        );
    }

    /// Each arm of the churn column, including the two that only a real
    /// history reaches.
    #[test]
    fn churn_drops_the_half_that_is_zero_and_never_blanks_the_uncounted() {
        let t = |added, removed, uncounted| {
            churn(&crate::shadow::Touched {
                files: 1,
                added,
                removed,
                uncounted,
            })
        };
        assert_eq!(t(48, 12, 0), "+48 −12");
        assert_eq!(t(48, 0, 0), "+48", "no +48 −0 in a column scanned for size");
        assert_eq!(t(0, 12, 0), "−12");
        assert_eq!(t(0, 0, 0), "", "git measured, and nothing changed");
        assert_eq!(
            t(0, 0, 2),
            "·2",
            "git would not measure — not the same, not blank"
        );
        assert_eq!(t(48, 12, 1), "+48 −12 ·1");
    }

    /// One unit, and the boundaries where it changes.
    #[test]
    fn an_age_reads_as_one_unit() {
        assert_eq!(ago(0), "0s");
        assert_eq!(ago(59), "59s");
        assert_eq!(ago(60), "1m");
        assert_eq!(ago(60 * 60 - 1), "59m");
        assert_eq!(ago(60 * 60), "1h");
        // Hours as far as two days: 36h reads as yesterday evening, `1d` does
        // not.
        assert_eq!(ago(36 * 60 * 60), "36h");
        assert_eq!(ago(48 * 60 * 60), "2d");
        // Away from the boundary too: `s / (23 * 60 * 60)` also yields 2 for
        // the line above, so the divisor is only pinned by a day that is not
        // adjacent to the switch.
        assert_eq!(ago(9 * 24 * 60 * 60), "9d");
    }

    use super::*;
    use crate::out::{emit, Format, Palette};

    fn session(id: &str, work: Work) -> Session {
        Session {
            id: id.into(),
            label: "claude".into(),
            running: Some(crate::image::Running::No),
            work: Some(work),
            behind: Some(0),
        }
    }

    fn sessions(rows: Vec<Session>) -> Sessions {
        Sessions {
            sessions: rows,
            base: "main".into(),
            leftovers: vec![],
            overlaps: vec![],
            unreadable: vec![],
        }
    }

    /// A session that has fallen behind is told what to do about it — and
    /// only when doing it would change something.
    ///
    /// `behind 12` was reported and unactionable for the whole life of this
    /// command: the number was right there and the only thing a user could do
    /// with it was worry. `omh sNN sync` is the answer now, and a dashboard
    /// that names the problem without naming the answer is the state this
    /// phase set out to leave.
    ///
    /// Named per session rather than as one sentence, because which session
    /// is the decision — and silent when every session is current, since a
    /// suggestion that is always there is one nobody reads.
    #[test]
    fn a_session_behind_its_base_is_told_what_to_do_about_it() {
        let behind = |id: &str, n: Option<usize>| {
            let mut row = session(id, Work::Clean);
            row.behind = n;
            row
        };

        let current = sessions(vec![behind("s01", Some(0))]);
        assert!(
            !current.asides().hints.join(" ").contains("sync"),
            "nothing to say when nothing is behind: {:?}",
            current.asides()
        );

        let stale = sessions(vec![
            behind("s01", Some(0)),
            behind("s02", Some(12)),
            behind("s03", Some(3)),
        ]);
        let said = stale.asides().hints.join("\n");
        assert!(
            said.contains("omh s02 sync") && said.contains("omh s03 sync"),
            "each one that is behind, by name: {said}"
        );
        assert!(
            !said.contains("omh s01 sync"),
            "and not the one that is current: {said}"
        );

        // *Could not tell* is not *behind*. Suggesting a sync over a question
        // omh failed to answer is advice built on a guess.
        let unknown = sessions(vec![behind("s01", None)]);
        assert!(
            !unknown.asides().hints.join(" ").contains("sync"),
            "an unanswered count is not a reason to act: {:?}",
            unknown.asides()
        );
        // But withholding the suggestion is not the same as saying nothing.
        // Beside rows that each carry a next step, silence reads as *this one
        // is fine* — which is this change's own defect, moved from the cell
        // into the advice.
        let said = format!("{:?}", unknown.asides());
        assert!(
            said.contains("could not measure") && said.contains("s01 log"),
            "the row omh could not measure still gets a route: {said}"
        );
        // A run of spaces is a line continuation whose indentation shipped —
        // `cargo fmt` joins the fold and the padding goes with it. It happened
        // in this very sentence, and it is the same guard `git_checks_from`
        // carries for the same reason.
        //
        // Warnings only. A hint is a command with its description aligned
        // after it, so a run of spaces is what it is *for*; the prose beside
        // it has no reason to hold one.
        for line in &unknown.asides().warnings {
            assert!(!line.contains("  "), "a fold's indentation shipped: {line}");
        }
    }

    /// The `running` column has four answers and renders four ways.
    ///
    /// The same rule as `behind` one column over, arrived at the same way: a
    /// `bool` meant *stopped* covered both a container that is down and a
    /// runtime that would not say, so a Docker daemon that is not running
    /// showed every live sandbox as stopped — in the human table and in JSON,
    /// with nothing on stderr.
    ///
    /// *Nobody asked* is kept apart from *asked and could not tell* because
    /// they lead different places: no runtime at all is a machine that cannot
    /// run sessions, and a runtime that will not answer is one that usually
    /// can.
    #[test]
    fn a_runtime_that_would_not_answer_is_not_rendered_as_a_stopped_sandbox() {
        use crate::image::Running;
        let render = |running| {
            let mut row = session("s01", Work::Clean);
            row.running = running;
            sessions(vec![row]).human(&out::Palette::plain())
        };

        assert!(render(Some(Running::Yes)).contains("up"));
        assert!(render(Some(Running::No)).contains("stopped"));
        for (a, b, what) in [
            (
                Some(Running::Unknown("daemon down".into())),
                Some(Running::No),
                "a runtime that would not answer is not a stopped sandbox",
            ),
            (
                Some(Running::Yes),
                Some(Running::Unknown("daemon down".into())),
                "and it is not a sandbox omh confirmed was up either — `up?` \
                 contains `up`, so asserting on that substring cannot tell them apart",
            ),
            (
                None,
                Some(Running::Unknown("daemon down".into())),
                "a question nobody asked is not a question that went unanswered",
            ),
        ] {
            assert_ne!(render(a), render(b), "{what}");
        }
    }

    /// JSON keeps the same three answers, where getting it wrong is worst.
    ///
    /// A script reading `running == false` over an unreachable runtime got a
    /// fiction, and `--json` returns before asides, so there was no second
    /// signal anywhere in the document.
    #[test]
    fn a_sandbox_omh_could_not_ask_about_is_null_and_not_false() {
        use crate::image::Running;
        let field = |running| {
            let mut row = session("s01", Work::Clean);
            row.running = running;
            sessions(vec![row]).json()["sessions"][0]["running"].clone()
        };

        assert_eq!(field(Some(Running::Yes)), json!(true));
        assert_eq!(field(Some(Running::No)), json!(false));
        assert_eq!(
            field(Some(Running::Unknown("daemon down".into()))),
            serde_json::Value::Null,
            "a question omh could not answer is not a `false`"
        );
        assert_eq!(field(None), serde_json::Value::Null);

        // …and the two nulls are told apart by the field beside them, which is
        // the only place a script can learn *why*: `--json` returns before
        // asides, so the warning the human gets never reaches it.
        let why = |running| {
            let mut row = session("s01", Work::Clean);
            row.running = running;
            sessions(vec![row]).json()["sessions"][0]["running_unknown"].clone()
        };
        assert_eq!(
            why(Some(Running::Unknown("daemon down".into()))),
            json!("daemon down"),
            "the runtime's reason reaches a script"
        );
        assert_eq!(
            why(None),
            serde_json::Value::Null,
            "and nobody-asked carries no reason, because there is none"
        );
    }

    /// A running session is offered the spelling that works on it.
    ///
    /// `sync` refuses while the sandbox is up and names `--down` itself, so
    /// the bare form is a line that exits non-zero when pasted — offered, in
    /// the first version of this, on a row the table is printing `up` beside.
    /// That is the most common input the feature exists for: an agent that has
    /// been running a while against trunk that moved.
    #[test]
    fn a_running_session_is_told_the_form_of_sync_that_works_on_it() {
        let running = |up: bool| {
            let mut row = session("s01", Work::Clean);
            row.behind = Some(4);
            row.running = Some(match up {
                true => crate::image::Running::Yes,
                false => crate::image::Running::No,
            });
            sessions(vec![row]).asides().hints.join("\n")
        };

        assert!(
            running(true).contains("omh s01 sync --down"),
            "a running session is told to stop it first: {}",
            running(true)
        );
        assert!(
            !running(false).contains("--down"),
            "and a stopped one is not told to stop something: {}",
            running(false)
        );
    }

    /// Every suggested command aligns on the same column, whatever the ids
    /// are called.
    ///
    /// The pad was computed from `str::len` — bytes — under a comment
    /// justifying it by ids that are not `sNN`, which is the one case where
    /// bytes and columns disagree. `out::display_width` exists for this and
    /// the module says so where it is defined.
    #[test]
    fn the_suggested_commands_line_up_for_ids_of_any_width() {
        let row = |id: &str| {
            let mut s = session(id, Work::Clean);
            s.behind = Some(2);
            s
        };
        let hints = sessions(vec![row("s01"), row("café"), row("a-long-one")])
            .asides()
            .hints;

        let columns: Vec<usize> = hints
            .iter()
            .map(|h| out::display_width(h.split("bring").next().unwrap()))
            .collect();
        assert!(
            columns.windows(2).all(|w| w[0] == w[1]),
            "the description starts at one column: {hints:#?}"
        );
    }

    /// The dashboard has the same three answers about `behind` as `log` does,
    /// and had been rendering two of them the same.
    ///
    /// `Some(0) | None => Cell::plain("")` — an empty cell for *up to date*
    /// and an empty cell for *omh could not count*. This file states the rule
    /// at the top and `log` carries a paragraph about it; the dashboard is
    /// where a user actually decides which session to open, and it was the one
    /// surface answering the question wrong.
    ///
    /// A stale session that looks current is how work gets done against code
    /// that moved — which is the failure this whole phase exists to close.
    #[test]
    fn the_dashboard_does_not_render_an_unanswered_count_as_up_to_date() {
        let render = |behind| {
            let mut row = session("s01", Work::Clean);
            row.behind = behind;
            sessions(vec![row]).human(&out::Palette::plain())
        };

        assert!(
            render(Some(12)).contains("12 behind main"),
            "a count omh could take is reported: {}",
            render(Some(12))
        );
        assert_ne!(
            render(None),
            render(Some(0)),
            "an unanswered question and a zero are the two answers it is most \
             dangerous to confuse — the dashboard is where that decision is made"
        );
        // Not `!contains("behind main")` — the honest rendering says *how far
        // behind main?* and contains it. What must not happen is a **number**
        // in front of it, which is the shape a reader acts on.
        //
        // Asked of the cell rather than of the row. Scanning the whole line
        // read the id, the label and the work column too, so it was green by
        // the fixture's good luck: one `Work::Uncommitted(3)` beside it, or a
        // session id with a digit in it, and this would have failed for a
        // reason with nothing to do with the claim.
        let unknown = behind_cell(None, "main");
        let cell = unknown.text();
        assert!(
            !cell.split_whitespace().any(|word| word
                .trim_matches(|c| c == '(' || c == ')')
                .parse::<usize>()
                .is_ok()),
            "*could not tell* is not dressed up as a count: {cell}"
        );
        assert!(
            cell.contains("how far behind main"),
            "and it does ask the question rather than going quiet: {cell}"
        );

        // The base is a parameter and nothing proved it was read. Every
        // fixture in this file says `main`, so three separate mutations —
        // hardcoding the word in either arm of the cell, or in the hint —
        // were unkillable.
        assert!(
            behind_cell(Some(4), "develop").text().contains("develop")
                && behind_cell(None, "develop").text().contains("develop"),
            "the base is read, not assumed"
        );
    }

    /// `omh info` renders the same column and had the same bug.
    ///
    /// The extraction's whole argument is that two copies is one more than can
    /// be checked — and the first version of it checked one. Restoring the old
    /// inline `match` in `Inventory` alone left the suite green, on a listing
    /// that is also somewhere a user picks which session to open.
    #[test]
    fn the_wide_listing_answers_the_same_question_the_same_way() {
        let render = |behind| {
            let mut row = session("s01", Work::Clean);
            row.behind = behind;
            Inventory {
                harnesses: vec![],
                adapters_dir: "/adapters".into(),
                editors: vec![],
                sessions: vec![row],
                base: "main".into(),
            }
            .human(&out::Palette::plain())
        };

        assert_ne!(
            render(None),
            render(Some(0)),
            "the wide listing keeps the two apart too"
        );
        assert!(
            render(Some(7)).contains("7 behind main"),
            "and still reports a count it could take: {}",
            render(Some(7))
        );
    }

    /// The three answers survive into JSON, which is where getting it wrong
    /// costs the most.
    ///
    /// A hint never reaches a script — `--json` returns before asides — so
    /// this field is the *only* carrier of staleness there. `unwrap_or(0)` at
    /// either call site is the original defect in the format with no second
    /// signal, and it was green across the whole suite.
    #[test]
    fn a_count_omh_could_not_take_is_null_and_not_zero_in_both_listings() {
        let row = |behind| {
            let mut s = session("s01", Work::Clean);
            s.behind = behind;
            s
        };
        let wide = |behind| {
            Inventory {
                harnesses: vec![],
                adapters_dir: "/adapters".into(),
                editors: vec![],
                sessions: vec![row(behind)],
                base: "main".into(),
            }
            .json()["sessions"][0]["behind"]
                .clone()
        };

        for (dashboard, listing, expected, what) in [
            (
                sessions(vec![row(None)]).json()["sessions"][0]["behind"].clone(),
                wide(None),
                serde_json::Value::Null,
                "a count omh could not take is null",
            ),
            (
                sessions(vec![row(Some(0))]).json()["sessions"][0]["behind"].clone(),
                wide(Some(0)),
                json!(0),
                "and a zero is a zero",
            ),
        ] {
            assert_eq!(dashboard, expected, "{what}, on the dashboard");
            assert_eq!(listing, expected, "{what}, in the wide listing");
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
    /// The reason to run `omh info` at all is usually "why can't it log in",
    /// and
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
            .next("omh s01 rm")
            .note("teammates keep it until you commit the deletion");

        let hints = action.asides().hints;
        assert!(
            hints.iter().any(|l| l.trim() == "omh s01 rm"),
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
            json!(["omh s01 rm"]),
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
