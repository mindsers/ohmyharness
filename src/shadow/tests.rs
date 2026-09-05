use super::*;

/// A checkout with history, a worktree holding a session's starting tree,
/// and a carried file the user never tracked.
fn fixture() -> (tempfile::TempDir, PathBuf, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("checkout");
    std::fs::create_dir_all(&root).unwrap();
    let run = |args: &[&str]| {
        Command::new("git")
            .current_dir(&root)
            .args(args)
            .output()
            .unwrap();
    };
    run(&["init", "-q", "-b", "main"]);
    run(&["config", "user.email", "t@example.com"]);
    run(&["config", "user.name", "t"]);
    std::fs::write(root.join("f.txt"), "base\n").unwrap();
    run(&["add", "-A"]);
    run(&[
        "commit",
        "-q",
        "-m",
        "a commit only the host should know about",
    ]);

    // A real worktree, made the way omh makes one, because the `.git`
    // pointer it writes is the *only* route from here back to the
    // checkout's object store. Built as a plain directory instead, the
    // isolation test could not reach the thing it exists to forbid: a leak
    // that resolves the checkout through that pointer and writes
    // `objects/info/alternates` passed, because there was no pointer to
    // resolve. The guard was correct and the fixture made it decorative.
    let wt = dir.path().join("wt");
    run(&[
        "worktree",
        "add",
        "-q",
        "-b",
        "omh/s01",
        wt.to_str().unwrap(),
    ]);
    std::fs::write(wt.join("f.txt"), "base\n").unwrap();
    std::fs::write(wt.join(".env"), "SECRET=1\n").unwrap();

    let shadow_dir = dir.path().join("shadow");
    std::fs::create_dir_all(&shadow_dir).unwrap();
    (dir, wt, shadow_dir)
}

