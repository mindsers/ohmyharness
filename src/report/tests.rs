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
            catalogue_dir: String::new(),
            catalogue: Vec::new(),
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
            catalogue_dir: String::new(),
            catalogue: Vec::new(),
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
        catalogue_dir: String::new(),
        catalogue: Vec::new(),
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
        sandbox: Some(DoctorSandbox {
            harness: "claude".into(),

            tag: "omh/claude:abc".into(),
        }),
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
        sandbox: Some(DoctorSandbox {
            harness: "claude".into(),

            tag: "t".into(),
        }),
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
        sandbox: Some(DoctorSandbox {
            harness: "claude".into(),

            tag: "t".into(),
        }),
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
/// `--dry-run` answers what the session gets, not how docker is spelled.
///
/// It printed 55 lines of `docker run` argv, one token per line, and
/// nothing else — no image, no summary, nothing a person could read. The
/// doc argued the argv *is* the product, pasteable behind a docker you are
/// debugging, and that reader is real. They are not the reader deciding
/// whether to let this tool near their repository, and that reader is the
/// one `--dry-run` exists for: it is the trust surface, and it answered
/// with bind mounts.
///
/// So the plan is the human form and the argv stays whole in `--json`,
/// which is where a script was reading it from anyway.
#[test]
fn a_dry_run_says_what_the_session_gets() {
    let report = DryRun {
        status: "claude on omh/s01".into(),
        worktree: "/h/.omh/worktrees/proj/s01".into(),
        image: "omh/claude:abc".into(),
        network: "omh-proj".into(),
        reads: vec![
            ("rules".into(), "composed with your AGENTS.md".into()),
            ("skills".into(), "2 selected".into()),
        ],
        writes: vec!["/work — this session's worktree".into()],
        argv: vec!["docker".into(), "run".into(), "--rm".into()],
    };

    let human = emit(&report, Format::Human, &Palette::plain());
    for want in [
        "claude on omh/s01",
        "omh/claude:abc",
        "omh-proj",
        "composed with your AGENTS.md",
        "2 selected",
        "/work",
    ] {
        assert!(human.contains(want), "no `{want}` in the plan: {human}");
    }
    assert!(
        !human.contains("--rm"),
        "the argv is the mechanism, and it is in --json: {human}"
    );

    assert_eq!(
        report.json()["argv"],
        json!(["docker", "run", "--rm"]),
        "and it is whole there, because that is where it was being read from"
    );
}

/// The leftover count does not name the leftovers.
///
/// Naming what a feature supplies — the fix for `mcp  nothing` — made the
/// middle column as wide as its longest row, and `hooks` carries six of
/// omh's own. The third column aligns to that, so `skills  review-diff`
/// was followed by a hundred spaces before its parenthetical, which wraps
/// on any real terminal.
///
/// `init` already says `(4 more in your catalogue)` with no names, and the
/// two reports should not word one fact two ways. What is *applicable* and
/// unselected is named on its own line below, by `notices`, which is the
/// half worth reading.
#[test]
fn the_leftover_count_does_not_name_the_leftovers() {
    let report = Repo {
        repo_id: "repo-deadbeef".into(),
        dir: "/r/.omh".into(),
        settings: vec![],
        features: vec![],
        using: vec![Using {
            capability: "hooks".into(),
            selected: Some(vec!["rust-test".into()]),
            unselected: vec!["go-test".into(), "python-test".into()],
            from_a_feature: vec![],
        }],
        notices: vec![],
    };
    let human = emit(&report, Format::Human, &Palette::plain());
    assert!(
        human.contains("2 more in your catalogue"),
        "the count is the fact worth carrying: {human}"
    );
    assert!(
        !human.contains("go-test"),
        "and the names are what made the row unreadable: {human}"
    );
}

/// What is in your catalogue is not what this repo declined.
///
/// The parenthetical read `(4 not selected: go-format, go-test,
/// python-format, python-test)` in a rust repo. Nobody declined those:
/// they name ecosystems this repo is not, and `catalogue_names` filters
/// them out of what `omh use` will even accept — so *not selected* claims a
/// decision where there was never a choice.
///
/// `init` was corrected for this in #88 and reads `N more in your
/// catalogue`. Two reports, one fact, and they said it two ways.
#[test]
fn what_is_in_your_catalogue_is_not_what_this_repo_declined() {
    let report = Repo {
        repo_id: "repo-deadbeef".into(),
        dir: "/r/.omh".into(),
        settings: vec![],
        features: vec![],
        using: vec![Using {
            capability: "hooks".into(),
            selected: Some(vec!["rust-test".into()]),
            unselected: vec!["go-test".into(), "python-test".into()],
            from_a_feature: vec![],
        }],
        notices: vec![],
    };
    let human = emit(&report, Format::Human, &Palette::plain());
    assert!(
        !human.contains("not selected"),
        "nobody declined a hook for an ecosystem this repo is not: {human}"
    );
    assert!(
        human.contains("2 more in your catalogue"),
        "and the same wording `init` settled on, for the same fact: {human}"
    );
}

