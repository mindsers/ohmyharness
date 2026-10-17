use super::*;
use ratatui::backend::TestBackend;
use ratatui::Terminal;

fn press(c: char) -> KeyEvent {
    KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)
}

fn code(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

fn with(c: char, modifiers: KeyModifiers) -> KeyEvent {
    KeyEvent::new(KeyCode::Char(c), modifiers)
}

fn typed(app: &mut App, line: &str) -> Next {
    for c in line.chars() {
        assert_eq!(app.key(press(c)), Next::Stay, "typing {c:?}");
    }
    app.key(code(KeyCode::Enter))
}

fn row(name: &str) -> Row {
    Row {
        name: name.into(),
        ..Row::default()
    }
}

fn model() -> Model {
    Model {
        root: "~/.omh".into(),
        sections: vec![
            Section {
                title: "defaults",
                page: Page::Defaults,
                rows: vec![
                    Row {
                        value: Some("30m".into()),
                        fallback: Some("never stopped".into()),
                        about: vec!["How long a session may sit unused.".into()],
                        edit: Edit::Text {
                            hint: "a duration: 90s, 30m, 2h".into(),
                        },
                        ..row("idle_timeout")
                    },
                    Row {
                        fallback: Some("auto".into()),
                        edit: Edit::Choice(vec!["auto".into(), "docker".into(), "podman".into()]),
                        ..row("runtime")
                    },
                    Row {
                        value: Some("skills = [\"tdd\"]".into()),
                        ..row("[use]")
                    },
                    Row {
                        fallback: Some("the only login captured".into()),
                        edit: Edit::Suggest(vec!["personal".into(), "work".into()]),
                        ..row("account")
                    },
                ],
                problem: None,
                import: Import::Not("your defaults are typed, not imported".into()),
            },
            Section {
                title: "skills",
                page: Page::Entries("skills"),
                rows: vec![
                    Row {
                        summary: Some("keep what works".into()),
                        ..row("keep")
                    },
                    Row {
                        summary: Some("read a diff before it lands".into()),
                        ..row("review-diff")
                    },
                ],
                problem: None,
                import: Import::From(vec!["claude".into(), "codex".into()]),
            },
            Section {
                title: "servers",
                page: Page::Servers,
                rows: vec![],
                problem: None,
                import: Import::From(vec![]),
            },
            Section {
                title: "hooks",
                page: Page::Entries("hooks"),
                rows: vec![],
                problem: Some("could not read ~/.omh/hooks: permission denied".into()),
                import: Import::Not("a hook lands in a repo".into()),
            },
        ],
    }
}

fn drawn_at(app: &App, width: u16, height: u16) -> String {
    let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
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

fn drawn(app: &App) -> String {
    drawn_at(app, 120, 40)
}

/// The screen's text run together across rows, as a person reads it.
fn read(app: &App, width: u16, height: u16) -> String {
    drawn_at(app, width, height)
        .replace(|c: char| "│─╭╮╰╯".contains(c), " ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// **No size breaks it.** A panic in a draw is a panic in raw mode, with the
/// terminal still the screen's — the costliest thing a layout can do. Widths
/// from 1 to 160 and heights from 1 to 60, on every page and with every kind
/// of box open, draw, and draw no control character.
#[test]
fn no_size_breaks_a_draw() {
    type State = Box<dyn Fn(&mut App)>;
    let states: Vec<State> = vec![
        Box::new(|_| {}),
        Box::new(|a| {
            a.key(press('2'));
        }),
        Box::new(|a| {
            a.key(press('4'));
        }),
        Box::new(|a| {
            a.key(code(KeyCode::Enter));
        }),
        Box::new(|a| {
            a.key(press('j'));
            a.key(code(KeyCode::Enter));
        }),
        Box::new(|a| {
            a.key(press('3'));
            a.key(press('a'));
        }),
        Box::new(|a| {
            a.key(press('?'));
        }),
        Box::new(|a| {
            a.key(press('2'));
            a.key(press('/'));
            a.key(press('r'));
        }),
        Box::new(|a| {
            a.report(
                "settings rm --yes skills keep".into(),
                false,
                "",
                "omh: a refusal\n  because of a reason",
            );
            a.confirm(
                vec!["would remove a thing".into()],
                vec!["a warning".into()],
                Act::Remove {
                    capability: "skills".into(),
                    name: "keep".into(),
                },
            );
        }),
    ];
    for (n, state) in states.iter().enumerate() {
        for width in (1..=160).step_by(7) {
            for height in (1..=60).step_by(5) {
                let mut app = App::new(model(), true);
                state(&mut app);
                let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
                terminal
                    .draw(|f| app.draw(f))
                    .unwrap_or_else(|e| panic!("state {n} at {width}x{height}: {e}"));
                for cell in terminal.backend().buffer().content() {
                    assert!(
                        !cell.symbol().chars().any(char::is_control),
                        "state {n} at {width}x{height} drew {:?}",
                        cell.symbol()
                    );
                }
            }
        }
    }
}

/// At every width the layout is for, the selected row's name and what it is
/// are on one line, its detail is drawn, and the keys fit.
#[test]
fn every_layout_shows_the_row_its_detail_and_the_keys() {
    for (width, height) in [(120, 30), (100, 30), (80, 24), (60, 20)] {
        let mut app = App::new(model(), true);
        app.key(press('2'));
        app.key(press('j'));
        let screen = drawn_at(&app, width, height);
        assert!(
            screen
                .lines()
                .any(|l| l.contains("review-diff") && l.contains("read a diff")),
            "{width}x{height}: the name and its summary are not on one line:\n{screen}"
        );
        assert!(
            screen.contains("skills/review-diff"),
            "{width}x{height}: no detail:\n{screen}"
        );
        let keys = screen.lines().last().unwrap_or_default();
        assert!(
            keys.contains("? help") && keys.contains("q quit"),
            "{width}x{height}: {keys}"
        );
    }
    // And where there is room, the detail is beside the list — level with it
    // and to its right — and not under it.
    let mut app = App::new(model(), true);
    app.key(press('2'));
    let screen = drawn_at(&app, 120, 30);
    let at = |needle: &str| {
        screen
            .lines()
            .enumerate()
            .find_map(|(y, l)| l.find(needle).map(|x| (y, x)))
            .unwrap_or_else(|| panic!("no `{needle}`:\n{screen}"))
    };
    let (row_y, row_x) = at("keep what works");
    let (detail_y, detail_x) = at("skills/keep");
    assert!(
        detail_y.abs_diff(row_y) <= 2 && detail_x > row_x,
        "the detail is not beside the list at 120 columns:\n{screen}"
    );
}

/// What omh owns is not offered for removal: no `d remove` in the detail or
/// the keys. `d` still asks the command, whose refusal says why and what to do
/// instead, so the screen keeps no rule of its own.
#[test]
fn what_omh_owns_is_not_offered_for_removal() {
    let mut model = model();
    model.sections[2].rows = vec![
        Row {
            badge: Some("omh".into()),
            fixed: true,
            ..row("codegraph")
        },
        row("linear"),
    ];
    let mut app = App::new(model, true);
    app.key(press('3'));
    // The detail's keys and the footer's, each on its own: either one
    // offering it is the screen offering it.
    let offered = |app: &App| {
        let detail = app.detail_text().to_string();
        let screen = drawn_at(app, 120, 30);
        let footer = screen.lines().last().unwrap_or_default().to_string();
        (detail.contains("d remove"), footer.contains("d remove"))
    };
    assert_eq!(offered(&app), (false, false), "codegraph");
    assert!(matches!(
        app.key(press('d')),
        Next::Preview(Act::Remove { .. })
    ));
    app.key(press('j'));
    assert_eq!(offered(&app), (true, true), "a server of yours");
}

/// The columns start on one line: the first page, the list's heading, and —
/// where it is beside the list — the detail's title. Each began a line lower
/// than the heading, and looked like a list that had slipped.
#[test]
fn the_columns_start_level() {
    for width in [120, 80] {
        let mut app = App::new(model(), true);
        app.key(press('2'));
        let screen = drawn_at(&app, width, 24);
        let heading = screen
            .lines()
            .find(|l| l.contains("Skills  2"))
            .unwrap_or_else(|| panic!("no heading at {width}:\n{screen}"));
        assert!(
            heading.contains("Defaults"),
            "{width}: the pages begin below the heading:\n{screen}"
        );
        if width >= 100 {
            assert!(
                heading.contains("skills/keep"),
                "{width}: the detail begins below the heading:\n{screen}"
            );
        }
    }
}

/// An unset default says what applies instead; a table says what it holds.
#[test]
fn a_default_says_what_it_is_and_what_applies_when_unset() {
    let app = App::new(model(), true);
    let screen = drawn(&app);
    let line = |name: &str| {
        screen
            .lines()
            .find(|l| l.contains(name))
            .unwrap_or_default()
            .to_string()
    };
    assert!(line("runtime").contains("default: auto"), "{screen}");
    assert!(line("[use]").contains("skills = [\"tdd\"]"), "{screen}");
    assert!(!screen.contains("(unset)"), "{screen}");
}

/// A page's rows under their headings, and `j` steps from the last row of one
/// heading to the first of the next — never onto the heading.
#[test]
fn rows_under_headings_and_the_cursor_steps_over_them() {
    let mut hooks = model();
    hooks.sections[3].problem = None;
    hooks.sections[3].rows = vec![
        Row {
            group: Some("yours".into()),
            ..row("mine")
        },
        Row {
            group: Some("shipped with omh".into()),
            ..row("rust-test")
        },
    ];
    let mut app = App::new(hooks, true);
    app.key(press('4'));
    let screen = drawn(&app);
    assert!(
        screen.contains("YOURS") && screen.contains("SHIPPED WITH OMH"),
        "{screen}"
    );
    app.key(press('j'));
    assert_eq!(app.row().map(|r| r.name.as_str()), Some("rust-test"));
    assert_eq!(
        app.key(press('d')),
        Next::Preview(Act::Remove {
            capability: "hooks".into(),
            name: "rust-test".into()
        })
    );
}

/// A choice is picked, not typed: `enter` on `runtime` offers its values, and
/// picking one sets it.
#[test]
fn a_choice_is_picked_not_typed() {
    let mut app = App::new(model(), true);
    app.key(press('j'));
    app.key(code(KeyCode::Enter));
    let screen = drawn(&app);
    for option in ["auto", "docker", "podman"] {
        assert!(screen.contains(option), "{screen}");
    }
    app.key(press('j'));
    assert_eq!(
        app.key(code(KeyCode::Enter)),
        Next::Run(Act::Set {
            key: "runtime".into(),
            value: "docker".into()
        })
    );
}

/// **A key typed in a pick stays in the pick.** Any key it did not know closed
/// it, so typing `podman` into `runtime`'s closed it on `p` — and the `d` that
/// followed reached the page, where it unsets the row: `runtime` gone, with no
/// question asked. A letter jumps to the option it starts; anything else does
/// nothing; Esc and q close.
#[test]
fn a_key_typed_in_a_pick_never_reaches_the_page() {
    let mut app = App::new(model(), true);
    app.key(press('j'));
    app.key(code(KeyCode::Enter));
    for c in "podman".chars() {
        assert_eq!(app.key(press(c)), Next::Stay, "`{c}` acted");
        assert!(
            matches!(app.modal, Some(Modal::Pick(_))),
            "`{c}` closed the pick"
        );
    }
    app.key(press('p'));
    assert_eq!(
        app.key(code(KeyCode::Enter)),
        Next::Run(Act::Set {
            key: "runtime".into(),
            value: "podman".into()
        }),
        "a letter jumps to the option it starts"
    );
    for close in [code(KeyCode::Esc), press('q')] {
        app.key(code(KeyCode::Enter));
        assert!(matches!(app.modal, Some(Modal::Pick(_))));
        assert_eq!(app.key(close), Next::Stay);
        assert_eq!(app.modal, None, "{close:?} closes it");
    }
}

/// A suggestion is picked, or another is typed.
#[test]
fn a_suggestion_is_picked_or_another_typed() {
    let mut app = App::new(model(), true);
    for _ in 0..3 {
        app.key(press('j'));
    }
    app.key(code(KeyCode::Enter));
    assert!(drawn(&app).contains(ANOTHER));
    app.key(code(KeyCode::End));
    assert_eq!(
        app.key(code(KeyCode::Enter)),
        Next::Stay,
        "another… asks for a line"
    );
    assert_eq!(
        typed(&mut app, "codex:work"),
        Next::Run(Act::Set {
            key: "account".into(),
            value: "codex:work".into()
        })
    );
}

/// A value is checked as it is typed, by what the caller handed in.
#[test]
fn a_value_is_checked_as_it_is_typed() {
    let check: Check = Box::new(|key, value| {
        (key == "idle_timeout" && value == "soon").then(|| "not a duration".to_string())
    });
    let mut app = App::new(model(), true).with_check(check);
    app.key(code(KeyCode::Enter));
    app.key(with('u', KeyModifiers::CONTROL));
    for c in "soon".chars() {
        app.key(press(c));
    }
    let screen = drawn(&app);
    assert!(screen.contains("! not a duration"), "{screen}");
    assert!(
        screen.contains("a duration: 90s, 30m, 2h"),
        "and the hint: {screen}"
    );
}

#[test]
fn enter_on_a_default_asks_for_its_value_starting_from_the_current_one() {
    let mut app = App::new(model(), true);
    assert_eq!(app.key(code(KeyCode::Enter)), Next::Stay);
    assert!(drawn(&app).contains("idle_timeout = 30m"));
    app.key(with('u', KeyModifiers::CONTROL));
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
    assert_eq!(app.key(code(KeyCode::Esc)), Next::Quit);
}

/// A table is the file's: neither set from a line nor unset by `d`, which
/// ran `omh settings unset '[use]'` and was told `[use]` was not set while it
/// sat in the file.
#[test]
fn a_table_in_your_defaults_is_not_set_from_a_line() {
    let mut app = App::new(model(), true);
    app.key(press('j'));
    app.key(press('j'));
    assert_eq!(app.key(code(KeyCode::Enter)), Next::Stay);
    assert_eq!(app.modal, None);
    assert!(drawn(&app).contains("[use] is a table"));
    for key in [press('d'), code(KeyCode::Delete)] {
        assert_eq!(app.key(key), Next::Stay, "{key:?} acted on a table");
        assert!(drawn(&app).contains("[use] is a table"));
    }
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

/// Removal asks first, in the preview's words and with its warnings, under a
/// title that names what goes; only `y` is yes.
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
    app.confirm(
        vec!["would remove ~/.omh/skills/review-diff".into()],
        vec!["a repo still uses skills/review-diff".into()],
        act.clone(),
    );
    let screen = drawn(&app);
    assert!(screen.contains("remove skills/review-diff?"), "{screen}");
    assert!(
        screen.contains("would remove ~/.omh/skills/review-diff"),
        "{screen}"
    );
    assert!(
        screen.contains("a repo still uses skills/review-diff"),
        "{screen}"
    );
    assert_eq!(app.key(press('n')), Next::Stay);
    assert!(drawn(&app).contains("kept"));
    app.confirm(vec![], vec![], act.clone());
    assert_eq!(
        app.key(code(KeyCode::Enter)),
        Next::Stay,
        "enter is not yes"
    );
    app.confirm(vec![], vec![], act.clone());
    assert_eq!(app.key(press('Y')), Next::Run(act));
}

/// A server is one form: three fields, Tab between them, Enter onward and then
/// add — and a wrong field is said, on that field.
#[test]
fn a_server_is_one_form_and_its_command_is_split_like_a_shell() {
    let mut app = App::new(model(), true);
    app.key(press('3'));
    assert!(drawn(&app).contains("no servers in your catalogue yet"));
    app.key(press('a'));
    let screen = drawn(&app);
    for label in FORM_LABELS {
        assert!(screen.contains(label.trim()), "{screen}");
    }
    assert_eq!(typed(&mut app, "linear"), Next::Stay);
    assert_eq!(
        typed(&mut app, "npx -y mcp-remote 'https://x/sse'"),
        Next::Stay
    );
    assert_eq!(typed(&mut app, "nope"), Next::Stay, "not KEY=VALUE");
    assert!(drawn(&app).contains("`nope` is not KEY=VALUE"));
    app.key(with('u', KeyModifiers::CONTROL));
    assert_eq!(
        typed(&mut app, "TOKEN=x"),
        Next::Run(Act::AddServer {
            name: "linear".into(),
            command: "npx".into(),
            args: vec!["-y".into(), "mcp-remote".into(), "https://x/sse".into()],
            env: vec![("TOKEN".into(), "x".into())],
        })
    );

    // A missing command is said on its field, and Shift-Tab goes back.
    let mut app = App::new(model(), true);
    app.key(press('3'));
    app.key(press('a'));
    app.key(press('x'));
    app.key(code(KeyCode::Tab));
    app.key(code(KeyCode::Tab));
    assert_eq!(app.key(code(KeyCode::Enter)), Next::Stay);
    assert!(drawn(&app).contains("the program to run, first"));
    assert!(matches!(&app.modal, Some(Modal::Form(f)) if f.at == 1));
    app.key(code(KeyCode::BackTab));
    assert!(matches!(&app.modal, Some(Modal::Form(f)) if f.at == 0));
}

/// **A value typed into env is not drawn.** The list hides a server's env
/// values and the status line hides them in the line it shows, and the form
/// drew them as typed — the one place on the screen a token was on it. Each
/// value is drawn as `•`, quoted ones too; the name stays, and what is added is
/// what was typed.
#[test]
fn an_env_value_is_not_drawn_as_it_is_typed() {
    let mut app = App::new(model(), true);
    app.key(press('3'));
    app.key(press('a'));
    typed(&mut app, "gh");
    // Arguments are kept as typed, so the field says where a token goes.
    assert!(
        drawn(&app).contains("a token goes in env"),
        "{}",
        drawn(&app)
    );
    typed(&mut app, "npx -y @acme/mcp");
    let env = "GITHUB_TOKEN=ghp_secret123 NOTE='two words'";
    for c in env.chars() {
        app.key(press(c));
    }
    let screen = drawn(&app);
    for secret in ["ghp_secret123", "two", "words"] {
        assert!(!screen.contains(secret), "`{secret}` is drawn:\n{screen}");
    }
    assert!(
        screen.contains("GITHUB_TOKEN=") && screen.contains("NOTE="),
        "{screen}"
    );
    assert_eq!(
        app.key(code(KeyCode::Enter)),
        Next::Run(Act::AddServer {
            name: "gh".into(),
            command: "npx".into(),
            args: vec!["-y".into(), "@acme/mcp".into()],
            env: vec![
                ("GITHUB_TOKEN".into(), "ghp_secret123".into()),
                ("NOTE".into(), "two words".into())
            ],
        })
    );
}

/// A name already taken is said on the name field, not after three fields:
/// `mcp add` refuses to write over a server, and omh's own are never yours to
/// replace.
#[test]
fn a_server_name_already_taken_is_said_on_its_field() {
    let mut model = model();
    model.sections[2].rows = vec![
        Row {
            fixed: true,
            ..row("codegraph")
        },
        row("linear"),
    ];
    for (name, says) in [("linear", "already a server"), ("codegraph", "omh's own")] {
        let mut app = App::new(model.clone(), true);
        app.key(press('3'));
        app.key(press('a'));
        assert_eq!(typed(&mut app, name), Next::Stay);
        match &app.modal {
            Some(Modal::Form(f)) => {
                assert_eq!(f.at, 0, "{name}: moved past the name");
                let error = f.error.clone().unwrap_or_default();
                assert!(error.contains(says), "{name}: {error}");
            }
            other => panic!("{name}: {other:?}"),
        }
    }
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

/// `/` narrows the page as you type; what is selected and acted on is the row
/// shown, `esc` takes the filter off, and another page starts clear.
#[test]
fn a_filter_narrows_the_page_and_acts_on_what_it_shows() {
    let mut app = App::new(model(), true);
    app.key(press('2'));
    app.key(press('/'));
    for c in "diff".chars() {
        app.key(press(c));
    }
    let screen = drawn(&app);
    assert!(
        screen.contains("review-diff") && !screen.contains("keep what works"),
        "{screen}"
    );
    app.key(code(KeyCode::Enter));
    assert_eq!(
        app.key(press('d')),
        Next::Preview(Act::Remove {
            capability: "skills".into(),
            name: "review-diff".into()
        })
    );
    app.key(code(KeyCode::Esc));
    assert!(drawn(&app).contains("keep what works"));
    app.key(press('/'));
    app.key(press('z'));
    assert!(drawn(&app).contains("nothing matches"));
    app.key(code(KeyCode::Enter));
    app.key(press('1'));
    assert!(app.filter.is_none(), "another page starts clear");
}

/// `?` shows every key, and any key closes it.
#[test]
fn help_shows_every_key_and_any_key_closes_it() {
    let mut app = App::new(model(), true);
    app.key(press('?'));
    let screen = drawn(&app);
    for key in [
        "change page",
        "filter the list",
        "import from a harness",
        "leave",
    ] {
        assert!(screen.contains(key), "{screen}");
    }
    assert_eq!(
        app.key(press('d')),
        Next::Stay,
        "a key closes help, and does nothing else"
    );
    assert_eq!(app.modal, None);
}

/// What a command said reads as a paragraph: a line too long for the screen
/// wraps under its own text rather than under the mark, and the command,
/// when it cannot sit beside the first line, comes after the last — not
/// between "In a repo:" and the line it introduces.
#[test]
fn what_a_command_said_wraps_under_itself_and_the_command_comes_last() {
    let mut app = App::new(model(), true);
    app.report(
        "--dry-run settings rm --yes mcp codegraph".into(),
        false,
        "",
        "omh: the `codegraph` server is omh's own — the codegraph feature, which is \
         switched off whole rather than taken apart. In a repo:\n  omh set codegraph off",
    );
    let screen = drawn_at(&app, 80, 30);
    let lines: Vec<&str> = screen.lines().collect();
    let first = lines
        .iter()
        .position(|l| l.contains("✗ the `codegraph`"))
        .unwrap_or_else(|| panic!("{screen}"));
    let footer = lines.len() - 1;
    let status = &lines[first + 1..footer];
    assert!(
        status.iter().all(|l| l.starts_with("   ")),
        "a line of the status starts under the mark:\n{screen}"
    );
    assert!(
        status
            .last()
            .is_some_and(|l| l.contains("$ omh --dry-run settings rm")),
        "the command is not last:\n{screen}"
    );
    assert!(
        status.iter().any(|l| l.contains("omh set codegraph off")),
        "{screen}"
    );
}

/// omh's own `omh: ` is not said again on a screen that is all omh; and once
/// what a command said is old news — the next key — it is one line, not a
/// quarter of the screen spent on what was already read.
#[test]
fn an_old_status_is_one_line_without_the_programs_name() {
    let mut app = App::new(model(), true);
    app.report(
        "settings rm mcp codegraph".into(),
        false,
        "",
        "omh: the `codegraph` server is omh's own. In a repo:\n  omh set codegraph off",
    );
    let screen = drawn_at(&app, 120, 30);
    assert!(screen.contains("✗ the `codegraph` server"), "{screen}");
    assert!(!screen.contains("omh: the"), "{screen}");
    assert!(screen.contains("omh set codegraph off"), "{screen}");
    app.key(press('j'));
    let screen = drawn_at(&app, 120, 30);
    assert!(
        screen.contains("✗ the `codegraph` server"),
        "still said:\n{screen}"
    );
    assert!(
        !screen.contains("omh set codegraph off"),
        "an old status keeps its whole height:\n{screen}"
    );
}

/// What a command said is one line with a mark for how it went and the
/// command on the right; it stays, dimmed, after the next key.
#[test]
fn what_a_command_said_is_marked_and_fades() {
    let mut app = App::new(model(), true);
    app.report(
        "settings set idle_timeout 45m".into(),
        true,
        "wrote → ~/.omh/default.toml",
        "",
    );
    let screen = drawn(&app);
    let line = screen
        .lines()
        .find(|l| l.contains("wrote →"))
        .unwrap_or_default();
    assert!(
        line.contains("✓") && line.contains("$ omh settings set idle_timeout 45m"),
        "{screen}"
    );
    app.key(press('j'));
    let mut terminal = Terminal::new(TestBackend::new(120, 40)).unwrap();
    terminal.draw(|f| app.draw(f)).unwrap();
    let buffer = terminal.backend().buffer().clone();
    let y = (0..40)
        .find(|&y| {
            (0..120)
                .map(|x| buffer[(x, y)].symbol().to_string())
                .collect::<String>()
                .contains("wrote →")
        })
        .expect("still there after a key");
    let x = (0..120).find(|&x| buffer[(x, y)].symbol() == "w").unwrap();
    assert!(
        buffer[(x, y)].modifier.contains(Modifier::DIM),
        "and dimmed"
    );
}

/// The row an act changed is marked once the page is read again, until the
/// next key.
#[test]
fn the_row_an_act_changed_is_marked_until_the_next_key() {
    let mut app = App::new(model(), true);
    app.key(press('j'));
    app.key(code(KeyCode::Enter));
    assert!(matches!(app.key(code(KeyCode::Enter)), Next::Run(_)));
    app.reload(model());
    assert_eq!(app.changed.as_deref(), Some("runtime"));
    // Selected as well — the selection follows it — and the mark is still ✓:
    // `▸` there said nothing about what changed.
    assert!(drawn(&app).contains("✓ runtime"), "{}", drawn(&app));
    app.key(press('k'));
    assert_eq!(app.changed, None);
    assert!(!drawn(&app).contains("✓ runtime"), "{}", drawn(&app));
}

/// A click selects; a click on what is selected opens it; the wheel moves; a
/// click on a page goes to it.
#[test]
fn a_mouse_selects_opens_scrolls_and_pages() {
    let event = |kind, column, row| MouseEvent {
        kind,
        column,
        row,
        modifiers: KeyModifiers::NONE,
    };
    let click = |column, row| event(MouseEventKind::Down(MouseButton::Left), column, row);
    let mut app = App::new(model(), true);
    drawn(&app);
    let pages = app.hit.borrow().pages;
    assert_eq!(app.mouse(click(pages.x + 2, pages.y + 1)), Next::Stay);
    assert_eq!(app.at, 1, "the second page");
    drawn(&app);
    let (list, lines) = {
        let hit = app.hit.borrow();
        (hit.list, hit.lines.clone())
    };
    let second = lines.iter().position(|l| *l == Some(1)).unwrap() as u16;
    app.mouse(click(list.x + 3, list.y + second));
    assert_eq!(app.row().map(|r| r.name.as_str()), Some("review-diff"));
    assert_eq!(
        app.mouse(click(list.x + 3, list.y + second)),
        Next::Run(Act::Edit {
            capability: "skills".into(),
            name: Some("review-diff".into())
        })
    );
    app.mouse(event(MouseEventKind::ScrollUp, list.x, list.y));
    assert_eq!(app.row().map(|r| r.name.as_str()), Some("keep"));
    app.mouse(event(MouseEventKind::ScrollDown, list.x, list.y));
    assert_eq!(app.row().map(|r| r.name.as_str()), Some("review-diff"));
    // Under a popup the mouse does nothing: what it would land on is behind
    // the popup, where it cannot be seen.
    app.key(press('?'));
    app.mouse(event(MouseEventKind::ScrollUp, list.x, list.y));
    app.mouse(click(pages.x + 2, pages.y));
    assert_eq!(app.at, 1, "still the second page");
    assert_eq!(app.row().map(|r| r.name.as_str()), Some("review-diff"));
}

/// No colour means no colour on any cell — bold and dim stay, and the
/// selection is reversed.
#[test]
fn no_colour_means_no_colour_anywhere() {
    let mut app = App::new(model(), false);
    app.key(press('2'));
    app.report("settings rm --yes skills x".into(), false, "", "omh: no");
    let mut terminal = Terminal::new(TestBackend::new(120, 40)).unwrap();
    terminal.draw(|f| app.draw(f)).unwrap();
    let buffer = terminal.backend().buffer().clone();
    assert!(buffer
        .content()
        .iter()
        .all(|c| c.fg == Color::Reset && c.bg == Color::Reset));
    assert!(buffer
        .content()
        .iter()
        .any(|c| c.modifier.contains(Modifier::REVERSED)));
}

/// **No key writes unasked.** Moving, paging, `a`, `i`, `/` and `?` open at
/// most a question; on no page does any of them come back as an act.
///
/// The keys that do act are walked separately, because each has one thing it
/// may do: `d` and Delete unset a default at once (a `set` undone, which asks
/// nothing, as `omh settings unset` does not) and *preview* a removal, never
/// run one; `e` and `enter` open `$EDITOR` or a question. None of them is a
/// removal, a server added, a value set or an import.
#[test]
fn a_key_never_writes_without_being_asked_to() {
    let moves: Vec<KeyEvent> = "jkhlgG123456789aiqx/?"
        .chars()
        .map(press)
        .chain(
            [
                KeyCode::Down,
                KeyCode::Up,
                KeyCode::Left,
                KeyCode::Right,
                KeyCode::Home,
                KeyCode::End,
                KeyCode::Tab,
                KeyCode::BackTab,
                KeyCode::Esc,
            ]
            .map(code),
        )
        .collect();
    let acts = [
        press('d'),
        press('e'),
        code(KeyCode::Delete),
        code(KeyCode::Enter),
    ];
    for page in 0..model().sections.len() {
        for key in &moves {
            let mut app = App::new(model(), true);
            app.at = page;
            match app.key(*key) {
                Next::Stay | Next::Quit => {}
                other => panic!("{key:?} on page {page} did {other:?} unasked"),
            }
        }
        for key in &acts {
            let mut app = App::new(model(), true);
            app.at = page;
            match app.key(*key) {
                Next::Stay
                | Next::Preview(Act::Remove { .. })
                | Next::Run(Act::Unset { .. })
                | Next::Run(Act::Edit { .. })
                | Next::Run(Act::EditDefaults) => {}
                other => panic!("{key:?} on page {page} did {other:?}"),
            }
        }
    }
}

/// A letter held with Ctrl or Alt is not that letter.
///
/// crossterm reports Ctrl-D as `d` with CONTROL, and the page matched the
/// letter alone: Ctrl-D — what a hand does to leave a terminal program — unset
/// the selected default on the spot, and Ctrl-E opened `$EDITOR`. Ctrl-D
/// leaves; every other held letter does nothing.
#[test]
fn a_letter_held_with_ctrl_or_alt_is_not_that_letter() {
    for page in 0..model().sections.len() {
        for c in "adeijkhlgGq123/?".chars() {
            for held in [KeyModifiers::CONTROL, KeyModifiers::ALT] {
                let mut app = App::new(model(), true);
                app.at = page;
                let before = (app.at, app.rows.clone());
                let next = app.key(with(c, held));
                if c == 'd' && held == KeyModifiers::CONTROL {
                    assert_eq!(next, Next::Quit, "Ctrl-D leaves");
                    continue;
                }
                assert_eq!(next, Next::Stay, "{held:?}-{c} on page {page}");
                assert_eq!((app.at, app.rows.clone()), before, "{held:?}-{c} moved");
                assert_eq!(app.modal, None, "{held:?}-{c} opened a question");
            }
        }
    }
}

#[test]
fn only_a_plain_y_says_yes() {
    let act = Act::Remove {
        capability: "skills".into(),
        name: "keep".into(),
    };
    for held in [KeyModifiers::CONTROL, KeyModifiers::ALT] {
        let mut app = App::new(model(), true);
        app.confirm(vec!["would remove it".into()], vec![], act.clone());
        assert_eq!(app.key(with('y', held)), Next::Stay);
    }
}

#[test]
fn ctrl_c_leaves_from_anywhere() {
    let ctrl_c = with('c', KeyModifiers::CONTROL);
    let mut app = App::new(model(), true);
    assert_eq!(app.key(ctrl_c), Next::Quit);
    app.key(code(KeyCode::Enter));
    assert_eq!(app.key(ctrl_c), Next::Quit, "in a line");
    app.confirm(vec![], vec![], Act::EditDefaults);
    assert_eq!(app.key(ctrl_c), Next::Quit, "in a question");
    app.key(press('2'));
    app.key(press('i'));
    assert_eq!(app.key(ctrl_c), Next::Quit, "in a pick");
    app.key(press('/'));
    assert_eq!(app.key(ctrl_c), Next::Quit, "in a filter");
}

/// The line edits the way a shell's does, and no key in it can panic.
///
/// The cursor is an index into the characters, so a Backspace that took the
/// character without moving the cursor back left it past the end, and the next
/// character typed panicked — in raw mode, with the terminal still the
/// screen's.
#[test]
fn the_line_edits_the_way_a_shells_does() {
    let mut line = Typed::default();
    let ctrl = |c| with(c, KeyModifiers::CONTROL);
    for key in [
        press('a'),
        press('b'),
        press('c'),
        code(KeyCode::Left),
        code(KeyCode::Backspace), // a|c
        code(KeyCode::Delete),    // a|
        code(KeyCode::Home),
        press('x'), // x|a
        code(KeyCode::End),
        press('y'), // xay|
        ctrl('a'),
        code(KeyCode::Right), // x|ay
        ctrl('e'),
        code(KeyCode::Right), // still at the end
        code(KeyCode::Delete),
        press('z'), // xayz|
        code(KeyCode::Home),
        code(KeyCode::Backspace),
        code(KeyCode::Left),
        press('-'), // -|xayz
    ] {
        line.edit(key);
    }
    assert_eq!(line.text.iter().collect::<String>(), "-xayz");
    line.edit(code(KeyCode::End));
    line.edit(ctrl('u'));
    assert!(
        line.text.is_empty(),
        "Ctrl-U takes the line before the cursor"
    );
}

/// The reason is the part that has to fit, at any width.
///
/// A read that failed says *where* first and *why* last, and boxes sized by
/// counting their own rows cut at every word that filled one — an indented
/// `  because …` line most of all. They are sized by the renderer's own count.
#[test]
fn what_a_box_says_is_drawn_whole_on_a_small_terminal() {
    let mut long = model();
    long.sections[3].problem = Some(
        "reading /Users/somebody-with-a-long-name/.omh/hooks/of-the-catalogue: \
         Permission denied (os error 13)"
            .into(),
    );
    let mut app = App::new(long, true);
    app.key(press('4'));
    for width in [60, 70, 80, 100] {
        assert!(read(&app, width, 24).contains("(os error 13)"), "{width}");
    }
    app.report(
        "settings rm --yes hooks rust-test".into(),
        false,
        "",
        "omh: omh ships hooks/rust-test, and `omh upgrade` would put it back. \
         To stop a repo running it:\n  omh unuse hooks rust-test\n  because of a line",
    );
    for width in [60, 70, 80, 100] {
        let screen = read(&app, width, 24);
        assert!(
            screen.contains("omh unuse hooks rust-test"),
            "{width}: {screen}"
        );
        assert!(screen.contains("because of a line"), "{width}: {screen}");
    }
    app.confirm(
        vec!["would remove ~/.omh/hooks/mine.json".into()],
        vec![
            "~/.omh/settings.toml is not read any more — it became default.toml, the \
             template a new repo is seeded from; merge anything you still want and \
             delete it"
                .into(),
        ],
        Act::Remove {
            capability: "hooks".into(),
            name: "mine".into(),
        },
    );
    for width in [60, 80] {
        let screen = read(&app, width, 24);
        assert!(screen.contains("delete it"), "{width}: {screen}");
        assert!(
            screen.contains("any other key keeps it"),
            "{width}: {screen}"
        );
    }
}

/// **Nothing read is a command to the terminal.** A skill's description, a
/// hook's `does` and a child's output are all text somebody else may have
/// written — an imported entry among them — and an escape drawn raw is the
/// terminal obeying it: a clipboard written through OSC 52, a line rewritten,
/// a title changed. Every control character is drawn as `�`.
#[test]
fn nothing_read_reaches_the_terminal_as_a_control_sequence() {
    let evil = "a\x1b]52;c;ZXZpbA==\x07b\x1b[2Jc\rd\u{9b}31m";
    let mut model = model();
    let row = &mut model.sections[1].rows[0];
    row.name = evil.into();
    row.value = Some(evil.into());
    row.summary = Some(evil.into());
    row.badge = Some(evil.into());
    row.warning = Some(evil.into());
    row.group = Some(evil.into());
    row.about = vec![evil.into()];
    let mut app = App::new(model, true);
    app.key(press('2'));
    app.report(evil.into(), false, evil, evil);
    for state in 0..2 {
        if state == 1 {
            app.confirm(vec![evil.into()], vec![evil.into()], Act::EditDefaults);
        }
        let mut terminal = Terminal::new(TestBackend::new(120, 40)).unwrap();
        terminal.draw(|f| app.draw(f)).unwrap();
        for cell in terminal.backend().buffer().content() {
            assert!(
                !cell.symbol().chars().any(char::is_control),
                "a control character reached the screen: {:?}",
                cell.symbol()
            );
        }
    }
    assert!(
        drawn(&app).contains("a�]52;c;ZXZpbA==�b"),
        "{}",
        drawn(&app)
    );
}

/// A line break in what omh read is a line break: a TOML error's caret points
/// at the column under the line it quotes, and drawn as `�` it pointed at
/// nothing. Every other control character is still `�`.
#[test]
fn a_multi_line_error_is_drawn_as_its_lines() {
    let error = "parse error at line 1, column 16\n  |\n1 | idle_timeout = \n  |                ^\nstring values must be quoted";
    let mut model = model();
    model.sections[0].problem = Some(error.into());
    model.sections[1].rows[0].warning = Some(error.into());
    let mut app = App::new(model, true);
    let screen = drawn_at(&app, 120, 40);
    assert!(!screen.contains('�'), "{screen}");
    assert!(
        screen
            .lines()
            .any(|l| l.contains("1 | idle_timeout =") && !l.contains("column 16")),
        "the quoted line is not its own:\n{screen}"
    );
    app.key(press('2'));
    let screen = drawn_at(&app, 120, 40);
    assert!(!screen.contains('�'), "{screen}");
    assert!(
        screen
            .lines()
            .any(|l| l.contains("string values must be quoted") && !l.contains("idle_timeout")),
        "{screen}"
    );
}

/// A section omh could not read says so, rather than drawing an empty list
/// that reads as "you have none" — and what it did read stays.
#[test]
fn a_section_that_could_not_be_read_says_so() {
    let mut app = App::new(model(), true);
    app.key(press('4'));
    // Read at a width with no column of pages, whose words would otherwise be
    // run in among the reason's when it wraps.
    let screen = read(&app, 60, 30);
    assert!(screen.contains("could not read ~/.omh/hooks"), "{screen}");
    assert!(!screen.contains("no hooks in your catalogue"), "{screen}");

    let mut partly = model();
    partly.sections[3].rows.push(row("rust-test"));
    app.reload(partly);
    let screen = read(&app, 60, 30);
    assert!(screen.contains("could not read ~/.omh/hooks"), "{screen}");
    assert!(screen.contains("rust-test"), "{screen}");
}

#[test]
fn a_reload_keeps_the_selection_on_the_same_row() {
    let mut app = App::new(model(), true);
    app.key(press('2'));
    app.key(press('j'));
    let mut fresh = model();
    fresh.sections[1].rows.insert(0, row("aaa"));
    app.reload(fresh);
    assert_eq!(app.row().map(|r| r.name.as_str()), Some("review-diff"));
    let mut fewer = model();
    fewer.sections[1].rows.truncate(1);
    app.reload(fewer);
    assert_eq!(app.row().map(|r| r.name.as_str()), Some("keep"));
}

#[test]
fn a_long_name_or_summary_is_cut_with_an_ellipsis() {
    assert_eq!(ellipsized("review-diff", 20), "review-diff");
    assert_eq!(ellipsized("review-diff", 7), "review…");
    assert_eq!(ellipsized("review-diff", 0), "");
    assert!(Span::raw(ellipsized("日本語のスキル", 5).as_str()).width() <= 5);
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