fn head_of(repo: &Path) -> String {
    let out = Command::new("git")
        .current_dir(repo)
        .args(["rev-parse", "HEAD"])
        .output()
        .unwrap();
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// The note reads as English in every shape it has, and never claims a
/// decision is waiting when none is.
///
/// A count is the whole content of this sentence, so a count rendered
/// wrong is the sentence rendered wrong — and *0 files need resolving* at
/// the top of a rebuilt context sends an agent looking through a clean
/// tree for a conflict.
///
/// Its size is asserted too. It arrives where the session's own subject
/// matter is competing for room, and the manifest claims a figure for it;
/// a sentence that grows into a paragraph should break the claim rather
/// than quietly cost more.
#[test]
fn the_note_says_what_arrived_and_what_is_left_in_the_words_for_it() {
    let one = note_for("main", 1, 0);
    assert!(
        one.contains("moved 1 commit under"),
        "one commit, not `1 commits`: {one}"
    );
    assert!(
        one.contains("nothing needs deciding"),
        "and nothing is waiting: {one}"
    );
    assert!(
        !one.contains("resolving"),
        "a clean merge does not mention resolving anything: {one}"
    );

    let many = note_for("develop", 12, 2);
    assert!(
        many.contains("develop moved 12 commits"),
        "the branch by name — a session's base is not always `main`: {many}"
    );
    assert!(
        many.contains("2 files still need resolving"),
        "and what is left, in the plural: {many}"
    );

    let single = note_for("main", 3, 1);
    assert!(
        single.contains("1 file still needs resolving"),
        "one file *needs*, not *need*: {single}"
    );

    // The other zero, and the one this test did not have on the first
    // pass. `--base` is a user-supplied argument, so a base that went
    // backwards is ordinary input rather than a corrupted repository.
    let back = note_for("release-1.0", 0, 0);
    assert!(
        !back.contains("0 commit"),
        "a count of nothing is not a sentence: {back}"
    );
    assert!(
        back.contains("moved backwards") && back.contains("release-1.0"),
        "it says what actually happened, by name: {back}"
    );
    assert!(
        back.contains("what changed") && !back.contains("what arrived"),
        "and does not claim something arrived when the base went back: {back}"
    );

    for note in [&one, &many, &single, &back] {
        assert!(
            note.contains("git show HEAD"),
            "every shape says where to read it: {note}"
        );
        // A ceiling, not the cost — the manifest's figure is asserted
        // byte for byte by `the_notes_declared_cost_matches_the_sentence_it_ships`
        // against one named shape, which is the only way a number about an
        // unbounded string can be checked at all. What this catches is the
        // sentence turning into a paragraph, which is a different mistake
        // and the one this arrives in the middle of somebody's context to
        // make.
        assert!(
            note.len() < 320,
            "the note has grown from a sentence into a paragraph: {} B",
            note.len()
        );
        // A run of spaces is a line continuation whose indentation shipped
        // — the same accident `git_checks_from` carries a guard for.
        assert!(!note.contains("  "), "a fold's indentation shipped: {note}");
    }
}

/// Commit in the sandbox the way the agent does, and answer with its id.
/// Run the **shipped** hook body against this fixture.
///
/// The command itself, with the guest's two paths swapped for the
/// temporary ones — not a re-implementation of it. An earlier draft wrote
/// the plumbing out again in Rust and asserted on that, which proves the
/// copy works and would pass against a hook that did nothing.
fn turn_snapshot(s: &Shadow, wt: &Path) {
    let body = turn_hook_command(s.gitdir.to_str().unwrap(), wt.to_str().unwrap());
    let out = Command::new("sh")
        .arg("-c")
        .arg(&body)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .output()
        .unwrap();
    // Deliberately no assertion on the exit status or the streams. The
    // body ends in `>/dev/null 2>&1 || true`, so success and silence are
    // true by construction — an earlier version asserted both and was
    // asserting nothing at all. What the hook did is asserted by the
    // caller, against the repository.
    let _ = out;
}

fn checkpoint(s: &Shadow, wt: &Path, file: &str, body: &str, subject: &str) -> String {
    std::fs::write(wt.join(file), body).unwrap();
    git(&s.gitdir, wt, &["add", "-A", "."]).unwrap();
    git(
        &s.gitdir,
        wt,
        &["commit", "-q", "--no-verify", "-m", subject],
    )
    .unwrap();
    git(&s.gitdir, wt, &["rev-parse", "HEAD"])
        .unwrap()
        .trim()
        .to_string()
}

/// Two syncs before one start leave one sentence, not two stacked.
///
/// The contract `leave_note`'s own comment argues for, asserted because
/// switching the write to an append keeps every other test green and the
/// agent gets a stale paragraph above a current one — with no way to tell
/// which of the two describes the tree in front of it.
#[test]
fn a_second_sync_replaces_the_note_rather_than_stacking_on_it() {
    let (_d, wt, shadow_dir) = fixture();
    let s = Shadow::new(&shadow_dir, "s01");
    s.ensure(&wt, &[]).unwrap();

    s.leave_note(&note_for("main", 3, 0)).unwrap();
    s.leave_note(&note_for("main", 1, 2)).unwrap();

    let left = std::fs::read_to_string(note_file(&s.gitdir)).unwrap();
    assert!(
        left.contains("moved 1 commit") && !left.contains("moved 3 commits"),
        "the second note is the one that is there: {left}"
    );
    assert_eq!(left.lines().count(), 1, "one sentence, not two: {left}");
    // Nothing left beside it either — the write goes through a temporary
    // so a half-written note can never be delivered as a whole one, and a
    // temporary that outlived its rename would be a file `git status`
    // inside the sandbox starts reporting.
    let strays: Vec<_> = std::fs::read_dir(&s.gitdir)
        .unwrap()
        .filter_map(|e| e.ok().map(|e| e.file_name().to_string_lossy().into_owned()))
        .filter(|n| n.contains("omh-note") && n != "omh-note")
        .collect();
    assert!(strays.is_empty(), "no scaffolding left behind: {strays:?}");
}

/// A note that cannot be written does not fail the sync, and does not
/// leave a fragment behind claiming to be one.
///
/// Both halves matter and the second is why the write goes through a
/// rename. `fs::write` truncates first, so a disk that fills mid-sentence
/// leaves a prefix on disk *and* returns an error — and the caller, told
/// the note failed, warns that the agent will find nothing while the agent
/// is handed half a sentence with the `git show HEAD` clause missing.
#[test]
fn a_note_that_cannot_be_written_leaves_nothing_claiming_to_be_one() {
    let (_d, wt, shadow_dir) = fixture();
    let s = Shadow::new(&shadow_dir, "s01");
    s.ensure(&wt, &[]).unwrap();

    // A directory where the note goes: the rename cannot replace it, which
    // is a write failure omh can produce on demand.
    std::fs::create_dir(note_file(&s.gitdir)).unwrap();
    let why = s.leave_note(&note_for("main", 3, 0)).unwrap_err();
    assert!(
        format!("{why:#}").contains("omh-note"),
        "the failure names the file: {why:#}"
    );
    let strays: Vec<_> = std::fs::read_dir(&s.gitdir)
        .unwrap()
        .filter_map(|e| e.ok().map(|e| e.file_name().to_string_lossy().into_owned()))
        .filter(|n| n.contains("omh-note") && n != "omh-note")
        .collect();
    assert!(
        strays.is_empty(),
        "and the half-written one is cleaned up rather than left to be read: {strays:?}"
    );
}

/// The hook photographs the tree and changes nothing the agent can see.
///
/// The four properties its doc claims, against the shipped command:
///
/// - HEAD does not move, the index is untouched, and `git status` says
///   afterwards exactly what it said before. This is the whole reason it
///   is a snapshot and not a `git commit` — an agent whose working tree
///   went clean at the end of every turn would find nothing to commit and
///   would rightly conclude omh had eaten its work.
/// - a turn that changed nothing writes nothing, or an idle agent adds an
///   identical commit for as long as the session lives.
/// - the snapshots chain, so the ref is a timeline rather than a single
///   photograph.
#[test]
fn a_turn_photographs_the_tree_and_leaves_the_agents_own_state_alone() {
    let (_d, wt, shadow_dir) = fixture();
    let s = Shadow::new(&shadow_dir, "s01");
    s.ensure(&wt, &[]).unwrap();
    let head = |what: &str| git(&s.gitdir, &wt, &["rev-parse", what]).unwrap();
    let status = || git(&s.gitdir, &wt, &["status", "--porcelain"]).unwrap();

    assert_eq!(
        s.turns(&wt).unwrap(),
        None,
        "a session that has never finished a turn has no ref, which is not \
         the same answer as a count of none"
    );

    std::fs::write(wt.join("in-flight.rs"), "fn later() {}\n").unwrap();
    let before = (head("HEAD"), status());

    turn_snapshot(&s, &wt);
    assert_eq!(
        s.turns(&wt).unwrap(),
        Some(1),
        "a dirty turn is photographed"
    );
    assert_eq!(
        (head("HEAD"), status()),
        before,
        "and HEAD and the working tree are exactly as the agent left them"
    );

    turn_snapshot(&s, &wt);
    assert_eq!(
        s.turns(&wt).unwrap(),
        Some(1),
        "a turn that changed nothing writes nothing"
    );

    std::fs::write(wt.join("in-flight.rs"), "fn later() { todo!() }\n").unwrap();
    turn_snapshot(&s, &wt);
    assert_eq!(
        s.turns(&wt).unwrap(),
        Some(2),
        "and the next change is its own"
    );

    // A timeline, not one photograph: the newest snapshot reaches the
    // first, which is what makes restoring an earlier turn's tree
    // possible at all.
    assert_eq!(
        git(&s.gitdir, &wt, &["rev-list", "--count", TURN_REF])
            .unwrap()
            .trim(),
        "2",
        "the snapshots chain"
    );
    // A sandbox omh cannot read is not a sandbox with no snapshots. This
    // is the arm `.unwrap_or(0)` used to swallow on the way to an
    // irreversible delete.
    let broken = Shadow::new(&shadow_dir, "s99");
    std::fs::create_dir_all(&broken.gitdir).unwrap();
    assert!(
        broken.turns(&wt).is_err(),
        "a directory that is not a repository has no answer, not zero"
    );

    // The shipped invocation stages into an index of its own, made fresh.
    // The assertion this replaces looked for `i=/omh/shadow/index ` with a
    // trailing space — a string no rendering could ever produce, since the
    // path is always followed by `;`. It passed for every possible value
    // of the index path, including the agent's own.
    let shipped = turn_hook_for_the_sandbox();
    assert!(
        shipped.contains("i=$(mktemp)") && !shipped.contains("/omh/shadow/index"),
        "a fresh index, never the agent's staging area: {shipped}"
    );
    assert!(
        shipped.contains("rm -f \"$i\""),
        "and it is cleaned up, so nothing accumulates in the mount: {shipped}"
    );

    // …and the agent's own work is still uncommitted, which is the state
    // the snapshot exists to be a rollback from.
    assert!(
        status().contains("in-flight.rs"),
        "the agent still has its work to commit: {}",
        status()
    );
}

/// An agent cannot make its own snapshots look like unharvested work.
///
/// `unkept`'s exclusion reaches `--all` and not `--reflog`, which is safe
/// only because git writes no reflog for a ref outside `refs/heads`,
/// `refs/remotes`, `refs/notes` and `HEAD`. `core.logAllRefUpdates =
/// always` flips exactly that, and the config is the agent's to write — so
/// the whole design rested on a default the sandbox could change.
///
/// Now the key is swept on every launch. This asserts the sweep, not the
/// git behaviour: with the value left in place, `unkept` finds the
/// snapshots through the reflog and `rm` refuses over omh's own commits
/// for the rest of the session.
#[test]
fn a_reflog_the_agent_turned_on_does_not_survive_the_next_launch() {
    let (_d, wt, shadow_dir) = fixture();
    let s = Shadow::new(&shadow_dir, "s01");
    s.ensure(&wt, &[]).unwrap();

    git(
        &s.gitdir,
        &wt,
        &["config", "core.logAllRefUpdates", "always"],
    )
    .unwrap();
    assert_eq!(
        git(
            &s.gitdir,
            &wt,
            &["config", "--get", "core.logallrefupdates"]
        )
        .unwrap()
        .trim(),
        "always",
        "the agent can set it"
    );

    s.ensure(&wt, &[]).unwrap();
    let after = Command::new("git")
        .arg("--git-dir")
        .arg(&s.gitdir)
        .args(["config", "--get", "core.logallrefupdates"])
        .output()
        .unwrap();
    assert!(
        !after.status.success(),
        "and the next launch takes it away, so the default the exclusion \
         relies on is the one in force: {after:?}"
    );
}

/// A hook that cannot do its job still ends the turn quietly.
///
/// The claim `turn_hook_command` makes and the one nothing was checking.
/// `every_hook_runs_quietly_when_its_tool_says_nothing` was credited with
/// it and does not do it: `when` renders as `[ -d /omh/shadow ] || exit 0`,
/// which is false on any host, so the body never executes there and the
/// test passes identically against a body of `exit 17`.
///
/// Three ways for it to be unable to work, all reachable in a sandbox: a
/// gitdir that is not there, a worktree that is not there, and a HEAD that
/// has never been born — `git checkout --orphan` is an ordinary move, and
/// before the `read-tree --empty` fallback it silently ended snapshots for
/// the rest of the session.
#[test]
fn a_hook_that_cannot_do_its_job_still_ends_the_turn_quietly() {
    let (_d, wt, shadow_dir) = fixture();
    let run = |gitdir: &str, worktree: &str| {
        let out = Command::new("sh")
            .arg("-c")
            .arg(turn_hook_command(gitdir, worktree))
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .output()
            .unwrap();
        assert!(
            out.status.success() && out.stdout.is_empty() && out.stderr.is_empty(),
            "a turn ends quietly whatever the hook found: {out:?}"
        );
    };

    run("/nowhere/gitdir", "/nowhere/worktree");
    run(shadow_dir.to_str().unwrap(), wt.to_str().unwrap());

    // An unborn HEAD, which is what the agent leaves behind after
    // `git checkout --orphan`. The snapshot must still be taken — the
    // whole point is a rollback target for a turn that went wrong, and
    // that is exactly the turn an orphan checkout produces.
    let s = Shadow::new(&shadow_dir, "s02");
    s.ensure(&wt, &[]).unwrap();
    git(
        &s.gitdir,
        &wt,
        &["checkout", "-q", "--orphan", "nothing-yet"],
    )
    .unwrap();
    std::fs::write(wt.join("in-flight.rs"), "fn later() {}\n").unwrap();
    turn_snapshot(&s, &wt);
    assert_eq!(
        s.turns(&wt).unwrap(),
        Some(1),
        "an unborn HEAD is still a tree worth photographing"
    );
}

/// A turn snapshot is omh's own, and none of the three guards that walk
/// refs may mistake it for the agent's stranded work.
///
/// Three queries see every ref in the sandbox, and a snapshot is by
/// construction not an ancestor of HEAD — which is exactly the shape all
/// three are hunting. Left alone they would each be permanently wrong, and
/// wrong in the direction that blocks:
///
/// - `preflight` refuses every `--keep`, naming omh's own commits as work
///   the agent stranded, and telling the user to go and delete them.
/// - `checkpoints().unreachable` prints that as a warning on every `log`,
///   and `incomplete()` is true forever.
/// - `unkept` feeds `at_stake`, so `omh sNN rm` refuses even for a session
///   whose work is entirely on the branch.
///
/// The three do **not** get the same fix, which is why this asserts them
/// apart. The first two ask *would a harvest drop commits?* — a snapshot
/// is never replayed, so it is not one of those. The third asks *what
/// would removing this destroy?* — a snapshot is a real answer to that,
/// and it is counted, just not as the agent's own work.
#[test]
fn a_turn_snapshot_is_not_the_agents_stranded_work() {
    let (_d, wt, shadow_dir) = fixture();
    let s = Shadow::new(&shadow_dir, "s01");
    s.ensure(&wt, &[]).unwrap();
    checkpoint(&s, &wt, "one.rs", "fn one() {}\n", "Add one");

    // Clean before: nothing stranded, nothing unkept beyond the commit
    // the agent just made.
    s.preflight(&wt).expect("a plain session harvests");
    let (unkept_before, _) = s.unkept(&wt).unwrap();

    std::fs::write(wt.join("in-flight.rs"), "fn later() {}\n").unwrap();
    turn_snapshot(&s, &wt);

    s.preflight(&wt)
        .expect("a snapshot is not a commit the harvest would drop");
    assert_eq!(
        s.checkpoints(&wt).unwrap().unreachable,
        0,
        "and `log` does not warn about it every time"
    );

    // Counted, because it is the only copy of a tree the agent may have
    // since thrown away — that is the whole reason the hook exists.
    let (unkept_after, _) = s.unkept(&wt).unwrap();
    assert_eq!(
        unkept_after, unkept_before,
        "but it is not counted as the agent's own unharvested commits"
    );
    assert_eq!(
        s.turns(&wt).unwrap(),
        Some(1),
        "it is counted as what it is, and separately"
    );
}

/// The numbers are the interface — `diff N` and `--keep 1,3-4` take them —
/// so they have to name the same commit tomorrow as today. Oldest first is
/// what makes that true: a new checkpoint appends, and nothing renumbers.
#[test]
fn checkpoints_are_numbered_from_the_oldest_and_never_renumber() {
    let (_d, wt, shadow_dir) = fixture();
    let s = Shadow::new(&shadow_dir, "s01");
    s.ensure(&wt, &[]).unwrap();

    checkpoint(&s, &wt, "one.rs", "fn one() {}\n", "Add one");
    checkpoint(&s, &wt, "two.rs", "fn two() {}\n", "Add two");
    let before: Vec<usize> = s
        .checkpoints(&wt)
        .unwrap()
        .commits
        .iter()
        .map(|c| c.number)
        .collect();
    let first_subject = s.checkpoints(&wt).unwrap().commits[0].subject.clone();

    checkpoint(&s, &wt, "three.rs", "fn three() {}\n", "Add three");
    let after = s.checkpoints(&wt).unwrap().commits;

    assert_eq!(before, vec![1, 2], "two commits, numbered from the oldest");
    assert_eq!(
        after.iter().map(|c| c.number).collect::<Vec<_>>(),
        vec![1, 2, 3],
        "a third appends rather than renumbering"
    );
    assert_eq!(
        after[0].subject, first_subject,
        "number 1 is still the same commit it was"
    );
    assert_eq!(after[0].subject, "Add one");
    assert_eq!(after[2].subject, "Add three");
}

/// The seed is omh's commit, not the agent's, and it holds the whole
/// starting tree: counted as a checkpoint it would be number 1 in every
/// session, offering the user a review of their own files.
#[test]
fn the_seed_is_not_one_of_the_agents_checkpoints() {
    let (_d, wt, shadow_dir) = fixture();
    let s = Shadow::new(&shadow_dir, "s01");
    s.ensure(&wt, &[]).unwrap();

    assert!(
        s.checkpoints(&wt).unwrap().commits.is_empty(),
        "a sandbox that has committed nothing has nothing to show"
    );

    checkpoint(&s, &wt, "one.rs", "fn one() {}\n", "Add one");
    let one = s.checkpoints(&wt).unwrap().commits;
    assert_eq!(
        one.len(),
        1,
        "the agent's commit, and not the seed: {one:?}"
    );
}

/// Which work is already the branch's, from the replay point — the same
/// record `--keep` replays from, so the line drawn here is the line the
/// next harvest will act on rather than a second opinion about it.
#[test]
fn checkpoints_say_which_have_already_been_handed_over() {
    let (_d, wt, shadow_dir) = fixture();
    let s = Shadow::new(&shadow_dir, "s01");
    s.ensure(&wt, &[]).unwrap();

    checkpoint(&s, &wt, "one.rs", "fn one() {}\n", "Add one");
    let handed = checkpoint(&s, &wt, "two.rs", "fn two() {}\n", "Add two");
    checkpoint(&s, &wt, "three.rs", "fn three() {}\n", "Add three");

    assert!(
        s.checkpoints(&wt)
            .unwrap()
            .commits
            .iter()
            .all(|c| !c.landed),
        "nothing has been handed over yet"
    );

    std::fs::write(&s.landed_record, format!("{handed}\n")).unwrap();
    let after = s.checkpoints(&wt).unwrap().commits;

    assert_eq!(
        after.iter().map(|c| c.landed).collect::<Vec<_>>(),
        vec![true, true, false],
        "the line falls after what the last harvest took: {after:?}"
    );
}

/// What a checkpoint touched, so the log answers *is this worth reading*
/// without a second command.
#[test]
fn a_checkpoint_reports_what_it_touched() {
    let (_d, wt, shadow_dir) = fixture();
    let s = Shadow::new(&shadow_dir, "s01");
    s.ensure(&wt, &[]).unwrap();

    std::fs::write(wt.join("a.rs"), "one\ntwo\nthree\n").unwrap();
    std::fs::write(wt.join("b.rs"), "one\n").unwrap();
    git(&s.gitdir, &wt, &["add", "-A", "."]).unwrap();
    git(
        &s.gitdir,
        &wt,
        &["commit", "-q", "--no-verify", "-m", "Add two files"],
    )
    .unwrap();
    // …then change one of them, so added and removed are different numbers
    // and neither can stand in for the other.
    std::fs::write(wt.join("a.rs"), "one\nchanged\nthree\nfour\n").unwrap();
    git(&s.gitdir, &wt, &["add", "-A", "."]).unwrap();
    git(
        &s.gitdir,
        &wt,
        &["commit", "-q", "--no-verify", "-m", "Change a"],
    )
    .unwrap();

    let all = s.checkpoints(&wt).unwrap().commits;
    let touched = |c: &Checkpoint| {
        let t = c.touched.clone().expect("not a merge");
        (t.files, t.added, t.removed, t.uncounted)
    };
    assert_eq!(touched(&all[0]), (2, 4, 0, 0));
    assert_eq!(touched(&all[1]), (1, 2, 1, 0));
}

/// The agent can rewrite what the log says without touching a commit.
///
/// `git replace` swaps one object for another at read time, and the gitdir
/// is mounted read-write — the read-only `config` does not cover
/// `refs/replace/`. Measured 2026-08-23 against git 2.55.0: with a
/// replacement in place the log printed a forged subject and a forged
/// (empty) file list beside the **real** commit id, on stdout, with nothing
/// on stderr. The harvest fetches the real object, so `--keep` would land
/// something the review never showed.
///
/// This is the one command whose whole purpose is to be believed.
#[test]
fn a_replaced_object_cannot_change_what_the_log_says() {
    let (_d, wt, shadow_dir) = fixture();
    let s = Shadow::new(&shadow_dir, "s01");
    s.ensure(&wt, &[]).unwrap();
    let seed = s.seed().unwrap();
    let real = checkpoint(
        &s,
        &wt,
        "creds.rs",
        "const KEY: &str = \"…\";\n",
        "real work",
    );

    // The agent forges an innocent-looking commit and points the real id at
    // it. Plain `git`, not this module's helper: this is the sandbox
    // acting, not omh.
    let forged = Command::new("git")
        .arg("--git-dir")
        .arg(&s.gitdir)
        .args([
            "commit-tree",
            &format!("{seed}^{{tree}}"),
            "-p",
            &seed,
            "-m",
            "tidy whitespace",
        ])
        .output()
        .unwrap();
    let forged = String::from_utf8_lossy(&forged.stdout).trim().to_string();
    Command::new("git")
        .arg("--git-dir")
        .arg(&s.gitdir)
        .args(["replace", "-f", &real, &forged])
        .output()
        .unwrap();

    let commits = s.checkpoints(&wt).unwrap().commits;
    assert_eq!(commits.len(), 1);
    assert_eq!(
        commits[0].subject, "real work",
        "the log reports the commit, not what the agent replaced it with"
    );
    assert_eq!(
        commits[0].touched.as_ref().map(|t| t.files),
        Some(1),
        "and what it really touched: {commits:?}"
    );
}

/// `info/grafts` rewrites ancestry from a file inside the mount.
///
/// Two commits vanish from the list and git says so only in a deprecation
/// hint on **stderr**, which `git` here discards on success. A review
/// silently shortened to one line, over an exit code of 0, is how a user
/// concludes there is little to review and runs `rm`.
///
/// Measured 2026-08-23: neither `--no-replace-objects` nor
/// `-c core.graftsFile=` stops this — the config key is simply not
/// consulted. `GIT_GRAFT_FILE` pointed away from the gitdir is, which is
/// why `GUEST_ENV` exists at all and why this guard is not in
/// `NEUTRALISED` beside the others.
#[test]
fn a_graft_file_cannot_hide_commits_from_the_log() {
    let (_d, wt, shadow_dir) = fixture();
    let s = Shadow::new(&shadow_dir, "s01");
    s.ensure(&wt, &[]).unwrap();
    let seed = s.seed().unwrap();
    checkpoint(&s, &wt, "one.rs", "fn one() {}\n", "Add one");
    checkpoint(&s, &wt, "two.rs", "fn two() {}\n", "Add two");
    let head = checkpoint(&s, &wt, "three.rs", "fn three() {}\n", "Add three");

    std::fs::create_dir_all(s.gitdir.join("info")).unwrap();
    std::fs::write(s.gitdir.join("info/grafts"), format!("{head} {seed}\n")).unwrap();

    let commits = s.checkpoints(&wt).unwrap().commits;
    assert_eq!(
        commits
            .iter()
            .map(|c| c.subject.as_str())
            .collect::<Vec<_>>(),
        vec!["Add one", "Add two", "Add three"],
        "every checkpoint is still there: {commits:?}"
    );
}

/// A merge has no diff of its own, and *0 files* is a measurement.
///
/// git prints the header for a merge and no numstat lines at all, so
/// counting the absence renders a merge that brought in a whole branch
/// exactly like an empty commit.
#[test]
fn a_merge_says_so_rather_than_reporting_that_it_touched_nothing() {
    let (_d, wt, shadow_dir) = fixture();
    let s = Shadow::new(&shadow_dir, "s01");
    s.ensure(&wt, &[]).unwrap();
    let seed = s.seed().unwrap();
    checkpoint(&s, &wt, "main.rs", "fn main() {}\n", "On the branch");
    git(&s.gitdir, &wt, &["checkout", "-q", "-b", "side", &seed]).unwrap();
    checkpoint(&s, &wt, "side.rs", "fn side() {}\n", "On the side");
    git(&s.gitdir, &wt, &["checkout", "-q", "-"]).unwrap();
    git(
        &s.gitdir,
        &wt,
        &["merge", "-q", "--no-ff", "side", "-m", "Merge the side"],
    )
    .unwrap();

    let commits = s.checkpoints(&wt).unwrap().commits;
    let merge = commits
        .iter()
        .find(|c| c.subject == "Merge the side")
        .unwrap_or_else(|| panic!("the merge is a checkpoint too: {commits:?}"));
    assert_eq!(merge.touched, None, "not measured, and not zero: {merge:?}");
    assert!(
        commits.iter().any(|c| c.touched.is_some()),
        "the ordinary commits are still measured"
    );
}

/// The numbers survive a merge, which is the case that broke them.
///
/// Measured 2026-08-23: `--reverse` alone is commit-date order, so a side
/// branch whose commits are older is spliced into the middle of the list
/// and everything after it shifts down. The guard for the numbering
/// invariant only appended to a linear history and never saw this.
#[test]
fn a_merge_does_not_renumber_the_checkpoints_already_listed() {
    let (_d, wt, shadow_dir) = fixture();
    let s = Shadow::new(&shadow_dir, "s01");
    s.ensure(&wt, &[]).unwrap();
    let seed = s.seed().unwrap();

    // Dated so the side branch is *older* than what follows it on the
    // branch — the arrangement date order gets wrong.
    let at = |when: &str, file: &str, subject: &str| {
        std::fs::write(wt.join(file), format!("// {subject}\n")).unwrap();
        git(&s.gitdir, &wt, &["add", "-A", "."]).unwrap();
        let out = Command::new("git")
            .envs(GUEST_ENV)
            .arg("--git-dir")
            .arg(&s.gitdir)
            .arg("--work-tree")
            .arg(&wt)
            .env("GIT_AUTHOR_DATE", when)
            .env("GIT_COMMITTER_DATE", when)
            .args(["commit", "-q", "--no-verify", "-m", subject])
            .output()
            .unwrap();
        assert!(out.status.success(), "{out:?}");
    };
    at("2026-08-20T11:00:00", "a.rs", "A");
    at("2026-08-20T13:00:00", "b.rs", "B");
    let before = s.checkpoints(&wt).unwrap().commits;

    git(&s.gitdir, &wt, &["checkout", "-q", "-b", "side", &seed]).unwrap();
    at("2026-08-20T12:00:00", "s.rs", "S, older than B");
    git(&s.gitdir, &wt, &["checkout", "-q", "-"]).unwrap();
    git(
        &s.gitdir,
        &wt,
        &["merge", "-q", "--no-ff", "side", "-m", "M"],
    )
    .unwrap();
    let after = s.checkpoints(&wt).unwrap().commits;

    for was in &before {
        let now = after
            .iter()
            .find(|c| c.id == was.id)
            .unwrap_or_else(|| panic!("{} left the list entirely", was.subject));
        assert_eq!(
            now.number, was.number,
            "{} was {} and is now {} — a selection typed before the merge would \
             land a different commit",
            was.subject, was.number, now.number
        );
    }
}

/// A replay point the history no longer reaches is a question, not a list
/// of new work.
///
/// `rev-list seed..landed` *succeeds* in this state — measured: exit 0, the
/// ids still resolve — so without asking about ancestry the set simply
/// matches nothing, every checkpoint reads as new, and the log offers a
/// `--keep` that `harvest` refuses for this exact reason.
#[test]
fn a_replay_point_the_history_lost_is_reported_rather_than_read_as_new_work() {
    let (_d, wt, shadow_dir) = fixture();
    let s = Shadow::new(&shadow_dir, "s01");
    s.ensure(&wt, &[]).unwrap();
    let seed = s.seed().unwrap();
    checkpoint(&s, &wt, "one.rs", "fn one() {}\n", "Add one");
    let handed = checkpoint(&s, &wt, "two.rs", "fn two() {}\n", "Add two");
    std::fs::write(&s.landed_record, format!("{handed}\n")).unwrap();
    assert!(
        !s.checkpoints(&wt).unwrap().replay_point_lost,
        "nothing is wrong yet"
    );

    // The agent rewinds below the replay point — one of the four commands
    // this repository exists to give back.
    git(&s.gitdir, &wt, &["reset", "-q", "--hard", &seed]).unwrap();
    checkpoint(&s, &wt, "other.rs", "fn other() {}\n", "Different work");
    let read = s.checkpoints(&wt).unwrap();

    assert!(
        read.replay_point_lost,
        "omh cannot tell what the branch already has: {read:?}"
    );
    assert!(
        read.commits.iter().all(|c| !c.landed),
        "and marks nothing as handed over on a guess"
    );
}

/// Work on a branch the sandbox wandered off is invisible to this read, and
/// `preflight` refuses to harvest over it.
///
/// Reported rather than hidden, because the alternative is a user reading a
/// clean review and then being refused by `--keep` citing commits they have
/// never been shown.
#[test]
fn commits_this_read_cannot_reach_are_counted_and_said() {
    let (_d, wt, shadow_dir) = fixture();
    let s = Shadow::new(&shadow_dir, "s01");
    s.ensure(&wt, &[]).unwrap();
    checkpoint(&s, &wt, "one.rs", "fn one() {}\n", "Add one");
    assert_eq!(s.checkpoints(&wt).unwrap().unreachable, 0);

    git(&s.gitdir, &wt, &["checkout", "-q", "-b", "spike"]).unwrap();
    checkpoint(&s, &wt, "spike.rs", "fn spike() {}\n", "A spike");
    checkpoint(&s, &wt, "spike2.rs", "fn more() {}\n", "More of it");
    git(&s.gitdir, &wt, &["checkout", "-q", "-"]).unwrap();

    let read = s.checkpoints(&wt).unwrap();
    assert_eq!(
        read.unreachable, 2,
        "two commits are on no branch this read follows: {read:?}"
    );
    assert_eq!(read.commits.len(), 1, "and the list cannot show them");
}

/// Binary files count as files and never as zero lines.
///
/// git answers `-` for a file it would not count, and a blank churn column
/// beside *1 file* is how a 200MB blob reads as a mode-bit change.
#[test]
fn a_file_git_would_not_count_is_not_counted_as_nothing() {
    let (_d, wt, shadow_dir) = fixture();
    let s = Shadow::new(&shadow_dir, "s01");
    s.ensure(&wt, &[]).unwrap();
    std::fs::write(wt.join("blob.bin"), [0u8, 1, 2, 0, 255]).unwrap();
    std::fs::write(wt.join("text.rs"), "one\ntwo\n").unwrap();
    git(&s.gitdir, &wt, &["add", "-A", "."]).unwrap();
    git(
        &s.gitdir,
        &wt,
        &["commit", "-q", "--no-verify", "-m", "Both kinds"],
    )
    .unwrap();

    let touched = s.checkpoints(&wt).unwrap().commits[0]
        .touched
        .clone()
        .expect("not a merge");
    assert_eq!(
        (touched.files, touched.added, touched.uncounted),
        (2, 2, 1),
        "both files counted, the text lines counted, the blob's not invented: {touched:?}"
    );
}

/// The age comes from a clock, and two answers it must never give are a
/// panic and a confident *just now*.
///
/// `%ct` is the committer date, so this dates the commit rather than the
/// authorship a rebase would have carried over.
#[test]
fn a_checkpoint_dated_in_the_future_reads_as_just_now_rather_than_failing() {
    let (_d, wt, shadow_dir) = fixture();
    let s = Shadow::new(&shadow_dir, "s01");
    s.ensure(&wt, &[]).unwrap();
    checkpoint(&s, &wt, "now.rs", "fn now() {}\n", "Made now");

    std::fs::write(wt.join("later.rs"), "fn later() {}\n").unwrap();
    git(&s.gitdir, &wt, &["add", "-A", "."]).unwrap();
    let out = Command::new("git")
        .envs(GUEST_ENV)
        .arg("--git-dir")
        .arg(&s.gitdir)
        .arg("--work-tree")
        .arg(&wt)
        .env("GIT_COMMITTER_DATE", "2099-01-01T00:00:00")
        .args(["commit", "-q", "--no-verify", "-m", "From the future"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{out:?}");

    // …and one carrying an old *author* date, which is what `rebase`,
    // `cherry-pick` and `--amend` leave behind. The list is ordered by
    // commit date, so an age read from the author date would run
    // backwards down a list the reader takes as chronological — and this
    // is the only arrangement that tells the two dates apart. The
    // future-dated commit above cannot: setting the committer date alone
    // leaves the author date at *now*, so both readings answer zero.
    std::fs::write(wt.join("replayed.rs"), "fn replayed() {}\n").unwrap();
    git(&s.gitdir, &wt, &["add", "-A", "."]).unwrap();
    let out = Command::new("git")
        .envs(GUEST_ENV)
        .arg("--git-dir")
        .arg(&s.gitdir)
        .arg("--work-tree")
        .arg(&wt)
        .env("GIT_AUTHOR_DATE", "2020-01-01T00:00:00")
        .args([
            "commit",
            "-q",
            "--no-verify",
            "-m",
            "Written long ago, committed now",
        ])
        .output()
        .unwrap();
    assert!(out.status.success(), "{out:?}");

    let commits = s.checkpoints(&wt).unwrap().commits;
    assert_eq!(
        commits[0].age.map(|age| age < 300),
        Some(true),
        "a commit made now is dated now: {:?}",
        commits[0]
    );
    assert_eq!(
        commits[1].age,
        Some(0),
        "and one dated in the future reads as just now: {:?}",
        commits[1]
    );
    assert_eq!(
        commits[2].age.map(|age| age < 300),
        Some(true),
        "and one written years ago but committed now is dated by the commit: {:?}",
        commits[2]
    );
}

/// A number names one checkpoint, and the patch is that commit's.
#[test]
fn a_checkpoint_number_shows_that_checkpoints_own_patch() {
    let (_d, wt, shadow_dir) = fixture();
    let s = Shadow::new(&shadow_dir, "s01");
    s.ensure(&wt, &[]).unwrap();
    checkpoint(&s, &wt, "one.rs", "fn one() {}\n", "Add one");
    let second = checkpoint(&s, &wt, "two.rs", "fn two() {}\n", "Add two");

    let patch = s.show(&wt, 2, crate::session::What::Patch).unwrap();
    assert!(
        patch.contains("+fn two() {}") && !patch.contains("+fn one() {}"),
        "checkpoint 2 is the second commit and nothing else: {patch}"
    );

    // Against git's own answer for that object, so the numbering and the
    // patch cannot drift apart while both look plausible.
    let theirs = Command::new("git")
        .arg("--git-dir")
        .arg(&s.gitdir)
        .args(["show", "-p", &second])
        .output()
        .unwrap();
    assert_eq!(
        patch,
        String::from_utf8_lossy(&theirs.stdout),
        "the same commit git would show for that id"
    );
}

/// A subject the agent chose cannot repaint a checkpoint review.
///
/// `git show` prints the subject, and git quotes paths but not subjects —
/// measured during the log work and reaching omh by a second route here.
/// Asserted through the report, because that is where the rule lives:
/// sanitised for a person, raw for a program.
#[test]
fn a_subject_the_agent_wrote_cannot_repaint_a_checkpoint_review() {
    let (_d, wt, shadow_dir) = fixture();
    let s = Shadow::new(&shadow_dir, "s01");
    s.ensure(&wt, &[]).unwrap();
    checkpoint(
        &s,
        &wt,
        "one.rs",
        "fn one() {}\n",
        "Fix \u{1b}[2K\rnothing at all",
    );

    let body = s.show(&wt, 1, crate::session::What::Summary).unwrap();
    let report = crate::report::Diff {
        label: "s01 checkpoint 1".into(),
        session: "s01".into(),
        checkpoint: Some(1),
        base: "its parent".into(),
        what: crate::session::What::Summary,
        body,
    };
    use crate::out::Report;
    let printed = report.human(&crate::out::Palette::plain());

    assert!(
        !printed.chars().any(|c| c.is_control() && c != '\n'),
        "no control character survives into omh's own output: {printed:?}"
    );
    assert!(
        printed.contains("nothing at all"),
        "the words still arrive: {printed}"
    );
    assert!(
        report.json()["summary"]
            .as_str()
            .is_some_and(|s| s.contains('\u{1b}')),
        "and a program still gets what git said: {}",
        report.json()
    );
}

/// A merge's summary and its patch describe the same change.
///
/// Measured 2026-08-23 against git 2.55.0: `show --stat` on a merge reports
/// the files it brought in, and `show -p` on the same merge prints
/// **nothing** — git's default `--cc` collapses a clean merge to an empty
/// diff. So `omh sNN diff 3` promised a change and `omh sNN diff 3 -p`, the
/// very next command, showed none. `--first-parent` makes them agree, and
/// changes neither answer on an ordinary commit.
#[test]
fn a_merges_summary_and_its_patch_do_not_contradict_each_other() {
    let (_d, wt, shadow_dir) = fixture();
    let s = Shadow::new(&shadow_dir, "s01");
    s.ensure(&wt, &[]).unwrap();
    let seed = s.seed().unwrap();
    checkpoint(&s, &wt, "main.rs", "fn main() {}\n", "On the branch");
    git(&s.gitdir, &wt, &["checkout", "-q", "-b", "side", &seed]).unwrap();
    checkpoint(&s, &wt, "side.rs", "fn side() {}\n", "On the side");
    git(&s.gitdir, &wt, &["checkout", "-q", "-"]).unwrap();
    git(
        &s.gitdir,
        &wt,
        &["merge", "-q", "--no-ff", "side", "-m", "Merge the side"],
    )
    .unwrap();
    let merge = s
        .checkpoints(&wt)
        .unwrap()
        .commits
        .iter()
        .find(|c| c.subject == "Merge the side")
        .map(|c| c.number)
        .expect("the merge is a checkpoint");

    let summary = s.show(&wt, merge, crate::session::What::Summary).unwrap();
    let patch = s.show(&wt, merge, crate::session::What::Patch).unwrap();

    assert!(
        summary.contains("side.rs"),
        "the summary names what the merge brought in: {summary}"
    );
    assert!(
        patch.contains("side.rs") && patch.contains("+fn side() {}"),
        "and the patch shows it, rather than being empty: {patch}"
    );
}

/// The user's pager is the last one named, so it is the one that runs.
///
/// `NEUTRALISED` pins `core.pager` to `cat` so a host-side read never runs
/// what the sandbox's config says — measured on a pty, a `core.pager` of
/// `sh -c "echo …; cat"` executes on a plain `git show`. Appending the
/// user's after it is what leaves paging working, and the *order* is the
/// whole mechanism: put it first and `cat` wins and nothing ever pages.
///
/// Asserted on the argument list rather than through a terminal, because
/// the invariant is an ordering and a captured-output test cannot see a
/// pager at all — git consults one only when stdout is a tty.
#[test]
fn the_pager_omh_names_last_is_the_users_own() {
    let ours = NEUTRALISED
        .iter()
        .position(|kv| kv.starts_with("core.pager="))
        .expect("the sandbox's pager is pinned");
    assert_eq!(
        NEUTRALISED[ours], "core.pager=cat",
        "pinned to something that cannot run anything"
    );

    // The command is built the same way `stream_show` builds it.
    let pinned: Vec<String> = NEUTRALISED
        .iter()
        .flat_map(|kv| ["-c".to_string(), kv.to_string()])
        .collect();
    let mine = format!("core.pager={}", user_pager(std::path::Path::new(".")));
    let full: Vec<String> = pinned
        .iter()
        .cloned()
        .chain(["-c".to_string(), mine.clone()])
        .collect();

    let at = |needle: &str| full.iter().position(|a| a == needle);
    assert!(
        at(&mine) > at("core.pager=cat"),
        "the user's pager comes after the sandbox's, or the sandbox's wins: {full:?}"
    );
    assert!(!mine.ends_with('='), "and it names something: {mine}");
}

/// A checkpoint read carries every guard an ordinary read carries.
///
/// `stream_show` builds its own invocation rather than going through
/// `git`, which is how a read ends up outside the list that makes reads
/// safe. Asserted on the arguments so the two cannot drift apart quietly.
#[test]
fn a_checkpoint_read_is_guarded_like_every_other_read() {
    let args = show_args(crate::session::What::Patch, "abc123", "never");
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    let guarded = guarded(&args);

    assert_eq!(guarded.first().map(String::as_str), Some("show"));
    assert!(
        guarded.contains(&"--no-textconv".to_string())
            && guarded.contains(&"--no-ext-diff".to_string()),
        "the flags a read has to carry, after the verb: {guarded:?}"
    );
    assert!(
        guarded.contains(&"--first-parent".to_string()),
        "so a merge's patch is not empty: {guarded:?}"
    );
    assert!(
        guarded.contains(&"--color=never".to_string()),
        "and colour is omh's decision, not git's guess: {guarded:?}"
    );
}

/// A number outside the range is refused, and the refusal says what the
/// numbers are.
///
/// The list is the only place these numbers come from, so being told *no
/// checkpoint 9* without being told there are three is being told to go
/// and run the other command.
#[test]
fn a_checkpoint_number_the_session_does_not_have_is_refused() {
    let (_d, wt, shadow_dir) = fixture();
    let s = Shadow::new(&shadow_dir, "s01");
    s.ensure(&wt, &[]).unwrap();

    let empty = s
        .show(&wt, 1, crate::session::What::Summary)
        .expect_err("nothing has been committed here");
    assert!(
        empty.to_string().contains("not committed anything"),
        "an empty sandbox says so rather than naming a range it does not have: {empty}"
    );

    checkpoint(&s, &wt, "one.rs", "fn one() {}\n", "Add one");
    checkpoint(&s, &wt, "two.rs", "fn two() {}\n", "Add two");
    let err = s
        .show(&wt, 9, crate::session::What::Summary)
        .expect_err("there is no checkpoint 9");
    assert!(
        err.to_string().contains('9') && err.to_string().contains("1 to 2"),
        "the refusal names both the number and the range: {err}"
    );
    // Zero is outside it too, and is what a reader who assumed the list was
    // zero-based would type first.
    assert!(
        s.show(&wt, 0, crate::session::What::Summary).is_err(),
        "the numbers start at 1"
    );
}

/// The options a listing declares, read the way git writes them.
///
/// The shapes are taken from real `git <verb> -h` output rather than
/// invented, because the first version of this was written against an
/// invented sample: it matched only an option at the start of a line, and
/// git puts the short alias first whenever there is one. Against the real
/// cherry-pick listing it answered *no* to eight of the nine options that
/// command has.
#[test]
fn an_option_listing_is_read_the_way_git_writes_it() {
    // Verbatim shapes from git 2.55.0.
    let help = "usage: git cherry-pick [--edit] [-n] [-m <parent-number>]\n\
        \x20   --quit                end revert or cherry-pick sequence\n\
        \x20   -n, --no-commit       don't automatically commit\n\
        \x20   -e, --[no-]edit       edit the commit message\n\
        \x20   --[no-]ff             allow fast-forward\n\
        \x20   --empty (stop|drop|keep)\n\
        \x20                         how to handle commits that become empty\n\
        \x20   -S, --[no-]gpg-sign[=<key-id>]\n\
        \x20                         GPG sign commit\n\
        \x20   --[no-]keep-redundant-commits\n\
        \x20                         deprecated: use --empty=keep instead\n\
        \x20   --verify              opposite of --no-rebase-merges\n";
    let found = options_in(help);
    let has = |o: &str| found.contains(o);

    assert!(has("--empty"), "the option omh actually asks about");
    // A long form behind a short one. The first version missed every one
    // of these, and its own test comment called `--no-commit` eternal.
    assert!(has("--no-commit") && has("-n"));
    assert!(has("--edit") && has("-e"), "and behind a negation as well");
    // A negatable option declares both spellings.
    assert!(has("--ff") && has("--no-ff"));
    assert!(has("--gpg-sign"), "…and one that also takes a value");
    assert!(has("--keep-redundant-commits"));

    assert!(
        !has("--empty=keep"),
        "a description that *mentions* an option does not declare one — this line \
         says `deprecated: use --empty=keep instead`"
    );
    assert!(!has("--no-such-option"), "and absence is absence");
    // The same rule on one line rather than two. `--verify  opposite of
    // --no-verify` is verbatim from `git rebase -h`: what follows the
    // description column is prose, and prose that names an option is not a
    // git that has it.
    assert!(has("--verify"), "the declaration is read");
    assert!(
        !has("--no-rebase-merges"),
        "and its description is not — that option is not declared here"
    );
    assert!(
        options_in("no options here at all\n").is_empty(),
        "text that is not a listing declares nothing, which is not the same as an \
         option being absent"
    );
}

/// The verb selects the listing. It is not decoration.
///
/// Asking `cherry-pick` and reading `commit`'s answer is the mutation the
/// first version of this test could not see: every plausible verb lists
/// `-n`, so `-n` alone proves nothing about which one was asked. Both
/// options here are ancient, so this does not go red as git grows.
#[test]
fn git_answers_for_the_verb_it_was_asked_about() {
    assert!(git_supports("merge", "--ff-only").unwrap());
    assert!(
        !git_supports("cherry-pick", "--ff-only").unwrap(),
        "cherry-pick has no --ff-only, and asking it must not answer for merge"
    );
    assert!(git_supports("cherry-pick", "--empty").unwrap());
    assert!(
        git_supports("cherry-pick", "--empty=").unwrap(),
        "the spelling every comment in this tree uses is the same question"
    );
    assert!(!git_supports("cherry-pick", "--no-such-option-ever").unwrap());
}

/// Not being able to ask is not an answer.
///
/// A verb git does not know prints a suggestion rather than a listing, so
/// nothing is declared — which must be an error, not *this option is
/// absent*. Collapsed into `false`, a user with no git on PATH was told
/// their git was too old to name checkpoints.
#[test]
fn a_git_that_cannot_answer_is_not_a_git_that_said_no() {
    let err = git_supports("not-a-git-verb-at-all", "--empty")
        .expect_err("nothing was listed, so nothing can be concluded");
    assert!(
        err.to_string().contains("listed no options"),
        "the refusal says what happened: {err}"
    );
}

/// `--keep 1,3-4` means those checkpoints, in that order.
#[test]
fn a_selection_names_checkpoints_in_the_order_it_lists_them() {
    assert_eq!(chosen("1,3-4", 4).unwrap(), vec![1, 3, 4]);
    assert_eq!(chosen("2", 4).unwrap(), vec![2], "one is a selection");
    assert_eq!(chosen("1-4", 4).unwrap(), vec![1, 2, 3, 4], "a whole range");
    // The user's order, not sorted. Reordering is half of what curating a
    // history is for, and a selection that silently sorted itself would
    // land a different history than the one on screen.
    assert_eq!(chosen("3,1", 4).unwrap(), vec![3, 1]);
    // Spaces are what a person types after a comma.
    assert_eq!(chosen(" 1, 3 - 4 ", 4).unwrap(), vec![1, 3, 4]);
}

/// Everything that is not a selection is refused, before anything moves.
///
/// Each of these is a plausible thing to type, and each would otherwise
/// resolve to *something* — an empty rebase, a commit picked twice, a
/// number that means a different commit than the one on screen.
#[test]
fn a_selection_that_cannot_mean_what_it_says_is_refused() {
    let refused = |spec: &str, because: &str| {
        let err = chosen(spec, 4)
            .map(|got| format!("{got:?}"))
            .expect_err(&format!("`{spec}` is not a selection: {because}"));
        err.to_string()
    };

    assert!(
        refused("", "nothing was named").contains("no checkpoints"),
        "an empty selection is a question, not everything"
    );
    assert!(refused("0", "the numbers start at 1").contains('0'));
    assert!(
        refused("9", "there are four").contains("1 to 4"),
        "the refusal names the range the list actually has"
    );
    assert!(refused("2-9", "the range runs past the end").contains("1 to 4"));
    assert!(refused("two", "not a number").contains("two"));
    assert!(refused("1,,2", "an empty element").contains("1,,2"));
    assert!(
        refused("4-2", "backwards").contains("4-2"),
        "a descending range is ambiguous — reversing it is a guess"
    );
    assert!(
        refused("1,1", "twice").contains('1'),
        "a commit picked twice applies twice, which is not what the list showed"
    );
    assert!(refused("-", "no numbers at all").contains('-'));
}

/// A selection is checked against the session's own list, not against
/// arithmetic.
#[test]
fn a_selection_is_bounded_by_what_the_session_has() {
    assert!(chosen("1", 1).is_ok());
    assert!(
        chosen("2", 1).is_err(),
        "one checkpoint means one valid number"
    );
    assert!(
        chosen("1", 0).is_err(),
        "and an empty sandbox has none at all"
    );
}

/// The isolation the sandbox is *for*, asserted as an invariant rather than
/// a mount list: whatever else the shadow gains, the checkout's commits are
/// never reachable from it. A shadow seeded by cloning, or by pointing at
/// the real object store to save disk, would pass every other test here and
/// hand the agent every branch you have.
#[test]
fn the_shadow_holds_no_commit_from_the_checkout() {
    let (d, wt, shadow_dir) = fixture();
    let checkout = d.path().join("checkout");
    let s = Shadow::new(&shadow_dir, "s01");
    s.ensure(&wt, &[]).unwrap();

    let host_commit = head_of(&checkout);
    let reachable = Command::new("git")
        .arg("--git-dir")
        .arg(&s.gitdir)
        .args(["cat-file", "-e", &host_commit])
        .output()
        .unwrap();
    assert!(
        !reachable.status.success(),
        "the host's history must not be reachable from the sandbox's repo"
    );

    let count = git(&s.gitdir, &wt, &["rev-list", "--all", "--count"]).unwrap();
    assert_eq!(count.trim(), "1", "the seed, and nothing else");
    let remotes = git(&s.gitdir, &wt, &["remote"]).unwrap();
    assert!(remotes.trim().is_empty(), "nowhere to push: {remotes:?}");
}

/// The seed is the only fixed point a harvest can replay from, and the
/// agent can write everywhere the container can reach. Recorded as a tag it
/// went away with one `git tag -d`; recorded in the gitdir's config the
/// agent rewrites it. So it lives on the host, outside the mount.
#[test]
fn the_seed_is_recorded_where_the_sandbox_cannot_reach_it() {
    let (_d, wt, shadow_dir) = fixture();
    let s = Shadow::new(&shadow_dir, "s01");
    s.ensure(&wt, &[]).unwrap();

    let seed = std::fs::read_to_string(&s.seed_record).unwrap();
    assert!(!seed.is_empty(), "a seed has to be recorded");
    assert!(
        !s.seed_record.starts_with(&s.gitdir),
        "the record must not sit inside the directory the agent can write"
    );

    // the agent does its worst inside its own repo
    let _ = git(&s.gitdir, &wt, &["tag", "-d", "session-start"]);
    assert_eq!(
        std::fs::read_to_string(&s.seed_record).unwrap(),
        seed,
        "the seed survives the sandbox"
    );
}

/// `carry_in` puts files the repo does not track into the worktree — the
/// user's `.env` among them. Left visible, the agent's first `git status`
/// offers to add a secret, and `git add -A` takes it.
#[test]
fn a_carried_file_is_not_something_the_shadow_tracks() {
    let (_d, wt, shadow_dir) = fixture();
    let s = Shadow::new(&shadow_dir, "s01");
    s.ensure(&wt, &[".env".to_string()]).unwrap();

    let tracked = git(&s.gitdir, &wt, &["ls-files"]).unwrap();
    assert!(
        !tracked.contains(".env"),
        "carried, not committed: {tracked}"
    );
    let status = git(&s.gitdir, &wt, &["status", "--porcelain"]).unwrap();
    assert!(
        status.trim().is_empty(),
        "a session opens on a clean tree or the agent starts by tidying: {status:?}"
    );
}

/// The one thing that stays walled. Everything else about this repo is
/// meant to work, so the agent has no standing reason to think a push
/// would — and `git push` reaching a real remote is the one mistake that
/// leaves the machine.
///
/// git's own hook rather than a shell pattern over the command line: git
/// knows what a push is, and the pattern omh used to match `git` at all
/// shipped broken once already by missing multi-line scripts.
#[test]
fn the_shadow_refuses_to_push() {
    let (_d, wt, shadow_dir) = fixture();
    let s = Shadow::new(&shadow_dir, "s01");
    s.ensure(&wt, &[]).unwrap();

    let hook = s.gitdir.join("hooks/pre-push");
    assert!(hook.exists(), "a pre-push hook has to be installed");

    let out = Command::new("sh").arg(&hook).output().unwrap();
    assert!(!out.status.success(), "the hook must refuse");

    // Asserting the *message*, not the exit code. A non-zero exit is what a
    // shell syntax error gives you too — and that is exactly what this
    // shipped: `NO_PUSH` contains an apostrophe, the script wrapped it in
    // single quotes, and the hook refused only by failing to parse. The
    // agent got `unexpected EOF while looking for matching \`'\`` and no
    // mention of omh at all, while a test asserting failure stayed green.
    let said = String::from_utf8_lossy(&out.stderr);
    assert!(
        said.contains("omh s commit"),
        "a refusal has to say where work actually leaves: {said}"
    );
    assert!(
        !said.contains("syntax error") && !said.contains("unexpected"),
        "the hook must run, not merely fail to parse: {said}"
    );

    // git skips a hook it cannot execute, with a *hint* and exit 0 — the
    // push then succeeds. A mode this test does not check is a wall that is
    // not there.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&hook).unwrap().permissions().mode();
        assert_eq!(mode & 0o111, 0o111, "git ignores a non-executable hook");
    }
}

/// Every other test here reaches the repository the way the *host* does,
/// through the private `git()` helper, which passes `--work-tree`. The
/// container never does that: it finds the repository through the `.git`
/// pointer omh mounts onto `/work`, and takes the worktree from wherever
/// that pointer sits.
///
/// The difference is not academic. A `core.worktree` written on the host
/// records a host path, outranks the pointer's own directory, and made
/// every git command in the sandbox fail with `fatal: Invalid path` — while
/// the whole suite stayed green, because `--work-tree` overrode the bad
/// value on every call a test made. This resolves with no `--work-tree` at
/// all, which is the only way that class of mistake is visible from here.
#[test]
fn the_pointer_file_alone_resolves_to_the_worktree_it_sits_in() {
    let (_d, wt, shadow_dir) = fixture();
    let s = Shadow::new(&shadow_dir, "s01");
    s.ensure(&wt, &[]).unwrap();

    // the container's view: discovery through the pointer, nothing else
    std::fs::write(wt.join(".git"), format!("gitdir: {}\n", s.gitdir.display())).unwrap();
    let out = Command::new("git")
        .current_dir(&wt)
        .args(["rev-parse", "--show-toplevel"])
        .output()
        .unwrap();

    assert!(
        out.status.success(),
        "git must work through the pointer alone: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&out.stdout).trim(),
        std::fs::canonicalize(&wt).unwrap().to_string_lossy(),
        "the worktree is where the pointer sits, not somewhere recorded on the host"
    );
}

/// `ensure` makes a repository out of seven subprocess calls, and a machine
/// that dies between the first and the last leaves a directory that looks
/// exactly like a finished one. Read as "seeded" because it exists, that
/// shadow opens the session on a repository with no seed and no exclude
/// list — the agent's first `git status` offers it the carried `.env`.
///
/// So the directory only appears once it is complete, and a leftover from
/// an attempt that did not get there is not mistaken for the real thing.
#[test]
fn a_half_built_shadow_is_never_mistaken_for_a_finished_one() {
    let (_d, wt, shadow_dir) = fixture();
    let s = Shadow::new(&shadow_dir, "s01");

    // what a launch killed partway through leaves behind
    Command::new("git")
        .args(["init", "-q", "--bare"])
        .arg(&s.gitdir)
        .output()
        .unwrap();
    assert!(!s.seed_record.exists(), "it never got as far as the seed");

    s.ensure(&wt, &[".env".to_string()]).unwrap();

    assert!(s.seed_record.exists(), "the seed has to be recorded");
    assert_eq!(
        git(&s.gitdir, &wt, &["rev-list", "--all", "--count"])
            .unwrap()
            .trim(),
        "1",
        "a finished shadow has its seed commit"
    );
    let status = git(&s.gitdir, &wt, &["status", "--porcelain"]).unwrap();
    assert!(!status.contains(".env"), "and its exclude list: {status:?}");
}

/// A hook the *agent* plants in its own gitdir must not run when omh
/// touches that gitdir from the host.
///
/// Nothing host-side reads an existing shadow today, so this cannot fire
/// yet — which is exactly why it is worth pinning now. The harvest
/// `Session::remove` already promises is a host-side reader of commits the
/// agent wrote, and it will be written by someone reading a doc comment.
#[test]
fn a_hook_the_sandbox_plants_does_not_run_on_the_host() {
    let (_d, wt, shadow_dir) = fixture();
    let s = Shadow::new(&shadow_dir, "s01");
    s.ensure(&wt, &[]).unwrap();

    // what an agent with a writable gitdir can arrange
    let planted = shadow_dir.join("planted");
    let hooks = shadow_dir.join("evil-hooks");
    std::fs::create_dir_all(&hooks).unwrap();
    let hook = hooks.join("post-checkout");
    std::fs::write(&hook, format!("#!/bin/sh\ntouch {}\n", planted.display())).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    // written the way the agent would: into the repository's own config
    Command::new("git")
        .arg("--git-dir")
        .arg(&s.gitdir)
        .args(["config", "core.hooksPath"])
        .arg(&hooks)
        .output()
        .unwrap();

    let _ = git(&s.gitdir, &wt, &["checkout", "-q", "--", "."]);

    assert!(
        !planted.exists(),
        "the sandbox's own config decided what ran on the host"
    );
}

/// "One string, two deliveries" is the reason `ARRANGEMENT` exists, and
/// only one of the two was pinned. The rules side is asserted three times
/// over; the seed commit's side was asserted nowhere, and replacing
/// `seed_message()` with a bare "The session starts here." left the whole
/// suite green.
///
/// This is the delivery the module doc calls the one a rules section
/// "cannot reach" — it arrives on `git log`, at the moment the agent is
/// working out what this repository is — so losing it silently is losing
/// the argument for the refactor.
#[test]
fn the_seed_commit_carries_the_arrangement_to_git_log() {
    let (_d, wt, shadow_dir) = fixture();
    let s = Shadow::new(&shadow_dir, "s01");
    s.ensure(&wt, &[]).unwrap();

    let said = git(&s.gitdir, &wt, &["log", "-1", "--format=%B"]).unwrap();
    assert!(
        said.contains(ARRANGEMENT),
        "the agent reads this on `git log` and nowhere else: {said}"
    );
}

/// The whole point: the agent's commits reach the branch with the messages
/// it wrote, and authored as the sandbox rather than as whoever the sandbox
/// claimed to be.
#[test]
fn a_harvest_lands_the_agents_commits_under_the_sandboxs_name() {
    let (d, wt, shadow_dir) = fixture();
    let checkout = d.path().join("checkout");
    let s = Shadow::new(&shadow_dir, "s01");
    s.ensure(&wt, &[]).unwrap();

    // the agent works, and claims to be someone it is not
    git(
        &s.gitdir,
        &wt,
        &["config", "user.name", "Nathanael Cherrier"],
    )
    .unwrap();
    git(
        &s.gitdir,
        &wt,
        &["config", "user.email", "nathanael@mindsers.it"],
    )
    .unwrap();
    std::fs::write(wt.join("f.txt"), "one\n").unwrap();
    git(&s.gitdir, &wt, &["commit", "-qam", "Fix the tap guard"]).unwrap();
    std::fs::write(wt.join("helper.rs"), "fn h() {}").unwrap();
    git(&s.gitdir, &wt, &["add", "-A", "."]).unwrap();
    git(&s.gitdir, &wt, &["commit", "-qm", "Extract helper"]).unwrap();
    std::fs::write(wt.join("tail.rs"), "fn t() {}").unwrap(); // never checkpointed

    let landed = s
        .harvest(&checkout, &wt, "omh/s01", &[], Keep::All)
        .unwrap()
        .landed;

    let log = git_in(&checkout, &["log", "--format=%an|%s", "main..omh/s01"]).unwrap();
    let lines: Vec<&str> = log.lines().collect();
    assert_eq!(landed, lines.len(), "reported count is what landed: {log}");
    assert!(
        lines.iter().any(|l| l.ends_with("|Extract helper")),
        "the agent's own messages are the point: {log}"
    );
    assert!(
        lines.iter().all(|l| l.starts_with(AUTHOR_NAME)),
        "the sandbox says who it is on the way in, whatever it claimed: {log}"
    );
    assert!(
        git_in(&checkout, &["show", "omh/s01:tail.rs"]).is_ok(),
        "work the agent never checkpointed is still its work"
    );
    assert!(
        git_in(
            &checkout,
            &[
                "rev-parse",
                "--verify",
                "-q",
                "refs/omh/s01-scratch/harvest"
            ]
        )
        .is_err(),
        "the fetched ref goes once the branch has the work, or the \
         pre-curation objects stay reachable in the user's repository"
    );
}

/// A carried secret must not reach the branch, and a path check alone
/// closes one of the ways it gets there. Measured before this existed: a
/// path strip caught `git add -f .env` and let through a copy under another
/// name and a value pasted into source.
///
/// Refused rather than stripped. Removing the path leaves an empty commit
/// with a misleading message and does nothing about the other shapes, and
/// rewriting the agent's work to hide a secret is the user's call.
#[test]
fn a_harvest_refuses_a_commit_holding_something_you_carried_in() {
    for (name, plant) in [
        ("force-added", "add-f"),
        ("copied under another name", "copy"),
        ("pasted into source", "inline"),
        ("written into a commit message", "message"),
    ] {
        let (d, wt, shadow_dir) = fixture();
        let checkout = d.path().join("checkout");
        let s = Shadow::new(&shadow_dir, "s01");
        s.ensure(&wt, &[".env".to_string()]).unwrap();
        // the checkout is where omh reads the bytes it carried
        std::fs::write(checkout.join(".env"), "API_TOKEN=ghp_abc123def456\n").unwrap();
        std::fs::write(wt.join(".env"), "API_TOKEN=ghp_abc123def456\n").unwrap();

        match plant {
            "add-f" => {
                git(&s.gitdir, &wt, &["add", "-f", ".env"]).unwrap();
            }
            "copy" => {
                std::fs::copy(wt.join(".env"), wt.join("config.bak")).unwrap();
                git(&s.gitdir, &wt, &["add", "-A", "."]).unwrap();
            }
            "inline" => {
                std::fs::write(wt.join("k.rs"), "const K = \"API_TOKEN=ghp_abc123def456\";")
                    .unwrap();
                git(&s.gitdir, &wt, &["add", "-A", "."]).unwrap();
            }
            // The door `-S` cannot see: a pickaxe reads diff content and
            // never the message. Measured — it finds nothing here.
            _ => {
                std::fs::write(wt.join("note.rs"), "fn n() {}").unwrap();
                git(&s.gitdir, &wt, &["add", "-A", "."]).unwrap();
            }
        }
        let subject = if plant == "message" {
            "note: API_TOKEN=ghp_abc123def456"
        } else {
            "Save config"
        };
        git(&s.gitdir, &wt, &["commit", "-qm", subject]).unwrap();

        let err = s
            .harvest(&checkout, &wt, "omh/s01", &[".env".to_string()], Keep::All)
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("carried"),
            "{name}: a harvest must refuse it, not carry it: {err}"
        );
        assert!(
            git_in(&checkout, &["log", "--oneline", "main..omh/s01"])
                .unwrap()
                .trim()
                .is_empty(),
            "{name}: and the branch must be untouched"
        );
    }
}

/// The **value** of a carried secret is refused on its own, not only the
/// line it came from.
///
/// `a_harvest_refuses_a_commit_holding_something_you_carried_in` plants
/// the whole line — `API_TOKEN=ghp_…` — and passed while the scan only
/// knew whole lines. Nobody pastes the whole line: they paste the token.
/// Reproduced 2026-09-04 with `const K = "ghp_abc123def456";` on a
/// carried `.env`, landed by `--keep` without a word. Three spellings a
/// carried file is written in, so the value is found after `=`, after
/// `export KEY=`, and after `key:`.
#[test]
fn a_harvest_refuses_a_commit_holding_only_the_value_of_a_carried_secret() {
    for (spelling, line) in [
        ("dotenv", "API_TOKEN=ghp_abc123def456"),
        (
            "shell export, quoted",
            "export API_TOKEN=\"ghp_abc123def456\"",
        ),
        ("yaml", "api_token: ghp_abc123def456"),
        (
            "json, trailing comma",
            "\"api_token\": \"ghp_abc123def456\",",
        ),
    ] {
        let (d, wt, shadow_dir) = fixture();
        let checkout = d.path().join("checkout");
        let s = Shadow::new(&shadow_dir, "s01");
        s.ensure(&wt, &[".env".to_string()]).unwrap();
        std::fs::write(checkout.join(".env"), format!("{line}\n")).unwrap();
        std::fs::write(wt.join(".env"), format!("{line}\n")).unwrap();

        // Only the value, in a shape the line never had.
        std::fs::write(wt.join("k.rs"), "const K: &str = \"ghp_abc123def456\";\n").unwrap();
        git(&s.gitdir, &wt, &["add", "-A", "."]).unwrap();
        git(&s.gitdir, &wt, &["commit", "-qm", "Save config"]).unwrap();

        let err = s
            .harvest(&checkout, &wt, "omh/s01", &[".env".to_string()], Keep::All)
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("carried"),
            "{spelling}: the value alone is the secret, and it must be refused: {err}"
        );
        assert!(
            git_in(&checkout, &["log", "--oneline", "main..omh/s01"])
                .unwrap()
                .trim()
                .is_empty(),
            "{spelling}: and the branch must be untouched"
        );
    }
}

