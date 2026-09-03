# Commands

```
omh init                          set this repo up
omh new <harness> [-- args…]      start a session, run an agent in it
omh s01 resume [harness]          rejoin one · claude · omp · opencode
omh graph [--stop]                browse the code graph in a browser
omh auth <harness> [-n <acct>]    log in once; repeat for several accounts
omh doctor [--harness <name>]     verify a harness sees your profile · d
omh why <thing>                   who put this here, and on what grounds
omh info                          harnesses, editors, sessions, your catalogue
omh sessions [attach|resume|log|diff|commit|push|sync|down|rm] …
                                  omh s · omh s01 log
omh settings [set|unset|edit|mcp] …
                                  you: your defaults
omh info --repo                   this checkout: what it uses, and why
omh use|unuse <capability> <name>
                                  omh use skills tdd · omh use --all
omh import <capability> <harness>  bring a setup you already have into omh
omh eject <harness> --to <dir>    write out the raw config and step aside
```

## `--dry-run`

**Everything runs; nothing is written.** Not a description of what would happen
— the real code path, with persistence withheld. So the layers a write would
reach, the list a selection would become and the refusals a bad name would earn
all happen exactly as they would for real, and the only difference is that the
file is left alone.

A command that cannot yet answer it **refuses the flag** rather than running.
`init` builds images, and the session verbs stop containers, replant commits
and delete worktrees; each has to compute what it *would* do, and a preview
that guessed would be worse than none. Read-only commands refuse it too, for
the opposite reason: `omh info` is its own dry run.

That rule exists because the flag used to be accepted and discarded —
`omh --dry-run use --all` wrote the file and printed `wrote →`.

## The shape of the CLI

Noun-verb groups with single-letter aliases: `omh s log`, `omh s01 commit`,
`omh d claude`. The noun on its own is the listing — `omh s` is every session,
and `omh s01` is that one.

**Every command is a named command.** A bare name used to be a harness —
the bare name launched — which meant any word could be a launch and so no word
could safely be a command. A `RESERVED` list of nineteen names existed to keep
them apart, and the selector had to parse each line twice to work out which
reading was meant.

`omh new claude` costs one word and removes the class. There is no bare slot to
shadow, so the list and the double parse are both gone. Harnesses and editors
still share a namespace, but only inside their own verbs — `omh new zed` and
`omh s attach zed` say which they mean.

### What every command prints

Two audiences, one answer. Every command builds a value and renders it either
for a person or for a program, so the two cannot disagree about what happened.

| | |
|---|---|
| `--json` | The answer as JSON, with stable field names. |
| `--color auto\|always\|never` | Defaults to `auto`: styled on a terminal, plain in a pipe. |
| `NO_COLOR` | Set and non-empty, it turns `auto` plain. |

