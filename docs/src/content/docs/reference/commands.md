---
title: Commands
description: Every command, what it does, and what it prints.
---

Every omh command: what it does, the flags it takes, and what it prints. Find
yours in the table, then follow it to its entry.

## At a glance

| Command | What it does |
|---|---|
| **Set up** | |
| [`omh init`](#omh-init) | Set this repo up, once. Decides everything, asks nothing. |
| [`omh upgrade`](#omh-upgrade) | Apply a newer omh: refresh the catalogue, check again which provides apply, rebuild the images whose recipe moved. |
| [`omh auth <harness>`](#omh-auth-harness---name-account---import) | Log a harness in once; repeat with `--name` for several accounts. |
| [`omh doctor`](#omh-doctor---harness-name--d) · `d` | Check, inside a real sandbox, that a harness sees your setup. |
| **Sessions** | |
| [`omh new <harness>`](#omh-new-harness----args) | Start a session and run an agent in it. |
| [`omh s`](#omh-sessions---s) · `omh sNN` | Every session and its state, or one, with what to run next. |
| [`omh sNN resume`](#omh-snn-resume--rejoin-it-as-what-it-was) | Rejoin a session, running the harness it ran before. |
| [`omh sNN attach`](#omh-snn-attach-editor--omh-snn-a) · `a` | Open a session in your editor, over SSH. |
| [`omh sNN log`](#omh-snn-log--what-the-agent-has-committed) | What the agent committed — or, with `--turns`, what each turn changed. |
| [`omh sNN diff`](#omh-snn-diff--the-shape-the-patch-or-one-checkpoint) | What the session changed, or one checkpoint. |
| [`omh sNN commit`](#getting-work-out-of-a-session) | Commit the work onto the session branch, after this repo's checks. |
| [`omh sNN commit --keep`](#keeping-the-agents-own-commits) | Keep the agent's own commits instead of one squashed commit. |
| [`omh sNN push`](#getting-work-out-of-a-session) | Push the session branch to origin under a name a reviewer can read. |
| [`omh sNN sync`](#omh-snn-sync--trunk-moved-and-the-session-catches-up) | Bring trunk into a session, or every session, merged on the host. |
| [`omh sNN down`](#omh-snn-down--stop-a-sandbox-keep-its-work) | Stop a sandbox, or every sandbox; worktrees and branches survive. |
| [`omh sNN rm`](#omh-snn-rm--and-what-it-refuses-to-take-with-it) | Remove a session. A branch holding work is kept. |
| [`omh graph`](#omh-graph---stop) | Browse this repo's code graph in a browser. |
| **Inspect** | |
| [`omh info`](#omh-info) | Harnesses, editors, sessions and your catalogue, on this machine. |
| [`omh info --repo`](#omh-info---repo) | This checkout: what it uses, and what decided it. |
| [`omh why <thing>`](#omh-why-thing) | Who put something here, and on what grounds. |
| **Configure** | |
| [`omh set` · `omh unset`](#omh-set--omh-unset) | Change a setting in this repo, or switch one of omh's features. |
| [`omh use` · `omh unuse`](#omh-use--omh-unuse) | Choose which catalogue entries this repo takes. |
| [`omh settings`](#omh-settings) | Your defaults for new repos, and your catalogue. |
| [`omh import`](#omh-import-capability-harness) | Bring a setup you already have into omh. |
| [`omh eject`](#omh-eject-harness---to-dir) | Write the config out as ordinary files, and step aside. |
| **Memory** | |
| [`omh memory`](#omh-memory-) | The notes this repo's sessions share, each with its date. |
| [`omh memory remember`](#omh-memory-remember) | Record a surprise as a note. |
| [`omh memory lint`](#omh-memory-lint) | Schema violations and graph hygiene, counted per rule. |
| [`omh memory rm`](#omh-memory-rm-key---at-path) | Remove one note, and report what linked to it. |
| [`omh memory stale`](#omh-memory-stale) | Notes the world has moved on from. |
| **Clean up** | |
| [`omh prune`](#omh-prune) | Remove what omh left behind and nothing claims any more. |

## `--dry-run`

**Everything runs; nothing is written.** Not a description of what would happen
— the real code path, with persistence withheld. So the layers a write would
reach, the list a selection would become and the refusals a bad name would earn
all happen exactly as they would for real, and the only difference is that the
file is left alone.

**A command that cannot yet answer it refuses the flag** rather than running:

| Previews | Refuses `--dry-run` |
|---|---|
| `new`, `sNN resume`, `upgrade`, `doctor`, `prune`, `set`, `unset`, `use`, `unuse`, `import`, `eject`, `settings` and its `set`, `unset`, `rm` and `mcp` verbs, `memory rm` | `init`, `auth`, `graph`, `info`, `why`, `settings edit`, the session listing and every session verb but `resume`, `memory` and its other verbs |

<details>
<summary>Why a command refuses the flag rather than guessing, or ignoring it</summary>

`init` builds images, and the session verbs stop containers, replant commits
and delete worktrees; each has to compute what it *would* do, and a preview
that guessed would be worse than none. Read-only commands refuse it too, for
the opposite reason: `omh info` is its own dry run.

That rule exists because the flag used to be accepted and discarded —
`omh --dry-run use --all` wrote the file and printed `wrote →`.

</details>

## The shape of the CLI

Noun-verb groups with single-letter aliases: `omh s sync`, `omh s01 commit`,
`omh d --harness claude`. The noun on its own is the listing — `omh s` is every
session, and `omh s01` is that one.

**Every command is a named command.** A harness runs through a verb —
`omh new claude` — and never as a word on its own. Harnesses and editors still
share a namespace, but only inside their own verbs — `omh new zed` and
`omh s01 attach zed` say which they mean.

<details>
<summary>Why a harness name on its own stopped being a command</summary>

A bare name used to be a harness — the bare name launched — which meant any word
could be a launch and so no word could safely be a command. It did two jobs,
too: start a session or reattach to one, depending on what was there. A
`RESERVED` list of nineteen names existed to keep them apart, and the selector
had to parse each line twice to work out which reading was meant.

`omh new claude` costs one word and removes the class. There is no bare slot to
shadow, so the list and the double parse are both gone.

</details>

### Flags mean one thing everywhere

A flag learned on one command is true on the next, or it is not there.

| | |
|---|---|
| `--from <path>` | Read this instead of where the adapter says: `omh import`, `omh settings mcp import`. |
| `--to <dir>` | Write here: `omh eject`. |
| `--yes` `-y` | *Yes, I have read the warning — answer the prompt in advance.* `omh sNN rm` and `omh settings rm`, for runs with nobody to ask. It does not force the removal, only the answer. |
| `--all` | Every one, not the one named: `omh use --all`. Session verbs have no `--all` — for `sync` and `down`, naming no session already means every session. |
| `--session <id>` `-s` | The session to act on. `omh s01 <verb>` says the same — see [`omh sessions`](#omh-sessions---s). |
| `--dry-run` | Do none of it and show what would be done, or refuse — [above](#--dry-run). |
| `--json` | The answer as JSON, or a refusal from a command that hands you a program and has none — [below](#what-every-command-prints). |

<details>
<summary>Why there is no <code>--force</code>, and what each one became</summary>

`omh sNN rm --yes`, `omh sNN commit --allow-conflicts` and `omh settings mcp import
--replace` were all `--force` until 0.10, which made three meanings for one word
— and `--force` on `rm` did not even force the removal, only the answer to its
prompt. Each is now named for what it does. `--force` parsed on all three as a
hidden alias through 0.10, and from 0.11 each command refuses it by naming its
own replacement — as `omh settings mcp import` does `--file`, which it took
before it read `--from` like `omh import` does.

</details>

### What every command prints

Two audiences, one answer. Every command builds a value and renders it either
for a person or for a program, so the two cannot disagree about what happened.

| | |
|---|---|
| `--json` | The answer as JSON, with stable field names. Never coloured, whatever `--color` says, and without hints. |
| `--color auto\|always\|never` | Defaults to `auto`: styled on a terminal, plain in a pipe. |
| `NO_COLOR` | Set and non-empty, it turns `auto` plain. `--color always` still wins over it. |

**stdout is the answer; stderr is everything else.** What a command was asked
for goes to stdout, so `omh s > sessions.txt` captures exactly that.
Warnings, progress and next-step hints go to stderr, so they still reach you
when stdout is redirected — and stay out of the file.

**JSON carries the fact, not the sentence.** The human column is English for a
reader; the JSON field is the thing a script would otherwise have to parse it
back out of. Where omh could not tell, the JSON says `unknown`, never an empty
string.

**Both flags are omh's own**, so `omh new claude --json` is **omh's** —
everything before the `--` is. Use `omh new claude -- --json` to hand it to the
harness instead. See [where omh's flags end and the harness's
begin](#where-omhs-flags-end-and-the-harnesss-begin).

**A command that hands you a program refuses `--json`.** `omh new`, `omh sNN
resume` and `omh settings edit` end by giving the terminal to a harness or an
editor, and `omh memory serve` speaks a protocol on stdout; none has an answer
to print, so the flag is refused rather than accepted and dropped. With
`--dry-run` the preview is the answer, and `omh --dry-run --json new claude` is
how a script reads a launch plan.

<details>
<summary>An example: <code>omh s</code> as a person and a program read it, and what its JSON fields mean</summary>

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
  "unchecked": [],
  "unreadable": []
}
```

**`unchecked` and `unreadable` are how a partial answer says so.** Both render
as nothing at all when they are empty, so without them a script cannot tell
"nothing was left behind" from "omh could not go and look": `leftovers: []` is
what a clean checkout prints either way. `unchecked` carries one line per read
omh could not make while sweeping for leftovers — an unreadable `shadow/`, a
container runtime it could not choose — plus, when omh could not read
`worktrees/` and so cannot tell a live session from an orphan, the count it
declined to name rather than hand you `omh <id> rm` for a session that is
running. `unreadable` is the sessions omh could not read for the overlap scan.

`work.state` is one of `clean`, `uncommitted`, `landed`, `unpushed`,
`published` or `unknown` — `uncommitted` and `unpushed` carry a `count`,
`published` carries the `branch`, and `landed` carries the whole `commit` this
session's work reached trunk as (the row a person reads shortens it). The one
that earns the enum is **`unknown`**: it means omh could not tell, which is a
different answer from `clean` and the one most dangerous to confuse with it.
The human column prints `?` for it, and the string version of this API printed
`""` for both.

`work` is `null` — not `unknown` — where nobody asked, so a caller can tell an
unanswered question from an unanswerable one.

</details>

<details>
<summary>Why <code>--color always</code> beats <code>NO_COLOR</code>, and <code>--json</code> beats both</summary>

`NO_COLOR` speaks for a user who has not said otherwise, so `--color always`
still wins over it — that is you, on this one run, with your own hands. An
**empty** `NO_COLOR` counts as unset, per [no-color.org](https://no-color.org):
presence alone is not the trigger, and an empty value is what a shell leaves
behind when a wrapper script unsets a variable badly.

`--json` never emits colour, whatever `--color` says, because a script setting
`--color always` in a wrapper should not break `jq`. It also suppresses hints,
which are prose for a person.

</details>

---

## `omh init`

Sets up the current repo, once. Decides everything, asks nothing, reports all of
it.

```
omh init
```

What it prints, and the seven things it does in order, are in
[Your first session](/getting-started/first-session/#what-it-actually-did).

**A one-time command.** The first run stamps the repo (`seeded-by`); running it
again in a repo already set up refuses and points at
[`omh upgrade`](#omh-upgrade), which is the verb that refreshes and rebuilds
from there on. The two split cleanly on that stamp: `init` runs when the repo is
not set up, `upgrade` when it is.

## `omh upgrade`

Applies a newly-installed omh to a repo you already set up, and looks again at
what the repo needs.

```
omh upgrade
omh upgrade --dry-run      decide and report all of it, touch nothing
```

Run it after you update omh (a `brew upgrade`, say): the new binary carries
fresh harness version pins, and this is what makes them take effect. Run it,
too, when the repo changes what it needs — a new lockfile, a second ecosystem —
because it is the command that looks at that again. It:

1. refreshes the managed catalogue from the binary — adapters, the base set,
   editors, stacks, hooks, markers — keeping anything you edited as `.yours`,
   just as `init` does;
2. asks the sandbox again which of this repo's provides apply, and corrects
   [`[provision]`](/reference/configuration/#provision--what-your-sandbox-is-built-with)
   before it builds anything;
3. rebuilds every image whose recipe moved (the base, each harness, and this
   repo's stack), reaps the superseded ones, and reports one word per harness —
   `rebuilt`, `current`, or `unpinned` (an adapter that pins no version, which
   omh cannot rebuild to anything reproducible);
4. names any session still running on an image it just superseded — a relaunch
   away from the new one — and advances the `seeded-by` stamp, so
   `omh doctor`'s drift row goes quiet.

A repo that has swapped a `yarn.lock` for a `pnpm-lock.yaml` since `init`:

```console
$ omh upgrade
```

```text title="omh upgrade"
refreshed claude, codex, omp, opencode
+ node/pnpm — applies here now
- node/yarn — no longer applies here
  rebuilt  claude
  rebuilt  codex
  rebuilt  omp
  rebuilt  opencode
```

**The `+` and `-` lines are edits to `[provision]`** in your committed
settings, so they are in `git diff` afterwards and reach your team with the next
push. When the table is already right there are no such lines, and the file is
not touched.

**Four things a second look never does:**

- **It never touches a `false`.** That can only have been typed, so it is a
  decision, and it outlives a condition that now applies.
- **It never reads "could not answer" as "no".** A condition that exits with
  anything but 0 or 1 changes nothing: the entry stays as it was, and the line
  names it with the code. A `package.json` caught half-written, in a repo that
  declares pnpm there, gets `node/pnpm's condition could not answer (exit 5) —
  left as it was`, and keeps pnpm.
- **It never records a silence.** If the sandbox could not be asked at all, the
  table is left exactly as it was and the report says the question was not put.
- **It never takes an empty directory for your repo.** The conditions first
  check that they can see the file each stack was detected by, and refuse to
  answer when they cannot — see
  [`the sandbox cannot see …`](/guides/troubleshooting/#the-sandbox-cannot-see--which-is-in-this-repo).

**The table is written before the images are built**, because they are built
from it. If a build then fails, the error says what had already been written:

```text
omh: `[provision]` in /Users/you/code/your-project/.omh/settings.toml had already been rewritten when this failed (+ node/pnpm, - node/yarn)
  because failed to build omh/claude:e3e863b9c2cb4291
```

**It rebuilds, so it needs Docker; it needs no login.**

**`--dry-run` decides and reports all of it and touches nothing** — it asks the
sandbox too, which is a read, and prints the `[provision]` lines a real run
would write. With no harness image built yet it has nowhere to ask, and says
that instead.

**It runs in a repo already set up**; in one that is not, it refuses and points
at `omh init`.

## `omh auth <harness> [--name <account>] [--import]`

Runs the harness's own login and captures the result.

```
omh auth <harness> [-n <acct>] [--import]
```

| | | Default |
|---|---|---|
| `<harness>` | `claude`, `codex`, `opencode` or `omp` | |
| `--name <account>` `-n` | the account to capture it as, e.g. `personal` or `work` | `default` |
| `--import` | copy the login this machine already has instead; needs no container runtime | |

```console
$ omh auth claude --name personal
$ omh auth claude --name work
```

**Log in once per account.** Repeat with another `--name` for several. See
[Accounts](/guides/accounts/).

**`--import` copies the adapter's `token` files**, so it works only where the
harness keeps its login in that file on this machine too — Codex by default
does; Claude Code on macOS keeps it in the Keychain, so there it finds nothing
to copy. A harness with no `token` file (omp) is refused. One that has it:

```console
$ omh auth codex --name work --import
```

```text title="omh auth codex --import"
`work` captured for codex
  copied from /Users/you/.codex/auth.json
```

## `omh doctor [--harness <name>]` · `d`

Launches the real image with the real mounts and checks the guest paths the
adapter claims. The only thing that can verify an adapter.

```
omh doctor [--harness <name>]
```

| | | Default |
|---|---|---|
| `--harness <name>` | the harness to verify | the one your host says you use |

[Troubleshooting](/guides/troubleshooting/#omh-doctor) says what each row means.

**It gathers the host first, and reports it whatever happens next.** These rows
are computed before any container work, and print under their own `host`
heading, above a second heading naming the harness and the image that ran:

- whether the container runtime is **answering** — not merely installed: a
  stopped Docker Desktop is on `PATH` and useless
- the stacks it detected, and the marker file that decided each
- settings set here that omh does not read
- what omh has left behind
- which omh set this checkout up
- disk free where omh keeps its state
- the host's git
- whether this checkout's own declared config still holds up, read the same
  way, without a container: every name in `[use]` still names a catalogue
  entry, every file in `.omh/hooks/` still parses, and every `carry_in` path
  still exists

A twelfth row appears only when this repo has no commit for a session branch to
fork from.

**Only a git omh cannot use at all fails.** The git row is the version, and
whether it can take a `--keep` selection, asked of the binary rather than
compared against a version number. A repo of prose has no stack, and that is not
a failure either.

**The sandbox has a git row of its own**, under the harness heading. It asks the
same question of the image — whether `merge-tree` takes `--merge-base` — and
prints the version.

**A detected stack whose toolchain will not be installed says so**, and says
which of the two reasons it is: `[provision]` switched it off, or it has not
been provisioned yet. Both install nothing; only one of them is a decision you
made. That is the answer to "why is there no python in my sandbox".

<details>
<summary>Why the host rows come first, and why a missing capability is not always a failure</summary>

Gathering them first is what matters: on a machine with no runtime, or one
where the image cannot be built, they are the whole of what omh can tell you,
and they used to be thrown away with the failure. A host row and an adapter row
used to share one undivided table, so a fact about your machine and a fact
about the container looked identical. A host-only run — nothing ran in a
sandbox at all — prints the one table it always did, with no heading to tell it
apart from a section that does not exist.

An older git that cannot name checkpoints is still a working git, and a
`doctor` that goes red over something you never run is one you stop running.
The same reasoning holds for the stacks row.

Nothing omh does needs `--merge-base` in the sandbox — omh merges on the host —
but the agent's commits and your turn-end check run inside, and a sandbox git
three Debian releases behind fails those in ways that name anything but git.
The row that failed printed eleven test names and no version; this one prints
the version.

</details>

## `omh new <harness> [-- args…]`

Starts a session and runs a harness in it. Arguments for the harness go after
`--`; everything before it is omh's.

```
omh new <harness> [-- args…]
```

| | |
|---|---|
| `<harness>` | `claude`, `codex`, `opencode` or `omp` |
| `-- args…` | handed to the harness untouched |

```console
$ omh new claude
$ omh new opencode
$ omh new claude -- --resume x
```

**`new` always starts a session that does not exist yet.** To go back into one
you already have, `omh sNN resume` rejoins it as the harness it ran, and
`omh sNN resume <harness>` rejoins it as a different one — see
[`omh sNN resume`](#omh-snn-resume--rejoin-it-as-what-it-was).

**What the harness cannot express is announced once, at launch**, rather than
silently dropped:

```console
$ omh new opencode
```

```text title="omh new opencode"
omh: opencode on omh/s01 — dropped hooks: git-note (no `session-start` moment),
     graph-first (no `search` tool),
     graph-orient (no `session-start` moment),
     graph-read (no way to inject text before a tool runs)
```

**Which credential set a launch uses** is `omh set account <name>`, per project.
See [Accounts](/guides/accounts/).

### Where omh's flags end and the harness's begin

Before the separator a flag is omh's; after it, the harness's. Nothing is
inferred and nothing is refused for being ambiguous, because nothing is
ambiguous:

```console
$ omh --dry-run new claude       # omh's
$ omh new claude --json          # omh's, reported as JSON
$ omh new claude -- --resume x   # claude's
$ omh new claude -- --json       # claude's, even though omh has one too
$ omh new claude -- -s work      # claude's, short flags included
$ omh new claude --resume x      # an error: omh has no --resume
```

**A flag omh cannot place is refused.** That last line is the trade: a flag
`omh new` does not know is likelier to be a typo than a gift. Put it after `--`
and it goes through untouched.

<details>
<summary>The rule this replaced, when a bare harness name had no separator</summary>

The bare name had no separator, so omh had to guess: it refused its own long
flags after a harness name and left short ones alone, on the reasoning that
`-s` is a flag plenty of harnesses have and refusing it would break launches
that worked. A line like `claude --json` could have been meant for either, and
that rule was a judgement about which mistake was likelier; an unknown flag was
forwarded on the assumption it belonged to the harness.

With `--` there is nothing to guess, so `omh new claude -- -s x` hands `-s` on
and `omh new claude -s x` is omh's. Everything before `--` is omh's, everything
after it is the harness's, and there is no third category.

</details>

## `omh sessions …` · `s`

Lists your sessions, and holds every verb that acts on one.

```
omh s                   sessions, their branches and state
omh s01                 that one, with what to run next
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

**The session goes first, and everything after it is what you would have typed
anyway.** Each verb has its own entry below.

```console
$ omh s01 diff        # that one — a session verb names its session
$ omh s01 commit -m "Fix the tap guard"
$ omh s01 rm
$ omh s01 resume      # rejoin it
$ omh s01 attach zed
```

`s` is the sessions namespace; `sNN` scopes it to one session, so `omh s01 diff`
is exactly `omh sessions --session s01 diff`. `--session` still works, and is
the only way to name a session whose id is not `sNN`. Naming it twice is refused
rather than resolved.

**Naming no session is not a hidden pick of the last one.** `--session`/`sNN` is
the one selector. Its absence means **every session** for the reversible verbs
that can act on all — `sync` and `down` with no session named reach all of them
(there is no `--all`). For every other session verb — `commit`, `push`, `log`,
`diff`, `attach`, `resume`, `rm` — naming no session is refused: omh never
guesses which one you meant, even when exactly one exists.

**A session named in front of a command that does not act on one is refused.**
The prefix still names the session when what follows is not a session verb, and
that command says so rather than dropping the id: `omh s01 why codegraph` is an
error, not a report about the repo. Running a harness in an existing session is
`omh sNN resume <harness>`.

**A container omh could not ask about is not a container that is stopped.**
The state column has four answers, not two:

| | |
|---|---|
| `up` | the sandbox is running — or paused, or restarting, which are the same thing for anything omh would do to it |
| `stopped` | it is not, or was never built |
| `up?` | omh asked and the runtime would not answer. The reason is on stderr |
| *(empty)* | nobody asked — no container runtime on this machine, said once above the table |

`up?` does not appear beside `up` in a real listing — every row is answered by
the same daemon in the same run, so when one row cannot be answered they all
look like this:

```console
$ omh s
```

```text title="omh s"
omh: could not tell whether s01's sandbox is running: cannot connect to the Docker daemon
omh: could not tell whether s02's sandbox is running: cannot connect to the Docker daemon
  s01  omh/s01  up?
  s02  omh/s02  up?  3 uncommitted  (12 behind main)
```

What each command does with `up?` differs, because the safe direction does:

- `sync`, `graph` and a launch **refuse**, naming what the runtime said.
- `down` leaves that session alone and says so, as a row rather than a gap —
  it exits non-zero, and reports *could not be asked* rather than *would not
  stop*, which is a claim omh cannot make about a container it never tried.
- `rm` removes the session anyway and warns that the code graph's entry for it
  was left behind.
- the idle reaper goes the other way on purpose: a session it cannot ask about
  is never stopped for being idle.

**A session that has fallen behind is told what to do about it.** The number
was there long before there was anything to do with it; `sync` is that thing.

```console
$ omh s
```

```text title="omh s"
  s01  omh/s01  up
  s02  omh/s02  stopped  3 uncommitted  (12 behind main)
  s03  omh/s03  stopped                 (how far behind main?)
omh: omh could not measure s03 against main — it may be working against code that moved, and `sync` is not offered over a count that failed
  omh s02 sync  bring main in, merged on the host
  omh s03 log   says why the count could not be taken
```

**The hint is offered only to the sessions it applies to**, and in the spelling
that works on the session as it stands.

**A count omh could not take is not a count of zero.** `s03` renders `(how far
behind main?)` rather than an empty cell, is not offered a sync, and is named
anyway: `omh s03 log` prints the reason git gave.

**Files more than one session is changing are named**:

```console
$ omh s
```

```text title="omh s"
  s01  omh/s01  up       3 uncommitted  (2 behind main)
  s03  omh/s03  stopped  1 uncommitted  (2 behind main)

  s01 and s03 both change src/base.rs, src/render.rs
```

It is part of the answer rather than a warning, so a redirected listing keeps
it. A session omh cannot read says so.

**Half-removed sessions are named too.** `omh s` also names ids that have a
container, a run directory or a **sandbox repository** but no worktree —
sessions removed by a version of omh that only took half of one down.
`omh sNN rm` clears them, and says what it would take with it first.

<details>
<summary>Why <code>up?</code>, a count that failed and an unreadable session are named rather than left blank, and what the collision line costs</summary>

`up?` is not cosmetic. Every command that acts on a container asks this
question first, and a Docker daemon that is down used to answer *not running*
— so `omh sNN sync` believed there was nothing to stop and would have written
over the files of a live agent.

The sync hint goes only where it applies because a suggestion under every row
is one nobody reads. And `sync` refuses while a sandbox is up and names
`--down` itself, so the bare form under a row marked `up` would be a line that
fails when pasted.

*Up to date* and *omh could not count* must never look the same on the surface
where you pick which session to open. A merge advised off a count that failed is
advice built on a guess, so `s03` gets no sync — but it is not passed over in
silence either: beside rows that each carry a next step, saying nothing reads as
*this one is fine*.

The collision line is the one git will not mention until a merge, said while
both sessions are open and either could still be redirected. Absence from the
collision section otherwise means *collides with nobody*, and
an empty section is how "no collisions" is rendered — so a partial answer would
be indistinguishable from a clean one.

The collision line costs nothing: the paths and the uncommitted count are one
`status` per session, asked once — read separately they were two subprocesses
and, worse, two snapshots of a worktree an agent is writing into.

</details>

<details>
<summary>Why there is no <code>ls</code> verb, and why typing it still answers</summary>

There is no `ls` verb. It was one until 2026.08, and what made this row
possible is the listing learning a scope; retiring the verb is the smaller and
separate call, so that one thing has one spelling rather than two.

Typing it still says so. The verb is kept as a tombstone because clap's
*unrecognized subcommand* does not name what replaced it, and this does. It was
once kept for a stronger reason — the line used to fall through to a top-level
`ls` and quietly list every session — but that spelling is gone, so the line no
longer parses either way.

</details>

### `omh sNN resume` — rejoin it as what it was

`omh new` starts a session; `resume` rejoins one, running **the harness it ran
before**.

```
omh sNN resume [harness] [-- args…]
```

| | |
|---|---|
| `[harness]` | rejoin as this harness instead, and record it |
| `-- args…` | arguments for the harness, as on `omh new` |

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

**No record** covers a session older than this release, one whose run directory
was cleared, and one `omh sNN attach <editor>` created — attaching an editor
makes a session without running a harness in it, so there is nothing to record.
**A record omh cannot read** is a marker omh wrote and cannot read now, which is
different news and says so.

**Naming a harness that differs is a switch.** `omh sNN resume <harness>` names
one instead. If it is the one already recorded, that is still a resume. If it is
a different one, an image is built per harness, so the sandbox stops and starts
on the other one, and the record is rewritten so every later `resume` follows.
omh says so rather than leaving you to notice:

```console
$ omh s01 resume claude
```

```text title="omh s01 resume claude"
omh: s01 was running omp; resuming as claude restarts its sandbox and records claude
claude on omh/s01
```

**omh records the harness at launch**, beside the session's last-used marker, and
`omh sNN rm` takes it away with the rest of the run directory.

<details>
<summary>Why omh keeps a record rather than reading staging, or guessing</summary>

The staging directory carries the name too — `run/<repo>/<id>/<harness>/`
survives `down` — but a session that has run two harnesses has two of them and
no way to say which was last, and staging is a side effect nothing promises to
keep. The marker is the answer omh means.

**Guessing would be easy and wrong.** omh knows which harness this host
prefers, and answering with it would attach claude to a worktree an afternoon
of opencode built, with nothing on screen to say so.

</details>

### `omh sNN attach [editor]` · `omh sNN a`

Opens the session in an editor over SSH.

```
omh sNN attach [editor]
```

| | | Default |
|---|---|---|
| `[editor]` | an editor omh has a recipe for — `omh info` lists them | `$OMH_EDITOR`, then `$EDITOR`, then every recipe printed |

```console
$ omh s01 attach
```

```text title="omh s01 attach"
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

**An editor that is not installed is not an error** — omh says so and prints the
URL. See [Editors](/guides/editors/).

**You do not need an editor to get a shell**: the `ssh` alias above reaches the
session directly, and `omh sNN` prints it as a next action while the session is
running. `ssh omh-<repo>-sNN` lands you in `/work` as the agent.

**It rejoins the harness the session recorded** — not whatever this host
prefers. A session whose harness record is damaged is refused rather than
reattached on a guess, and `omh sNN resume <harness>` is how you say which one.

<details>
<summary>Why attach does not use this host's harness</summary>

Attaching from a machine set up for one harness to a session another built would
otherwise stop its container and start the wrong image over the same worktree.

</details>

### `omh sNN` shows what the agent did and what it cost

Scoped to one session, `omh sNN` reads the harness transcript and reports what
produced the diff you are about to review:

```console
$ omh s01
```

```text title="omh s01"
s01   claude   up   3 uncommitted

  47 turns, 128 tool calls, 12 files touched · $2.14
  checks: passed (3)
  ssh omh-ohmyharness-3f9a2c1b-s01   open a shell in the session
```

**It never reaches into the container.** The transcript is mounted per session
under `~/.omh`.

**No transcript is not an empty session.** A harness that does not record a
transcript omh can read says **activity not recorded** rather than a fabricated
empty session, and a transcript omh opens and cannot parse says **could not
read** — never *0 turns*.

**Claude Code's transcripts and Codex's rollouts are both read**; for Codex,
cached input counts once, as a cache read.

**Cost is summed per model from a dated price table.** A model omh has no price
for — every Codex model, today — reports its tokens and **cost unknown**, never
`$0`. All of it is in `--json`.

### `omh sNN` shows which hooks fired

Every hook rendered for a launch writes one decision — fired, silent or
refused — to that session's own events file, and `omh sNN` reads it back as a
`hooks this session` block:

```console
$ omh s01
```

```text title="omh s01"
  s01  omh/s01  stopped  1 uncommitted

  activity not recorded — no transcript written yet
  hooks this session
    graph-orient   1 fired
    graph-read   1 fired, 14 silent
    dormant        graph-first, git-turn
```

**Fired and silent land on the same line** when a hook did both across the
session — `graph-read` narrows to `Read`, so it is checked far more often than
it has anything to say.

**A hook that rendered and never once fired is named under dormant**, not
omitted — silence there would read exactly like the hook was never part of the
launch.

**A harness with no hooks capability at all says not recorded**, never
`0 fired`: that would misleadingly read as guards that ran and found nothing.

**`omh eject` carries none of this** — the wrapper that writes it is spliced in
only when a real launch is staging the events file, never into a config handed
to you.

Two more lines appear only when something is wrong, and both are in `--json`
alongside `hooks`, `dormant` and `unreadable`:

```console
$ omh s01
```

```text title="omh s01"
  s01  omh/s01  stopped

  hooks this session
    graph-read   2 fired
  tdd-guard reported activity (2 fired) this launch never rendered
  1 event line could not be read
```

**unlisted** — *"reported activity … this launch never rendered"* — is a name
in the events file that this launch's own ledger does not contain. It is shown
apart from the counts omh vouches for, and never folded into them. Expect it in
one of two cases — something in the sandbox invented the name, or a renderer
rendered a hook and failed to record it. What it does *not* mean any more is a
leftover from an earlier launch: a launch now starts its own events file
empty, so the two halves describe the same window.

**unreadable** counts lines omh could not parse as an observation, including
any that is not valid UTF-8. They are counted, never dropped, and never
silently rounded to nothing: a damaged record and a quiet launch must not look
alike. If the record is damaged past reading altogether, the block says
**could not read** and names the file and the reason; if the launch wrote a
ledger but its events file is missing entirely — a shadow gitdir that never
mounted — it says so rather than reporting every hook as dormant, which would
assert they ran and never fired.

<details>
<summary>How far the counts can be trusted: the ledger, the events file, and the limit of the split</summary>

The two halves are deliberately unequal in trust: omh writes the ledger itself,
on the host, before the container starts, while the events file lives in the
sandbox where the agent can write to it. That is why an unlisted name stays out
of the counts.

That trust split is bounded, and worth being plain about: it keeps *unrendered*
names out of the counts. Within the names a launch did render, an observation
is still the sandbox's own claim about itself — the guest is handed every
rendered hook name in its settings document, so a hook's `run` could write a
line under any of them.

</details>

<details>
<summary>What recording a hook costs — measured 2026-09-09</summary>

The write itself is cheap enough to leave unconditional rather than gate
behind a setting: measured 2026-09-09, one append averages ~0.65ms in the
shell wrapper claude's hooks run through (200 appends in a loop) and ~0.03ms
through `node`'s `appendFileSync` (opencode, omp) — against `git-turn`'s own
~80ms per turn, the heaviest thing omh already does on every tool call.

</details>

### `omh sNN log` — what the agent has committed

Lists what the agent has committed inside the sandbox, newest first.

```
omh sNN log [--turns]
```

| | |
|---|---|
| `--turns` | omh's snapshot of the tree at the end of each turn instead — [below](#omh-snn-log---turns--what-the-agent-did-when-it-never-committed) |

```console
$ omh s01 log
```

```text title="omh s01 log"
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

**The uncommitted line is measured in the sandbox**, where `--keep` measures it:
that is the work about to be swept into a *Work in progress* commit, shown
before it happens rather than after.

**Two things are said rather than shown.** A merge reads `merge` instead of a
file count, because git has no diff for one until you name a parent and *0
files* is a measurement. Files git will not count lines for — binary, or
anything the agent marked `-diff` — read `·N`, never a blank.

**A harvest `--keep` would refuse is not offered.** When the sandbox is in a
state `--keep` would refuse — commits on a branch it wandered off, or a rewind
below the last handover — the log says so on stderr and does not offer a
harvest it knows would be refused.

**A session whose sandbox has never run says so and exits 0** — asking to see
the agent's work before the agent has run is an ordinary thing to do.

<details>
<summary>Why this command exists</summary>

Until it existed you could not tell the agent had been committing at all: its
work first became visible when `omh sNN commit --keep` opened a rebase todo,
which is late to find out there are eleven of them.

</details>

### `omh sNN log --turns` — what the agent did, when it never committed

Most agents never run `git commit`, so `omh s01 log` has nothing to show for a
session you can plainly see changed things. omh photographs the tree at the end
of every turn, and this reads those.

```
omh sNN log --turns
```

```console
$ omh s01 log --turns
```

```text title="omh s01 log --turns"
s01 · 3 turns

  ~0  4m   turn end  7 files  +220 −61
  ~1  35m  turn end  1 file   +9
  ~2  1h   turn end  3 files  +48 −12
```

**These are omh's snapshots, not the agent's commits, and the two lists never
mix.** There are no numbers here for `diff` or `--keep` to take — a row is
identified by the ref that reaches it, `refs/omh/turn~0` being the newest. A
snapshot is a photograph of a tree, not work anybody chose to keep.

**The `turn end` column is omh's own subject** on every genuine snapshot, so
anything else appearing on that ref is visible as the row that reads
differently.

**To put the files back** to how they stood two snapshots ago, inside the
sandbox:

```console
$ git restore --source=refs/omh/turn~2 -- .
```

:::caution[Restore, never reset]
**Not `git reset --hard refs/omh/turn~2`.** The snapshot chain has a root of
its own — it is not built on the session's history — so resetting onto it moves
the branch into omh's commits, and `omh s01 log` then numbers *those* as the
agent's checkpoints. `omh s01 commit --keep` would replant them onto your
branch as the agent's work, which is the one thing this separation exists to
prevent. `restore` puts the files back and leaves the branch alone, which is
what was wanted anyway.
:::

**`~2` counts snapshots, not turns**: a turn that changed nothing does not take
one, so on a quiet session `~2` may be further back than two turns.

**They cost nothing in the agent's context**: the hook that writes them injects
no text. A turn that changed nothing writes no snapshot — though it still costs
the time to find that out — and the hook touches neither `HEAD`, the index nor
the worktree, so after it runs the agent's own `git status` says exactly what
it said before.

**`omh sNN rm` mentions them** when it removes a session, and never refuses over
them.

<details>
<summary>Why snapshots carry no numbers, and why <code>rm</code> does not refuse over them</summary>

That is deliberate, and it is the second attempt: numbering them `3 / 2 / 1`
meant `omh s01 commit --keep 2` quietly replanted the *agent's* checkpoint 2,
a different list, with no error.

There is one for nearly every session that ever ran, and a refusal that fires
almost always is one people answer with `--force` unread.

</details>

### `omh sNN diff` — the shape, the patch, or one checkpoint

Shows what a session changed — as a summary, as the patch, or for one
checkpoint.

```
omh sNN diff [n] [-p]
```

| | | Default |
|---|---|---|
| `[n]` | one checkpoint, by its number in `omh sNN log` | the whole session |
| `-p` `--patch` | the patch itself, through your pager, rather than a summary | |
| `--base <branch>` | compare against this; refused alongside a checkpoint number, which is measured against its own parent | the repo's default branch |

```console
$ omh s01 diff          # what the session changed, as a summary
$ omh s01 diff -p       # the patch, through your pager
$ omh s01 diff 4        # one checkpoint, by its number in `omh s01 log`
$ omh s01 diff 4 -p
```

**`-p` hands the terminal to git**, so it is git's own pager and git's own
colours — the same arrangement `omh sNN commit` uses for your editor.

**Under `--json` it never pages**: the patch is a field in the answer, because
a pager between a script and the object it asked for is a hang with no error.

**A number outside the session's list is refused**, with the range — it is
validated against the session's own list before anything is printed.

**Your pager is whatever git resolves in *your* checkout** — `GIT_PAGER`, then
your `core.pager`, then `PAGER` — so `delta` and friends work here as they do
everywhere else. omh asks git for it rather than guessing at the order. The
sandbox's own git config never gets a say.

<details>
<summary>Why the number is checked, and why omh names a pager at all</summary>

The number is checked not for safety — the sandbox's repository holds nothing
you may not see — but because a command that prints any object you name is a
different command from one that shows you a checkpoint.

The sandbox's git config cannot carry a pager anyway: omh rewrites that file to
a ten-key allowlist on every launch and mounts it read-only. The reason omh has
to name a pager at all is that same hardening — without it, a host-side read is
pinned to no pager at all.

</details>

### Getting work out of a session

`commit` and `push` are how the work comes out, run on the host — the same
place `diff` already runs. The worktree lives under `~/.omh/worktrees/` so your
IDE does not index it, and you should never have to go there.

```
omh sNN commit [-m …] [--skip-carried] [--no-verify] [--allow-conflicts]
omh sNN push [name] [--pr]
```

| | |
|---|---|
| `-m <message>` | the commit message, written **verbatim**. Without it, git opens your editor |
| `--skip-carried` | commit everything but a carried file git tracks — below |
| `--no-verify` | land the work without running this repo's checks — [below](#omh-snn-commit-runs-this-repos-checks-first) |
| `--allow-conflicts` | commit even with conflict markers in the files — [below](#omh-snn-commit-will-not-land-a-conflict) |
| `--keep` | the agent's own commits instead of one — [below](#keeping-the-agents-own-commits) |
| `push` `[name]` | the branch name on origin. Required the first time, remembered after |
| `push` `--pr` | open the pull request with `gh` once it is pushed |

```console
$ omh s01 diff
$ omh s01 commit -m "Fix the tap guard"
$ omh s01 push fix/tap-guard --pr
  omh/s01 → origin/fix/tap-guard
```

**`commit` stages the agent's work and writes your message verbatim** — no
trailer, no generated summary. omh has no view on what the work was for and will
not invent one.

**What omh put in the worktree is not the agent's work and never lands in the
commit.** The rules are mounted rather than written, so git does not see them at
all; carried files are a different matter:

```console
$ omh s01 commit -m "Fix the tap guard"
```

```text title="omh s01 commit"
omh: config.toml is listed in carry_in and git tracks it, so what is in the worktree is your local copy rather than the branch's.
  omh will neither publish that nor drop it silently.

  fix the cause:  omh set carry_in        (carry_in is for files git does not track; a tracked file is already in the worktree)
  or just this once:  omh s01 commit --skip-carried
```

`carry_in` is for files git does **not** track, so this means the list needs
fixing — see [Configuration](/reference/configuration/#keeping-the-agents-git-status-clean).
`--skip-carried` commits everything else meanwhile.

**`push` requires a name the first time and remembers it after**, so omh refuses
rather than choosing for you:

```console
$ omh s01 push
```

```text title="omh s01 push"
omh: omh/s01 is a session id, not a branch name
  name it:  omh s01 push <name>
```

**`--pr` opens the pull request with `gh`** when it is installed, and prints the
command when it is not. It is never a dependency: a repo on a non-GitHub remote
is a normal repo.

<details>
<summary>Why <code>commit</code> refuses a tracked carried file, and why <code>push</code> wants a name</summary>

omh refuses rather than quietly leaving the file out because it cannot tell a
credential you were carrying from a change you meant.

`omh/s01` records when the work happened rather than what it was, and on origin
it outlives the session that would explain it.

</details>

### `omh sNN commit` runs this repo's checks first

Before it lands anything, `omh sNN commit` runs the repo's turn-end hook
commands inside the sandbox — the same checks the agent's own loop runs when a
turn ends — and refuses the commit if one fails, printing what it said:

```console
$ omh s01 commit -m "Fix the tap guard"
```

```text title="omh s01 commit"
omh: `cargo test` failed in the sandbox — the work was not committed
  test tap::guard ... FAILED
omh: a check failed. Fix it, or `omh s01 commit --no-verify` to land anyway
```

**A check is a `turn-end` hook that runs a command**, for a stack this repo has.
The checks run as the agent, in the running sandbox, so the answer is the one
the agent would get — not one your host's toolchain gives.

**`--no-verify` skips them**, as `git commit --no-verify` skips git's.

**When the sandbox is stopped, the checks are reported as not run**, never as
passing — a fact `omh sNN` shows, not a green light. The last result is
recorded so `omh sNN` can show whether the work you are about to push passed
its own checks.

### `omh sNN commit` will not land a conflict

```console
$ omh s01 commit -m "Resolve the merge"
```

```text title="omh s01 commit"
omh: s01 still has 3 conflict markers in its files:
  src/tap.rs:41: leftover conflict marker
  src/tap.rs:43: leftover conflict marker
  src/tap.rs:47: leftover conflict marker
Resolve them first, or:
  omh s01 commit --keep --allow-conflicts   commit them anyway
```

**Both ways of committing refuse**, because both would land them: `-m` stages
the files as they are and `--keep` replants the agent's commits on top of them.
A file the agent created and never added counts — that is the one `-m` would
sweep up.

**`--allow-conflicts` is there because a conflict marker at the start of a line
is not always a conflict.** A test fixture holds them on purpose. It is not
spelled `--force`, which on `rm` answers a question about unreviewed work —
[flags mean one thing everywhere](#flags-mean-one-thing-everywhere).

### Keeping the agent's own commits

The agent commits as it works, into a repository of its own that is not this
branch — see [Sessions](/concepts/sessions/#the-agent-has-git-and-it-is-not-yours).
`--keep` brings those commits here instead of squashing the files into one.

```
omh sNN commit --keep [n,m-o] [--edit]
```

| | |
|---|---|
| `--keep` | every checkpoint since the last handover, in order, and opens nothing |
| `--keep 1,3-4` | those, in that order, by the numbers `omh sNN log` printed |
| `--edit` | open the list, as `git rebase -i` does, so you reorder, reword and drop by hand |

```console
$ omh s01 commit --keep          # all of them, in order, no editor
$ omh s01 commit --keep 1,3-4    # these, in this order
$ omh s01 commit --keep --edit   # the todo, for people who want it
```

**What you keep arrives with the messages the agent wrote** — which is the
point, because it wrote them while it still had the context you would be
reconstructing from a diff.

**It is repeatable**: land some work, let the agent carry on, land the rest.
Each round takes only what the last one did not.

**Every commit is authored `omh sandbox <sandbox@omh.invalid>`**, whatever the
agent set in its own config. The sandbox does not get to say who wrote something
on your branch.

**Not with `-m`.** The two are mutually exclusive.

**`--skip-carried` has no meaning here and is ignored**: `--keep` refuses a
carried file rather than dropping it, for the reason below.

**`--keep` refuses rather than repairing, in these cases and no others:**

- **A carried file reached a commit** — by `git add -f`, copied under another
  name, pasted into source, or written into a message. It will not rewrite your
  history to hide a secret. Drop the commit in the sandbox and harvest again.
  What it searches for is below.
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
  `omh sNN commit -m`.

**The branch is untouched in every case, and nothing is lost in any of them**:
omh fetches the sandbox's work into your repository before it replants, so even
a refusal that happens after the fetch leaves the commits reachable there.

<details>
<summary>How <code>--keep</code> recognises a carried file inside a commit, and what it cannot catch</summary>

omh knows the bytes it carried in, so it can tell: every line of the file, and
the value on it — what follows the first `=` or `:`, quotes off — when that
value is twelve characters or more, holds no whitespace, and is not a number or
a word every configuration file has (`true`, `localhost`, `development`). A
shorter or commoner value is not searched for, so `PORT=8080` does not refuse
every harvest, and a secret that short is not one this scan can protect.

</details>

<details>
<summary>Why <code>--keep</code> and <code>-m</code> cannot be combined, and a correction to an earlier version of this page</summary>

Running both would put the squashed content on the branch first, and git's
patch-id then drops the replanted commits as already applied — the granular
history gone, with nothing said.

An earlier version of this said the first two refusals happen before the work
leaves the sandbox. They do not — the carried-file scan reads the fetched
commits, which is how it can see them at all.

</details>

### `omh sNN commit --keep` — the agent's own commits, curated

**The numbers are the ones `omh s01 log` printed, and the order is yours** —
`3,1` lands the third checkpoint and then the first. Reordering is half of what
curating a history is for, so a selection is never quietly sorted.

**Refused rather than resolved, and refused before anything moves**: a number
outside the session's range, a range that runs backwards, a checkpoint named
twice, and a number naming work the branch already has. That last one is
ordinary to type — `log` numbers every checkpoint, including the ones below the
line — and replaying it would apply the same patch a second time.

**`--edit` is the only form that needs a terminal**, and it says so when there
isn't one. Without that check, git runs the unedited list, exits 0, and omh
reports a curation that never happened.

### `omh sNN sync` — trunk moved, and the session catches up

Brings trunk into a session — or, with no session named, into every one — merged
on the host.

```
omh sNN sync [--down] [--base <branch>]
omh s sync [--down]     bring trunk into every session, merged on the host
```

| | | Default |
|---|---|---|
| `--down` | stop the sandbox first, rather than refusing because it is up | |
| `--base <branch>` | merge against this instead | the repo's default branch |

```console
$ omh s01 sync
```

```text title="omh s01 sync"
s01 · 3 commits from main

  1 file needs resolving:
    src/tap.rs

  omh s01 log             the checkpoint this can be undone from
  omh s01 resume          the markers are in the sandbox, where fixing them cannot hurt you
```

**Your checkout's commits never enter the sandbox's repository.** The merge runs
**on the host**, in your repository, and the session receives files — so the
isolation the sandbox exists for survives a sync.

**What the agent finds when it starts again:**

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

**The conflict markers name the sides** — `<<<<<<< main` and `>>>>>>> s01` —
rather than two object ids.

**`omh s sync` with no session named syncs every session**, stopping at the
first that needs a person — a conflict to resolve, or an error — and naming the
ones it did not reach. Trunk moves once; you bring it into all of them without a
command each. Naming a session (`omh s01 sync`) scopes it to that one. A
conflict stops the run rather than piling up behind the later syncs; resolve it
and run `omh s sync` again.

**It refuses while the sandbox is up**, and `--down` stops it first.

**`omh s01 diff` still shows the agent's work after a sync**, not trunk's — the
session's baseline moves with it.

**A file trunk deleted is deleted from the session too**, so the worktree holds
the merged tree and nothing more. A file the agent changed and trunk deleted is
a conflict like any other: the agent's version stays, uncommitted, and is named
in the report. Carried files and omh's own placeholders are never removed,
whatever trunk did.

**It needs a host git whose `merge-tree` takes `--merge-base`** — git 2.39.5
does not. `omh doctor` asks the binary and says so if yours cannot.

<details>
<summary>Why a running sandbox is refused, and why a deleted file is removed</summary>

Not about the files — the checkpoint makes an overwrite recoverable. It is about
what the agent *believes* the tree contains, which lives in its conversation and
not on disk: left running, it edits a version that no longer exists and trunk's
changes disappear inside a plausible patch. Stopping is the fix, not the price;
the harness restarts and reads the tree as it now is.

A session that kept a file trunk deleted would show it under `diff` as the
agent's own addition and land it again at `commit`.

</details>

### `omh sNN down` — stop a sandbox, keep its work

Stops a session's sandbox. Its worktree and branch survive, so the stop is
reversible.

```
omh sNN down            stop that one
omh s down              stop every container, keep the worktrees and branches
```

**With no session named it stops every sandbox, and asks first** when there is a
terminal to ask at. A pipe or a CI runner has nobody to answer, and there it
proceeds without asking. A declined prompt stops nothing.

**A sandbox omh could not ask about is left alone**, and said so — see
[`up?`](#omh-sessions---s).

<details>
<summary>Why a wide <code>down</code> asks, and why a script is not asked</summary>

`down` with no session stops every sandbox — the one place acting on all of
them is what you mean, and the one command whose blast radius grows with how
much work is in flight. So it asks first when there is a terminal to ask at.
Without one it proceeds: the stop is reversible — the worktree and branch
survive — and omitting the session is the explicit way to mean all, so a script
that did so meant it.

</details>

### `omh sNN rm` — and what it refuses to take with it

Removes the session — its container, its worktree, its staging, and the
repository the sandbox had. Asks first over work no branch has.

```
omh sNN rm [--yes]
```

| | |
|---|---|
| `--yes` `-y` | answer the question about unreviewed work in advance, for runs with nobody to ask |

**It counts the agent's commits before it removes anything.** The session's
branch survives a removal and its files were on disk until it ran. The agent's
own commits are the exception: they live only in the sandbox's repository, and
`rm` deletes that. After a `git reset --hard` in the sandbox they were the only
copies there ever were.

So `rm` counts them first. On a terminal it asks; with nobody to ask — a
script, a CI job, a closed pipe — it refuses and names every way out:

```console
$ omh s01 rm
```

```text title="omh s01 rm"
omh: s01 has 2 commits that no branch has. Removing it deletes the only copy:
  omh s01 log                 read what is there
  omh s01 commit --keep       put it on omh/s01
  omh s01 commit -m "…"       or take the files as they stand
  omh s01 rm --yes            remove it anyway
```

Nothing is taken down before that refusal — not the container, not the marker
`omh s` reads, not the repository the refusal is about.

**`--yes` is the way past that question, and only that.** It does not make
the removal itself try harder: `git worktree remove --force` is passed either
way, so a worktree that will not go will not go with `--yes` either.

**The count is wider than the one `omh s01 log` prints**, on purpose. `log`
numbers what you can act on; this asks whether anything in that repository
exists nowhere else, however the agent left it. Work thrown away with
`reset --hard` is still in there, and so is a branch the agent wandered off —
neither appears in the numbered list, and both are gone once the repository is.

**A sandbox that never ran removes quietly. One omh cannot read does not**:
that is a third answer, not a quiet yes, and it is the state a half-finished
removal leaves behind. `--yes` covers it too, so nobody is stuck — they are
asked once.

**An id with no session is refused.** `rm` refuses an id this checkout has no
session for — no worktree, no sandbox repository, no run directory — rather
than reporting a removal of nothing. A stray `omh/<id>` branch is named as
information, never under a removal claim and never with the `git branch -D` a
real removal's leftover offers.

<details>
<summary>Why a mistyped id must not print a success line</summary>

Absence and completion are different answers, and a mistyped id must not print
a success line. Naming a stray branch without the `git branch -D` means a typo
cannot hand you a command that destroys unreviewed work.

</details>

#### What happens to the session's branch

**`rm` never deletes a branch holding work that exists nowhere else.**
Unreviewed agent work must be unloseable, so the branch outlives the session
that produced it. What `rm` does with it:

| The branch | `rm` | and says |
|---|---|---|
| holds commits trunk does not have | keeps it | `branch omh/s01 kept (3 commits to review)` |
| has no commits | drops it | |
| holds work already on trunk | drops it, and names where | `dropped — its work is on main as e15d33c` |
| cannot be counted against trunk | keeps it | `kept — omh could not count it against main` |
| was counted, but whether it landed could not be settled | keeps it | `kept (3 commits; omh could not tell whether they are already on main)` |
| git refuses to delete it | keeps it | `kept — git would not delete it: <git's words>` |

A branch it keeps comes with how to review or discard it:

```console
$ omh s01 rm
```

```text title="omh s01 rm"
removed session s01; branch omh/s01 kept (3 commits to review)
  git log main..omh/s01
  git branch -D omh/s01
```

The summary is the answer and goes to stdout; the two commands are next steps
and go to stderr, so they reach you here and stay out of anything you redirect.

**A branch whose work is already on trunk is dropped too, and says where.** omh
compares the branch's tree against trunk's commits since it forked and, when
the trees settle nothing, the branch's whole change against each of trunk's
own:

```console
$ omh s
  s01  omh/s01  stopped  landed e15d33c  (1 behind main)

$ omh s01 rm
removed session s01; branch omh/s01 dropped — its work is on main as e15d33c
```

A squash that had to **resolve a conflict** is not this branch's work — neither its tree nor its patch is what landed — so that
branch is kept, which is the only direction this is allowed to be wrong in.

**A branch omh cannot *read* is kept too, and says so** — that read is one
`git log --left-right <base>...<branch>`, and it has no answer in a checkout
whose default branch exists only as `origin/<name>`:

```console
$ omh s01 rm
```

```text title="omh s01 rm"
removed session s01; branch omh/s01 kept — omh could not count it against main
  git log omh/s01
  git branch -D omh/s01
```

When the count came back and only the *landing* could not be settled, the count
is said rather than thrown away. **A delete that git refuses is its own answer**,
not a landing omh could not settle.

<details>
<summary>Why a branch is dropped only when keeping it would preserve nothing</summary>

Dropping a branch is irreversible and justified by one fact only — that keeping
it would preserve nothing — so anything short of that fact falls the other way.

A branch with no commits meets it: keeping it preserved nothing —
`worktree remove --force` has already discarded anything uncommitted — while a
namespace filling with dead refs trains you to ignore the ones that matter.

A landed branch's commit is named because a deletion justified by *it is
already on trunk* is only checkable if omh says where.

The next steps carry no `review with` label because a suggested command is
reproduced exactly — a line you cannot select and paste is worse than no line.

</details>

<details>
<summary>How omh proves a branch's work already landed — squash merges, patch-ids and the 500-commit window</summary>

Counting commits is a question about *ancestry*, and a squash merge — the
default on GitHub, and this project's own merge button — gives the work a new
sha with different parents. So a branch whose content landed a month ago still
counts two commits, and omh kept it: measured here on `omh/s02`, whose work
reached `main` as `58acbaa` and whose branch outlived it by a month. That is why
omh compares trees and patches rather than counting.

**A patch that matches is then checked byte for byte.** `git patch-id` finds the
candidate cheaply, and it is not evidence of sameness on its own: it strips
whitespace, so a Makefile recipe indented with a tab and the same line indented
with spaces share an id, and it tells two binaries at one path apart only on git
2.39 and later. What settles it is the two patches being the same bytes, which
holds on every version — so a whitespace-only difference keeps the branch.

omh reads the **first 500 commits trunk gained after this branch forked** —
where a session's own squash almost always lands — and past that it says it
could not tell rather than that nothing landed. A branch that forked long ago
and was merged much later is the case that answers *could not tell*.

What the proof says is that **trunk's history** holds this content, not that
trunk's tip still does: work that landed and was then reverted still reads as
landed, and the branch goes. The commit named is where it is, and `git show` on
it is the recovery. Deleting a branch is also not the end of it — `git reflog`
keeps the tip for 30 days.

</details>

<details>
<summary>The JSON a removal reports: <code>landed</code>, <code>branch_kept</code>, <code>left_behind</code> and the rest</summary>

Under `--json`, `landed` carries the commit when there is a proof and
`landed_unknown` carries the reason when there is not; never both, and the pair
travels on `omh sNN commit` too — where it is always `null`, because `commit`
takes the cheap look only and never asks the expensive question. `delete_refused`
and `delete_unconfirmed` carry git's words for the two ways the deletion itself
can go wrong.

When git refuses the delete, the proof omh acted on is still in `--json` under
`landed`. When the delete ran and omh could not re-read the branch afterwards,
`branch_kept` is `null` rather than `false`: omh does not know, and a `false`
would be a claim about a ref it never managed to look at.

`left_behind` lists what the removal could not take with it — see below.

</details>

#### A removal that did not finish

**Something left behind is named, and the removal still succeeds.**
`left_behind` lists what the removal could not take with it — the sandbox's
repository, the run directory, the container — each with the reason, and is `[]`
when everything went. The session is still removed and the command still
succeeds; the same line is printed under the summary, and `omh sNN rm` again
clears what stayed once the reason is gone.

**A worktree that survives is a failure, not a footnote.** `rm` asks the disk
whether the worktree is actually gone rather than trusting git's exit code.
When something survives, the command exits non-zero and reports what it
observed:

```console
$ omh s01 rm --yes
```

```text title="omh s01 rm --yes"
omh: s01 is partly removed — its worktree is still there:
  ~/.omh/worktrees/repo-5a4ec022/s01
  Permission denied (os error 13)
what went: the container, the sandbox repository
the branch omh/s01 is still there.
  omh s01 rm              run it again once the directory is free
```

The list is what omh **watched happen**, part by part — not a summary of what
it set out to do. A step it never reached, because there was no usable
container runtime or because the daemon would not answer, is named separately
rather than counted as done.

<details>
<summary>Why omh checks the disk rather than git's exit code</summary>

A git that exits 0 while leaving the directory behind used to be believed —
`removed session s01`, exit 0, worktree still there.

</details>

## `omh graph [--stop]`

Serves the code graph for this repo in a browser.

```
omh graph [--stop]
```

| | |
|---|---|
| `--stop` | stop the graph server; the session keeps running |

```console
$ omh graph
```

```text title="omh graph"
omh: graph at http://127.0.0.1:56286
  every session's graph for this repo, in one place
  stop with: omh graph --stop
```

**One service per repo, not per session**, and it needs no session to exist.
See [Code graph](/guides/code-graph/#omh-graph).

## `omh info`

Everything omh knows about here: harnesses, editors, which sessions exist, and
what your catalogue holds per capability.

```
omh info [--repo]
```

| | |
|---|---|
| `--repo` | this checkout instead — [`omh info --repo`](#omh-info---repo) |

**What each session is *doing* is `omh s`** — this one never asks git, so it
costs no subprocess per session.

**`--repo` narrows the same question** from the machine to this checkout; the
two are one command because *what you have* is one question asked at two
scopes.

## `omh info --repo`

**This checkout.** What it uses, what it decided, and what decided it.

```
omh info --repo                       effective here, with provenance
omh info --repo --json                the same, for a script
```

```console
$ omh info --repo
```

```text title="omh info --repo"
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

**It reports rather than writes.** What changes a value here is
[`omh set` and `omh unset`](#omh-set--omh-unset), and what changes a selection
is [`omh use` and `omh unuse`](#omh-use--omh-unuse).

**`keyed as`** is what omh calls this checkout's state: the directory under
`~/.omh/worktrees`, the cache volume, and the name of every container it
starts. It is the checkout's basename plus a digest of where the checkout is.
`--json` reports it as `repo_id`, which is how to find out which `docker ps`
row belongs to which project.

**`everything` is an absent `[use]` list** — a new checkout follows your
catalogue as it grows. `· +N from omh's features` is what the base set adds on
top, and it is counted separately because those are not yours to select: `omh set
<feature> off` is their switch. With `codegraph` off here, its rules section,
its server and its hooks are the ones missing from those counts.

<details>
<summary>Why the key carries a digest of the path</summary>

Two projects called `api` are two projects — before 2026.08 they shared one key,
and the second one's `omh new` resumed into the first one's session.

</details>

## `omh why <thing>`

Who put this here, and on what grounds. Needs no session and no container.

```
omh why <thing>
```

| | |
|---|---|
| `<thing>` | a base-set entry, something you added, something omh rejected, or a settings key |

```console
$ omh why codegraph
```

```text title="omh why codegraph"
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
[measure the cost, argue the benefit](/concepts/trust/#measure-the-cost-argue-the-benefit).

**A name matching nothing prints what *is* known** rather than guessing — the
same rule as `omh s01 attach emacs`.

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

## `omh set` · `omh unset`

Change a setting, or switch one of omh's features. **Which file it lands in
follows from the key**, not from a flag you have to remember.

```
omh set <key> <value> [--local|--save] [--anyway]
omh set account <harness>:<name>      an account for one harness alone
omh set <feature> on|off
omh unset <key> [--local|--save]
omh unset account <harness>           drop that harness's own account
omh unset <feature>                   let omh's own default decide again
```

| | |
|---|---|
| `--save` | write the committed `settings.toml`, whatever the key is |
| `--local` | write the gitignored `settings.local.toml`, whatever the key is |

```console
$ omh set carry_in '[".env"]'
omh: nothing was ignoring settings.local.toml — added it to /Users/you/proj/.omh/.gitignore
wrote → /Users/you/proj/.omh/settings.local.toml (gitignored)

$ omh set idle_timeout 30m
wrote → /Users/you/proj/.omh/settings.toml (committed)
```

**Which file**, in one rule that `omh use` / `omh unuse` follow too:

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

**The word in brackets is the point**: `settings.toml` and `settings.local.toml`
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

**`omh set` also makes its own premise true**: the gitignored file is only safe
because git ignores it, and that line used to be `omh init`'s alone. If nothing
is ignoring it, `omh set` adds the line and says it did.

**`omh unset` removes the key from every repo file that holds it:**

```console
$ omh unset carry_in
```

```text title="omh unset carry_in"
removed carry_in from the shared layer
removed carry_in from the local layer
```

An unqualified `unset` stays inside the repo, and a named flag touches one file
on purpose — so whatever still supplies the value afterwards is named.

<details>
<summary>Why <code>unset</code> reaches every file rather than the one <code>set</code> would write</summary>

*Where would `set` put this* and *where is this set* are different questions,
and answering the second with the first left a committed `carry_in` standing
while reporting success.

</details>

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

**Switched off, and *following omh's default*, are different states** — which is
why `unset` exists for a feature at all.

**Naming an entry is refused**, saying which feature contains it:

```console
$ omh set graph-rules off
```

```text title="omh set graph-rules off"
omh: `graph-rules` is part of the `codegraph` feature, not a feature itself. A feature is all or nothing.
  omh set codegraph off
```

<details>
<summary>Why settings keys and feature names can never collide, and why entry names may</summary>

Settings keys never collide with feature names, and a test fails the build if
they ever do: `omh set <name>` writes two different shapes into one file, so a
name meaning both could not be resolved by any error message. Feature and
*entry* names do overlap on purpose — the manifest names a feature after its
principal entry — and the feature reading wins.

</details>

## `omh use` · `omh unuse`

Which catalogue entries this project takes.

```
omh use <capability> <name>           → <repo>/.omh/settings.toml, committed
omh unuse <capability> <name>
omh use --all                         resync every list to the whole catalogue
```

| | |
|---|---|
| `<capability>` | `rules`, `skills`, `mcp`, `commands`, `subagents` or `hooks` |
| `<name>` | the catalogue entry. Without it, `omh use` takes everything the catalogue has of that kind |
| `--all` | resync every list to the whole catalogue |

**A capability with no list is following the whole catalogue**, so the first
`omh use` on it writes the catalogue out rather than narrowing to one name, and
says so:

```console
$ omh use skills review-diff
```

```text title="omh use skills review-diff"
skills was following your whole catalogue; wrote its 3 entries as the list
using skills/review-diff — wrote → /Users/you/proj/.omh/settings.toml
```

**It writes the committed file** — what a project uses is a fact about the
project, and a teammate cloning it should get the same selection. `omh set` has
the same default for the same reason; what must *not* be committable by accident
is decided per key, in `src/key.rs`, rather than by which command you reached
for.

**A name that is not there is refused.** `unuse` refuses a name this repo never
used, rather than writing the list back and reporting success for a typo.
`omh use` refuses one your catalogue does not have, and names `omh settings
edit` as the way to create it.

**omh's own are not selectable in either direction** — `codegraph`, `memory`,
the six generated hooks and their rules sections. `omh set <feature> on|off` is
their switch, because a feature is all or nothing. See
[Configuration](/reference/configuration/#a-feature-is-not-selectable).

**A guard is never turned on for you.** A guard — a hook that refuses rather
than advises — is a catalogue entry like any other and `omh use hooks tdd-guard`
finds it, but neither `omh init` nor `omh use --all` ever turns one on for you:
every other catalogue hook is safe to run unattended, and a guard can refuse the
agent's first edit. See
[Guards, and their bypass](/reference/configuration/#guards-and-their-bypass).

## `omh settings`

**You**, before a repo exists. `~/.omh/default.toml` is the template `omh init`
seeds a new repo's `settings.toml` from — read once, at `init`, and never at
launch.

```
omh settings                          at a terminal, a screen; anywhere else, the report
omh settings set <key> <value> [--anyway]
                                      → ~/.omh/default.toml
omh settings unset <key>              remove one of your defaults
omh settings set <feature> on|off     switch one of omh's features, for new repos
omh settings use <capability> <name>  have new repos take an entry of your catalogue
omh settings unuse <capability> <name>
                                      have new repos leave one out
omh settings use <capability> --all   have new repos follow your catalogue for a kind again
omh settings edit [<capability> [name]]
                                      $EDITOR on your defaults, or on one entry
omh settings rm <capability> <name> [--yes]
                                      take one entry out of your catalogue

omh settings mcp ls
omh settings mcp add [--replace] <name> <cmd> [args…] [--env K=V]
omh settings mcp import <harness> [--from <path>] [--replace]
```

| | |
|---|---|
| `--yes` `-y` | remove without asking. Off a terminal there is nobody to ask, so `rm` refuses without it |
| `--env K=V` | an environment variable the server runs with. Repeat for several |
| `--from <path>` | read this file instead of where the adapter says the harness keeps its servers |
| `--replace` | overwrite a server of the same name. Without it `mcp add` refuses one, naming what it runs, and `mcp import` keeps yours and reports the difference. omh's own servers are refused either way |

```console
$ omh settings set idle_timeout 45m
wrote → /Users/you/.omh/default.toml (seeds new repos)

$ omh settings
your defaults /Users/you/.omh/default.toml
  idle_timeout  45m

omh also reads
  carry_in        Untracked files a session needs — a worktree holds tracked files only. The one path by which a secret reaches the agent, so keep it short.
  account         Which captured login a session is launched with, by name.
  sandbox_memory  How much memory a session's sandbox may use, as the runtime spells it (`4g`, `512m`). Unset means the runtime's default.
  sandbox_cpus    How many CPUs a session's sandbox may use, as the runtime spells it (`2`, `1.5`). Unset means the runtime's default.
  runtime         Which runtime builds and runs the sandbox. Unset means `auto`.
  ca_cert         A CA certificate the sandbox must trust, as a path to a PEM — for a network that inspects TLS and signs it with its own root.
  persistence     How a session's terminal survives detaching. Unset means `dtach`.

your catalogue
  rules        0
  skills       1  new repos get all
  servers      0
  commands     0
  subagents    0
  hooks        0
```

**The second list is the useful half**: every key omh reads, which no settings
file can show. Then your catalogue, page by page, as the screen has it: how
many, what new repos get, and every entry that is wrong — a link to nothing, a
skill no harness loads — and, beside a key nothing reads, the one it probably
meant. `--json` carries the same.

**At a terminal, `omh settings` is a screen** over the same `~/.omh`: your
defaults, then one page per kind of catalogue entry — rules, skills, servers,
commands, subagents, hooks. Piped, redirected, or with `--json` or `--dry-run`,
it is the report above.

From 100 columns it is three: the pages, the list, and what the selected row
is. From 70, what the row is sits under the list, as below; narrower, the
pages are one line along the top.

```
 omh settings  ~/.omh
▌ 1 Defaults      │ Defaults  8
  2 Rules       1 │
  3 Skills      2 │   carry_in        default: nothing carried
  4 Servers     1 │   account         default: no login captured — `omh auth <…
  5 Commands    0 │ ✓ idle_timeout    45m
  6 Subagents   0 │   sandbox_memory  default: the runtime's own
  7 Hooks       0 │   sandbox_cpus    default: the runtime's own
                  │   runtime         default: auto
                  │   ca_cert         default: no extra CA
                  │   persistence     default: dtach
                  │
                  │
                  ├─────────────────────────────────────────────────────────────
                  │ idle_timeout
                  │
                  │ How long a session may sit unused before omh stops it.
                  │
                  │ now     45m
                  │ a duration: 90s, 30m, 2h, 1d
                  │
 ✓ wrote → /Users/you/.omh/default.toml (seeds new repos)
   $ omh settings set idle_timeout 45m
 ↵ set  d unset  e edit the file  / filter  ⇥ page  ↑↓ move       ? help  q quit
```

| | |
|---|---|
| `↓` `↑` (`j` `k`), `home` `end` (`g` `G`) | move in the list |
| `⇥` `⇧⇥` or `→` `←` (`l` `h`), `1`–`7` | change page |
| letters, in a list to pick from | go to the option they spell — `pod` is `podman`; `esc` or `q` closes it, and no other key does |
| `enter` | set a default — picked from its choices where it has them, typed where it does not; open a rule, skill, command, subagent or hook in `$EDITOR` |
| `e` | `$EDITOR` on the entry, or on the file a page lives in |
| `a` | a new entry, opened in `$EDITOR`; a new server, asked for in one form |
| `d` | unset a default; remove an entry or a server, after asking |
| `i` | import this kind from a harness whose adapter says where it keeps them — the pick says where each reads, and when nothing is there |
| `space` | whether the repos `omh init` sets up after get the entry: `●` they do, `○` they do not |
| `/` | search every page by name, description or value — the rail counts what each holds; `esc` takes it off |
| `u` | undo the last change that has an exact inverse: a value set back, an unset put back, a `space` flipped back — not one that wrote a guard's kind as a list, or took a rule off the middle of its list |
| `?` | every key, on the screen — `j` `k` scroll it where it is cut, and `esc`, `q` or `?` close it |
| `ctrl-l` | read your catalogue again, for a file changed outside the screen |
| `q` `esc` `ctrl-d` `ctrl-c` | leave |

**A letter held with Ctrl or Alt is not that letter**: Ctrl-D leaves, as it
leaves a shell, and does not unset the default under the cursor; Ctrl-L reads
again, as it redraws one. In a line
being typed, Ctrl-A, Ctrl-E and Ctrl-U go to its start, go to its end, and
clear what is before the cursor.

**The mouse selects a row, opens it with a second click, scrolls the list and
changes page.** The screen holds the mouse while it is drawn, and gives it back
before `$EDITOR` runs and when it ends — so selecting text to copy takes the
key your terminal uses to get past a program's mouse: Shift in most, Option in
iTerm2.

**A row says what it is**: a default, its value or, unset, what applies
instead; an entry, its description; a hook, when it runs — as a column of its
own — and what it does; a server, what it runs, its arguments counted and its
environment named. Hooks are yours first, then omh's own, under a heading each.
The servers page is yours alone: omh's own servers — `codegraph`, `memory` —
are parts of its features, switched with their hooks and rules on your defaults
page and listed nowhere else. A key nothing reads is under one of its own, naming the key it was
probably meant to be, and a skill with no `SKILL.md` or a link to nothing is
marked on its row.

**The rail says what a page holds before you go to it**: its number, how many,
`!` where an entry has a warning and `✗` where omh could not look — whose
count is `—`, not `0`. **The foot lists the keys that act here**, the row's
first: one that would only be refused is not there — no `d` on omh's own,
no `i` where nothing imports — and while a box is open, the box's own. `?`
lists every key. A list longer than its room has a thumb at its edge, and keeps
the heading of the group you are in at the top.

**When it closes, the screen says what it ran** — each command as a shell
runs it, `--yes` on a removal included — where scrollback keeps them: to read
back, or to paste into a script. A server added on the screen is the one line
that is not whole: its env values and arguments can be a token, so they are
held back, and that is said. A
key nothing reads that was probably meant to be another moves its value there
on Enter, `omh settings set` then `omh settings unset`.

**Every change is a command, run as one.** The screen has no write of its
own: a key becomes the line you would have typed — `omh settings set`,
`omh settings rm`, `omh settings mcp add`, `omh import` — which omh runs, and
the screen shows what it said marked ✓, `!` or ✗, with the line beside it or,
where there is no room, after it, and marks the row it changed until the next
key. So its refusals, warnings and
wording are that command's, and anything you did there you can script.
Removing previews first, and asks in the preview's words, with the preview's
warnings; a hook omh ships is refused before the question rather than after
it. The one exception is `$EDITOR`, which the screen runs itself through
`omh settings edit`'s own steps — a refusal printed by a child would land on
the terminal just handed to the editor, behind the screen drawn back over it.

**Your catalogue, and nothing of this checkout's.** A hook in a repo's
`.omh/hooks/` is not on the hooks page, and the screen does not import hooks:
a hook lands in a repo, so `omh import hooks <harness>` is run in one. A
server is drawn by its command, as `omh settings mcp ls` prints it, with its
environment named and its arguments counted — never a value of either, since
those are where a token goes — and the line shown after adding one hides the
same.

**Nothing read is drawn as a control sequence.** A skill's description, a
hook's `does` and a command's output are drawn with every control character
as `�` — a line break stays one — so text somebody else wrote, an entry `i`
just imported, cannot drive your terminal. A value typed into a new server's
env is drawn as `•`, as it is hidden everywhere else.

**What an edit left is read back.** When `$EDITOR` closes, the entry is read
again, and one that no longer parses is said, not reported done. A hook omh
ships can be opened, and the screen says what `omh upgrade` does to an edit:
puts its own back and keeps yours as `<name>.json.yours`. A link to nothing is
refused before any editor runs — opened, an editor writes through it, out of
the catalogue — and the refusal names the `rm` that takes it out.

**What a new repo gets is said and changed here.** Each entry is marked `●`
or `○`, and each page's heading says how many: *new repos get all* while a kind
has no list — every entry, including one you add later — or *new repos get 1 of
4* once it has one. `space` changes an entry, running `omh settings use` or
`unuse`, which write `~/.omh/default.toml`. A feature of omh's is
switched in one place, its row on your defaults page —
`omh settings set <feature> on|off` — whole, with its server, hooks and rules.
On a kind with no list, `unuse` names the entry in `[unuse]` and the kind goes
on following the rest of your catalogue — *new repos get all but 1* — and `use`
takes it back off. A guard is the exception: it refuses the agent's edits, so it
is never seeded unless a list names it, and `use` on one writes the kind as a
list, and says so. `omh settings use <capability> --all` takes a kind's list and
its exclusions out again. Your defaults
list omh's features too, under their own heading, and `[use]` is marked when it
names an entry your catalogue does not have, as `omh init` would warn. A repo
that exists keeps its own: `omh use`, `omh unuse` and `omh set <feature> off`
there.

**Not `omh set`.** `omh set` is this checkout; `omh settings` is what a new one
starts from. `omh setting` is refused rather than guessed at.

**Keys omh reads and `[omh]` are seeded, and `[use]` and `[unuse]` come to a
new repo's lists**; anything else in the file is refused by name rather than
left behind, and so is a kind in those lists that is not one. `[mcp.<name>.env]` is
**refused**: a server's environment can be a token and the file `init` writes
is committed. It already has a home — `omh settings mcp add <name> <command>
--env KEY=value` puts it on the server in `~/.omh/mcp.json`, where servers live.

**MCP lives under `omh settings` because MCP servers *are* configuration.** They
live in your catalogue; a repo overrides a server's environment without
redeclaring it. See [Configuration](/reference/configuration/).

**Your defaults and your catalogue.** The five verbs above all act on `~/.omh`;
the screen is that catalogue listed for changing it. `omh info` is where the
machine is described, and what you have in `~/.omh` is part of what it means
by *what you have here*.

**`edit` and `rm` take a name, so they validate one**: `omh settings edit
skills ../../.ssh/id_rsa` is refused before `$EDITOR` sees it, and the same
name is refused before `rm` deletes anything.

**An entry opens as the file it is, and a new one is made the shape its kind
is read in.** `omh settings edit rules house` opens `house.md`, the file
`omh info` lists as `house`. A new rule, command or subagent is made `.md`, a
new hook `.json`, and a new skill a directory — a skill is mounted as a
directory holding a `SKILL.md`, and that file is what opens, for a new skill as
for one you have. A new skill nothing was saved into is taken back rather than
left as an empty directory. A name is a name whole:
`omh settings edit rules v1.2` opens `v1.2.md`. A server has no file of its
own, so `omh settings edit mcp` takes no name and opens `mcp.json`.

**A new entry opens as its kind's shape** — front matter for a skill, a
subagent or a command, a hook's JSON, a rule's heading — and one saved untouched
is taken back.

**`$EDITOR` runs as a shell would run it**, as git runs it — `$VISUAL` first,
then `$EDITOR`:
`EDITOR='code --wait'` works, a path with a space in it is quoted inside the
value, and an empty `EDITOR` is no editor set, which is `vi`. Its status is the
command's: an editor that fails fails the edit, naming it, and one `sh` cannot
find is said in words, since `sh`'s own are behind the screen.

**One `rm` for every kind of entry**, servers included — `omh settings rm mcp
linear` and `omh settings rm skills review-diff` are the same act. At a
terminal it asks; anywhere else it needs `--yes`. A name two files answer to
(`house.md` and `house.txt`) is refused rather than guessed at, and so is a
hook omh ships: `omh upgrade` would put it back, so the refusal names
`omh unuse hooks <name>`, which stops a repo running it. A server omh ships —
`codegraph`, `memory` — is refused too: it is one of omh's features, switched
off whole with `omh set <feature> off` rather than taken apart, and the screen
does not offer to remove it. There is no trash, so before it asks it says
what goes with the entry and that it cannot be undone — a server's env by
name, never by value; the files in a skill's directory; of a link, that only
the link goes:

```console
$ omh settings rm skills review-diff
omh: 1 file goes with it — SKILL.md — and it cannot be undone
remove /Users/you/.omh/skills/review-diff? (y/N)
  > y
removed /Users/you/.omh/skills/review-diff
```

<details>
<summary>Why <code>omh setting</code> is refused, why the key list lives in the binary, and why <code>edit</code> draws no fence past the name</summary>

What omh reads is a table in the binary, so no settings file can show it, and
until this command existed the only way to learn a key's name was to already
know it.

`omh set` and `omh settings` are one letter apart with opposite scopes, so clap
is deliberately not told to accept unambiguous prefixes.

Past that there is no fence to draw — `$EDITOR` is a full program running as
you, and the boundary that matters is elsewhere: every catalogue directory omh
mounts into a sandbox is mounted **read-only**, so the agent can read a selected
skill and cannot write one.

</details>

## `omh import <capability> <harness>`

**The way in.** Bring a setup you already have into your catalogue, rather
than rebuilding it.

```
omh import <capability> <harness>     read it out of where that harness keeps it
omh import <capability> <harness> --from <path>
                                      read it out of somewhere else
```

| | |
|---|---|
| `<capability>` | `skills`, `commands`, `subagents`, `rules` or `hooks` |
| `<harness>` | the installed harness to read it out of |
| `--from <path>` | read this instead of where the adapter says that harness keeps it |

```console
$ omh import skills claude
```

```text title="omh import skills claude"
import from claude skills (~/.claude/skills)
  imported  tdd

wrote → ~/.omh/skills
```

**MCP servers come in through `omh settings mcp import <harness>`.** `omh import
mcp` refuses and names it: a server is a record in one file, not an entry with
its own.

**It never overwrites.** An entry whose name you already have is kept and
said so, so running it twice is safe and tells you nothing changed:

```console
$ omh import skills claude
```

```text title="omh import skills claude"
import from claude skills (~/.claude/skills)
  kept  tdd  already in your catalogue
```

**`--dry-run` reads everything and writes nothing**, which is worth using first
if you are not sure what a harness has:

```console
$ omh import skills claude --dry-run
```

```text title="omh import skills claude --dry-run"
import from claude skills (~/.claude/skills)
  imported  tdd

--dry-run: nothing written
```

**Hooks land in the repo, not the catalogue** — a hook binds to a project's
own commands, so `cargo test` here and `pnpm test` next door is one name and
two bodies. They go to `<repo>/.omh/hooks/` and into `[use]`, or they would
arrive dead. Everything else is yours across projects and goes to `~/.omh` —
so it needs no repo, and `omh import skills claude` works from anywhere.

**A symlink is refused at any depth, as is a name that is a path** — an
imported entry is content omh will mount read-only into a sandbox, and neither
of those is content.

**`--from` reads a config from somewhere other than where the adapter says that
harness keeps it.** Useful for a setup in an unusual place, and for seeing what
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

| | |
|---|---|
| `<harness>` | which harness's shapes to render into |
| `--to <dir>` | where to write. Required, and deliberately not the checkout |

```console
$ omh eject claude --to /tmp/ejtest
```

```text title="omh eject claude --to /tmp/ejtest"
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
a sandbox: the memory server is invoked with `--notes /omh/memory`, hooks
read `$OMH_GRAPH_PROJECT`, rules point at `/work`. On your host none of that
resolves, and omh names the files rather than rewriting them.

**A capability the harness has no binding for is named too**, so a reader
comparing the output to their omh setup does not assume something was lost.

<details>
<summary>Why an exit exists, why it costs almost nothing, why it names files rather than fixing them, and why <code>--to</code> is required</summary>

For an opinionated tool, a credible exit is what makes adoption safe. You are
handing omh your rules, your credentials and your sandbox policy, and being
able to leave with all of it is the difference between a default and a cage.

It is nearly free to build because omh already renders exactly these documents
every time it launches. The only difference here is the destination: a launch
stages them for a container, mounts them read-only at guest paths and links
directories at layer paths that exist nowhere but inside the sandbox. Eject
renders the same content through the same functions and writes real files.

omh does not rewrite the sandbox paths because it does not know where you want
your notes or whether you will run the harness in a container of your own — and
a guess written into a file you are about to depend on is worse than being told
to look.

`--to` is required and deliberately not the checkout: the command exists to
*show* you what you would be keeping, and one that overwrote a working tree by
default would be the opposite of reassuring.

</details>

## `omh memory …`

The note store: a graph of linked Markdown notes, scoped to this repo, that
survives session removal and a switch from one harness to another. That
survival is what makes it memory rather than context.

```
omh memory
```

```console
$ omh memory
```

```text title="omh memory"
credentials-are-a-named-volume                     2026-08-05  1 ref
surprise/mounting-a-credential-file-returns-ebusy  2026-08-07  1 ref
```

The `surprise/` on the second key is the shipped template's namespace, not
decoration — see [`omh memory remember`](#omh-memory-remember).

**Every line carries its date.** A note presented without its age cannot be
judged.

**One store per repo**, at `~/.omh/memory/<repo>/`, shared by every session of
that repo and never committed. Inside the sandbox it is mounted at
`/omh/memory`.

**A note reaches the next session of this repo the moment it is written**, the
way the code graph does. It reaches nobody else.

**Nothing merges.** A setting has one value; a note is a claim, and two claims
about one topic are two facts — so a disagreement is two keys, and a key
claimed twice is what `omh memory lint` calls `DuplicateKey`.

**A store at the pre-0.14 path is moved** onto the new one the next time omh
runs, which it says out loud. Two stores are named and neither is touched.

<details>
<summary>Why the store lives outside the checkout, and the committed layer it replaced in 0.14</summary>

It lives outside the checkout on purpose: a session is a git worktree holding
tracked files only, and removing one runs `git worktree remove --force`, so a
store inside the repo would be invisible to the sandbox and destroyed by
`omh sNN rm`.

It was `~/.omh/notes/<repo>/local/` until 0.14, when the committed layer went
and `local` stopped naming anything. That committed layer lived under
`<repo>/.omh/notes`, promoted by hand and shared through review, and one store
replaced it. When two stores are found, neither is touched, because which of two
notes under one key is the current one is not omh's guess to make.

There is one place a write can go, which is the point: a writer that could
reach a committed layer would push wrong facts to teammates through git, where
they arrive with the authority of a reviewed change.

The date on every line is there because a store the agent writes to unattended
would otherwise become a machine for laundering guesses into facts.

</details>

### `omh memory remember`

Records a surprise as a note in this repo's store.

```
omh memory remember --expected <text> --observed <text> --evidence <text> --answers <question>
```

| | | |
|---|---|---|
| `--expected` | what you thought would happen | required |
| `--observed` | what actually happened. Its first sentence is the key | required |
| `--evidence` | the command, the error, the file | required |
| `--answers` | a question this note answers, as somebody would later ask it. Repeat for several | at least one |
| `--relates-to <key>` | a note this connects to, by key. Repeat for several | |
| `--invalidated-by <trigger>` | when the note stops being true — see [`omh memory stale`](#omh-memory-stale) | never |
| `--source` | who observed it | `cli` |
| `--if-exists skip\|suffix\|override` | what to do when the derived key is taken | `error` |

```console
$ omh memory remember \
    --expected "A bind mount of the token file would persist the login." \
    --observed "Mounting a credential file returns EBUSY." \
    --evidence 'EBUSY from the mount syscall' \
    --answers "why does mounting the token file fail?"
```

```text title="omh memory remember"
recorded /home/you/.omh/memory/omh-1a2b3c4d/surprise/mounting-a-credential-file-returns-ebusy.md
```

**The first three arguments are the discipline.** Something with nothing to put
in `--expected` has learned nothing worth recording, so the filter runs for
free. The fourth is what makes the note findable: the question it answers, in
the words somebody would later ask it in. Leave it out and the note is refused.
[Memory](/reference/memory/) has the file this writes.

**The key is derived, never chosen.** It comes from a template in
`.omh/memory.toml`, so the same observation cannot be recorded twice under two
spellings — `Mounting a credential FILE returns EBUSY.` and `mounting a
credential  file returns ebusy` produce one key, and the second write is a
conflict that says *update that note instead*:

```console
$ omh memory remember --expected … --observed "mounting a  credential FILE returns ebusy" --evidence … --answers …
```

```text title="omh memory remember"
omh: `surprise/mounting-a-credential-file-returns-ebusy` is already recorded; update that note instead
```

**The slug is the observation's first sentence**, and the terminators are `.`,
`!` and `?` only — a semicolon or a colon does not end one, so a long
`--observed` yields a long key.

**`--if-exists skip|suffix|override` makes retry policy an argument.** Skipping
is a mode you ask for, never a fallback: as a fallback, every genuine conflict
disappears silently. `override` is the destructive one and says so — it removes
the note that held the key, wherever that note was stored, and leaves the
replacement at the key's own path:

```console
$ omh memory remember --if-exists override …
```

```text title="omh memory remember --if-exists override"
replaced /home/you/.omh/memory/omh-1a2b3c4d/surprise/mounting-a-credential-file-returns-ebusy.md — the note that was there is gone
```

### `omh memory lint`

Schema violations and graph hygiene, and the store-quality meter: a count per
rule, with no questions asked and no model pass.

```
omh memory lint
```

```console
$ omh memory lint
```

```text title="omh memory lint"
refused  a `surprise` note needs a `## Evidence` section
warning  `deploy` links to `rollback`, which is not in the store
warning  nothing in the store links to `deploy`
warning  nothing in the store links to `surprise/mounting-a-credential-file-returns-ebusy`

  1  MissingSection
  1  DanglingLink
  2  Orphan
omh: 1 violation the schema refuses
```

**Schemas refuse and hygiene warns.** The two words are not decoration.

**It fails when the schema refused something, and not for warnings** — the run
above exits 1 because of the one `refused` line, and would exit 0 without it.
Until the agent's own writes are refused, this is what a hook or a CI step can
gate on.

**A young store is mostly orphans.** `Orphan` fires on every note nothing links
to — that is the rule working, not the store failing. It is the count over time
that means something.

Three rules are worth naming because they describe the store rather than one
note's shape:

| Rule | | |
|---|---|---|
| `UnclosedFence` | refuses | a note whose code fence never closes: everything after it is quoted, including headings and `[[links]]`, so the note does not mean what it looks like |
| `DuplicateKey` | warns | one key claimed by two files — a key is a primary key, `remember` refuses to create a second, and this is how one that arrived by hand becomes visible |
| `Orphan` | warns | a note nothing links to, which is the store's shape rather than one note's, so it never refuses a write |

<details>
<summary>Why schemas refuse and hygiene only warns, and why the exit code follows the same split</summary>

Agents negotiate with warnings and cannot negotiate with a refused write — and a
store-wide problem must never fail somebody else's write.

A gate that also tripped on hygiene would be red for every store with an
unlinked note in it.

</details>

### `omh memory rm <key> [--at <path>]`

Removes one note and reports what pointed at it.

```
omh memory rm <key> [--at <path>]
```

| | |
|---|---|
| `<key>` | the note, as `omh memory lint` and the recall output name it |
| `--at <path>` | which file, when one key reached two of them — any trailing run of path components |

```console
$ omh memory rm credentials-are-a-named-volume
```

```text title="omh memory rm credentials-are-a-named-volume"
removed credentials-are-a-named-volume
  still linked from surprise/mounting-a-credential-file-returns-ebusy — those links now dangle, and `omh memory lint` lists them
```

**Deletion never cascades**, and neighbours are never rewritten. A dangling link
is visible and the lint finds it; a silently pruned neighbourhood is neither.
This is the same rule that makes `omh sNN rm` keep a branch holding commits.

**`--at` is the repair path for two files claiming one key**, which
`omh memory lint` reports as `DuplicateKey`. It takes any trailing run of path
components — `dup.md` alone is enough when it is unambiguous — and `rm` prints
the store-relative form:

```console
$ omh memory rm surprise/the-mount-failed
```

```text title="omh memory rm surprise/the-mount-failed"
omh: `surprise/the-mount-failed` is one key over 2 files — name one with --at: ns/dup.md, surprise/the-mount-failed.md
```

**A `--at` that names no note is an error**, never a fall-through to a different
one: it is the flag you reach for to be careful, so it is the one input that
must not be quietly ignored.

### `omh memory stale`

Notes the world has moved on from. **A join against facts omh already holds**,
never a judgement — there is no "old enough to be suspect" here, because that
would be a threshold nobody calibrated wearing the clothes of a fact.

```
omh memory stale
```

```console
$ omh memory stale
```

```text title="omh memory stale"
stale:
  surprise/the-base-set-pins-its-version  2026-08-10  — the base set is 2026.08; this note pinned 2020.01
  surprise/the-entrypoint-is-tiny         2026-08-10  — `src/main.rs` was f328e4d9d04c31d0d70d16d21a07d1613be9d577, is now 75275760623f7f5fe5ad07f415dc5b1a374c87cd

omh cannot tell:
  surprise/a-symbol-nobody-can-check  2026-08-10  — no indexed code graph reachable from the host

no expiry — carries only its date:
  surprise/nothing-pins-this-at-all  2026-08-10

1 still current
omh: 2 notes the world has moved past
```

**Three groups because they are three different claims.** **"omh cannot tell"
is never folded into "still current"** — that is the one failure that would
make this command a liar rather than merely incomplete.

**The exit code keeps them apart too:**

| Exit | When |
|---|---|
| 1 | anything is stale |
| 2 | nothing is, and something could not be told |
| 0 | every note that declares an expiry is still current |

**A note declares its expiry with `--invalidated-by`**, from a closed set omh can
evaluate itself:

| | invalid when |
|---|---|
| `file:<path>@<hash>` | the file's content changes, or it is deleted |
| `image:<digest>` | the **base** image recipe changes. Pin it as `image:current` and omh records what it would build now — the value is a `git hash-object` of recipe text that lives inside the binary, so there is nothing to read it off. A harness layer changing does not fire it |
| `base:<version>` | the base set moves to a **later month**. `parse_ym` compares year and month, so a re-cut within the same month is not a change, and a rollback to an older set is not staleness |
| `symbol:<name>` | the code graph no longer contains it |
| *(absent)* | never — it carries only its date |

**Anything else is refused at the door.** A trigger omh cannot evaluate is a
note advertising an expiry it does not have, which is worse than carrying none,
because somebody trusts it.

**A `file:` path recorded inside the sandbox** as `/work/src/main.rs` is stored
relative to the repo. Without that, every file trigger would report stale the
moment it was written.

**`symbol:` always answers "cannot tell" today.** It fires correctly the day a
symbol set can be supplied, and there is a test that says so — so this is a
gap, not a stub.

<details>
<summary>Why <code>symbol:</code> cannot be checked from the host yet</summary>

The code graph lives in a container volume, queried per session through a
running sandbox under a per-session project name; a host-side command has none
of those. Asking for one would put a container exec in the middle of a command
whose whole point is that it only joins facts already at hand.

</details>

### What the agent sees

Inside a session the store is two MCP tools, not a command. omh runs the server
itself rather than pointing the harness at a graph server directly, because the
guarantee below cannot be enforced on a server the agent talks to on its own.

**`recall(question)`** ranks, expands one hop, and returns the neighbourhood in
one call. Every line carries the note's date:

```
surprise/mounting-a-credential-file-returns-ebusy  2026-08-07
└─ credentials-are-a-named-volume                  2026-08-05
```

Ranking is by how *rare* the question's words are in this store — a word in
every note cannot tell two notes apart, so it barely counts. Ties break on
recency, then on what the rest of the store points at. **Nothing picks a
winner**: contradicting notes both come back, and reconciling them is the
agent's job, done with the dates in hand. [What recall matches](/reference/memory/#what-recall-matches)
has the rest.

**`remember(expected, observed, evidence, answers)`** records a surprise, with
the same filter as [`omh memory remember`](#omh-memory-remember): `answers` is
the questions the note will be found by. It cannot pass a source or an
override: provenance is omh's (the session from its environment, the harness
from the MCP handshake), and no argument turns the schema off.

**The tool description carries a census of the store** — counts, never titles,
so what it costs stops growing with the graph. It is recomputed on every
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
that needs a store nobody has grown yet.
A subcommand that printed "not implemented" would be worse than its absence,
because `--help` advertises it.

## `omh prune`

Removes what omh left behind that nothing claims any more — across the whole
machine.

```
omh prune                                    remove what nothing claims any more
omh --dry-run prune                          the same report, having removed nothing
omh prune --dangerously-include-unsafe       and what omh cannot vouch for, after asking
```

| | |
|---|---|
| `--dangerously-include-unsafe` | also remove what omh cannot vouch for, after naming each one and asking — [below](#--dangerously-include-unsafe) |

```console
$ omh prune
```

```text title="omh prune"
removed 3 things
  directory  ~/.omh/run/web-deadbeef
  directory  ~/.omh/scratch/web-deadbeef
  directory  ~/.omh/keys/web-deadbeef
left 2 things
  0    belong to checkouts still on this machine
  1    omh could not attribute
         ~/.omh/notes/stranger-1a2b3c4d — omh has no record of which checkout this belongs to — it predates the record, or was never set up by this omh
  1    omh cannot vouch for
         ~/.omh/shadow/web-deadbeef — holds 1 thing omh cannot vouch for (s01.git)
  those need `omh prune --dangerously-include-unsafe`, which names each one and asks first
unread 1 class omh could not list, so the counts above are a floor:
  there is no container runtime to ask about volumes, containers or networks
```

**What it looks for.** omh creates things that outlive the checkout that made
them: a cache volume, a container and a network per session, the per-checkout
state under `~/.omh`, and the `tmp.*` remnants of operations that did not
finish. Every one is keyed by a digest of the checkout's absolute path, and omh
records that path so it can tell whose each one is.

**It is machine-wide**, because an orphan outlives the checkout that made it:
a per-checkout view structurally cannot see the ones that matter.

**It removes only what it can prove is nobody's.** The owning checkout has to
be recorded and verified gone, and nothing there may hold work. There is no
`--yes`.

**Every bucket is printed every time, including the empty ones.** A report that
lists only what it found reads as an inventory; one that also says *nothing omh
could not attribute* is telling you it looked. The last line is the same rule:
a class omh failed to enumerate makes every count above it a floor, and you
cannot tell that from a small number.

<details>
<summary>Why omh had to start recording paths, and why there is no <code>--yes</code></summary>

The digest is **one-way**, so until omh started recording the checkout's path it
could see these things existed and never whose they were.

A confirmation over a set that is safe by construction is ceremony, and ceremony
is what teaches people to confirm without reading — so that when the question
that matters arrives, it gets the same reflex.

</details>

### What it will not touch

**Anything omh could not attribute.** On a machine with history this is most of
them, and that is the honest answer rather than a disappointing one: ids from
before 0.8.0 carry no path digest at all, so there is nothing to compare. omh
recovers what it can from evidence it already wrote — a worktree's own `.git`
pointer, and the checkout an image was built for — and says so when it cannot.

**Anything holding work.** A sandbox repository, a worktree, or a note store
that still has something in it is left alone even when its checkout is gone.
Notes are in that list on purpose: a note is something a person wrote, not
state omh derived, and orphaned is not the same as worthless.

**Images.** They are content-addressed and shared — the same `omh/claude:<hash>`
serves every checkout whose base resolves to it, and the label recording which
checkout built it names only whoever got there first. Removing one because that
checkout is gone would take the image other checkouts are running, so omh
refuses it explicitly and uses those labels only to *attribute* other things.
Reclaiming images needs the in-use check, which is not written yet.

**A checkout whose whole tree has vanished.** An ejected disk answers "not
there" exactly as a deleted directory does. Deleting a checkout leaves the
directory it sat in; unmounting takes the tree, so an absent path whose parent
is also absent is reported as unattributable rather than gone. Deleting a whole
project tree at once reads the same way — less is reclaimed, and nothing is
reclaimed wrongly.

### `--dangerously-include-unsafe`

Reaches the two buckets above. It names every item with its own reason and asks
once, so what you are authorising is in front of you at the moment you
authorise it.

**Off a terminal it refuses.** There is no `--force` here and there should not
be: nothing about running in a script makes destroying unreviewed commits safer.

## Next

<div class="cards">

- [Configuration](/reference/configuration/)\
  Your catalogue, what a repo takes from it, and which file a setting lands in.
- [Sessions](/concepts/sessions/)\
  What a session is: a container, a git worktree and a branch.
- [Troubleshooting](/guides/troubleshooting/)\
  `omh doctor`, row by row, and what to do when something does not start.
- [Memory](/reference/memory/)\
  The note file `omh memory remember` writes, and what recall matches.

</div>
