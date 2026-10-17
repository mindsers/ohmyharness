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
//! drawn frame come out.

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Clear, List, ListItem, ListState, Paragraph, Wrap};
use ratatui::Frame;

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
    /// The capability a command names it by, or `None` for your defaults.
    pub capability: Option<&'static str>,
    pub rows: Vec<Row>,
    /// The read that failed, when one did. **Could not look is not none**: a
    /// section omh could not read says so, rather than drawing an empty list
    /// that reads as "you have no skills".
    pub problem: Option<String>,
    /// The harnesses `i` can import this kind from.
    pub importers: Vec<String>,
    /// Why `i` does nothing here, when it does nothing.
    pub no_import: Option<String>,
}

/// One default, one server, or one entry.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Row {
    pub name: String,
    /// Shown beside the name: a default's value, a server's command.
    pub value: Option<String>,
    /// Who it is: `omh's`, `shipped`, `nothing reads this`.
    pub note: Option<String>,
    /// What it is, below the list when it is selected.
    pub about: Vec<String>,
    /// Whether `enter` may set it. A table in your defaults file is not a value.
    pub settable: bool,
}

/// What a key asked for: one command line omh accepts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Act {
    Set {
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
}

/// How a status line reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    Ok,
    Warn,
    Bad,
    Info,
}

pub struct App {
    model: Model,
    at: usize,
    rows: Vec<usize>,
    modal: Option<Modal>,
    status: Vec<(Tone, String)>,
    /// The command line the last act was, so what the screen did is
    /// something you can read, and type next time.
    said: Option<String>,
    look: Look,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Modal {
    Input(Input),
    Confirm {
        lines: Vec<String>,
        act: Act,
    },
    Pick {
        capability: String,
        harnesses: Vec<String>,
        at: usize,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Input {
    title: String,
    label: String,
    text: Vec<char>,
    cursor: usize,
    error: Option<String>,
    then: Then,
}

/// What a submitted line becomes.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Then {
    Value {
        key: String,
    },
    NewEntry {
        capability: String,
    },
    ServerName,
    ServerCommand {
        name: String,
    },
    ServerEnv {
        name: String,
        command: String,
        args: Vec<String>,
    },
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
            status: Vec::new(),
            said: None,
            look: Look { color },
        }
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
        for (i, section) in self.model.sections.iter().enumerate() {
            let named = was
                .get(i)
                .cloned()
                .flatten()
                .and_then(|name| section.rows.iter().position(|r| r.name == name));
            self.rows[i] = named.unwrap_or(self.rows[i].min(section.rows.len().saturating_sub(1)));
        }
        self.at = self.at.min(self.model.sections.len().saturating_sub(1));
    }

    /// What a command said, in its own words: its answer, then what it
    /// wrote to stderr — warnings when it worked, the refusal when it did not.
    pub fn report(&mut self, said: String, ok: bool, stdout: &str, stderr: &str) {
        self.said = Some(clean(&said));
        self.status = stdout
            .lines()
            .filter(|l| !l.trim().is_empty())
            .map(|l| (if ok { Tone::Ok } else { Tone::Info }, clean(l)))
            .chain(
                stderr
                    .lines()
                    .filter(|l| !l.trim().is_empty())
                    .map(|l| (if ok { Tone::Warn } else { Tone::Bad }, clean(l))),
            )
            .collect();
        if self.status.is_empty() {
            self.status
                .push((if ok { Tone::Ok } else { Tone::Bad }, "done".to_string()));
        }
    }

    /// One line of the screen's own, for something no command was run for.
    pub fn note(&mut self, line: impl Into<String>) {
        self.said = None;
        self.status = vec![(Tone::Info, clean(&line.into()))];
    }

    /// Ask before running `act`, showing what its preview said it would do.
    pub fn confirm(&mut self, lines: Vec<String>, act: Act) {
        let lines = lines.iter().map(|l| clean(l)).collect();
        self.modal = Some(Modal::Confirm { lines, act });
    }