/// What the value rule must **not** do: read every carried setting as a
/// secret. `DEBUG=true` and `PORT=8080` are carried in `.env` files
/// everywhere, and `true` and `8080` are in every codebase; a scan that
/// refused on them would refuse every harvest, and a refusal that fires
/// always is one people learn to route around.
///
/// Green before the value rule and green after — it is the constraint the
/// rule is designed under, kept so that widening it later has to argue
/// with this.
#[test]
fn an_ordinary_configuration_value_is_not_treated_as_a_secret() {
    let (d, wt, shadow_dir) = fixture();
    let checkout = d.path().join("checkout");
    let s = Shadow::new(&shadow_dir, "s01");
    s.ensure(&wt, &[".env".to_string()]).unwrap();
    let env = "DEBUG=true\nPORT=8080\nNODE_ENV=development\nHOST=localhost\nRETRIES=3\n";
    std::fs::write(checkout.join(".env"), env).unwrap();
    std::fs::write(wt.join(".env"), env).unwrap();

    std::fs::write(
        wt.join("server.rs"),
        "const DEBUG: bool = true;\nconst PORT: u16 = 8080;\nconst ENV: &str = \"development\";\nconst HOST: &str = \"localhost\";\n",
    )
    .unwrap();
    git(&s.gitdir, &wt, &["add", "-A", "."]).unwrap();
    git(
        &s.gitdir,
        &wt,
        &["commit", "-qm", "wire the server on 8080"],
    )
    .unwrap();

    s.harvest(&checkout, &wt, "omh/s01", &[".env".to_string()], Keep::All)
        .expect("ordinary configuration values are not secrets");
}

