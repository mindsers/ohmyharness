//! The screen `omh settings` is at a terminal: your defaults, your servers and
//! every catalogue entry, changed in place.
//!
//! **What it shows and how a key moves through it, and nothing else.** It
//! never reads `~/.omh` and never writes it. The [`Model`] is handed in, and
//! what a key asks for comes back as an [`Act`] — one command line omh
//! accepts, which the caller runs and reports. So everything the screen can do
//! is something you could have typed, it is said in the words that command
//! says it in, and the two cannot drift: there is only one of them.
//!
//! A leaf. Nothing in the crate is imported here, which keeps it out of the
//! module cycle `testsrc` ratchets, and keeps it testable without a terminal,
//! a home directory or a child process: a key goes in, and a [`Next`] and a
//! drawn frame come out. What the screen needs to know about a value — is
//! `tmux` a persistence mode? — is asked through a [`Check`] the caller hands
//! in, so the answer is `key::quarrel`'s and not a second copy of it.
//!
//! ## How it looks
//!
//! Three columns where there is room — pages, the list, what the selected
//! entry is — and the list over its detail where there is not. One accent
//! colour marks where you are; everything secondary is dimmed rather than
//! greyed, so it reads on a light terminal as on a dark one. What a command
//! said is one line above the keys, with a mark for how it went, and fades
//! at the next key.

use ratatui::crossterm::event::MouseEventKind;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent};
use ratatui::layout::{Constraint, Layout, Margin, Position, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{
    Block, BorderType, Clear, List, ListItem, ListState, Padding, Paragraph, Wrap,
};
use ratatui::Frame;
use std::cell::RefCell;

/// What there is to show, read by the caller.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Model {
    /// Where your catalogue is, for the header.
    pub root: String,
    pub sections: Vec<Section>,
}

/// One page of the screen: your defaults, or one kind of catalogue entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Section {
    pub title: &'static str,
    pub page: Page,
    pub rows: Vec<Row>,
    /// The read that failed, when one did. **Could not look is not none**: a
    /// section omh could not read says so, rather than drawing an empty list
    /// that reads as "you have no skills".
    pub problem: Option<String>,
    pub import: Import,
    /// What belongs on the page, said where an empty one has room for it.
    pub about: Vec<String>,
    /// How much of the page new repos get: "new repos get all", "… 1 of 4".
    pub seeds: Option<String>,
}

/// Which kind of page, by what each key does on it.
///
/// Three kinds, so three values: an `Option<&str>` spelled them `None`,
/// `Some("mcp")` and `Some(_)`, and whether a key meant the right thing on the
/// servers page hung on which arm of each match came first.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Page {
    /// Your defaults: `enter` sets, `d` unsets.
    Defaults,
    /// Your servers — records in one file, so `e` opens the file.
    Servers,
    /// One kind of entry, by the capability a command names it with.
    Entries(&'static str),
}

impl Page {
    /// What a command line calls this page's kind, if it is a kind.
    pub fn capability(self) -> Option<&'static str> {
        match self {
            Page::Defaults => None,
            Page::Servers => Some("mcp"),
            Page::Entries(capability) => Some(capability),
        }
    }
}

/// What `i` does on a page.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Import {
    /// Pick one of these harnesses to import from.
    From(Vec<Source>),
    /// Nothing, for this reason.
    Not(String),
}

/// A harness to import from, and where it reads — said beside it in the pick.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Source {
    pub harness: String,
    /// The place, as the caller says it: `~/.claude/skills — nothing there`.
    pub from: String,
}

/// One default, one server, or one entry.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Row {
    pub name: String,
    /// What it is set to: a default's value, a table's contents.
    pub value: Option<String>,
    /// What applies when there is no value — `auto`, `never stopped`.
    pub fallback: Option<String>,
    /// One line about it, drawn beside the name.
    pub summary: Option<String>,
    /// A short tag after the name: `omh`, `rust`.
    pub badge: Option<String>,
    /// Something wrong with it, drawn as a warning — `no SKILL.md`, `did you
    /// mean carry_in?`.
    pub warning: Option<String>,
    /// The heading it sits under, on a page that has them.
    pub group: Option<String>,
    /// What it is, in the detail beside the list.
    pub about: Vec<String>,
    /// How `enter` changes its value, on your defaults.
    pub edit: Edit,
    /// omh's own — a server of one of its features, a hook it ships — which
    /// the command refuses to remove, so the screen does not offer to. `d`
    /// still asks the command, and its refusal says what to do instead.
    pub fixed: bool,
    /// Whether the repos `omh init` sets up after get it, where that is a
    /// question: an entry of your catalogue, a server omh owns.
    pub seeded: Option<bool>,
    /// What flips that — the command, worked out by the caller, so the screen
    /// keeps no rule about `[use]` or features of its own.
    pub toggle: Option<Act>,
    /// What undoes the flip exactly, worked out by the caller with it — `None`
    /// where no one command puts the file back: a guard taken by a kind with
    /// no list writes the kind as one, and a rule taken off its list goes
    /// back at the end of it, where it reads differently.
    pub untoggle: Option<Act>,
    /// The key one nothing reads was probably meant to be: Enter moves its
    /// value there.
    pub meant: Option<String>,
}

/// How `enter` sets a default.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum Edit {
    /// It does not: a table is the file's to hold, and `e` opens the file.
    #[default]
    None,
    /// A line, with what the value looks like.
    Text { hint: String },
    /// One of these, and nothing else.
    Choice(Vec<String>),
}

/// What a key asked for: one command line omh accepts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Act {
    Set {
        key: String,
        value: String,
    },
    /// A value omh cannot read, written anyway: `set` refuses one without
    /// `--anyway`, for a value only a newer omh reads.
    SetAnyway {
        key: String,
        value: String,
    },
    Unset {
        key: String,
    },
    /// `$EDITOR` on your defaults file.
    EditDefaults,
    /// `$EDITOR` on one entry, or on the place a kind lives.
    Edit {
        capability: String,
        name: Option<String>,
    },
    Remove {
        capability: String,
        name: String,
    },
    AddServer {
        name: String,
        command: String,
        args: Vec<String>,
        env: Vec<(String, String)>,
    },
    Import {
        capability: String,
        harness: String,
    },
    /// Have the repos `omh init` sets up after take an entry.
    Use {
        capability: String,
        name: String,
    },
    /// Have them leave one out.
    Unuse {
        capability: String,
        name: String,
    },
    /// A key nothing reads, its value moved to the key it was meant to be:
    /// that one set, this one unset.
    Move {
        from: String,
        to: String,
        value: String,
    },
}

impl Act {
    /// The row this act changes, when it leaves one to point at.
    fn subject(&self) -> Option<String> {
        match self {
            Act::Set { key, .. } | Act::SetAnyway { key, .. } | Act::Unset { key } => {
                Some(key.clone())
            }
            Act::Edit { name, .. } => name.clone(),
            Act::Use { name, .. } | Act::Unuse { name, .. } => Some(name.clone()),
            Act::Move { to, .. } => Some(to.clone()),
            Act::AddServer { name, .. } => Some(name.clone()),
            Act::EditDefaults | Act::Remove { .. } | Act::Import { .. } => None,
        }
    }
}

/// What the caller does after a key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Next {
    Stay,
    Quit,
    /// Run it, then [`App::report`] what it said.
    Run(Act),
    /// Preview it, then [`App::confirm`] with what the preview said — or
    /// report the refusal. Removal is the one act that asks first, and it
    /// asks in the words of the preview, so a refusal comes before the
    /// question rather than after somebody has said yes.
    Preview(Act),
    /// Read your catalogue again: a file changed outside the screen.
    Reload,
}

/// How a status line reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    Ok,
    Warn,
    Bad,
    Info,
}

/// What a value would draw from omh if it were set: a warning, or nothing.
/// `key::quarrel`, handed in, so the screen holds no rule of its own.
pub type Check = Box<dyn Fn(&str, &str) -> Option<String>>;

pub struct App {
    model: Model,
    at: usize,
    /// The selected row of each page, as an index into the rows it shows.
    rows: Vec<usize>,
    modal: Option<Modal>,
    status: Option<Status>,
    look: Look,
    check: Option<Check>,
    /// Your home, said as `~` in what a command printed — as the screen says
    /// every path of its own.
    home: Option<String>,
    filter: Option<Filter>,
    /// The row an act just changed, marked until the next key.
    changed: Option<String>,
    /// The row the act being run will change, once the page is read again.
    pending: Option<(usize, String)>,
    /// What undoes the act being run, kept once the command says it worked.
    pending_undo: Option<Act>,
    /// What undoes the last change that worked, for `u`.
    undo: Option<Act>,
    /// Where the last draw put things, for a mouse to find them.
    hit: RefCell<Hit>,
}

/// What a command said, and whether it is still news.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Status {
    tone: Tone,
    lines: Vec<(Tone, String)>,
    /// The command line it was, so what the screen did is something you can
    /// read — and, but for the values it hides, type next time.
    said: Option<String>,
    /// Drawn whole until the next key, dimmed after.
    fresh: bool,
}

/// The rows of the current page narrowed to the ones that match.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct Filter {
    text: String,
    /// Still being typed: keys go to the filter, not the page.
    typing: bool,
}

#[derive(Debug, Clone, Default)]
struct Hit {
    pages: Rect,
    list: Rect,
    /// For each row of `list` on screen, the shown row it is — `None` for a
    /// heading or a blank.
    lines: Vec<Option<usize>>,
    /// How far help can scroll at the size it was last drawn.
    help_end: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Modal {
    Input(Input),
    Confirm {
        lines: Vec<String>,
        /// What the preview said on stderr: said before the question, not
        /// dropped because it was not the answer.
        warnings: Vec<String>,
        act: Act,
    },
    Pick(Pick),
    Form(Form),
    Help {
        scroll: usize,
    },
}

/// A line being typed, and where the cursor is in it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct Typed {
    text: Vec<char>,
    cursor: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Input {
    title: String,
    /// What the key is for, and what its value looks like.
    about: Vec<String>,
    label: String,
    line: Typed,
    error: Option<String>,
    /// Enter was pressed on a value the check flags: the next writes it
    /// anyway. Typing again starts over.
    forcing: bool,
    then: Then,
}

/// What a submitted line becomes.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Then {
    Value { key: String },
    NewEntry { capability: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Pick {
    title: String,
    options: Vec<String>,
    at: usize,
    /// The option that is the current value, marked.
    current: Option<usize>,
    /// The option that applies when nothing is set, marked too.
    fallback: Option<usize>,
    /// What is being set, as the row says it.
    about: Vec<String>,
    /// A line beside each option — where an import reads — or none.
    notes: Vec<String>,
    /// The letters typed so far, while an option starts with them.
    typed: String,
    then: Chosen,
}

/// What a picked option becomes.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Chosen {
    Import { capability: String },
    Value { key: String },
}

/// A new server, asked for in one form rather than three questions in a row.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Form {
    fields: [Typed; 3],
    at: usize,
    error: Option<String>,
    /// Esc was pressed with something typed: the next one throws it away.
    leaving: bool,
}

const FORM_LABELS: [&str; 3] = [
    "name: ",
    "command and arguments: ",
    "env (KEY=VALUE …), or nothing: ",
];

