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
                        edit: Edit::Choice(vec!["personal".into(), "work".into()]),
                        ..row("account")
                    },
                ],
                problem: None,
                import: Import::Not("your defaults are typed, not imported".into()),
                about: Vec::new(),
                seeds: None,
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
                import: Import::From(vec![
                    Source {
                        harness: "claude".into(),
                        from: "~/.claude/skills".into(),
                    },
                    Source {
                        harness: "codex".into(),
                        from: "~/.codex/skills — nothing there".into(),
                    },
                ]),
                about: Vec::new(),
                seeds: None,
            },
            Section {
                title: "servers",
                page: Page::Servers,
                rows: vec![],
                problem: None,
                import: Import::From(vec![]),
                about: Vec::new(),
                seeds: None,
            },
            Section {
                title: "hooks",
                page: Page::Entries("hooks"),
                rows: vec![],
                problem: Some("could not read ~/.omh/hooks: permission denied".into()),
                import: Import::Not("a hook lands in a repo".into()),
                about: Vec::new(),
                seeds: None,
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
    // The footer is where the keys are; the detail no longer repeats them.
    let offered = |app: &App| {
        let screen = drawn_at(app, 120, 30);
        let footer = screen.lines().last().unwrap_or_default().to_string();
        footer.contains("d remove")
    };
    assert!(!offered(&app), "codegraph");
    assert!(matches!(
        app.key(press('d')),
        Next::Preview(Act::Remove { .. })
    ));
    app.key(press('j'));
    assert!(offered(&app), "a server of yours");
}

/// The keys at the foot are the ones that act here, the row's own first: no
/// `↵ set` on a table, no `d unset` on what is unset, no `i` where import is
/// refused, no `d remove` on an empty page — and while a popup is open, the
/// popup's. The detail does not repeat them.
#[test]
fn the_footer_offers_what_acts_here() {
    let footer = |app: &App| {
        drawn_at(app, 120, 30)
            .lines()
            .last()
            .unwrap_or_default()
            .to_string()
    };
    let mut app = App::new(model(), true);
    let f = footer(&app);
    assert!(
        f.contains("↵ set") && f.contains("d unset"),
        "idle_timeout: {f}"
    );
    assert!(
        f.find("d unset") < f.find("⇥ page"),
        "the row's keys come first: {f}"
    );
    app.key(press('j'));
    let f = footer(&app);
    assert!(
        f.contains("↵ set") && !f.contains("d unset"),
        "runtime: {f}"
    );
    app.key(press('j'));
    let f = footer(&app);
    assert!(
        !f.contains("↵ set") && !f.contains("d unset") && f.contains("e "),
        "[use]: {f}"
    );
    app.key(press('3'));
    let f = footer(&app);
    assert!(
        !f.contains("d remove") && f.contains("a add"),
        "no servers: {f}"
    );
    app.key(press('4'));
    let f = footer(&app);
    assert!(!f.contains("i import"), "hooks: {f}");
    app.key(press('2'));
    assert!(!app.detail_text().to_string().contains("d remove"));
    app.key(press('a'));
    let f = footer(&app);
    assert!(f.contains("esc") && !f.contains("⇥ page"), "a popup: {f}");
}

/// The rail says what a page holds before you go to it: its number, a `!`
/// where a row has a warning, and `✗` where omh could not look — whose count
/// is `—`, not `0`, since could not look is not none.
#[test]
fn the_rail_numbers_the_pages_and_marks_their_trouble() {
    let mut model = model();
    model.sections[1].rows[0].warning = Some("no SKILL.md in it".into());
    let mut app = App::new(model, true);
    let screen = drawn_at(&app, 120, 30);
    let rail = |title: &str| {
        screen
            .lines()
            .find(|l| l.split('│').next().is_some_and(|r| r.contains(title)))
            .and_then(|l| l.split('│').next())
            .unwrap_or_else(|| panic!("no {title} in the rail:\n{screen}"))
            .to_string()
    };
    assert!(
        rail("Defaults").contains("1 Defaults"),
        "{}",
        rail("Defaults")
    );
    let skills = rail("Skills");
    assert!(
        skills.contains("2 Skills") && skills.contains('!'),
        "{skills}"
    );
    assert!(rail("Hooks").contains('✗'), "{}", rail("Hooks"));
    assert!(!rail("Servers").contains('!'), "{}", rail("Servers"));
    app.key(press('4'));
    let screen = drawn_at(&app, 120, 30);
    assert!(screen.lines().any(|l| l.contains("Hooks  —")), "{screen}");
}

/// A long page: three of yours and thirty shipped, as the hooks page is.
fn long_model() -> Model {
    let mut model = model();
    model.sections[1].rows = (0..33)
        .map(|i| Row {
            group: Some(if i < 3 { "yours" } else { "shipped with omh" }.into()),
            summary: Some(format!("entry number {i}")),
            about: match i % 3 {
                0 => vec!["one line".into()],
                1 => vec!["a line".into(), "and a second".into(), "and a third".into()],
                _ => vec![],
            },
            ..row(&format!("entry-{i:02}"))
        })
        .collect();
    model
}