/// The value rule, as a table: where a value is worth searching for and
/// where it is not.
#[test]
fn a_value_needle_is_derived_only_where_a_value_is_worth_searching_for() {
    let value = |line: &str| -> Vec<String> {
        Shadow::needles_of_line(line)
            .into_iter()
            .filter(|n| n != line.trim())
            .collect()
    };
    // Found.
    assert_eq!(
        value("API_TOKEN=ghp_abc123def456"),
        vec!["ghp_abc123def456"]
    );
    assert_eq!(
        value("export TOKEN='ghp_abc123def456'"),
        vec!["ghp_abc123def456"]
    );
    assert_eq!(value("token: ghp_abc123def456"), vec!["ghp_abc123def456"]);
    assert_eq!(
        value("\"token\": \"ghp_abc123def456\","),
        vec!["ghp_abc123def456"]
    );
    assert_eq!(
        value("DATABASE_URL=postgres://u:p@h/db"),
        vec!["postgres://u:p@h/db"],
        "a URL with credentials is a secret whatever the key says"
    );
    // Not found, each for a reason.
    for (line, why) in [
        ("DEBUG=true", "a boolean"),
        ("PORT=8080", "a number"),
        ("NODE_ENV=development", "a word too common to search for"),
        ("HOST=localhost", "on the skip list"),
        ("TOKEN=short", "under twelve characters"),
        (
            "TOKEN=has a space in it",
            "a value with whitespace is a sentence",
        ),
        ("# API_TOKEN=ghp_abc123def456", "a comment"),
        ("just a line with no separator at all", "no key"),
        ("=ghp_abc123def456", "no key before the separator"),
    ] {
        assert!(
            value(line).is_empty(),
            "`{line}` yields no value needle: {why}"
        );
    }
    // The whole line is still a needle where it was before.
    assert!(Shadow::needles_of_line("API_TOKEN=ghp_abc123def456")
        .contains(&"API_TOKEN=ghp_abc123def456".to_string()));
}