    pub fn key(&mut self, key: KeyEvent) -> Next {
        if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
            return Next::Quit;
        }
        match self.modal.take() {
            Some(modal) => self.key_in_modal(modal, key),
            None => self.key_on_page(key),
        }
    }

    fn key_on_page(&mut self, key: KeyEvent) -> Next {
        let count = self.model.sections.len();
        let rows = self.section().map_or(0, |s| s.rows.len());
        match key.code {
            KeyCode::Char('q') | KeyCode::Esc => return Next::Quit,
            KeyCode::Tab | KeyCode::Right | KeyCode::Char('l') if count > 0 => {
                self.at = (self.at + 1) % count;
            }
            KeyCode::BackTab | KeyCode::Left | KeyCode::Char('h') if count > 0 => {
                self.at = (self.at + count - 1) % count;
            }
            KeyCode::Char(c @ '1'..='9') => {
                let n = c as usize - '1' as usize;
                if n < count {
                    self.at = n;
                }
            }
            KeyCode::Down | KeyCode::Char('j') if rows > 0 => {
                self.rows[self.at] = (self.rows[self.at] + 1).min(rows - 1);
            }
            KeyCode::Up | KeyCode::Char('k') if rows > 0 => {
                self.rows[self.at] = self.rows[self.at].saturating_sub(1);
            }
            KeyCode::Home | KeyCode::Char('g') if rows > 0 => self.rows[self.at] = 0,
            KeyCode::End | KeyCode::Char('G') if rows > 0 => self.rows[self.at] = rows - 1,
            KeyCode::Enter => return self.enter(),
            KeyCode::Char('e') => return self.edit(),
            KeyCode::Char('a') => self.add(),
            KeyCode::Char('d') | KeyCode::Delete => return self.remove(),
            KeyCode::Char('i') => self.import(),
            _ => {}
        }
        Next::Stay
    }

    fn enter(&mut self) -> Next {
        let Some(section) = self.section() else {
            return Next::Stay;
        };
        match section.capability {
            None => {
                let Some(row) = self.row() else {
                    return Next::Stay;
                };
                if !row.settable {
                    let line = format!("{} is a table — e opens the file it is in", row.name);
                    self.note(line);
                    return Next::Stay;
                }
                let current: Vec<char> = row.value.clone().unwrap_or_default().chars().collect();
                self.modal = Some(Modal::Input(Input {
                    title: "set a default".into(),
                    label: format!("{} = ", row.name),
                    cursor: current.len(),
                    text: current,
                    error: None,
                    then: Then::Value {
                        key: row.name.clone(),
                    },
                }));
                Next::Stay
            }
            Some("mcp") => Next::Stay,
            Some(_) => self.edit(),
        }
    }

    fn edit(&mut self) -> Next {
        let Some(section) = self.section() else {
            return Next::Stay;
        };
        match section.capability {
            None => Next::Run(Act::EditDefaults),
            // A server is a record in one file, so the file is what opens.
            Some("mcp") => Next::Run(Act::Edit {
                capability: "mcp".into(),
                name: None,
            }),
            Some(capability) => Next::Run(Act::Edit {
                capability: capability.into(),
                name: self.row().map(|r| r.name.clone()),
            }),
        }
    }

    fn add(&mut self) {
        let Some(section) = self.section() else {
            return;
        };
        match section.capability {
            None => self.note("every key omh reads is listed — enter sets one"),
            Some("mcp") => {
                self.modal = Some(Modal::Input(Input::new(
                    "new server",
                    "name: ",
                    Then::ServerName,
                )));
            }
            Some(capability) => {
                let title = format!("new entry in {}", section.title);
                self.modal = Some(Modal::Input(Input::new(
                    &title,
                    "name: ",
                    Then::NewEntry {
                        capability: capability.into(),
                    },
                )));
            }
        }
    }

    fn remove(&mut self) -> Next {
        let (Some(section), Some(row)) = (self.section(), self.row()) else {
            return Next::Stay;
        };
        match section.capability {
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
        let Some(capability) = section.capability else {
            self.note("your defaults are typed, not imported");
            return;
        };
        if section.importers.is_empty() {
            let line = section.no_import.clone().unwrap_or_else(|| {
                format!(
                    "no installed harness keeps {} where omh can read them",
                    section.title
                )
            });
            self.note(line);
            return;
        }
        self.modal = Some(Modal::Pick {
            capability: capability.into(),
            harnesses: section.importers.clone(),
            at: 0,
        });
    }

    fn key_in_modal(&mut self, modal: Modal, key: KeyEvent) -> Next {
        match modal {
            Modal::Confirm { act, .. } => match key.code {
                KeyCode::Char('y') | KeyCode::Char('Y') => Next::Run(act),
                // Anything else keeps it, the rule `ask::confirm` follows: one
                // way to say yes, and everything else is not it.
                _ => {
                    self.note("kept");
                    Next::Stay
                }
            },
            Modal::Pick {
                capability,
                harnesses,
                at,
            } => match key.code {
                KeyCode::Down | KeyCode::Char('j') => {
                    let at = (at + 1).min(harnesses.len().saturating_sub(1));
                    self.modal = Some(Modal::Pick {
                        capability,
                        harnesses,
                        at,
                    });
                    Next::Stay
                }
                KeyCode::Up | KeyCode::Char('k') => {
                    self.modal = Some(Modal::Pick {
                        capability,
                        harnesses,
                        at: at.saturating_sub(1),
                    });
                    Next::Stay
                }
                KeyCode::Enter => match harnesses.get(at) {
                    Some(harness) => Next::Run(Act::Import {
                        capability,
                        harness: harness.clone(),
                    }),
                    None => Next::Stay,
                },
                _ => Next::Stay,
            },
            Modal::Input(mut input) => match key.code {
                KeyCode::Esc => Next::Stay,
                KeyCode::Enter => self.submit(input),
                _ => {
                    input.edit(key);
                    self.modal = Some(Modal::Input(input));
                    Next::Stay
                }
            },
        }
    }

    /// A line was entered. Either it is an act, the next question, or the
    /// same question again with what was wrong with the answer.
    fn submit(&mut self, mut input: Input) -> Next {
        let line: String = input.text.iter().collect();
        let refuse = |mut input: Input, error: String, app: &mut Self| {
            input.error = Some(error);
            app.modal = Some(Modal::Input(input));
            Next::Stay
        };
        match std::mem::replace(&mut input.then, Then::ServerName) {
            Then::Value { key } => Next::Run(Act::Set { key, value: line }),
            Then::NewEntry { capability } => {
                let name = line.trim().to_string();
                if name.is_empty() {
                    input.then = Then::NewEntry { capability };
                    return refuse(input, "a name, first".into(), self);
                }
                Next::Run(Act::Edit {
                    capability,
                    name: Some(name),
                })
            }
            Then::ServerName => {
                let name = line.trim().to_string();
                if name.is_empty() {
                    return refuse(input, "a name, first".into(), self);
                }
                self.modal = Some(Modal::Input(Input::new(
                    &format!("new server {name}"),
                    "command and arguments: ",
                    Then::ServerCommand { name },
                )));
                Next::Stay
            }
            Then::ServerCommand { name } => match words(&line) {
                Ok(words) if !words.is_empty() => {
                    let mut words = words.into_iter();
                    let command = words.next().unwrap_or_default();
                    self.modal = Some(Modal::Input(Input::new(
                        &format!("new server {name}"),
                        "env (KEY=VALUE …), or nothing: ",
                        Then::ServerEnv {
                            name,
                            command,
                            args: words.collect(),
                        },
                    )));
                    Next::Stay
                }
                Ok(_) => {
                    input.then = Then::ServerCommand { name };
                    refuse(input, "the program to run, first".into(), self)
                }
                Err(e) => {
                    input.then = Then::ServerCommand { name };
                    refuse(input, e, self)
                }
            },
            Then::ServerEnv {
                name,
                command,
                args,
            } => match words(&line).and_then(|w| env(&w)) {
                Ok(env) => Next::Run(Act::AddServer {
                    name,
                    command,
                    args,
                    env,
                }),
                Err(e) => {
                    input.then = Then::ServerEnv {
                        name,
                        command,
                        args,
                    };
                    refuse(input, e, self)
                }
            },
        }
    }

    fn section(&self) -> Option<&Section> {
        self.model.sections.get(self.at)
    }

    fn row(&self) -> Option<&Row> {
        self.row_in(self.at)
    }

    fn row_in(&self, section: usize) -> Option<&Row> {
        let s = self.model.sections.get(section)?;
        s.rows.get(*self.rows.get(section)?)
    }

    pub fn draw(&self, f: &mut Frame) {
        let [head, body, status, foot] = Layout::vertical([
            Constraint::Length(1),
            Constraint::Fill(1),
            Constraint::Length(5),
            Constraint::Length(1),
        ])
        .areas(f.area());

        f.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(" omh settings ", self.look.title()),
                Span::styled(format!(" {}", self.model.root), self.look.dim()),
            ])),
            head,
        );

        let [pages, page] =
            Layout::horizontal([Constraint::Length(20), Constraint::Fill(1)]).areas(body);
        self.draw_pages(f, pages);
        self.draw_page(f, page);
        self.draw_status(f, status);
        f.render_widget(
            Paragraph::new(Span::styled(self.hints(), self.look.dim())),
            foot,
        );

        match &self.modal {
            Some(Modal::Input(input)) => self.draw_input(f, input),
            Some(Modal::Confirm { lines, .. }) => self.draw_confirm(f, lines),
            Some(Modal::Pick { harnesses, at, .. }) => self.draw_pick(f, harnesses, *at),
            None => {}
        }
    }

    fn draw_pages(&self, f: &mut Frame, area: Rect) {
        let items: Vec<ListItem> = self
            .model
            .sections
            .iter()
            .enumerate()
            .map(|(i, s)| {
                let count = match s.capability {
                    None => s.rows.iter().filter(|r| r.value.is_some()).count(),
                    Some(_) => s.rows.len(),
                };
                let mark = if s.problem.is_some() {
                    "!".to_string()
                } else {
                    count.to_string()
                };
                ListItem::new(format!("{} {:<11}{mark:>3}", i + 1, s.title))
            })
            .collect();
        let mut state = ListState::default().with_selected(Some(self.at));
        f.render_stateful_widget(
            List::new(items)
                .block(Block::bordered())
                .highlight_style(self.look.selected()),
            area,
            &mut state,
        );
    }

    fn draw_page(&self, f: &mut Frame, area: Rect) {
        let Some(section) = self.section() else {
            return;
        };
        let [list, about] =
            Layout::vertical([Constraint::Fill(1), Constraint::Length(7)]).areas(area);

        let block = Block::bordered().title(format!(" {} ", section.title));
        // Above what was found, never instead of it: one entry that would not
        // read is no reason to hide the ones that did.
        let list = match &section.problem {
            Some(problem) => {
                let [said, rest] =
                    Layout::vertical([Constraint::Length(3), Constraint::Fill(1)]).areas(list);
                f.render_widget(
                    Paragraph::new(Span::styled(problem.clone(), self.look.bad()))
                        .wrap(Wrap { trim: false })
                        .block(Block::bordered().title(" could not look ")),
                    said,
                );
                if section.rows.is_empty() {
                    return;
                }
                rest
            }
            None => list,
        };
        if section.rows.is_empty() {
            let empty = match section.capability {
                Some(_) => format!(
                    "no {} in your catalogue — a adds one, i imports some",
                    section.title
                ),
                None => "nothing omh reads".to_string(),
            };
            f.render_widget(
                Paragraph::new(Span::styled(empty, self.look.dim())).block(block),
                list,
            );
        } else {
            let width = section
                .rows
                .iter()
                .map(|r| r.name.chars().count())
                .max()
                .unwrap_or(0);
            let items: Vec<ListItem> = section
                .rows
                .iter()
                .map(|r| {
                    let mut spans = vec![Span::raw(format!("{:<width$}  ", r.name))];
                    match &r.value {
                        Some(v) => spans.push(Span::styled(v.clone(), self.look.value())),
                        None if section.capability.is_none() => {
                            spans.push(Span::styled("(unset)", self.look.dim()));
                        }
                        None => {}
                    }
                    if let Some(note) = &r.note {
                        spans.push(Span::styled(format!("  {note}"), self.look.dim()));
                    }
                    ListItem::new(Line::from(spans))
                })
                .collect();
            let mut state = ListState::default().with_selected(Some(self.rows[self.at]));
            f.render_stateful_widget(
                List::new(items)
                    .block(block)
                    .highlight_style(self.look.selected()),
                list,
                &mut state,
            );
        }

        if let Some(row) = self.row() {
            let title = match section.capability {
                Some(capability) => format!(" {capability}/{} ", row.name),
                None => format!(" {} ", row.name),
            };
            let lines: Vec<Line> = row.about.iter().map(|l| Line::raw(l.clone())).collect();
            f.render_widget(
                Paragraph::new(Text::from(lines))
                    .wrap(Wrap { trim: false })
                    .block(Block::bordered().title(title)),
                about,
            );
        }
    }

    fn draw_status(&self, f: &mut Frame, area: Rect) {
        let mut lines = Vec::new();
        if let Some(said) = &self.said {
            lines.push(Line::from(Span::styled(
                format!("= omh {said}"),
                self.look.dim(),
            )));
        }
        for (tone, line) in &self.status {
            lines.push(Line::from(Span::styled(
                line.clone(),
                self.look.tone(*tone),
            )));
        }
        f.render_widget(
            Paragraph::new(Text::from(lines))
                .wrap(Wrap { trim: false })
                .block(Block::bordered()),
            area,
        );
    }

    fn hints(&self) -> String {
        let page = match self.section().and_then(|s| s.capability) {
            None => "enter set · d unset · e edit the file",
            Some("mcp") => "a add · d remove · i import · e edit the file",
            Some(_) => "enter/e edit · a new · d remove · i import",
        };
        format!(
            " ↑↓ move · ⇥ or 1-{} page · {page} · q quit",
            self.model.sections.len()
        )
    }

    fn draw_input(&self, f: &mut Frame, input: &Input) {
        let area = centered(f.area(), 72, 6);
        f.render_widget(Clear, area);
        let text: String = input.text.iter().collect();
        let mut lines = vec![Line::from(vec![
            Span::styled(input.label.clone(), self.look.title()),
            Span::raw(text),
        ])];
        if let Some(error) = &input.error {
            lines.push(Line::from(Span::styled(error.clone(), self.look.bad())));
        }
        lines.push(Line::from(Span::styled(
            "enter to go on · esc to leave it",
            self.look.dim(),
        )));
        f.render_widget(
            Paragraph::new(Text::from(lines))
                .wrap(Wrap { trim: false })
                .block(Block::bordered().title(format!(" {} ", input.title))),
            area,
        );
        let before: String = input.text[..input.cursor].iter().collect();
        let x = area.x
            + 1
            + Span::raw(input.label.clone()).width() as u16
            + Span::raw(before).width() as u16;
        f.set_cursor_position((x.min(area.right().saturating_sub(2)), area.y + 1));
    }

    fn draw_confirm(&self, f: &mut Frame, said: &[String]) {
        let area = centered(f.area(), 80, said.len() as u16 + 4);
        f.render_widget(Clear, area);
        let mut lines: Vec<Line> = said.iter().map(|l| Line::raw(l.clone())).collect();
        lines.push(Line::from(Span::styled(
            "y to go ahead · any other key keeps it",
            self.look.dim(),
        )));
        f.render_widget(
            Paragraph::new(Text::from(lines))
                .wrap(Wrap { trim: false })
                .block(Block::bordered().title(" sure? ")),
            area,
        );
    }

    fn draw_pick(&self, f: &mut Frame, harnesses: &[String], at: usize) {
        let area = centered(f.area(), 40, harnesses.len() as u16 + 3);
        f.render_widget(Clear, area);
        let items: Vec<ListItem> = harnesses.iter().map(|h| ListItem::new(h.clone())).collect();
        let mut state = ListState::default().with_selected(Some(at));
        f.render_stateful_widget(
            List::new(items)
                .block(Block::bordered().title(" import from "))
                .highlight_style(self.look.selected()),
            area,
            &mut state,
        );
    }
}

