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
omh sessions ls|rm|down|diff  s   omh s ls, omh s diff s01
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
  costs       0.46s to index this repo, cold   measured 2026-08-06
              index_repository --mode fast, 821 nodes / 3813 edges, in the sandbox
              3.4 MB on disk                   measured 2026-08-06
              the graph volume after a cold index of this repo
  instead of  gitnexus            PolyForm-Noncommercial licence
              codegraphcontext    needs a Neo4j service running
              @sdsrs/code-graph   close second; needs a node runtime rather than a static binary
  installed   shared
  remove      omh config mcp rm codegraph

  answered from ~/.omh/base/2026.08.toml · 2026.08
```

**Cost is measured; benefit is argued.** Every cost carries the date it was
taken *and how*, and one predating the current base-set version is marked stale
— so re-cutting the base set puts every carried-over number up for
re-affirmation. The `because` line is a judgment you are free to disagree with —
see
[measure the cost, argue the benefit](design/trust.md#measure-the-cost-argue-the-benefit).

### Six answers, because authorship differs

| | |
|---|---|
| **omh's choice** | in the base set, and your copy matches what omh ships |
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
omh s down [id]       stop the container, keep the worktree and branch
omh s rm [id]         remove the session — a branch with commits is kept
```

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
through the same three layers as everything else. See
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
credentials-are-a-named-volume            team   2026-06-12  1 ref
mounting-a-credential-file-returns-ebusy  local  2026-08-07  1 ref
```

**Every line carries its date and its layer.** A note presented without age and
origin cannot be judged, and a store the agent writes to unattended would
otherwise become a machine for laundering guesses into facts.

### `omh memory remember`

```console
$ omh memory remember \
    --expected "A bind mount of the token file would persist the login." \
    --observed "Mounting a credential file returns EBUSY; the harness rewrites in place." \
    --evidence 'EBUSY from the mount syscall'
recorded ~/.omh/notes/omh/local/surprise/mounting-a-credential-file-returns-ebusy.md
```

The three arguments are the discipline. Something with nothing to put in
`--expected` has learned nothing worth recording, so the filter runs for free.

**The key is derived, never chosen.** It comes from a template in
`.omh/keys.toml`, so the same observation cannot be recorded twice under two
spellings — `Mounting a credential FILE returns EBUSY.` and `mounting a
credential  file returns ebusy` produce one key, and the second write is a
conflict that says *update that note instead*:

```console
$ omh memory remember --observed "mounting a  credential FILE returns ebusy." …
Error: `surprise/mounting-a-credential-file-returns-ebusy` is already recorded;
       update that note instead
```

`--if-exists skip|suffix|override` makes retry policy an argument. Skipping is a
mode you ask for, never a fallback: as a fallback, every genuine conflict
disappears silently.

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

    1  MissingSection
    1  DanglingLink
```

The two words are not decoration. **Schemas refuse and hygiene warns**, because
agents negotiate with warnings and cannot negotiate with a refused write — and
because a store-wide problem must never fail somebody else's write.

### `omh memory rm <key> [--layer team|local]`

Removes one note and reports what pointed at it.

```console
$ omh memory rm credentials-are-a-named-volume
removed credentials-are-a-named-volume
  still linked from mounting-a-credential-file-returns-ebusy — those links now
  dangle, and `omh memory lint` lists them
```

**Deletion never cascades**, and neighbours are never rewritten. A dangling link
is visible and the lint finds it; a silently pruned neighbourhood is neither.
This is the same rule that makes `omh s rm` keep a branch holding commits.

`--layer` is needed only when one key exists in both layers, which is a
disagreement rather than a duplicate. Without it, `rm` names both and removes
neither.

### What is not here yet

`recall`, `promote` and `stale` arrive with the milestones that give them
something to act on — see [the design](design/memory.md). A subcommand that
printed "not implemented" would be worse than its absence, because `--help`
advertises it.
