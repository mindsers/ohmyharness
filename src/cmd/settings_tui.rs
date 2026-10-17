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
use crate::tui::{Act, App, Import, Model, Next, Page, Row, Section};
use crate::{base, config, hook, key};
use anyhow::{Context, Result};
use ratatui::crossterm::event::{self, Event, KeyEventKind};

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

pub(crate) fn run(paths: &Paths, color: bool) -> Result<()> {
    let mut app = App::new(model(paths), color);
    let mut screen = Screen::open()?;
    drive(&mut app, &mut screen.terminal, paths)
}

/// The terminal while the screen has it, given back however this ends.
///
/// A guard rather than a `restore()` after the loop, because the loop is not
/// the only way out: `try_init` turns raw mode on *before* it enters the
/// alternate screen and asks the terminal its size, so a failure in either
/// returned with raw mode on, and the shell somebody came back to needed
/// `reset`. ratatui's panic hook covers a panic; this covers every `?`.
struct Screen {
    terminal: ratatui::DefaultTerminal,
}

impl Screen {
    fn open() -> Result<Self> {
        match ratatui::try_init() {
            Ok(terminal) => Ok(Self { terminal }),
            Err(e) => {
                ratatui::restore();
                Err(e).context("opening the screen")
            }
        }
    }
}

impl Drop for Screen {
    fn drop(&mut self) {
        ratatui::restore();
    }
}