/// `nothing` is not what a repo running two MCP servers is using.
///
/// `omh info --repo` said `mcp  nothing` in a repo whose `omh's features`
/// block two lines above reported `codegraph on` and `memory on`, and whose
/// `omh info` listed both servers in the catalogue. All three are true at
/// once: a feature owns its server, so it is excluded from `[use]`, and
/// `[use]` is what this row reads.
///
/// `nothing` is still the right word for a capability where a feature is
/// not supplying one — the distinction is between *you chose nothing* and
/// *you chose nothing and omh brings some anyway*, and only the second one
/// was being misreported.
#[test]
fn a_capability_a_feature_supplies_does_not_report_nothing() {
    let report = Repo {
        repo_id: "repo-deadbeef".into(),
        dir: "/r/.omh".into(),
        settings: vec![],
        features: vec![],
        using: vec![
            Using {
                capability: "mcp".into(),
                selected: Some(vec![]),
                unselected: vec![],
                from_a_feature: vec!["codegraph".into(), "memory".into()],
            },
            Using {
                capability: "skills".into(),
                selected: Some(vec![]),
                unselected: vec![],
                from_a_feature: vec![],
            },
        ],
        notices: vec![],
    };
    let human = emit(&report, Format::Human, &Palette::plain());
    let mcp = human
        .lines()
        .find(|l| l.trim_start().starts_with("mcp"))
        .unwrap_or_else(|| panic!("no mcp row: {human}"));
    assert!(
        mcp.contains("2 from omh's features"),
        "two servers are running here and the row said otherwise: {mcp:?}"
    );
    let skills = human
        .lines()
        .find(|l| l.trim_start().starts_with("skills"))
        .unwrap_or_else(|| panic!("no skills row: {human}"));
    assert!(
        skills.contains("nothing"),
        "and where nothing supplies one, `nothing` is the answer: {skills:?}"
    );
}

/// A capability holding nothing is nothing to report.
///
/// `omh info`'s catalogue block and `omh use --all`'s summary both printed
/// a row per capability whether or not there was anything in it, so a fresh
/// install read `rules 0 / skills 0 / mcp 2 … / commands 0 / subagents 0`.
/// Four of the six rows taught the reader only that omh has six
/// capabilities, which is not what either command was asked.
///
/// The same cut `Init` already makes, for the same reason.
#[test]
fn a_capability_holding_nothing_is_not_a_row() {
    let report = Inventory {
        harnesses: vec![],
        adapters_dir: "/h/.omh/adapters".into(),
        editors: vec![],
        sessions: vec![],
        base: "2026.08".into(),
        catalogue_dir: "/h/.omh".into(),
        catalogue: vec![
            Catalogue {
                capability: "rules".into(),
                entries: vec![],
            },
            Catalogue {
                capability: "skills".into(),
                entries: vec!["review-diff".into()],
            },
        ],
    };
    let human = emit(&report, Format::Human, &Palette::plain());
    assert!(
        human.contains("skills") && human.contains("review-diff"),
        "what you have is the answer: {human}"
    );
    assert!(
        !human.contains("rules"),
        "and a capability you have nothing in is not part of it: {human}"
    );
}

/// What `init` reports, without a container in sight.
///
/// Nearly all of this report's coverage rides on `#[ignore]`, because
/// `init` builds an image — so a plain `cargo test`, and every macOS run,
/// was blind to the whole of `Init::human` and `Init::json`. These are
/// decisions the renderer makes on its own, and they need no runtime at
/// all: mutation found the `using` loop, the `notices` loop, the JSON keys
/// for both, the empty-capability skip and every one of the three rows
/// `init` *kept* to be deletable with the suite green.
fn an_init() -> Init {
    Init {
        harness: Some("claude".into()),
        harness_on_host: true,
        image: Some("omh/claude:abc".into()),
        stacks: vec![("rust".into(), "Cargo.toml".into())],
        using: vec![
            Using {
                capability: "rules".into(),
                selected: None,
                unselected: vec![],
                from_a_feature: vec![],
            },
            Using {
                capability: "skills".into(),
                selected: Some(vec!["review-diff".into()]),
                unselected: vec!["refactor".into()],
                from_a_feature: vec![],
            },
            Using {
                capability: "hooks".into(),
                selected: Some(vec![]),
                unselected: vec![],
                from_a_feature: vec![],
            },
        ],
        notices: vec!["warning: [use] names an entry nothing answers to: skills/nope".into()],
        next: vec![("omh new claude".into(), "start a session".into())],
        hooks: Hooks::Measured(vec![("rust-test".into(), "`cargo`".into())]),
        ..Default::default()
    }
}