/// Stacked, the list over its detail, the rule between them stays put as the
/// cursor moves: it was the selected row's detail that sized it, so the list
/// grew and shrank under every `j`. It is the page's tallest detail now. It
/// meets the column rule as `├`, and a page with no row has no rule at all.
#[test]
fn the_stacked_split_holds_still() {
    let mut app = App::new(long_model(), true);
    app.key(press('2'));
    let rule = |app: &App| {
        let screen = drawn_at(app, 80, 24);
        screen
            .lines()
            .position(|l| l.contains("├─"))
            .unwrap_or_else(|| panic!("no junction:\n{screen}"))
    };
    let first = rule(&app);
    for _ in 0..6 {
        app.key(press('j'));
        assert_eq!(
            rule(&app),
            first,
            "the rule moved:\n{}",
            drawn_at(&app, 80, 24)
        );
    }
    let mut app = App::new(model(), true);
    app.key(press('3'));
    let screen = drawn_at(&app, 80, 24);
    assert!(!screen.contains("──"), "a rule over no detail:\n{screen}");
}

/// A list longer than its room says so with a thumb at its edge, and the
/// heading of the group the cursor is in stays at the top once it has
/// scrolled away: at 60×20 the visible hooks had no group at all.
#[test]
fn a_long_list_says_there_is_more_and_keeps_its_heading() {
    let mut app = App::new(long_model(), true);
    app.key(press('2'));
    let screen = drawn_at(&app, 80, 24);
    assert!(screen.contains('┃'), "no sign of more:\n{screen}");
    app.key(press('G'));
    let screen = drawn_at(&app, 80, 24);
    assert!(screen.contains("entry-32"), "{screen}");
    let list: Vec<&str> = screen
        .lines()
        .skip_while(|l| !l.contains("Skills  33"))
        .skip(2)
        .collect();
    assert!(
        list.first().is_some_and(|l| l.contains("SHIPPED WITH OMH")),
        "the heading scrolled away:\n{screen}"
    );
    // At the top, nothing is pinned twice.
    app.key(press('g'));
    let screen = drawn_at(&app, 80, 24);
    assert_eq!(screen.matches("YOURS").count(), 1, "{screen}");
}

/// `?` lists every key, under what it is for, and a description too long for
/// the box goes on under itself: at 60 columns they restarted under the key,
/// and `g`, `G` and the line-editing keys were not there at all.
#[test]
fn help_lists_every_key_under_what_it_is_for() {
    let mut app = App::new(model(), true);
    app.key(press('?'));
    let screen = drawn_at(&app, 120, 40);
    for said in [
        "MOVE", "CHANGE", "TYPING", "LEAVE", "g G", "ctrl-u", "ctrl-a", "letters",
    ] {
        assert!(screen.contains(said), "no `{said}`:\n{screen}");
    }
    let screen = drawn_at(&app, 60, 40);
    let (key, rest) = ("e", "$EDITOR on the entry");
    let line = screen
        .lines()
        .position(|l| l.contains(rest))
        .unwrap_or_else(|| panic!("{screen}"));
    let column = screen.lines().nth(line).unwrap().find(rest).unwrap();
    let next = screen.lines().nth(line + 1).unwrap();
    let text_at = next
        .char_indices()
        .skip(3)
        .find(|(_, c)| !c.is_whitespace() && *c != '│');
    assert!(
        text_at.is_some_and(|(at, _)| at >= column),
        "`{key}`'s description wrapped under the key:\n{screen}"
    );
}

/// Help is read whole at 80×24, where it was cut after `ctrl-a` with no way
/// on — `j` closed it. It scrolls with the keys a list does, and only esc, q
/// and `?` close it. It lists `u`, says `/` searches every page, and keeps
/// ctrl-l out of the keys that leave, which it is not one of.
#[test]
fn help_scrolls_where_it_is_cut_and_lists_every_key() {
    let mut app = App::new(model(), true);
    app.key(press('?'));
    let mut seen = String::new();
    for _ in 0..30 {
        let screen = drawn_at(&app, 80, 24);
        seen.push_str(&screen);
        assert!(
            matches!(app.modal, Some(Modal::Help { .. })),
            "closed:\n{screen}"
        );
        app.key(press('j'));
    }
    for said in ["undo", "ctrl-l", "every page", "every change is a command"] {
        assert!(seen.contains(said), "never shown: `{said}`");
    }
    let screen = drawn_at(&app, 120, 40);
    let leave = screen.find("LEAVE").expect("a LEAVE group");
    let reread = screen.find("read your catalogue again").expect("ctrl-l");
    assert!(reread < leave, "ctrl-l is under LEAVE:\n{screen}");
    for key in [press('x'), press(' ')] {
        app.key(key);
        assert!(matches!(app.modal, Some(Modal::Help { .. })));
    }
    for close in [code(KeyCode::Esc), press('q'), press('?')] {
        app.key(press('?'));
        app.key(close);
        assert_eq!(app.modal, None, "{close:?} did not close it");
    }
}