`NO_COLOR` speaks for a user who has not said otherwise, so `--color always`
still wins over it — that is you, on this one run, with your own hands. An
**empty** `NO_COLOR` counts as unset, per [no-color.org](https://no-color.org):
presence alone is not the trigger, and an empty value is what a shell leaves
behind when a wrapper script unsets a variable badly.

**stdout is the answer; stderr is everything else.** What a command was asked
for goes to stdout, so `omh s > sessions.txt` captures exactly that.
Warnings, progress and next-step hints go to stderr, so they still reach you
when stdout is redirected — and stay out of the file.

`--json` never emits colour, whatever `--color` says, because a script setting
`--color always` in a wrapper should not break `jq`. It also suppresses hints,
which are prose for a person.

**JSON carries the fact, not the sentence.** The human column is English for a
reader; the JSON field is the thing a script would otherwise have to parse it
back out of:

```console
$ omh s
  s01  omh/s01  stopped  ?  (1 behind main)

$ omh s --json
{
  "base": "main",
  "leftovers": [],
  "overlaps": [],
  "sessions": [
    {
      "behind": 1,
      "id": "s01",
      "label": "omh/s01",
      "running": false,
      "running_unknown": null,
      "work": {
        "state": "unknown"
      }
    }
  ],
  "unreadable": []
}
```

`work.state` is one of `clean`, `uncommitted`, `unpushed`, `published` or
`unknown` — `uncommitted` and `unpushed` carry a `count`, `published` carries
the `branch`. The one that earns the enum is **`unknown`**: it means omh could
not tell, which is a different answer from `clean` and the one most dangerous to
confuse with it. The human column prints `?` for it, and the string version of
this API printed `""` for both.

`work` is `null` — not `unknown` — where nobody asked, so a caller can tell an
unanswered question from an unanswerable one.

Both flags are omh's own, so `omh new claude --json` is **omh's** — everything
before the `--` is. See [where omh's flags end and the harness's
begin](#where-omhs-flags-end-and-the-harnesss-begin). Use `omh new claude -- --json`
to hand it to the harness instead.

---

## `omh init`

Sets up the current repo. Decides everything, asks nothing, reports all of it.
Covered in [Getting started](getting-started.md#what-it-actually-did).

Safe to re-run: it never overwrites files you have edited.

## `omh new <harness> [-- args…]`

Starts a session and runs a harness in it. Arguments for the harness go after
`--`; everything before it is omh's.

```console
$ omh new claude
$ omh new opencode
$ omh new claude -- --resume x
```

`new` always starts a session that does not exist yet. To go back into one you
already have, `omh sNN resume` rejoins it as the harness it ran, and
`omh sNN resume <harness>` rejoins it as a different one.

A bare harness name used to do both — start or reattach, depending on what was
there. It is not a command any more: any word could be a harness, so no word
could be a command, and `RESERVED` existed to keep nineteen names out of the
way. Naming the verb costs one word and removes the whole class.

If the harness cannot express part of your profile, that is announced once at
launch rather than silently dropped:

```console
$ omh new opencode
omh: opencode on omh/s01 — dropped hooks: git-note (no `session-start` moment),
     graph-first (no `search` tool),
     graph-orient (no `session-start` moment),
     graph-read (no way to inject text before a tool runs)
```

Which credential set a launch uses is `omh set account <name>`, per project. See
[Accounts](accounts.md).

## `omh s attach [editor]` · `omh s a`

Opens the session in an editor over SSH. With no argument it resolves
`$OMH_EDITOR` then `$EDITOR`, and falls back to printing every recipe:

```console
$ omh s attach
session s01 is up

  ssh://omh-ohmyharness-3f9a2c1b-s01/work
  ssh omh-ohmyharness-3f9a2c1b-s01

  code    code --remote ssh-remote+omh-ohmyharness-3f9a2c1b-s01 /work
  cursor  cursor --remote ssh-remote+omh-ohmyharness-3f9a2c1b-s01 /work
  nvim    ssh -t omh-ohmyharness-3f9a2c1b-s01 cd /work && nvim
  zed     zed ssh://omh-ohmyharness-3f9a2c1b-s01/work
```

One row per editor omh has, keyed by the name it knows it under —
`omh info` lists them. JetBrains Gateway reaches the same host through the
`ssh` alias above; it is not an editor omh ships a recipe for.

An editor that is not installed is **not an error** — omh says so and prints the
URL. See [Editors](editors.md).

## `omh graph [--stop]`

Serves the code graph for this repo in a browser.

```console
$ omh graph
omh: graph at http://127.0.0.1:56286
  every session's graph for this repo, in one place
  stop with: omh graph --stop
```

One service per repo, not per session, and it needs no session to exist. See
[Code graph](code-graph.md#omh-graph).

## `omh auth <harness> [--name <account>]`

Runs the harness's own login and captures the result. `--name` defaults to
`default`.

```console
$ omh auth claude --name personal
$ omh auth claude --name work
```

See [Accounts](accounts.md).

## `omh doctor [--harness <name>]` · `d`

Launches the real image with the real mounts and checks the guest paths the
adapter claims. The only thing that can verify an adapter. See
[Troubleshooting](troubleshooting.md).

**It gathers the host first, and reports it whatever happens next.** Seven
rows, computed before any container work: whether the container runtime is
**answering** (not merely installed — a stopped Docker Desktop is on `PATH` and
useless), the stacks it detected and the marker file that decided each,
settings set here that omh does not read, what omh has left behind, which omh
set this checkout up, disk free where omh keeps its state, and the host's git.
An eighth appears only when this repo has no commit for a session branch to
fork from.

They print in the same table as the adapter rows, above them. Gathering them
first is what matters: on a machine with no runtime, or one where the image
cannot be built, they are the whole of what omh can tell you. They are computed without a sandbox on purpose — on a machine with no
runtime, or one where the image cannot be built, they are the whole of what
omh can tell you, and they used to be thrown away with the failure.

The git row is the version, and whether it can take a `--keep` selection, asked
of the binary rather than compared against a version number. Only a git omh
cannot use at all fails: an older git that cannot name checkpoints is still a
working git, and a `doctor` that goes red over something you never run is one
you stop running. The same reasoning holds for the stacks row — a repo of prose
has no stack and that is not a failure.

A detected stack whose toolchain will not be installed says so, and says which
of the two reasons it is: `[provision]` switched it off, or it has not been
provisioned yet. Both install nothing; only one of them is a decision you made.
That is the answer to "why is there no python in my sandbox".

## `omh why <thing>`

Who put this here, and on what grounds. Needs no session and no container.

```console
$ omh why codegraph
codegraph — omh's choice, in the base set since 2026.06

  because     structural queries instead of re-grepping the repo every task
  part of     codegraph
  brings      graph-orient, graph-first, graph-read, graph-refresh, graph-rules
  costs       0.46s to index this repo, cold   measured 2026-08-06
              index_repository --mode fast, 821 nodes / 3813 edges, in the sandbox
              3.4 MB on disk                   measured 2026-08-06
              the graph volume after a cold index of this repo
  instead of  gitnexus            PolyForm-Noncommercial licence
              codegraphcontext    needs a Neo4j service running
              @sdsrs/code-graph   close second; needs a node runtime rather than a static binary
  installed   this repo
  remove      omh set codegraph off — the feature, its server and its hooks
              together. Nothing is uninstalled and the next repo gets it back

  answered from ~/.omh/base/2026.08.toml · 2026.08
```

**Everything omh ships belongs to a feature**, and removal follows the feature:
`omh set codegraph off` takes the server and its four hooks together.
`omh why graph-first` answers "part of codegraph" from the other direction, and
says `(off here)` when this repo has switched the feature off in
`.omh/settings.toml`.

**Cost is measured; benefit is argued.** Every cost carries the date it was
taken *and how*, and one predating the current base-set version is marked stale
— so re-cutting the base set puts every carried-over number up for
re-affirmation. The `because` line is a judgment you are free to disagree with —
see
[measure the cost, argue the benefit](design/trust.md#measure-the-cost-argue-the-benefit).

### Seven answers, because authorship differs

| | |
|---|---|
| **omh's choice** | in the base set, and your copy matches what omh ships |
| **omh's own, generated at launch** | a hook or a rules section. Not a file anywhere — omh writes it into the session and nothing else, which is what lets a fix reach you with the upgrade. A leftover of the same name in your profile is named as no longer read |
| **not what omh ships now** | omh's entry, and your copy differs. Both values shown, and **no claim about who changed it** — omh cannot tell an edit from an upgrade |
| **not installed here** | in the base set, absent from your profile. Not an error |
| **written by omh init** | derived from your repo, like `rust-format` from `Cargo.toml`. omh's writing, not omh's opinion |
| **your choice** | you added it. **No rationale is offered** — omh does not have one and will not invent one |
| **considered, not in the base set** | rejected, with the reasoning. `omh why gitnexus` explains the licence problem |

That last row is why rejections are recorded at all: without them the same
candidate gets re-litigated every time somebody rediscovers it.

A name matching nothing prints what *is* known rather than guessing — the same
rule as `omh s attach emacs`.

## `omh info`

Everything omh knows about here: harnesses, editors, which sessions exist, and
what your catalogue holds per capability. What each session is *doing* is
`omh s` — this one never asks git, so it costs no subprocess per session.

`--repo` narrows the same question from the machine to this checkout; the two
are one command because *what you have* is one question asked at two scopes.

## `omh sessions …` · `s`

```
omh s                 sessions, their branches and state
omh s01               that one, with what to run next
omh s log [--turns]   what the agent committed inside the sandbox, or what omh
                       photographed at the end of each turn
omh s diff [n] [-p]   what changed — the session, or one checkpoint
omh s commit [-m …]   commit that work onto the session branch
omh s commit --keep [n,m-o] [--edit]   keep the agent's own commits
omh s sync [--down]   bring trunk into the session, merged on the host
omh s push [name]     push it to origin under a name a reviewer can read
omh s resume          rejoin it, running the harness it ran before
omh s down            stop the container, keep the worktree and branch
omh s rm [--force]    remove the session — its container, its worktree, its staging,
                       and the repository the sandbox had. Asks first over work
                       no branch has; `--force` is for runs with nobody to ask,
                       and does not make the removal itself try harder.
```

**The noun on its own is the listing, and a session on its own is one row of
it.**

```console
$ omh s
  s01  omh/s01  stopped  2 uncommitted
  s02  omh/s02  stopped  2 uncommitted

  s01 and s02 both change shared.rs

$ omh s01
  s01  omh/s01  stopped  2 uncommitted

  s01 and s02 both change shared.rs
```

The collision survives the focus, because it is a fact about s01 — a collision
between two *other* sessions does not follow you in. Every session is still
read either way; that is what makes the line sayable at all.

There is no `ls` verb. It was one until 2026.08, and what made this row
possible is the listing learning a scope; retiring the verb is the smaller and
separate call, so that one thing has one spelling rather than two.

Typing it still says so. The verb is kept as a tombstone because clap's
*unrecognized subcommand* does not name what replaced it, and this does. It was
once kept for a stronger reason — the line used to fall through to a top-level
`ls` and quietly list every session — but that spelling is gone, so the line no
longer parses either way.

**The session goes first, and everything after it is what you would have typed
anyway.**

```console
$ omh s diff          # the session you were last in
$ omh s01 diff        # that one
$ omh s01 commit -m "Fix the tap guard"
$ omh s01 rm
$ omh s01 resume      # rejoin it
$ omh s01 attach zed
```

`s` is the sessions namespace scoped to the session you were last in; `sNN` is
the same namespace scoped to that one, so `omh s01 diff` is exactly
`omh sessions --session s01 diff`. When what follows is not a session verb the
prefix still names the session and the command runs where it lives — which is
what covers a launch, since `sessions` has no verb for starting a harness.

`--session` still works, and is the only way to name a session whose id is not
`sNN`. Naming it twice is refused rather than resolved.

`down` with no session stops every sandbox — the one place acting on all of
them is what you mean, and the one command whose blast radius grows with how
much work is in flight. So it asks first, and `--all` is how you say it without
being asked. Silence declines, and a pipe or a CI runner has nobody to answer,
so a wide `down` there stops nothing rather than everything.

**A session that has fallen behind is told what to do about it.** The number
was there long before there was anything to do with it; `sync` is that thing.

```console
$ omh s
  s01  omh/s01  up
  s02  omh/s02  stopped  3 uncommitted  (12 behind main)
  s03  omh/s03  stopped                 (how far behind main?)
omh: omh could not measure s03 against main — it may be working against code that moved, and `sync` is not offered over a count that failed
  omh s02 sync  bring main in, merged on the host
  omh s03 log   says why the count could not be taken
```

`up?` does not appear beside `up` in a real listing — every row is answered by
the same daemon in the same run, so when one row cannot be answered they all
look like this:

```console
$ omh s
omh: could not tell whether s01's sandbox is running: cannot connect to the Docker daemon
omh: could not tell whether s02's sandbox is running: cannot connect to the Docker daemon
  s01  omh/s01  up?
  s02  omh/s02  up?  3 uncommitted  (12 behind main)
```

Offered for the sessions it applies to and no others — a suggestion under every
row is one nobody reads — and in the spelling that works on the session as it
stands. `sync` refuses while a sandbox is up and names `--down` itself, so the
bare form under a row marked `up` would be a line that fails when pasted.

**And a container omh could not ask about is not a container that is stopped.**
The state column has four answers, not two:

| | |
|---|---|
| `up` | the sandbox is running — or paused, or restarting, which are the same thing for anything omh would do to it |
| `stopped` | it is not, or was never built |
| `up?` | omh asked and the runtime would not answer. The reason is on stderr |
| *(empty)* | nobody asked — no container runtime on this machine, said once above the table |

`up?` is not cosmetic. Every command that acts on a container asks this
question first, and a Docker daemon that is down used to answer *not running*
— so `omh sNN sync` believed there was nothing to stop and would have written
over the files of a live agent.

What each command does with `up?` differs, because the safe direction does:

- `sync`, `graph` and a launch **refuse**, naming what the runtime said.
- `down` leaves that session alone and says so, as a row rather than a gap —
  it exits non-zero, and reports *could not be asked* rather than *would not
  stop*, which is a claim omh cannot make about a container it never tried.
- `rm` removes the session anyway and warns that the code graph's entry for it
  was left behind.
- the idle reaper goes the other way on purpose: a session it cannot ask about
  is never stopped for being idle.

**A count omh could not take is not a count of zero.** `s03` renders `(how far
behind main?)` rather than an empty cell, because *up to date* and *omh could
not count* must never look the same on the surface where you pick which session
to open. It is not offered a sync — a merge advised off a count that failed is
advice built on a guess — but it is not passed over in silence either: beside
rows that each carry a next step, saying nothing reads as *this one is fine*.
`omh s03 log` prints the reason git gave.

`omh s` also names ids that have a container, a run directory or a **sandbox
repository** but no worktree — sessions removed by a version of omh that only
took half of one down. `omh sNN rm` clears them, and says what it would take
with it first.

And it names files more than one session is changing:

```console
$ omh s
  s01  omh/s01  up       3 uncommitted  (2 behind main)
  s03  omh/s03  stopped  1 uncommitted  (2 behind main)

  s01 and s03 both change src/base.rs, src/render.rs
```

That is the collision git will not mention until a merge, said while both
sessions are open and either could still be redirected. It costs nothing: the
paths and the uncommitted count are one `status` per session, asked once — read
separately they were two subprocesses and, worse, two snapshots of a worktree
an agent is writing into. It is part of the answer rather than a warning, so a
redirected listing keeps it.

A session omh cannot read says so. Absence from that section otherwise means
*collides with nobody*, and an empty section is how "no collisions" is
rendered — so a partial answer would be indistinguishable from a clean one.

### Where omh's flags end and the harness's begin

Before the separator a flag is omh's; after it, the harness's. Nothing is
inferred and nothing is refused for being ambiguous, because nothing is
ambiguous:

```console
$ omh --dry-run new claude       # omh's
$ omh new claude -- --resume x   # claude's
$ omh new claude -- --json       # claude's, even though omh has one too
```

This replaced a rule. The bare name had no separator, so omh had to guess:
it refused its own long flags after a harness name and left short ones alone,
on the reasoning that `-s` is a flag plenty of harnesses have and refusing it
would break launches that worked. With `--` there is nothing to guess, so
`omh new claude -- -s x` hands `-s` on and `omh new claude -s x` is omh's.

**`omh new` does not guess.** The bare name has to: `omh new claude --json` could
be meant for either, and the rule above is a judgement about which mistake is
likelier. Under `omh new` the separator decides instead — everything before
`--` is omh's, everything after it is the harness's, and there is no third
category.

```console
$ omh new claude --json          # omh's, reported as JSON
$ omh new claude -- --json       # claude's
$ omh new claude -- -s work      # claude's, short flags included
$ omh new claude --resume x      # an error: omh has no --resume
```

That last line is the trade. The bare name forwards an unknown flag on the
assumption it belongs to the harness; `omh new` refuses it, because a flag it
cannot place is likelier to be a typo than a gift. Put it after `--` and it
goes through untouched.

### `omh sNN resume` — rejoin it as what it was

`omh new` starts a session; `resume` rejoins one, running **the harness it ran
before**. omh records that at launch, beside the session's last-used marker, and
`omh sNN rm` takes it away with the rest of the run directory.

The staging directory carries the name too — `run/<repo>/<id>/<harness>/`
survives `down` — but a session that has run two harnesses has two of them and
no way to say which was last, and staging is a side effect nothing promises to
keep. The marker is the answer omh means.

Three things can happen, and they are three different sentences:

```console
$ omh s02 resume
opencode on omh/s02

$ omh s01 resume
omh: omh has no record of a harness running in s01, so it cannot rejoin as one.
  omh s01 resume <harness>   rejoin it as that
  omh s                      what is here

$ omh s03 resume
omh: omh recorded a harness for s03 and cannot read it back: …/.harness is empty
  omh s03 resume <harness>   rejoin it as that, which rewrites the record
```

The middle one covers a session older than this release, one whose run
directory was cleared, and one `omh s attach <editor>` created — attaching an
editor makes a session without running a harness in it, so there is nothing to
record. The last one is a marker omh wrote and cannot read now, which is
different news and says so.

`omh sNN resume <harness>` names one instead. If it is the one already
recorded, that is still a resume. If it is a different one it is a **switch** —
an image is built per harness, so the sandbox stops and starts on the other
one, and the record is rewritten so every later `resume` follows. omh says so
rather than leaving you to notice:

```console
$ omh s01 resume claude
omh: s01 was running omp; resuming as claude restarts its sandbox and records claude
claude on omh/s01
```

**Guessing would be easy and wrong.** omh knows which harness this host
prefers, and answering with it would attach claude to a worktree an afternoon
of opencode built, with nothing on screen to say so.

### `omh sNN log` — what the agent has committed

Newest first, and until this existed you could not tell the agent had been
committing at all: its work first became visible when `omh s commit --keep`
opened a rebase todo, which is late to find out there are eleven of them.

```console
$ omh s01 log
s01 · 4 checkpoints, 2 not yours yet · 2 behind main

  4  12m  Extract the tap guard into its own function  3 files   +48 −12
  3  38m  Add the failing test first                   1 file    +23
  ────────────────────────── yours from here ───────────────────────────
  2  1h   Fix typo                                     1 file    +1 −1
  1  1h   Rename shadow to sandbox repo                12 files  +90 −90

  uncommitted in the sandbox: 2 files
```

**Numbered from the oldest**, so a number keeps meaning the same commit as the
agent commits more — and they are what `omh s01 commit --keep` will take. Above
the line is work your branch has never seen; below it is what a previous
`--keep` already handed over.

The uncommitted line is measured in the sandbox, where `--keep` measures it:
that is the work about to be swept into a *Work in progress* commit, shown
before it happens rather than after.

Two things are said rather than shown. A merge reads `merge` instead of a file
count, because git has no diff for one until you name a parent and *0 files* is
a measurement. Files git will not count lines for — binary, or anything the
agent marked `-diff` — read `·N`, never a blank.

And when the sandbox is in a state `--keep` would refuse — commits on a branch
it wandered off, or a rewind below the last handover — the log says so on
stderr and does not offer a harvest it knows would be refused.

A session whose sandbox has never run says so and exits 0 — asking to see the
agent's work before the agent has run is an ordinary thing to do.

### `omh sNN log --turns` — what the agent did, when it never committed

Most agents never run `git commit`, so `omh s01 log` has nothing to show for a
session you can plainly see changed things. omh photographs the tree at the end
of every turn, and this reads those:

```console
$ omh s01 log --turns
s01 · 3 turns

  ~0  4m   turn end  7 files  +220 −61
  ~1  35m  turn end  1 file   +9
  ~2  1h   turn end  3 files  +48 −12
```

**These are omh's snapshots, not the agent's commits, and the two lists never
mix.** There are no numbers here for `diff` or `--keep` to take — a row is
identified by the ref that reaches it, `refs/omh/turn~0` being the newest.
That is deliberate, and it is the second attempt: numbering them `3 / 2 / 1`
meant `omh s01 commit --keep 2` quietly replanted the *agent's* checkpoint 2,
a different list, with no error. A snapshot is a photograph of a tree, not
work anybody chose to keep.

The `turn end` column is omh's own subject on every genuine snapshot, so
anything else appearing on that ref is visible as the row that reads
differently.

What they are good for is the other direction. Inside the sandbox, to get the
files back to how they stood two snapshots ago:

```console
$ git restore --source=refs/omh/turn~2 -- .
```

**Not `git reset --hard refs/omh/turn~2`.** The snapshot chain has a root of
its own — it is not built on the session's history — so resetting onto it moves
the branch into omh's commits, and `omh s01 log` then numbers *those* as the
agent's checkpoints. `omh s01 commit --keep` would replant them onto your
branch as the agent's work, which is the one thing this separation exists to
prevent. `restore` puts the files back and leaves the branch alone, which is
what was wanted anyway.

`~2` counts snapshots, not turns: a turn that changed nothing does not take
one, so on a quiet session `~2` may be further back than two turns.

They cost nothing in the agent's context: the hook that writes them injects no
text. A turn that changed nothing writes no snapshot — though it still costs
the time to find that out — and the hook touches neither `HEAD`, the index nor
the worktree, so after it runs the agent's own `git status` says exactly what
it said before.

`omh sNN rm` mentions them when it removes a session, and never refuses over
them: there is one for nearly every session that ever ran, and a refusal that
fires almost always is one people answer with `--force` unread.

### `omh sNN diff` — the shape, the patch, or one checkpoint

```console
$ omh s01 diff          # what the session changed, as a summary
$ omh s01 diff -p       # the patch, through your pager
$ omh s01 diff 4        # one checkpoint, by its number in `omh s01 log`
$ omh s01 diff 4 -p
```

`-p` hands the terminal to git, so it is git's own pager and git's own colours
— the same arrangement `omh s commit` uses for your editor. Under `--json` it
never pages: the patch is a field in the answer, because a pager between a
script and the object it asked for is a hang with no error.

The number is validated against the session's own list before anything is
printed, and a number outside it is refused with the range. Not for safety —
the sandbox's repository holds nothing you may not see — but because a command
that prints any object you name is a different command from one that shows you
a checkpoint.

Your pager is whatever git resolves in *your* checkout — `GIT_PAGER`, then your
`core.pager`, then `PAGER` — so `delta` and friends work here as they do
everywhere else. omh asks git for it rather than guessing at the order.

The sandbox's own git config never gets a say. It cannot carry a pager anyway:
omh rewrites that file to a ten-key allowlist on every launch and mounts it
read-only. The reason omh has to name a pager at all is that same hardening —
without it, a host-side read is pinned to no pager at all.

### `omh sNN commit --keep` — the agent's own commits, curated

```console
$ omh s01 commit --keep          # all of them, in order, no editor
$ omh s01 commit --keep 1,3-4    # these, in this order
$ omh s01 commit --keep --edit   # the todo, for people who want it
```

The numbers are the ones `omh s01 log` printed, and the order is yours — `3,1`
lands the third checkpoint and then the first. Reordering is half of what
curating a history is for, so a selection is never quietly sorted.

Refused rather than resolved, and refused before anything moves: a number
outside the session's range, a range that runs backwards, a checkpoint named
twice, and a number naming work the branch already has. That last one is
ordinary to type — `log` numbers every checkpoint, including the ones below the
line — and replaying it would apply the same patch a second time.

`--edit` is the only form that needs a terminal, and it says so when there
isn't one. Without that check, git runs the unedited list, exits 0, and omh
reports a curation that never happened.

### `omh sNN sync` — trunk moved, and the session catches up

```console
$ omh s01 sync
s01 · 3 commits from main

  1 file needs resolving:
    src/tap.rs

  omh s01 log             the checkpoint this can be undone from
  omh s01 resume          the markers are in the sandbox, where fixing them cannot hurt you
```

Your checkout's commits never enter the sandbox's repository. The merge runs
**on the host**, in your repository, and the session receives files — so the
isolation the sandbox exists for survives a sync.

What the agent finds when it starts again:

- a commit saying `base moved to <sha>`, so `git show HEAD` is exactly what
  arrived and nothing else;
- conflicted files sitting in the worktree with their markers, uncommitted, so
  `git status` inside the sandbox is the to-do list — and `git checkout --
  <path>` takes the pre-sync version of one back;
- a checkpoint written just before the sync, so `omh s01 log` shows the point
  the whole thing can be undone from;
- and, on Claude Code, one sentence at its next start saying the tree moved —
  delivered once, not at every context rebuild. opencode and omp have no way to
  say anything at that moment; they name what they dropped when the session
  launches, and the commit above is still there to be read.

The conflict markers read `<<<<<<< main` and `>>>>>>> s01`, which is to say
they name the sides rather than two object ids.

**It refuses while the sandbox is up**, and `--down` stops it first. Not about
the files — the checkpoint makes an overwrite recoverable. It is about what the
agent *believes* the tree contains, which lives in its conversation and not on
disk: left running, it edits a version that no longer exists and trunk's changes
disappear inside a plausible patch. Stopping is the fix, not the price; the
harness restarts and reads the tree as it now is.

`omh s01 diff` still shows the agent's work after a sync, not trunk's — the
session's baseline moves with it.

Needs git 2.38 on the host. `omh doctor` says so if yours is older.

### `omh sNN commit` will not land a conflict

```console
$ omh s01 commit -m "Resolve the merge"
omh: s01 still has 3 conflict markers in its files:
  src/tap.rs:41: leftover conflict marker
  src/tap.rs:43: leftover conflict marker
  src/tap.rs:47: leftover conflict marker
Resolve them first, or:
  omh s01 commit --keep --force   commit them anyway
```

Both ways of committing refuse, because both would land them: `-m` stages the
files as they are and `--keep` replants the agent's commits on top of them. A
file the agent created and never added counts — that is the one `-m` would
sweep up.

`--force` is there because a conflict marker at the start of a line is not
always a conflict. A test fixture holds them on purpose.

### `omh sNN rm` — and what it refuses to take with it

The session's branch survives a removal and its files were on disk until it
ran. The agent's own commits are the exception: they live only in the sandbox's
repository, and `rm` deletes that. After a `git reset --hard` in the sandbox
they were the only copies there ever were.

So `rm` counts them first, and says so rather than asking:

```console
$ omh s01 rm
omh: s01 has 2 commits that no branch has. Removing it deletes the only copy:
  omh s01 log                 read what is there
  omh s01 commit --keep       put it on omh/s01
  omh s01 commit -m "…"       or take the files as they stand
  omh s01 rm --force          remove it anyway
```

Nothing is taken down before that refusal — not the container, not the marker
`omh s` reads, not the repository the refusal is about. `--force` is the way
past, and it means what it says.

**The count is wider than the one `omh s01 log` prints**, on purpose. `log`
numbers what you can act on; this asks whether anything in that repository
exists nowhere else, however the agent left it. Work thrown away with
`reset --hard` is still in there, and so is a branch the agent wandered off —
neither appears in the numbered list, and both are gone once the repository is.

A sandbox that never ran removes quietly. One omh **cannot read** does not:
that is a third answer, not a quiet yes, and it is the state a half-finished
removal leaves behind. `--force` covers it too, so nobody is stuck — they are
asked once.

### Getting work out of a session

The worktree lives under `~/.omh/worktrees/` so your IDE does not index it, and
you should never have to go there. `commit` and `push` are how the work comes
out, run on the host — the same place `diff` already runs:

```console
$ omh s diff
$ omh s commit -m "Fix the tap guard"
$ omh s push fix/tap-guard --pr
  omh/s01 → origin/fix/tap-guard
```

`commit` stages the agent's work and writes your message **verbatim** — no
trailer, no generated summary. omh has no view on what the work was for and will
not invent one. Without `-m`, git opens your editor.

What omh put in the worktree is not the agent's work and never lands in the
commit. The rules are mounted rather than written, so git does not see them at
all; carried files are a different matter:

```console
$ omh s commit -m "Fix the tap guard"
Error: config.toml is listed in carry_in and git tracks it, so what is in the
worktree is your local copy rather than the branch's.
  omh will neither publish that nor drop it silently.

  fix the cause:  omh set carry_in        (carry_in is for files git does not
                  track; a tracked file is already in the worktree)
  or just this once:  omh s commit --skip-carried
```

`carry_in` is for files git does **not** track, so this means the list needs
fixing — see [Configuration](configuration.md#keeping-the-agents-git-status-clean).
`--skip-carried` commits everything else meanwhile. omh refuses rather than
quietly leaving the file out because it cannot tell a credential you were
carrying from a change you meant.

`push` requires a name the first time and remembers it after. `omh/s01` records
when the work happened rather than what it was, and on origin it outlives the
session that would explain it — so omh refuses rather than choosing for you:

```console
$ omh s push
Error: omh/s01 is a session id, not a branch name
  name it:  omh s push <name>
```

`--pr` opens the pull request with `gh` when it is installed, and prints the
command when it is not. It is never a dependency: a repo on a non-GitHub remote
is a normal repo.

### Keeping the agent's own commits

The agent commits as it works, into a repository of its own that is not this
branch — see [Sessions](sessions.md#the-agent-has-git-and-it-is-not-yours).
`--keep` brings those commits here instead of squashing the files into one:

```console
$ omh s commit --keep
```

On its own it takes every checkpoint since the last handover, in order, and
opens nothing. `--keep 1,3-4` takes those, in that order; `--keep --edit` opens
the list, as `git rebase -i` does, so you reorder, reword and drop by hand.
What you keep arrives with the messages the agent wrote — which is the point, because it wrote them while it still had the context
you would be reconstructing from a diff.

`--skip-carried` has no meaning here and is ignored: `--keep` refuses a carried
file rather than dropping it, for the reason below.

Mutually exclusive with `-m`. Running both would put the squashed content on the
branch first, and git's patch-id then drops the replanted commits as already
applied — the granular history gone, with nothing said.

Every commit is authored `omh sandbox <sandbox@omh.invalid>`, whatever the agent
set in its own config. The sandbox does not get to say who wrote something on
your branch.

`--keep` refuses rather than repairing, in these cases and no others:

- **A carried file reached a commit** — by `git add -f`, copied under another
  name, pasted into source, or written into a message. omh knows the bytes it
  carried in, so it can tell; it will not rewrite your history to hide a secret.
  Drop the commit in the sandbox and harvest again.
- **The sandbox's repository is mid-rebase or mid-merge, detached, or has commits
  no branch there can reach.** The detached case was measured leaving work
  behind while reporting success; the other two are the same shape, argued
  rather than measured.
- **The session's worktree is not on its branch**, so omh will not move a branch
  the session left.
- **The branch moved while you were curating.** Nothing is written.
- **The session has no sandbox repository yet** — nothing has run in it.
- **omh cannot tell which commits are new.** `--keep` replays from the point it
  last handed over, and that point can stop meaning anything two ways: an agent
  that `reset --hard`s below it, or a record that is unreadable, empty or no
  longer a commit git knows. Replaying from the start of the session would offer
  the branch work it already has, so omh stops and says to take the files with
  `omh s commit -m`.

`--keep` is repeatable otherwise: land some work, let the agent carry on, land
the rest. Each round takes only what the last one did not.

The branch is untouched in every case, and nothing is lost in any of them: omh
fetches the sandbox's work into your repository before it replants, so even a
refusal that happens after the fetch leaves the commits reachable there. An
earlier version of this said the first two refusals happen before the work
leaves the sandbox. They do not — the carried-file scan reads the fetched
commits, which is how it can see them at all.

**`rm` never deletes a branch that has commits.** Unreviewed agent work must be
unloseable, so the branch outlives the session that produced it, and `rm` tells
you how to review or discard it:

```console
$ omh s01 rm
removed session s01; branch omh/s01 kept (3 commits to review)
  git log main..omh/s01
  git branch -D omh/s01
```

The summary is the answer and goes to stdout; the two commands are next steps
and go to stderr, so they reach you here and stay out of anything you redirect.
They carry no `review with` label because a suggested command is reproduced
exactly — a line you cannot select and paste is worse than no line.

A branch with **no** commits is dropped. Keeping it preserved nothing —
`worktree remove --force` has already discarded anything uncommitted — while a
namespace filling with dead refs trains you to ignore the ones that matter.

A branch omh cannot *count* is kept too, and says so — that count is
`git rev-list <base>..<branch>`, and it has no answer in a checkout whose
default branch exists only as `origin/<name>`. Dropping a branch is
irreversible and justified by one fact only, that it holds nothing, so anything
short of that fact falls the other way:

```console
$ omh s01 rm
removed session s01; branch omh/s01 kept — omh could not count it against main
  git log omh/s01
  git branch -D omh/s01
```

## `omh set` · `omh unset`

Change a setting, or switch one of omh's features. **Which file it lands in
follows from the key**, not from a flag you have to remember.

```
omh set <key> <value> [--local|--save]
omh set <feature> on|off
omh unset <key> [--local|--save]
omh unset <feature>                   let omh's own default decide again
```

One rule, and `omh use` / `omh unuse` follow it too:

1. **Every repo file that already gives it a value.** Writing one while another
   still declares the key is how a command reports success over something that
   never changed.
2. If none does — the **committed** `settings.toml`, because what runtime a
   project wants and which of omh's features it runs with are facts about the
   project a teammate cloning should get.
3. **Except** a key that can name a credential, which goes to the gitignored
   `settings.local.toml`. That exception is a table in `src/key.rs`, per key,
   and it is the whole of what keeps a secret out of git now that committed is
   the default. `omh why <key>` says which a key is.

```console
$ omh set carry_in '[".env"]'
omh: nothing was ignoring settings.local.toml — added it to /Users/you/proj/.omh/.gitignore
wrote → /Users/you/proj/.omh/settings.local.toml (gitignored)

$ omh set idle_timeout 30m
wrote → /Users/you/proj/.omh/settings.toml (committed)
```

The word in brackets is the point: `settings.toml` and `settings.local.toml`
do not read as opposites at a glance in a path eighty columns wide.

**`--local` and `--save` name the file outright**, and are the only way to put
one key in both. A key held in two files is then updated in both:

```console
$ omh set --local idle_timeout 45m
wrote → /Users/you/proj/.omh/settings.local.toml (gitignored)

$ omh set idle_timeout 20m
wrote → /Users/you/proj/.omh/settings.toml (committed)
wrote → /Users/you/proj/.omh/settings.local.toml (gitignored)
```

A named flag steps outside rule 1, so it can write a file something outranks —
and says so rather than reporting a change you cannot observe.

`omh set` also makes its own premise true: the gitignored file is only safe
because git ignores it, and that line used to be `omh init`'s alone. If nothing
is ignoring it, `omh set` adds the line and says it did.

### Features take `on` or `off`

One of omh's own features is a boolean in the `[omh]` table rather than a bare
key. It follows the same rule about which file, and takes the same two flags.

```console
$ omh set codegraph off
codegraph is off here
  nothing was uninstalled; the next repo gets it back
  wrote → /Users/you/proj/.omh/settings.toml (committed)

$ omh unset codegraph
this checkout no longer switches codegraph
```

Switched off, and *following omh's default*, are different states — which is
why `unset` exists for a feature at all.

Settings keys never collide with feature names, and a test fails the build if
they ever do: `omh set <name>` writes two different shapes into one file, so a
name meaning both could not be resolved by any error message. Feature and
*entry* names do overlap on purpose — the manifest names a feature after its
principal entry — and the feature reading wins. Naming an entry is refused,
saying which feature contains it:

```console
$ omh set graph-rules off
omh: `graph-rules` is part of the `codegraph` feature, not a feature itself. A feature is all or nothing.
  omh set codegraph off
```

`omh unset` removes the key from every repo file that holds it, because *where
would `set` put this* and *where is this set* are different questions, and
answering the second with the first left a committed `carry_in` standing while
reporting success:

```console
$ omh unset carry_in
removed carry_in from the shared layer
removed carry_in from the local layer
```

Anything still supplying the value afterwards is named — an unqualified `unset`
stays inside the repo, and a named flag touches one file on purpose — so
whatever still supplies the value is named.

## `omh settings`

**You**, before a repo exists. `~/.omh/default.toml` is the template `omh init`
seeds a new repo's `settings.toml` from — read once, at `init`, and never at
launch.

```
omh settings                          your defaults, and what omh reads
omh settings set <key> <value>        → ~/.omh/default.toml
omh settings unset <key>              remove one of your defaults
omh settings edit [<capability> [name]]
                                      $EDITOR on your defaults, or on one entry

omh settings mcp ls
omh settings mcp add <name> <cmd> [args…] [--env K=V]
omh settings mcp rm <name>
omh settings mcp import <harness> [--file <path>] [--force]
```

```console
$ omh settings set idle_timeout 45m
wrote → /Users/you/.omh/default.toml (seeds new repos)

$ omh settings
your defaults /Users/you/.omh/default.toml
  idle_timeout  45m

omh also reads
  carry_in     Untracked files a session needs — a worktree holds tracked files only. The one path by which a secret reaches the agent, so keep it short.
  account      Which captured login a session is launched with, by name.
  runtime      Which runtime builds and runs the sandbox. Unset means `auto`.
  persistence  How a session's terminal survives detaching. Unset means `dtach`.
```

The second list is the useful half. What omh reads is a table in the binary, so
no settings file can show it, and until this command existed the only way to
learn a key's name was to already know it.

**Not `omh set`.** They are one letter apart with opposite scopes — `omh set` is
this checkout, `omh settings` is what a new one starts from — so clap is
deliberately not told to accept unambiguous prefixes: `omh setting` is refused
rather than guessed at.

Keys omh reads, plus `[use]` and `[omh]`, are seeded; anything else in the
file is refused by name rather than left behind. `[mcp.<name>.env]` is **refused**: a
server's environment can be a token and the file `init` writes is committed. It
already has a home — `omh settings mcp add <name> <command> --env KEY=value` puts
it on the server in `~/.omh/mcp.json`, where servers live.

MCP lives under `omh settings` because MCP servers **are** configuration. They
live in your catalogue; a repo overrides a server's environment without
redeclaring it. See [Configuration](configuration.md).

**Your catalogue is `omh info`.** This command is your *defaults* — the four
verbs above all act on `~/.omh`, and what you have in it is part of what
`omh info` means by *what you have here*.

`edit` takes a name, so it validates one: `omh settings edit skills ../../.ssh/id_rsa`
is refused before `$EDITOR` sees it. Past that there is no fence to draw —
`$EDITOR` is a full program running as you, and the boundary that matters is
elsewhere: every catalogue directory omh mounts into a sandbox is mounted
**read-only**, so the agent can read a selected skill and cannot write one.

## `omh info --repo`

**This checkout.** What it uses, what it decided, and what decided it.

```
omh info --repo                       effective here, with provenance
omh info --repo --json                the same, for a script
```

It reports rather than writes. What changes a value here is `omh set` and
`omh unset`, and what changes a selection is `omh use` and `omh unuse` — both
above.

**`keyed as`** is what omh calls this checkout's state: the directory under
`~/.omh/worktrees`, the cache volume, and the name of every container it
starts. It is the checkout's basename plus a digest of where the checkout is,
because two projects called `api` are two projects — before 2026.08 they shared
one key, and the second one's `omh new` resumed into the first one's session.
`--json` reports it as `repo_id`, which is how to find out which `docker ps`
row belongs to which project.

```console
$ omh info --repo
this repo /Users/you/proj/.omh
  keyed as proj-561662ee

settings
  (nothing set)

omh's features
  codegraph   off here
  git-notice  on
  memory      on

using
  rules      everything · +2 from omh's features
  skills     everything
  mcp        everything · +1 from omh's features
  commands   everything
  subagents  everything
  hooks      everything · +2 from omh's features
```

`everything` is an absent `[use]` list — a new checkout follows your catalogue
as it grows. `· +N from omh's features` is what the base set adds on top, and
it is counted separately because those are not yours to select: `omh set
<feature> off` is their switch. With `codegraph` off here, its rules section,
its server and its hooks are the ones missing from those counts.

## `omh use` · `omh unuse`

Which catalogue entries this project takes.

```
omh use <capability> <name>           → <repo>/.omh/settings.toml, committed
omh unuse <capability> <name>
omh use --all                         resync every list to the whole catalogue
```

Capabilities: `rules`, `skills`, `mcp`, `commands`, `subagents`, `hooks`.

The write target is the **committed** file — what a project uses is a fact
about the project, and a teammate cloning it should get the same selection.
`omh set` has the same default for the same reason; what must *not* be
committable by accident is decided per key, in `src/key.rs`, rather than by
which command you reached for.

A capability with no list is following the whole catalogue, so the first
`omh use` on it writes the catalogue out rather than narrowing to one name, and
says so:

```console
$ omh use skills review-diff
skills was following your whole catalogue; wrote its 3 entries as the list
using skills/review-diff — wrote → /Users/you/proj/.omh/settings.toml
```

`unuse` refuses a name this repo never used, rather than writing the list back
and reporting success for a typo. `omh use` refuses one your catalogue does not
have, and names `omh settings edit` as the way to create it.

omh's own — `codegraph`, `memory`, the five generated hooks and their rules
sections — are not selectable in either direction. `omh set <feature> on|off`
is their switch, because a feature is all or nothing. See
[Configuration](configuration.md#a-feature-is-not-selectable).

## `omh import <capability> <harness>`

**The way in.** Bring a setup you already have into your catalogue, rather
than rebuilding it.

```
omh import <capability> <harness>     read it out of where that harness keeps it
omh import <capability> <harness> --from <path>
                                      read it out of somewhere else
```

Capabilities: `skills`, `commands`, `subagents`, `rules`, `hooks`, `mcp`.

```console
$ omh import skills claude
import from claude skills (~/.claude/skills)
  imported  tdd

wrote → ~/.omh/skills
```

It **never overwrites**. An entry whose name you already have is kept and
said so, so running it twice is safe and tells you nothing changed:

```console
$ omh import skills claude
import from claude skills (~/.claude/skills)
  kept  tdd  already in your catalogue
```

`--dry-run` reads everything and writes nothing, which is worth using first
if you are not sure what a harness has:

```console
$ omh import skills claude --dry-run
import from claude skills (~/.claude/skills)
  imported  tdd

--dry-run: nothing written
```

**Hooks land in the repo, not the catalogue** — a hook binds to a project's
own commands, so `cargo test` here and `pnpm test` next door is one name and
two bodies. They go to `<repo>/.omh/hooks/` and into `[use]`, or they would
arrive dead. Everything else is yours across projects and goes to `~/.omh`.

A symlink is refused at any depth, as is a name that is a path — an imported
entry is content omh will mount read-only into a sandbox, and neither of
those is content.

`--from` reads a config from somewhere other than where the adapter says that
harness keeps it. Useful for a setup in an unusual place, and for seeing what
omh would take without pointing it at your own.

This is the other half of [`omh eject`](#omh-eject-harness---to-dir): one
brings a setup in, the other hands it back.

## `omh eject <harness> --to <dir>`

**The exit.** Everything omh renders on a launch, written as ordinary files a
harness reads without a container.

```
omh eject <harness> --to <dir>        write it out and step aside
omh eject <harness> --to <dir> --dry-run
                                      render everything, write nothing
```

For an opinionated tool, a credible exit is what makes adoption safe. You are
handing omh your rules, your credentials and your sandbox policy, and being
able to leave with all of it is the difference between a default and a cage.

It is nearly free to build because omh already renders exactly these documents
every time it launches. The only difference here is the destination: a launch
stages them for a container, mounts them read-only at guest paths and links
directories at layer paths that exist nowhere but inside the sandbox. Eject
renders the same content through the same functions and writes real files.

```console
$ omh eject claude --to /tmp/ejtest
eject claude /tmp/ejtest

  rules  /tmp/ejtest/CLAUDE.md
  rules  /tmp/ejtest/AGENTS.md
  mcp    /tmp/ejtest/.mcp.json
  hooks  /tmp/ejtest/home/.claude/settings.json

omh: these still name paths only omh's sandbox has — `/omh`, `/work`, `$OMH_*` — so they need editing before a harness reads them outside one:
    /tmp/ejtest/CLAUDE.md
    /tmp/ejtest/AGENTS.md
    /tmp/ejtest/.mcp.json
    /tmp/ejtest/home/.claude/settings.json

  these are yours now — omh is not in the path
```

Two things in that output are the whole design.

**`/work` lands at the root, everything else under `home/`.** An adapter
declares where the *container* expects a file, and neither `/work/CLAUDE.md`
nor `$HOME/.claude/settings.json` is a host path. Keeping them apart is what
lets you tell "this belongs in my repo" from "this belongs in my dotfiles"
without knowing omh's mount layout.

**The warning is not a caveat, it is the honest half.** omh renders these for
a sandbox: the memory server is invoked with `--local /omh/notes/local`, hooks
read `$OMH_GRAPH_PROJECT`, rules point at `/work`. On your host none of that
resolves. omh names the files rather than rewriting them, because it does not
know where you want your notes or whether you will run the harness in a
container of your own — and a guess written into a file you are about to
depend on is worse than being told to look.

A capability the harness has no binding for is named too, so a reader
comparing the output to their omh setup does not assume something was lost.

`--to` is required and deliberately not the checkout: the command exists to
*show* you what you would be keeping, and one that overwrote a working tree by
default would be the opposite of reassuring.

## `omh memory …`

The note store: a graph of linked Markdown notes, scoped to this repo, that
survives session removal and a switch from one harness to another. That
survival is what makes it memory rather than context.

Two layers, and they **do not merge**. A setting has one value; a note is a
claim, and two claims about one topic are two facts — so the layer is part of a
note's identity, and `team/deploy` and `local/deploy` are different notes that
both retrieve.

| layer | where | who sees it |
|---|---|---|
| `team` | `<repo>/.omh/notes/` — committed | everyone who clones |
| `local` | `~/.omh/notes/<repo>/local/` | you, on this machine |

The local layer lives outside the checkout on purpose. A session is a git
worktree holding tracked files only, and removing one runs `git worktree remove
--force`, so a store inside the repo would be invisible to the sandbox and
destroyed by `omh s rm`. Inside the sandbox it is mounted at `/omh/notes/local`.

```console
$ omh memory
credentials-are-a-named-volume                     team   2026-08-05  1 ref
surprise/mounting-a-credential-file-returns-ebusy  local  2026-08-07  1 ref
```

The `surprise/` on the second key is the shipped template's namespace, not
decoration — see `omh memory remember` below.

**Every line carries its date and its layer.** A note presented without age and
origin cannot be judged, and a store the agent writes to unattended would
otherwise become a machine for laundering guesses into facts.

### `omh memory remember`

```console
$ omh memory remember \
    --expected "A bind mount of the token file would persist the login." \
    --observed "Mounting a credential file returns EBUSY." \
    --evidence 'EBUSY from the mount syscall'
recorded /home/you/.omh/notes/omh/local/surprise/mounting-a-credential-file-returns-ebusy.md
```

The three arguments are the discipline. Something with nothing to put in
`--expected` has learned nothing worth recording, so the filter runs for free.

**The key is derived, never chosen.** It comes from a template in
`.omh/memory.toml`, so the same observation cannot be recorded twice under two
spellings — `Mounting a credential FILE returns EBUSY.` and `mounting a
credential  file returns ebusy` produce one key, and the second write is a
conflict that says *update that note instead*:

```console
$ omh memory remember --expected … --observed "mounting a  credential FILE returns ebusy" --evidence …
Error: `surprise/mounting-a-credential-file-returns-ebusy` is already recorded;
       update that note instead
```

The slug is the observation's **first sentence**, and the terminators are `.`,
`!` and `?` only — a semicolon or a colon does not end one, so a long
`--observed` yields a long key.

`--if-exists skip|suffix|override` makes retry policy an argument. Skipping is a
mode you ask for, never a fallback: as a fallback, every genuine conflict
disappears silently. `override` is the destructive one and says so — it removes
the note that held the key, wherever that note was stored, and leaves the
replacement at the key's own path:

```console
$ omh memory remember --if-exists override …
replaced /home/you/.omh/notes/omh/local/surprise/mounting-a-credential-file-returns-ebusy.md — the note that was there is gone
```

Writes go to **`local` only**, always. A writer that could reach the committed
layer would push wrong facts to teammates through git, where they arrive with
the authority of a reviewed change.

### `omh memory lint`

Schema violations and graph hygiene, and the store-quality meter: a count per
rule, with no questions asked and no model pass.

```console
$ omh memory lint
refused  local  a `surprise` note needs a `## Evidence` section
warning  team   `deploy` links to `rollback`, which is not in the store
warning  team   nothing in the store links to `deploy`
warning  local  nothing in the store links to `surprise/mounting-a-credential-file-returns-ebusy`

    1  MissingSection
    1  DanglingLink
    2  Orphan
Error: 1 violation the schema refuses
```

`Orphan` fires on every note nothing links to, so a young store is mostly
orphans — that is the rule working, not the store failing. It is the count over
time that means something.

The two words are not decoration. **Schemas refuse and hygiene warns**, because
agents negotiate with warnings and cannot negotiate with a refused write — and
because a store-wide problem must never fail somebody else's write.

The same split decides the exit code: **the command fails when the schema
refused something, and not for warnings** — the run above exits 1 because of the
one `refused` line, and would exit 0 without it. Until the agent's own writes
are refused, this is what a hook or a CI step can gate on, and a gate that also
tripped on hygiene would be red for every store with an unlinked note in it.

Three rules are worth naming because they describe the store rather than one
note's shape. `UnclosedFence` refuses a note whose code fence never closes:
everything after it is quoted, including headings and `[[links]]`, so the note
does not mean what it looks like. `DuplicateKey` warns when one key is claimed
by two files in one layer — §6 makes a key a primary key, `remember` refuses to
create a second, and this is how one that arrived by hand becomes visible.
`CrossLayerLink` warns when a *committed* note links to one that is not: the
link works on the machine that wrote it and dangles in every clone, which is
invariant 2 and the reason `promote` exists. It warns rather than refuses
because the note at fault is somebody else's, and an agent writing right now
cannot fix it — `promote` is where the same condition is fatal.

### `omh memory promote <key>…`

local → team. **The only place a human gates anything**, because it is the only
place a wrong note reaches somebody else. Everything else is invisible: a memory
you have to approve is a notebook, and nobody keeps one.

```console
$ omh memory promote surprise/credentials-live-in-a-volume
omh: `surprise/credentials-live-in-a-volume` links to
     surprise/ebusy-on-a-file-mount, which is not committed — promote it too,
     or drop the link
Error: promoted nothing
```

That refusal is **invariant 2**: a committed note may only link to committed
notes, because a teammate's clone has no gitignored layer to follow the link
into. The lint warns about it; `promote` refuses, since a warning is negotiable
and this is the last point at which it can be caught.

These are two mechanisms, not two settings on one dial. A rule's `Severity`
gates a *write* — `CrossLayerLink` is a warning there because the note at fault
is committed and the agent writing right now cannot fix somebody else's. A
`Blocker` gates a *promotion*, and there the same condition is fatal. Raising
the rule to a refusal to make them agree would fail every `remember` in a repo
whose committed layer has one bad link.

Invariant 2 is not the only thing refused, because it is not the only way a
promotion can publish something wrong:

- **A note the schema refuses** is never shared. An unclosed fence is the case
  that matters: it quotes every heading and link below it, so the invariant-2
  check would read no links at all and pass on the one note whose links cannot
  be seen.
- **A key that is not a key.** `validate_key` guards the mint; a key read back
  off disk has never been through it, and every path here is built from it.
- **A destination that already exists.** The conflict above asks whether a
  *key* is committed; the write lands on a *path*, and a note whose frontmatter
  disagrees with its filename owns one without the other.
- **A key claimed by two local files** — `rm` refuses this store and asks for
  `--at`; promoting one and leaving the other would be a third story about it.
- **An ignore check that could not answer.** `git check-ignore` reports
  ignored, not-ignored, or failure, and the third is not the second.

Name them together and both go:

```console
$ omh memory promote surprise/credentials-live-in-a-volume surprise/ebusy-on-a-file-mount
promoted surprise/credentials-live-in-a-volume → .omh/notes/…
promoted surprise/ebusy-on-a-file-mount → .omh/notes/…

not shared until committed:
  git add :/.omh/notes && git commit
```

Two notes that link to each other are unpromotable one at a time, which is why
the check knows what else is in the batch.

`:/` in that last line is git's root-relative pathspec, so the command works
from wherever you happened to run `promote`. A bare `.omh/notes` matches
nothing from a subdirectory, and a command that fails is a poor way to end the
one step that actually shares the note.

**One blocked key stops the whole batch.** A half-finished promotion leaves a
store nobody planned, and you would have to work out which half landed.

**The key never changes** — identity is `(layer, key)`, so promotion moves a
note between layers rather than renaming it — and nothing else is rewritten.
Notes that pointed at it keep working, because from `local` a key resolves into
either layer.

### `omh memory rm <key> [--layer team|local] [--at <path>]`

Removes one note and reports what pointed at it.

```console
$ omh memory rm credentials-are-a-named-volume
removed credentials-are-a-named-volume (team)
  it was committed — teammates keep it until you commit the deletion
  still linked from surprise/mounting-a-credential-file-returns-ebusy — those links now dangle, and `omh memory lint` lists them
```

**Deletion never cascades**, and neighbours are never rewritten. A dangling link
is visible and the lint finds it; a silently pruned neighbourhood is neither.
This is the same rule that makes `omh s rm` keep a branch holding commits.

`--layer` is needed when one key exists in both layers, which is a disagreement
rather than a duplicate. Without it, `rm` names both and removes neither.

`--at` is the repair path for something worse: two files in *one* layer claiming
one key, which `omh memory lint` reports as `DuplicateKey`. It takes any
trailing run of path components — `dup.md` alone is enough when it is
unambiguous — and `rm` prints the layer-relative form:

```console
$ omh memory rm surprise/the-mount-failed
Error: `surprise/the-mount-failed` is one key over 2 files in local — name one
       with --at: ns/dup.md, surprise/the-mount-failed.md
```

A `--at` that names no note is an error, never a fall-through to a different
one: it is the flag you reach for to be careful, so it is the one input that
must not be quietly ignored.

### `omh memory stale`

Notes the world has moved on from. **A join against facts omh already holds**,
never a judgement — there is no "old enough to be suspect" here, because that
would be a threshold nobody calibrated wearing the clothes of a fact.

```console
$ omh memory stale
stale:
  surprise/the-base-set-pins-its-version   local  2026-08-10  — the base set is
                                                                2026.08; this note
                                                                pinned 2020.01
  surprise/the-entrypoint-is-tiny          local  2026-08-10  — `src/main.rs` was
                                                                f328e4d…, is now
                                                                8370720…

omh cannot tell:
  surprise/a-symbol-nobody-can-check       local  2026-08-10  — no indexed code
                                                                graph reachable
                                                                from the host

no expiry — carries only its date:
  surprise/nothing-pins-this-at-all        local  2026-08-10

1 still current
```

Three groups because they are three different claims. **"omh cannot tell" is
never folded into "still current"** — that is the one failure that would make
this command a liar rather than merely incomplete.

A note declares its expiry with `--invalidated-by`, from a closed set omh can
evaluate itself:

| | invalid when |
|---|---|
| `file:<path>@<hash>` | the file's content changes, or it is deleted |
| `image:<digest>` | the **base** image recipe changes. Pin it as `image:current` and omh records what it would build now — the value is a `git hash-object` of recipe text that lives inside the binary, so there is nothing to read it off. A harness layer changing does not fire it |
| `base:<version>` | the base set moves to a **later month**. `parse_ym` compares year and month, so a re-cut within the same month is not a change, and a rollback to an older set is not staleness |
| `symbol:<name>` | the code graph no longer contains it |
| *(absent)* | never — it carries only its date |

Anything else is **refused at the door**. A trigger omh cannot evaluate is a
note advertising an expiry it does not have, which is worse than carrying none,
because somebody trusts it.

A `file:` path recorded inside the sandbox as `/work/src/main.rs` is stored
relative to the repo. Without that, every file trigger would report stale the
moment it was written.

**`symbol:` always answers "cannot tell" today.** The code graph lives in a
container volume, queried per session through a running sandbox under a
per-session project name; a host-side command has none of those. Asking for one
would put a container exec in the middle of a command whose whole point is that
it only joins facts already at hand. It fires correctly the day a symbol set can
be supplied, and there is a test that says so — so this is a gap, not a stub.

### What the agent sees

Inside a session the store is two MCP tools, not a command. omh runs the server
itself rather than pointing the harness at a graph server directly, because the
guarantee below cannot be enforced on a server the agent talks to on its own.

**`recall(question)`** ranks, expands one hop, and returns the neighbourhood in
one call. Every line carries the note's date and layer:

```
credentials-are-a-named-volume            team · 2026-06-12
└─ mounting-a-credential-file-returns-ebusy  local · 2026-08-07
```

Ranking is by how *rare* the question's words are in this store — a word in
every note cannot tell two notes apart, so it barely counts. Ties break on
recency, then on what the rest of the store points at. **Layer is never a
tiebreak**: contradicting notes both come back, and reconciling them is the
agent's job, done with dates and layers in hand.

**`remember(expected, observed, evidence)`** records a surprise. The three
arguments are the filter — something with nothing to put in `expected` has
learned nothing. It cannot pass a layer, a source, or an override: writes go to
`local`, provenance is omh's (the session from its environment, the harness
from the MCP handshake), and strict mode has no off switch over MCP.

The tool description carries a census of the store — counts, never titles, so
what it costs stops growing with the graph. It is recomputed on every
`tools/list`, so a note written this session is advertised in it.

### `omh memory serve`

Hidden, and not for you to run: it speaks JSON-RPC on stdin and waits, which is
indistinguishable from a hang. The harness spawns it inside the sandbox.

That means an `omh` binary has to exist *in there*. On Linux the running binary
is mounted as-is; elsewhere omh cross-builds one into `~/.omh/bin` on first
launch and mounts it read-only. A **released** omh carries no sources and
cannot do this — that needs a published Linux build, and until then the server
degrades to absent with one line at launch rather than failing it.

### What is not here yet

Hub pages — notes whose job is joining others — wait on a calibrated lint, and
that needs a store nobody has grown yet. See [the design](design/memory.md#14-build-order).
A subcommand that printed "not implemented" would be worse than its absence,
because `--help` advertises it.