/// Every row `init` keeps is a row somebody reads, and each was deletable.
///
/// The report guarded what it *removed* — `harnesses`, `editors`,
/// `not yet done` — and nothing it retained. Deleting the harness row, the
/// image rows or the stack rows left the suite green, which is the shape
/// of a report that can be hollowed out one line per commit.
#[test]
fn init_keeps_the_rows_it_kept() {
    let human = emit(&an_init(), Format::Human, &Palette::plain());
    for row in [
        "harness",
        "claude",
        "image",
        "omh/claude:abc",
        "stack",
        "rust",
    ] {
        assert!(human.contains(row), "no `{row}` row: {human}");
    }
}

/// A capability with nothing in it is skipped; one following the whole
/// catalogue is not.
///
/// Both directions, because both were free. Widening the skip deletes the
/// `everything in your catalogue` rows — the state `Using`'s own doc
/// argues is distinct from a complete list — and dropping it restores the
/// five `0 selected` rows a fresh install has no use for.
#[test]
fn an_empty_capability_is_skipped_and_an_unpinned_one_is_not() {
    let human = emit(&an_init(), Format::Human, &Palette::plain());
    assert!(
        human.contains("rules") && human.contains("everything in your catalogue"),
        "following the catalogue is something to say: {human}"
    );
    assert!(
        !human.contains("hooks"),
        "nothing selected and nothing left over is nothing to report: {human}"
    );
    assert!(
        human.contains("1 more in your catalogue"),
        "and what is left over is counted, not called declined: {human}"
    );
}

/// The notices reach both reports, or the two disagree.
///
/// `init` took half of `omh info --repo`'s answer: it shared the
/// derivation and not the reporting, so a `[use]` entry naming something
/// nothing answers to vanished from one and was warned about by name in
/// the other. Dropping the loop or the JSON key left the suite green.
#[test]
fn init_carries_the_notices_the_repo_report_carries() {
    let init = an_init();
    let human = emit(&init, Format::Human, &Palette::plain());
    assert!(
        human.contains("nothing answers to: skills/nope"),
        "the warning is on the page somebody reads first: {human}"
    );
    assert_eq!(
        init.json()["notices"],
        json!(["warning: [use] names an entry nothing answers to: skills/nope"]),
        "and a script reads the same fact"
    );
}

/// A sandbox that answered says what it found; one that was never asked
/// says so instead.
///
/// Both arms, because pinning only the unhealthy one leaves the whole
/// `Measured` arm — the held-back rows, which are the feature — deletable
/// in silence.
#[test]
fn a_measured_sandbox_and_an_unasked_one_do_not_read_alike() {
    let measured = emit(&an_init(), Format::Human, &Palette::plain());
    assert!(
        measured.contains("rust-test") && measured.contains("`cargo`"),
        "what is held back is named, with what it needs: {measured}"
    );
    assert!(
        !measured.contains("not measured"),
        "the sandbox answered: {measured}"
    );
    assert!(
        an_init().json()["hooks_unchecked"].is_null(),
        "and a script reads these as answers"
    );

    let unasked = Init {
        hooks: Hooks::Unchecked("the sandbox could not be asked".into()),
        ..an_init()
    };
    let human = emit(&unasked, Format::Human, &Palette::plain());
    assert!(
        human.contains("not measured — the sandbox could not be asked"),
        "and an empty list is not a clean bill of health: {human}"
    );
    assert_eq!(unasked.json()["held_back"], json!([]));
    assert_eq!(
        unasked.json()["hooks_unchecked"],
        json!("the sandbox could not be asked"),
        "the one key that tells the two empties apart"
    );
}

/// What `init` says to run next reaches both forms.
#[test]
fn the_next_block_is_in_the_report_and_in_the_json() {
    let init = an_init();
    let human = emit(&init, Format::Human, &Palette::plain());
    assert!(
        human.contains("next") && human.contains("omh new claude"),
        "{human}"
    );
    assert_eq!(init.json()["next"][0]["run"], json!("omh new claude"));
    assert_eq!(init.json()["next"][0]["does"], json!("start a session"));
}

/// selection that happens to name all of today's entries does not. They
/// look identical the moment you print them as a list of names, and they
/// diverge the first time somebody adds a skill — one repo gets it, the
/// other silently does not, and `omh info --repo` said the same thing about both.
///
/// So the human form says `everything`, and the machine form says `null`
/// rather than an array.
#[test]
fn following_the_catalogue_is_not_a_list_that_happens_to_be_complete() {
    let report = Repo {
        repo_id: "repo-deadbeef".into(),
        dir: "/r/.omh".into(),
        settings: vec![],
        features: vec![],
        using: vec![
            Using {
                capability: "rules".into(),
                selected: None,
                unselected: vec![],
                from_a_feature: vec![],
            },
            Using {
                capability: "skills".into(),
                selected: Some(vec!["a".into(), "b".into()]),
                unselected: vec![],
                from_a_feature: vec![],
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
/// `omh info --repo` exists because of the three-layer merge, and the question it
/// is opened to answer is "why is this value this". A row that gives the
/// winner and drops the losers answers the easy half.
#[test]
fn an_overridden_setting_names_what_it_overrode() {
    let report = Repo {
        repo_id: "repo-deadbeef".into(),
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
