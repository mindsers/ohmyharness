//! `omh settings` at a terminal: the screen in [`crate::tui`], fed from
//! `~/.omh` and run against omh itself.
//!
//! **Every change is a command line, run as one.** What a key asks for comes
//! back from the screen as an [`Act`]; [`argv`] turns it into the line you
//! would have typed, and that line is run by this same binary as a child —
//! with its answer and its warnings shown on the screen, and the line itself
//! shown above them. So the screen has no write of its own to get wrong: the
//! guards, the refusals and the words are the command's, and there is no
//! second copy of any of them to drift.
//!
//! A child rather than a call for a reason that is not taste: every command
//! speaks through `out::Ctx`, which prints, and printing onto a screen the
//! terminal is drawing is a screen torn in half. A child's output is captured;
//! the screen decides where it goes.
//!
//! **One exception, `$EDITOR`**, which is run here through the same steps
//! `omh settings edit` takes — [`settings::edit_target`] and
//! [`settings::open_in_editor`] — rather than by a child. The editor has to
//! be handed the terminal, so a child's refusal would be printed onto it, and
//! the screen drawn back afterwards would cover the one line that said why.
//! Worked out here, the target is refused while the screen is still up to say
//! so.

use crate::adapter::{Adapter, Capability};
use crate::cmd::harvest::plural;
use crate::cmd::settings::{self, Edited};
use crate::profile::Paths;
use crate::tui::{Act, App, Edit, Import, Model, Next, Page, Row, Section};
use crate::{config, hook, key};
use anyhow::{Context, Result};
use ratatui::crossterm::event::{
    self, DisableMouseCapture, EnableMouseCapture, Event, KeyEventKind,
};

/// Whether this run should be a screen rather than the report.
///
/// Both ends a terminal: a screen drawn into a pipe is escape sequences in
/// somebody's file, and one waiting on a pipe for keys waits forever. `--json`
/// and `--dry-run` are a script's flags, so they get the report wherever they
/// are typed.
pub(crate) fn wanted(json: bool, dry_run: bool) -> bool {
    !json
        && !dry_run
        && std::io::IsTerminal::is_terminal(&std::io::stdin())
        && std::io::IsTerminal::is_terminal(&std::io::stdout())
}

pub(crate) fn run(paths: &Paths, color: bool, ctx: &crate::out::Ctx) -> Result<()> {
    let mut app = App::new(model(paths), color)
        .with_check(Box::new(refusal))
        .with_home(dirs::home_dir().map(|h| h.display().to_string()));
    let mut screen = Screen::open()?;
    let mut ran = Vec::new();
    let driven = drive(&mut app, &mut screen.terminal, paths, &mut ran);
    // The screen gone, what it ran is said where scrollback keeps it: it held
    // the terminal, so nothing it did was left behind — a session's commands,
    // to read back or paste into a script.
    drop(screen);
    if let Some(said) = what_ran(&ran) {
        ctx.say(&said);
    }
    driven
}

/// The lines [`ran_lines`] made, said once the screen closes — and, where one
/// held a value back, that it did. `None` for a screen that changed nothing.
fn what_ran(ran: &[(String, bool)]) -> Option<crate::report::Action> {
    if ran.is_empty() {
        return None;
    }
    let mut said = crate::report::Action::new("settings-screen-ran", "the screen ran:");
    for (line, _) in ran {
        said = said.note(line.clone());
    }
    if ran.iter().any(|(_, held)| *held) {
        said = said.note(
            "a server's env values and arguments are not shown — type them again to run \
             that line",
        );
    }
    Some(said)
}

/// What `omh settings set` would refuse a value for, asked as it is typed.
fn refusal(name: &str, value: &str) -> Option<String> {
    match key::describes(name) {
        Some(k) => key::quarrel(k, value),
        None => Some(format!("nothing in omh reads `{name}`")),
    }
}

/// `omh settings` off a terminal: your defaults, and your catalogue as the
/// screen reads it — each page's count, what new repos get, and every entry
/// that is wrong — so a broken entry is not visible only on a terminal.
pub(crate) fn show(paths: &Paths, ctx: &crate::out::Ctx) -> Result<()> {
    let mut said = settings::settings_report(paths)?;
    said.meant = said
        .unread
        .iter()
        .filter_map(|u| key::nearest(&u.key).map(|k| (u.key.clone(), k.to_string())))
        .collect();
    let model = model(paths);
    // Your defaults as the screen's first page says them: what it marks on
    // each, the features and their switch, what a table holds.
    if let Some(defaults) = model.sections.iter().find(|s| s.page == Page::Defaults) {
        let row = |name: &str| defaults.rows.iter().find(|r| r.name == name);
        for k in &mut said.known {
            k.warning = row(&k.key).and_then(|r| r.warning.clone());
        }
        for t in &mut said.tables {
            if let Some(r) = row(&t.key) {
                t.value = r.value.clone().unwrap_or_default();
                t.warning = r.warning.clone();
            }
        }
        said.features = defaults
            .rows
            .iter()
            .filter(|r| r.group.as_deref() == Some("features"))
            .map(|r| crate::report::FeatureSwitch {
                feature: r.name.clone(),
                on: r.value.as_deref().or(r.fallback.as_deref()) == Some("on"),
                set: r.value.is_some(),
            })
            .collect();
    }
    said.catalogue = model
        .sections
        .into_iter()
        .filter(|s| s.page != Page::Defaults)
        .map(|s| crate::report::CataloguePage {
            kind: s.title.to_string(),
            count: match (&s.problem, s.rows.is_empty()) {
                (Some(_), true) => None,
                _ => Some(s.rows.len()),
            },
            seeds: s.seeds,
            problem: s.problem,
            warnings: s
                .rows
                .into_iter()
                .filter_map(|r| {
                    r.warning
                        .map(|w| (r.name, w.lines().next().unwrap_or_default().to_string()))
                })
                .collect(),
        })
        .collect();
    ctx.say(&said);
    Ok(())
}

/// The terminal while the screen has it, given back however this ends.
///
/// A guard rather than a `restore()` after the loop, because the loop is not
/// the only way out: `try_init` turns raw mode on *before* it enters the
/// alternate screen and asks the terminal its size, so a failure in either
/// returned with raw mode on, and the shell somebody came back to needed
/// `reset`.
///
/// **And the mouse.** The screen takes clicks, and ratatui's `restore` — and
/// the panic hook `try_init` installs — knows nothing of mouse capture: left
/// on, every click and scroll in the shell after is an escape sequence typed
/// at the prompt. So capture is given back here, in `suspend` before
/// `$EDITOR`, and in a panic hook of this screen's own, before ratatui's.
struct Screen {
    terminal: ratatui::DefaultTerminal,
}

impl Screen {
    fn open() -> Result<Self> {
        let terminal = match ratatui::try_init() {
            Ok(terminal) => terminal,
            Err(e) => {
                ratatui::restore();
                return Err(e).context("opening the screen");
            }
        };
        let theirs = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            let _ = ratatui::crossterm::execute!(std::io::stdout(), DisableMouseCapture);
            theirs(info);
        }));
        // Built before the capture, so a capture that fails is given back by
        // the guard like everything else.
        let screen = Self { terminal };
        ratatui::crossterm::execute!(std::io::stdout(), EnableMouseCapture)
            .context("taking the mouse")?;
        Ok(screen)
    }
}

impl Drop for Screen {
    fn drop(&mut self) {
        let _ = ratatui::crossterm::execute!(std::io::stdout(), DisableMouseCapture);
        ratatui::restore();
    }
}

fn drive(
    app: &mut App,
    terminal: &mut ratatui::DefaultTerminal,
    paths: &Paths,
    ran: &mut Vec<(String, bool)>,
) -> Result<()> {
    let exe = std::env::current_exe().context("finding omh")?;
    loop {
        terminal
            .draw(|f| app.draw(f))
            .context("drawing the screen")?;
        // A resize, a focus change, a paste: none is a key, and the next draw
        // is the whole answer to each.
        let next = match event::read().context("reading the terminal")? {
            Event::Key(key) if key.kind == KeyEventKind::Press => app.key(key),
            Event::Mouse(mouse) => app.mouse(mouse),
            _ => continue,
        };
        match next {
            Next::Stay => {}
            Next::Quit => return Ok(()),
            Next::Reload => {
                app.reload(model(paths));
                terminal.clear().context("drawing the screen")?;
            }
            Next::Preview(act) => {
                let mut line = vec!["--dry-run".to_string()];
                line.extend(argv(&act));
                match omh(&exe, &line) {
                    Ok(out) if out.status.success() => {
                        app.confirm(lines(&out.stdout), lines(&out.stderr), act);
                    }
                    // Shown as the removal you would type: typed, it meets the
                    // same refusal, which comes before the question.
                    Ok(out) => report(app, &line, &out),
                    Err(e) => app.report(shown(&line), false, "", &format!("{e:#}")),
                }
            }
            Next::Run(act) => {
                let line = argv(&act);
                let worked = match &act {
                    Act::EditDefaults => edit(app, terminal, paths, None, None, &line)?,
                    Act::Edit { capability, name } => edit(
                        app,
                        terminal,
                        paths,
                        Some(capability),
                        name.as_deref(),
                        &line,
                    )?,
                    _ => run_all(app, &exe, &command_lines(&act)),
                };
                if worked {
                    ran.extend(ran_lines(&act));
                }
                app.reload(model(paths));
            }
        }
    }
}

/// `$EDITOR`, given the terminal whole, through `omh settings edit`'s own steps.
fn edit(
    app: &mut App,
    terminal: &mut ratatui::DefaultTerminal,
    paths: &Paths,
    capability: Option<&str>,
    name: Option<&str>,
    line: &[String],
) -> Result<bool> {
    let target = match settings::edit_target(paths, capability, name, config::Layer::Personal) {
        Ok(target) => target,
        Err(e) => {
            app.report(shown(line), false, "", &format!("{e:#}"));
            return Ok(false);
        }
    };
    suspend().context("handing the terminal to $EDITOR")?;
    let edited = settings::open_in_editor(&target);
    let (ok, said, mut warned) = match &edited {
        Ok(Edited::Done) => (true, "editor closed".to_string(), String::new()),
        Ok(Edited::Kept(dir)) => (
            true,
            "editor closed".to_string(),
            settings::kept_without_skill_md(dir),
        ),
        Ok(Edited::NothingSaved(dir)) => (
            true,
            format!(
                "editor closed — nothing was saved into {}, so it was not made",
                dir.display()
            ),
            String::new(),
        ),
        Err(e) => (false, String::new(), format!("{e:#}")),
    };
    // If the screen cannot come back, what the editor did still reaches the
    // person, in the error that ends the screen.
    resume(terminal).with_context(|| {
        format!(
            "after `omh {}`: {}",
            shown(line),
            if ok { &said } else { &warned }
        )
    })?;
    // Read back, not assumed: the editor closing is not the entry being one.
    if ok && warned.is_empty() {
        warned = after_edit(&model(paths), capability, name).unwrap_or_default();
    }
    app.report(shown(line), ok, &said, &warned);
    Ok(ok)
}

/// What an edit left, read back from a fresh read of the catalogue: a file
/// that no longer parses, an entry that is not one — or a hook omh ships,
/// which `omh upgrade` puts back over the edit. `None` when there is nothing
/// to say.
fn after_edit(model: &Model, capability: Option<&str>, name: Option<&str>) -> Option<String> {
    let section = model
        .sections
        .iter()
        .find(|s| s.page.capability() == capability)?;
    let Some(name) = name else {
        return section.problem.clone();
    };
    let row = section.rows.iter().find(|r| r.name == name)?;
    let cap = capability.unwrap_or_default();
    match (
        &row.warning,
        row.fixed && section.page == Page::Entries("hooks"),
    ) {
        (Some(w), _) => Some(format!("{cap}/{name}: {w}")),
        (None, true) => Some(format!(
            "omh ships {cap}/{name}: `omh upgrade` puts its own back, keeping yours as \
             {name}.json.yours"
        )),
        (None, false) => None,
    }
}

/// Every command line an act is, in order: one, but for a move — the key it
/// was meant to be set, then the one nothing reads unset.
pub(crate) fn command_lines(act: &Act) -> Vec<Vec<String>> {
    match act {
        Act::Move { from, .. } => vec![
            argv(act),
            ["settings", "unset", "--", from].map(String::from).to_vec(),
        ],
        _ => vec![argv(act)],
    }
}

/// What the screen ran, as it says it once it closes: each line as a shell
/// runs it, and whether a value in it is held back.
///
/// Not [`shown`], which is the line as a person would type it — no `--yes`,
/// one status for a move's two lines — and printed in scrollback that read as
/// a script nothing could run. A server's env values and arguments can be a
/// token, so that one line is `shown`'s, with them held back: never drawn,
/// here least of all, where scrollback keeps it.
fn ran_lines(act: &Act) -> Vec<(String, bool)> {
    command_lines(act)
        .iter()
        .map(|line| {
            if line.starts_with(&["settings".into(), "mcp".into(), "add".into()]) {
                return (format!("omh {}", shown(line)), true);
            }
            // `--` only where a word after it would read as a flag.
            let at = line.iter().position(|w| w == "--");
            let needed = at.is_some_and(|i| line[i + 1..].iter().any(|w| w.starts_with('-')));
            let words: Vec<String> = line
                .iter()
                .enumerate()
                .filter(|(i, _)| needed || Some(*i) != at)
                .map(|(_, w)| w.clone())
                .collect();
            (format!("omh {}", said(&words)), false)
        })
        .collect()
}