/// A pick says what it is setting and marks both the value now and the one
/// that applies unset: `account`'s marked nothing, though one applied.
#[test]
fn a_pick_says_what_it_sets_and_marks_the_default() {
    let mut model = model();
    model.sections[0].rows[1].about = vec!["Which runtime builds and runs the sandbox.".into()];
    let mut app = App::new(model, true);
    app.key(press('j'));
    app.key(code(KeyCode::Enter));
    // The box's own lines: the list and the detail behind it say both too.
    let screen = drawn(&app);
    let inside: Vec<String> = screen
        .lines()
        .skip_while(|l| !l.contains("╭ set runtime"))
        .take_while(|l| !l.contains('╰'))
        .filter_map(|l| {
            let open = l.find('╭').or_else(|| l.find('│'))?;
            Some(l[open..].to_string())
        })
        .collect();
    let popup = inside.join("\n");
    assert!(popup.contains("Which runtime builds"), "{screen}");
    assert!(
        inside
            .iter()
            .any(|l| l.contains("auto") && l.contains("default")),
        "{screen}"
    );
}

/// A page with nothing on it says what would be there, where it lives, and
/// how to get one — with the room it has, not a blank pane.
#[test]
fn an_empty_page_says_what_belongs_on_it() {
    let mut model = model();
    model.sections[2].about = vec!["a server an agent can call, run by the harness".into()];
    let mut app = App::new(model, true);
    app.key(press('3'));
    let screen = drawn_at(&app, 120, 30);
    assert!(screen.contains("a server an agent can call"), "{screen}");
}

/// Enter on a server says what it can do, rather than nothing at all.
#[test]
fn enter_on_a_server_says_what_acts_on_it() {
    let mut model = model();
    model.sections[2].rows = vec![row("linear")];
    let mut app = App::new(model, true);
    app.key(press('3'));
    assert_eq!(app.key(code(KeyCode::Enter)), Next::Stay);
    let screen = drawn(&app);
    assert!(
        screen.contains("e opens") && screen.contains("d removes"),
        "{screen}"
    );
}

