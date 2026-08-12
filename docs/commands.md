# Commands

```
omh init                          set this repo up
omh <harness> [args…]             claude · opencode   ← bare name = run an agent
omh attach [editor]           a   open the session in your editor, over SSH
omh graph [--stop]                browse the code graph in a browser
omh auth <harness> [account]      log in once; repeat for several accounts
omh doctor [harness]          d   verify a harness really sees your profile
omh why <thing>                   who put this here, and on what grounds
omh ls                            harnesses, editors, sessions
omh sessions ls|diff|commit|push|down|rm  s   omh s diff, omh s push fix/x
omh config [set|unset|edit|mcp] c omh c mcp import claude
```

## The shape of the CLI

Noun-verb groups with single-letter aliases: `omh s ls`, `omh c mcp ls`,
`omh d claude`.

**A bare name is always a harness.** Editors live under `attach`, so `omh claude`
and `omh attach zed` cannot be confused for one another — the bare slot means
exactly one thing.

That creates a hazard: since `omh <anything>` is a harness, an adapter could
shadow a real command. A `RESERVED` list prevents it, and rather than trusting
anyone to keep that list current, a test introspects the CLI definition and
fails if any command or alias is missing from it.

---

## `omh init`

Sets up the current repo. Decides everything, asks nothing, reports all of it.
Covered in [Getting started](getting-started.md#what-it-actually-did).

Safe to re-run: it never overwrites files you have edited.

## `omh <harness> [args…]`

Runs a harness inside a session, creating one if needed. Arguments after the
name are passed through to the harness untouched.

```console
$ omh claude
$ omh claude --resume
$ omh opencode
```

Running it a second time **reattaches** rather than starting a second agent.
Use `--new` to force a fresh session.

If the harness cannot express part of your profile, that is announced once at
launch rather than silently dropped:

```console
$ omh opencode
omh: opencode on omh/s01 — dropped 1 subagents, 2 hooks (unsupported)
```

`-a <account>` selects a credential set for this launch. See [Accounts](accounts.md).

## `omh attach [editor]` · `a`

Opens the session in an editor over SSH. With no argument it resolves
`$OMH_EDITOR` then `$EDITOR`, and falls back to printing every recipe:

```console
$ omh attach
session s01 is up

  ssh://omh-ohmyharness-s01/work
  ssh omh-ohmyharness-s01

  VS Code / Cursor   code --remote ssh-remote+omh-ohmyharness-s01 /work
  Zed                zed ssh://omh-ohmyharness-s01/work
  JetBrains          Gateway → SSH → omh-ohmyharness-s01
```

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

## `omh auth <harness> [account]`

Runs the harness's own login and captures the result. `account` defaults to
`default`.

```console
$ omh auth claude personal
$ omh auth claude work
```

See [Accounts](accounts.md).

## `omh doctor [harness]` · `d`

Launches the real image with the real mounts and checks the guest paths the
adapter claims. The only thing that can verify an adapter. See
[Troubleshooting](troubleshooting.md).

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
  installed   shared
  remove      omh config mcp rm codegraph — the feature, server and hooks together

  answered from ~/.omh/base/2026.08.toml · 2026.08
```

**Everything omh ships belongs to a feature**, and removal follows the feature:
`omh config mcp rm codegraph` takes the server and its four hooks together.
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
rule as `omh attach emacs`.

## `omh ls`

Everything omh knows about: harnesses, editors, sessions and their state.

## `omh sessions …` · `s`

```
omh s ls              sessions, their branches and state
omh s diff [id]       what the agent changed
omh s commit [-m …]   commit that work onto the session branch
omh s push [name]     push it to origin under a name a reviewer can read
omh s down [id]       stop the container, keep the worktree and branch
omh s rm <id>         remove the session — a branch with commits is kept
```

`diff`, `commit` and `push` default to the most recent session; `-s` names
another. `rm` takes an id, and `down` with none stops every session.

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

  fix the cause:  omh config edit
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

**The agent cannot do any of this itself.** git does not work inside the session
— the worktree's `.git` points at an admin directory on the host, which omh does
not mount — so the sandbox is told as much, and told not to try to repair it.
See `omh why git-unavailable`.

**`rm` never deletes a branch that has commits.** Unreviewed agent work must be
unloseable, so the branch outlives the session that produced it, and `rm` tells
you how to review or discard it:

```console
$ omh s rm s01
removed session s01; branch omh/s01 kept (3 commits to review)
  review with  git log main..omh/s01
  discard with git branch -D omh/s01
```

A branch with **no** commits is dropped. Keeping it preserved nothing —
`worktree remove --force` has already discarded anything uncommitted — while a
namespace filling with dead refs trains you to ignore the ones that matter.

## `omh config …` · `c`

```
omh config                            effective settings, with provenance
omh config set <key> <value>          → the gitignored layer by default
omh config unset <key> [--layer]      lets the layer beneath resurface
omh config edit [--layer]             $EDITOR escape hatch

omh config mcp ls
omh config mcp add <name> <cmd> [args…] [--env K=V]
omh config mcp rm <name> [--layer]
omh config mcp import <harness> [--file] [--force]
```

Every value reports where it came from and what it beat:

```console
$ omh config
policy:
  carry_in         [".env.local"]     ← local (overrides shared)
  idle_timeout     30m                ← personal
```

MCP lives under `config` because MCP servers **are** configuration, resolved
through the same three settings layers as everything else. See
[Configuration](configuration.md).

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