/// Run an act's lines in order, stopping at the first that fails, and say
/// what they said as one: what ran, joined, and every line's output. Answers
/// whether every line worked.
fn run_all(app: &mut App, exe: &std::path::Path, lines: &[Vec<String>]) -> bool {
    let shown_all = lines
        .iter()
        .map(|l| shown(l))
        .collect::<Vec<_>>()
        .join(" && ");
    let (mut out, mut err) = (String::new(), String::new());
    for line in lines {
        match omh(exe, line) {
            Ok(done) => {
                out.push_str(&String::from_utf8_lossy(&done.stdout));
                err.push_str(&String::from_utf8_lossy(&done.stderr));
                if !done.status.success() {
                    if done.stdout.is_empty() && done.stderr.is_empty() {
                        err.push_str(&format!("it ended with {}, and said nothing", done.status));
                    }
                    app.report(shown_all, false, &out, &err);
                    return false;
                }
            }
            // On the screen rather than out of it: a child that would not
            // start is one failed act, and leaving took every unsaved thing on
            // the screen with it.
            Err(e) => {
                err.push_str(&format!("{e:#}"));
                app.report(shown_all, false, &out, &err);
                return false;
            }
        }
    }
    app.report(shown_all, true, &out, &err);
    true
}

/// The command line an act is, after `omh`.
///
/// Flags first, then `--`, then what the act names: a value like `-x` — or
/// `--help`, which clap answers by printing help and exiting 0 — would
/// otherwise be read as a flag, and the screen would report the help text
/// in green over a setting it never wrote.
///
/// Guarded by `every_act_is_a_line_omh_accepts_and_means_what_it_says`, which
/// parses each one back through the real grammar: a line here that clap read
/// differently — `--env` swallowed as a server argument, say — would be a
/// screen that does something other than what it shows.
pub(crate) fn argv(act: &Act) -> Vec<String> {
    let s = |words: &[&str]| words.iter().map(|w| w.to_string()).collect::<Vec<_>>();
    match act {
        Act::Set { key, value } => s(&["settings", "set", "--", key, value]),
        Act::SetAnyway { key, value } => s(&["settings", "set", "--anyway", "--", key, value]),
        // The first of its two lines; `command_lines` adds the unset after it.
        // A move undone writes the typo back, a key nothing reads, which `set`
        // refuses without `--anyway`.
        Act::Move { to, value, .. } if key::describes(to).is_none() => {
            s(&["settings", "set", "--anyway", "--", to, value])
        }
        Act::Move { to, value, .. } => s(&["settings", "set", "--", to, value]),
        Act::Use { capability, name } => s(&["settings", "use", "--", capability, name]),
        Act::Unuse { capability, name } => s(&["settings", "unuse", "--", capability, name]),
        Act::Unset { key } => s(&["settings", "unset", "--", key]),
        Act::EditDefaults => s(&["settings", "edit"]),
        Act::Edit { capability, name } => {
            let mut line = s(&["settings", "edit", "--", capability]);
            line.extend(name.clone());
            line
        }
        // `--yes` because the screen has already asked, in the words of this
        // same line's preview.
        Act::Remove { capability, name } => s(&["settings", "rm", "--yes", "--", capability, name]),
        Act::AddServer {
            name,
            command,
            args,
            env,
        } => {
            // `--env` first: the server's arguments run to the end of the line,
            // so anything after them is one of them. And everything after
            // `--`, so a name, a command or an argument that looks like a flag
            // is still what it was.
            let mut line = s(&["settings", "mcp", "add"]);
            for (k, v) in env {
                line.push("--env".into());
                line.push(format!("{k}={v}"));
            }
            line.push("--".into());
            line.push(name.clone());
            line.push(command.clone());
            line.extend(args.iter().cloned());
            line
        }
        Act::Import {
            capability,
            harness,
        } if capability == "mcp" => s(&["settings", "mcp", "import", "--", harness]),
        Act::Import {
            capability,
            harness,
        } => s(&["import", "--", capability, harness]),
    }
}

/// Run one line through this binary, its output captured.
///
/// Plain, because the screen paints; and with nothing on stdin, because
/// nothing here will answer a question — a command that would ask refuses
/// instead, which is what `--yes` on `rm` is for.
fn omh(exe: &std::path::Path, line: &[String]) -> Result<std::process::Output> {
    std::process::Command::new(exe)
        .args(["--color", "never"])
        .args(line)
        .stdin(std::process::Stdio::null())
        .output()
        // The line as drawn, not as run: this error is drawn too, and the line
        // that runs carries the values `shown` hides.
        .with_context(|| format!("running omh {}", shown(line)))
}

fn report(app: &mut App, line: &[String], out: &std::process::Output) {
    let stdout = String::from_utf8_lossy(&out.stdout);
    let mut stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    // A child killed by a signal says nothing, and "failed" alone would leave
    // nobody anywhere to look.
    if !out.status.success() && stdout.trim().is_empty() && stderr.trim().is_empty() {
        stderr = format!("it ended with {}, and said nothing", out.status);
    }
    app.report(shown(line), out.status.success(), &stdout, &stderr);
}

fn lines(bytes: &[u8]) -> Vec<String> {
    String::from_utf8_lossy(bytes)
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(str::to_string)
        .collect()
}

/// The line as it is drawn: [`said`], with each `--env` value hidden and a
/// server's arguments counted rather than shown.
///
/// It stays on the screen where anybody looking can read it, and those are
/// where a server's token goes — the same reason the servers page names
/// variables and counts arguments. What runs still carries both.
///
/// The `--` that keeps a value like `-x` from reading as a flag is left out
/// when nothing after it begins with `-`: the line is the same line without
/// it, and easier to read.
fn shown(line: &[String]) -> String {
    let split = line.iter().position(|w| w == "--");
    let (flags, named) = match split {
        Some(at) => (&line[..at], &line[at + 1..]),
        None => (line, &line[line.len()..]),
    };
    let mut hidden = Vec::with_capacity(line.len());
    let mut after_env = false;
    // The screen's own: it previews with `--dry-run` and answers with `--yes`
    // because it asked. The line shown is the one a person would type.
    for word in flags.iter().filter(|w| *w != "--dry-run" && *w != "--yes") {
        hidden.push(match word.split_once('=') {
            Some((k, _)) if after_env => format!("{k}=…"),
            _ => word.clone(),
        });
        after_env = word == "--env";
    }
    // A server's own arguments are whatever follows its name and command.
    let adding_a_server = line.starts_with(&["settings".into(), "mcp".into(), "add".into()]);
    let (named, counted) = match adding_a_server {
        true => named.split_at(named.len().min(2)),
        false => (named, &named[named.len()..]),
    };
    if split.is_some() && named.iter().any(|w| w.starts_with('-')) {
        hidden.push("--".into());
    }
    hidden.extend(named.iter().cloned());
    match counted.len() {
        0 => said(&hidden),
        n => format!("{} … ({n} argument{})", said(&hidden), plural(n)),
    }
}