/// No modifier but Shift: a letter as it was typed.
///
/// crossterm reports Ctrl-D as `d` held with CONTROL, so a page that matched
/// the letter alone unset a default on Ctrl-D — the key a hand reaches for to
/// leave a terminal program.
fn plain(key: KeyEvent) -> bool {
    !key.modifiers.intersects(
        KeyModifiers::CONTROL | KeyModifiers::ALT | KeyModifiers::SUPER | KeyModifiers::META,
    )
}

impl App {
    pub fn new(model: Model, color: bool) -> Self {
        let model = cleaned(model);
        let rows = vec![0; model.sections.len()];
        Self {
            model,
            at: 0,
            rows,
            modal: None,
            status: None,
            look: Look { color },
            check: None,
            home: None,
            filter: None,
            changed: None,
            pending: None,
            pending_undo: None,
            undo: None,
            hit: RefCell::new(Hit::default()),
        }
    }

    /// Ask this, as a value is typed, what omh would say about it.
    pub fn with_check(mut self, check: Check) -> Self {
        self.check = Some(check);
        self
    }

    /// Say `home` as `~` in what a command printed, for display alone: the
    /// line that ran is the line that ran.
    pub fn with_home(mut self, home: Option<String>) -> Self {
        self.home = home.filter(|h| !h.is_empty());
        self
    }

    /// A line a command printed, its paths from your home down.
    fn homed(&self, line: &str) -> String {
        let Some(home) = &self.home else {
            return clean(line);
        };
        let mut out = String::new();
        let mut rest = line;
        while let Some(at) = rest.find(home.as_str()) {
            let after = &rest[at + home.len()..];
            out.push_str(&rest[..at]);
            // Only the home itself: `/Users/you` in `/Users/young` is not it.
            match after.chars().next() {
                None | Some('/') => out.push('~'),
                Some(_) => out.push_str(home),
            }
            rest = after;
        }
        out.push_str(rest);
        clean(&out)
    }

    /// A fresh read, after an act. The selection follows the row by name, so
    /// an entry that moved in the list stays selected and one that went
    /// leaves the cursor where it was.
    pub fn reload(&mut self, model: Model) {
        let was: Vec<Option<String>> = (0..self.model.sections.len())
            .map(|i| self.row_in(i).map(|r| r.name.clone()))
            .collect();
        self.model = cleaned(model);
        self.rows.resize(self.model.sections.len(), 0);
        self.at = self.at.min(self.model.sections.len().saturating_sub(1));
        for i in 0..self.model.sections.len() {
            let shown = self.shown(i);
            let named = was
                .get(i)
                .cloned()
                .flatten()
                .and_then(|name| self.position(i, &shown, &name));
            self.rows[i] = named.unwrap_or(self.rows[i].min(shown.len().saturating_sub(1)));
        }
        if let Some((page, name)) = self.pending.take() {
            let shown = self.shown(page);
            if let Some(i) = self.position(page, &shown, &name) {
                if page == self.at {
                    self.rows[page] = i;
                    self.changed = Some(name);
                }
            }
        }
    }

    fn position(&self, page: usize, shown: &[usize], name: &str) -> Option<usize> {
        shown
            .iter()
            .position(|&r| self.model.sections[page].rows[r].name == name)
    }

    /// What a command said, in its own words: its answer, then what it
    /// wrote to stderr — warnings when it worked, the refusal when it did not.
    pub fn report(&mut self, said: String, ok: bool, stdout: &str, stderr: &str) {
        // What undoes it is kept only once it worked; the last change that did
        // is what `u` undoes, and one that did not is not a change.
        match ok {
            true => self.undo = self.pending_undo.take(),
            false => self.pending_undo = None,
        }
        let (out, err) = if ok {
            (Tone::Ok, Tone::Warn)
        } else {
            (Tone::Info, Tone::Bad)
        };
        let mut lines: Vec<(Tone, String)> = [(stdout, out), (stderr, err)]
            .into_iter()
            .flat_map(|(text, tone)| {
                // Every line here is omh's: its name said again is noise.
                text.lines()
                    .filter(|l| !l.trim().is_empty())
                    .map(move |l| (tone, l.strip_prefix("omh: ").unwrap_or(l)))
            })
            .map(|(tone, l)| (tone, self.homed(l)))
            .collect();
        if lines.is_empty() {
            lines.push(if ok {
                (Tone::Ok, "done".to_string())
            } else {
                (Tone::Bad, "failed, and said nothing".to_string())
            });
        }
        let tone = match ok {
            false => Tone::Bad,
            true if lines.iter().any(|(t, _)| *t == Tone::Warn) => Tone::Warn,
            true => Tone::Ok,
        };
        self.status = Some(Status {
            tone,
            lines,
            said: Some(clean(&said)),
            fresh: true,
        });
    }

    /// One line of the screen's own, for something no command was run for.
    pub fn note(&mut self, line: impl Into<String>) {
        self.status = Some(Status {
            tone: Tone::Info,
            lines: vec![(Tone::Info, clean(&line.into()))],
            said: None,
            fresh: true,
        });
    }

    /// Ask before running `act`, showing what its preview said it would do,
    /// and any warning it gave on the way.
    pub fn confirm(&mut self, lines: Vec<String>, warnings: Vec<String>, act: Act) {
        self.modal = Some(Modal::Confirm {
            lines: lines.iter().map(|l| self.homed(l)).collect(),
            warnings: warnings.iter().map(|l| self.homed(l)).collect(),
            act,
        });
    }

