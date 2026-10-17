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

use crate::adapter::{Adapter, Capability};
use crate::profile::Paths;
use crate::tui::{Act, App, Model, Next, Row, Section};
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
    let mut terminal = ratatui::try_init().context("opening the screen")?;
    let outcome = (|| -> Result<()> {
        loop {
            terminal.draw(|f| app.draw(f))?;
            // A resize, a focus change, a paste: none is a key, and the next
            // draw is the whole answer to each.
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
                    let out = omh(&line)?;
                    if out.status.success() {
                        app.confirm(lines(&out.stdout), act);
                    } else {
                        // The line that refused is the preview, so that is the
                        // one shown — not the removal nobody ran.
                        report(&mut app, &line, &out);
                    }
                }
                Next::Run(act) if hands_over_the_terminal(&act) => {
                    // `$EDITOR` is a program with a screen of its own, so it
                    // gets the terminal whole, and this one is put back after.
                    suspend()?;
                    let line = argv(&act);
                    let status = std::process::Command::new(std::env::current_exe()?)
                        .args(&line)
                        .status();
                    resume(&mut terminal)?;
                    match status {
                        Ok(status) => app.report(
                            shown(&line),
                            status.success(),
                            if status.success() {
                                "editor closed"
                            } else {
                                ""
                            },
                            if status.success() {
                                ""
                            } else {
                                "the editor did not exit cleanly"
                            },
                        ),
                        Err(e) => app.report(shown(&line), false, "", &format!("{e}")),
                    }
                    app.reload(model(paths));
                }
                Next::Run(act) => {
                    let line = argv(&act);
                    let out = omh(&line)?;
                    report(&mut app, &line, &out);
                    app.reload(model(paths));
                }
            }
        }
    })();
    ratatui::restore();
    outcome
}

/// The command line an act is, after `omh`.
///
/// Guarded by `every_act_is_a_line_omh_accepts_and_means_what_it_says`, which
/// parses each one back through the real grammar: a line here that clap read
/// differently — `--env` swallowed as a server argument, say — would be a
/// screen that does something other than what it shows.
pub(crate) fn argv(act: &Act) -> Vec<String> {
    let s = |words: &[&str]| words.iter().map(|w| w.to_string()).collect::<Vec<_>>();
    match act {
        Act::Set { key, value } => s(&["settings", "set", key, value]),
        Act::Unset { key } => s(&["settings", "unset", key]),
        Act::EditDefaults => s(&["settings", "edit"]),
        Act::Edit { capability, name } => {
            let mut line = s(&["settings", "edit", capability]);
            line.extend(name.clone());
            line
        }
        // `--yes` because the screen has already asked, in the words of this
        // same line's preview.
        Act::Remove { capability, name } => s(&["settings", "rm", capability, name, "--yes"]),
        Act::AddServer {
            name,
            command,
            args,
            env,
        } => {
            // `--env` first: the server's arguments run to the end of the line,
            // so anything after them is one of them. And `--` before them, so
            // an argument that looks like a flag is still an argument.
            let mut line = s(&["settings", "mcp", "add"]);
            for (k, v) in env {
                line.push("--env".into());
                line.push(format!("{k}={v}"));
            }
            line.push(name.clone());
            line.push(command.clone());
            line.push("--".into());
            line.extend(args.iter().cloned());
            line
        }
        Act::Import {
            capability,
            harness,
        } if capability == "mcp" => s(&["settings", "mcp", "import", harness]),
        Act::Import {
            capability,
            harness,
        } => s(&["import", capability, harness]),
    }
}

fn hands_over_the_terminal(act: &Act) -> bool {
    matches!(act, Act::EditDefaults | Act::Edit { .. })
}