/// The line as somebody would type it into a shell, quoted where it has to be.
fn said(line: &[String]) -> String {
    line.iter()
        .map(|w| {
            let plain = !w.is_empty()
                && w.chars()
                    .all(|c| c.is_ascii_alphanumeric() || "-_./:=@,+%".contains(c));
            if plain {
                w.clone()
            } else {
                format!("'{}'", w.replace('\'', r"'\''"))
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn suspend() -> Result<()> {
    ratatui::crossterm::execute!(std::io::stdout(), DisableMouseCapture)?;
    ratatui::crossterm::terminal::disable_raw_mode()?;
    ratatui::crossterm::execute!(
        std::io::stdout(),
        ratatui::crossterm::terminal::LeaveAlternateScreen
    )?;
    Ok(())
}

fn resume(terminal: &mut ratatui::DefaultTerminal) -> Result<()> {
    ratatui::crossterm::execute!(
        std::io::stdout(),
        ratatui::crossterm::terminal::EnterAlternateScreen
    )?;
    ratatui::crossterm::terminal::enable_raw_mode()?;
    ratatui::crossterm::execute!(std::io::stdout(), EnableMouseCapture)?;
    terminal.clear()?;
    Ok(())
}

/// Everything the screen shows, read fresh.
///
/// **Your catalogue alone** — `~/.omh` — and never this checkout's own hooks,
/// which `Profile::entries` adds for a launch. The screen removes what it
/// lists, and a list that mixed the two would offer the repo's files under
/// your catalogue's name.
pub(crate) fn model(paths: &Paths) -> Model {
    let adapters = Adapter::load_dir(&paths.adapters());
    let mut sections = vec![defaults(paths, &adapters)];
    for cap in Capability::ALL {
        let mut section = match cap {
            Capability::Mcp => servers(paths),
            _ => entries(paths, cap),
        };
        section.import = match (&adapters, cap) {
            // A hook binds to one project's commands and lands in that repo,
            // so importing one is not something your catalogue can take.
            (_, Capability::Hooks) => Import::Not(
                "a hook lands in a repo, not in your catalogue — in one, \
                 `omh import hooks <harness>`"
                    .into(),
            ),
            (Ok(adapters), _) => Import::From(
                adapters
                    .iter()
                    .filter_map(|a| {
                        let template = a.supports(cap)?.import.as_deref()?;
                        let (from, empty) = import_from(paths, template);
                        Some(crate::tui::Source {
                            harness: a.name.to_string(),
                            from,
                            empty,
                        })
                    })
                    .collect(),
            ),
            (Err(e), _) => Import::Not(format!("could not read the adapters: {e:#}")),
        };
        section.about = belongs(cap, &section.import);
        seeds(paths, cap, &mut section);
        sections.push(section);
    }
    Model {
        root: tilde(&paths.root, dirs::home_dir().as_deref()),
        sections,
    }
}

/// Whether the repos `omh init` sets up after get each row, and what flips
/// it — and how many the page's new repos get.
///
/// An entry of yours is taken by `[use]`'s list where the kind has one, and
/// otherwise unless `[unuse]` leaves it out or it is a guard nobody named;
/// `omh settings use`/`unuse` change one. (omh's own are not toggled here:
/// they are its features', on your defaults page.) "All" is said only while a
/// kind has no list — a list holding every entry is not the same fact, since
/// an entry added after it is not seeded.
fn seeds(paths: &Paths, cap: Capability, section: &mut Section) {
    // The answer `omh init` writes, asked of the same function: the two
    // drifting is how a guard was drawn as seeded.
    let now = match crate::cmd::catalogue::new_repo_takes(paths, cap, false) {
        Ok(now) => now,
        Err(e) => {
            section.problem.get_or_insert_with(|| format!("{e:#}"));
            return;
        }
    };
    // A file of yours named as one of omh's own is its feature's: `use`
    // refuses the name, so the row takes no `space` and is not counted.
    let owned = match settings::shipped_owned(cap) {
        Ok(owned) => owned,
        Err(e) => {
            section.problem.get_or_insert_with(|| format!("{e:#}"));
            return;
        }
    };
    for row in &mut section.rows {
        if let Some(feature) = owned.get(&row.name) {
            row.about.push(format!(
                "omh's — part of the `{feature}` feature, switched whole on your \
                 defaults page: omh settings set {feature} on|off"
            ));
            continue;
        }
        let on = now.takes.contains(&row.name);
        row.seeded = Some(on);
        if !on && !now.listed && now.guards.contains(&row.name) {
            row.about.push(
                "a guard: it refuses the agent's edits, so new repos get it only if you \
                 use it"
                    .into(),
            );
        }
        let (capability, name) = (cap.key().to_string(), row.name.clone());
        let (unuse, use_it) = (
            Act::Unuse {
                capability: capability.clone(),
                name: name.clone(),
            },
            Act::Use { capability, name },
        );
        // What puts the file back: the opposite verb, except where the flip
        // writes more than its own name. A guard taken by a kind with no list
        // writes the kind as one; a rule taken off its list comes back at the
        // end of it, and rules are read in the list's order.
        let last = now.takes.last() == Some(&row.name);
        let guard = now.guards.contains(&row.name);
        (row.toggle, row.untoggle) = match on {
            true => {
                let exact = !now.listed || cap != Capability::Rules || last;
                (Some(unuse), exact.then_some(use_it))
            }
            false => {
                let exact = now.listed || !guard;
                (Some(use_it), exact.then_some(unuse))
            }
        };
    }
    let total = section.rows.iter().filter(|r| r.seeded.is_some()).count();
    let got = section
        .rows
        .iter()
        .filter(|r| r.seeded == Some(true))
        .count();
    // Guards not taken are not "left out": nobody ever turns one on for you.
    let guards = section
        .rows
        .iter()
        .filter(|r| now.guards.contains(&r.name) && r.seeded == Some(false))
        .count();
    let left = total - got - guards;
    let mut said = match (now.listed, total - got, left, guards) {
        (true, ..) => format!("new repos get {got} of {total}"),
        (false, 0, ..) => "new repos get all".to_string(),
        (false, _, 0, _) => "new repos get all but the guards".to_string(),
        (false, _, n, 0) => format!("new repos get all but {n}"),
        (false, _, n, _) => format!("new repos get all but {n}, and no guards"),
    };
    if cap == Capability::Hooks {
        // A shipped hook runs only where its stack does.
        said.push_str(", by stack");
    }
    section.seeds = (total > 0).then_some(said);
}

/// What one of your defaults' lists — `[use]` or `[unuse]` — names that it
/// cannot mean, each with what `omh init` does about it: a kind that is not one
/// is refused, a name `[use]` lists that your catalogue does not have is warned
/// about, and one `[unuse]` leaves out is nothing to leave out. `[use] skill =
/// …` meant every skill, silently.
fn names_nothing(paths: &Paths, table: &str) -> Option<String> {
    let mut wrong = Vec::new();
    let doc = config::read_doc(&config::Layer::Personal.file(paths)).ok()?;
    let held = doc.get(table).and_then(|t| t.as_table_like())?;
    for (kind, _) in held.iter() {
        if Capability::from_key(kind).is_none() {
            wrong.push(format!(
                "{} — `omh init` refuses it",
                config::not_a_kind(table, kind)
            ));
        }
    }
    for cap in Capability::ALL {
        let listed = match config::list_in(paths, config::Layer::Personal, table, cap) {
            Ok(Some(listed)) => listed,
            Ok(None) => continue,
            Err(e) => {
                wrong.push(format!("{e:#} — `omh init` refuses it"));
                continue;
            }
        };
        // The catalogue as the kind's page reads it: what is on disk, whether
        // or not `~/.omh/base` can be read.
        let catalogue = match crate::profile::Profile::resolve(paths).entries(cap) {
            Ok(names) => names,
            Err(e) => {
                wrong.push(format!("could not check [{table}] {cap}: {e:#}"));
                continue;
            }
        };
        let then = match table {
            config::USE => "`omh init` will warn",
            _ => "it leaves out nothing",
        };
        wrong.extend(
            listed
                .into_iter()
                .filter(|n| !catalogue.contains(n))
                .map(|n| {
                    format!(
                        "[{table}] {}/{n}, which your catalogue does not have — {then}",
                        cap.key()
                    )
                }),
        );
    }
    (!wrong.is_empty()).then(|| format!("names {}", wrong.join("; ")))
}

/// How your defaults switch omh's features for new repos: `[omh]` in
/// `default.toml`, by name. A feature not there is on.
fn feature_switches(paths: &Paths) -> std::collections::BTreeMap<String, bool> {
    let Ok(doc) = config::read_doc(&config::Layer::Personal.file(paths)) else {
        return Default::default();
    };
    doc.get(config::OMH)
        .and_then(|t| t.as_table_like())
        .map(|t| {
            t.iter()
                .filter_map(|(k, v)| v.as_bool().map(|b| (k.to_string(), b)))
                .collect()
        })
        .unwrap_or_default()
}

/// Where an import reads, as the pick says it: the adapter's path on this
/// host — from where omh was started, for `$REPO` — and whether there is
/// anything there. The bare list of harness names said neither.
fn import_from(paths: &Paths, template: &str) -> (String, bool) {
    let home = dirs::home_dir();
    let at = crate::adapter::expand_host(
        template,
        home.as_deref().unwrap_or(std::path::Path::new("~")),
        &paths.repo,
    );
    // `$REPO` is where omh was started, so it is said from there: `./`.
    let shown = match template.contains("$REPO") {
        true => format!("{}, where omh was started", template.replace("$REPO", ".")),
        false => tilde(&at, home.as_deref()),
    };
    match at.try_exists() {
        Ok(true) => (shown, false),
        Ok(false) => (format!("{shown} — nothing there"), true),
        // Could not look is not none: offered, and the import says why.
        Err(e) => (format!("{shown} — could not look: {e}"), false),
    }
}

/// How far a change to an entry goes, said on each: the screen spoke of new
/// repos alone, under which an edit read as reaching none.
const REACHES: &str = "reaches every repo that takes it, at its next launch";

/// What belongs on a page, for one with nothing on it yet: what the kind is,
/// where one lives in your catalogue, and how to get one.
fn belongs(cap: Capability, import: &Import) -> Vec<String> {
    let (what, at) = match cap {
        Capability::Rules => (
            "a rule is Markdown omh adds to the instructions every session is given",
            "rules/<name>.md",
        ),
        Capability::Skills => (
            "a skill is a directory with a SKILL.md, which an agent loads when a task calls for it",
            "skills/<name>/SKILL.md",
        ),
        Capability::Mcp => (
            "a server is a program an agent can call tools in, run by the harness — \
             omh's own are its features', switched on your defaults page",
            "a record in mcp.json",
        ),
        Capability::Commands => (
            "a command is a prompt you run by its name",
            "commands/<name>.md",
        ),
        Capability::Subagents => (
            "a subagent is an agent of its own that the main one can hand work to",
            "subagents/<name>.md",
        ),
        Capability::Hooks => (
            "a hook is a command omh runs at a moment in a session — its start, before or \
             after a tool, the end of a turn",
            "hooks/<name>.json",
        ),
    };
    let mut out = vec![what.to_string(), format!("at    {at}"), String::new()];
    out.push(match cap {
        Capability::Mcp => "a adds one".into(),
        _ => "a makes one in $EDITOR".into(),
    });
    if let Import::From(harnesses) = import {
        if !harnesses.is_empty() {
            let names: Vec<&str> = harnesses.iter().map(|h| h.harness.as_str()).collect();
            out.push(format!("i imports from {}", names.join(", ")));
        }
    }
    out
}

/// A path from your home down, as the header says it: `~/.omh`.
fn tilde(path: &std::path::Path, home: Option<&std::path::Path>) -> String {
    match home.and_then(|home| path.strip_prefix(home).ok()) {
        Some(below) => format!("~/{}", below.display()),
        None => path.display().to_string(),
    }
}

/// A path from the catalogue down — `hooks/mine.json` — since the header
/// already says where the catalogue is.
fn within(paths: &Paths, path: &std::path::Path) -> String {
    match path.strip_prefix(&paths.root) {
        Ok(below) => below.display().to_string(),
        Err(_) => path.display().to_string(),
    }
}

fn section(page: Page, title: &'static str) -> Section {
    Section {
        title,
        page,
        rows: Vec::new(),
        problem: None,
        import: Import::From(Vec::new()),
        about: Vec::new(),
        seeds: None,
    }
}

/// What a value of this shape looks like, for the line it is typed into.
fn hint(shape: &key::Shape) -> String {
    match shape {
        key::Shape::Text => String::new(),
        key::Shape::Paths => "a list of paths: [\".env\", \".env.local\"]".into(),
        key::Shape::Path => "an absolute path".into(),
        key::Shape::Duration => "a duration: 90s, 30m, 2h, 1d".into(),
        key::Shape::Size => "a size: 4g, 512m".into(),
        key::Shape::Count => "a number above zero: 2, 1.5".into(),
        key::Shape::Choice(allowed) => format!("one of {}", allowed.join(", ")),
    }
}

/// Every key omh reads, set or not — the half no settings file can show — and
/// anything else the file holds, named for what it is.
fn defaults(paths: &Paths, adapters: &Result<Vec<Adapter>>) -> Section {
    let mut out = section(Page::Defaults, "defaults");
    out.import = Import::Not("your defaults are typed, not imported".into());
    let file = config::Layer::Personal.file(paths);
    let mine = match config::values(paths, config::Layer::Personal) {
        Ok(mine) => mine,
        Err(e) => {
            out.problem = Some(format!("{e:#}"));
            return out;
        }
    };
    for k in key::KEYS {
        let mut warning = None;
        let mut fallback = k.unset.to_string();
        let edit = match (k.name, &k.shape) {
            (_, key::Shape::Choice(allowed)) => {
                Edit::Choice(allowed.iter().map(|a| a.to_string()).collect())
            }
            // The logins omh has captured, by name and by one harness's — and
            // only those, since a launch takes nothing else: `another…` let a
            // name be typed, which was for `codex:work`, and that is here.
            ("account", _) => {
                let typed = Edit::Text {
                    hint: "a login's name, as `omh auth <harness> -n <name>` captured it".into(),
                };
                match logins(paths, adapters) {
                    Ok(held) => {
                        (fallback, warning) = unset_account(&held);
                        let names: std::collections::BTreeSet<String> =
                            held.iter().flat_map(|(_, names)| names.clone()).collect();
                        let qualified = held.iter().flat_map(|(harness, names)| {
                            names.iter().map(move |n| format!("{harness}:{n}"))
                        });
                        match names.is_empty() {
                            true => typed,
                            false => Edit::Choice(names.into_iter().chain(qualified).collect()),
                        }
                    }
                    Err(e) => {
                        warning = Some(format!("could not list the logins: {e:#}"));
                        typed
                    }
                }
            }
            (_, shape) => Edit::Text { hint: hint(shape) },
        };
        // A value omh cannot read, marked as long as it is in the file — not
        // only while it is being typed.
        let value = mine.get(k.name).cloned();
        let warning = value
            .as_deref()
            .and_then(|v| key::quarrel(k, v))
            .or(warning);
        out.rows.push(Row {
            name: k.name.to_string(),
            value,
            fallback: Some(fallback),
            warning,
            about: vec![
                k.does.to_string(),
                "seeds a new repo's .omh/settings.toml, which is committed — a repo that \
                 exists keeps its own"
                    .into(),
            ],
            edit,
            ..Row::default()
        });
    }
    // omh's features, switched for the repos `omh init` sets up after, under a
    // heading of their own — where `[omh]` was one table row to open the file
    // for.
    let switched = feature_switches(paths);
    let shipped = crate::base::Manifest::shipped();
    for feature in settings::shipped_features().unwrap_or_default() {
        let mut about = vec![format!(
            "one of omh's features — its servers, hooks and rules section, on or off \
                 together, for the repos `omh init` sets up after. A repo that exists \
                 keeps its own: `omh set {feature} off` there."
        )];
        // What switching it whole switches, by name: the sentence above named
        // none of them.
        match &shipped {
            Ok(manifest) => {
                about.push(String::new());
                for (kind, label) in [
                    (crate::base::Kind::Mcp, "server  "),
                    (crate::base::Kind::Hook, "hooks   "),
                    (crate::base::Kind::Rules, "rules   "),
                ] {
                    let names: Vec<&str> = manifest
                        .entries
                        .iter()
                        .filter(|e| e.feature == feature && e.kind == kind)
                        .map(|e| e.name.as_str())
                        .collect();
                    if !names.is_empty() {
                        about.push(format!("{label}{}", names.join(", ")));
                    }
                }
            }
            Err(e) => about.push(format!("could not read what it brings: {e:#}")),
        }
        out.rows.push(Row {
            value: switched
                .get(&feature)
                .map(|on| if *on { "on" } else { "off" }.to_string()),
            fallback: Some("on".into()),
            about,
            edit: Edit::Choice(vec!["on".into(), "off".into()]),
            group: Some("features".into()),
            name: feature,
            ..Row::default()
        });
    }
    let doc = config::read_doc(&file);
    for (name, value) in &mine {
        if key::describes(name).is_some() || *name == format!("[{}]", config::OMH) {
            continue;
        }
        let table = name.starts_with('[');
        let row = match table {
            // What the table holds, on one line: `[use]` was drawn as unset.
            true => Row {
                name: name.clone(),
                value: Some(table_summary(&doc, name)),
                warning: [config::USE, config::UNUSE]
                    .iter()
                    .any(|t| *name == format!("[{t}]"))
                    .then(|| names_nothing(paths, name.trim_matches(['[', ']'])))
                    .flatten(),
                about: match name.trim_matches(['[', ']']) {
                    config::USE => vec![
                        "what `omh init` gives a new repo from your catalogue, where a kind \
                         is listed. space on an entry's page changes it."
                            .into(),
                        format!("file  {}", within(paths, &file)),
                    ],
                    config::UNUSE => vec![
                        "what `omh init` leaves out of a new repo, of a kind with no list — \
                         it takes the rest, and one you add later. space on an entry's page \
                         changes it."
                            .into(),
                        format!("file  {}", within(paths, &file)),
                    ],
                    _ => vec![
                        format!("a table in {}", within(paths, &file)),
                        "e opens the file to change it".into(),
                    ],
                },
                ..Row::default()
            },
            false => Row {
                name: name.clone(),
                value: Some(value.clone()),
                warning: Some(match key::nearest(name) {
                    Some(meant) => format!("nothing omh reads — did you mean `{meant}`?"),
                    None => "nothing omh reads".into(),
                }),
                about: vec![format!("file  {}", within(paths, &file))],
                edit: Edit::Text {
                    hint: String::new(),
                },
                // Apart from the keys omh reads, under a heading saying so.
                group: Some("read by nothing".into()),
                meant: key::nearest(name).map(String::from),
                ..Row::default()
            },
        };
        out.rows.push(row);
    }
    // Tables are read; the heading is for what is not, so it comes last.
    out.rows.sort_by_key(|r| r.group.is_some());
    out
}

/// The logins captured, by harness, for every harness that has one.
fn logins(paths: &Paths, adapters: &Result<Vec<Adapter>>) -> Result<Vec<(String, Vec<String>)>> {
    let adapters = match adapters {
        Ok(adapters) => adapters,
        Err(e) => anyhow::bail!("could not read the adapters: {e:#}"),
    };
    let mut held = Vec::new();
    for adapter in adapters {
        let names = crate::auth::accounts(paths, adapter)?;
        if !names.is_empty() {
            held.push((adapter.name.to_string(), names));
        }
    }
    Ok(held)
}

/// What a launch does with `account` unset, from the logins there are — the
/// rule `auth::resolve` applies: none is a login to capture, one is used, and
/// several are refused rather than guessed between.
fn unset_account(held: &[(String, Vec<String>)]) -> (String, Option<String>) {
    let several: Vec<String> = held
        .iter()
        .filter(|(_, names)| names.len() > 1)
        .map(|(harness, names)| format!("{harness} has {} ({})", names.len(), names.join(", ")))
        .collect();
    match (held, several.is_empty()) {
        ([], _) => (
            "no login captured — `omh auth <harness>` captures one".into(),
            None,
        ),
        (_, false) => (
            "none chosen".into(),
            Some(format!(
                "none chosen — {}, and a launch refuses to guess between them",
                several.join("; ")
            )),
        ),
        ([(harness, names)], true) => (format!("{}, the only login {harness} has", names[0]), None),
        (_, true) => ("the one login each harness has".into(), None),
    }
}

/// A table of the defaults file on one line: `skills = ["tdd"] · hooks = []`.
fn table_summary(doc: &Result<toml_edit::DocumentMut>, name: &str) -> String {
    let key = name.trim_start_matches('[').trim_end_matches(']');
    let Ok(doc) = doc else {
        return "a table".into();
    };
    match doc.get(key).and_then(|item| item.as_table()) {
        Some(table) if table.is_empty() => "empty".into(),
        Some(table) => table
            .iter()
            .map(|(k, v)| format!("{k} = {}", v.to_string().trim()))
            .collect::<Vec<_>>()
            .join(" · "),
        None => "a table".into(),
    }
}

fn servers(paths: &Paths) -> Section {
    let mut out = section(Page::Servers, "servers");
    let path = config::mcp_path(paths);
    let held = match path.try_exists() {
        Ok(false) => return out,
        Ok(true) => crate::render::parse_layers(std::slice::from_ref(&path)),
        Err(e) => Err(e).with_context(|| format!("reading {}", path.display())),
    };
    let held = match held {
        Ok(held) => held,
        Err(e) => {
            out.problem = Some(format!("{e:#}"));
            return out;
        }
    };
    // Which are omh's, asked of the binary, as `rm` and `mcp add` ask it: a
    // feature's server is the feature's, switched with its hooks and rules on
    // your defaults page and listed nowhere else — here it read as a server
    // that could be switched apart from them.
    let shipped = match settings::shipped_servers() {
        Ok(shipped) => shipped,
        Err(e) => {
            out.problem = Some(format!("{e:#}"));
            Default::default()
        }
    };
    for (name, server) in held
        .into_iter()
        .filter(|(name, _)| !shipped.contains_key(name))
    {
        // The command, as `omh settings mcp ls` prints it, and the arguments
        // only counted: `--header "Authorization: Bearer …"` is how a remote
        // server is reached, and this is drawn for anybody looking at the
        // screen. `e` opens the file, for somebody who meant to read them.
        let mut about = vec![format!("runs  {}", server.command)];
        if !server.args.is_empty() {
            about.push(format!(
                "with  {} argument{} — e opens the file to read them",
                server.args.len(),
                plural(server.args.len())
            ));
        }
        // The names only. A server's environment is where its token goes.
        let env = server.env.keys().cloned().collect::<Vec<_>>().join(", ");
        if !env.is_empty() {
            about.push(format!("env   {env}"));
        }
        about.push(format!("file  {}", within(paths, &path)));
        about.push(REACHES.into());
        // Names and counts, as the detail says them: never a value.
        let mut summary = vec![server.command.clone()];
        if !server.args.is_empty() {
            summary.push(format!(
                "{} arg{}",
                server.args.len(),
                plural(server.args.len())
            ));
        }
        if !env.is_empty() {
            summary.push(format!("env {env}"));
        }
        out.rows.push(Row {
            summary: Some(summary.join(" · ")),
            name,
            about,
            ..Row::default()
        });
    }
    out
}

fn entries(paths: &Paths, cap: Capability) -> Section {
    let mut out = section(Page::Entries(cap.key()), cap.key());
    let dir = paths.root.join(cap.source());
    // Absent is a kind you have none of; every other failure is said.
    match dir.try_exists() {
        Ok(false) => return out,
        Ok(true) => {}
        Err(e) => {
            out.problem = Some(format!("reading {}: {e}", dir.display()));
            return out;
        }
    }
    let listed = match crate::profile::listed(&dir, cap) {
        Ok(listed) => listed,
        Err(e) => {
            out.problem = Some(format!("{e:#}"));
            return out;
        }
    };
    let shipped = settings::shipped_hooks();
    let mut triggers = std::collections::BTreeMap::new();
    for (name, path) in listed {
        let ships = cap == Capability::Hooks && shipped.contains(&name);
        let said = describe(cap, &path);
        if let Some(trigger) = &said.trigger {
            triggers.insert(name.clone(), trigger.clone());
        }
        let mut about = said.more;
        about.push(match path.is_dir() {
            true => format!("dir   {}", within(paths, &path)),
            false => format!("file  {}", within(paths, &path)),
        });
        // How far an edit goes: not new repos alone.
        about.push(REACHES.into());
        if ships {
            about.push(
                "omh ships this one and `omh upgrade` refreshes it — `omh unuse hooks` \
                 stops a repo running it"
                    .into(),
            );
        }
        out.rows.push(Row {
            name,
            summary: said.summary,
            badge: said.badge,
            warning: said.warning,
            // Yours first, then omh's: the eight it ships are the same on every
            // machine, and the ones you wrote are the ones you came to change.
            group: (cap == Capability::Hooks).then(|| {
                match ships {
                    true => "shipped with omh",
                    false => "yours",
                }
                .to_string()
            }),
            about,
            fixed: ships,
            ..Row::default()
        });
    }
    if cap == Capability::Hooks {
        out.rows
            .sort_by_key(|r| r.group.as_deref() != Some("yours"));
        // When it runs, as a column: the one somebody scans first.
        let width = triggers
            .values()
            .map(|t| t.chars().count())
            .max()
            .unwrap_or(0);
        for row in &mut out.rows {
            if let (Some(trigger), Some(what)) = (triggers.get(&row.name), &row.summary) {
                row.summary = Some(format!("{trigger:<width$}  {what}"));
            }
        }
    }
    out
}

/// What an entry says about itself, read from the entry.
#[derive(Debug, Default)]
struct Described {
    summary: Option<String>,
    /// When a hook runs, kept apart so the page can line it up as a column.
    trigger: Option<String>,
    badge: Option<String>,
    warning: Option<String>,
    more: Vec<String>,
}

/// What an entry is, from the entry itself.
///
/// A file that will not read says so, in place of a description: the row is
/// still there and still removable, and an empty description would read as
/// an entry with nothing to say rather than one omh could not open. So does a
/// skill with no `SKILL.md` — half made, or half removed — which looks like
/// any other skill and is one nothing loads, and a link to nothing, which
/// `listed` keeps and which is an entry no harness can read.
fn describe(cap: Capability, path: &std::path::Path) -> Described {
    let warned = |warning: String| Described {
        warning: Some(warning),
        ..Described::default()
    };
    if path.is_symlink() && !path.exists() {
        let to = std::fs::read_link(path)
            .map(|t| t.display().to_string())
            .unwrap_or_else(|e| format!("somewhere omh could not read ({e})"));
        return warned(format!("a link to nothing — {to}"));
    }
    let file = match cap {
        Capability::Skills if path.is_dir() => path.join("SKILL.md"),
        _ => path.to_path_buf(),
    };
    if file.is_dir() {
        return Described::default();
    }
    let raw = match std::fs::read_to_string(&file) {
        Ok(raw) => raw,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound && cap == Capability::Skills => {
            return warned("no SKILL.md in it — nothing will load this skill".into());
        }
        Err(e) => return warned(format!("could not read {}: {e}", file.display())),
    };
    if cap == Capability::Hooks {
        return match hook::Hook::parse(&raw, &file.display().to_string()) {
            Ok(h) => {
                let what = match &h.action {
                    hook::Action::Run(cmd) => cmd.clone(),
                    hook::Action::Refuse { text } => format!("refuses — {text}"),
                    hook::Action::Inject { text, .. } => format!("tells the agent — {text}"),
                };
                let tools = h
                    .tools
                    .iter()
                    .map(|t| t.to_string())
                    .collect::<Vec<_>>()
                    .join(", ");
                let on = match tools.is_empty() {
                    true => h.on.to_string(),
                    false => format!("{} {tools}", h.on),
                };
                Described {
                    summary: Some(what.clone()),
                    trigger: Some(on.clone()),
                    badge: h.stack.clone(),
                    more: vec![format!("on    {on}"), format!("does  {what}")],
                    ..Described::default()
                }
            }
            Err(e) => warned(format!("not a hook omh can run: {e:#}")),
        };
    }
    // A `description:` in front matter is what the entry says about itself;
    // failing that, its first line of prose.
    let described = raw
        .lines()
        .find_map(|l| l.strip_prefix("description:"))
        .map(|d| d.trim().trim_matches('"').to_string());
    let first = || {
        raw.lines()
            .map(str::trim)
            .find(|l| !l.is_empty() && *l != "---" && !l.contains(':'))
            .map(|l| l.trim_start_matches('#').trim().to_string())
    };
    // A skill or a subagent is chosen by its description: an empty one, or
    // none, is one an agent is shown nothing for.
    let chosen_by_it = matches!(cap, Capability::Skills | Capability::Subagents);
    let warning = match described.as_deref() {
        _ if !chosen_by_it => None,
        Some("") => {
            Some("its `description:` is empty — what an agent is shown to know when to use it")
        }
        None => Some(
            "no `description:` in its front matter — what an agent is shown to know when to use it",
        ),
        Some(_) => None,
    };
    let summary = described.filter(|d| !d.is_empty()).or_else(first);
    Described {
        more: summary.iter().cloned().collect(),
        summary,
        warning: warning.map(String::from),
        ..Described::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::{Cli, Cmd, McpCmd, SettingsCmd};
    use clap::Parser;

    fn parsed(act: &Act) -> Cmd {
        let mut line = vec!["omh".to_string()];
        line.extend(argv(act));
        match Cli::try_parse_from(&line) {
            Ok(cli) => cli.cmd,
            Err(e) => panic!(
                "`omh {}` is not a line omh accepts: {e}",
                line[1..].join(" ")
            ),
        }
    }

    /// A feature's row says what it brings — its server, its hooks, its
    /// section of the rules — which is what switching it whole switches.
    /// "Its servers, hooks and rules section" named none of them.
    #[test]
    fn a_feature_says_what_it_brings() {
        let (_dir, paths) = paths();
        let model = model(&paths);
        let manifest = crate::base::Manifest::shipped().unwrap();
        let defaults = page(&model, "defaults");
        for feature in defaults
            .rows
            .iter()
            .filter(|r| r.group.as_deref() == Some("features"))
        {
            let said = feature.about.join("\n");
            for entry in manifest
                .entries
                .iter()
                .filter(|e| e.feature == feature.name)
            {
                assert!(
                    said.contains(&entry.name),
                    "{}: no {}:\n{said}",
                    feature.name,
                    entry.name
                );
            }
        }
    }

    /// Each row says how far a change to it goes, which is not "new repos"
    /// alone: an entry reaches every repo that takes it, at its next launch,
    /// and a default lands in a new repo's committed settings file. The screen
    /// said only what new repos get, under which an edit read as reaching no
    /// repo at all.
    #[test]
    fn each_row_says_how_far_a_change_to_it_goes() {
        let (_dir, paths) = paths();
        write(
            &paths.root.join("skills/tdd/SKILL.md"),
            "---\nname: tdd\ndescription: x\n---\n",
        );
        write(
            &config::mcp_path(&paths),
            r#"{"mcpServers":{"linear":{"command":"npx"}}}"#,
        );
        let model = model(&paths);
        let about = |title: &str, name: &str| {
            page(&model, title)
                .rows
                .iter()
                .find(|r| r.name == name)
                .unwrap_or_else(|| panic!("no {title}/{name}"))
                .about
                .join("\n")
        };
        for (title, name) in [("skills", "tdd"), ("servers", "linear")] {
            let said = about(title, name);
            assert!(
                said.contains("every repo that takes it") && said.contains("next launch"),
                "{title}/{name}: {said}"
            );
        }
        let said = about("defaults", "idle_timeout");
        assert!(
            said.contains("settings.toml") && said.contains("committed"),
            "{said}"
        );
    }

    /// A skill or a subagent with no description in its front matter is
    /// marked: the description is what an agent is shown to know when to use
    /// one. An empty `description:` — what the skeleton opens with — drew a
    /// blank row with no warning, and a `SKILL.md` stripped of its front
    /// matter drew its first line of prose as though that were one.
    #[test]
    fn an_entry_with_no_description_is_marked() {
        let dir = tempfile::tempdir().unwrap();
        let skill = |name: &str, body: &str| {
            let at = dir.path().join(name);
            std::fs::create_dir_all(&at).unwrap();
            std::fs::write(at.join("SKILL.md"), body).unwrap();
            describe(Capability::Skills, &at)
        };
        let warned = |d: Described| d.warning.unwrap_or_default();
        assert!(
            warned(skill("empty", "---\nname: empty\ndescription: \n---\n\n"))
                .contains("description")
        );
        assert!(warned(skill("bare", "# Bare\n\nDoes things.\n")).contains("description"));
        assert_eq!(
            skill("whole", "---\nname: whole\ndescription: does x\n---\n").warning,
            None
        );
        let agent = dir.path().join("researcher.md");
        std::fs::write(&agent, "---\nname: researcher\n---\nprose\n").unwrap();
        assert!(warned(describe(Capability::Subagents, &agent)).contains("description"));
        // A rule has no front matter to speak of; its heading is what it says.
        let rule = dir.path().join("house.md");
        std::fs::write(&rule, "# house\n").unwrap();
        assert_eq!(describe(Capability::Rules, &rule).warning, None);
    }

    /// The screen asks, as a value is typed, what the command would refuse
    /// it for — a key nothing reads among them, now that `set` refuses one —
    /// so `↵` twice writes it anyway rather than meeting a refusal that names
    /// a flag the screen cannot pass.
    #[test]
    fn the_screen_asks_what_set_would_refuse() {
        assert!(refusal("idle_timeout", "45m").is_none());
        assert!(refusal("idle_timeout", "soon").is_some());
        let unread = refusal("my_new_key", "1").unwrap_or_default();
        assert!(unread.contains("nothing in omh reads"), "{unread:?}");
    }

    /// A move undone writes the typo back, which is a key nothing reads — so
    /// that line is `--anyway`, since `set` refuses such a key now. The move
    /// itself is not: the key it meant is read, and a value it cannot read is
    /// said rather than written.
    #[test]
    fn a_move_undone_writes_the_typo_back_anyway() {
        let first = |act: &Act| {
            let mut argv = vec!["omh".to_string()];
            argv.extend(command_lines(act).remove(0));
            Cli::try_parse_from(&argv).unwrap().cmd
        };
        let value = "[\".env\"]".to_string();
        let forward = Act::Move {
            from: "carry_ins".into(),
            to: "carry_in".into(),
            value: value.clone(),
        };
        let back = Act::Move {
            from: "carry_in".into(),
            to: "carry_ins".into(),
            value,
        };
        assert!(matches!(
            first(&forward),
            Cmd::Settings {
                cmd: Some(SettingsCmd::Set { anyway: false, .. })
            }
        ));
        assert!(matches!(
            first(&back),
            Cmd::Settings {
                cmd: Some(SettingsCmd::Set { anyway: true, .. })
            }
        ));
    }

    /// A shell's reading of a line `said` wrote: words, single quotes, `'\''`.
    fn unquote(line: &str) -> Vec<String> {
        let (mut words, mut word, mut quoted, mut any) = (Vec::new(), String::new(), false, false);
        for c in line.chars() {
            match c {
                '\'' => {
                    quoted = !quoted;
                    any = true;
                }
                '\\' if !quoted => {}
                ' ' if !quoted => {
                    if any || !word.is_empty() {
                        words.push(std::mem::take(&mut word));
                    }
                    any = false;
                }
                c => word.push(c),
            }
        }
        if any || !word.is_empty() {
            words.push(word);
        }
        words
    }

    /// What the screen ran is said once it closes, where scrollback keeps it,
    /// and each line runs as printed: `omh` on every one — a typo's move was
    /// `omh … && settings unset …` — and `--yes` on a removal, which in a
    /// script refused without it. A server's env values and arguments can be a
    /// token, so they are held back and that is said: never drawn, even here.
    #[test]
    fn what_the_screen_ran_runs_as_printed() {
        let runs = |act: &Act| {
            for ((text, held), line) in ran_lines(act).into_iter().zip(command_lines(act)) {
                assert!(!held, "{text}");
                let words = unquote(&text);
                assert_eq!(words[0], "omh", "{text}");
                let mut want = vec!["omh".to_string()];
                want.extend(line.into_iter().filter(|w| w != "--"));
                let got: Vec<String> = words.into_iter().filter(|w| w != "--").collect();
                assert_eq!(got, want, "{text}");
            }
        };
        runs(&Act::Set {
            key: "carry_in".into(),
            value: "[\".env\", \"a b\"]".into(),
        });
        runs(&Act::Use {
            capability: "skills".into(),
            name: "-x".into(),
        });
        let remove = Act::Remove {
            capability: "skills".into(),
            name: "gone".into(),
        };
        runs(&remove);
        assert!(ran_lines(&remove)[0].0.contains("--yes"));
        let moved = Act::Move {
            from: "carry_ins".into(),
            to: "carry_in".into(),
            value: "[\".env\"]".into(),
        };
        assert_eq!(ran_lines(&moved).len(), 2, "{:?}", ran_lines(&moved));
        runs(&moved);

        let server = ran_lines(&Act::AddServer {
            name: "linear".into(),
            command: "npx".into(),
            args: vec!["-y".into(), "--header".into(), "tok-456".into()],
            env: vec![("LINEAR_TOKEN".into(), "sk-123".into())],
        });
        let (text, held) = &server[0];
        assert!(*held, "{text}");
        let said = format!("{:?}", what_ran(&server).unwrap());
        assert!(said.contains("not shown"), "and that is said: {said}");
        assert!(text.starts_with("omh settings mcp add"), "{text}");
        assert!(
            text.contains("LINEAR_TOKEN"),
            "the name is not the secret: {text}"
        );
        assert!(
            !text.contains("sk-123") && !text.contains("tok-456"),
            "{text}"
        );
    }

    /// The screen shows a line and runs it, so the line has to be one omh
    /// accepts **and** reads the way the act meant — every field, not just
    /// that it parsed. `--env` after the server's command is the case this
    /// exists for: it parses, as an argument to the server.
    #[test]
    fn every_act_is_a_line_omh_accepts_and_means_what_it_says() {
        let set = Act::Set {
            key: "carry_in".into(),
            value: "[\".env\", \"a b\"]".into(),
        };
        assert!(matches!(
            parsed(&set),
            Cmd::Settings { cmd: Some(SettingsCmd::Set { key, value, .. }) }
                if key == "carry_in" && value == "[\".env\", \"a b\"]"
        ));
        assert!(matches!(
            parsed(&Act::Set { key: "runtime".into(), value: "".into() }),
            Cmd::Settings { cmd: Some(SettingsCmd::Set { value, .. }) } if value.is_empty()
        ));
        // A value that looks like a flag is still the value — `--help` worst
        // of all, which clap answers with help and exit 0.
        for value in ["-x", "--help", "--"] {
            assert!(
                matches!(
                    parsed(&Act::Set { key: "runtime".into(), value: value.into() }),
                    Cmd::Settings { cmd: Some(SettingsCmd::Set { value: v, .. }) } if v == value
                ),
                "`{value}` did not arrive as the value"
            );
        }
        // A move is two lines: the key it meant set, the typo unset.
        let moved: Vec<Cmd> = command_lines(&Act::Move {
            from: "carry_ins".into(),
            to: "carry_in".into(),
            value: "[\".env\"]".into(),
        })
        .into_iter()
        .map(|line| {
            let mut argv = vec!["omh".to_string()];
            argv.extend(line);
            Cli::try_parse_from(&argv).unwrap().cmd
        })
        .collect();
        assert!(matches!(
            moved.as_slice(),
            [
                Cmd::Settings { cmd: Some(SettingsCmd::Set { key: set, value, .. }) },
                Cmd::Settings { cmd: Some(SettingsCmd::Unset { key: unset }) },
            ] if set == "carry_in" && value == "[\".env\"]" && unset == "carry_ins"
        ));
        // What new repos take: `settings use` and `unuse`, by kind and name.
        assert!(matches!(
            parsed(&Act::Use { capability: "skills".into(), name: "-x".into() }),
            Cmd::Settings { cmd: Some(SettingsCmd::Use { capability, name: Some(name), all: false }) }
                if capability == "skills" && name == "-x"
        ));
        assert!(matches!(
            parsed(&Act::Unuse { capability: "hooks".into(), name: "rust-test".into() }),
            Cmd::Settings { cmd: Some(SettingsCmd::Unuse { capability, name }) }
                if capability == "hooks" && name == "rust-test"
        ));
        // Anyway is `--anyway`, and a plain set is not.
        assert!(matches!(
            parsed(&Act::SetAnyway { key: "persistence".into(), value: "tmux".into() }),
            Cmd::Settings { cmd: Some(SettingsCmd::Set { key, value, anyway: true }) }
                if key == "persistence" && value == "tmux"
        ));
        assert!(matches!(
            parsed(&set),
            Cmd::Settings {
                cmd: Some(SettingsCmd::Set { anyway: false, .. })
            }
        ));
        assert!(matches!(
            parsed(&Act::Unset { key: "runtime".into() }),
            Cmd::Settings { cmd: Some(SettingsCmd::Unset { key }) } if key == "runtime"
        ));
        assert!(matches!(
            parsed(&Act::EditDefaults),
            Cmd::Settings {
                cmd: Some(SettingsCmd::Edit {
                    capability: None,
                    name: None
                })
            }
        ));
        assert!(matches!(
            parsed(&Act::Edit { capability: "skills".into(), name: Some("tdd".into()) }),
            Cmd::Settings { cmd: Some(SettingsCmd::Edit { capability: Some(c), name: Some(n) }) }
                if c == "skills" && n == "tdd"
        ));
        assert!(matches!(
            parsed(&Act::Remove { capability: "mcp".into(), name: "linear".into() }),
            Cmd::Settings { cmd: Some(SettingsCmd::Rm { capability, name, yes: true }) }
                if capability == "mcp" && name == "linear"
        ));
        let add = Act::AddServer {
            name: "linear".into(),
            command: "npx".into(),
            // `--env` first, where clap would take it for the flag if nothing
            // said the arguments had begun.
            args: vec!["--env".into(), "-y".into(), "--".into(), "x y".into()],
            env: vec![("TOKEN".into(), "a=b".into()), ("MODE".into(), "".into())],
        };
        match parsed(&add) {
            Cmd::Settings {
                cmd:
                    Some(SettingsCmd::Mcp {
                        cmd:
                            McpCmd::Add {
                                name,
                                command,
                                args,
                                env,
                                replace,
                            },
                    }),
            } => {
                // A new server: the form refuses a name already taken, so the
                // screen never writes over one.
                assert!(!replace);
                assert_eq!(name, "linear");
                assert_eq!(command, "npx");
                assert_eq!(args, ["--env", "-y", "--", "x y"]);
                assert_eq!(
                    env,
                    [
                        ("TOKEN".to_string(), "a=b".to_string()),
                        ("MODE".to_string(), String::new())
                    ]
                );
            }
            _ => panic!("`omh {}` is not a server being added", argv(&add).join(" ")),
        }
        assert!(matches!(
            parsed(&Act::Import { capability: "mcp".into(), harness: "claude".into() }),
            Cmd::Settings { cmd: Some(SettingsCmd::Mcp { cmd: McpCmd::Import { harness, file: None, force: false } }) }
                if harness == "claude"
        ));
        assert!(matches!(
            parsed(&Act::Import { capability: "skills".into(), harness: "codex".into() }),
            Cmd::Import { capability, harness, from: None } if capability == "skills" && harness == "codex"
        ));
    }

    fn paths() -> (tempfile::TempDir, Paths) {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths {
            root: dir.path().join("home/.omh"),
            repo: dir.path().join("repo"),
        };
        std::fs::create_dir_all(&paths.root).unwrap();
        std::fs::create_dir_all(paths.repo.join(".git")).unwrap();
        (dir, paths)
    }

    fn page<'m>(model: &'m Model, title: &str) -> &'m Section {
        model
            .sections
            .iter()
            .find(|s| s.title == title)
            .unwrap_or_else(|| panic!("no {title} page"))
    }

    /// **Your catalogue, never this checkout's.** A launch reads both and lets
    /// the repo's hook win; the screen removes what it lists, so listing a
    /// repo's hook here would offer to delete a file in somebody's checkout
    /// under your catalogue's name.
    #[test]
    fn the_screen_lists_your_catalogue_and_never_the_repos_hooks() {
        let (_dir, paths) = paths();
        let hook = r#"{"on":"stop","run":"true","does":"x"}"#;
        std::fs::create_dir_all(paths.root.join("hooks")).unwrap();
        std::fs::write(paths.root.join("hooks/mine.json"), hook).unwrap();
        std::fs::create_dir_all(paths.repo.join(".omh/hooks")).unwrap();
        std::fs::write(paths.repo.join(".omh/hooks/theirs.json"), hook).unwrap();

        let model = model(&paths);
        let names: Vec<&str> = page(&model, "hooks")
            .rows
            .iter()
            .map(|r| r.name.as_str())
            .collect();
        assert_eq!(names, ["mine"]);
    }

    /// A server's environment is where its token goes, and its arguments are
    /// where the rest go — `--header "Authorization: Bearer …"` is how a
    /// remote server is reached. So the screen shows what `omh settings mcp
    /// ls` shows, the command, names the variables and counts the arguments,
    /// and never draws a value of either.
    #[test]
    fn a_servers_env_is_named_and_never_shown() {
        let (_dir, paths) = paths();
        std::fs::write(
            config::mcp_path(&paths),
            r#"{"mcpServers":{"linear":{"command":"npx","args":["mcp-remote","--header","Authorization: Bearer sk-arg-secret"],"env":{"LINEAR_TOKEN":"sk-very-secret"}}}}"#,
        )
        .unwrap();
        let model = model(&paths);
        let servers = page(&model, "servers");
        let row = &servers.rows[0];
        assert!(
            row.about.iter().any(|l| l.contains("LINEAR_TOKEN")),
            "{row:?}"
        );
        assert!(
            row.about.iter().any(|l| l.contains("3 arguments")),
            "{row:?}"
        );
        let everything = format!("{model:?}");
        assert!(!everything.contains("sk-very-secret"), "{everything}");
        assert!(!everything.contains("sk-arg-secret"), "{everything}");
    }

    /// The servers page is yours: each says what it runs in names and counts —
    /// every one read `npx`. omh's own are parts of its features, switched on
    /// your defaults page, and not listed here: switching `codegraph` here read
    /// as turning the server alone off.
    #[test]
    fn servers_are_yours_first_and_say_what_they_run() {
        let (_dir, paths) = paths();
        std::fs::write(
            config::mcp_path(&paths),
            r#"{"mcpServers":{"memory":{"command":"omh","args":["memory","serve"]},"linear":{"command":"npx","args":["-y","mcp-remote","--header","Authorization: Bearer sk-arg"],"env":{"LINEAR_TOKEN":"sk-env"}}}}"#,
        )
        .unwrap();
        let model = model(&paths);
        let servers = page(&model, "servers");
        let rows: Vec<(&str, Option<&str>, Option<&str>)> = servers
            .rows
            .iter()
            .map(|r| (r.name.as_str(), r.group.as_deref(), r.summary.as_deref()))
            .collect();
        // omh's own server is not here at all: it is part of a feature, and a
        // feature is on your defaults page alone.
        assert_eq!(
            rows,
            [("linear", None, Some("npx · 4 args · env LINEAR_TOKEN"))]
        );
        assert!(servers.rows.iter().all(|r| r.badge.is_none()));
        let everything = format!("{model:?}");
        assert!(!everything.contains("sk-arg") && !everything.contains("sk-env"));
    }

    /// A path on the screen is said from the catalogue down, and the header's
    /// from your home: a full path on every row broke mid-word at 100 columns
    /// and said `/Users/first.last/.omh` again on every one.
    #[test]
    fn paths_are_said_from_the_catalogue_down() {
        let (_dir, paths) = paths();
        write(
            &paths.root.join("hooks/mine.json"),
            r#"{"on":"turn-end","run":"true"}"#,
        );
        write(
            &paths.root.join("skills/tdd/SKILL.md"),
            "---\ndescription: x\n---\n",
        );
        write(&paths.root.join("default.toml"), "carry_ins = []\n");
        let model = model(&paths);
        let about = |title: &str, name: &str| {
            page(&model, title)
                .rows
                .iter()
                .find(|r| r.name == name)
                .unwrap()
                .about
                .join("\n")
        };
        assert!(
            about("hooks", "mine").contains("file  hooks/mine.json"),
            "{}",
            about("hooks", "mine")
        );
        assert!(
            about("skills", "tdd").contains("dir   skills/tdd"),
            "{}",
            about("skills", "tdd")
        );
        assert!(about("defaults", "carry_ins").contains("file  default.toml"));
        let everything = format!("{model:?}");
        let root = paths.root.display().to_string();
        assert!(
            !everything
                .replace(&format!("root: {root:?}"), "")
                .contains(&root),
            "a full path is still drawn: {everything}"
        );
        // The header's, from your home.
        let home = paths.root.parent().unwrap();
        assert_eq!(tilde(&paths.root, Some(home)), "~/.omh");
        assert_eq!(tilde(&paths.root, None), root);
    }

    /// A value in the file that omh cannot read is marked on its row, the way
    /// a key nothing reads is — not only when it is typed. Written `--anyway`,
    /// or by hand, `persistence = "tmux"` read as any other value.
    #[test]
    fn a_stored_value_omh_cannot_read_is_marked() {
        let (_dir, paths) = paths();
        write(
            &paths.root.join("default.toml"),
            "persistence = \"tmux\"\nruntime = \"docker\"\n",
        );
        let model = model(&paths);
        let row = |n: &str| {
            page(&model, "defaults")
                .rows
                .iter()
                .find(|r| r.name == n)
                .unwrap()
                .clone()
        };
        let warning = row("persistence").warning.unwrap_or_default();
        assert!(warning.contains("dtach"), "{warning:?}");
        assert_eq!(row("runtime").warning, None);
    }

    /// Every entry says whether the repos `omh init` sets up after get it, and
    /// carries the act that flips it: `[use]` in your defaults for an entry of
    /// yours, the feature's switch for a server omh owns. The page says how
    /// many — "all" while the kind has no list, which is a different fact
    /// from every entry being on one.
    #[test]
    fn every_entry_says_whether_new_repos_get_it() {
        let (_dir, paths) = paths();
        write(
            &paths.root.join("skills/tdd/SKILL.md"),
            "---\ndescription: x\n---\n",
        );
        write(
            &paths.root.join("skills/review-diff/SKILL.md"),
            "---\ndescription: y\n---\n",
        );
        write(&paths.root.join("rules/house.md"), "# house\n");
        write(
            &config::mcp_path(&paths),
            r#"{"mcpServers":{"codegraph":{"command":"codebase-memory-mcp"},"linear":{"command":"npx"}}}"#,
        );
        write(
            &paths.root.join("default.toml"),
            "[use]\nskills = [\"tdd\"]\n\n[omh]\ncodegraph = false\n",
        );
        let model = model(&paths);
        let row = |title: &str, name: &str| {
            page(&model, title)
                .rows
                .iter()
                .find(|r| r.name == name)
                .unwrap_or_else(|| panic!("no {title}/{name}"))
                .clone()
        };
        let skill = row("skills", "tdd");
        assert_eq!(skill.seeded, Some(true));
        assert_eq!(
            skill.toggle,
            Some(Act::Unuse {
                capability: "skills".into(),
                name: "tdd".into()
            })
        );
        let skill = row("skills", "review-diff");
        assert_eq!(skill.seeded, Some(false));
        assert_eq!(
            skill.toggle,
            Some(Act::Use {
                capability: "skills".into(),
                name: "review-diff".into()
            })
        );
        assert_eq!(
            page(&model, "skills").seeds.as_deref(),
            Some("new repos get 1 of 2")
        );
        // A kind with no list: every entry, said as "all".
        assert_eq!(row("rules", "house").seeded, Some(true));
        assert_eq!(
            page(&model, "rules").seeds.as_deref(),
            Some("new repos get all")
        );
        // A server omh owns is part of its feature, and a feature is switched
        // in one place — your defaults, with its hooks and rules. On the
        // servers page it had its own ● and `space`, which ran the same switch
        // and read as turning the server alone off: it is not listed there.
        assert!(page(&model, "servers")
            .rows
            .iter()
            .all(|r| r.name != "codegraph"));
        assert_eq!(row("servers", "linear").seeded, Some(true));
        assert_eq!(
            page(&model, "servers").seeds.as_deref(),
            Some("new repos get all")
        );
        // Your defaults list the features, switched for new repos, under their
        // own heading — not as an `[omh]` table.
        let defaults = page(&model, "defaults");
        let feature = defaults
            .rows
            .iter()
            .find(|r| r.name == "codegraph")
            .unwrap();
        assert_eq!(feature.value.as_deref(), Some("off"));
        assert_eq!(feature.group.as_deref(), Some("features"));
        assert_eq!(feature.edit, Edit::Choice(vec!["on".into(), "off".into()]));
        assert!(defaults.rows.iter().all(|r| r.name != "[omh]"));
    }

    /// The marks are what `omh init` would write, asked of the same function:
    /// a guard is not taken unless a list names it, and what `[unuse]` leaves
    /// out is not taken, while the rest of the kind still follows the
    /// catalogue. A guard was drawn ● here and seeded by nobody, until a
    /// `space` on another hook wrote it into a list.
    #[test]
    fn a_guard_and_what_you_left_out_are_not_marked_taken() {
        let (_dir, paths) = paths();
        write(
            &paths.root.join("hooks/mine.json"),
            r#"{"on":"turn-end","run":"make check"}"#,
        );
        write(
            &paths.root.join("hooks/other.json"),
            r#"{"on":"turn-end","run":"make lint"}"#,
        );
        write(
            &paths.root.join("hooks/my-guard.json"),
            r#"{"on":"before-tool","tools":["edit"],"refuse":"no"}"#,
        );
        let marks = |paths: &Paths| {
            let model = model(paths);
            let hooks = page(&model, "hooks").clone();
            let of = |name: &str| {
                hooks
                    .rows
                    .iter()
                    .find(|r| r.name == name)
                    .unwrap_or_else(|| panic!("no hooks/{name}"))
                    .clone()
            };
            (of("mine"), of("other"), of("my-guard"), hooks.seeds.clone())
        };
        let (mine, _, guard, seeds) = marks(&paths);
        assert_eq!(mine.seeded, Some(true));
        assert_eq!(guard.seeded, Some(false), "nobody named it");
        assert_eq!(
            guard.toggle,
            Some(Act::Use {
                capability: "hooks".into(),
                name: "my-guard".into()
            })
        );
        assert!(
            guard.about.iter().any(|l| l.contains("a guard")),
            "{:?}",
            guard.about
        );
        assert_eq!(
            seeds.as_deref(),
            Some("new repos get all but the guards, by stack")
        );

        // A file of yours named as one of omh's own is its feature's, switched
        // on your defaults page: `use` refuses it, so `space` does not offer it.
        write(
            &paths.root.join("hooks/graph-first.json"),
            r#"{"on":"turn-end","run":"true"}"#,
        );
        let drawn = model(&paths);
        let owned = page(&drawn, "hooks")
            .rows
            .iter()
            .find(|r| r.name == "graph-first")
            .unwrap()
            .clone();
        assert_eq!((owned.seeded, owned.toggle), (None, None));
        assert!(
            owned
                .about
                .iter()
                .any(|l| l.contains("omh settings set codegraph")),
            "{:?}",
            owned.about
        );
        std::fs::remove_file(paths.root.join("hooks/graph-first.json")).unwrap();
        // Owned is per kind: `memory` is omh's server, and a hook of yours.
        write(
            &paths.root.join("hooks/memory.json"),
            r#"{"on":"turn-end","run":"true"}"#,
        );
        let again = model(&paths);
        let yours = page(&again, "hooks")
            .rows
            .iter()
            .find(|r| r.name == "memory")
            .unwrap()
            .clone();
        assert_eq!(yours.seeded, Some(true));
        std::fs::remove_file(paths.root.join("hooks/memory.json")).unwrap();

        write(
            &paths.root.join("default.toml"),
            "[unuse]\nhooks = [\"other\"]\n",
        );
        let (mine, other, guard, seeds) = marks(&paths);
        assert_eq!(
            mine.seeded,
            Some(true),
            "the rest still follows the catalogue"
        );
        assert_eq!(other.seeded, Some(false));
        assert_eq!(guard.seeded, Some(false));
        assert_eq!(
            seeds.as_deref(),
            Some("new repos get all but 1, and no guards, by stack")
        );
    }

    /// What undoes a flip is worked out with it, from what the flip writes:
    /// the opposite verb where that puts the file back, and nothing where it
    /// does not — a guard taken by a kind with no list writes the kind as one,
    /// and a rule taken off the middle of its list would come back at the end,
    /// where rules read in another order.
    #[test]
    fn what_undoes_a_flip_is_worked_out_with_it() {
        let (_dir, paths) = paths();
        for (at, body) in [
            ("hooks/mine.json", r#"{"on":"turn-end","run":"make check"}"#),
            ("hooks/other.json", r#"{"on":"turn-end","run":"make lint"}"#),
            (
                "hooks/my-guard.json",
                r#"{"on":"before-tool","tools":["edit"],"refuse":"no"}"#,
            ),
            ("rules/a.md", "# a\n"),
            ("rules/b.md", "# b\n"),
            ("rules/c.md", "# c\n"),
        ] {
            write(&paths.root.join(at), body);
        }
        write(
            &paths.root.join("default.toml"),
            "[use]\nrules = [\"a\", \"b\"]\n\n[unuse]\nhooks = [\"other\"]\n",
        );
        let model = model(&paths);
        let back = |title: &str, name: &str| {
            page(&model, title)
                .rows
                .iter()
                .find(|r| r.name == name)
                .unwrap_or_else(|| panic!("no {title}/{name}"))
                .untoggle
                .clone()
        };
        let act = |use_it: bool, capability: &str, name: &str| {
            let (capability, name) = (capability.to_string(), name.to_string());
            Some(match use_it {
                true => Act::Use { capability, name },
                false => Act::Unuse { capability, name },
            })
        };
        assert_eq!(back("hooks", "mine"), act(true, "hooks", "mine"));
        assert_eq!(back("hooks", "other"), act(false, "hooks", "other"));
        assert_eq!(back("hooks", "my-guard"), None, "a list, written whole");
        assert_eq!(back("rules", "a"), None, "a would come back after b");
        assert_eq!(back("rules", "b"), act(true, "rules", "b"));
        assert_eq!(back("rules", "c"), act(false, "rules", "c"));
    }

    /// `[use]` naming an entry your catalogue does not have is marked, as
    /// `omh init` would warn about it — after an `rm`, say.
    #[test]
    fn a_use_list_naming_nothing_is_marked() {
        let (_dir, paths) = paths();
        write(&paths.root.join("skills/tdd/SKILL.md"), "x");
        write(
            &paths.root.join("default.toml"),
            "[use]\nskills = [\"tdd\", \"gone\"]\n",
        );
        let model = model(&paths);
        let row = page(&model, "defaults")
            .rows
            .iter()
            .find(|r| r.name == "[use]")
            .unwrap()
            .clone();
        let warning = row.warning.unwrap_or_default();
        assert!(warning.contains("skills/gone"), "{warning:?}");
        assert!(
            row.about.iter().any(|l| l.contains("omh init")),
            "{:?}",
            row.about
        );
    }

    /// Each list says what is wrong with it, and what `omh init` does about
    /// it: warns of a name your catalogue does not have in `[use]`, refuses a
    /// kind that is not one — and of `[unuse]` naming nothing, says nothing,
    /// since leaving out nothing is harmless. Both rows carried one warning,
    /// both tables' together, ending "`omh init` will warn" for all of it.
    #[test]
    fn each_list_says_what_init_does_with_what_it_cannot_mean() {
        let (_dir, paths) = paths();
        write(&paths.root.join("skills/tdd/SKILL.md"), "x");
        write(
            &paths.root.join("default.toml"),
            "[use]\nskills = [\"tdd\", \"gone\"]\nskill = [\"x\"]\n\n\
             [unuse]\nrules = [\"gone\"]\n",
        );
        let model = model(&paths);
        let warning = |name: &str| {
            page(&model, "defaults")
                .rows
                .iter()
                .find(|r| r.name == name)
                .unwrap()
                .warning
                .clone()
                .unwrap_or_default()
        };
        let used = warning("[use]");
        assert!(
            used.contains("skills/gone") && used.contains("will warn"),
            "{used}"
        );
        assert!(
            used.contains("[use] skill,") && used.contains("refuses"),
            "{used}"
        );
        assert!(!used.contains("skills/tdd"), "tdd is there: {used}");
        assert!(!used.contains("rules/gone"), "{used}");
        let unused = warning("[unuse]");
        assert!(unused.contains("rules/gone"), "{unused}");
        assert!(unused.contains("leaves out nothing"), "{unused}");
        assert!(
            !unused.contains("will warn") && !unused.contains("skills/gone"),
            "{unused}"
        );
    }

    /// An import says where each harness reads: the adapter's path on this
    /// host, and for a server Claude Code keeps per project, that it is read
    /// from where omh was started.
    #[test]
    fn an_import_says_where_each_harness_reads() {
        let (_dir, paths) = paths();
        seed_adapters(&paths);
        let model = model(&paths);
        let from = |title: &str, harness: &str| match &page(&model, title).import {
            Import::From(sources) => sources
                .iter()
                .find(|s| s.harness == harness)
                .map(|s| s.from.clone())
                .unwrap_or_else(|| panic!("no {harness} for {title}")),
            Import::Not(why) => panic!("{title}: {why}"),
        };
        assert!(
            from("skills", "claude").contains(".claude/skills"),
            "{}",
            from("skills", "claude")
        );
        let servers = from("servers", "claude");
        assert!(
            servers.contains("./.mcp.json") && servers.contains("where omh was started"),
            "{servers}"
        );
        // The checkout here has none, and says so rather than looking empty.
        assert!(servers.contains("nothing there"), "{servers}");
    }

    /// **Could not look is not none.** A catalogue directory omh cannot read
    /// is said on its page, and the page is not drawn as an empty kind.
    #[cfg(unix)]
    #[test]
    fn a_catalogue_directory_omh_cannot_read_is_said_not_shown_empty() {
        use std::os::unix::fs::PermissionsExt;
        if unsafe { libc::geteuid() } == 0 {
            return; // root reads through 0o000, so nothing would be tested
        }
        let (_dir, paths) = paths();
        let skills = paths.root.join("skills");
        std::fs::create_dir_all(skills.join("tdd")).unwrap();
        let was = std::fs::metadata(&skills).unwrap().permissions().mode();
        std::fs::set_permissions(&skills, std::fs::Permissions::from_mode(0o000)).unwrap();
        let model = model(&paths);
        std::fs::set_permissions(&skills, std::fs::Permissions::from_mode(was)).unwrap();

        let skills = page(&model, "skills");
        assert!(
            skills
                .problem
                .as_deref()
                .is_some_and(|p| p.contains("skills")),
            "{skills:?}"
        );
        // And an absent directory is simply a kind you have none of.
        assert_eq!(page(&model, "commands").problem, None);
    }

    /// Every key omh reads is a row, set or not — the half no file can show —
    /// and what the file holds besides is named for what it is.
    #[test]
    fn every_key_omh_reads_is_a_row_and_the_rest_is_named() {
        let (_dir, paths) = paths();
        std::fs::write(
            config::Layer::Personal.file(&paths),
            "idle_timeout = \"45m\"\ncarry_ins = []\n[use]\nskills = []\n",
        )
        .unwrap();
        let model = model(&paths);
        let defaults = page(&model, "defaults");
        for k in key::KEYS {
            assert!(defaults.rows.iter().any(|r| r.name == k.name), "{}", k.name);
        }
        let row = |n: &str| defaults.rows.iter().find(|r| r.name == n).unwrap();
        assert_eq!(row("idle_timeout").value.as_deref(), Some("45m"));
        // A key nothing reads is said to be one — and, near a key omh does
        // read, which one it was probably meant to be.
        assert_eq!(
            row("carry_ins").warning.as_deref(),
            Some("nothing omh reads — did you mean `carry_in`?")
        );
        assert_eq!(row("carry_ins").meant.as_deref(), Some("carry_in"));
        // A table holds something, and says what: `[use]` was drawn unset.
        assert_eq!(row("[use]").edit, Edit::None);
        assert_eq!(row("[use]").value.as_deref(), Some("skills = []"));
        // Every unset key says what applies instead, from the table — but
        // `account`, which is worked out from the logins there are.
        for k in key::KEYS.iter().filter(|k| k.name != "account") {
            assert_eq!(row(k.name).fallback.as_deref(), Some(k.unset), "{}", k.name);
        }
    }

    /// A key with a fixed set of values is picked from exactly that set — the
    /// table's, not a copy of it.
    #[test]
    fn a_key_with_choices_is_picked_from_the_tables_choices() {
        let (_dir, paths) = paths();
        let model = model(&paths);
        let defaults = page(&model, "defaults");
        for k in key::KEYS {
            let row = defaults.rows.iter().find(|r| r.name == k.name).unwrap();
            match k.shape {
                key::Shape::Choice(allowed) => assert_eq!(
                    row.edit,
                    Edit::Choice(allowed.iter().map(|a| a.to_string()).collect()),
                    "{}",
                    k.name
                ),
                _ => assert!(
                    matches!(row.edit, Edit::Text { .. } | Edit::Choice(_)),
                    "{} is typed, or picked from what there is",
                    k.name
                ),
            }
        }
    }

    /// The shipped adapters, where `Paths::adapters()` looks.
    fn seed_adapters(paths: &Paths) {
        let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("adapters");
        std::fs::create_dir_all(paths.adapters()).unwrap();
        for entry in std::fs::read_dir(src).unwrap() {
            let entry = entry.unwrap();
            std::fs::copy(entry.path(), paths.adapters().join(entry.file_name())).unwrap();
        }
    }

    /// `account` offers the logins captured, by name — what `omh auth`
    /// leaves behind, the file claude's adapter names as proof.
    #[test]
    fn account_offers_the_logins_captured() {
        let (_dir, paths) = paths();
        seed_adapters(&paths);
        let token = paths.creds("claude").join("work/.claude/.credentials.json");
        std::fs::create_dir_all(token.parent().unwrap()).unwrap();
        // Not `{}`, which `auth` reads as a placeholder and not a login.
        std::fs::write(&token, r#"{"token":"seeded"}"#).unwrap();
        let model = model(&paths);
        let row = page(&model, "defaults")
            .rows
            .iter()
            .find(|r| r.name == "account")
            .unwrap();
        // Every value a launch takes, and only those: `another…` let one be
        // typed, and every login there is is already here — `claude:work`
        // for one harness's, which was what typing was for.
        assert_eq!(
            row.edit,
            Edit::Choice(vec!["work".into(), "claude:work".into()])
        );
        assert_eq!(row.warning, None);
        assert_eq!(
            row.fallback.as_deref(),
            Some("work, the only login claude has")
        );
    }

    /// Unset, `account` is what a launch will do, worked out from the logins
    /// there are: it said "the only login captured" with two captured, where
    /// a launch refuses to guess between them.
    #[test]
    fn an_unset_account_says_what_a_launch_will_do() {
        let (_dir, paths) = paths();
        seed_adapters(&paths);
        let account = |paths: &Paths| {
            let model = model(paths);
            let row = page(&model, "defaults")
                .rows
                .iter()
                .find(|r| r.name == "account")
                .unwrap()
                .clone();
            (row.fallback.unwrap_or_default(), row.warning)
        };
        let (fallback, warning) = account(&paths);
        assert!(
            fallback.contains("no login captured") && fallback.contains("omh auth"),
            "{fallback}"
        );
        assert_eq!(warning, None, "none yet is where everybody starts");
        for name in ["work", "personal"] {
            let token = paths
                .creds("claude")
                .join(name)
                .join(".claude/.credentials.json");
            std::fs::create_dir_all(token.parent().unwrap()).unwrap();
            std::fs::write(&token, r#"{"token":"seeded"}"#).unwrap();
        }
        let (fallback, warning) = account(&paths);
        let warning = warning.unwrap_or_default();
        assert!(
            warning.contains("claude has 2 (personal, work)") && warning.contains("refuses"),
            "{warning}"
        );
        assert!(fallback.contains("none chosen"), "{fallback}");
    }

    /// Logins omh could not list are said on the row, not taken for none:
    /// the row is still typed into, and says why it offers nothing.
    #[test]
    fn logins_omh_could_not_list_are_said() {
        let (_dir, paths) = paths();
        seed_adapters(&paths);
        // A file where the harness's directory of logins goes: NotADirectory,
        // which is not a kind of absence.
        std::fs::create_dir_all(paths.root.join("creds")).unwrap();
        std::fs::write(paths.creds("claude"), "").unwrap();
        let model = model(&paths);
        let row = page(&model, "defaults")
            .rows
            .iter()
            .find(|r| r.name == "account")
            .unwrap();
        assert!(matches!(row.edit, Edit::Text { .. }), "{:?}", row.edit);
        let warning = row.warning.as_deref().unwrap_or_default();
        assert!(
            warning.contains("could not list the logins") && warning.contains("creds/claude"),
            "{warning:?}"
        );
    }

    /// Hooks are yours first and then omh's, each saying when it runs and
    /// what it does, with its stack as a badge.
    #[test]
    fn hooks_are_yours_first_and_say_when_they_run() {
        let (_dir, paths) = paths();
        for (name, body) in [
            (
                "rust-test",
                r#"{"on":"turn-end","stack":"rust","run":"cargo test"}"#,
            ),
            (
                "mine",
                r#"{"on":"before-tool","tools":["edit"],"run":"make check"}"#,
            ),
        ] {
            write(&paths.root.join(format!("hooks/{name}.json")), body);
        }
        let model = model(&paths);
        let hooks = page(&model, "hooks");
        let names: Vec<&str> = hooks.rows.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(names, ["mine", "rust-test"]);
        assert_eq!(hooks.rows[0].group.as_deref(), Some("yours"));
        assert_eq!(hooks.rows[1].group.as_deref(), Some("shipped with omh"));
        // When it runs is a column of its own, the one somebody scans first:
        // what it does starts at the same place on every row.
        assert_eq!(
            hooks.rows[1].summary.as_deref(),
            Some("turn-end          cargo test")
        );
        assert_eq!(
            hooks.rows[0].summary.as_deref(),
            Some("before-tool edit  make check")
        );
        assert_eq!(hooks.rows[1].badge.as_deref(), Some("rust"));
    }

    /// Every page of your catalogue says what belongs on it, for when there is
    /// nothing on it yet; and a key nothing reads is under its own heading,
    /// apart from the ones omh does.
    #[test]
    fn a_page_says_what_belongs_on_it_and_unread_keys_stand_apart() {
        let (_dir, paths) = paths();
        write(&paths.root.join("default.toml"), "carry_ins = []\n");
        let model = model(&paths);
        for section in model.sections.iter().filter(|s| s.page != Page::Defaults) {
            assert!(!section.about.is_empty(), "{} says nothing", section.title);
        }
        let defaults = page(&model, "defaults");
        let unread = defaults
            .rows
            .iter()
            .find(|r| r.name == "carry_ins")
            .unwrap();
        assert_eq!(unread.group.as_deref(), Some("read by nothing"));
        assert!(defaults
            .rows
            .iter()
            .filter(|r| key::describes(&r.name).is_some())
            .all(|r| r.group.is_none()));
    }

    /// A link to nothing is an entry no harness can read, and says so.
    #[cfg(unix)]
    #[test]
    fn a_link_to_nothing_says_so() {
        let (_dir, paths) = paths();
        std::fs::create_dir_all(paths.root.join("rules")).unwrap();
        std::os::unix::fs::symlink(
            paths.root.join("gone.md"),
            paths.root.join("rules/house.md"),
        )
        .unwrap();
        let model = model(&paths);
        let row = &page(&model, "rules").rows[0];
        assert!(
            row.warning
                .as_deref()
                .is_some_and(|w| w.contains("a link to nothing")),
            "{row:?}"
        );
    }

    /// What `rm` refuses to remove, the screen marks as fixed or leaves out: a
    /// hook omh ships is fixed, and omh's own servers are not on the servers
    /// page at all — they are its features', on your defaults page.
    #[test]
    fn what_rm_refuses_is_marked_fixed() {
        let (_dir, paths) = paths();
        let shipped = settings::shipped_servers().unwrap();
        assert!(shipped.contains_key("codegraph") && shipped.contains_key("memory"));
        let mut servers = serde_json::Map::new();
        for name in shipped.keys() {
            servers.insert(name.clone(), serde_json::json!({ "command": "true" }));
        }
        servers.insert("linear".into(), serde_json::json!({ "command": "npx" }));
        write(
            &config::mcp_path(&paths),
            &serde_json::json!({ "mcpServers": servers }).to_string(),
        );
        let hook = settings::shipped_hooks()[0].clone();
        let body = r#"{"on":"stop","run":"true","does":"x"}"#;
        write(&paths.root.join(format!("hooks/{hook}.json")), body);
        write(&paths.root.join("hooks/mine.json"), body);
        let model = model(&paths);
        let fixed = |title: &str, name: &str| {
            page(&model, title)
                .rows
                .iter()
                .find(|r| r.name == name)
                .unwrap_or_else(|| panic!("no {title}/{name}"))
                .fixed
        };
        let servers: Vec<&str> = page(&model, "servers")
            .rows
            .iter()
            .map(|r| r.name.as_str())
            .collect();
        assert_eq!(servers, ["linear"], "omh's own are its features'");
        assert!(!fixed("servers", "linear"));
        assert!(fixed("hooks", &hook), "hooks/{hook}");
        assert!(!fixed("hooks", "mine"));
    }

    /// What an edit left is read back, and said: the editor closing is not
    /// the entry being one. "✓ editor closed" sat beside a hook the row one
    /// column over called not a hook omh can run.
    #[test]
    fn what_an_edit_left_is_read_back() {
        let (_dir, paths) = paths();
        write(&paths.root.join("hooks/mine.json"), "{ \"on\": ");
        write(
            &paths.root.join("hooks/fine.json"),
            r#"{"on":"turn-end","run":"true"}"#,
        );
        let model = model(&paths);
        let broken = after_edit(&model, Some("hooks"), Some("mine")).unwrap_or_default();
        assert!(
            broken.contains("hooks/mine") && broken.contains("not a hook omh can run"),
            "{broken}"
        );
        assert_eq!(after_edit(&model, Some("hooks"), Some("fine")), None);
        write(&paths.root.join("default.toml"), "idle_timeout = \n");
        let model = super::model(&paths);
        let defaults = after_edit(&model, None, None).unwrap_or_default();
        assert!(defaults.contains("default.toml"), "{defaults}");
        // A hook omh ships is read back whole, and `omh upgrade` puts its own
        // back over an edit.
        let shipped = settings::shipped_hooks()[0].clone();
        write(
            &paths.root.join(format!("hooks/{shipped}.json")),
            r#"{"on":"turn-end","run":"true"}"#,
        );
        let model = super::model(&paths);
        let note = after_edit(&model, Some("hooks"), Some(&shipped)).unwrap_or_default();
        assert!(
            note.contains("omh upgrade") && note.contains(&format!("{shipped}.json.yours")),
            "{note}"
        );
    }

    fn write(path: &std::path::Path, body: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, body).unwrap();
    }

    /// Every page names a kind a command accepts, and the servers page is the
    /// servers page.
    ///
    /// The screen builds every act from a page's capability, so a key spelled
    /// here that `from_key` does not know is a page whose every action is
    /// refused — and the servers page, read as an entries page, offers
    /// `settings edit mcp <name>`.
    #[test]
    fn every_page_names_a_kind_omh_accepts() {
        let (_dir, paths) = paths();
        let model = model(&paths);
        for section in &model.sections {
            if let Some(key) = section.page.capability() {
                assert!(
                    Capability::from_key(key).is_some(),
                    "{}: `{key}` is not a capability",
                    section.title
                );
            }
        }
        assert_eq!(page(&model, "servers").page, Page::Servers);
        assert_eq!(page(&model, "defaults").page, Page::Defaults);
        assert_eq!(
            model
                .sections
                .iter()
                .filter(|s| matches!(s.page, Page::Entries(_)))
                .count(),
            Capability::ALL.len() - 1
        );
    }

    /// The hooks page imports nothing, and says where hooks are imported.
    ///
    /// `omh import hooks` writes into the repo's `.omh/hooks/`, and this screen
    /// is your catalogue alone.
    #[test]
    fn the_hooks_page_imports_nothing_and_says_where_to() {
        let (_dir, paths) = paths();
        let model = model(&paths);
        match &page(&model, "hooks").import {
            Import::Not(why) => assert!(why.contains("omh import hooks"), "{why}"),
            Import::From(h) => panic!("the hooks page offers an import from {h:?}"),
        }
    }

    /// **Could not look is not none**, on every page that reads a file: a
    /// defaults file, a servers file and the adapters that will not read are
    /// said, never drawn as nothing set or nothing to import.
    #[test]
    fn every_read_the_screen_makes_is_said_when_it_fails() {
        let (_dir, broken) = paths();
        write(
            &config::Layer::Personal.file(&broken),
            "idle_timeout = [unclosed\n",
        );
        write(&config::mcp_path(&broken), "{ not json");
        let read = model(&broken);
        assert!(
            page(&read, "defaults").problem.is_some(),
            "a defaults file that will not parse"
        );
        // By name, the file that would not parse.
        assert!(
            page(&read, "servers")
                .problem
                .as_deref()
                .is_some_and(|p| p.contains("mcp.json")),
            "a servers file that will not parse: {:?}",
            page(&read, "servers").problem
        );
        assert!(page(&read, "servers").rows.is_empty());

        let (_dir2, unsure) = paths();
        write(
            &config::mcp_path(&unsure),
            r#"{"mcpServers":{"codegraph":{"command":"codebase-memory-mcp"}}}"#,
        );
        write(&unsure.base().join("2026.08.toml"), "[[entry]\n");
        write(&unsure.adapters().join("claude.toml"), "name = [unclosed\n");
        let read = model(&unsure);
        let servers = page(&read, "servers");
        // Which servers are omh's is asked of the binary, as `rm` asks it, so
        // an unreadable base set under `~/.omh` leaves it known — and omh's own
        // is its feature's, on your defaults page, not a row here.
        assert_eq!(servers.problem, None, "{servers:?}");
        assert!(servers.rows.is_empty(), "{servers:?}");
        for title in ["skills", "rules", "servers"] {
            match &page(&read, title).import {
                Import::Not(why) => assert!(why.contains("adapters"), "{title}: {why}"),
                Import::From(h) => panic!("{title} offers an import from {h:?} it could not read"),
            }
        }
    }

    /// A skill with no `SKILL.md` says so, rather than looking like any other.
    #[test]
    fn a_skill_with_no_skill_md_says_nothing_will_load_it() {
        let (_dir, paths) = paths();
        std::fs::create_dir_all(paths.root.join("skills/half")).unwrap();
        let model = model(&paths);
        let row = &page(&model, "skills").rows[0];
        assert!(
            row.warning
                .as_deref()
                .is_some_and(|w| w.contains("no SKILL.md")),
            "{row:?}"
        );
    }

    fn drawn(app: &App) -> String {
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(120, 40)).unwrap();
        terminal.draw(|f| app.draw(f)).unwrap();
        let buffer = terminal.backend().buffer().clone();
        buffer
            .content()
            .chunks(120)
            .map(|row| row.iter().map(|c| c.symbol()).collect::<String>())
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// A child that fails saying nothing — killed by a signal, say — is
    /// reported with how it ended, not as a bare "failed".
    #[cfg(unix)]
    #[test]
    fn a_child_that_fails_saying_nothing_is_reported_with_how_it_ended() {
        use std::os::unix::process::ExitStatusExt;
        let mut app = App::new(Model::default(), false);
        let out = std::process::Output {
            status: std::process::ExitStatus::from_raw(9),
            stdout: Vec::new(),
            stderr: Vec::new(),
        };
        report(&mut app, &argv(&Act::Unset { key: "k".into() }), &out);
        let screen = drawn(&app);
        assert!(screen.contains("signal"), "{screen}");
    }

    /// A child that will not start is an error on the screen, and the line it
    /// names is the line as drawn — never one carrying the values `shown`
    /// hides.
    #[test]
    fn a_child_that_will_not_start_names_the_line_as_drawn() {
        let add = Act::AddServer {
            name: "linear".into(),
            command: "npx".into(),
            args: vec!["Authorization: Bearer sk-arg-secret".into()],
            env: vec![("LINEAR_TOKEN".into(), "sk-very-secret".into())],
        };
        let e = omh(std::path::Path::new("/nonexistent/omh"), &argv(&add))
            .expect_err("there is no omh there");
        let said = format!("{e:#}");
        assert!(said.contains("running omh settings mcp add"), "{said}");
        assert!(!said.contains("sk-very-secret"), "{said}");
        assert!(!said.contains("sk-arg-secret"), "{said}");
    }

    /// The line shown is the one you would type: not the screen's own
    /// `--dry-run` preview, which read like an instruction under a refusal,
    /// nor the `--yes` it adds because it already asked. Typed at a terminal,
    /// that line asks the same question or meets the same refusal.
    #[test]
    fn the_line_shown_is_the_one_you_would_type() {
        let line: Vec<String> = [
            "--dry-run",
            "settings",
            "rm",
            "--yes",
            "--",
            "mcp",
            "codegraph",
        ]
        .map(String::from)
        .to_vec();
        assert_eq!(shown(&line), "settings rm mcp codegraph");
        let rm = argv(&Act::Remove {
            capability: "skills".into(),
            name: "tdd".into(),
        });
        assert!(rm.iter().any(|w| w == "--yes"), "what runs still answers");
        assert_eq!(shown(&rm), "settings rm skills tdd");
    }

    /// The line shown for a new server names its environment and hides the
    /// values, and counts its arguments rather than drawing them: it stays on
    /// the screen where anybody looking can read it, and those are where a
    /// token goes.
    #[test]
    fn the_line_shown_for_a_server_hides_its_env_and_its_arguments() {
        let add = Act::AddServer {
            name: "linear".into(),
            command: "npx".into(),
            args: vec![
                "--header".into(),
                "Authorization: Bearer sk-arg-secret".into(),
            ],
            env: vec![("LINEAR_TOKEN".into(), "sk-very-secret".into())],
        };
        let shown = shown(&argv(&add));
        assert!(shown.contains("LINEAR_TOKEN="), "{shown}");
        assert!(shown.contains("linear npx"), "{shown}");
        assert!(!shown.contains("sk-very-secret"), "{shown}");
        assert!(!shown.contains("sk-arg-secret"), "{shown}");
        // And it is only the shown line: what runs still carries both.
        let line = argv(&add);
        assert!(line.iter().any(|w| w == "LINEAR_TOKEN=sk-very-secret"));
        assert!(line.iter().any(|w| w.contains("sk-arg-secret")));
    }

    /// What the screen prints as the line it ran is a line a shell reads back
    /// into the same words.
    #[test]
    fn the_line_shown_is_quoted_the_way_a_shell_reads_it() {
        let act = Act::Set {
            key: "carry_in".into(),
            value: "[\".env\"]".into(),
        };
        assert_eq!(shown(&argv(&act)), r#"settings set carry_in '[".env"]'"#);
        // The `--` is kept where it is the difference.
        assert_eq!(
            shown(&argv(&Act::Set {
                key: "k".into(),
                value: "-x".into()
            })),
            "settings set -- k -x"
        );
        for value in ["it's", "", "a b", "$HOME", "x\\y"] {
            let line = argv(&Act::Set {
                key: "k".into(),
                value: value.into(),
            });
            assert_eq!(
                crate::tui::words(&said(&line)).unwrap(),
                line,
                "`{}` does not read back as what ran",
                said(&line)
            );
        }
    }
}