impl Input {
    fn new(title: &str, label: &str, then: Then) -> Self {
        Self {
            title: title.into(),
            label: label.into(),
            text: Vec::new(),
            cursor: 0,
            error: None,
            then,
        }
    }

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
            KeyCode::Char(c) if !ctrl => {
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
        self.error = None;
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
    let mut word = String::new();
    let mut in_word = false;
    let mut quote: Option<char> = None;
    let mut chars = line.chars();
    while let Some(c) = chars.next() {
        match (quote, c) {
            (Some(q), c) if c == q => quote = None,
            (Some('"'), '\\') | (None, '\\') => match chars.next() {
                Some(next) => {
                    word.push(next);
                    in_word = true;
                }
                None => return Err("a `\\` with nothing after it".into()),
            },
            (Some(_), c) => word.push(c),
            (None, '\'' | '"') => {
                quote = Some(c);
                in_word = true;
            }
            (None, c) if c.is_whitespace() => {
                if in_word {
                    out.push(std::mem::take(&mut word));
                    in_word = false;
                }
            }
            (None, c) => {
                word.push(c);
                in_word = true;
            }
        }
    }
    if let Some(q) = quote {
        return Err(format!("a {q} that is never closed"));
    }
    if in_word {
        out.push(word);
    }
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

fn cleaned(mut model: Model) -> Model {
    model.root = clean(&model.root);
    for section in &mut model.sections {
        section.problem = section.problem.as_deref().map(clean);
        section.no_import = section.no_import.as_deref().map(clean);
        section.importers = section.importers.iter().map(|h| clean(h)).collect();
        for row in &mut section.rows {
            // The name too, though an act is built from it: a name that had
            // to be cleaned is one no command will find, and a removal that
            // refuses is the safe way for that to end.
            row.name = clean(&row.name);
            row.value = row.value.as_deref().map(clean);
            row.note = row.note.as_deref().map(clean);
            row.about = row.about.iter().map(|l| clean(l)).collect();
        }
    }
    model
}

/// `KEY=VALUE` words, as `--env` takes them.
fn env(words: &[String]) -> Result<Vec<(String, String)>, String> {
    words
        .iter()
        .map(|w| match w.split_once('=') {
            Some((k, v)) if !k.is_empty() => Ok((k.to_string(), v.to_string())),
            _ => Err(format!("`{w}` is not KEY=VALUE")),
        })
        .collect()
}

fn centered(area: Rect, width: u16, height: u16) -> Rect {
    let width = width.min(area.width.saturating_sub(4));
    let height = height.min(area.height.saturating_sub(2));
    Rect {
        x: area.x + (area.width.saturating_sub(width)) / 2,
        y: area.y + (area.height.saturating_sub(height)) / 2,
        width,
        height,
    }
}

/// The screen's styles. **No colour means no colour**: under `NO_COLOR` or
/// `--color never` every style is plain except the selection, which is
/// reversed video — a list nobody can see the cursor in is not a list.
#[derive(Debug, Clone, Copy)]
struct Look {
    color: bool,
}

impl Look {
    fn paint(&self, style: Style) -> Style {
        if self.color {
            style
        } else {
            Style::default()
        }
    }

    fn title(&self) -> Style {
        self.paint(Style::default().add_modifier(Modifier::BOLD))
    }

    fn dim(&self) -> Style {
        self.paint(Style::default().fg(Color::DarkGray))
    }

    fn value(&self) -> Style {
        self.paint(Style::default().fg(Color::Cyan))
    }

    fn bad(&self) -> Style {
        self.paint(Style::default().fg(Color::Red))
    }

    fn selected(&self) -> Style {
        Style::default().add_modifier(Modifier::REVERSED)
    }

    fn tone(&self, tone: Tone) -> Style {
        match tone {
            Tone::Ok => self.paint(Style::default().fg(Color::Green)),
            Tone::Warn => self.paint(Style::default().fg(Color::Yellow)),
            Tone::Bad => self.bad(),
            Tone::Info => Style::default(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    fn press(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)
    }

    fn code(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn typed(app: &mut App, line: &str) -> Next {
        for c in line.chars() {
            assert_eq!(app.key(press(c)), Next::Stay, "typing {c:?}");
        }
        app.key(code(KeyCode::Enter))
    }

    fn model() -> Model {
        Model {
            root: "~/.omh".into(),
            sections: vec![
                Section {
                    title: "defaults",
                    capability: None,
                    rows: vec![
                        Row {
                            name: "idle_timeout".into(),
                            value: Some("30m".into()),
                            about: vec!["How long a session may sit unused.".into()],
                            settable: true,
                            ..Row::default()
                        },
                        Row {
                            name: "runtime".into(),
                            settable: true,
                            ..Row::default()
                        },
                        Row {
                            name: "[use]".into(),
                            value: Some("…".into()),
                            ..Row::default()
                        },
                    ],
                    problem: None,
                    importers: vec![],
                    no_import: None,
                },
                Section {
                    title: "skills",
                    capability: Some("skills"),
                    rows: vec![
                        Row {
                            name: "keep".into(),
                            ..Row::default()
                        },
                        Row {
                            name: "review-diff".into(),
                            ..Row::default()
                        },
                    ],
                    problem: None,
                    importers: vec!["claude".into(), "codex".into()],
                    no_import: None,
                },
                Section {
                    title: "servers",
                    capability: Some("mcp"),
                    rows: vec![],
                    problem: None,
                    importers: vec![],
                    no_import: None,
                },
                Section {
                    title: "hooks",
                    capability: Some("hooks"),
                    rows: vec![],
                    problem: Some("could not read ~/.omh/hooks: permission denied".into()),
                    importers: vec![],
                    no_import: Some("a hook lands in a repo".into()),
                },
            ],
        }
    }

    fn drawn(app: &App) -> String {
        let mut terminal = Terminal::new(TestBackend::new(120, 40)).unwrap();
        terminal.draw(|f| app.draw(f)).unwrap();
        let buffer = terminal.backend().buffer().clone();
        (0..buffer.area.height)
            .map(|y| {
                (0..buffer.area.width)
                    .map(|x| buffer[(x, y)].symbol().to_string())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// **No key writes.** Every key a person can press either moves, opens a
    /// question, or comes back as an act — so a walk over every key on every
    /// page that never answers `y` or submits a line must produce no `Run` of
    /// anything but an editor, and no `Preview` it was not asked for.
    #[test]
    fn a_key_never_writes_without_being_asked_to() {
        let keys: Vec<KeyEvent> = "jkhlg123456789aiqx"
            .chars()
            .map(press)
            .chain([
                code(KeyCode::Down),
                code(KeyCode::Up),
                code(KeyCode::Tab),
                code(KeyCode::BackTab),
                code(KeyCode::Esc),
            ])
            .collect();
        for page in 0..model().sections.len() {
            for key in &keys {
                let mut app = App::new(model(), true);
                app.at = page;
                match app.key(*key) {
                    Next::Stay | Next::Quit => {}
                    other => panic!("{key:?} on page {page} did {other:?} unasked"),
                }
            }
        }
    }

    #[test]
    fn enter_on_a_default_asks_for_its_value_starting_from_the_current_one() {
        let mut app = App::new(model(), true);
        assert_eq!(app.key(code(KeyCode::Enter)), Next::Stay);
        assert!(drawn(&app).contains("idle_timeout = 30m"));
        // Replace the value: clear the line, then type.
        app.key(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL));
        assert_eq!(app.key(code(KeyCode::End)), Next::Stay);
        assert_eq!(
            typed(&mut app, "45m"),
            Next::Run(Act::Set {
                key: "idle_timeout".into(),
                value: "45m".into()
            })
        );
    }

    #[test]
    fn esc_leaves_a_question_unanswered() {
        let mut app = App::new(model(), true);
        app.key(code(KeyCode::Enter));
        assert_eq!(app.key(code(KeyCode::Esc)), Next::Stay);
        assert_eq!(app.modal, None);
        // And the next esc leaves the screen, rather than being swallowed.
        assert_eq!(app.key(code(KeyCode::Esc)), Next::Quit);
    }

    #[test]
    fn a_table_in_your_defaults_is_not_set_from_a_line() {
        let mut app = App::new(model(), true);
        app.key(press('j'));
        app.key(press('j'));
        assert_eq!(app.key(code(KeyCode::Enter)), Next::Stay);
        assert_eq!(app.modal, None);
        assert!(drawn(&app).contains("[use] is a table"));
    }

    #[test]
    fn d_unsets_a_default_and_says_so_when_there_is_none() {
        let mut app = App::new(model(), true);
        assert_eq!(
            app.key(press('d')),
            Next::Run(Act::Unset {
                key: "idle_timeout".into()
            })
        );
        app.key(press('j'));
        assert_eq!(app.key(press('d')), Next::Stay);
        assert!(drawn(&app).contains("runtime is not set"));
    }

    /// Removal asks first, and asks in the preview's words; only `y` is yes.
    #[test]
    fn removing_previews_then_asks_and_only_y_goes_ahead() {
        let mut app = App::new(model(), true);
        app.key(press('2'));
        app.key(press('j'));
        let act = Act::Remove {
            capability: "skills".into(),
            name: "review-diff".into(),
        };
        assert_eq!(app.key(press('d')), Next::Preview(act.clone()));
        for no in ['n', 'Y'] {
            app.confirm(
                vec!["would remove ~/.omh/skills/review-diff".into()],
                act.clone(),
            );
            assert!(drawn(&app).contains("would remove ~/.omh/skills/review-diff"));
            if no == 'Y' {
                assert_eq!(app.key(press(no)), Next::Run(act.clone()));
            } else {
                assert_eq!(app.key(press(no)), Next::Stay);
                assert!(drawn(&app).contains("kept"));
            }
        }
        app.confirm(vec![], act.clone());
        assert_eq!(
            app.key(code(KeyCode::Enter)),
            Next::Stay,
            "enter is not yes"
        );
    }

    #[test]
    fn a_server_is_three_questions_and_its_command_is_split_like_a_shell() {
        let mut app = App::new(model(), true);
        app.key(press('3'));
        assert!(drawn(&app).contains("no servers in your catalogue"));
        app.key(press('a'));
        assert_eq!(typed(&mut app, "linear"), Next::Stay);
        assert!(drawn(&app).contains("command and arguments"));
        assert_eq!(
            typed(&mut app, "npx -y mcp-remote 'https://x/sse'"),
            Next::Stay
        );
        assert!(drawn(&app).contains("env"));
        assert_eq!(typed(&mut app, "nope"), Next::Stay, "not KEY=VALUE");
        assert!(drawn(&app).contains("`nope` is not KEY=VALUE"));
        app.key(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL));
        assert_eq!(
            typed(&mut app, "TOKEN=x"),
            Next::Run(Act::AddServer {
                name: "linear".into(),
                command: "npx".into(),
                args: vec!["-y".into(), "mcp-remote".into(), "https://x/sse".into()],
                env: vec![("TOKEN".into(), "x".into())],
            })
        );
    }

    #[test]
    fn a_new_entry_is_opened_in_the_editor_by_name() {
        let mut app = App::new(model(), true);
        app.key(press('2'));
        app.key(press('a'));
        assert_eq!(typed(&mut app, "  "), Next::Stay, "a blank name is not one");
        assert_eq!(
            typed(&mut app, "tdd"),
            Next::Run(Act::Edit {
                capability: "skills".into(),
                name: Some("tdd".into())
            })
        );
    }

    #[test]
    fn import_picks_from_the_harnesses_that_keep_this_kind() {
        let mut app = App::new(model(), true);
        app.key(press('2'));
        app.key(press('i'));
        app.key(press('j'));
        assert_eq!(
            app.key(code(KeyCode::Enter)),
            Next::Run(Act::Import {
                capability: "skills".into(),
                harness: "codex".into()
            })
        );
        app.key(press('4'));
        assert_eq!(app.key(press('i')), Next::Stay);
        assert!(drawn(&app).contains("a hook lands in a repo"));
    }

    /// A section omh could not read says so, rather than drawing an empty
    /// list that reads as "you have none" — and what it did read stays.
    #[test]
    fn a_section_that_could_not_be_read_says_so() {
        let mut app = App::new(model(), true);
        app.key(press('4'));
        let screen = drawn(&app);
        assert!(screen.contains("could not read ~/.omh/hooks"), "{screen}");
        assert!(!screen.contains("no hooks in your catalogue"), "{screen}");

        let mut partly = model();
        partly.sections[3].rows.push(Row {
            name: "rust-test".into(),
            ..Row::default()
        });
        app.reload(partly);
        let screen = drawn(&app);
        assert!(screen.contains("could not read ~/.omh/hooks"), "{screen}");
        assert!(screen.contains("rust-test"), "{screen}");
    }

    #[test]
    fn a_report_shows_the_command_and_both_streams() {
        let mut app = App::new(model(), true);
        app.report(
            "settings set idle_timeout 45m".into(),
            true,
            "wrote → ~/.omh/default.toml\n",
            "warning: something\n",
        );
        let screen = drawn(&app);
        assert!(screen.contains("= omh settings set idle_timeout 45m"));
        assert!(screen.contains("wrote → ~/.omh/default.toml"));
        assert!(screen.contains("warning: something"));
    }

    #[test]
    fn a_reload_keeps_the_selection_on_the_same_row() {
        let mut app = App::new(model(), true);
        app.key(press('2'));
        app.key(press('j'));
        let mut fresh = model();
        fresh.sections[1].rows.insert(
            0,
            Row {
                name: "aaa".into(),
                ..Row::default()
            },
        );
        app.reload(fresh);
        assert_eq!(app.row().map(|r| r.name.as_str()), Some("review-diff"));
        // And a row that went leaves the cursor in range.
        let mut fewer = model();
        fewer.sections[1].rows.truncate(1);
        app.reload(fewer);
        assert_eq!(app.row().map(|r| r.name.as_str()), Some("keep"));
    }

    /// **Nothing read is a command to the terminal.** A skill's description,
    /// a hook's `does` and a child's output are all text somebody else may have
    /// written — an imported entry among them — and an escape drawn raw is
    /// the terminal obeying it: a clipboard written through OSC 52, a line
    /// rewritten, a title changed. Every control character is drawn as `�`.
    #[test]
    fn nothing_read_reaches_the_terminal_as_a_control_sequence() {
        let evil = "a\x1b]52;c;ZXZpbA==\x07b\x1b[2Jc\rd\u{9b}31m";
        let mut model = model();
        model.sections[1].rows[0].name = evil.into();
        model.sections[1].rows[0].value = Some(evil.into());
        model.sections[1].rows[0].note = Some(evil.into());
        model.sections[1].rows[0].about = vec![evil.into()];
        let mut app = App::new(model, true);
        app.key(press('2'));
        app.report(evil.into(), false, evil, evil);
        let mut terminal = Terminal::new(TestBackend::new(120, 40)).unwrap();
        terminal.draw(|f| app.draw(f)).unwrap();
        let buffer = terminal.backend().buffer().clone();
        for cell in buffer.content() {
            assert!(
                !cell.symbol().chars().any(char::is_control),
                "a control character reached the screen: {:?}",
                cell.symbol()
            );
        }
        assert!(
            drawn(&app).contains("a�]52;c;ZXZpbA==�b"),
            "{}",
            drawn(&app)
        );

        app.confirm(vec![evil.into()], Act::EditDefaults);
        terminal.draw(|f| app.draw(f)).unwrap();
        for cell in terminal.backend().buffer().content() {
            assert!(!cell.symbol().chars().any(char::is_control));
        }
    }

    #[test]
    fn words_split_like_a_shell_and_expand_nothing() {
        let w = |s: &str| words(s).unwrap();
        assert_eq!(w("npx -y  pkg"), ["npx", "-y", "pkg"]);
        assert_eq!(w("a 'b c' \"d e\""), ["a", "b c", "d e"]);
        assert_eq!(w(r#"a\ b "c\"d" 'e\f'"#), ["a b", "c\"d", "e\\f"]);
        assert_eq!(w("'' x"), ["", "x"]);
        assert_eq!(w("$HOME"), ["$HOME"]);
        assert!(words("'open").is_err());
        assert!(words("trailing\\").is_err());
        assert_eq!(w("   "), Vec::<String>::new());
    }
}