/// Narrow, the page's name and its count are one line, not a tab line, a
/// heading saying the name again and a blank: two of twenty rows back.
#[test]
fn a_narrow_screen_names_the_page_once() {
    let mut app = App::new(model(), true);
    app.key(press('2'));
    let screen = drawn_at(&app, 60, 20);
    assert_eq!(screen.matches("Skills").count(), 1, "{screen}");
    let first = screen
        .lines()
        .position(|l| l.contains("keep"))
        .unwrap_or_else(|| panic!("{screen}"));
    assert!(first <= 3, "the first row is on line {first}:\n{screen}");
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

/// An unset default says what applies instead, dimmed — the dim is what
/// says it is not yours, and it survives `NO_COLOR` — and a table says what
/// it holds. `default:` before nine of thirteen rows was noise the dim
/// already carried; the detail says it once, beside the value.
#[test]
fn a_default_says_what_it_is_and_what_applies_when_unset() {
    let mut app = App::new(model(), true);
    let screen = drawn(&app);
    let line = |screen: &str, name: &str| {
        screen
            .lines()
            .find(|l| l.contains(name))
            .unwrap_or_default()
            .to_string()
    };
    assert!(line(&screen, "runtime").contains("auto"), "{screen}");
    assert!(!screen.contains("default:"), "{screen}");
    assert!(
        line(&screen, "[use]").contains("skills = [\"tdd\"]"),
        "{screen}"
    );
    assert!(!screen.contains("(unset)"), "{screen}");
    app.key(press('j'));
    let screen = drawn(&app);
    assert!(screen.contains("auto · default"), "{screen}");
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

/// A word typed in a pick goes to the option it spells. Each letter jumped
/// on its own, so `podman` ended on the last letter any option started —
/// `a`, for `auto` — and `↵` wrote that. Letters build a prefix while some
/// option starts with it; one that fits none starts again from itself.
#[test]
fn a_word_typed_in_a_pick_goes_to_the_option_it_spells() {
    let mut app = App::new(model(), true);
    app.key(press('j'));
    let typed = |app: &mut App, word: &str| {
        app.key(code(KeyCode::Enter));
        for c in word.chars() {
            app.key(press(c));
        }
        app.key(code(KeyCode::Enter))
    };
    let set = |value: &str| {
        Next::Run(Act::Set {
            key: "runtime".into(),
            value: value.into(),
        })
    };
    assert_eq!(typed(&mut app, "podman"), set("podman"));
    assert_eq!(typed(&mut app, "do"), set("docker"));
    assert_eq!(typed(&mut app, "au"), set("auto"));
    // A letter no option starts is nothing: `z` leaves `podman` where it is.
    assert_eq!(typed(&mut app, "poz"), set("podman"));
}

/// A new entry named as one that is there is said in the box, which stays
/// open: `a tdd` on Skills opened the skill that was already there, as
/// though it were new — the server form's own check, missing here.
#[test]
fn a_new_entry_named_as_one_that_is_there_is_said() {
    let mut app = App::new(model(), true);
    app.key(press('2'));
    let name = app.model.sections[1].rows[0].name.clone();
    app.key(press('a'));
    for c in name.chars() {
        app.key(press(c));
    }
    assert_eq!(app.key(code(KeyCode::Enter)), Next::Stay);
    let Some(Modal::Input(input)) = &app.modal else {
        panic!("the box closed: {}", drawn(&app));
    };
    let said = input.error.clone().unwrap_or_default();
    assert!(said.contains(&name) && said.contains("already"), "{said:?}");
    app.key(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL));
    for c in "fresh".chars() {
        app.key(press(c));
    }
    assert_eq!(
        app.key(code(KeyCode::Enter)),
        Next::Run(Act::Edit {
            capability: "skills".into(),
            name: Some("fresh".into())
        })
    );
}

// `a_suggestion_is_picked_or_another_typed` went with `another…`: every login
// a launch takes is offered by name and by one harness's, so there was nothing
// left to type — `account_offers_the_logins_captured` in the driver says so.

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

/// A box being typed into holds its width: it grew from 58 to 76 columns
/// and re-centred the moment a warning appeared, under the cursor. Behind any
/// box the screen is dim, and the one that removes is not coloured like help.
#[test]
fn a_popup_holds_still_and_says_what_kind_it_is() {
    let check: Check = Box::new(|key, value| {
        (key == "idle_timeout" && value.contains("soon"))
            .then(|| "`idle_timeout` was given `soon`, which is not a duration omh can read — it takes `90s`, `30m`, `2h`, `1d`, or bare seconds".to_string())
    });
    let mut app = App::new(model(), true).with_check(check);
    app.key(code(KeyCode::Enter));
    let frame = |app: &App| {
        let mut terminal = Terminal::new(TestBackend::new(120, 30)).unwrap();
        terminal.draw(|f| app.draw(f)).unwrap();
        terminal.backend().buffer().clone()
    };
    let corners = |buffer: &ratatui::buffer::Buffer| {
        let y = (0..30)
            .find(|&y| (0..120).any(|x| buffer[(x, y)].symbol() == "╭"))
            .expect("a box");
        let left = (0..120).find(|&x| buffer[(x, y)].symbol() == "╭").unwrap();
        let right = (0..120).find(|&x| buffer[(x, y)].symbol() == "╮").unwrap();
        (left, right, y)
    };
    let before = corners(&frame(&app));
    for c in "soon".chars() {
        app.key(press(c));
    }
    let buffer = frame(&app);
    assert!(drawn(&app).contains("not a duration"));
    let after = corners(&buffer);
    assert_eq!((before.0, before.1), (after.0, after.1), "the box moved");
    // Behind it, dim: the rail's first page is outside every box.
    let (x, y) = (0..30u16)
        .flat_map(|y| (0..20u16).map(move |x| (x, y)))
        .find(|&(x, y)| buffer[(x, y)].symbol() == "D")
        .unwrap();
    assert!(
        buffer[(x, y)].modifier.contains(Modifier::DIM),
        "the screen behind is not dim"
    );
    // The box that removes is the warning's colour; help is not.
    app.key(code(KeyCode::Esc));
    app.confirm(vec!["would remove it".into()], vec![], Act::EditDefaults);
    let (x, _, y) = corners(&frame(&app));
    assert_eq!(
        frame(&app)[(x, y)].fg,
        Color::Yellow,
        "a removal looks like help"
    );
    app.key(press('n'));
    app.key(press('?'));
    let (x, _, y) = corners(&frame(&app));
    assert_ne!(frame(&app)[(x, y)].fg, Color::Yellow);
}

/// A value the check flags takes Enter twice: `set` refuses it, so the first
/// Enter says so and stays, and the second writes it with `--anyway` — for a
/// value only a newer omh reads. Typing again starts over.
#[test]
fn a_flagged_value_takes_enter_twice() {
    let check: Check = Box::new(|key, value| {
        (key == "idle_timeout" && value.starts_with("soon")).then(|| "not a duration".to_string())
    });
    let mut app = App::new(model(), true).with_check(check);
    app.key(code(KeyCode::Enter));
    app.key(with('u', KeyModifiers::CONTROL));
    for c in "soon".chars() {
        app.key(press(c));
    }
    assert_eq!(app.key(code(KeyCode::Enter)), Next::Stay, "written at once");
    assert!(matches!(app.modal, Some(Modal::Input(_))));
    assert!(
        drawn(&app).contains("again writes it anyway"),
        "{}",
        drawn(&app)
    );
    app.key(press('!'));
    assert_eq!(
        app.key(code(KeyCode::Enter)),
        Next::Stay,
        "typing again starts over"
    );
    assert_eq!(
        app.key(code(KeyCode::Enter)),
        Next::Run(Act::SetAnyway {
            key: "idle_timeout".into(),
            value: "soon!".into()
        })
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

/// **The mask reads a line the way the line is read.** What is added is cut
/// by `words` — shell quoting, `\\` escapes — and a mask with rules of its
/// own drew whatever the two disagreed on: `K=q\\ z` is one value to `words`
/// and ended at the space for the mask, so `z` was drawn. Every value below is
/// lowercase and every name uppercase, so a lowercase letter on the screen is
/// a value drawn.
#[test]
fn the_env_mask_reads_a_line_as_words_does() {
    for (line, values) in [
        ("K=qz", vec!["qz"]),
        ("K=q\\ z", vec!["q z"]),
        ("K=\"q\\\"z y\"", vec!["q\"z y"]),
        ("'K=q z'", vec!["q z"]),
        ("K=q\"z y\"", vec!["qz y"]),
        ("\"K\"=qz", vec!["qz"]),
        ("K='q z' L=yy", vec!["q z", "yy"]),
        ("K=", vec![""]),
    ] {
        let parsed = parse_env(&words(line).unwrap()).unwrap();
        assert_eq!(
            parsed.iter().map(|(_, v)| v.as_str()).collect::<Vec<_>>(),
            values,
            "`{line}` is not the case it claims to be"
        );
        let chars: Vec<char> = line.chars().collect();
        let masked = masked_env(&chars);
        assert_eq!(
            masked.chars().count(),
            chars.len(),
            "the cursor would drift"
        );
        assert!(
            !masked.chars().any(|c| c.is_lowercase()),
            "`{line}` is drawn as `{masked}`"
        );
    }
    // A line that does not parse yet — mid-typing — draws no value either.
    for line in ["K=q\\", "K=\"q z", "K='q"] {
        let masked = masked_env(&line.chars().collect::<Vec<_>>());
        assert!(
            !masked.chars().any(|c| c.is_lowercase()),
            "`{line}` → `{masked}`"
        );
    }
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

/// Esc in a form with something typed in it asks first: one Esc threw away
/// three fields. An empty form closes at once, and typing again un-asks.
#[test]
fn esc_in_a_form_with_something_typed_asks_first() {
    let mut app = App::new(model(), true);
    app.key(press('3'));
    app.key(press('a'));
    app.key(code(KeyCode::Esc));
    assert_eq!(app.modal, None, "an empty form closes at once");
    app.key(press('a'));
    for c in "linear".chars() {
        app.key(press(c));
    }
    app.key(code(KeyCode::Esc));
    assert!(
        matches!(app.modal, Some(Modal::Form(_))),
        "it threw them away"
    );
    assert!(drawn(&app).contains("esc again"), "{}", drawn(&app));
    app.key(press('x'));
    app.key(code(KeyCode::Esc));
    assert!(matches!(app.modal, Some(Modal::Form(_))), "typing un-asks");
    app.key(code(KeyCode::Esc));
    assert_eq!(app.modal, None);
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

/// Import picks from the harnesses that keep this kind, each beside where it
/// reads — the pick was the bare list `claude codex omp opencode`, and where a
/// harness keeps its servers is the checkout omh was started in.
#[test]
fn import_picks_from_the_harnesses_that_keep_this_kind() {
    let mut app = App::new(model(), true);
    app.key(press('2'));
    app.key(press('i'));
    let screen = drawn(&app);
    assert!(
        screen
            .lines()
            .any(|l| l.contains("claude") && l.contains("~/.claude/skills")),
        "{screen}"
    );
    assert!(screen.contains("nothing there"), "{screen}");
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
/// shown, `esc` takes the filter off, and it stays as you change page.
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
    assert!(app.filter.is_some(), "a filter is a search, across pages");
}

/// The filter searches every page: the rail says how many on each match, so
/// "which page was that hook on" is answered without going to each — and the
/// page you go to is already narrowed.
#[test]
fn a_filter_searches_every_page() {
    let mut app = App::new(model(), true);
    app.key(press('/'));
    for c in "keep".chars() {
        app.key(press(c));
    }
    app.key(code(KeyCode::Enter));
    let screen = drawn_at(&app, 120, 30);
    let rail = |title: &str| {
        screen
            .lines()
            .find_map(|l| {
                l.split('│')
                    .next()
                    .filter(|r| r.contains(title))
                    .map(str::to_string)
            })
            .unwrap_or_default()
    };
    assert!(rail("Skills").contains("1/2"), "{}", rail("Skills"));
    assert!(rail("Defaults").contains("0/4"), "{}", rail("Defaults"));
    app.key(press('2'));
    let screen = drawn_at(&app, 120, 30);
    assert!(
        screen.contains("keep what works") && !screen.contains("read a diff"),
        "{screen}"
    );
}

/// `?` shows every key, and a key help has no use for does nothing: it
/// neither reaches the page — `d` there unsets a default — nor closes help,
/// which any key did, so `j` to read on closed it instead.
#[test]
fn help_shows_every_key_and_a_key_it_has_no_use_for_does_nothing() {
    let mut app = App::new(model(), true);
    app.key(press('?'));
    let screen = drawn(&app);
    for key in [
        "change page",
        "search every page",
        "import from a harness",
        "leave",
    ] {
        assert!(screen.contains(key), "{screen}");
    }
    assert_eq!(app.key(press('d')), Next::Stay, "`d` reached the page");
    assert!(matches!(app.modal, Some(Modal::Help { .. })));
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

/// The selection is a bar across the row, in colour as without it. In colour
/// it was a two-cell `▸` and a bold name — the colourless screen had the
/// stronger cursor — and the `▸` took the place of a row's `!`, so a selected
/// row hid its warning. Column 0 is the row's state now, never the cursor.
#[test]
fn the_selection_is_a_bar_and_keeps_the_rows_mark() {
    let mut model = model();
    model.sections[1].rows[0].warning = Some("no SKILL.md in it".into());
    let mut app = App::new(model, true);
    app.key(press('2'));
    let mut terminal = Terminal::new(TestBackend::new(120, 30)).unwrap();
    terminal.draw(|f| app.draw(f)).unwrap();
    let buffer = terminal.backend().buffer().clone();
    let line = |y: u16| {
        (0..120)
            .map(|x| buffer[(x, y)].symbol().to_string())
            .collect::<String>()
    };
    let y = (0..30)
        .find(|&y| line(y).contains("keep") && line(y).contains("! "))
        .unwrap_or_else(|| panic!("the selected row lost its mark:\n{}", drawn(&app)));
    assert!(!line(y).contains('▸'), "{}", line(y));
    let name = line(y).find("keep").unwrap();
    let x = line(y)[..name].chars().count() as u16;
    for x in x..x + 20 {
        assert!(
            buffer[(x, y)].modifier.contains(Modifier::REVERSED),
            "cell {x} of the selected row is not in the bar:\n{}",
            line(y)
        );
    }
    // And only that row.
    let other = (0..30).find(|&y| line(y).contains("review-diff")).unwrap();
    let x = line(other)[..line(other).find("review-diff").unwrap()]
        .chars()
        .count() as u16;
    assert!(!buffer[(x, other)].modifier.contains(Modifier::REVERSED));
}

/// The accent is for where you are and what you press. A badge is a fact
/// about a row — `rust`, `omh` — and in cyan, seven of them on the hooks page
/// out-shouted the page you were on.
#[test]
fn a_badge_is_not_in_the_accent() {
    let mut model = model();
    model.sections[1].rows[1].badge = Some("rust".into());
    let mut app = App::new(model, true);
    app.key(press('2'));
    let mut terminal = Terminal::new(TestBackend::new(120, 30)).unwrap();
    terminal.draw(|f| app.draw(f)).unwrap();
    let buffer = terminal.backend().buffer().clone();
    let (x, y) = (0..30u16)
        .flat_map(|y| (0..117u16).map(move |x| (x, y)))
        .find(|&(x, y)| {
            (0..4)
                .map(|i| buffer[(x + i, y)].symbol())
                .collect::<String>()
                == "rust"
        })
        .expect("the badge is drawn");
    assert_ne!(buffer[(x, y)].fg, Color::Cyan, "the badge is in the accent");
}

/// A group's heading has a line above it whenever rows come before it, not
/// only when another group did: `FEATURES` sat under `[use]` with no gap.
#[test]
fn a_heading_after_rows_has_room_above_it() {
    let mut model = model();
    model.sections[0].rows.push(Row {
        group: Some("features".into()),
        ..row("codegraph")
    });
    let app = App::new(model, true);
    let screen = drawn_at(&app, 120, 30);
    let lines: Vec<&str> = screen.lines().collect();
    let at = lines.iter().position(|l| l.contains("FEATURES")).unwrap();
    let above = lines[at - 1].split('│').nth(1).unwrap_or_default();
    assert!(above.trim().is_empty(), "{screen}");
}

/// A row's `!` outranks the `✓` of a change just made: `space` seeded a
/// skill with no `SKILL.md`, and the row answered with a green `✓` over the
/// warning. And inside the bar no span keeps its colour — the glyphs carry
/// the meaning, as under `NO_COLOR` — where a warned row's bar ran green,
/// cyan, reverse and yellow along its length.
#[test]
fn a_warning_outranks_a_change_and_the_bar_is_one_tone() {
    let mut model = model();
    let toggle = Act::Use {
        capability: "skills".into(),
        name: "keep".into(),
    };
    model.sections[1].rows[0].seeded = Some(false);
    model.sections[1].rows[0].toggle = Some(toggle.clone());
    model.sections[1].rows[0].warning = Some("no SKILL.md in it".into());
    let mut app = App::new(model.clone(), true);
    app.key(press('2'));
    assert_eq!(app.key(press(' ')), Next::Run(toggle));
    app.report(
        "settings use -- skills keep".into(),
        true,
        "new repos take skills/keep",
        "",
    );
    // As the driver does after every act.
    app.reload(model);
    let mut terminal = Terminal::new(TestBackend::new(120, 30)).unwrap();
    terminal.draw(|f| app.draw(f)).unwrap();
    let buffer = terminal.backend().buffer().clone();
    let y = (0..30u16)
        .find(|&y| {
            (0..120u16)
                .map(|x| buffer[(x, y)].symbol().to_string())
                .collect::<String>()
                .contains("no SKILL.md in it")
        })
        .expect("the row");
    let line: String = (0..120u16)
        .map(|x| buffer[(x, y)].symbol().to_string())
        .collect();
    let row = &line[line.find('│').unwrap() + '│'.len_utf8()..];
    assert!(row.trim_start().starts_with('!'), "{line}");
    let x0 = line[..line.find('│').unwrap()].chars().count() as u16 + 1;
    let colours: std::collections::BTreeSet<String> = (x0..x0 + 50)
        .map(|x| format!("{:?}", buffer[(x, y)].fg))
        .collect();
    assert!(colours.len() <= 1, "the bar runs {colours:?}: {line}");
}

/// `space` flips whether the repos `omh init` sets up after get a row, by the
/// act the row carries; the row's `●`/`○` and the page's heading say which way
/// it is now. A row with nothing to flip takes no `space`.
#[test]
fn space_flips_what_new_repos_get() {
    let mut model = model();
    let unuse = Act::Unuse {
        capability: "skills".into(),
        name: "keep".into(),
    };
    model.sections[1].rows[0].seeded = Some(true);
    model.sections[1].rows[0].toggle = Some(unuse.clone());
    model.sections[1].rows[1].seeded = Some(false);
    model.sections[1].rows[1].toggle = Some(Act::Use {
        capability: "skills".into(),
        name: "review-diff".into(),
    });
    model.sections[1].seeds = Some("new repos get 1 of 2".into());
    let mut app = App::new(model, true);
    app.key(press('2'));
    let screen = drawn_at(&app, 120, 30);
    assert!(screen.contains("new repos get 1 of 2"), "{screen}");
    let line = |name: &str| {
        screen
            .lines()
            .find(|l| l.contains(name))
            .unwrap_or_default()
            .to_string()
    };
    assert!(line("keep what works").contains('●'), "{screen}");
    assert!(line("read a diff").contains('○'), "{screen}");
    let footer = screen.lines().last().unwrap_or_default();
    assert!(footer.contains("space"), "{footer}");
    assert_eq!(app.key(press(' ')), Next::Run(unuse));
    app.key(press('1'));
    assert_eq!(
        app.key(press(' ')),
        Next::Stay,
        "a default has nothing to flip"
    );
}

/// `u` undoes the last change that has an inverse — a value set back, an
/// unset put back, a `use` left out again — once the command said it worked.
/// Every change is already a command, so its inverse is one too. One that
/// failed leaves nothing to undo; and undoing is itself undone by `u`.
#[test]
fn u_undoes_the_last_change() {
    let mut app = App::new(model(), true);
    // idle_timeout is 30m: set it to 45m.
    app.key(code(KeyCode::Enter));
    app.key(with('u', KeyModifiers::CONTROL));
    let set = typed(&mut app, "45m");
    assert!(matches!(set, Next::Run(Act::Set { .. })));
    assert_eq!(
        app.key(press('u')),
        Next::Stay,
        "nothing until it is said to have worked"
    );
    app.report("settings set idle_timeout 45m".into(), true, "wrote", "");
    assert!(drawn(&app).contains("u undoes"), "{}", drawn(&app));
    let undo = app.key(press('u'));
    assert_eq!(
        undo,
        Next::Run(Act::Set {
            key: "idle_timeout".into(),
            value: "30m".into()
        })
    );
    // An unset is undone by setting it back.
    let mut app = App::new(model(), true);
    assert!(matches!(app.key(press('d')), Next::Run(Act::Unset { .. })));
    app.report("settings unset idle_timeout".into(), true, "removed", "");
    assert_eq!(
        app.key(press('u')),
        Next::Run(Act::Set {
            key: "idle_timeout".into(),
            value: "30m".into()
        })
    );
    // A change that failed leaves nothing to undo.
    let mut app = App::new(model(), true);
    app.key(press('d'));
    app.report("settings unset idle_timeout".into(), false, "", "omh: no");
    assert_eq!(app.key(press('u')), Next::Stay);
}

/// A servers page whose file omh could not read takes no `a` or `i`: the
/// form let three fields be filled, and the add then failed on the file it
/// could not read. `e` is the way in, to fix the file.
#[test]
fn a_servers_page_omh_could_not_read_takes_no_add() {
    let mut model = model();
    model.sections[2].problem = Some("parsing ~/.omh/mcp.json: trailing characters".into());
    model.sections[2].import = Import::From(vec![Source {
        harness: "claude".into(),
        from: "./.mcp.json".into(),
    }]);
    let mut app = App::new(model, true);
    app.key(press('3'));
    let screen = drawn_at(&app, 120, 30);
    let footer = screen.lines().last().unwrap_or_default();
    assert!(
        !footer.contains("add") && !footer.contains("import"),
        "{footer}"
    );
    assert!(footer.contains("edit the file"), "{footer}");
    for key in ['a', 'i'] {
        app.key(press(key));
        assert_eq!(app.modal, None, "`{key}` opened something: {}", drawn(&app));
    }
}

/// Undoing puts back what was there even where omh cannot read it: a value
/// written `--anyway` was put back with a plain `set`, which refused it, so
/// `u` after changing it did nothing but fail.
#[test]
fn u_puts_back_a_value_written_anyway() {
    let check: Check = Box::new(|key, value| {
        (key == "idle_timeout" && value == "forever").then(|| "not a duration".to_string())
    });
    let mut model = model();
    model.sections[0].rows[0].value = Some("forever".into());
    let mut app = App::new(model, true).with_check(check);
    app.key(code(KeyCode::Enter));
    app.key(with('u', KeyModifiers::CONTROL));
    let set = typed(&mut app, "2h");
    assert!(matches!(set, Next::Run(Act::Set { .. })), "{set:?}");
    app.report("settings set idle_timeout 2h".into(), true, "wrote", "");
    assert_eq!(
        app.key(press('u')),
        Next::Run(Act::SetAnyway {
            key: "idle_timeout".into(),
            value: "forever".into()
        })
    );
}

/// `space` is undone by what the caller says undoes it, not by the opposite
/// verb: `use` on a guard of a kind with no list writes the kind as a list,
/// and `unuse` after it left the list standing — the rest of the kind frozen,
/// where `u` promised the file back. Where nothing undoes a flip exactly,
/// `u` is not offered.
#[test]
fn space_is_undone_by_what_the_caller_says_undoes_it() {
    let mut model = model();
    let unuse = Act::Unuse {
        capability: "skills".into(),
        name: "keep".into(),
    };
    let back = Act::Use {
        capability: "skills".into(),
        name: "keep".into(),
    };
    model.sections[1].rows[0].seeded = Some(true);
    model.sections[1].rows[0].toggle = Some(unuse.clone());
    model.sections[1].rows[0].untoggle = Some(back.clone());
    let guard = Act::Use {
        capability: "skills".into(),
        name: "review-diff".into(),
    };
    model.sections[1].rows[1].seeded = Some(false);
    model.sections[1].rows[1].toggle = Some(guard.clone());
    let mut app = App::new(model, true);
    app.key(press('2'));
    assert_eq!(app.key(press(' ')), Next::Run(unuse.clone()));
    app.report("settings unuse -- skills keep".into(), true, "", "");
    assert_eq!(app.key(press('u')), Next::Run(back));

    app.key(press('j'));
    assert_eq!(app.key(press(' ')), Next::Run(guard));
    app.report("settings use -- skills review-diff".into(), true, "", "");
    assert_eq!(app.key(press('u')), Next::Stay);
    assert!(!drawn(&app).contains("u undoes"), "{}", drawn(&app));
}

/// Enter on a key that was probably meant to be another moves its value
/// there, the typo with it: fixing `carry_ins` was two acts on two rows, and
/// the value was retyped in between. Where the meant key has a value already,
/// that is said, and nothing is written over it.
#[test]
fn enter_on_a_typo_moves_it_to_the_key_it_meant() {
    let mut model = model();
    model.sections[0].rows.push(Row {
        value: Some("[\".env\"]".into()),
        warning: Some("nothing omh reads — did you mean `carry_in`?".into()),
        meant: Some("carry_in".into()),
        ..row("carry_ins")
    });
    model.sections[0].rows.push(row("carry_in"));
    let mut app = App::new(model.clone(), true);
    for _ in 0..4 {
        app.key(press('j'));
    }
    assert_eq!(app.row().map(|r| r.name.as_str()), Some("carry_ins"));
    // The row names the key it meant; the foot says what Enter does with it.
    assert!(drawn(&app)
        .lines()
        .last()
        .unwrap_or_default()
        .contains("↵ move it"));
    assert_eq!(
        app.key(code(KeyCode::Enter)),
        Next::Run(Act::Move {
            from: "carry_ins".into(),
            to: "carry_in".into(),
            value: "[\".env\"]".into()
        })
    );
    model.sections[0].rows.last_mut().unwrap().value = Some("[\".env.local\"]".into());
    let mut app = App::new(model, true);
    for _ in 0..4 {
        app.key(press('j'));
    }
    assert_eq!(app.key(code(KeyCode::Enter)), Next::Stay);
    assert!(
        drawn(&app).contains("carry_in is set already"),
        "{}",
        drawn(&app)
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
/// leaves and Ctrl-L reads again, as in a shell; every other held letter does
/// nothing.
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
                if c == 'l' && held == KeyModifiers::CONTROL {
                    assert_eq!(next, Next::Reload, "Ctrl-L reads again");
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

/// Ctrl-L reads your catalogue again, for a file changed outside the screen
/// — as Ctrl-L redraws in a shell.
#[test]
fn ctrl_l_reads_the_catalogue_again() {
    let mut app = App::new(model(), true);
    assert_eq!(app.key(with('l', KeyModifiers::CONTROL)), Next::Reload);
    assert_eq!(app.key(press('l')), Next::Stay, "a plain l changes page");
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