    pub fn key(&mut self, key: KeyEvent) -> Next {
        if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
            return Next::Quit;
        }
        // What a command said stays, dimmed, until something else is said;
        // the mark on the row it changed goes now.
        if let Some(status) = &mut self.status {
            status.fresh = false;
        }
        self.changed = None;
        let next = match self.modal.take() {
            Some(modal) => self.key_in_modal(modal, key),
            None if self.filter.as_ref().is_some_and(|f| f.typing) => self.key_in_filter(key),
            None if plain(key) => self.key_on_page(key),
            // Ctrl-D leaves, as it leaves a shell, and Ctrl-L reads again, as
            // it redraws one; every other held letter is not the letter.
            None if key.code == KeyCode::Char('d')
                && key.modifiers.contains(KeyModifiers::CONTROL) =>
            {
                Next::Quit
            }
            None if key.code == KeyCode::Char('l')
                && key.modifiers.contains(KeyModifiers::CONTROL) =>
            {
                Next::Reload
            }
            None => Next::Stay,
        };
        if let Next::Run(act) = &next {
            self.pending = act.subject().map(|name| (self.at, name));
            self.pending_undo = self.inverse(act);
        }
        next
    }

    /// A click or a turn of the wheel, where the last draw put things.
    pub fn mouse(&mut self, event: MouseEvent) -> Next {
        if self.modal.is_some() {
            return Next::Stay;
        }
        let at = Position::new(event.column, event.row);
        let hit = self.hit.borrow().clone();
        match event.kind {
            MouseEventKind::ScrollDown => self.step(1),
            MouseEventKind::ScrollUp => self.step(-1),
            MouseEventKind::Down(MouseButton::Left) if hit.pages.contains(at) => {
                let n = usize::from(at.y - hit.pages.y);
                if n < self.model.sections.len() {
                    self.go_to(n);
                }
            }
            MouseEventKind::Down(MouseButton::Left) if hit.list.contains(at) => {
                let line = usize::from(at.y - hit.list.y);
                if let Some(Some(row)) = hit.lines.get(line) {
                    // A click on what is selected is `enter`, as in every list
                    // that takes a mouse.
                    if self.rows[self.at] == *row {
                        let next = self.enter();
                        if let Next::Run(act) = &next {
                            self.pending = act.subject().map(|name| (self.at, name));
                            self.pending_undo = self.inverse(act);
                        }
                        return next;
                    }
                    self.rows[self.at] = *row;
                }
            }
            _ => {}
        }
        Next::Stay
    }

    /// The command that undoes an act, from what the screen shows now: a
    /// value set back to the one it had, or unset if it had none; an unset put
    /// back; a flip by what its row says undoes it. `None` where there is no
    /// inverse to run — a removal, which takes the file with it, and a flip
    /// no one command puts back.
    fn inverse(&self, act: &Act) -> Option<Act> {
        let had = |key: &str| {
            self.model
                .sections
                .iter()
                .filter(|s| s.page == Page::Defaults)
                .flat_map(|s| &s.rows)
                .find(|r| r.name == key)
                .and_then(|r| r.value.clone())
        };
        // What was there goes back as it was, readable or not: one written
        // `--anyway` put back with a plain `set` was refused.
        let put_back = |key: &str, value: String| {
            let key = key.to_string();
            match self.check.as_ref().and_then(|c| c(&key, &value)) {
                Some(_) => Act::SetAnyway { key, value },
                None => Act::Set { key, value },
            }
        };
        match act {
            Act::Set { key, .. } | Act::SetAnyway { key, .. } => Some(match had(key) {
                Some(value) => put_back(key, value),
                None => Act::Unset { key: key.clone() },
            }),
            Act::Unset { key } => had(key).map(|value| put_back(key, value)),
            // The caller's, beside the flip it worked out: the opposite verb
            // is not always the way back.
            Act::Use { .. } | Act::Unuse { .. } => self
                .model
                .sections
                .iter()
                .flat_map(|s| &s.rows)
                .find(|r| r.toggle.as_ref() == Some(act))
                .and_then(|r| r.untoggle.clone()),
            Act::Move { from, to, value } => Some(Act::Move {
                from: to.clone(),
                to: from.clone(),
                value: value.clone(),
            }),
            Act::EditDefaults
            | Act::Edit { .. }
            | Act::Remove { .. }
            | Act::AddServer { .. }
            | Act::Import { .. } => None,
        }
    }

    fn key_on_page(&mut self, key: KeyEvent) -> Next {
        let count = self.model.sections.len();
        match key.code {
            // Esc takes a filter off before it leaves.
            KeyCode::Esc if self.filter.is_some() => self.filter = None,
            KeyCode::Char('q') | KeyCode::Esc => return Next::Quit,
            KeyCode::Tab | KeyCode::Right | KeyCode::Char('l') if count > 0 => {
                self.go_to((self.at + 1) % count);
            }
            KeyCode::BackTab | KeyCode::Left | KeyCode::Char('h') if count > 0 => {
                self.go_to((self.at + count - 1) % count);
            }
            KeyCode::Char(c @ '1'..='9') => {
                let n = c as usize - '1' as usize;
                if n < count {
                    self.go_to(n);
                }
            }
            KeyCode::Down | KeyCode::Char('j') => self.step(1),
            KeyCode::Up | KeyCode::Char('k') => self.step(-1),
            KeyCode::Home | KeyCode::Char('g') => self.step(isize::MIN),
            KeyCode::End | KeyCode::Char('G') => self.step(isize::MAX),
            KeyCode::Enter => return self.enter(),
            KeyCode::Char('e') => return self.edit(),
            KeyCode::Char('a') => self.add(),
            KeyCode::Char('d') | KeyCode::Delete => return self.remove(),
            KeyCode::Char('i') => self.import(),
            KeyCode::Char('/') => {
                self.filter = Some(Filter {
                    text: String::new(),
                    typing: true,
                });
                self.rows[self.at] = 0;
            }
            KeyCode::Char('?') => self.modal = Some(Modal::Help { scroll: 0 }),
            KeyCode::Char('u') => {
                if let Some(act) = self.undo.take() {
                    return Next::Run(act);
                }
            }
            // Whether new repos get the row: the act it carries, the caller's.
            KeyCode::Char(' ') => {
                if let Some(act) = self.row().and_then(|r| r.toggle.clone()) {
                    return Next::Run(act);
                }
            }
            _ => {}
        }
        Next::Stay
    }

    fn key_in_filter(&mut self, key: KeyEvent) -> Next {
        let Some(filter) = &mut self.filter else {
            return Next::Stay;
        };
        match key.code {
            KeyCode::Esc => self.filter = None,
            KeyCode::Enter => {
                filter.typing = false;
                if filter.text.is_empty() {
                    self.filter = None;
                }
            }
            KeyCode::Backspace => {
                filter.text.pop();
                self.rows[self.at] = 0;
            }
            KeyCode::Down => self.step(1),
            KeyCode::Up => self.step(-1),
            KeyCode::Char(c) if plain(key) => {
                filter.text.push(c);
                self.rows[self.at] = 0;
            }
            _ => {}
        }
        Next::Stay
    }

    /// The filter goes with you: it is a search, and the rail says what it
    /// finds on every page.
    fn go_to(&mut self, page: usize) {
        self.at = page;
    }

    /// Move the selection by `by` shown rows, stopping at either end.
    fn step(&mut self, by: isize) {
        let shown = self.shown(self.at).len();
        if shown == 0 {
            return;
        }
        let at = self.rows[self.at] as isize;
        self.rows[self.at] = at.saturating_add(by).clamp(0, shown as isize - 1) as usize;
    }

    /// The rows of a page that are drawn, in order: all of them, or the ones
    /// the filter matches, on the page it is typed on.
    fn shown(&self, page: usize) -> Vec<usize> {
        let Some(section) = self.model.sections.get(page) else {
            return Vec::new();
        };
        let needle = match &self.filter {
            Some(f) if !f.text.is_empty() => f.text.to_lowercase(),
            _ => return (0..section.rows.len()).collect(),
        };
        section
            .rows
            .iter()
            .enumerate()
            .filter(|(_, r)| {
                [Some(&r.name), r.summary.as_ref(), r.value.as_ref()]
                    .into_iter()
                    .flatten()
                    .any(|t| t.to_lowercase().contains(&needle))
            })
            .map(|(i, _)| i)
            .collect()
    }

    fn enter(&mut self) -> Next {
        let Some(section) = self.section() else {
            return Next::Stay;
        };
        match section.page {
            Page::Defaults => {
                let Some(row) = self.row().cloned() else {
                    return Next::Stay;
                };
                if let Some(meant) = &row.meant {
                    let taken = section
                        .rows
                        .iter()
                        .any(|r| &r.name == meant && r.value.is_some());
                    if taken {
                        self.note(format!(
                            "{meant} is set already — d removes {}, or e opens the file",
                            row.name
                        ));
                        return Next::Stay;
                    }
                    return Next::Run(Act::Move {
                        from: row.name.clone(),
                        to: meant.clone(),
                        value: row.value.clone().unwrap_or_default(),
                    });
                }
                match &row.edit {
                    Edit::None => {
                        self.note(format!(
                            "{} is a table — e opens the file it is in",
                            row.name
                        ));
                    }
                    Edit::Text { hint } => self.ask_value(&row, hint),
                    Edit::Choice(options) => self.pick_value(&row, options.clone()),
                }
                Next::Stay
            }
            Page::Servers => {
                self.note("a server is a record in mcp.json — e opens it, d removes one");
                Next::Stay
            }
            Page::Entries(_) => self.edit(),
        }
    }

    fn ask_value(&mut self, row: &Row, hint: &str) {
        let current: Vec<char> = row.value.clone().unwrap_or_default().chars().collect();
        let mut about = row.about.clone();
        if !hint.is_empty() {
            about.push(hint.to_string());
        }
        self.modal = Some(Modal::Input(Input {
            title: format!("set {}", row.name),
            about,
            label: format!("{} = ", row.name),
            line: Typed {
                cursor: current.len(),
                text: current,
            },
            error: None,
            forcing: false,
            then: Then::Value {
                key: row.name.clone(),
            },
        }));
    }

    fn pick_value(&mut self, row: &Row, options: Vec<String>) {
        let current = row
            .value
            .as_ref()
            .and_then(|v| options.iter().position(|o| o == v.trim_matches('"')));
        let fallback = row
            .fallback
            .as_ref()
            .and_then(|f| options.iter().position(|o| o == f));
        self.modal = Some(Modal::Pick(Pick {
            title: format!("set {}", row.name),
            at: current.unwrap_or(0),
            current,
            fallback,
            about: row.about.clone(),
            notes: Vec::new(),
            options,
            typed: String::new(),
            then: Chosen::Value {
                key: row.name.clone(),
            },
        }));
    }

    fn edit(&mut self) -> Next {
        let Some(section) = self.section() else {
            return Next::Stay;
        };
        match section.page {
            Page::Defaults => Next::Run(Act::EditDefaults),
            // A server is a record in one file, so the file is what opens.
            Page::Servers => Next::Run(Act::Edit {
                capability: "mcp".into(),
                name: None,
            }),
            Page::Entries(capability) => Next::Run(Act::Edit {
                capability: capability.into(),
                name: self.row().map(|r| r.name.clone()),
            }),
        }
    }

    fn add(&mut self) {
        let Some(section) = self.section() else {
            return;
        };
        match section.page {
            Page::Defaults => self.note("every key omh reads is listed — enter sets one"),
            Page::Servers if section.problem.is_some() => {
                self.note("omh could not read your servers — e opens the file to fix it")
            }
            Page::Servers => {
                self.modal = Some(Modal::Form(Form {
                    fields: Default::default(),
                    at: 0,
                    error: None,
                    leaving: false,
                }));
            }
            Page::Entries(capability) => {
                self.modal = Some(Modal::Input(Input {
                    title: format!("new entry in {}", section.title),
                    about: vec![format!(
                        "opened in $EDITOR, as `omh settings edit {capability} <name>` opens it"
                    )],
                    label: "name: ".into(),
                    line: Typed::default(),
                    error: None,
                    forcing: false,
                    then: Then::NewEntry {
                        capability: capability.into(),
                    },
                }));
            }
        }
    }

    fn remove(&mut self) -> Next {
        let (Some(section), Some(row)) = (self.section(), self.row()) else {
            return Next::Stay;
        };
        match section.page.capability() {
            None if row.edit == Edit::None => {
                self.note(format!(
                    "{} is a table — e opens the file it is in",
                    row.name
                ));
                Next::Stay
            }
            None if row.value.is_none() => {
                let line = format!("{} is not set — there is nothing to unset", row.name);
                self.note(line);
                Next::Stay
            }
            None => Next::Run(Act::Unset {
                key: row.name.clone(),
            }),
            Some(capability) => Next::Preview(Act::Remove {
                capability: capability.into(),
                name: row.name.clone(),
            }),
        }
    }

    fn import(&mut self) {
        let Some(section) = self.section() else {
            return;
        };
        if section.page == Page::Servers && section.problem.is_some() {
            return self.note("omh could not read your servers — e opens the file to fix it");
        }
        let line = match (section.page.capability(), &section.import) {
            (Some(capability), Import::From(harnesses)) if !harnesses.is_empty() => {
                self.modal = Some(Modal::Pick(Pick {
                    title: format!("import {} from", section.title),
                    options: harnesses.iter().map(|h| h.harness.clone()).collect(),
                    notes: harnesses.iter().map(|h| h.from.clone()).collect(),
                    at: 0,
                    current: None,
                    fallback: None,
                    about: Vec::new(),
                    typed: String::new(),
                    then: Chosen::Import {
                        capability: capability.into(),
                    },
                }));
                return;
            }
            (_, Import::Not(why)) => why.clone(),
            (None, Import::From(_)) => "your defaults are typed, not imported".to_string(),
            (Some(_), Import::From(_)) => format!(
                "no installed harness keeps {} where omh can read them",
                section.title
            ),
        };
        self.note(line);
    }

    fn key_in_modal(&mut self, modal: Modal, key: KeyEvent) -> Next {
        match modal {
            // Scrolled as a list is; closed only by the keys that close
            // everything — `j` closed it, at 80×24 where it was cut.
            Modal::Help { mut scroll } => {
                let end = self.hit.borrow().help_end;
                match key.code {
                    KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char('?') => return Next::Stay,
                    KeyCode::Down | KeyCode::Char('j') => scroll = (scroll + 1).min(end),
                    KeyCode::Up | KeyCode::Char('k') => scroll = scroll.saturating_sub(1),
                    KeyCode::Home | KeyCode::Char('g') => scroll = 0,
                    KeyCode::End | KeyCode::Char('G') => scroll = end,
                    KeyCode::PageDown => scroll = (scroll + 10).min(end),
                    KeyCode::PageUp => scroll = scroll.saturating_sub(10),
                    _ => {}
                }
                self.modal = Some(Modal::Help { scroll });
                Next::Stay
            }
            Modal::Confirm { act, .. } => match key.code {
                KeyCode::Char('y') | KeyCode::Char('Y') if plain(key) => Next::Run(act),
                // Anything else keeps it, the rule `ask::confirm` follows: one
                // way to say yes, and everything else is not it.
                _ => {
                    self.note("kept");
                    Next::Stay
                }
            },
            Modal::Pick(mut pick) => {
                let starts =
                    |o: &String, with: &str| o.to_lowercase().starts_with(&with.to_lowercase());
                // A letter some option starts with — the next one after `at`
                // that does, so one letter again goes on to the next.
                let next_with = |pick: &Pick, with: &str| {
                    let after = pick.options.iter().enumerate().cycle().skip(pick.at + 1);
                    after
                        .take(pick.options.len())
                        .find(|(_, o)| starts(o, with))
                        .map(|(i, _)| i)
                };
                match key.code {
                    // Letters first: a word typed builds a prefix while some
                    // option starts with it. Each letter jumped on its own, so
                    // `podman` ended on `auto`, the last letter any option
                    // started. One that fits none starts again from itself;
                    // and a letter no option starts is what it is in a list.
                    KeyCode::Char(c) if plain(key) => {
                        let word = format!("{}{c}", pick.typed);
                        if let Some(i) = pick.options.iter().position(|o| starts(o, &word)) {
                            pick.at = i;
                            pick.typed = word;
                        } else if let Some(i) = next_with(&pick, &c.to_string()) {
                            pick.at = i;
                            pick.typed = c.to_string();
                        } else {
                            pick.typed.clear();
                            match c {
                                'j' => {
                                    pick.at =
                                        (pick.at + 1).min(pick.options.len().saturating_sub(1))
                                }
                                'k' => pick.at = pick.at.saturating_sub(1),
                                'g' => pick.at = 0,
                                'G' => pick.at = pick.options.len().saturating_sub(1),
                                // Closed by the keys that close everything
                                // else: it was taken off the screen with
                                // `take` and is not put back.
                                'q' => return Next::Stay,
                                // Any other key stays here — one that closed
                                // the pick went on to the page, where
                                // `podman`'s `d` unset the row.
                                _ => {}
                            }
                        }
                    }
                    KeyCode::Down => {
                        pick.typed.clear();
                        pick.at = (pick.at + 1).min(pick.options.len().saturating_sub(1));
                    }
                    KeyCode::Up => {
                        pick.typed.clear();
                        pick.at = pick.at.saturating_sub(1);
                    }
                    KeyCode::Home => pick.at = 0,
                    KeyCode::End => pick.at = pick.options.len().saturating_sub(1),
                    KeyCode::Backspace => {
                        pick.typed.pop();
                    }
                    KeyCode::Enter => return self.chosen(pick),
                    KeyCode::Esc => return Next::Stay,
                    _ => {}
                }
                self.modal = Some(Modal::Pick(pick));
                Next::Stay
            }
            Modal::Form(form) => self.key_in_form(form, key),
            Modal::Input(mut input) => match key.code {
                KeyCode::Esc => Next::Stay,
                KeyCode::Enter => self.submit(input),
                _ => {
                    input.line.edit(key);
                    input.error = None;
                    input.forcing = false;
                    self.modal = Some(Modal::Input(input));
                    Next::Stay
                }
            },
        }
    }

    fn chosen(&mut self, pick: Pick) -> Next {
        let Some(option) = pick.options.get(pick.at).cloned() else {
            return Next::Stay;
        };
        match pick.then {
            Chosen::Import { capability } => Next::Run(Act::Import {
                capability,
                harness: option,
            }),
            Chosen::Value { key } => Next::Run(Act::Set { key, value: option }),
        }
    }

    fn key_in_form(&mut self, mut form: Form, key: KeyEvent) -> Next {
        let leaving_the_name =
            form.at == 0 && matches!(key.code, KeyCode::Enter | KeyCode::Tab | KeyCode::Down);
        if let Some(why) = leaving_the_name
            .then(|| self.taken(&form.fields[0].text.iter().collect::<String>()))
            .flatten()
        {
            form.error = Some(why);
            self.modal = Some(Modal::Form(form));
            return Next::Stay;
        }
        match key.code {
            // Something typed is asked about once before it goes.
            KeyCode::Esc if form.leaving || form.fields.iter().all(|f| f.text.is_empty()) => {
                return Next::Stay;
            }
            KeyCode::Esc => {
                form.leaving = true;
                form.error = Some("esc again throws away what is typed".into());
            }
            KeyCode::Tab | KeyCode::Down => form.at = (form.at + 1).min(2),
            KeyCode::BackTab | KeyCode::Up => form.at = form.at.saturating_sub(1),
            KeyCode::Enter if form.at < 2 => form.at += 1,
            KeyCode::Enter => return self.submit_form(form),
            _ => {
                form.fields[form.at].edit(key);
                form.error = None;
                form.leaving = false;
            }
        }
        self.modal = Some(Modal::Form(form));
        Next::Stay
    }

    /// Why a server cannot be called `name`: one of that name is in your
    /// catalogue, and `mcp add` will not write over it — or is omh's own.
    fn taken(&self, name: &str) -> Option<String> {
        let name = name.trim();
        let row = self
            .model
            .sections
            .iter()
            .filter(|s| s.page == Page::Servers)
            .flat_map(|s| &s.rows)
            .find(|r| r.name == name)?;
        Some(match row.fixed {
            true => format!("`{name}` is omh's own server — it is switched off, not replaced"),
            false => format!("`{name}` is already a server — d removes it, e opens the file"),
        })
    }

    /// A finished form is a server, or the form again on the field that was
    /// wrong, saying what was.
    fn submit_form(&mut self, mut form: Form) -> Next {
        let [name, command, env] = form
            .fields
            .clone()
            .map(|l| l.text.iter().collect::<String>());
        let name = name.trim().to_string();
        let wrong = match (
            name.is_empty(),
            words(&command),
            words(&env).and_then(|w| parse_env(&w)),
        ) {
            (true, _, _) => Some((0, "a name, first".to_string())),
            _ if self.taken(&name).is_some() => self.taken(&name).map(|why| (0, why)),
            (false, Err(e), _) => Some((1, e)),
            (false, Ok(w), _) if w.is_empty() => Some((1, "the program to run, first".into())),
            (false, _, Err(e)) => Some((2, e)),
            (false, Ok(words), Ok(env)) => {
                let mut words = words.into_iter();
                return Next::Run(Act::AddServer {
                    name,
                    command: words.next().unwrap_or_default(),
                    args: words.collect(),
                    env,
                });
            }
        };
        if let Some((at, why)) = wrong {
            form.at = at;
            form.error = Some(why);
        }
        self.modal = Some(Modal::Form(form));
        Next::Stay
    }

    /// A line was entered. Either it is an act or the same question again
    /// with what was wrong with the answer.
    fn submit(&mut self, mut input: Input) -> Next {
        let line: String = input.line.text.iter().collect();
        match input.then.clone() {
            Then::Value { key } => {
                let flagged = self.check.as_ref().and_then(|check| check(&key, &line));
                match (flagged, input.forcing) {
                    (None, _) => Next::Run(Act::Set { key, value: line }),
                    (Some(_), true) => Next::Run(Act::SetAnyway { key, value: line }),
                    // `set` would refuse it: said here, and one more Enter is
                    // `--anyway`.
                    (Some(_), false) => {
                        input.forcing = true;
                        input.error = Some(
                            "omh cannot read this — ↵ again writes it anyway, for a value \
                             only a newer omh takes"
                                .into(),
                        );
                        self.modal = Some(Modal::Input(input));
                        Next::Stay
                    }
                }
            }
            Then::NewEntry { capability } => {
                let name = line.trim().to_string();
                if name.is_empty() {
                    input.error = Some("a name, first".into());
                    self.modal = Some(Modal::Input(input));
                    return Next::Stay;
                }
                // The server form's own check: one that is there opened as
                // though it were new.
                let there = self
                    .model
                    .sections
                    .iter()
                    .filter(|s| s.page.capability() == Some(capability.as_str()))
                    .flat_map(|s| &s.rows)
                    .any(|r| r.name == name);
                if there {
                    input.error = Some(format!(
                        "`{name}` is already one of yours — ↵ on its row opens it"
                    ));
                    self.modal = Some(Modal::Input(input));
                    return Next::Stay;
                }
                Next::Run(Act::Edit {
                    capability,
                    name: Some(name),
                })
            }
        }
    }

    fn section(&self) -> Option<&Section> {
        self.model.sections.get(self.at)
    }

    fn row(&self) -> Option<&Row> {
        self.row_in(self.at)
    }

    fn row_in(&self, page: usize) -> Option<&Row> {
        let section = self.model.sections.get(page)?;
        let shown = self.shown(page);
        section.rows.get(*shown.get(*self.rows.get(page)?)?)
    }

    // ── Drawing ─────────────────────────────────────────────────────────────

    pub fn draw(&self, f: &mut Frame) {
        let area = f.area();
        let status = self.status_text(area.width);
        let status_height = match &status {
            None => 0,
            // As tall as what it says, up to a quarter of the screen: a
            // refusal's reason and its next step come after its first line,
            // and a box that cut there kept the part nobody needed.
            Some(text) => (Paragraph::new(text.clone())
                .wrap(Wrap { trim: false })
                .line_count(area.width) as u16)
                .clamp(1, (area.height / 4).max(1)),
        };
        let [head, body, said, foot] = Layout::vertical([
            Constraint::Length(1),
            Constraint::Fill(1),
            Constraint::Length(status_height),
            Constraint::Length(1),
        ])
        .areas(area);

        self.draw_head(f, head);
        match Width::of(area.width) {
            Width::Wide => {
                let [pages, a, list, b, detail] = Layout::horizontal([
                    Constraint::Length(18),
                    Constraint::Length(1),
                    Constraint::Fill(6),
                    Constraint::Length(1),
                    Constraint::Fill(5),
                ])
                .areas(body);
                self.draw_pages(f, pages);
                self.draw_rule(f, a);
                self.draw_list(f, list, true);
                self.draw_rule(f, b);
                // Level with the list's heading, as the pages are.
                self.draw_detail(f, detail.inner(Margin::new(1, 0)));
            }
            Width::Medium => {
                let [pages, a, rest] = Layout::horizontal([
                    Constraint::Length(18),
                    Constraint::Length(1),
                    Constraint::Fill(1),
                ])
                .areas(body);
                self.draw_pages(f, pages);
                self.draw_rule(f, a);
                // Where the list's rule meets the column's, one line, not two.
                if let Some(y) = self.draw_stacked(f, rest, true) {
                    f.buffer_mut()[(a.x, y)]
                        .set_symbol("├")
                        .set_style(self.look.dim());
                }
            }
            Width::Narrow => {
                let [tabs, rest] =
                    Layout::vertical([Constraint::Length(1), Constraint::Fill(1)]).areas(body);
                self.draw_tabs(f, tabs);
                self.hit.borrow_mut().pages = Rect::default();
                // The page's name and count are the tab line's, here.
                self.draw_stacked(f, rest, false);
            }
        }
        if let Some(text) = status {
            f.render_widget(Paragraph::new(text).wrap(Wrap { trim: false }), said);
        }
        f.render_widget(Paragraph::new(self.keys(foot.width)), foot);

        match &self.modal {
            Some(Modal::Input(input)) => self.draw_input(f, input),
            Some(Modal::Confirm {
                lines,
                warnings,
                act,
            }) => self.draw_confirm(f, lines, warnings, act),
            Some(Modal::Pick(pick)) => self.draw_pick(f, pick),
            Some(Modal::Form(form)) => self.draw_form(f, form),
            Some(Modal::Help { scroll }) => self.draw_help(f, *scroll),
            None => {}
        }
    }

    fn draw_head(&self, f: &mut Frame, area: Rect) {
        f.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(" omh settings", self.look.title()),
                Span::styled(format!("  {}", self.model.root), self.look.dim()),
            ])),
            area,
        );
    }

    fn draw_rule(&self, f: &mut Frame, area: Rect) {
        let rule: Vec<Line> = (0..area.height)
            .map(|_| Line::from(Span::styled("│", self.look.dim())))
            .collect();
        f.render_widget(Paragraph::new(rule), area);
    }

    /// The list over its detail, where there is no room for them side by side.
    ///
    /// **The split is the page's, not the row's**: as tall as the tallest
    /// detail among the page's rows, up to two fifths. Sized by the selected
    /// one, the list grew and shrank under every `j`. A page with no row has
    /// no detail and no rule. Answers the rule's line, for the junction.
    fn draw_stacked(&self, f: &mut Frame, area: Rect, head: bool) -> Option<u16> {
        let section = self.section()?;
        let width = area.width.saturating_sub(2);
        let tallest = match self.row() {
            None => 0,
            Some(_) => section
                .rows
                .iter()
                .map(|row| {
                    Paragraph::new(self.detail_for(section, row))
                        .wrap(Wrap { trim: false })
                        .line_count(width) as u16
                })
                .max()
                .unwrap_or(0),
        };
        let height = match tallest {
            0 => 0,
            n => (n + 1).min(area.height * 2 / 5),
        };
        let [list, rule, detail_area] = Layout::vertical([
            Constraint::Fill(1),
            Constraint::Length(u16::from(height > 0)),
            Constraint::Length(height.saturating_sub(1)),
        ])
        .areas(area);
        self.draw_list(f, list, head);
        if height == 0 {
            return None;
        }
        f.render_widget(
            Paragraph::new(Span::styled(
                "─".repeat(usize::from(rule.width)),
                self.look.dim(),
            )),
            rule,
        );
        f.render_widget(
            Paragraph::new(self.detail_text()).wrap(Wrap { trim: false }),
            detail_area.inner(Margin::new(1, 0)),
        );
        Some(rule.y)
    }

    fn draw_pages(&self, f: &mut Frame, area: Rect) {
        // Level with the list's own heading, so the two columns start on one
        // line.
        self.hit.borrow_mut().pages = area;
        let mut lines = Vec::new();
        for (i, s) in self.model.sections.iter().enumerate() {
            let here = i == self.at;
            let mark = match here {
                true => Span::styled("▌ ", self.look.accent()),
                false => Span::raw("  "),
            };
            // What the page holds, before you go to it: `✗` where omh could
            // not look, `!` where a row has a warning, and how many. Your
            // defaults are every key omh reads, so a count there would be the
            // same number forever.
            let warned = s.rows.iter().any(|r| r.warning.is_some());
            let mut right = Vec::new();
            let searching = self.filter.as_ref().is_some_and(|f| !f.text.is_empty());
            match (s.problem.is_some(), s.page) {
                (true, _) => right.push(Span::styled("✗", self.look.tone(Tone::Bad))),
                // What a search finds here, of how many.
                (false, _) if searching => right.push(Span::styled(
                    format!("{}/{}", self.shown(i).len(), s.rows.len()),
                    match self.shown(i).is_empty() {
                        true => self.look.dim(),
                        false => self.look.accent(),
                    },
                )),
                (false, page) => {
                    if warned {
                        right.push(Span::styled("!", self.look.tone(Tone::Warn)));
                    }
                    if page != Page::Defaults {
                        right.push(Span::styled(
                            format!("{}{}", if warned { " " } else { "" }, s.rows.len()),
                            self.look.dim(),
                        ));
                    }
                }
            }
            let used: usize = right.iter().map(Span::width).sum();
            let number = Span::styled(format!("{} ", i + 1), self.look.dim());
            let width = usize::from(area.width).saturating_sub(2 + 2 + used + 1);
            let title = Span::styled(
                pad(&capitalised(s.title), width),
                match here {
                    true => self.look.title(),
                    false => Style::default(),
                },
            );
            let mut line = vec![mark, number, title];
            line.extend(right);
            lines.push(Line::from(line));
        }
        f.render_widget(Paragraph::new(lines), area);
    }

    /// One line naming the page, for a terminal too narrow for the column.
    fn draw_tabs(&self, f: &mut Frame, area: Rect) {
        let Some(section) = self.section() else {
            return;
        };
        let count = match (&section.problem, section.rows.is_empty()) {
            (Some(_), true) => "—".to_string(),
            _ => self.shown(self.at).len().to_string(),
        };
        let mut line = vec![
            Span::styled(" ‹ ", self.look.dim()),
            Span::styled(capitalised(section.title), self.look.title()),
            Span::styled(format!("  {count}"), self.look.dim()),
            Span::styled(
                format!("  {}/{} ›", self.at + 1, self.model.sections.len()),
                self.look.dim(),
            ),
        ];
        if let Some(filter) = &self.filter {
            line.push(Span::styled(
                format!(
                    "   / {}{}",
                    filter.text,
                    if filter.typing { "▏" } else { "" }
                ),
                match filter.typing {
                    true => self.look.accent(),
                    false => self.look.dim(),
                },
            ));
        }
        f.render_widget(Paragraph::new(Line::from(line)), area);
    }

    /// The page's rows, under its heading — which a narrow screen's tab line
    /// says instead, when `head` is false.
    fn draw_list(&self, f: &mut Frame, area: Rect, head: bool) {
        let Some(section) = self.section() else {
            return;
        };
        let area = area.inner(Margin::new(1, 0));
        let shown = self.shown(self.at);

        // The page's own heading: what it is, how many, and the filter.
        // A page omh could not read holds an unknown number, not none.
        let count = match (&section.problem, section.rows.is_empty()) {
            (Some(_), true) => "—".to_string(),
            _ => shown.len().to_string(),
        };
        let mut heading = vec![
            Span::styled(capitalised(section.title), self.look.title()),
            Span::styled(format!("  {count}"), self.look.dim()),
        ];
        // The filter before what new repos get: it was off the end of a long
        // heading, where nothing said it was still on.
        if let Some(filter) = &self.filter {
            heading.push(Span::styled(
                format!(
                    "   / {}{}",
                    filter.text,
                    if filter.typing { "▏" } else { "" }
                ),
                match filter.typing {
                    true => self.look.accent(),
                    false => self.look.dim(),
                },
            ));
        }
        if let Some(seeds) = &section.seeds {
            let used: usize = heading.iter().map(Span::width).sum();
            let room = usize::from(area.width).saturating_sub(used + 3);
            if room > 1 {
                heading.push(Span::styled(
                    format!("   {}", ellipsized(seeds, room)),
                    self.look.dim(),
                ));
            }
        }
        let lines = if head { 2 } else { 0 };
        let [head_area, mut list] =
            Layout::vertical([Constraint::Length(lines), Constraint::Fill(1)]).areas(area);
        f.render_widget(Paragraph::new(Line::from(heading)), head_area);

        // Above what was found, never instead of it — the servers page with
        // omh's base set unreadable has both — and as tall as it needs, up to
        // half the list: a failed read says *where* first and *why* last.
        if let Some(problem) = &section.problem {
            let text = Text::from(
                problem
                    .lines()
                    .enumerate()
                    .map(|(i, line)| {
                        Line::from(Span::styled(
                            match i {
                                0 => format!("✗ could not look — {line}"),
                                _ => format!("  {line}"),
                            },
                            self.look.tone(Tone::Bad),
                        ))
                    })
                    .collect::<Vec<_>>(),
            );
            let height = (Paragraph::new(text.clone())
                .wrap(Wrap { trim: false })
                .line_count(list.width) as u16)
                .min((list.height / 2).max(1));
            let [said, _, rest] = Layout::vertical([
                Constraint::Length(height),
                Constraint::Length(1),
                Constraint::Fill(1),
            ])
            .areas(list);
            f.render_widget(Paragraph::new(text).wrap(Wrap { trim: false }), said);
            list = rest;
        }

        let empty = match (section.rows.is_empty(), shown.is_empty()) {
            (true, _) if section.problem.is_some() => Some(Vec::new()),
            (true, _) => Some(match section.page {
                Page::Defaults => vec![Line::from(Span::styled(
                    "nothing omh reads",
                    self.look.dim(),
                ))],
                Page::Servers | Page::Entries(_) => vec![
                    Line::from(Span::styled(
                        format!("no {} in your catalogue yet", section.title),
                        self.look.dim(),
                    )),
                    Line::from(vec![
                        Span::styled("a", self.look.key()),
                        Span::styled(" adds one · ", self.look.dim()),
                        Span::styled("i", self.look.key()),
                        Span::styled(" imports some", self.look.dim()),
                    ]),
                ],
            }),
            (false, true) => Some(vec![Line::from(Span::styled(
                "nothing matches",
                self.look.dim(),
            ))]),
            (false, false) => None,
        };
        if let Some(lines) = empty {
            self.hit.borrow_mut().list = Rect::default();
            f.render_widget(Paragraph::new(lines), list);
            return;
        }

        let name_width = shown
            .iter()
            .map(|&r| Span::raw(section.rows[r].name.as_str()).width())
            .max()
            .unwrap_or(0)
            .min(28)
            .min(usize::from(list.width) / 2);
        // Badges in a column of their own, so what follows them lines up.
        let badge_width = shown
            .iter()
            .filter_map(|&r| section.rows[r].badge.as_deref())
            .map(|b| Span::raw(b).width() + 1)
            .max()
            .unwrap_or(0)
            .min(10);
        let mut items = Vec::new();
        let mut index = Vec::new();
        // Where each heading is, by item, and what it says.
        let mut headings: Vec<(usize, String)> = Vec::new();
        let mut group: Option<&str> = None;
        let selected = self.rows[self.at].min(shown.len() - 1);
        let mut selected_item = 0;
        for (i, &r) in shown.iter().enumerate() {
            let row = &section.rows[r];
            if row.group.as_deref() != group {
                if !items.is_empty() {
                    items.push(ListItem::new(""));
                    index.push(None);
                }
                group = row.group.as_deref();
                if let Some(heading) = group {
                    headings.push((items.len(), heading.to_uppercase()));
                    items.push(ListItem::new(Span::styled(
                        heading.to_uppercase(),
                        self.look.dim(),
                    )));
                    index.push(None);
                }
            }
            if i == selected {
                selected_item = items.len();
            }
            items.push(ListItem::new(self.row_line(
                section,
                row,
                i == selected,
                (name_width, badge_width),
                usize::from(list.width),
            )));
            index.push(Some(i));
        }

        // The window onto the items: the least scrolling that shows the
        // cursor. Past the heading of the cursor's group, that heading is
        // pinned to the top, where it says what the rows under it are.
        let total = items.len();
        let room = usize::from(list.height);
        let offset_for = |room: usize| selected_item.saturating_sub(room.saturating_sub(1));
        let mut offset = offset_for(room);
        let heading = headings.iter().rev().find(|(at, _)| *at <= selected_item);
        let pinned = match heading {
            Some((at, text)) if total > room && room > 1 && *at < offset => {
                offset = offset_for(room - 1);
                (*at < offset).then(|| text.clone())
            }
            _ => None,
        };
        let mut rows_area = list;
        if let Some(text) = &pinned {
            let [top, rest] =
                Layout::vertical([Constraint::Length(1), Constraint::Fill(1)]).areas(list);
            f.render_widget(
                Paragraph::new(Span::styled(text.clone(), self.look.dim())),
                top,
            );
            rows_area = rest;
        } else if total <= room {
            offset = 0;
        }
        let mut state = ListState::default()
            .with_offset(offset)
            .with_selected(Some(selected_item));
        f.render_stateful_widget(
            List::new(items).highlight_style(self.look.selected()),
            rows_area,
            &mut state,
        );
        let offset = state.offset();

        // More than fits: a thumb at the list's right edge says how much, and
        // where in it you are.
        if total > room && room > 0 {
            let edge = Rect {
                x: list.right(),
                width: 1,
                ..list
            };
            let thumb = (room * room / total).max(1);
            let at = (offset * room / total).min(room - thumb);
            for y in 0..room {
                let symbol = if (at..at + thumb).contains(&y) {
                    "┃"
                } else {
                    " "
                };
                f.buffer_mut()[(edge.x, edge.y + y as u16)]
                    .set_symbol(symbol)
                    .set_style(self.look.dim());
            }
        }

        let mut hit = self.hit.borrow_mut();
        hit.list = rows_area;
        hit.lines = index
            .into_iter()
            .skip(offset)
            .take(usize::from(rows_area.height))
            .collect();
    }

    fn row_line(
        &self,
        section: &Section,
        row: &Row,
        selected: bool,
        (name_width, badge_width): (usize, usize),
        width: usize,
    ) -> Line<'static> {
        let changed = self.changed.as_deref() == Some(row.name.as_str());
        // Column 0 is the row's state; where the cursor is, the bar says. What
        // is wrong with it outranks a change just made to it: `space` on a
        // skill with no SKILL.md answered with a green ✓ over the `!`.
        let mark = match (row.warning.is_some(), changed) {
            (true, _) => Span::styled("! ", self.look.tone(Tone::Warn)),
            (false, true) => Span::styled("✓ ", self.look.tone(Tone::Ok)),
            (false, false) => Span::raw("  "),
        };
        // Inside the bar nothing is dim: reversed, dim reads as a gap in it.
        let dim = match selected {
            true => Style::default(),
            false => self.look.dim(),
        };
        let name = Span::styled(
            pad(&ellipsized(&row.name, name_width), name_width),
            match selected {
                true => self.look.title(),
                false => Style::default(),
            },
        );
        // Whether new repos get it, where that is a question on this page.
        let seeds = match (row.seeded, section.rows.iter().any(|r| r.seeded.is_some())) {
            (Some(true), _) => Some(Span::styled("● ", self.look.accent())),
            (Some(false), _) => Some(Span::styled("○ ", dim)),
            (None, true) => Some(Span::raw("  ")),
            (None, false) => None,
        };
        let mut spans = vec![mark];
        spans.extend(seeds);
        spans.extend([name, Span::raw("  ")]);
        if badge_width > 0 {
            let badge = row.badge.as_deref().unwrap_or_default();
            spans.push(Span::styled(
                pad(&ellipsized(badge, badge_width - 1), badge_width),
                dim,
            ));
        }
        let used: usize = spans.iter().map(Span::width).sum();
        let room = width.saturating_sub(used);
        // A row is one line: a warning's first, and the rest in the detail.
        let warning = row
            .warning
            .as_deref()
            .map(|w| w.lines().next().unwrap_or_default());
        let (text, style) = match (warning, &row.value, section.page) {
            (Some(w), _, Page::Entries(_)) => (w.to_string(), self.look.tone(Tone::Warn)),
            (_, Some(v), _) => match warning {
                Some(w) => (format!("{v}  ! {w}"), self.look.tone(Tone::Warn)),
                None => (v.clone(), Style::default()),
            },
            (_, None, Page::Defaults) => (
                // Dim says it is not yours, and survives `NO_COLOR`.
                match &row.fallback {
                    Some(f) => f.clone(),
                    None => "not set".to_string(),
                },
                dim,
            ),
            (_, None, _) => (row.summary.clone().unwrap_or_default(), dim),
        };
        spans.push(Span::styled(ellipsized(&text, room), style));
        // Inside the bar, one tone: reversed, every colour along it was a
        // stripe — green, cyan, yellow — and the glyphs already say what each
        // colour did, as they do under `NO_COLOR`.
        if selected {
            for span in &mut spans {
                span.style = span.style.fg(Color::Reset).bg(Color::Reset);
            }
        }
        Line::from(spans)
    }

    /// What the selected row is: its name and what it says about itself.
    fn detail_text(&self) -> Text<'static> {
        match (self.section(), self.row()) {
            (Some(section), Some(row)) => self.detail_for(section, row),
            // Nothing on the page: what would be, where it lives, how to get
            // one — rather than a blank pane.
            (Some(section), None) if section.problem.is_none() => Text::from(
                section
                    .about
                    .iter()
                    .map(|l| Line::raw(l.clone()))
                    .collect::<Vec<_>>(),
            ),
            _ => Text::default(),
        }
    }

    /// What one row is, as the detail draws it.
    fn detail_for(&self, section: &Section, row: &Row) -> Text<'static> {
        let title = match section.page.capability() {
            Some(capability) => format!("{capability}/{}", row.name),
            None => row.name.clone(),
        };
        let mut lines = vec![
            Line::from(vec![
                Span::styled(title, self.look.title()),
                Span::styled(
                    row.badge
                        .clone()
                        .map(|b| format!("  {b}"))
                        .unwrap_or_default(),
                    self.look.dim(),
                ),
            ]),
            Line::default(),
        ];
        if let Some(w) = &row.warning {
            for (i, line) in w.lines().enumerate() {
                lines.push(Line::from(Span::styled(
                    format!("{}{line}", if i == 0 { "! " } else { "  " }),
                    self.look.tone(Tone::Warn),
                )));
            }
        }
        lines.extend(row.about.iter().map(|l| Line::raw(l.clone())));
        if let Some(on) = row.seeded {
            lines.push(Line::from(vec![
                Span::styled("new repos  ", self.look.dim()),
                Span::raw(match on {
                    true => "get it",
                    false => "do not get it",
                }),
                Span::styled("  — space changes it", self.look.dim()),
            ]));
        }
        if section.page == Page::Defaults {
            lines.push(Line::default());
            lines.push(Line::from(match (&row.value, &row.fallback) {
                (Some(v), _) => vec![
                    Span::styled("now     ", self.look.dim()),
                    Span::styled(v.clone(), self.look.title()),
                ],
                (None, Some(f)) => vec![
                    Span::styled("now     ", self.look.dim()),
                    Span::raw(f.clone()),
                    Span::styled(" · default", self.look.dim()),
                ],
                (None, None) => vec![Span::styled("unset", self.look.dim())],
            }));
            if let Edit::Choice(options) = &row.edit {
                if !options.is_empty() {
                    lines.push(Line::from(vec![
                        Span::styled("one of  ", self.look.dim()),
                        Span::raw(options.join(" · ")),
                    ]));
                }
            }
            if let Edit::Text { hint } = &row.edit {
                lines.push(Line::from(Span::styled(hint.clone(), self.look.dim())));
            }
        }
        // The keys are the footer's, which follows the row: a second list of
        // them here spent two rows of a stacked screen saying it again.
        Text::from(lines)
    }

    fn draw_detail(&self, f: &mut Frame, area: Rect) {
        f.render_widget(
            Paragraph::new(self.detail_text()).wrap(Wrap { trim: false }),
            area,
        );
    }

    /// What the last command said: a mark for how it went and its first line,
    /// with the line it was on the right; and the rest of what it said under.
    fn status_text(&self, width: u16) -> Option<Text<'static>> {
        let status = self.status.as_ref()?;
        let (first, rest) = status.lines.split_first()?;
        let fade = |style: Style| match status.fresh {
            true => style,
            false => self.look.dim(),
        };
        let glyph = match status.tone {
            Tone::Ok => "✓ ",
            Tone::Warn => "! ",
            Tone::Bad => "✗ ",
            Tone::Info => "· ",
        };
        // Wrapped here rather than by the paragraph, so a long line goes on
        // under its own text and not under the mark.
        let room = usize::from(width).saturating_sub(3);
        let mut opening = wrapped(&first.1, room).into_iter();
        let mut head = vec![
            Span::styled(format!(" {glyph}"), fade(self.look.tone(status.tone))),
            Span::styled(
                opening.next().unwrap_or_default(),
                fade(self.look.tone(first.0)),
            ),
        ];
        // The command beside the first line where it fits, and after the last
        // where it does not: between them it splits a sentence from the line
        // it introduces.
        let mut last = None;
        if let Some(said) = &status.said {
            let said = format!("$ omh {said} ");
            let said_width = Span::raw(said.as_str()).width();
            let used: usize = head.iter().map(Span::width).sum();
            let room = usize::from(width).saturating_sub(used);
            match said_width + 2 <= room {
                true => {
                    head.push(Span::raw(" ".repeat(room - said_width)));
                    head.push(Span::styled(said, self.look.dim()));
                }
                false => {
                    last = Some(Line::from(Span::styled(
                        format!("   {said}"),
                        self.look.dim(),
                    )))
                }
            }
        }
        let mut lines = vec![Line::from(head)];
        let more = opening.map(|l| (first.0, l)).chain(
            rest.iter()
                .flat_map(|(tone, line)| wrapped(line, room).into_iter().map(move |l| (*tone, l))),
        );
        for (tone, line) in more {
            lines.push(Line::from(Span::styled(
                format!("   {line}"),
                fade(self.look.tone(tone)),
            )));
        }
        lines.extend(last);
        // Read already, it is one line: what it was about, not all of it again.
        if !status.fresh {
            lines.truncate(1);
        }
        Some(Text::from(lines))
    }

    /// The keys that act here, as many as fit, with `?` and `q` always there:
    /// a popup's own while one is open, and otherwise the row's first, then the
    /// page's, then moving — the order they are dropped in from the right.
    fn keys(&self, width: u16) -> Line<'static> {
        let page: Vec<(&str, &str)> = match &self.modal {
            Some(Modal::Input(_)) => vec![("↵", "done"), ("esc", "cancel"), ("ctrl-u", "clear")],
            Some(Modal::Pick(_)) => {
                vec![
                    ("↑↓", "move"),
                    ("↵", "pick"),
                    ("letters", "go to what they spell"),
                    ("esc", "close"),
                ]
            }
            Some(Modal::Confirm { .. }) => vec![("y", "remove"), ("any other key", "keeps it")],
            Some(Modal::Form(_)) => {
                vec![
                    ("⇥", "next field"),
                    ("↵", "next, then add"),
                    ("esc", "cancel"),
                ]
            }
            Some(Modal::Help { .. }) => vec![("j k", "scroll"), ("esc", "close")],
            None if self.filter.as_ref().is_some_and(|f| f.typing) => {
                vec![("↵", "keep the filter"), ("esc", "drop it"), ("↑↓", "move")]
            }
            None => {
                let mut keys = self.page_keys();
                if self.undo.is_some() {
                    keys.insert(0, ("u", "undoes it"));
                }
                keys
            }
        };
        let right = self.chips(&[("?", "help"), ("q", "quit")]);
        let room = usize::from(width).saturating_sub(right.width() + 2);
        let mut take = page.len();
        let mut left = self.chips(&page[..take]);
        while take > 0 && left.width() + 1 > room {
            take -= 1;
            left = self.chips(&page[..take]);
        }
        let gap = usize::from(width).saturating_sub(left.width() + right.width() + 1);
        let mut spans = vec![Span::raw(" ")];
        spans.extend(left.spans);
        spans.push(Span::raw(" ".repeat(gap)));
        spans.extend(right.spans);
        Line::from(spans)
    }

    /// What acts on this page and this row: a key that would only be told no
    /// is not offered — `↵ set` on a table, `d` on what is unset or is omh's,
    /// `i` where import is refused.
    fn page_keys(&self) -> Vec<(&'static str, &'static str)> {
        let mut keys = Vec::new();
        if let Some(section) = self.section() {
            let row = self.row();
            let importable = matches!(&section.import, Import::From(h) if !h.is_empty());
            match section.page {
                Page::Defaults => {
                    if row.is_some_and(|r| r.meant.is_some()) {
                        keys.push(("↵", "move it to the key it meant"));
                    } else if let Some(row) = row.filter(|r| r.edit != Edit::None) {
                        keys.push(("↵", "set"));
                        if row.value.is_some() {
                            keys.push(("d", "unset"));
                        }
                    }
                    keys.push(("e", "edit the file"));
                }
                Page::Servers => {
                    if row.is_some_and(|r| !r.fixed) {
                        keys.push(("d", "remove"));
                    }
                    if row.is_some_and(|r| r.toggle.is_some()) {
                        keys.push(("space", "new repos"));
                    }
                    // An unread file takes no add: the form filled, then failed.
                    if section.problem.is_none() {
                        keys.push(("a", "add"));
                        if importable {
                            keys.push(("i", "import"));
                        }
                    }
                    keys.push(("e", "edit the file"));
                }
                Page::Entries(_) => {
                    if let Some(row) = row {
                        keys.push(("↵", "edit"));
                        if !row.fixed {
                            keys.push(("d", "remove"));
                        }
                        if row.toggle.is_some() {
                            keys.push(("space", "new repos"));
                        }
                    }
                    keys.push(("a", "new"));
                    if importable {
                        keys.push(("i", "import"));
                    }
                }
            }
        }
        keys.extend([("/", "filter"), ("⇥", "page"), ("↑↓", "move")]);
        keys
    }

    fn chips(&self, keys: &[(&str, &str)]) -> Line<'static> {
        let mut spans = Vec::new();
        for (i, (key, does)) in keys.iter().enumerate() {
            if i > 0 {
                spans.push(Span::raw("  "));
            }
            spans.push(Span::styled(key.to_string(), self.look.key()));
            spans.push(Span::styled(format!(" {does}"), self.look.dim()));
        }
        Line::from(spans)
    }

    /// A box over the screen, as tall as what it says.
    ///
    /// One being typed into is one width from first key to last: sized by
    /// what it said, it grew and re-centred when a warning appeared, under
    /// the cursor. The rest are as wide as they say. Behind any box the screen
    /// is dim, and the one that removes is in the warning's colour.
    fn popup(&self, f: &mut Frame, title: &str, text: Text<'static>, kind: Popup) -> Rect {
        let screen = f.area();
        let widest = text.lines.iter().map(Line::width).max().unwrap_or(0) as u16;
        // Never wider than a comfortable line — a long warning wraps in the box
        // rather than stretching it across the screen — and narrower than its
        // own minimum, a screen gets a box as wide as it is: `clamp` with the
        // bounds the wrong way round is a panic, in raw mode.
        let most = screen.width.saturating_sub(2).clamp(1, 76);
        let width = match kind {
            Popup::Typing => 64.min(most),
            Popup::Showing | Popup::Removing => (widest + 4)
                .max(Span::raw(title).width() as u16 + 6)
                .clamp(36.min(most), most),
        };
        f.buffer_mut()
            .set_style(screen, Style::default().add_modifier(Modifier::DIM));
        let paragraph = Paragraph::new(text).wrap(Wrap { trim: false });
        let height = (paragraph.line_count(width.saturating_sub(4)) as u16 + 2)
            .min(screen.height.saturating_sub(2).max(1));
        let area = centered(screen, width, height);
        f.render_widget(Clear, area);
        f.render_widget(
            paragraph.block(
                Block::bordered()
                    .border_type(BorderType::Rounded)
                    .border_style(match kind {
                        Popup::Removing => self.look.tone(Tone::Warn),
                        Popup::Typing | Popup::Showing => self.look.accent(),
                    })
                    .title(Span::styled(format!(" {title} "), self.look.title()))
                    .padding(Padding::horizontal(1)),
            ),
            area,
        );
        area
    }

    fn draw_input(&self, f: &mut Frame, input: &Input) {
        let mut lines: Vec<Line> = input
            .about
            .iter()
            .map(|l| Line::from(Span::styled(l.clone(), self.look.dim())))
            .collect();
        if !lines.is_empty() {
            lines.push(Line::default());
        }
        let before = lines.clone();
        let typed: String = input.line.text.iter().collect();
        lines.push(Line::from(vec![
            Span::styled(input.label.clone(), self.look.title()),
            Span::raw(typed.clone()),
        ]));
        // What omh would say about the value as it stands, before it is
        // written rather than after.
        let warning = match (&input.error, &input.then, &self.check) {
            (Some(e), _, _) => Some((Tone::Bad, format!("✗ {e}"))),
            (None, Then::Value { key }, Some(check)) if !typed.is_empty() => {
                check(key, &typed).map(|w| (Tone::Warn, format!("! {w}")))
            }
            _ => None,
        };
        if let Some((tone, w)) = warning {
            lines.push(Line::from(Span::styled(clean(&w), self.look.tone(tone))));
        }
        lines.push(Line::default());
        lines.push(self.chips(&[("↵", "save"), ("esc", "cancel")]));
        let area = self.popup(f, &input.title, Text::from(lines), Popup::Typing);
        let above = Paragraph::new(Text::from(before))
            .wrap(Wrap { trim: false })
            .line_count(area.width.saturating_sub(4)) as u16;
        let typed_before: String = input.line.text[..input.line.cursor].iter().collect();
        let x = area.x
            + 2
            + Span::raw(input.label.as_str()).width() as u16
            + Span::raw(typed_before.as_str()).width() as u16;
        f.set_cursor_position((
            x.min(area.right().saturating_sub(2)),
            (area.y + 1 + above).min(area.bottom().saturating_sub(2)),
        ));
    }

    fn draw_confirm(&self, f: &mut Frame, said: &[String], warnings: &[String], act: &Act) {
        let title = match act {
            Act::Remove { capability, name } => format!("remove {capability}/{name}?"),
            _ => "sure?".to_string(),
        };
        let mut lines: Vec<Line> = said.iter().map(|l| Line::raw(l.clone())).collect();
        lines.extend(
            warnings
                .iter()
                .map(|l| Line::from(Span::styled(l.clone(), self.look.tone(Tone::Warn)))),
        );
        lines.push(Line::default());
        lines.push(Line::from(vec![
            Span::styled("y", self.look.key()),
            Span::styled(" remove · any other key keeps it", self.look.dim()),
        ]));
        self.popup(f, &title, Text::from(lines), Popup::Removing);
    }

    fn draw_pick(&self, f: &mut Frame, pick: &Pick) {
        let mut lines: Vec<Line> = pick
            .about
            .iter()
            .map(|l| Line::from(Span::styled(l.clone(), self.look.dim())))
            .collect();
        if !lines.is_empty() {
            lines.push(Line::default());
        }
        lines.extend(pick.options.iter().enumerate().map(|(i, option)| {
            let here = i == pick.at;
            let mut spans = vec![
                match here {
                    true => Span::styled("▸ ", self.look.accent()),
                    false => Span::raw("  "),
                },
                Span::styled(
                    option.clone(),
                    match here {
                        true => self.look.title(),
                        false => Style::default(),
                    },
                ),
            ];
            if pick.current == Some(i) {
                spans.push(Span::styled("  now", self.look.dim()));
            }
            if pick.fallback == Some(i) {
                spans.push(Span::styled("  default", self.look.dim()));
            }
            if let Some(note) = pick.notes.get(i) {
                let widest = pick
                    .options
                    .iter()
                    .map(|o| Span::raw(o.as_str()).width())
                    .max();
                let pad = widest.unwrap_or(0) - Span::raw(option.as_str()).width();
                spans.push(Span::styled(
                    format!("{}  {note}", " ".repeat(pad)),
                    self.look.dim(),
                ));
            }
            Line::from(spans)
        }));
        self.popup(f, &pick.title, Text::from(lines), Popup::Showing);
    }

    fn draw_form(&self, f: &mut Frame, form: &Form) {
        let mut lines = Vec::new();
        for (i, (label, field)) in FORM_LABELS.iter().zip(&form.fields).enumerate() {
            let here = i == form.at;
            let typed = drawn_field(i, &field.text);
            lines.push(Line::from(vec![
                Span::styled(if here { "▸ " } else { "  " }, self.look.accent()),
                Span::styled(
                    label.to_string(),
                    match here {
                        true => self.look.title(),
                        false => self.look.dim(),
                    },
                ),
                Span::raw(typed),
            ]));
        }
        if let Some(e) = &form.error {
            lines.push(Line::from(Span::styled(
                format!("✗ {e}"),
                self.look.tone(Tone::Bad),
            )));
        }
        if form.at == 1 {
            lines.push(Line::from(Span::styled(
                "  a token goes in env: arguments are kept as typed, and env values are not shown",
                self.look.dim(),
            )));
        }
        lines.push(Line::default());
        lines.push(self.chips(&[
            ("⇥", "next field"),
            ("↵", "next, then add"),
            ("esc", "cancel"),
        ]));
        let area = self.popup(f, "new server", Text::from(lines), Popup::Typing);
        let field = &form.fields[form.at];
        let typed_before = drawn_field(form.at, &field.text[..field.cursor]);
        let x = area.x
            + 4
            + Span::raw(FORM_LABELS[form.at]).width() as u16
            + Span::raw(typed_before.as_str()).width() as u16;
        f.set_cursor_position((
            x.min(area.right().saturating_sub(2)),
            (area.y + 1 + form.at as u16).min(area.bottom().saturating_sub(2)),
        ));
    }

    fn draw_help(&self, f: &mut Frame, scroll: usize) {
        let groups: &[(&str, &[(&str, &str)])] = &[
            (
                "move",
                &[
                    ("↑↓  j k", "move in the list"),
                    ("g G  home end", "to the top, to the bottom"),
                    ("⇥ ⇧⇥  → ←  l h", "change page"),
                    ("1–7", "go to a page"),
                    ("click", "select · again to open · wheel to scroll"),
                ],
            ),
            (
                "change",
                &[
                    ("↵", "set a default · open an entry in $EDITOR"),
                    ("e", "$EDITOR on the entry, or on the page's file"),
                    ("a", "a new entry, or a new server"),
                    ("d", "unset a default · remove an entry, after asking"),
                    ("i", "import from a harness"),
                    ("space", "whether the repos `omh init` sets up after get it"),
                    ("u", "undo the last change"),
                    ("/", "search every page — esc takes it off"),
                    ("ctrl-l", "read your catalogue again"),
                ],
            ),
            (
                "typing",
                &[
                    ("← →  home end", "move in the line"),
                    ("ctrl-a  ctrl-e", "to its start, to its end"),
                    ("ctrl-u", "clear what is before the cursor"),
                    (
                        "letters",
                        "in a list to pick from, go to the option they spell",
                    ),
                ],
            ),
            ("leave", &[("q  esc  ctrl-d  ctrl-c", "leave")]),
        ];
        let width = groups
            .iter()
            .flat_map(|(_, rows)| rows.iter())
            .map(|(k, _)| Span::raw(*k).width())
            .max()
            .unwrap_or(0);
        // What a description has beside its key, in the widest box this
        // screen gets: one too long goes on under itself, not under the key.
        let most = usize::from(f.area().width.saturating_sub(2).clamp(1, 76));
        let room = most.saturating_sub(4 + width + 2).max(12);
        let mut lines: Vec<Line> = Vec::new();
        for (i, (heading, rows)) in groups.iter().enumerate() {
            if i > 0 {
                lines.push(Line::default());
            }
            lines.push(Line::from(Span::styled(
                heading.to_uppercase(),
                self.look.dim(),
            )));
            for (key, does) in rows.iter() {
                for (j, part) in wrapped(does, room).into_iter().enumerate() {
                    lines.push(Line::from(vec![
                        Span::styled(pad(if j == 0 { key } else { "" }, width), self.look.key()),
                        Span::raw("  "),
                        Span::raw(part),
                    ]));
                }
            }
        }
        lines.push(Line::default());
        for part in wrapped(
            "every change is a command, shown once it runs · esc closes this",
            most.saturating_sub(4),
        ) {
            lines.push(Line::from(Span::styled(part, self.look.dim())));
        }
        // What fits, from where it is scrolled to, and a line saying there is
        // more: cut at 80×24, it ended mid-list with nothing to say so.
        let room = usize::from(f.area().height.saturating_sub(4)).max(2);
        let end = lines.len().saturating_sub(room - 1);
        self.hit.borrow_mut().help_end = if lines.len() > room { end } else { 0 };
        if lines.len() > room {
            let at = scroll.min(end);
            let cue = match (at > 0, at < end) {
                (true, true) => "↑↓ more — j k scroll",
                (false, _) => "↓ more — j k scroll",
                (true, false) => "↑ more — k scrolls back",
            };
            lines = lines.into_iter().skip(at).take(room - 1).collect();
            lines.push(Line::from(Span::styled(cue, self.look.dim())));
        }
        self.popup(f, "keys", Text::from(lines), Popup::Showing);
    }
}