/// A secret is matched as the bytes it is, not as a pattern.
///
/// The needle carries a `*` deliberately, and the choice is the whole
/// worth of the test. `--grep` reads a pattern in whatever language
/// `grep.patternType` names, and `*` is a quantifier in all three of them,
/// so this is red on any machine. A `+` would not be: it is literal under
/// `basic`, which is the default, and only bites the people who set
/// `extended` or `perl` — this test was written with one and passed for
/// the author's dotfiles rather than for git.
///
/// Planted in the **message only**, because that is the one door `-S`
/// cannot see: the pickaxe is already a fixed string and would have caught
/// it in a diff, and the test would then have proved nothing about the
/// path it exists for.
#[test]
fn a_secret_that_looks_like_a_pattern_is_still_caught() {
    let (d, wt, shadow_dir) = fixture();
    let checkout = d.path().join("checkout");
    let s = Shadow::new(&shadow_dir, "s01");
    s.ensure(&wt, &[".env".to_string()]).unwrap();

    // `*` and `/` are ordinary base64. As a pattern this matches `KEY=acd…`
    // and `KEY=abbcd…` — never the literal bytes the agent wrote.
    let secret = "KEY=ab*cd/ef12345==";
    std::fs::write(checkout.join(".env"), format!("{secret}\n")).unwrap();
    std::fs::write(wt.join(".env"), format!("{secret}\n")).unwrap();

    std::fs::write(wt.join("note.rs"), "fn n() {}").unwrap();
    git(&s.gitdir, &wt, &["add", "-A", "."]).unwrap();
    git(
        &s.gitdir,
        &wt,
        &["commit", "-qm", &format!("ship with {secret}")],
    )
    .unwrap();

    let err = s
        .harvest(&checkout, &wt, "omh/s01", &[".env".to_string()], Keep::All)
        .unwrap_err()
        .to_string();
    assert!(
        err.contains("carried"),
        "a secret that is its own regex still has to be refused: {err}"
    );
    assert!(
        git_in(&checkout, &["log", "--oneline", "main..omh/s01"])
            .unwrap()
            .trim()
            .is_empty(),
        "and the branch must be untouched"
    );
}

/// A carried line that is not valid regex must not break the harvest.
///
/// The same defect from the other side, and this one takes the whole
/// feature down rather than letting something through: an unbalanced `[` is
/// a syntax error to `--grep`, so `git log` exits 128, the harvest fails,
/// and `--keep` stays dead for that session until the file changes.
///
/// Measured under every `grep.patternType` — `fatal: command line,
/// 'SECRET=a[bc': brackets ([ ]) not balanced` under `basic` and
/// `extended`, `missing terminating ] for character class` under `perl`.
/// The wording moves with the setting; the exit code does not.
#[test]
fn a_carried_line_that_is_not_a_regex_does_not_break_the_harvest() {
    let (d, wt, shadow_dir) = fixture();
    let checkout = d.path().join("checkout");
    let s = Shadow::new(&shadow_dir, "s01");
    s.ensure(&wt, &[".env".to_string()]).unwrap();

    let carried = "SECRET_TOKEN=a[bcdefgh";
    std::fs::write(checkout.join(".env"), format!("{carried}\n")).unwrap();
    std::fs::write(wt.join(".env"), format!("{carried}\n")).unwrap();

    std::fs::write(wt.join("work.rs"), "fn work() {}").unwrap();
    git(&s.gitdir, &wt, &["add", "-A", "."]).unwrap();
    git(&s.gitdir, &wt, &["commit", "-qm", "Ordinary work"]).unwrap();

    let landed = s
        .harvest(&checkout, &wt, "omh/s01", &[".env".to_string()], Keep::All)
        .expect("a carried line nobody committed is not a reason to refuse")
        .landed;
    assert_eq!(landed, 1, "the agent's commit still has to land");
}

/// A carried file the scan could not read is **named**, not passed over.
///
/// This is risk 4d, and the point is narrow: none of these three is a
/// refusal, and none should be — omh will not block a harvest because it
/// could not read a file. What was wrong is that they failed open and
/// *silent*, so `omh s commit --keep` reported a clean landing in exactly
/// the same words whether it had scanned the carried files or not.
///
/// The `NotText` row is the one that matters most in practice. `carry_in`'s
/// own documentation gives `certs/` as its second example, so a carried
/// keystore, `.p12` or DER key is the common case rather than the exotic
/// one — and its only remaining protection is its path, which a copy under
/// another name walks straight around.
///
/// Asserted by cause and by path rather than against the sentence, so
/// rewording the warning does not fail this.
#[test]
fn a_carried_file_the_scan_could_not_read_is_named() {
    for (name, plant, want) in [
        (
            "a keystore, or anything else that is not UTF-8",
            "binary",
            Unreadable::NotText,
        ),
        (
            "a file deleted from the checkout mid-session",
            "delete",
            Unreadable::Missing,
        ),
        (
            "a file of nothing but short lines and comments",
            "unmatchable",
            Unreadable::NothingToMatch,
        ),
    ] {
        let (d, wt, shadow_dir) = fixture();
        let checkout = d.path().join("checkout");
        let s = Shadow::new(&shadow_dir, "s01");
        s.ensure(&wt, &["secret.bin".to_string()]).unwrap();

        match plant {
            // Invalid UTF-8 rather than merely unusual bytes: a lone 0x80
            // is a continuation byte with nothing to continue, which is
            // what `read_to_string` rejects. Random high bytes that happen
            // to decode would make this test pass for the wrong reason.
            "binary" => {
                std::fs::write(checkout.join("secret.bin"), [0x80, 0x81, 0xfe, 0xff]).unwrap();
            }
            "delete" => { /* never written: the scan finds nothing there */ }
            _ => {
                std::fs::write(checkout.join("secret.bin"), "k=1\n# a note\n//x\n").unwrap();
            }
        }

        // Something for the harvest to actually do. With nothing to hand
        // over, `harvest` returns before the carried scan runs at all and
        // this would assert against a path it never took.
        std::fs::write(wt.join("work.rs"), "fn work() {}").unwrap();
        git(&s.gitdir, &wt, &["add", "-A", "."]).unwrap();
        git(&s.gitdir, &wt, &["commit", "-qm", "Ordinary work"]).unwrap();

        let got = s
            .harvest(
                &checkout,
                &wt,
                "omh/s01",
                &["secret.bin".to_string()],
                Keep::All,
            )
            .expect("an unreadable carried file is not a reason to refuse a harvest");

        assert_eq!(got.landed, 1, "{name}: the work still lands");
        assert_eq!(
            got.unscanned,
            vec![Unscanned {
                path: "secret.bin".into(),
                why: want,
            }],
            "{name}: the harvest has to say it could not read this"
        );
    }
}

/// A carried file omh may not read is named, and the harvest still lands.
///
/// `needles` documented itself as failing open — "refusing a harvest
/// because a file could not be read would be worse" — and then caught
/// only `InvalidData`. Every other `io::Error` propagated, so the one
/// shape that most literally *is* "a file omh could not read" was the one
/// shape that failed **closed**: `omh s commit --keep` aborted and the
/// agent's commits did not land, over a `chmod 600` that is the ordinary
/// state of a carried secret.
#[test]
#[cfg(unix)]
fn a_carried_file_omh_may_not_read_is_named_rather_than_fatal() {
    use std::os::unix::fs::PermissionsExt;
    let (d, wt, shadow_dir) = fixture();
    let checkout = d.path().join("checkout");
    let s = Shadow::new(&shadow_dir, "s01");
    s.ensure(&wt, &[".env".to_string()]).unwrap();

    let secret = checkout.join(".env");
    std::fs::write(&secret, "API_TOKEN=ghp_abc123def456\n").unwrap();
    std::fs::set_permissions(&secret, std::fs::Permissions::from_mode(0o000)).unwrap();

    std::fs::write(wt.join("work.rs"), "fn work() {}").unwrap();
    git(&s.gitdir, &wt, &["add", "-A", "."]).unwrap();
    git(&s.gitdir, &wt, &["commit", "-qm", "Ordinary work"]).unwrap();

    let got = s.harvest(&checkout, &wt, "omh/s01", &[".env".to_string()], Keep::All);
    std::fs::set_permissions(&secret, std::fs::Permissions::from_mode(0o600)).unwrap();

    let got = got.expect("a file omh may not read is not a reason to lose the work");
    assert_eq!(got.landed, 1, "the agent's commit still lands");
    assert_eq!(
        got.unscanned,
        vec![Unscanned {
            path: ".env".into(),
            why: Unreadable::CouldNotRead,
        }],
        "and omh says it could not look, rather than saying nothing"
    );
}