/// Run one line through this binary, its output captured.
///
/// Plain, because the screen paints; and with nothing on stdin, because
/// nothing here will answer a question — a command that would ask refuses
/// instead, which is what `--yes` on `rm` is for.
fn omh(line: &[String]) -> Result<std::process::Output> {
    std::process::Command::new(std::env::current_exe().context("finding omh")?)
        .args(["--color", "never"])
        .args(line)
        .stdin(std::process::Stdio::null())
        .output()
        .with_context(|| format!("running omh {}", line.join(" ")))
}

fn report(app: &mut App, line: &[String], out: &std::process::Output) {
    app.report(
        shown(line),
        out.status.success(),
        &String::from_utf8_lossy(&out.stdout),
        &String::from_utf8_lossy(&out.stderr),
    );
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
fn shown(line: &[String]) -> String {
    let (head, args) = match line.iter().position(|w| w == "--") {
        Some(at) => (&line[..at], &line[at + 1..]),
        None => (line, &line[line.len()..]),
    };
    let mut hidden = Vec::with_capacity(head.len());
    let mut after_env = false;
    for word in head {
        hidden.push(match word.split_once('=') {
            Some((k, _)) if after_env => format!("{k}=…"),
            _ => word.clone(),
        });
        after_env = word == "--env";
    }
    match args.len() {
        0 => said(&hidden),
        n => format!(
            "{} -- … ({n} argument{})",
            said(&hidden),
            if n == 1 { "" } else { "s" }
        ),
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
        match (&adapters, cap) {
            // A hook binds to one project's commands and lands in that repo,
            // so importing one is not something your catalogue can take.
            (_, Capability::Hooks) => {
                section.no_import = Some(
                    "a hook lands in a repo, not in your catalogue — in one, \
                     `omh import hooks <harness>`"
                        .into(),
                );
            }
            (Ok(adapters), _) => {
                section.importers = adapters
                    .iter()
                    .filter(|a| a.supports(cap).is_some_and(|b| b.import.is_some()))
                    .map(|a| a.name.to_string())
                    .collect();
            }
            (Err(e), _) => {
                section.no_import = Some(format!("could not read the adapters: {e:#}"));
            }
        }
        sections.push(section);
    }
    Model {
        root: paths.root.display().to_string(),
        sections,
    }
}

fn section(capability: Option<&'static str>, title: &'static str) -> Section {
    Section {
        title,
        capability,
        rows: Vec::new(),
        problem: None,
        importers: Vec::new(),
        no_import: None,
    }
}

/// Every key omh reads, set or not — the half no settings file can show — and
/// anything else the file holds, named for what it is.
fn defaults(paths: &Paths) -> Section {
    let mut out = section(None, "defaults");
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
    let mut out = section(Some("mcp"), "servers");
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
        Err(e) => {
            out.problem = Some(format!("could not read omh's base set: {e:#}"));
            Default::default()
        }
    };
    for (name, server) in held {
        // The command and a count, as `omh settings mcp ls` shows it, and not
        // the arguments themselves: `--header "Authorization: Bearer …"` is
        // how a remote server is reached, and this is drawn for anybody
        // looking at the screen. `e` opens the file, for somebody who meant to.
        let mut about = vec![format!("runs  {}", server.command)];
        if !server.args.is_empty() {
            about.push(format!(
                "with  {} argument{} — e opens the file to read them",
                server.args.len(),
                if server.args.len() == 1 { "" } else { "s" }
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
    let (key, title) = match cap {
        Capability::Rules => ("rules", "rules"),
        Capability::Skills => ("skills", "skills"),
        Capability::Commands => ("commands", "commands"),
        Capability::Subagents => ("subagents", "subagents"),
        Capability::Hooks => ("hooks", "hooks"),
        Capability::Mcp => ("mcp", "servers"),
    };
    let mut out = section(Some(key), title);
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
    let shipped = crate::cmd::settings::shipped_hooks();
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
/// an entry with nothing to say rather than one omh could not open.
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
        assert_eq!(said(&argv(&act)), r#"settings set carry_in '[".env"]'"#);
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