/// How wide the screen is, for the layout it gets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Width {
    /// Pages, list and detail side by side.
    Wide,
    /// Pages beside the list, the detail under it.
    Medium,
    /// No column for the pages: one line names the page.
    Narrow,
}

impl Width {
    fn of(columns: u16) -> Self {
        match columns {
            100.. => Width::Wide,
            70..=99 => Width::Medium,
            _ => Width::Narrow,
        }
    }
}

fn capitalised(title: &str) -> String {
    let mut chars = title.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

/// `text` cut to `width` columns, with `…` where it was cut.
fn ellipsized(text: &str, width: usize) -> String {
    if Span::raw(text).width() <= width {
        return text.to_string();
    }
    if width == 0 {
        return String::new();
    }
    let mut out = String::new();
    for c in text.chars() {
        let next = format!("{out}{c}");
        if Span::raw(next.as_str()).width() + 1 > width {
            break;
        }
        out = next;
    }
    out.push('…');
    out
}

/// `text` broken at its spaces into lines at most `width` wide, each keeping
/// the line's own indent; a word wider than that is a line of its own.
fn wrapped(text: &str, width: usize) -> Vec<String> {
    let indent: String = text.chars().take_while(|&c| c == ' ').collect();
    let mut out = Vec::new();
    let mut line = indent.clone();
    for word in text.split(' ').filter(|w| !w.is_empty()) {
        let used = Span::raw(line.as_str()).width();
        let started = used > indent.len();
        if started && used + 1 + Span::raw(word).width() > width {
            out.push(std::mem::replace(&mut line, indent.clone()));
        } else if started {
            line.push(' ');
        }
        line.push_str(word);
    }
    out.push(line);
    out
}

/// `text` padded with spaces to `width` columns.
fn pad(text: &str, width: usize) -> String {
    let have = Span::raw(text).width();
    format!("{text}{}", " ".repeat(width.saturating_sub(have)))
}

impl Typed {
    /// The editing keys a shell's line has, and no more.
    fn edit(&mut self, key: KeyEvent) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Char('a') if ctrl => self.cursor = 0,
            KeyCode::Char('e') if ctrl => self.cursor = self.text.len(),
            KeyCode::Char('u') if ctrl => {
                self.text.drain(..self.cursor);
                self.cursor = 0;
            }
            KeyCode::Char(c) if plain(key) => {
                self.text.insert(self.cursor, c);
                self.cursor += 1;
            }
            KeyCode::Backspace if self.cursor > 0 => {
                self.cursor -= 1;
                self.text.remove(self.cursor);
            }
            KeyCode::Delete if self.cursor < self.text.len() => {
                self.text.remove(self.cursor);
            }
            KeyCode::Left => self.cursor = self.cursor.saturating_sub(1),
            KeyCode::Right => self.cursor = (self.cursor + 1).min(self.text.len()),
            KeyCode::Home => self.cursor = 0,
            KeyCode::End => self.cursor = self.text.len(),
            _ => {}
        }
    }
}