/// A carried directory omh may not read is named, not fatal either.
///
/// The same gap one level up: `read_dir` was `?`-ed, so an unreadable
/// `certs/` took the launch and the harvest down. `carry_in`'s own
/// documentation gives `certs/` as its second example.
#[test]
#[cfg(unix)]
fn a_carried_directory_omh_may_not_read_is_named_rather_than_fatal() {
    use std::os::unix::fs::PermissionsExt;
    let (d, wt, shadow_dir) = fixture();
    let checkout = d.path().join("checkout");
    let s = Shadow::new(&shadow_dir, "s01");
    s.ensure(&wt, &["certs".to_string()]).unwrap();

    let certs = checkout.join("certs");
    std::fs::create_dir_all(&certs).unwrap();
    std::fs::write(certs.join("deploy.key"), "PRIVATE KEY MATERIAL HERE\n").unwrap();
    std::fs::set_permissions(&certs, std::fs::Permissions::from_mode(0o000)).unwrap();

    std::fs::write(wt.join("work.rs"), "fn work() {}").unwrap();
    git(&s.gitdir, &wt, &["add", "-A", "."]).unwrap();
    git(&s.gitdir, &wt, &["commit", "-qm", "Ordinary work"]).unwrap();

    let got = s.harvest(&checkout, &wt, "omh/s01", &["certs".to_string()], Keep::All);
    std::fs::set_permissions(&certs, std::fs::Permissions::from_mode(0o700)).unwrap();

    let got = got.expect("a directory omh may not read is not a reason to lose the work");
    assert_eq!(
        got.unscanned,
        vec![Unscanned {
            path: "certs".into(),
            why: Unreadable::CouldNotRead,
        }],
        "the directory is named, and the harvest stands"
    );
}

/// A directory carried in reports the file inside it, not the entry.
///
/// `carry_in`'s documented example is `certs/`, and a warning naming
/// `certs/` tells the user nothing about which of eight files it could not
/// read. `needles` already walks the directory; the report has to arrive at
/// the same resolution.
#[test]
fn a_carried_directory_names_the_file_it_could_not_read() {
    let (d, wt, shadow_dir) = fixture();
    let checkout = d.path().join("checkout");
    let s = Shadow::new(&shadow_dir, "s01");
    s.ensure(&wt, &["certs".to_string()]).unwrap();

    std::fs::create_dir_all(checkout.join("certs")).unwrap();
    std::fs::write(checkout.join("certs/deploy.p12"), [0x80, 0xfe]).unwrap();
    std::fs::write(
        checkout.join("certs/notes.txt"),
        "this line is long enough to be a needle\n",
    )
    .unwrap();

    std::fs::write(wt.join("work.rs"), "fn work() {}").unwrap();
    git(&s.gitdir, &wt, &["add", "-A", "."]).unwrap();
    git(&s.gitdir, &wt, &["commit", "-qm", "Ordinary work"]).unwrap();

    let got = s
        .harvest(&checkout, &wt, "omh/s01", &["certs".to_string()], Keep::All)
        .expect("an unreadable carried file is not a reason to refuse a harvest");

    assert_eq!(
        got.unscanned,
        vec![Unscanned {
            path: "certs/deploy.p12".into(),
            why: Unreadable::NotText,
        }],
        "the readable sibling is not a gap and the unreadable one is named in full"
    );
}

/// The launch-time answer is the harvest-time answer.
///
/// Two ways to ask "can this be scanned" would drift, and the drift would
/// be invisible: the launch would promise a scan the harvest then did not
/// perform, or warn about a file it read perfectly well. So this asserts
/// they agree rather than asserting `unscannable`'s output on its own —
/// the property is the agreement, and a test of either half alone would
/// survive the two coming apart.
#[test]
fn what_launch_says_is_unscannable_is_what_the_harvest_could_not_read() {
    let (d, wt, shadow_dir) = fixture();
    let checkout = d.path().join("checkout");
    let s = Shadow::new(&shadow_dir, "s01");
    let carried = vec!["certs".to_string(), "gone.key".to_string()];
    s.ensure(&wt, &carried).unwrap();

    std::fs::create_dir_all(checkout.join("certs")).unwrap();
    std::fs::write(checkout.join("certs/deploy.p12"), [0x80, 0xfe]).unwrap();
    std::fs::write(checkout.join("certs/short.env"), "k=1\n").unwrap();
    // `gone.key` is never written: named in carry_in, not on disk.

    let at_launch = unscannable(&checkout, &carried).unwrap();

    std::fs::write(wt.join("work.rs"), "fn work() {}").unwrap();
    git(&s.gitdir, &wt, &["add", "-A", "."]).unwrap();
    git(&s.gitdir, &wt, &["commit", "-qm", "Ordinary work"]).unwrap();
    let at_harvest = s
        .harvest(&checkout, &wt, "omh/s01", &carried, Keep::All)
        .unwrap()
        .unscanned;

    assert_eq!(
        at_launch, at_harvest,
        "the two moments must give the same answer about the same files"
    );
    assert_eq!(
        at_launch,
        vec![
            Unscanned {
                path: "certs/deploy.p12".into(),
                why: Unreadable::NotText
            },
            Unscanned {
                path: "certs/short.env".into(),
                why: Unreadable::NothingToMatch
            },
            Unscanned {
                path: "gone.key".into(),
                why: Unreadable::Missing
            },
        ],
        "each cause, named, in a stable order"
    );
}

/// An empty carried file is not a gap.
///
/// It yields no needles, like the three causes above, and reporting it
/// would be a false positive: there is no content in it to have been
/// copied anywhere, so nothing went unchecked. The distinction is worth a
/// test because it is one character of implementation — the
/// `!body.trim().is_empty()` in `needles` — and losing it would put a line
/// in the warning for every empty `.env` placeholder anyone carries, which
/// is how a warning stops being read.
#[test]
fn an_empty_carried_file_is_not_reported_as_unscanned() {
    let (d, _wt, _shadow_dir) = fixture();
    let checkout = d.path().join("checkout");
    std::fs::write(checkout.join("empty.env"), "").unwrap();
    std::fs::write(checkout.join("blank.env"), "\n\n   \n").unwrap();

    assert_eq!(
        unscannable(
            &checkout,
            &["empty.env".to_string(), "blank.env".to_string()]
        )
        .unwrap(),
        vec![],
        "nothing in it means nothing went unchecked"
    );
}

/// The warning says which file and why, and does not overstate the gap.
///
/// Separate from the guards above because those assert the *data* and this
/// asserts the *sentence* — the one a user acts on. Both matter and they
/// break independently: a correct `Vec<Unscanned>` rendered as
/// "1 file skipped" would pass every assertion above.
#[test]
fn the_unscanned_warning_names_the_file_and_keeps_the_path_check_honest() {
    assert_eq!(
        unscanned_warning(&[]),
        None,
        "nothing to report is reported by saying nothing"
    );

    let msg = unscanned_warning(&[Unscanned {
        path: "certs/deploy.p12".into(),
        why: Unreadable::NotText,
    }])
    .expect("something to report");

    assert!(msg.contains("certs/deploy.p12"), "names the file: {msg}");
    assert!(msg.contains("not text"), "says why: {msg}");
    // The path check survives all three causes, and a warning that let the
    // reader believe the file was wholly unguarded would send them looking
    // for a problem they do not have.
    assert!(
        msg.contains("path itself is still checked"),
        "says what is still guarded: {msg}"
    );
}

/// Landing the same work twice is not landing it twice.
///
/// The harvest replayed from the seed every time, and nothing recorded what
/// had already been kept — so a second `--keep` offered commits that were
/// on the branch already. Whether that duplicated them or died applying
/// them depended on whether the patches still fitted; measured against git
/// 2.55.0, an edit to a line a later commit also touched conflicts, and
/// `--keep` reported nothing but `Could not apply`.
///
/// Landing twice must be a no-op, and the branch must not move.
#[test]
fn harvesting_twice_keeps_nothing_the_second_time() {
    let (d, wt, shadow_dir) = fixture();
    let checkout = d.path().join("checkout");
    let s = Shadow::new(&shadow_dir, "s01");
    s.ensure(&wt, &[]).unwrap();

    std::fs::write(wt.join("f.txt"), "base\nadded\n").unwrap();
    git(&s.gitdir, &wt, &["commit", "-qam", "Add a line"]).unwrap();
    std::fs::write(wt.join("f.txt"), "base\nadded\nmore\n").unwrap();
    git(&s.gitdir, &wt, &["commit", "-qam", "Add another"]).unwrap();

    assert_eq!(
        s.harvest(&checkout, &wt, "omh/s01", &[], Keep::All)
            .unwrap()
            .landed,
        2
    );
    let tip = git_in(&checkout, &["rev-parse", "omh/s01"]).unwrap();

    assert_eq!(
        s.harvest(&checkout, &wt, "omh/s01", &[], Keep::All)
            .unwrap()
            .landed,
        0,
        "there is nothing new to keep, and saying so is the whole job"
    );
    assert_eq!(
        git_in(&checkout, &["rev-parse", "omh/s01"]).unwrap(),
        tip,
        "and a branch with nothing to add must not move"
    );
}

/// The second harvest takes what the first one did not.
///
/// The loop the replay point exists for: work, keep, work again, keep
/// again. Only the new commits land, in order, once each.
///
/// **This is the shape of the loop, not the guard for it**, and that is
/// measured rather than assumed: with the replay point neutralised it stays
/// green. Replaying a round that is already on the branch gives git a patch
/// whose id is upstream, and it drops it — so the incremental path repairs
/// itself and says nothing.
///
/// Editing the same line each round instead of appending was tried, on the
/// theory that a stale context would make the replay conflict. It does not:
/// the patch-id match happens first, and the test stays green that way too.
/// The guard is `harvesting_twice_keeps_nothing_the_second_time`, where
/// nothing is new and the replayed patch has nowhere to go.
///
/// Recorded so the next person does not spend the same hour proving it.
#[test]
fn a_second_harvest_takes_only_what_is_new() {
    let (d, wt, shadow_dir) = fixture();
    let checkout = d.path().join("checkout");
    let s = Shadow::new(&shadow_dir, "s01");
    s.ensure(&wt, &[]).unwrap();

    std::fs::write(wt.join("f.txt"), "base\nfirst\n").unwrap();
    git(&s.gitdir, &wt, &["commit", "-qam", "The first round"]).unwrap();
    assert_eq!(
        s.harvest(&checkout, &wt, "omh/s01", &[], Keep::All)
            .unwrap()
            .landed,
        1
    );

    std::fs::write(wt.join("f.txt"), "base\nfirst\nsecond\n").unwrap();
    git(&s.gitdir, &wt, &["commit", "-qam", "The second round"]).unwrap();

    assert_eq!(
        s.harvest(&checkout, &wt, "omh/s01", &[], Keep::All)
            .unwrap()
            .landed,
        1,
        "only the round that has not landed yet"
    );
    let log = git_in(&checkout, &["log", "--format=%s", "main..omh/s01"]).unwrap();
    let subjects: Vec<&str> = log.lines().collect();
    assert_eq!(
        subjects,
        vec!["The second round", "The first round"],
        "each round lands once, in order: {log}"
    );
}

/// The refusal quotes the agent's subject line, so it may not carry escapes.
///
/// `refuse_carried` names the commit it found — sha and subject, straight
/// from `git log --oneline`. Measured: git quotes a *path* by default
/// (`core.quotePath` renders an escape as a literal `\033`), and does not
/// quote a **subject** at all, which arrives with its bytes intact.
///
/// This is the message that says omh refused to publish a secret. A subject
/// that can clear the line and print something else is a forged answer to
/// the one question this whole guard exists to answer.
#[test]
fn a_refusal_cannot_be_repainted_by_the_subject_it_quotes() {
    let (d, wt, shadow_dir) = fixture();
    let checkout = d.path().join("checkout");
    let s = Shadow::new(&shadow_dir, "s01");
    s.ensure(&wt, &[".env".to_string()]).unwrap();

    let secret = "API_TOKEN=ghp_abc123def456";
    std::fs::write(checkout.join(".env"), format!("{secret}\n")).unwrap();
    std::fs::write(wt.join(".env"), format!("{secret}\n")).unwrap();
    std::fs::write(wt.join("note.rs"), "fn n() {}").unwrap();
    git(&s.gitdir, &wt, &["add", "-A", "."]).unwrap();
    // the secret is in the subject, which is the door `-S` cannot see — and
    // the subject also clears the line and writes its own answer
    git(
        &s.gitdir,
        &wt,
        &[
            "commit",
            "-qm",
            &format!(
                "note: {secret}{}[2K\rcommitted to main, nothing carried",
                '\u{1b}'
            ),
        ],
    )
    .unwrap();

    let err = s
        .harvest(&checkout, &wt, "omh/s01", &[".env".to_string()], Keep::All)
        .unwrap_err()
        .to_string();
    assert!(err.contains("carried"), "it still has to refuse: {err}");
    assert!(
        !err.contains('\u{1b}'),
        "and it may not hand the terminal to the subject it quotes: {err:?}"
    );
}

