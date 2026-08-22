# Git

**Designed and not built.** The worktree model, the sandbox's own repository and
`omh s commit --keep` ship today and are described in
[Sessions](../sessions.md). This page is the design for what goes *around* them:
the loop a developer works in, and the way several sandboxes of one repo are
reached from one place. None of it exists yet.

It also rests on defects in what does exist. Those are in
[Foundations](#foundations), each measured against git 2.55.0 on 2026-08-21, and
they land first.

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
| **read the change** before landing it | `omh s diff` prints `--stat` only. The patch is in a worktree the docs tell you never to enter. |
| **know what the agent has been doing** | Its commits are invisible until `--keep` opens a `rebase -i` todo. `s ls` does not count them; `s rm` destroys them without saying they existed. |
| **land work in stages** | `--keep` replays from the seed every time. Measured: a second run re-lists commits already on the branch, then dies on `Could not apply`. |
| **not fall behind trunk** | `s ls` reports `behind 12` and offers nothing. The session works against stale code until it is abandoned. |

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

Overlap costs nothing to compute. `s ls` already runs `status --porcelain -uall`
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
$ omh s01 claude      launch into it
```

`s` is the sessions namespace scoped to the current session; `sNN` is the same
namespace scoped to that one. The desugaring is literal:

```
omh s01 log      ≡  omh sessions --session s01 log
omh s01 commit   ≡  omh sessions --session s01 commit
```

One extension, and it is the only place the equivalence is not a pure alias:
when what follows is not a session verb, the prefix still sets the session and
the command runs where it lives — `omh s01 claude`, `omh s01 attach zed`,
`omh s01 graph`. Same rule, and it covers the launch, which `sessions` has no
verb for.

**The value is the deletions.** All four of these work today and mean the same
thing:

```console
$ omh s diff              # current
$ omh s diff s01          # positional
$ omh s -s s01 diff       # flag after the namespace
$ omh -s s01 s diff       # flag before it
```

Four spellings, two mechanisms, applied unevenly — `rm` requires the positional,
`commit` and `push` ignore it, and `push` cannot have it because that slot is
the branch name. So this is one form and three removals: the positional session
leaves `s diff`, `s down` and `graph`; the required positional leaves `s rm`;
`--session` stays, because it is what the prefix desugars to and the only way to
name an id that is not `sNN`.

Two rules make the parse unambiguous:

- a leading token matching `s\d+` is lifted into the session, and the rest is
  parsed exactly as it is today
- a session id may not be named like an omh command. `RESERVED` and the test
  that no bundled definition shadows a command already exist; `validate_id`
  joins them, or `--session diff` creates a session you can never address

`omh s01 ls` is the one nonsense combination. It errors — *"`ls` lists every
session; drop the `s01`"* — rather than ignoring the scope.

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

`omh s ls` survives as an alias. `omh s01` on its own is the same row with
detail and the commands worth running next.

### `omh sNN log` — make the invisible visible

Reads the shadow gitdir on the host. This is the command that changes how a
session feels: today you cannot tell that the agent has been committing at all.

```
s01 · 4 checkpoints, 2 not yours yet · 2 behind main

  4  12m   Extract the tap guard into its own function     3 files  +48 −12
  3  38m   Add the failing test first                      1 file   +23
  ─────────────── yours from here ───────────────────────────────────────
  2   1h   Fix typo                                        1 file   +1 −1
  1   1h   Rename shadow → sandbox repo                   12 files  +90 −90

  uncommitted in the session: 2 files  +11 −3

  omh s01 diff 4          read one
  omh s01 commit --keep   bring the 2 new ones onto the branch
```

Numbered, so there is no object id to copy and no ref to name. The uncommitted
line matters as much as the checkpoints: it is the work `--keep` would sweep
into "Work in progress", shown before that happens rather than after.

### `omh sNN diff` — a real patch, and one checkpoint

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

### `omh sNN commit --keep [selection]`

The flagship feature currently requires knowing `pick`/`squash`/`drop`, that
reordering can conflict, and how to abort a rebase. Under the layering rule that
is backwards. `log` numbers the checkpoints, and you name what you want:

```console
$ omh s01 commit --keep          # all of them, no editor
$ omh s01 commit --keep 1,3-4    # these, in this order
$ omh s01 commit --keep --edit   # the todo, for people who want it
```

Underneath it is still a rebase, with the todo generated by omh and delivered
through `GIT_SEQUENCE_EDITOR` pointed at omh's own binary — not at `cp`, because
that value goes through `sh -c` and a profile path with a space in it would turn
a curation into a syntax error.

`--edit` is then the only path that needs a terminal, which is where the tty
guard goes. That closes a measured hole: with stdin not a terminal, `rebase -i`
proceeds on the unedited todo, exits 0, and omh reports a curation that never
happened.

### `omh sNN sync` — trunk moves, and nobody's work is at risk

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
   `<sha>`"*, the sha as text and never as an object — and advance the replay
   point past it, so the harvest never replays trunk's changes as the agent's.
   Cleanly merged paths only; the conflicted ones stay uncommitted, for the
   reason below.
7. **Guard the exit.** `commit` refuses while `git diff --check` reports
   leftover conflict markers, naming the files. Measured: `--check` recognises
   exactly the markers `merge-tree` writes, so nothing here needs a parser.

The payoff is worth being loud about: **a conflict is text, and text crosses the
boundary safely.** The agent resolves trunk conflicts with the whole tree in
front of it, in a repository where it cannot hurt you. That is a capability the
isolation *creates* rather than costs.

Two honest rough edges. `merge-tree` labels conflict hunks with the object ids
it was handed — `<<<<<<< 4822532db…`, measured — which reads badly for whoever
fixes it; relabelling needs measuring rather than asserting. And it wants git
≥ 2.38; a scratch-worktree merge is the fallback and `omh doctor` is where the
version check belongs.

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

## Reading the sandbox from the host is a new trust boundary

`log` and `diff` are the **first host-side reads of an agent-writable gitdir**.
`shadow.rs` predicted this exactly: *"Today nothing host-side reads an existing
shadow… That is a property of the call sites, not of anything enforcing it."*
These commands end that property, so the neutralised set grows **before** they
ship:

- **`diff.external=`, `--no-ext-diff`, `--no-textconv`.** Rendering a patch runs
  the diff machinery, and the diff machinery is configurable to *execute
  programs*. The shadow's config is in a read-write mount. Without this,
  `omh sNN diff 4` is arbitrary code execution on the host, as you, from a
  config line the agent wrote.
- **Sanitise what is printed.** Checkpoint subjects are agent-authored text
  rendered inside omh's own report frames. Control characters and ANSI escapes
  let a subject line forge omh's output, and `committed to main` is four words.
  Strip C0/C1 except newline at the render boundary, once, for every
  agent-authored string.
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
| **Landed.** `worktree add -b` was silently overridden when the base existed only on the remote: git's DWIM won and checked out trunk itself. Every review path then refused, and `s ls` reported a branch that was never created. | `git worktree add -b omh/s02 ../wt2 main` → `Preparing worktree (new branch 'main')`. `--no-guess-remote` does not help; resolving the base to a commit first does. One cause, two symptoms: with only a remote-tracking *ref* and no remote configured, the same base instead fails outright with `invalid reference: main`. |
| **Landed.** The carried-secret scan over messages was a regex, not a literal — `--grep` without `-F` is a POSIX BRE. | `KEY=ab+cd/ef12345==` in a commit message is not matched; `-F` matches it. A carried line containing `[` makes `git log` exit 128 and `--keep` fail permanently for that session. |
| **`--keep` is not idempotent.** The replay point above is the fix. | The second run lists commits already on the branch in the todo, then `Could not apply 8eac520`. |
| **`rebase -i` without a terminal** keeps everything and reports a curation that never happened. Fixed by `--keep [selection]`; the tty guard moves to `--edit`. | With stdin from `/dev/null` the rebase runs the unedited todo, exits 0, and omh would report `kept 2`. |
| **Ambient `GIT_*` redirects host-side calls.** They rely on `current_dir` alone; `shadow`'s own helper is safe because it passes both paths explicitly. | Argued, not measured. |

## Order

1. `commits()` returns a `Result`; nothing drops a branch on an unanswered
   question.
2. Resolve the base to a commit before `worktree add`, and assert HEAD after.
3. `-F` on the carried-secret message scan.
4. The sandbox's exclude list is rewritten on every launch rather than at the
   first one — [risks](risks.md#security) 4c, and the cheapest item here.
5. The replay point, and `--keep` becomes repeatable.
6. The neutralised set grows, agent-authored text is sanitised at the render
   boundary, and the sandbox's `config` is mounted read-only — the gate on 7,
   and what narrows [risks](risks.md#security) 2b.
7. `omh sNN log`, and `diff` grows `-p` and a checkpoint argument.
8. The selector: `sNN` in, three spellings out.
9. `--keep [selection]`, and `--edit` for the todo.
10. `omh sNN sync`, the conflict-marker guard on `commit`, and the one-shot
    note the next launch delivers.
11. `omh doctor` learns git: the version, `merge-tree --write-tree`, and that a
    read-only `config` really does refuse inside a real container.
12. The dashboard learns work, staleness and overlap; `rm` learns to refuse;
    `s ls` reports orphaned sandbox repositories; the arrangement gains its two
    sentences — no history here, and trunk may move under you.

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

- **Per-turn checkpoints.** A `turn-end` hook committing the tree onto a side
  ref — never the branch, so curation stays clean — would give `log` a timeline
  even for agents that never commit, and give the agent a `reset --hard` target
  for every turn. It costs a base-set entry and a commit per turn, so it is a
  [base set](base-set.md) decision rather than a git one.
- **Naming sessions.** `omh s01 push <name>` refuses to invent a branch name,
  correctly. A session that carried a name from the start would answer that once
  instead of at the end — but `omh claude` asking a question at launch would
  cost the thing `init` exists to protect.

## What this changes in [Decisions](decisions.md), when it lands

| Decision | Becomes |
|---|---|
| Getting work back | `omh sNN commit`, squash or `--keep` — repeatable, and curated by selection rather than by a rebase todo |
| Repo exposure | unchanged; the worktree stops being a *user-facing* noun |
| — | **Naming a session**: `sNN` first, one form, because several sandboxes of one repo are reached from one place |
| — | **Staying current**: `omh sNN sync` merges on the host and delivers files, because no commit of yours may enter the sandbox |