/// A line split the way a shell splits a command: on whitespace, with quotes
/// holding a word together and a backslash taking the next character as it is.
///
/// What a person types for a server's command is what they would type in a
/// shell — `npx -y mcp-remote 'https://…'` — so that is how it is read. No
/// expansion of any kind: `$HOME` stays `$HOME`, which is what the server
/// will be handed.
pub fn words(line: &str) -> Result<Vec<String>, String> {
    let mut out = Vec::new();
    // `None` between words; `Some("")` once `''` has begun one.
    let mut word: Option<String> = None;
    let mut quote: Option<char> = None;
    let mut chars = line.chars();
    while let Some(c) = chars.next() {
        match (quote, c) {
            (Some(q), c) if c == q => quote = None,
            (Some('"'), '\\') | (None, '\\') => match chars.next() {
                Some(next) => word.get_or_insert_default().push(next),
                None => return Err("a `\\` with nothing after it".into()),
            },
            (Some(_), c) => word.get_or_insert_default().push(c),
            (None, '\'' | '"') => {
                quote = Some(c);
                word.get_or_insert_default();
            }
            (None, c) if c.is_whitespace() => out.extend(word.take()),
            (None, c) => word.get_or_insert_default().push(c),
        }
    }
    if let Some(q) = quote {
        return Err(format!("a {q} that is never closed"));
    }
    out.extend(word);
    Ok(out)
}