/// A converter the sandbox named does not run on the host.
///
/// Both halves of a textconv driver are the agent's to write: the driver
/// itself is a config key in a gitdir omh mounts read-write, and the
/// `.gitattributes` naming it sits in `/work`. So a host-side read of the
/// sandbox that produces a diff runs whatever the agent chose, as the user,
/// outside the container.
///
/// Measured against git 2.55.0, and not what the plan for this assumed:
///
/// | read | textconv | external diff |
/// |---|---|---|
/// | `log -S` | **runs** | no |
/// | `log -p`, `show` | **runs** | no |
/// | `git diff` | **runs** | **runs** |
/// | `log --grep` | no | no |
///
/// So the live half is textconv, not `diff.external` — the log family will
/// not run an external diff without `--ext-diff`, which nothing here
/// passes. `--no-textconv` is what closes it, and it is a per-command flag
/// rather than a config key, which is why the helpers add it by verb.
#[test]
fn a_converter_the_sandbox_named_does_not_run_on_the_host() {
    let (d, wt, shadow_dir) = fixture();
    let s = Shadow::new(&shadow_dir, "s01");
    s.ensure(&wt, &[]).unwrap();

    let marker = d.path().join("THE-CONVERTER-RAN");
    let conv = d.path().join("conv.sh");
    std::fs::write(
        &conv,
        format!("#!/bin/sh\ntouch {}\ncat \"$1\"\n", marker.display()),
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&conv, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    // both halves are the agent's: the driver in its own config, the
    // attributes in its own worktree
    git(
        &s.gitdir,
        &wt,
        &["config", "diff.pwn.textconv", conv.to_str().unwrap()],
    )
    .unwrap();
    std::fs::write(wt.join(".gitattributes"), "* diff=pwn\n").unwrap();
    std::fs::write(wt.join("f.txt"), "SECRET=abc123def456\n").unwrap();
    git(&s.gitdir, &wt, &["add", "-A", "."]).unwrap();
    git(&s.gitdir, &wt, &["commit", "-qm", "a checkpoint"]).unwrap();

    // omh reads the sandbox, on the host, the way a `log` or `show` would
    let _ = git(
        &s.gitdir,
        &wt,
        &["log", "--oneline", "-S", "SECRET=abc123def456", "HEAD"],
    );

    assert!(
        !marker.exists(),
        "a driver the sandbox named ran on the host, as the user"
    );
}

/// A key with two values is one key, and dropping it is one job.
///
/// `config --list --name-only` prints a key once **per value**, so a
/// multi-valued key arrives twice — and `--unset-all` removes every value
/// on the first call, leaving the second to exit 5 with empty stderr. Read
/// as a failure that aborts the launch, which is what it did: a shadow from
/// before the config was mounted read-only, where an agent had ever run
/// `git config --add` twice, could not be relaunched at all. The error
/// named a key and said nothing else.
///
/// The module's own rule, from the fetch a few hundred lines up: following
/// omh's instructions must not brick omh. Neither may upgrading omh.
#[test]
fn a_key_with_two_values_does_not_brick_the_next_launch() {
    let (_d, wt, shadow_dir) = fixture();
    let s = Shadow::new(&shadow_dir, "s01");
    s.ensure(&wt, &[]).unwrap();

    // what a shadow from before the read-only mount can hold
    git(&s.gitdir, &wt, &["config", "--add", "core.gitproxy", "one"]).unwrap();
    git(&s.gitdir, &wt, &["config", "--add", "core.gitproxy", "two"]).unwrap();
    assert_eq!(
        git(
            &s.gitdir,
            &wt,
            &["config", "--list", "--local", "--name-only"]
        )
        .unwrap()
        .lines()
        .filter(|k| k.trim() == "core.gitproxy")
        .count(),
        2,
        "the precondition is that git lists the key once per value"
    );

    s.ensure(&wt, &[])
        .expect("a key with two values is still just a key to drop");

    let config = std::fs::read_to_string(s.gitdir.join("config")).unwrap();
    assert!(!config.contains("gitproxy"), "and it is gone:\n{config}");
}

/// Refreshing keeps what git records about the repository itself.
///
/// The allowlist cannot be a list of keys someone remembered: `git init`
/// writes what it detected about *this* filesystem, and that differs by
/// platform — `core.ignorecase` and `core.precomposeunicode` on macOS,
/// neither on a case-sensitive Linux box, `extensions.objectformat` in a
/// sha256 repository. Drop one and the repository git reads is not the one
/// git made: paths that differ only in case become two files, accented
/// filenames flip between NFC and NFD and read as modified.
///
/// So the guard is measured against a fresh `git init` on the machine
/// running it, rather than against a list in this file. A git that starts
/// recording something new turns this red on the platform where it matters.
#[test]
fn refreshing_keeps_what_git_records_about_the_repository() {
    let (d, wt, shadow_dir) = fixture();
    let pristine = d.path().join("pristine.git");
    Command::new("git")
        .args(["init", "-q", "--bare", "--template="])
        .arg(&pristine)
        .output()
        .unwrap();
    let listed = Command::new("git")
        .arg("--git-dir")
        .arg(&pristine)
        .args(["config", "--list", "--local", "--name-only"])
        .output()
        .unwrap();
    let expected: Vec<String> = String::from_utf8_lossy(&listed.stdout)
        .lines()
        .map(str::trim)
        .filter(|k| !k.is_empty() && *k != "core.bare")
        .map(str::to_string)
        .collect();
    assert!(
        !expected.is_empty(),
        "a fresh `git init` records something, or this test is vacuous"
    );

    let s = Shadow::new(&shadow_dir, "s01");
    s.ensure(&wt, &[]).unwrap();
    s.ensure(&wt, &[]).unwrap();

    let kept = git(
        &s.gitdir,
        &wt,
        &["config", "--list", "--local", "--name-only"],
    )
    .unwrap();
    for key in expected {
        assert!(
            kept.lines().any(|k| k.trim() == key),
            "{key} is something git recorded about this repository and omh \
             dropped it. kept:\n{kept}"
        );
    }
}

/// The sandbox's config is omh's again at every launch.
///
/// The gitdir is mounted read-write because the agent commits through it,
/// so every key in it is the agent's to set — and several of them turn
/// `git` into *run whatever this repository says*. `NEUTRALISED` answers
/// that for the calls omh makes, but only for the ones that remember to,
/// and only host-side: inside the container the agent reads its own config
/// with nothing filtering it.
///
/// So the file becomes omh's own view again on every launch, and what the
/// agent set in between does not survive. Not because a relaunch is a
/// boundary — it is not, the agent can set it again a second later — but
/// because a key that persists is one omh will still be reading a week
/// later, long after whatever wrote it.
#[test]
fn the_sandboxs_config_is_omhs_again_at_every_launch() {
    let (_d, wt, shadow_dir) = fixture();
    let s = Shadow::new(&shadow_dir, "s01");
    s.ensure(&wt, &[]).unwrap();

    // the agent makes its repository into one that runs things
    for (key, value) in [
        ("diff.pwn.textconv", "/bin/sh"),
        ("core.sshCommand", "/bin/sh"),
        ("core.hooksPath", "/tmp/mine"),
    ] {
        git(&s.gitdir, &wt, &["config", key, value]).unwrap();
    }
    std::fs::write(wt.join("agent.rs"), "fn main() {}").unwrap();
    git(&s.gitdir, &wt, &["add", "-A", "."]).unwrap();
    git(&s.gitdir, &wt, &["commit", "-qm", "a checkpoint"]).unwrap();
    let checkpoint = git(&s.gitdir, &wt, &["rev-parse", "HEAD"]).unwrap();

    s.ensure(&wt, &[]).unwrap();

    let config = std::fs::read_to_string(s.gitdir.join("config")).unwrap();
    for key in ["textconv", "sshCommand", "hooksPath"] {
        assert!(
            !config.contains(key),
            "{key} survived a relaunch:\n{config}"
        );
    }
    assert_eq!(
        git(&s.gitdir, &wt, &["config", "user.email"])
            .unwrap()
            .trim(),
        AUTHOR_EMAIL,
        "and the identity omh needs to commit at all is still there"
    );
    assert_eq!(
        git(&s.gitdir, &wt, &["rev-parse", "HEAD"]).unwrap(),
        checkpoint,
        "and the agent's work is untouched"
    );
}

/// A record omh cannot read is not a session that never landed.
///
/// The two failures this covers arrive by different routes and meet in the
/// same place. **Unreadable** is a permissions or I/O fault on a record that
/// exists. **Empty** is what an interrupted write leaves: `fs::write`
/// truncates before it writes, so a process killed in that window leaves
/// zero bytes where a commit id was — the very window this file documents a
/// few dozen lines above, in the code that does the writing.
///
/// Either one read as *never harvested* replays from the seed and skips the
/// ancestry check with it, offering the branch everything it already has.
/// That is the defect the record exists to close, so neither may spell the
/// same as absent.
#[test]
fn a_record_omh_cannot_read_is_not_a_session_that_never_landed() {
    for (name, break_it) in [
        ("empty", 0usize),
        #[cfg(unix)]
        ("unreadable", 1usize),
    ] {
        let (d, wt, shadow_dir) = fixture();
        let checkout = d.path().join("checkout");
        let s = Shadow::new(&shadow_dir, "s01");
        s.ensure(&wt, &[]).unwrap();
        std::fs::write(wt.join("f.txt"), "base\nfirst\n").unwrap();
        git(&s.gitdir, &wt, &["commit", "-qam", "The first round"]).unwrap();
        s.harvest(&checkout, &wt, "omh/s01", &[], Keep::All)
            .unwrap();
        let tip = git_in(&checkout, &["rev-parse", "omh/s01"]).unwrap();

        match break_it {
            0 => std::fs::write(&s.landed_record, "").unwrap(),
            _ => {
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    std::fs::set_permissions(
                        &s.landed_record,
                        std::fs::Permissions::from_mode(0o000),
                    )
                    .unwrap();
                }
            }
        }

        std::fs::write(wt.join("f.txt"), "base\nfirst\nsecond\n").unwrap();
        git(&s.gitdir, &wt, &["commit", "-qam", "The second round"]).unwrap();

        let outcome = s.harvest(&checkout, &wt, "omh/s01", &[], Keep::All);
        assert!(
            outcome.is_err(),
            "{name}: a record omh cannot read must not read as a session that \
             never landed"
        );
        assert_eq!(
            git_in(&checkout, &["rev-parse", "omh/s01"]).unwrap(),
            tip,
            "{name}: and the branch must not move on a refusal"
        );

        #[cfg(unix)]
        if break_it == 1 {
            use std::os::unix::fs::PermissionsExt;
            let _ =
                std::fs::set_permissions(&s.landed_record, std::fs::Permissions::from_mode(0o644));
        }
    }
}

/// Removing a session takes its replay point with it.
///
/// Ids come back around — `next_id` is the highest `sNN` plus one — so a
/// record left behind is inherited by a session that has nothing to do with
/// it, and says a branch has already been handed commits it has never seen.
/// The seed goes for this reason and this goes with it.
#[test]
fn reaping_takes_the_replay_point_with_it() {
    let (d, wt, shadow_dir) = fixture();
    let checkout = d.path().join("checkout");
    let s = Shadow::new(&shadow_dir, "s01");
    s.ensure(&wt, &[]).unwrap();
    std::fs::write(wt.join("f.txt"), "base\nwork\n").unwrap();
    git(&s.gitdir, &wt, &["commit", "-qam", "Some work"]).unwrap();
    s.harvest(&checkout, &wt, "omh/s01", &[], Keep::All)
        .unwrap();
    assert!(
        s.landed().unwrap().is_some(),
        "the precondition is that a harvest recorded one"
    );

    s.reap();

    assert!(
        Shadow::new(&shadow_dir, "s01").landed().unwrap().is_none(),
        "the next session to take this id must start from its own seed"
    );
}

/// A sandbox that rewound past what you kept is refused, not replayed.
///
/// `git reset --hard` is one of the four commands this repository exists to
/// give back, so an agent dropping a checkpoint below the point omh already
/// harvested is ordinary. What is not ordinary is what a harvest could do
/// about it: the record names a commit the history no longer reaches, and
/// replaying from the seed instead would offer the branch work it already
/// has. Neither is omh's to choose, so it stops and names the way out.
#[test]
fn a_sandbox_that_rewound_past_what_you_kept_is_refused() {
    let (d, wt, shadow_dir) = fixture();
    let checkout = d.path().join("checkout");
    let s = Shadow::new(&shadow_dir, "s01");
    s.ensure(&wt, &[]).unwrap();
    let start = git(&s.gitdir, &wt, &["rev-parse", "HEAD"]).unwrap();

    std::fs::write(wt.join("f.txt"), "base\nkept\n").unwrap();
    git(&s.gitdir, &wt, &["commit", "-qam", "Work that gets kept"]).unwrap();
    s.harvest(&checkout, &wt, "omh/s01", &[], Keep::All)
        .unwrap();

    // the agent rewinds behind what omh already took
    git(&s.gitdir, &wt, &["reset", "-q", "--hard", start.trim()]).unwrap();
    std::fs::write(wt.join("g.txt"), "different\n").unwrap();
    git(&s.gitdir, &wt, &["add", "-A", "."]).unwrap();
    git(&s.gitdir, &wt, &["commit", "-qm", "A different direction"]).unwrap();

    let err = s
        .harvest(&checkout, &wt, "omh/s01", &[], Keep::All)
        .unwrap_err()
        .to_string();
    assert!(
        err.contains("omh s commit -m"),
        "a refusal has to say what to do instead: {err}"
    );
}

/// A carried file must never be **tracked** in the seed, and the reason is
/// not the one you would guess.
///
/// Tracking it looks like a straight improvement: a tracked file survives
/// `git clean -fdx` without needing a mount, and without the mount the
/// agent can edit it with `sed -i` and `mv`, which a mountpoint refuses
/// with `Device or resource busy`. Measured, all of that is true.
///
/// What is also true, and settles it: a tracked file is in the tree of
/// **every commit that follows it**, so any fetch that brings one agent
/// commit brings the file. The harvest fetches this repository into the
/// *user's own*, so a carried secret in the seed is copied into the real
/// repository by every harvest that gets past `preflight` — measured,
/// readable there with `git cat-file -p`.
///
/// Stated that way on purpose. "A fetch takes every reachable object" is
/// true and invites three rebuttals that all fail: `--depth=1` fetches one
/// commit and still carries the blob, because it is in that commit's tree;
/// there is no transport that fetches a range; and the seed cannot be
/// excluded because it is the rebase base `replant` needs. `--filter=blob:none`
/// is worse than none of them — the shadow sets no `uploadpack.allowFilter`,
/// so git warns, sends the blob anyway, and leaves the user's repository a
/// promisor pointing at a directory `omh s rm` deletes.
///
/// On the success path it is unreachable afterwards, because the scratch
/// ref is deleted, and "gc will get it eventually" is not a thing to say
/// about somebody's credentials. On a *failure* path it is worse than that:
/// the ref is kept deliberately — that is what makes a failed replant
/// recoverable — so the secret would sit reachable from a live ref in the
/// user's repository until someone noticed it. Every refusal after the
/// fetch lands there: a carried file in a commit, a branch that moved, a
/// replant that conflicted.
///
/// So the mount stays and `sed -i` stays broken, and this guards the trade
/// against being quietly reversed by someone fixing the visible half.
#[test]
fn a_carried_file_is_never_in_the_seed_the_harvest_fetches() {
    let (d, wt, shadow_dir) = fixture();
    let checkout = d.path().join("checkout");
    let s = Shadow::new(&shadow_dir, "s01");
    std::fs::write(wt.join(".env"), "API_TOKEN=ghp_abc123def456\n").unwrap();
    std::fs::write(wt.join("cert.pem"), "BEGIN-ghp_abc123def456-END\n").unwrap();
    // Two, because a partially-effective exclusion passes a test that
    // carries one.
    s.ensure(&wt, &[".env".to_string(), "cert.pem".to_string()])
        .unwrap();

    let tracked = git(&s.gitdir, &wt, &["ls-tree", "-r", "--name-only", "HEAD"]).unwrap();
    // `ls-files`, not only `ls-tree`: staging the file without committing it
    // is the *other* way to make `git clean` spare it, and it leaves the
    // tree clean. `harvest` then runs `add -A .` and commits the index
    // before it fetches, so the index-only variant leaks too — through a
    // commit omh makes itself.
    let staged = git(&s.gitdir, &wt, &["ls-files"]).unwrap();
    for probe in [&tracked, &staged] {
        assert!(
            !probe.contains(".env") && !probe.contains("cert.pem"),
            "the seed must neither track nor stage a carried file: {probe}"
        );
    }

    // and the half that matters — what a harvest would carry across
    git_in(
        &checkout,
        &[
            "-c",
            "protocol.file.allow=always",
            "fetch",
            "-q",
            &s.gitdir.to_string_lossy(),
            "+HEAD:refs/omh/probe",
        ],
    )
    .unwrap();
    let objects = git_in(&checkout, &["rev-list", "--objects", "refs/omh/probe"]).unwrap();
    // Non-vacuity. An enumeration that returns nothing passes every
    // assertion below without looking at anything, and one typo in the
    // refspec is all that takes.
    assert!(
        !objects.trim().is_empty(),
        "the probe enumerated no objects, so it proved nothing"
    );
    let mut read_a_blob = false;
    for line in objects.lines() {
        let oid = line.split_whitespace().next().unwrap_or_default();
        // `unwrap`, not `unwrap_or_default`: an unreadable object would
        // otherwise pass this assertion as an empty string, which is the
        // vacuous pass this test exists to not be.
        let body = git_in(&checkout, &["cat-file", "-p", oid]).unwrap();
        read_a_blob |= body.contains("base");
        assert!(
            !body.contains("ghp_abc123def456"),
            "a fetch put the carried secret in the user's repository: {line}"
        );
    }
    assert!(
        read_a_blob,
        "the enumeration never read file content, so a leak in one would \
         not have been seen"
    );
}

/// A selection lands exactly those commits, in the order it named them.
///
/// The order is the half that is easy to get wrong and impossible to see:
/// a rebase that sorted the todo would land the same set and a different
/// history, and every assertion about *which* commits arrived would still
/// pass.
#[test]
fn a_selection_lands_those_commits_in_that_order() {
    let (d, wt, shadow_dir) = fixture();
    let checkout = d.path().join("checkout");
    let s = Shadow::new(&shadow_dir, "s01");
    s.ensure(&wt, &[]).unwrap();
    // Separate files, so reordering is a clean replay rather than a
    // conflict — what is under test is the selection, not merge.
    for m in ["one", "two", "three", "four"] {
        std::fs::write(wt.join(format!("{m}.rs")), format!("fn {m}() {{}}\n")).unwrap();
        git(&s.gitdir, &wt, &["add", "-A", "."]).unwrap();
        git(&s.gitdir, &wt, &["commit", "-qm", m]).unwrap();
    }
    let ids: Vec<String> = s
        .checkpoints(&wt)
        .unwrap()
        .commits
        .iter()
        .map(|c| c.id.clone())
        .collect();

    // `--keep 3,1` — the third checkpoint, then the first.
    let harvest = s
        .harvest(
            &checkout,
            &wt,
            "omh/s01",
            &[],
            Keep::These(vec![ids[2].clone(), ids[0].clone()]),
        )
        .unwrap();

    let log = git_in(&checkout, &["log", "--format=%s", "main..omh/s01"]).unwrap();
    let on_branch: Vec<&str> = log.lines().collect();
    assert_eq!(
        on_branch,
        vec!["one", "three"],
        "newest first from `git log`, so `three` was applied first: {log}"
    );
    assert_eq!(harvest.landed, 2, "and the count is what arrived: {log}");
}