fn drive(app: &mut App, terminal: &mut ratatui::DefaultTerminal, paths: &Paths) -> Result<()> {
    let exe = std::env::current_exe().context("finding omh")?;
    loop {
        terminal.draw(|f| app.draw(f))?;
        // A resize, a focus change, a paste: none is a key, and the next draw
        // is the whole answer to each.
        let Event::Key(key) = event::read()? else {
            continue;
        };
        if key.kind != KeyEventKind::Press {
            continue;
        }
        match app.key(key) {
            Next::Stay => {}
            Next::Quit => return Ok(()),
            Next::Preview(act) => {
                let mut line = vec!["--dry-run".to_string()];
                line.extend(argv(&act));
                match omh(&exe, &line) {
                    Ok(out) if out.status.success() => {
                        app.confirm(lines(&out.stdout), lines(&out.stderr), act);
                    }
                    // The line that refused is the preview, so that is the one
                    // shown — not the removal nobody ran.
                    Ok(out) => report(app, &line, &out),
                    Err(e) => app.report(shown(&line), false, "", &format!("{e:#}")),
                }
            }
            Next::Run(act) => {
                let line = argv(&act);
                match &act {
                    Act::EditDefaults => edit(app, terminal, paths, None, None, &line)?,
                    Act::Edit { capability, name } => edit(
                        app,
                        terminal,
                        paths,
                        Some(capability),
                        name.as_deref(),
                        &line,
                    )?,
                    _ => match omh(&exe, &line) {
                        Ok(out) => report(app, &line, &out),
                        // On the screen rather than out of it: a child that
                        // would not start is one failed act, and leaving took
                        // every unsaved thing on the screen with it.
                        Err(e) => app.report(shown(&line), false, "", &format!("{e:#}")),
                    },
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
) -> Result<()> {
    let target = match settings::edit_target(paths, capability, name, config::Layer::Personal) {
        Ok(target) => target,
        Err(e) => {
            app.report(shown(line), false, "", &format!("{e:#}"));
            return Ok(());
        }
    };
    suspend()?;
    let edited = settings::open_in_editor(&target);
    // If the screen cannot come back, what the editor did still reaches the
    // person, in the error that ends the screen.
    resume(terminal).with_context(|| match &edited {
        Ok(_) => format!("after `omh {}`, which worked", shown(line)),
        Err(e) => format!("after `omh {}`, which failed: {e:#}", shown(line)),
    })?;
    match edited {
        Ok(Edited::Done) => app.report(shown(line), true, "editor closed", ""),
        Ok(Edited::NothingSaved(dir)) => app.report(
            shown(line),
            true,
            &format!(
                "editor closed — nothing was saved into {}, so it was not made",
                dir.display()
            ),
            "",
        ),
        Err(e) => app.report(shown(line), false, "", &format!("{e:#}")),
    }
    Ok(())
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
    for word in flags {
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
    let manifest = base::Manifest::load_dir(&paths.base());
    let mut sections = vec![defaults(paths)];
    for cap in Capability::ALL {
        let mut section = match cap {
            Capability::Mcp => servers(paths, &manifest),
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
                    .filter(|a| a.supports(cap).is_some_and(|b| b.import.is_some()))
                    .map(|a| a.name.to_string())
                    .collect(),
            ),
            (Err(e), _) => Import::Not(format!("could not read the adapters: {e:#}")),
        };
        sections.push(section);
    }
    Model {
        root: paths.root.display().to_string(),
        sections,
    }
}

fn section(page: Page, title: &'static str) -> Section {
    Section {
        title,
        page,
        rows: Vec::new(),
        problem: None,
        import: Import::From(Vec::new()),
    }
}

/// Every key omh reads, set or not — the half no settings file can show — and
/// anything else the file holds, named for what it is.
fn defaults(paths: &Paths) -> Section {
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
        out.rows.push(Row {
            name: k.name.to_string(),
            value: mine.get(k.name).cloned(),
            note: None,
            about: vec![k.does.to_string()],
            settable: true,
        });
    }
    for (name, value) in &mine {
        if key::describes(name).is_some() {
            continue;
        }
        let table = name.starts_with('[');
        out.rows.push(Row {
            name: name.clone(),
            value: (!table).then(|| value.clone()),
            note: Some(if table {
                "a table".into()
            } else {
                "nothing omh reads".into()
            }),
            about: vec![format!("in {}", file.display())],
            settable: !table,
        });
    }
    out
}

fn servers(paths: &Paths, manifest: &Result<base::Manifest>) -> Section {
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
    let owned = match manifest {
        Ok(manifest) => manifest
            .owns()
            .get(&Capability::Mcp)
            .cloned()
            .unwrap_or_default(),
        // Unknown is not "none of them": a server drawn without its feature's
        // name reads as one of yours, and removing `codegraph` takes its hooks
        // and its rules section too.
        Err(e) => {
            out.problem = Some(format!(
                "could not read omh's base set, so which of these servers are omh's \
                 own is unknown — a row below may be one: {e:#}"
            ));
            Default::default()
        }
    };
    for (name, server) in held {
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
        if !server.env.is_empty() {
            about.push(format!(
                "env   {}",
                server.env.keys().cloned().collect::<Vec<_>>().join(", ")
            ));
        }
        about.push(format!("in    {}", path.display()));
        out.rows.push(Row {
            note: owned
                .get(&name)
                .map(|feature| format!("omh's — the {feature} feature")),
            name,
            value: Some(server.command),
            about,
            settable: false,
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
    for (name, path) in listed {
        let shipped = cap == Capability::Hooks && shipped.contains(&name);
        let mut about = describe(cap, &path);
        about.push(format!("at {}", path.display()));
        if shipped {
            about.push(
                "omh ships this one and `omh upgrade` refreshes it — `omh unuse hooks` \
                 stops a repo running it"
                    .into(),
            );
        }
        out.rows.push(Row {
            note: shipped.then(|| "shipped with omh".to_string()),
            name,
            value: None,
            about,
            settable: false,
        });
    }
    out
}

/// One line saying what an entry is, from the entry itself.
///
/// A file that will not read says so, in place of a description: the row is
/// still there and still removable, and an empty description would read as
/// an entry with nothing to say rather than one omh could not open. So does a
/// skill with no `SKILL.md` — half made, or half removed — which looks like
/// any other skill and is one nothing loads.
fn describe(cap: Capability, path: &std::path::Path) -> Vec<String> {
    let file = match cap {
        Capability::Skills if path.is_dir() => path.join("SKILL.md"),
        _ => path.to_path_buf(),
    };
    if file.is_dir() {
        return Vec::new();
    }
    let raw = match std::fs::read_to_string(&file) {
        Ok(raw) => raw,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound && cap == Capability::Skills => {
            return vec!["no SKILL.md in it — nothing will load this skill".into()];
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Vec::new(),
        Err(e) => return vec![format!("could not read {}: {e}", file.display())],
    };
    if cap == Capability::Hooks {
        return match hook::Hook::parse(&raw, &file.display().to_string()) {
            Ok(h) => vec![h.does().to_string()],
            Err(e) => vec![format!("not a hook omh can run: {e:#}")],
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
    described.or_else(first).into_iter().collect()
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
            Cmd::Settings { cmd: Some(SettingsCmd::Set { key, value }) }
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
                            },
                    }),
            } => {
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
        assert_eq!(row("carry_ins").note.as_deref(), Some("nothing omh reads"));
        assert!(!row("[use]").settable);
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
    /// defaults file, a servers file, the base set and the adapters that will
    /// not read are said, never drawn as nothing set or nothing to import.
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
        assert!(
            page(&read, "servers").problem.is_some(),
            "a servers file that will not parse"
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
        assert!(
            servers
                .problem
                .as_deref()
                .is_some_and(|p| p.contains("unknown")),
            "an unreadable base set leaves which servers are omh's unknown: {servers:?}"
        );
        assert_eq!(servers.rows.len(), 1, "and the server is still listed");
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
            row.about.iter().any(|l| l.contains("no SKILL.md")),
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

    /// The line shown for a new server names its environment and hides the    /// The line shown for a new server names its environment and hides the
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