/// Text fit to draw: every control character becomes `�`.
///
/// **Nothing read is a command to the terminal.** What this screen draws is
/// mostly text somebody else wrote — a skill's description, a hook's `does`, a
/// harness's file that `i` just imported, a child's output — and ratatui
/// writes a cell's text to the terminal as it is. An `ESC` in a description
/// would be the terminal obeying the description: OSC 52 writing the
/// clipboard, a cleared screen, a rewritten line. Replaced at every way in,
/// so no draw has to remember.
fn clean(text: &str) -> String {
    text.chars()
        .map(|c| if c.is_control() { '\u{fffd}' } else { c })
        .collect()
}

/// [`clean`], line by line: a line break stays one, so an error that quotes a
/// line and points under it still lines up. Anything else — `\r` among them —
/// is `�`.
fn clean_lines(text: &str) -> String {
    text.split('\n').map(clean).collect::<Vec<_>>().join("\n")
}

fn cleaned(mut model: Model) -> Model {
    let all = |v: &[String]| v.iter().map(|l| clean(l)).collect::<Vec<_>>();
    model.root = clean(&model.root);
    for section in &mut model.sections {
        section.problem = section.problem.as_deref().map(clean_lines);
        section.import = match &section.import {
            Import::From(harnesses) => Import::From(
                harnesses
                    .iter()
                    .map(|h| Source {
                        harness: clean(&h.harness),
                        from: clean(&h.from),
                    })
                    .collect(),
            ),
            Import::Not(why) => Import::Not(clean(why)),
        };
        for row in &mut section.rows {
            // The name too, though an act is built from it: a name that had
            // to be cleaned is one no command will find, and a removal that
            // refuses is the safe way for that to end.
            row.name = clean(&row.name);
            for text in [
                &mut row.value,
                &mut row.fallback,
                &mut row.summary,
                &mut row.badge,
                &mut row.group,
            ] {
                *text = text.as_deref().map(clean);
            }
            row.warning = row.warning.as_deref().map(clean_lines);
            row.about = all(&row.about);
            row.edit = match &row.edit {
                Edit::None => Edit::None,
                Edit::Text { hint } => Edit::Text { hint: clean(hint) },
                Edit::Choice(options) => Edit::Choice(all(options)),
            };
        }
    }
    model
}