/// What a selection leaves out stays out.
///
/// The commits omh did not name are still in the fetched range, and a
/// rebase that ignored the todo would replay all four while every
/// assertion about the two that *are* there kept passing.
#[test]
fn a_selection_leaves_the_rest_where_they_were() {
    let (d, wt, shadow_dir) = fixture();
    let checkout = d.path().join("checkout");
    let s = Shadow::new(&shadow_dir, "s01");
    s.ensure(&wt, &[]).unwrap();
    for m in ["one", "two", "three", "four"] {
        std::fs::write(wt.join(format!("{m}.rs")), format!("fn {m}() {{}}\n")).unwrap();
        git(&s.gitdir, &wt, &["add", "-A", "."]).unwrap();
        git(&s.gitdir, &wt, &["commit", "-qm", m]).unwrap();
    }
    let ids: Vec<String> = s
        .checkpoints(&wt)
        .unwrap()
        .commits
        .iter()
        .map(|c| c.id.clone())
        .collect();

    s.harvest(
        &checkout,
        &wt,
        "omh/s01",
        &[],
        Keep::These(vec![ids[0].clone(), ids[1].clone()]),
    )
    .unwrap();

    let log = git_in(&checkout, &["log", "--format=%s", "main..omh/s01"]).unwrap();
    assert_eq!(log.lines().count(), 2, "two of the four: {log}");
    assert!(
        !log.contains("three") && !log.contains("four"),
        "the ones not named are not on the branch: {log}"
    );
    // The assertion that used to stand here — that the sandbox still holds
    // `three` and `four` — was a tautology: nothing in `harvest` touches
    // the sandbox's own history, so no mutation could redden it. Worse,
    // its comment claimed it was what made a second `--keep` able to take
    // them, which was the one thing that was **not** true. The real guard
    // is the next test.
}

/// A selection takes exactly what it names, and does not sweep the
/// uncommitted tail into a commit nobody asked for.
///
/// `harvest` commits whatever the agent left behind before it does
/// anything else, so `--keep` never drops the tail of a session. For a
/// selection that sweep is a trap: the numbers were resolved *before* it
/// ran, so the commit it makes is one the user could not have named. It
/// would be created, left unapplied, and — before the replay point learned
/// to stop at what was skipped — recorded as handed over. Work invented by
/// the command that then abandoned it.
#[test]
fn a_selection_does_not_sweep_up_work_the_user_could_not_have_named() {
    let (d, wt, shadow_dir) = fixture();
    let checkout = d.path().join("checkout");
    let s = Shadow::new(&shadow_dir, "s01");
    s.ensure(&wt, &[]).unwrap();
    for m in ["one", "two"] {
        std::fs::write(wt.join(format!("{m}.rs")), format!("fn {m}() {{}}\n")).unwrap();
        git(&s.gitdir, &wt, &["add", "-A", "."]).unwrap();
        git(&s.gitdir, &wt, &["commit", "-qm", m]).unwrap();
    }
    let ids: Vec<String> = s
        .checkpoints(&wt)
        .unwrap()
        .commits
        .iter()
        .map(|c| c.id.clone())
        .collect();
    // …and then the agent keeps working, without committing.
    std::fs::write(wt.join("in-flight.rs"), "fn later() {}\n").unwrap();

    s.harvest(
        &checkout,
        &wt,
        "omh/s01",
        &[],
        Keep::These(vec![ids[0].clone()]),
    )
    .unwrap();

    let log = git(&s.gitdir, &wt, &["log", "--format=%s"]).unwrap();
    assert!(
        !log.contains("Work in progress"),
        "a selection made a commit the user never named: {log}"
    );
    let read = s.checkpoints(&wt).unwrap();
    assert_eq!(
        read.commits.len(),
        2,
        "still two checkpoints, not three: {read:?}"
    );
    assert!(
        read.uncommitted > 0,
        "and the tail is still where the next `--keep` can see it: {read:?}"
    );
}

/// A second `--keep` brings the rest, which a partial handover must not
/// make impossible.
///
/// This is the guard for the defect the record write used to carry:
/// `harvest` recorded the fetched HEAD whatever was taken, so after
/// `--keep 1,3` checkpoints 2 and 4 read as already handed over. `log`
/// drew no divider, a second `--keep` said *nothing new to keep*, naming
/// one refused it as already on the branch, and `omh sNN rm` then deleted
/// the only copy — with every screen the user could check agreeing the
/// work was safe.
///
/// Two harvests, because one cannot see it. Every assertion about the
/// first is satisfied by the broken version.
#[test]
fn a_second_keep_brings_what_the_first_selection_left() {
    let (d, wt, shadow_dir) = fixture();
    let checkout = d.path().join("checkout");
    let s = Shadow::new(&shadow_dir, "s01");
    s.ensure(&wt, &[]).unwrap();
    for m in ["one", "two", "three", "four"] {
        std::fs::write(wt.join(format!("{m}.rs")), format!("fn {m}() {{}}\n")).unwrap();
        git(&s.gitdir, &wt, &["add", "-A", "."]).unwrap();
        git(&s.gitdir, &wt, &["commit", "-qm", m]).unwrap();
    }
    let ids: Vec<String> = s
        .checkpoints(&wt)
        .unwrap()
        .commits
        .iter()
        .map(|c| c.id.clone())
        .collect();

    // `--keep 1,3` — skipping 2, which is what makes the replay point
    // unable to advance past it.
    s.harvest(
        &checkout,
        &wt,
        "omh/s01",
        &[],
        Keep::These(vec![ids[0].clone(), ids[2].clone()]),
    )
    .unwrap();
    let read = s.checkpoints(&wt).unwrap();
    assert!(
        read.commits.iter().filter(|c| c.landed).count() <= 1,
        "only what was taken and everything before it may read as handed over: {:?}",
        read.commits
    );

    // …and then the rest.
    let landed = s
        .harvest(&checkout, &wt, "omh/s01", &[], Keep::All)
        .unwrap()
        .landed;
    let log = git_in(&checkout, &["log", "--format=%s", "main..omh/s01"]).unwrap();
    for m in ["one", "two", "three", "four"] {
        assert!(
            log.contains(m),
            "`{m}` never reached the branch across two harvests: {log}"
        );
    }
    assert!(
        landed >= 2,
        "the second harvest brought the ones the first left: {log}"
    );
}

/// Curation is `--edit`'s headline behaviour, and this is the only test
/// that executes it. It cannot be reached from `tests/cli.rs` by
/// construction — `--edit` refuses without a terminal and no test process
/// has one — so if this goes, the `-i` path has no coverage at all.
///
/// It once read "every other test passes `curate: false` while `--keep`
/// only ever passes `true`", which was true of the `bool` that `Keep`
/// replaced in #56 and is worth keeping only as the reason the test
/// exists: deleting the `-i` left the suite green.
///
/// A sequence editor that *edits* the todo, not one that accepts it. An
/// editor that exits without touching the list is behaviourally identical
/// to `-q`, so it proves the branch was taken and nothing about what it
/// does — this drops a line, which is the whole point of opening the list.
///
/// It also pins the count. Reported from the commits *fetched*, "kept 3"
/// printed over a branch that got 1.
#[test]
fn curating_drops_what_the_user_drops_and_says_so() {
    let (d, wt, shadow_dir) = fixture();
    let checkout = d.path().join("checkout");
    let s = Shadow::new(&shadow_dir, "s01");
    s.ensure(&wt, &[]).unwrap();
    // Separate files, so dropping the middle commit is a clean drop
    // rather than a conflict — the curation is what is under test here.
    for m in ["one", "two", "three"] {
        std::fs::write(wt.join(format!("{m}.rs")), format!("fn {m}() {{}}\n")).unwrap();
        git(&s.gitdir, &wt, &["add", "-A", "."]).unwrap();
        git(&s.gitdir, &wt, &["commit", "-qm", m]).unwrap();
    }

    // the user opens the list and deletes the middle pick
    let editor = d.path().join("drop-one.sh");
    std::fs::write(&editor, "#!/bin/sh\nsed -i.bak '2d' \"$1\"\n").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&editor, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    git_in(
        &checkout,
        &["config", "sequence.editor", &editor.to_string_lossy()],
    )
    .unwrap();

    let landed = s
        .harvest(&checkout, &wt, "omh/s01", &[], Keep::Edit)
        .unwrap()
        .landed;

    let log = git_in(&checkout, &["log", "--format=%s", "main..omh/s01"]).unwrap();
    let on_branch = log.lines().count();
    assert_eq!(on_branch, 2, "one of the three was dropped: {log}");
    assert_eq!(
        landed, on_branch,
        "the number reported is what landed, not what was offered: {log}"
    );
}

/// Two of the three states that made a harvest report success while leaving
/// commits behind — measured against the replant that ran without them.
/// The third, an interrupted rebase, has its own test below.
#[test]
fn a_harvest_refuses_a_history_it_cannot_see_all_of() {
    let (_d, wt, shadow_dir) = fixture();
    let s = Shadow::new(&shadow_dir, "s01");
    s.ensure(&wt, &[]).unwrap();
    let cp = |m: &str| {
        std::fs::write(wt.join("f.txt"), format!("{m}\n")).unwrap();
        git(&s.gitdir, &wt, &["commit", "-qam", m]).unwrap();
    };
    cp("first");
    cp("second");
    s.preflight(&wt).expect("a clean history harvests");

    // the agent looks at an old checkpoint and does not come back
    git(&s.gitdir, &wt, &["checkout", "-q", "HEAD~1"]).unwrap();
    let err = s.preflight(&wt).unwrap_err().to_string();
    assert!(err.contains("detached"), "{err}");
    assert!(err.contains(&s.branch), "say how to put it back: {err}");

    // a branch it made and wandered off
    git(&s.gitdir, &wt, &["checkout", "-q", &s.branch]).unwrap();
    git(&s.gitdir, &wt, &["branch", "aside", "HEAD"]).unwrap();
    git(&s.gitdir, &wt, &["reset", "-q", "--hard", "HEAD~1"]).unwrap();
    let err = s.preflight(&wt).unwrap_err().to_string();
    assert!(
        err.contains("no branch it is on can reach"),
        "stranded commits are the ones no other check sees: {err}"
    );
}

/// An interrupted rebase leaves HEAD on something that is neither the work
/// nor a decision anyone made, and the marker is right there in the gitdir.
#[test]
fn a_harvest_refuses_a_repository_left_mid_rebase() {
    let (_d, wt, shadow_dir) = fixture();
    let s = Shadow::new(&shadow_dir, "s01");
    s.ensure(&wt, &[]).unwrap();
    std::fs::create_dir_all(s.gitdir.join("rebase-merge")).unwrap();

    let err = s.preflight(&wt).unwrap_err().to_string();
    assert!(
        err.contains("rebase-merge"),
        "name what is in progress: {err}"
    );
}

/// The seed is the fixed point a replant measures from, and it is recorded
/// on the host precisely so the sandbox cannot move it.
#[test]
fn the_seed_survives_the_sandbox_deleting_everything_it_can_reach() {
    let (_d, wt, shadow_dir) = fixture();
    let s = Shadow::new(&shadow_dir, "s01");
    s.ensure(&wt, &[]).unwrap();
    let seed = s.seed().unwrap();

    let _ = git(&s.gitdir, &wt, &["tag", "-d", "session-start"]);
    let _ = std::fs::remove_dir_all(s.gitdir.join("refs/tags"));

    assert_eq!(s.seed().unwrap(), seed);
    assert!(!seed.is_empty());
}

/// Checkpointing is the feature, and a checkpoint is a commit, and git
/// refuses to commit without an identity. The container carries no global
/// git config, so unless the repository brings its own the agent's first
/// `git commit` dies on `Author identity unknown` — the whole point of the
/// module, unavailable, on a machine that has never been configured.
///
/// Asserted as *the repository carries an identity*, not as "a commit
/// works here", because a commit working here proves nothing. git invents
/// an identity from the OS user and hostname when it can, so on a developer
/// machine the commit succeeds with no config at all — deleting the two
/// lines this guards left it green, while CI, whose runner has an empty
/// gecos field, failed on `empty ident name`. A test that cannot go red on
/// the machine you are writing it on is decoration.
///
/// The commit is still made, because config that is set and not honoured
/// would be its own bug.
#[test]
fn the_agent_can_commit_without_any_identity_of_its_own() {
    let (_d, wt, shadow_dir) = fixture();
    let s = Shadow::new(&shadow_dir, "s01");
    s.ensure(&wt, &[]).unwrap();

    // a file the seed already tracks, so `-a` stages it
    std::fs::write(wt.join("f.txt"), "base\nthe agent's work\n").unwrap();
    let out = Command::new("git")
        .current_dir(&wt)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .arg("--git-dir")
        .arg(&s.gitdir)
        .arg("--work-tree")
        .arg(&wt)
        .args(["commit", "-q", "-am", "a checkpoint"])
        .output()
        .unwrap();

    assert!(
        out.status.success(),
        "a sandbox with no git identity must still be able to check point: {}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );

    // The assertion that can actually fail here.
    for (key, want) in [("user.name", AUTHOR_NAME), ("user.email", AUTHOR_EMAIL)] {
        let got = git(&s.gitdir, &wt, &["config", "--local", key]).unwrap_or_default();
        assert_eq!(
            got.trim(),
            want,
            "the repository has to bring its own {key}; nothing in a \
             container supplies one"
        );
    }
}

/// The exclude list follows the mounts, and the mounts change under it.
///
/// omh derives what the sandbox's repository must not track from the mounts
/// it is about to make, and wrote that list once — when the repository was
/// created. Switch a capability on afterwards and the mount it adds inside
/// `/work` is a file the existing sandbox neither tracks nor excludes, so
/// the agent's own `git add -A` commits omh's rendered document — MCP
/// environment and all — into a history `omh s commit --keep` replays onto
/// the branch.
///
/// Asserted as the property that matters rather than as a line in a file:
/// the document cannot be staged. A test that greps `info/exclude` passes
/// for a list that git never reads.
#[test]
fn a_capability_added_later_is_still_kept_out_of_the_sandboxs_history() {
    let (_d, wt, shadow_dir) = fixture();
    let s = Shadow::new(&shadow_dir, "s01");
    s.ensure(&wt, &[".env".to_string()]).unwrap();

    std::fs::write(wt.join("agent.rs"), "fn main() {}").unwrap();
    git(&s.gitdir, &wt, &["add", "-A"]).unwrap();
    git(&s.gitdir, &wt, &["commit", "-q", "-m", "a checkpoint"]).unwrap();
    let checkpoint = git(&s.gitdir, &wt, &["rev-parse", "HEAD"]).unwrap();

    // the next launch mounts one more document inside /work
    let grown = [".env".to_string(), ".mcp.json".to_string()];
    s.ensure(&wt, &grown).unwrap();

    // what the mount would put there, credentials and all
    std::fs::write(wt.join(".mcp.json"), "{\"env\":{\"TOKEN\":\"sk-live-42\"}}").unwrap();
    git(&s.gitdir, &wt, &["add", "-A", "."]).unwrap();
    let staged = git(&s.gitdir, &wt, &["diff", "--cached", "--name-only"]).unwrap();

    assert!(
        !staged.contains(".mcp.json"),
        "a document omh mounted is not the agent's work to commit: staged {staged:?}"
    );
    assert_eq!(
        git(&s.gitdir, &wt, &["rev-parse", "HEAD"]).unwrap(),
        checkpoint,
        "and refreshing the list must not disturb what the agent already did"
    );
}

/// Relaunching into a running session is ordinary — resuming twice, an
/// editor attaching alongside a terminal. Re-seeding there would throw away
/// every checkpoint the agent had made, which is the one thing this whole
/// repository exists to keep.
#[test]
fn seeding_twice_keeps_the_work_the_agent_already_did() {
    let (_d, wt, shadow_dir) = fixture();
    let s = Shadow::new(&shadow_dir, "s01");
    s.ensure(&wt, &[]).unwrap();

    std::fs::write(wt.join("agent.rs"), "fn main() {}").unwrap();
    git(&s.gitdir, &wt, &["add", "-A"]).unwrap();
    git(&s.gitdir, &wt, &["commit", "-q", "-m", "a checkpoint"]).unwrap();
    let checkpoint = git(&s.gitdir, &wt, &["rev-parse", "HEAD"]).unwrap();

    s.ensure(&wt, &[]).unwrap();

    assert_eq!(
        git(&s.gitdir, &wt, &["rev-parse", "HEAD"]).unwrap(),
        checkpoint,
        "a relaunch must not discard the agent's checkpoints"
    );
}
