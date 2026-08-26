# Git

**Built, in 0.6.0.** This page is the design for what goes *around* the worktree
model and the sandbox's own repository: the loop a developer works in, and the
way several sandboxes of one repo are reached from one place. All twelve steps
in [Order](#order) have landed — each names the pull request that did it, and
the entries record what was measured or got corrected on the way rather than
only what was intended.

One thing is still open, and it is named at step 11: that a read-only `config`
really does refuse inside a real container. It needs docker rather than a unit
test.

It rested on defects in what already existed. Those are in
[Foundations](#foundations), each measured against git 2.55.0 on 2026-08-21, and
they landed first.

## Three layers, and only one is yours

**Your repository.** Your branches, your history, your remotes. Git is the right
tool here and omh hands you real git without apology. omh's only jobs are to put
work into it and to never damage it.

**The session.** omh's noun. It has an id, a state, a change, checkpoints and a
distance from trunk. Every omh command works at this level, and git does not
appear.

**The mechanism.** The worktree, the branch `omh/sNN`, the shadow gitdir, the
seed, the replay point. omh's private business. It is explained here and in
[Sessions](../sessions.md), and it appears nowhere else — not in output, not in
an error message, not in a suggested command.

The rule discriminates correctly at the edge, which is why it is worth writing
down rather than leaving to taste. `omh s01 rm` telling you `git branch -D
omh/s01` is fine: the branch is in your repository, it is yours, and git is how
you handle it. `omh s01 diff` telling you `git -C ~/.omh/worktrees/proj/s01
checkout omh/s01` is never fine — that is omh asking you to hand-repair its own
plumbing. Same command family, opposite verdicts, one rule.

Stated so a test can hold it:

> No omh output names a git invocation against omh's own plumbing, a worktree
> path, or a shadow gitdir. The only git omh ever hands you runs in **your**
> checkout, against **your** branch.

It goes red today, in at least four messages. That is the point of writing it as
an assertion over rendered reports rather than as a paragraph.

A second thing falls out of owning the mechanism: it can be changed silently.
The worktree directory is keyed by the checkout's *basename* today, so two
clones both called `api` share one — a real bug, and one that can be fixed
without a deprecation, because nothing user-facing depends on that path.

## What a developer cannot do today

Four workflows, not a feature list.

| I want to | today |
|---|---|
| ~~**read the change** before landing it~~ — landed (#54, #55) | `omh s diff` printed `--stat` only and the patch was in a worktree the docs tell you never to enter. `omh sNN log` numbers the agent's commits and `omh sNN diff [n] [-p]` reads one, or the session, as a patch. |
| **know what the agent has been doing** | Its commits were invisible until `--keep` opened a `rebase -i` todo. `omh s` does not count them; `s rm` destroys them without saying they existed. |
| ~~**land work in stages**~~ — landed (#50) | `--keep` replayed from the seed every time. Measured: a second run re-listed commits already on the branch, then died on `Could not apply`. It replays from what it last handed over now. |
| ~~**not fall behind trunk**~~ — landed (#60, #62) | `omh s` reported `behind 12` and offered nothing. `omh sNN sync` is the answer and the dashboard names it, per session, for the sessions it actually applies to. |

Everything else about the git story holds up. The worktree boundary, the
shadow's isolation, refuse-never-strip, fetch-before-replant: those were probed
and they are sound. The work is in the loop around them.

## Sessions are peers over one base

Git's unit is one repository at one path, and to act on a worktree you stand in
it. omh's unit is different and has no git noun: **a set of changes over one
base, all created by omh from one checkout.** That is a fact about them that git
cannot know, and it is where omh gets to be better rather than merely safe.

It buys questions nothing else can answer:

- which session is ahead, which is stale, which holds work you have not taken
- **which two sessions are about to collide** — `s01` and `s03` both changing
  `src/render.rs` is a conflict git will only mention at merge time, once both
  are branches and both are expensive to abandon

Overlap costs nothing to compute. `omh s` already runs `status --porcelain -uall`
per session for its uncommitted count; the paths are in output it currently
parses and throws away.

**One place to look, one session to act on.** The dashboard is the only
cross-session surface. There is no `--all`: trunk moving means three syncs that
can each conflict differently, and you want to see the first outcome before
starting the second. So the dashboard names what needs doing and prints the
lines; it does not run them.

## Naming a session

The selector goes **first**, and everything after it is what you would have
typed anyway.

```console
$ omh s               every session — the dashboard
$ omh s01             that session — state, work, what is not yours yet
$ omh s01 diff -p     act on it
$ omh s01 resume      rejoin it
```

`s` is the sessions namespace scoped to the current session; `sNN` is the same
namespace scoped to that one. The desugaring is literal:

```
omh s01 log      ≡  omh sessions --session s01 log
omh s01 commit   ≡  omh sessions --session s01 commit
```

One extension, and it is the only place the equivalence is not a pure alias:
when what follows is not a session verb, the prefix still sets the session and
the command runs where it lives — `omh s01 attach zed`,
`omh s01 graph`. Same rule, and it covers the launch, which `sessions` has no
verb for.

**The value is the deletions.** All four of these worked before #53 and meant
the same thing:

```console
$ omh s diff              # the session omh picks
$ omh s diff s01          # positional
$ omh s -s s01 diff       # flag after the namespace
$ omh -s s01 s diff       # flag before it
```

Four spellings, two mechanisms, applied unevenly — `rm` *required* the
positional, `diff`, `down` and `graph` took one or not, and `commit` and `push`
had no field to read one from, so naming a session there was a parse error
rather than something ignored. `push` could not have had one, because that slot
is the branch name. So this is one form and three removals: the positional
session left `s diff`, `s down` and `graph`; the required positional left
`s rm`; `--session` stays, because it is what the prefix desugars to and the
only way to name an id that is not `sNN`.

Naming it twice — `omh s01 -s s02 diff` — is refused rather than resolved, and
after the parse rather than before it, because clap is what knows that
`--session s02`, `--session=s02`, `-s s02` and `-ss02` are one flag. The first
version scanned argv for two whole tokens and let the other two spellings
through.

Two rules make the parse unambiguous:

- a leading token matching `s\d+` is lifted into the session, and the rest is
  parsed exactly as it is today
- a session id may not be named like an omh command. `RESERVED` and the test
  that no bundled definition shadows a command already exist; `validate_id`
  joins them, or `--session diff` creates a session you can never address

`omh s01 ls` names a verb retired in 2026.08. It errors — *"there is no `ls`
verb any more"* — rather than ignoring the scope, which is what it would have
done when this was written: the line fell through to a live top-level `ls` and
listed every session. That fall-through is gone twice over now, so what the
tombstone buys is the wording, not the refusal.

## The replay point

One record does most of the work below. Today the harvest always replays
`seed..scratch`, and the seed never moves. Add a sibling of `<id>.seed` —
`<id>.landed` — written on the host and never read from the sandbox, for the
same reason the seed is not.

It is advanced by exactly two operations: a successful `--keep`, and a `sync`.
Everything else follows:

- **`--keep` becomes repeatable.** The range is `landed..scratch`, so a second
  harvest replays only what is new.
- **`log` can draw a line** between what is yours and what is not.
- **`sync` can fold trunk into the shadow** without the harvest later replaying
  trunk's own changes as though the agent had made them.
- **`rm` can refuse honestly.** *"s01 has 7 commits you have not kept"* becomes
  a question it can ask, which is what [risks.md](risks.md) 2c is waiting for.

**When the sandbox rewrote history below it** — `reset --hard`, amend, rebase,
all things the shadow exists to permit — `landed` stops being an ancestor of the
tip. Detect it with `merge-base --is-ancestor` and **refuse**, naming it, and
point at `omh sNN commit -m` to take the files without the history. Replaying
from the seed instead would duplicate or conflict, silently in the first case.

## The surfaces

Two new verbs. Everything else is a flag on something that exists.

### `omh s` — the dashboard

```
base main · 3 sessions

  s01  running claude     12 files  +340 −52   4 not yours   2 behind
  s02  stopped             3 files  +18  −4    kept          up to date
  s03  running opencode    9 files  +210 −33   1 not yours   2 behind

  s01 and s03 both change src/render.rs, src/base.rs

  omh s01 sync     omh s03 sync
```

That last line is a sketch and the shipped shape differs from it in three ways
worth knowing, since the rest of this section is written as though it were the
implementation: the suggestions are one per session rather than side by side,
each carries what it does, and they are **asides on stderr** — `omh s >
sessions.txt` is a record of what is in flight and advice is not part of that
record. *Up to date* is an empty cell rather than those words; what changed is
that the cell it used to share with *omh could not count* is no longer shared.

`omh s` remains the short spelling of `omh sessions`.

**The suggestion landed (#62), and with it the fix behind it.** The column had
been rendering *up to date* and *omh could not count* as the same empty cell —
the pair this design's own report rules call the most dangerous to confuse, on
the surface where a user picks which session to open. A stale session that
looks current is how work gets done against code that moved, which is the
failure this whole phase exists to close. Unknown now says so, in `WARN`, and
never carries a number; and the suggestion is offered only for a count omh
actually took, because advising a merge off a question that failed is advice
built on a guess. That row is given `omh sNN log` instead, which prints the
reason — silence beside rows that each carry a next step reads as *this one is
fine*, which is the same collapse moved one layer out.

The count itself was being manufactured the way this section warns against.
`sessions_ls` took it with `.ok()`, so *git would not answer* arrived as an
absence with the answer discarded — in a function that ten lines earlier routes
the identical failure from `changed()` into a reported list, and which `log`
has always handled by printing git's own words. Both call sites say why now.

**`omh s01` on its own is this row — landed (#67), by giving something up.**

It was an error for as long as `omh s` required a verb. Step 8 refused
`omh s01 ls` by name — *"`ls` lists every session; drop the `s01`"* — and the
reason recorded at the time was an implementation one rather than a semantic
one: the listing took no session argument, so honouring the scope was not
possible, and ignoring it would have listed every session while looking like it
had listed one.

What opened it is the listing learning a scope. Once it can be narrowed, *list
every session, but one* stops being nonsense — it is this row — and the no-verb
case is free: `omh s` is the listing, `omh s01` is the listing scoped to one
session.

Retiring `ls` followed, so that one thing has one spelling; it is not what made
the row possible, and deleting it outright turned out to be the wrong shape.
`omh s01 ls` stayed typeable and stopped being refusable, falling through to the
top-level inventory with the session silently dropped — the very harm step 8's
refusal was written against. The verb is kept as a hidden tombstone that refuses
by name. That is the selector's own rule reaching the last place it had
not, rather than a special case — which is also why `omh s01` is that row and
not a menu of verbs. The user named a session; answering with a list of things
they could have typed instead discards what they said.

Every session is still read when one is asked for, because a collision is a
fact about two of them: *"s01 and s03 both change src/render.rs"* has to
survive the focus, and it does.

### `omh sNN log` — make the invisible visible

**Landed in #54.** Reads the sandbox's gitdir on the host. This is the command
that changes how a session feels: before it, you could not tell that the agent
had been committing at all.

```
s01 · 4 checkpoints, 2 not yours yet · 2 behind main

  4  12m  Extract the tap guard into its own function  3 files   +48 −12
  3  38m  Add the failing test first                   1 file    +23
  ────────────────────────── yours from here ───────────────────────────
  2  1h   Fix typo                                     1 file    +1 −1
  1  1h   Rename shadow to sandbox repo                12 files  +90 −90

  uncommitted in the sandbox: 2 files

  omh s01 commit --keep    bring the 2 new ones onto the branch
```

Numbered, so there is no object id to copy and no ref to name — and numbered
from the *oldest*, so a number keeps meaning the same commit as the agent
commits more. `--topo-order` is what makes that true; measured, plain
`--reverse` splices a merged side branch into the middle of the list and
renumbers everything after it.

The uncommitted line matters as much as the checkpoints: it is the work
`--keep` would sweep into "Work in progress", shown before that happens rather
than after. Measured in the sandbox, where `--keep` measures it —
`Session::uncommitted` answers a different question and would count work the
agent checkpointed an hour ago.

Two answers are *omh did not measure this*, and neither may render as zero: a
merge, which has no diff of its own until you name a parent, and a file git
will not count lines for. They read `merge` and `·N`.

The read carries `NEUTRALISED` and `GUEST_ENV`, and for this command those are
not about executing anything — they are about being believed. Measured against
git 2.55.0 in a gitdir the agent owns: one `git replace` prints a forged subject
and a forged file list beside the real commit id, and one line in `info/grafts`
cuts the list to its newest entry with nothing but a deprecation hint on stderr.
The config key for grafts does not close them; `GIT_GRAFT_FILE` does.

Two states make the list incomplete, and both are ones `harvest` refuses over:
commits on a branch the sandbox wandered off, and a replay point the history no
longer reaches. The log reports both and withholds the `--keep` hint, because a
hint is a promise that the line can be pasted.

### `omh sNN diff` — a real patch, and one checkpoint

**Landed in #55.**

```console
$ omh s01 diff          # the summary it prints today
$ omh s01 diff -p       # the patch, through your pager
$ omh s01 diff 4        # one checkpoint
$ omh s01 diff 4 -p
```

`-p` hands the terminal to git — inherited stdio, git's own pager and colour —
rather than reimplementing paging. `commit`'s editor path already establishes
that pattern. Under `--json` it never pages and the patch is a string field.

A checkpoint argument is validated to be inside the session's own range before
anything is printed. Not for safety: a command that will print any object in the
store is a different command from one that shows you a checkpoint.

The pager was the one place this could not simply hand the terminal to git —
because of omh's own hardening, not a live threat. Measured on a pty, a
repository's `core.pager` of `sh -c "echo …; cat"` executes on a plain
`git show`, which is why `NEUTRALISED` pins the key to `cat`. The sandbox's
config cannot carry one regardless: it is rewritten to a ten-key allowlist each
launch and mounted read-only (#52). The pin is what would leave `-p` unable to
page, so the user's pager — resolved by asking git in the user's own checkout,
so their `core.pager` counts — is appended after it, and the last `-c` wins.

A first draft of this called the sandbox's config "a file the agent writes" and
built the mechanism on that premise. It was three layers out of date, and the
review that caught it was reading omh's own mount code.

`git show` prints the whole commit header — author, subject, body — and quotes
none of it, while paths it does quote. So the summary goes through
`out::untrusted` on the way to a person and stays raw on the way to a program,
the same split `log` makes, reached by a second route. A `--stat` survives
sanitising byte-for-byte (its graph has no control characters); a patch would
not, and never reaches it — a patch for a person is always paged.

Two answers must agree that used not to. `show --stat` on a merge reports the
files it brought in and `show -p` on the same merge prints nothing at all,
git's `--cc` collapsing a clean merge; `--first-parent` makes them agree and
changes neither answer elsewhere. And the JSON key names the content —
`summary` for a `--stat`, `patch` for a patch, never both — so
`jq -r .patch | git apply` works for the reason it used to fail.

### `omh sNN commit --keep [selection]`

The flagship feature required knowing `pick`/`squash`/`drop`, that reordering
can conflict, and how to abort a rebase. Under the layering rule that
is backwards. `log` numbers the checkpoints, and you name what you want:

```console
$ omh s01 commit --keep          # all of them, no editor
$ omh s01 commit --keep 1,3-4    # these, in this order
$ omh s01 commit --keep --edit   # the todo, for people who want it
```

**Landed in #56, with one mechanism changed — reviewed and kept.** The two were
measured against the same history and are user-visibly identical, including that
both refuse a selected merge; see
[decisions](decisions.md#deviations-from-a-written-design-ratified). `--keep` and `--keep --edit` are
a rebase, as designed. A *selection* is `cherry-pick`, which is not what this
said.

The design called for a generated todo delivered through `GIT_SEQUENCE_EDITOR`
pointed at omh's own binary — not at `cp`, because that value goes through
`sh -c` and a profile path with a space would turn a curation into a syntax
error. All of that is true and was measured: unquoted, such a path dies as *No
such file or directory*; quoted it runs, and git appends the todo path
afterwards as one properly quoted argument even when the repository's own path
contains spaces.

It was dropped for something simpler. `cherry-pick <a> <b>` **is** "these
commits, in this order", so there is no editor, no `sh -c`, no quoting, and no
hidden subcommand for `RESERVED` to know about. It is also the only one of the
two a unit test can reach: `current_exe()` inside a test is the *test harness*,
so the todo would have been delivered by running the test binary with
`sequence` as a filter — matching nothing, exiting 0, and leaving git to replay
the unedited list. Both selection tests failed exactly that way before the
mechanism changed, which is how the limitation was found rather than argued.

Rebase stays for everything else: `All` and `Edit` mean "the whole range, in
order". A merge inside that range is where the two differ, and an earlier draft
of this had it backwards: measured, plain `rebase --onto` **flattens** a merge —
it is discarded and both sides are linearised, no flag, no warning — while
`cherry-pick` refuses outright. Linearising without asking is the friendlier
failure for a whole-range replay; refusing is the right one for a selection, so
a selected merge is turned away before anything runs.

Two flags rather than one: `--empty=drop --allow-empty`. Measured — `--empty`
governs commits that *become* empty, and a commit that started empty is
`--allow-empty`'s business. Without the second, an agent's `git commit
--allow-empty` marker (the hazard `stamp` records having met once) aborts a
selection with a message that names no commit and blames a conflict that never
happened, while `--keep` on the same history succeeds.

**The git floor, closed in #57 without naming a version.** `cherry-pick
--empty=` is newer than everything else omh asks of git, and the release that
introduced it was not verifiable here. omh cannot check a version it cannot
name — so it asks the binary instead: `git <verb> -h` lists the options that
git has, needs no repository, and keeps answering as git grows. Measured, it
exits **129** and prints the whole listing on **stdout**, so the status is
ignored on purpose. An earlier note here said the usage line went to stderr; it
does not — that was zsh's MULTIOS merging the streams inside the shell that
measured it. Both are still read, because a version-manager shim may use
either, but that is defensiveness rather than a split that exists.

`omh doctor` reports it, and `--keep <selection>` refuses up front with the
command that still works — rather than the usage dump git would produce after
the fetch. `--keep` and `--keep --edit` ask nothing new of git and are
unaffected.

`--edit` is then the only path that needs a terminal, which is where the tty
guard goes. That closes a measured hole: with stdin not a terminal, `rebase -i`
proceeds on the unedited todo, exits 0, and omh reports a curation that never
happened.

Two more refusals, both before anything moves — `harvest` promises the branch is
untouched when a replant fails, and the cheapest way to keep that promise is not
to have started. A number outside the session's range is refused with the range.
A number naming work the branch already has is refused by name: `log` numbers
every checkpoint including the ones below the divider, so `--keep 1` about
landed work is a reasonable thing to type, and replaying it applies the same
patch twice.

### `omh sNN sync` — trunk moves, and nobody's work is at risk

*Landed (#60). Three claims below were wrong when written and are corrected in
place, each against a measurement.*

The constraint is absolute and is what shapes the mechanism: **no commit from
your checkout may enter the sandbox's repository.** So the merge happens on the
host, in your repository, and the sandbox only ever receives files.

1. **Checkpoint first**, in the shadow, so `log` shows the point sync can be
   undone from.
2. **Take the session's tree** — `write-tree` from the throwaway index `diff`
   already builds, with omh's own mounted paths excluded as they already are.
3. **Merge on the host**: `merge-tree --write-tree --merge-base <branch tip>
   <new base> <session tree>`. Measured: it returns a merged tree plus the
   conflicted paths, exits 1 on conflict, and touches no worktree, index or ref.
4. **Materialise** the merged tree into the session, skipping mount
   destinations.
5. **Move the baseline** — `update-ref` with an expected old value, then
   `reset --mixed` — so `omh sNN diff` still shows the agent's work and not
   trunk's. If the branch carries commits of its own they rebase onto the new
   base in a scratch worktree first, the shape `harvest` already uses, and a
   conflict there refuses, because those commits are yours.
6. **Record it in the shadow** as an omh-authored commit — *"base moved to
   `<sha>`"*, the sha as text and never as an object. Cleanly merged paths
   only; the conflicted ones stay uncommitted, for the reason below.

   **The replay point does not move, and this paragraph used to say it did.**
   The idea was that advancing it past omh's commit stops a harvest replaying
   trunk's changes as the agent's. It does — and it also marks the step-1
   checkpoint as handed over, because the replay point is one ancestor pointer
   and everything below it is "already taken". That checkpoint holds the
   agent's pre-sync work. Built as written, a sync followed by `--keep`
   harvests *nothing*: measured, not reasoned — the test asserts the harvest
   lands the agent's commits and does not land trunk's, and advancing the
   pointer makes it fail on the first half. What the step was protecting is
   true anyway: `cherry-pick` drops a commit whose changes are already on the
   branch, so trunk's changes cannot arrive twice.
7. **Guard the exit.** `commit` refuses while `git diff --check` reports
   leftover conflict markers, naming file and line, and `--force` is how you
   mean it anyway (a fixture holding markers on purpose is a real thing — this
   repository has some).

   Two measurements shaped it. `--check` reports **whitespace errors too, by
   echoing the offending line of the file** — so an agent that writes
   `\t…: leftover conflict marker` on an oddly indented line makes git print
   something indistinguishable from git finding a conflict, and omh refuses a
   commit over a comment. Turning every whitespace check off with one `-c
   core.whitespace=-…` leaves only git's own lines, and the repository cannot
   turn them back on (last `-c` wins). Second, `--check` diffs what git
   **tracks**: a marker in a file the agent never added is invisible to it, and
   `commit -m` is a `git add -A`. So the check runs against the throwaway
   review index, where the tree a commit would make is already staged.

The payoff is worth being loud about: **a conflict is text, and text crosses the
boundary safely.** The agent resolves trunk conflicts with the whole tree in
front of it, in a repository where it cannot hurt you. That is a capability the
isolation *creates* rather than costs.

Both rough edges this section listed turned out better than predicted.

The labels are **not** object ids. Measured: `merge-tree` labels each side with
*the string it was handed*, so the ids only appeared because the draft handed it
ids. Naming the session's tree under `refs/s01` first — a ref git's own DWIM
resolves back to `s01` — makes the markers read `<<<<<<< main` / `>>>>>>> s01`,
which is what anyone resolving a conflict expects to see. The ref is deleted
immediately after; the merge needs it only for the label.

The version floor is real — `--write-tree` wants git ≥ 2.38 — and there is no
fallback. A scratch-worktree merge would put a commit from your checkout into
the sandbox's repository, which is the one thing this section opens by
forbidding, so a git too old means the command is unavailable rather than
slower. `omh doctor` says so by name, separately from the `--keep <selection>`
floor (2.34), because a user on 2.35 has one of the two.

#### Sync requires the sandbox stopped

It refuses while one is up and names `omh sNN down` — or `omh sNN sync --down`,
which does both. An earlier draft of this design had it running live; the
reversal is worth keeping, because the argument for live was wrong in an
instructive way.

Running live is safe for **files**. The checkpoint in step 1 makes an overwrite
recoverable, and only the span between step 2's snapshot and step 4's write is
even at risk. What it is not safe for is the **agent's context**. What the agent
believes the tree contains lives in its conversation, not on disk, and no
checkpoint on disk reaches it. It then edits a version that no longer exists, or
writes a whole file back from stale content, and trunk's changes vanish inside a
plausible-looking patch that you review later. That is the silent-wrong-work
class, which this project treats as the worst outcome available — the same
reasoning that makes a partial harvest refuse rather than succeed quietly.

Stopping is not the price of avoiding that. Stopping is the **fix**: the harness
restarts and reads the tree as it now is. And whether an agent mid-turn may be
interrupted is a judgement only you can make, which is why omh refuses instead
of making it.

The two commits the host writes into the shadow — steps 1 and 6 — are also
writes to an index the agent's own git uses. Stopped, that is nobody's race.
Running, git's own locking makes it a loud transient failure on whichever side
loses rather than corruption; but a loud transient failure inside somebody's
turn is not a thing to design for.

#### What the agent finds when it comes back

Three things, and they are why the mechanism records anything in the sandbox's
repository at all rather than just rewriting files.

- **A commit that explains it.** `git show HEAD` is exactly what moved
  underneath it, and nothing else. Files changing with no explanation is the
  confusing version of this; a commit is the legible one.
- **A `git status` that is the to-do list.** Conflicted paths are deliberately
  not in that commit: they sit in the worktree with their markers, uncommitted,
  so the sandbox's own `status` and `diff` name exactly what needs a decision.
  Worth saying out loud that `git checkout -- <path>` there takes the pre-sync
  side back — recoverable from the step-1 checkpoint, but not obvious.
- **A sentence at session start.** *Trunk moved 12 commits while you were
  stopped; `git show HEAD`; two files need resolving.* omh's hook vocabulary
  already has `session-start` with `capture` and `inject`, so this is a base-set
  hook reading a file omh left behind, not a new mechanism. It has to be
  one-shot, because `session-start` re-fires on resume and compact — a fact
  `base.rs` already records.

### Per-turn snapshots — a timeline for an agent that never commits

**Landed (#66).** A `turn-end` hook photographs the tree onto `refs/omh/turn`: a
`mktemp` index, `read-tree`, `add -A`, `write-tree`, `commit-tree`,
`update-ref`. Never the branch,
so `--keep` still curates the agent's own commits and nothing else, and never
`HEAD`, the index or the worktree — an agent whose working tree went clean at
the end of every turn would find nothing to commit and would rightly conclude
omh had eaten its work.

It costs nothing in context: a `run` hook injects no text. Measured 2026-08-24
it costs ~80ms and ~1.3 KB per turn on a 400-file worktree, and ~50ms with no
disk at all when the tree has not changed — the comparison that finds that out
is most of the time either way — the new tree is compared against the previous
snapshot's and an unchanged one writes no commit.

**Three guards walk refs, and all three exclude the ref.** A snapshot is by
construction not an ancestor of HEAD, which is exactly the shape all three
hunt, and left alone each was permanently wrong in the direction that blocks:
every `--keep` refusing and telling the user to delete omh's own commits, a
warning on every `log`, and `rm` refusing for any session that ever had one
dirty turn.

| | asks | with the exclusion |
|---|---|---|
| `preflight` | would a harvest drop commits? | a snapshot is never replayed, so it is not one |
| `checkpoints().unreachable` | the same, for `log` | the same |
| `unkept` → `at_stake` | what would removing this destroy? | the agent's own work, which a snapshot is not |

An earlier version of this table said `unkept` **counted** them and that the
three "did not all want the same fix". Neither was true: `unkept` carries the
same `--exclude`, and the PR's own test asserts the count does not move. What
differs is not the guard but the **message**. `rm` asks a separate question —
`Shadow::turns`, which counts the ref directly — and names what it finds
without refusing over it.

That split is the judgement worth recording. Refusing would fire for nearly
every session that ever ran, and a guard that fires almost always is answered
with `--force` unread — at which point it has stopped protecting the agent's
own commits, which is what it was built for.

**What the exclusion costs, stated rather than discovered.** The gitdir is a
read-write mount, so the agent can write `refs/omh/*` itself. Before this,
doing so only made `preflight` noisier; an exclusion turns it into somewhere
quiet. One fixed ref name bounds that to a single place, and `omh sNN log
--turns` prints what is in it — the hiding place is also the display case.
`--exclude` is a `rev-list` option and must sit after the subcommand and before
the `--all` it applies to; it does not reach `--reflog`, which is safe here
only because git writes no reflog for a ref outside `refs/heads`,
`refs/remotes`, `refs/notes` and `HEAD`. Measured, not assumed — it is the fact
the whole design rests on, and **one config line away from being false**:
`core.logAllRefUpdates = always` makes git log every ref under `refs/`, at
which point `unkept` sees the snapshots through the reflog arm the exclusion
cannot reach, and `rm` refuses for the rest of the session. The sandbox's
config is rewritten each launch and pins that key, so an agent that sets it
gets it back — see [risks](risks.md) 5b.

## Reading the sandbox from the host is a new trust boundary

`log` and `diff` are the **first host-side reads of an agent-writable gitdir**.
`shadow.rs` predicted this exactly: *"Today nothing host-side reads an existing
shadow… That is a property of the call sites, not of anything enforcing it."*
These commands end that property, so the neutralised set grows **before** they
ship:

- **Landed (#51): `--no-textconv`, `--no-ext-diff`, and `diff.external=`.** The
  diff machinery is configurable to *execute programs*, and both halves of that
  configuration are the agent's: the driver is a config key in a read-write
  mount, and the `.gitattributes` naming it sits in `/work`. So a host-side read
  that produces a diff runs what the agent chose, as you, outside the container.

  Which half is live was measured rather than assumed, and this page had it
  wrong: **textconv is the vector, not `diff.external`.**

  | read | textconv | external diff |
  |---|---|---|
  | `log -S` | **runs** | no |
  | `log -p`, `show` | **runs** | no |
  | `git diff` | **runs** | **runs** |
  | `log --grep` | no | no |

  The log family will not run an external driver without `--ext-diff`, which
  nothing passes — so the key alone would have left the real hole open.
  `--no-textconv` closes it, and it is a per-command flag rather than a config
  key, so the helpers add it by verb. Measured against git 2.55.0 with a driver
  the sandbox named, which ran on the host until this landed.
- **Landed (#51, extended in #52): sanitise what is printed.** `out::untrusted`
  strips C0/C1 except newline. An escape sequence in agent-authored text
  repaints omh's own output, and `committed to main` is four words.

  Which text needs it was measured rather than guessed. git **quotes a path**
  by default — `core.quotePath` renders an escape as a literal `\033` — so a
  *path* is safe wherever it appears. git **does not quote a subject**:
  `log --oneline` hands one back with its bytes intact. So the sites are the
  four that carry git's stderr into an error, and the two in `refuse_carried`
  that name the commit they found — which is the message saying omh refused to
  publish a secret, and therefore the one worth forging.

  That reasoning once ended "so the `--stat` summary is already safe", which
  held only while a summary was nothing but paths. `log` (#54) prints subjects
  and `diff <n>` (#55) prints a whole commit header — author, subject, body,
  none of it quoted — so both sanitise at the render boundary. The rule was
  right and the inventory of what a summary contains was what went stale.
- **Name the gitdir explicitly and inherit no `GIT_*`.** Host-side calls rely on
  `current_dir` today, so an ambient `GIT_DIR` — omh run from inside a hook, or
  from `rebase --exec` — redirects them wholesale.

## What this refuses to do

- **No history in the sandbox.** The seed keeps exactly one parentless commit.
  What the agent gains instead is a sentence in the arrangement saying so, so it
  stops concluding the repository is broken and reaching for `fetch
  --unshallow`.
- **No remotes, no fetch, no push** from the sandbox. Unchanged, including the
  honest position that the hook is a signpost and not a wall.
- **No omh verb for anything you could run on the host.** No `omh sNN rebase`,
  `cherry-pick`, `stash` or `blame`. The moment you want real git you are on the
  host with real git; omh's job is to make that always possible and never
  dangerous.
- **No merge UI.** Conflicts are markers in files. The agent fixes them, or you
  do, with your own tools.
- **No rewriting the agent's history to hide a secret.** Unchanged.
- **No batch actions across sessions.** You look at all of them; you act on one.

Considered and not proposed, with the reason:

- **Moving work between sessions** (`omh s01 take --into s03`). Real workflow,
  and omh is uniquely placed for it — but it is a second way to land work, and
  one landing path is why `-m` and `--keep` are mutually exclusive today.
- **Forking a session from another.** Cheap and useful for parallel
  exploration, but it is a *launch* flag rather than a git feature.
- **Comparing two sessions** (`omh s01 diff --against s03`). The
  parallel-exploration payoff and the one worth revisiting soonest. Held back
  because `diff` already grows three ways here.

## Invariants

Written the way the suite should assert them.

1. The sandbox's repository holds no commit from your checkout — including
   after a sync.
2. Nothing read from the sandbox is trusted: not identity, not config, not diff
   drivers, not bytes rendered to your terminal.
3. Your branch is untouched unless the work is already fetched into your
   repository.
4. Nothing nobody has reviewed is destroyed without being named first — extended
   from branch commits to the sandbox's checkpoints.
5. Every host-side git call names its gitdir and worktree explicitly and
   inherits no `GIT_*`.
6. No omh output names omh's own plumbing.

## Foundations

Defects measured on 2026-08-21 against git 2.55.0. The first four gate the
design; each lands with its red test first, and the ones that have landed say
so rather than being deleted — the evidence is what justifies the order. Between them and
the hardening in step 6 of the order below, this work closes or narrows
[risks](risks.md) 2, 2b, 2c, 4b, 4c, 8b, 8c and 9.

| | evidence |
|---|---|
| **Landed in #46.** `rm` deleted a branch holding unreviewed commits. `Session::commits` reads a git *failure* as `0`, and `remove` turns `0` into `branch -D`. A base that resolves only as `origin/<name>` — `default_branch` verifies no local ref — makes `rev-list` fail. | `removed session s01; branch omh/s01 dropped (no commits)`, with one commit on it. Contradicts the invariant [commands.md](../commands.md) states outright. |
| **Landed.** `worktree add -b` was silently overridden when the base existed only on the remote: git's DWIM won and checked out trunk itself. Every review path then refused, and `omh s` reported a branch that was never created. | `git worktree add -b omh/s02 ../wt2 main` → `Preparing worktree (new branch 'main')`. `--no-guess-remote` does not help; resolving the base to a commit first does. One cause, two symptoms: with only a remote-tracking *ref* and no remote configured, the same base instead fails outright with `invalid reference: main`. |
| **Landed.** The carried-secret scan over messages read a pattern, not a literal — and which pattern language depends on the reader's `grep.patternType`. | Measured across all three settings: `*` in a secret is missed by every one, `+` by `extended` and `perl` (the `basic` default finds it), and `[` is fatal everywhere, taking `--keep` down for the session. `-F` matches the bytes regardless. The first measurement of this used a `+` and was taken on a machine configured `perl` — the defect is real, the example was the author's dotfiles. |
| **Landed (#50).** `--keep` was not idempotent — the replay point is the fix. | The second run listed commits already on the branch in the todo, then `Could not apply 8eac520`. |
| **`rebase -i` without a terminal** keeps everything and reports a curation that never happened. Fixed by `--keep [selection]`; the tty guard moves to `--edit`. | With stdin from `/dev/null` the rebase runs the unedited todo, exits 0, and omh would report `kept 2`. |
| **Ambient `GIT_*` redirects host-side calls.** They rely on `current_dir` alone; `shadow`'s own helper is safe because it passes both paths explicitly. | Argued, not measured. |

## Order

1. **Landed (#46).** `commits()` returns a `Result`; nothing drops a branch on
   an unanswered question.
2. **Landed (#47).** Resolve the base to a commit before `worktree add`, and
   assert HEAD after.
3. **Landed (#48).** `-F` on the carried-secret message scan.
4. **Landed (#49).** The sandbox's exclude list is rewritten on every launch
   rather than at the first one — [risks](risks.md#security) 4c.
5. **Landed (#50).** The replay point, and `--keep` becomes repeatable.
6. **Landed (#51).** The neutralised set grows and agent-authored text is
   sanitised at the render boundary — the gate on 7.
6b. **Landed (#52).** The sandbox's `config` rewritten each launch and mounted
   read-only, which narrows [risks](risks.md#security) 2b. Split from 6: that
   one is about what a host-side *read* may execute, this about what the sandbox
   may *write*. Verified inside a real container, which the host stand-in got
   right for the wrong reason — `Resource busy`, not `Read-only file system`.
7. **Landed.** `omh sNN log` (7a, #54) — the sandbox's commits, numbered from
   the oldest, with the line where the next harvest starts — and `diff` grown
   a `-p` and a checkpoint argument (7b, #55).
8. **Landed (#53).** The selector: `sNN` in, three spellings out.
9. **Landed (#56).** `--keep [selection]`, and `--edit` for the todo. A
   selection is a `cherry-pick` rather than the generated rebase todo this
   design called for — see the section above for what was measured and why it
   changed.
10. **Landed (#60, #61).** `omh sNN sync` and the conflict-marker guard on
    `commit`, then the one-shot note the next launch delivers (#61). Three of
    this section's claims were wrong and are corrected above where they were
    made: the replay point must *not* advance, the conflict labels are
    readable, and `diff --check` needed two measurements rather than the
    "nothing here needs a parser" it was given.
11. **Landed (#57, #60).** `omh doctor` learns git: the host's version, whether
    it can take a `--keep` selection, and — since `sync` now ships — whether it
    can merge on the host, asked of the binary rather than compared against a
    version omh cannot name. The two capabilities are reported apart because
    their version floors are three minor versions apart. Neither turns the
    check red: a doctor that fails over a capability nothing calls is one
    people learn to ignore. Still open: that a read-only `config` really does
    refuse inside a real container.
12. **Landed (#58, #59, #62).** `rm` refuses over work no branch has —
    [risks](risks.md#security) 2c, closed. `omh s` names files two sessions are
    both changing, and reports sandbox repositories with no session left —
    [risks](risks.md) 8c. The arrangement gained its two sentences. The
    dashboard learned staleness in #62 — `behind N` had been a number with
    nothing to do about it since the command existed, and `sync` is the thing
    to do. `omh s01` alone became that row in #67, which retired the `ls`
    verb to a tombstone that refuses rather than deleting it — see step 8.

Steps 1–6 are repair and hardening, and can land in any order. Steps 7–12 are
the loop, and each is useful on the day it ships.

Step 6 is worth one more note. Read-only `config` is not belt-and-braces beside
the neutralised flags: the flags are a list someone must remember at every new
call site, and the mount is a property of the container. Measured 2026-08-21 —
`commit`, `checkout -b`, `stash`, `reset --hard`, `rebase` and `gc` never write
that file, and when it cannot be replaced `git config` and `git remote add` fail
with nothing half-written. It costs one staged file and one mount, the same
pattern `GUEST_PRE_PUSH` already uses.

It does not retire that hook, and an earlier draft of this said it did.
Measured 2026-08-21: `git push <url> <ref>` needs no remote in config, the hook
fires on it exactly as before, and the remote stays empty. What changes is the
route git's own error message walks the agent down — `git remote add` now fails
with `could not write config file … Read-only file system`, so on that route the
agent meets a filesystem error rather than omh's sentence. The protection there
is stronger and the explanation is gone, which is an argument for one more
clause in the arrangement — adding a remote will not work either, and that is
the arrangement rather than a fault to repair — not an argument against the
mount.

**Not the way to fix that: an `.invalid` remote.** Configuring one so the agent
meets a refusal sooner does the opposite, because `pre-push` runs *after* git
checks remote status. Measured 2026-08-21: against an unreachable remote the
push dies on `Could not resolve host` and the hook never runs, while the same
hook fires normally against a reachable one. It also puts `origin/<branch>
[gone]` and *"the upstream is gone (use `git branch --unset-upstream` to
fixup)"* into every `git status`, and turns "attempts nothing" into "attempts a
DNS lookup", which is not nothing on a network whose resolver hijacks NXDOMAIN.
Worth keeping only as a note that a hostname is a delivery surface — the error
quotes the URL verbatim — should a remote ever have to exist for another
reason.

## Not decided

- ~~**Per-turn checkpoints.**~~ Landed (#66) — see the section above.
- **Naming sessions.** `omh s01 push <name>` refuses to invent a branch name,
  correctly. A session that carried a name from the start would answer that once
  instead of at the end — but `omh new claude` asking a question at launch would
  cost the thing `init` exists to protect.

## What this changes in [Decisions](decisions.md), when it lands

| Decision | Becomes |
|---|---|
| Getting work back | `omh sNN commit`, squash or `--keep` — repeatable, and curated by selection rather than by a rebase todo |
| Repo exposure | unchanged; the worktree stops being a *user-facing* noun |
| — | **Naming a session**: `sNN` first, one form, because several sandboxes of one repo are reached from one place |
| — | **Staying current**: `omh sNN sync` merges on the host and delivers files, because no commit of yours may enter the sandbox |