/// An env line as it is drawn: every name, and every value as `•`.
///
/// **Read the way [`words`] reads it**, rule for rule — quotes, `\\` escapes,
/// whitespace — because `words` is what cuts the line that is added. A mask
/// with rules of its own drew whatever the two disagreed on: `K=a\\ b` is one
/// value to `words`, and ended at the space for the mask. A character is drawn
/// until its word's content has taken its first `=`, which is where
/// `parse_env` splits it; every one after, to the end of the word, is `•` —
/// quotes and backslashes too. One character out for each in, so the cursor
/// lands where it is.
fn masked_env(text: &[char]) -> String {
    let mut out = String::with_capacity(text.len());
    let mut quote: Option<char> = None;
    // The current word's content has taken its `=`: what follows is a value.
    let mut value = false;
    let mut chars = text.iter().copied();
    let draw = |out: &mut String, c: char, value: &mut bool, content: bool| {
        out.push(if *value { '•' } else { c });
        if content && c == '=' {
            *value = true;
        }
    };
    while let Some(c) = chars.next() {
        match (quote, c) {
            (Some(q), c) if c == q => {
                draw(&mut out, c, &mut value, false);
                quote = None;
            }
            (Some('"'), '\\') | (None, '\\') => {
                draw(&mut out, c, &mut value, false);
                if let Some(next) = chars.next() {
                    draw(&mut out, next, &mut value, true);
                }
            }
            (Some(_), c) => draw(&mut out, c, &mut value, true),
            (None, '\'' | '"') => {
                draw(&mut out, c, &mut value, false);
                quote = Some(c);
            }
            (None, c) if c.is_whitespace() => {
                out.push(c);
                value = false;
            }
            (None, c) => draw(&mut out, c, &mut value, true),
        }
    }
    out
}

/// A form field as it is drawn: the env field masked, the others as typed.
fn drawn_field(at: usize, text: &[char]) -> String {
    match at {
        2 => masked_env(text),
        _ => text.iter().collect(),
    }
}

/// `KEY=VALUE` words, as `--env` takes them.
fn parse_env(words: &[String]) -> Result<Vec<(String, String)>, String> {
    words
        .iter()
        .map(|w| match w.split_once('=') {
            Some((k, v)) if !k.is_empty() => Ok((k.to_string(), v.to_string())),
            _ => Err(format!("`{w}` is not KEY=VALUE")),
        })
        .collect()
}

fn centered(area: Rect, width: u16, height: u16) -> Rect {
    let width = width.min(area.width);
    let height = height.min(area.height);
    Rect {
        x: area.x + (area.width.saturating_sub(width)) / 2,
        y: area.y + (area.height.saturating_sub(height)) / 2,
        width,
        height,
    }
}

/// What a box is for, which is how it is drawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Popup {
    /// Typed into: one width from the first key to the last.
    Typing,
    /// Read: as wide as what it says.
    Showing,
    /// Asking to remove: in the warning's colour.
    Removing,
}

/// The screen's styles.
///
/// **One accent, and dim rather than grey.** Grey is a colour, and dark grey
/// on a dark theme — Solarized, most of them — is close to invisible; the
/// terminal's own *dim* follows its theme. **No colour means no colour**:
/// under `NO_COLOR` or `--color never` nothing is coloured, but bold and dim
/// stay, which are not colours, and the selection is reversed video — a list
/// nobody can see the cursor in is not a list.
#[derive(Debug, Clone, Copy)]
struct Look {
    color: bool,
}

impl Look {
    fn colored(&self, color: Color) -> Style {
        match self.color {
            true => Style::default().fg(color),
            false => Style::default(),
        }
    }

    fn title(&self) -> Style {
        Style::default().add_modifier(Modifier::BOLD)
    }

    fn dim(&self) -> Style {
        Style::default().add_modifier(Modifier::DIM)
    }

    fn accent(&self) -> Style {
        self.colored(Color::Cyan)
    }

    /// A key in a list of keys.
    fn key(&self) -> Style {
        self.accent().add_modifier(Modifier::BOLD)
    }

    /// The cursor: a bar across the row, the same with colour as without —
    /// reversed reads on a light theme as on a dark one.
    fn selected(&self) -> Style {
        Style::default().add_modifier(Modifier::REVERSED)
    }

    fn tone(&self, tone: Tone) -> Style {
        match tone {
            Tone::Ok => self.colored(Color::Green),
            Tone::Warn => self.colored(Color::Yellow),
            Tone::Bad => self.colored(Color::Red),
            Tone::Info => Style::default(),
        }
    }
}

#[cfg(test)]
mod tests;
