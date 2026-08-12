# Configuration

Your setup is declared once and rendered into whatever shape each harness reads.
This page covers where it lives, how it resolves, and how to change things.

> Selection — `[use]`, `omh use` / `unuse` — is not built yet. Until it is, a
> repo gets your whole catalogue. See [The profile](design/profile.md) for the
> model and the reasoning.

## One catalogue, and it is personal

```
~/.omh/
  rules/  skills/  commands/  subagents/  hooks/   the only place these live
  mcp.json
  settings.toml                                    your defaults
```

**Rules are a directory of named files, not one `AGENTS.md`.** `tdd.md`,
`commit-style.md` and `rust-idiom.md` are separate things you hold. That also
makes the catalogue uniform: every capability is a directory of named entries,
with `mcp.json` the lone exception because a server is a record rather than a
file.

A repo holds configuration, and one kind of content:

```
<repo>/.omh/
  settings.toml        committed: settings, and which of omh's features are on
  settings.local.toml  gitignored: your overrides, and the secrets the other must not hold
  memory.toml          committed: how the note store keys and expires
  hooks/               committed: hooks that only make sense in this repo
<repo>/AGENTS.md       the project's own rules — tracked, and actually read
```

A project cannot declare a skill, an MCP server, a command or a subagent. It
names ones from your catalogue.

### Why hooks are the exception

A skill is a way *you* work — it travels with you across repos, which is why it
belongs to you. A hook binds to a repo's own commands, and the stack hooks are
the proof: `cargo test` here, `pnpm test` next door, one name and two bodies. A
capability that is project-specific by nature has to be declarable where the
project is, or the catalogue fills with entries that are only ever right in one
place.

So the rule is not "no content in the repo". It is **content lives where its
scope is**, and hooks are the one capability whose scope is the repo.

**A project hook beats a catalogue hook of the same name**, which is how a repo
overrides your personal `format` hook with the one it actually needs, without
renaming anything. **Names from the base set are reserved** — a file answering
to one is an error naming both, because a repo that could replace
`graph-refresh` could make the graph lie while looking installed.

### What that costs

**A repo can no longer ship a skill, an MCP server or a command to your
teammates.** The committed layer used to do exactly that. What a repo still
shares is its rules file — which for the first time actually reaches the agent
— its hooks, and its settings.

Recorded, not built: a catalogue entry could carry a `source` and `omh sync`
could fetch the missing ones, which restores team sharing without putting
content back in the repo.

## What goes in the catalogue

Most of what you already have needs no translation, which is what makes
adopting the catalogue cheap.

**Skills follow [Agent Skills](https://agentskills.io/specification)** — a
`SKILL.md` with `name` and `description` required, `license`, `compatibility`,
`metadata` and `allowed-tools` optional, in a directory named after the skill.
`metadata` is the spec's own extension point for anything a client needs that
it does not define. Claude Code, opencode, Cursor and Codex CLI all read it, so
`~/.omh/skills/<name>/SKILL.md` is copied to whichever harness you launch,
unchanged, and nothing is lost in the trip.

**Rules are markdown.** `~/.omh/rules/*.md` is prose, and prose travels.

**MCP servers** are the MCP spec's own shape, re-rendered per harness — the one
place omh has always translated.

**Commands** are markdown with frontmatter, and both harnesses read the same
shape. Copied.

**Hooks** are omh's own vocabulary, translated at staging. See below.

**Subagents are the exception**, and it is worth knowing before you fill that
directory. There is no cross-tool standard: `name`, `description`, `tools`,
`model` and `hidden` are common to Claude Code and opencode, but
`permissionMode`, `disallowedTools` and `skills` are Claude's alone, while
`temperature`, `maxSteps`, `mode` and `permissions` are opencode's. omh copies
the file as written, so a subagent authored for one harness reaches the other
with fields it will ignore. Nothing is lost, and nothing is translated.

The thread running through all of it: what does not travel is **tool names** —
`allowed-tools` in a skill, `tools` in a subagent, a hook's matcher. That is
why omh keeps one closed tool vocabulary and one `[tools]` map per adapter.

## Writing a hook

A hook is the one kind of content a repo can declare, and the only thing omh
carries that *executes*. It goes in `<repo>/.omh/hooks/<name>.json` if it needs
to know which repo it is in, and in `~/.omh/hooks/` if it works anywhere.

**You write what you want; the adapter says how this harness spells it.**

```json
{ "on": "turn-end", "run": "cargo test" }
```

```json
{ "on": "before-tool", "tools": ["read"],
  "when": "[ \"$(wc -c < \"$OMH_TOOL_FILE\")\" -gt 8000 ]",
  "inject": "$OMH_TOOL_FILE is large — ask for one symbol instead." }
```

| Field | Meaning |
|---|---|
| `on` | `session-start` · `turn-end` · `before-tool` · `after-tool` |
| `tools` | `edit` · `read` · `shell` · `search`. Empty means every tool |
| `when` | a shell predicate; non-zero keeps the hook silent |
| `capture` | a command whose output binds to `$OMH_CAPTURE` |
| `run` | executes; output ignored |
| `inject` | text into the agent's context |

Exactly one of `run` or `inject`. `capture` needs `inject` — collecting output
nobody reads says nothing. A hook wanting a moment, tool or field this harness
has no word for is **dropped by name at launch**, saying what it asked for; the
rest still ship.

### What the payload gives you

`$OMH_TOOL_FILE` and `$OMH_TOOL_COMMAND` are the tool call, in omh's words.
Mention one in any body and omh reads it for you — `${OMH_TOOL_FILE:-none}`
counts too. Mention neither and your hook pays for nothing, which matters:
`before-tool` on `read` fires on the most frequent tool there is.

### Three rules about `inject`

It is prose that reaches a shell, and that combination fails quietly, so all
three are refused when the file is read rather than discovered at runtime:

- **every `$` must name a variable.** A bare one expands to nothing and your
  sentence arrives with a hole in it, while every check on the text still
  passes. Write `$$` for a literal dollar.
- **no `$(…)`.** Running a command from inside a sentence is what `capture` is
  for.
- **`${…}` has to be a well-formed expansion.** `${ high }` is a *bad
  substitution*, which a shell reports at run time — so the hook exits without
  injecting anything, and every check on its text still passes.

### Degrade to a no-op, never to an error

A catalogue hook runs in repos it was never tested against. `graph-refresh`
ends in `|| true` so a missing indexer cannot fail somebody's turn; `when`
carries the rest. A hook that breaks a turn gets deleted, and then it protects
nobody.

### Names omh ships are omh's

`graph-orient`, `graph-first`, `graph-read`, `graph-refresh` and
`git-unavailable` are generated from the [base set](design/base-set.md), not
files. A hook file answering to one of those names is an **error naming both**
— it would not override omh's, and it would not run, and a hook that is
committed, reviewed and silently inert is worse than one that refuses to start.
To be rid of omh's, switch the feature off with `[omh]`.

## `[use]` — what this repo takes from your catalogue

The catalogue is everything you have. `[use]` is what *this* project uses:

```toml
# <repo>/.omh/settings.toml
[use]
rules     = ["tdd", "commit-style"]           # and in this order
skills    = ["review-diff"]
mcp       = ["linear"]
hooks     = ["notify-on-stop", "rust-test"]   # yours and this repo's
commands  = []
subagents = ["*"]
```

**One mechanism: an allowlist.** No `exclude`, no `include`/`exclude` pair.
Removing something is deleting its name, and there is one place to look to
answer "is this on here".

**Absent means everything; `[]` means nothing.** Those differ on purpose: a repo
that never configured a selection gets the whole catalogue, so upgrading changes
nothing and a new checkout is useful before it is configured — while a list you
emptied by deleting its last name means what it says. `"*"` is "keep following
the catalogue as it grows", written down.

**For `rules`, the list is the order.** Rules build on each other, and a general
one followed by its exception reads differently reversed. Without a list they
compose in filename order, which is the fallback rather than the plan.

It layers like everything else in these files: later wins, **per capability,
wholesale**. Your `~/.omh/settings.toml` can carry a default selection for every
project; a repo naming `skills` replaces it outright. Merging would let a repo
add to your list and never take anything off it, which is the thing an allowlist
exists to make possible.

### A feature is not selectable

`codegraph` and `memory` sit in `~/.omh/mcp.json` looking exactly like servers
you added, because `omh init` seeded them there. They are not yours to select:

```console
$ omh use mcp codegraph
mcp/codegraph is omh's — part of the `codegraph` feature. `[use]` names your
entries; a feature is all or nothing, so `omh repo enable codegraph` and
`omh repo disable codegraph` are its switches.
```

An empty `[use]` leaves every omh feature whole — server, hooks and rules
section together. Taking one apart is the state `[omh]` refuses to let anybody
express, and `[use]` does not get a second door to it.

### What is not selected gets said out loud

`omh init` writes `[use]` with every entry named, because an explicit list is
editable and reviewable in a way `"*"` is not — you curate by deleting lines.
That has one failure mode, and it gets the treatment every silence in omh gets:

```console
$ omh claude
omh: 1 catalogue entry is not selected here: skills/refactor
omh:   omh use skills refactor    ·    omh use --all
omh: warning: [use] names an entry nothing answers to: skills/reveiw-diff
```

Neither is fatal. A typo in a list is something to be told about, not a reason
to refuse to start work.

## Settings, and their three layers

Content has one home; **settings** keep their layers, because a setting has one
value and the useful question is which file decided it:

```
~/.omh/settings.toml  →  <repo>/.omh/settings.toml  →  <repo>/.omh/settings.local.toml
```

Later wins. A machine-wide preference and a one-repo exception are both
expressible, which two layers could not do: the rules for a project belong in
the repo; the API key that makes one of them work does not.

Undebuggable without provenance — the standard complaint about oh-my-zsh, and
the thing [trust](design/trust.md) exists to prevent. So every effective value
reports where it came from and what it beat:

```console
$ omh repo
this repo  /Users/you/proj/.omh

settings
  carry_in         [".env.local"]           ← local (overrides shared)
  idle_timeout     30m                      ← personal

omh's features
  codegraph        off here
  git-notice       on
  memory           on

using
  rules            tdd, commit-style
  skills           review-diff   (1 not selected: refactor)
  mcp              everything
  commands         nothing
  subagents        everything
  hooks            rust-test, rust-format
```

## Two scopes, two commands

`omh config` means **you** — your catalogue and your defaults. `omh repo` means
**this checkout**.

```console
# this repo
$ omh use skills tdd                    # → <repo>/.omh/settings.toml   (committed)
$ omh unuse mcp linear
$ omh use --all                         # resync every list to the catalogue
$ omh repo disable codegraph            # → [omh] in settings.toml
$ omh repo enable codegraph
$ omh repo set carry_in '[".env"]'      # → settings.local.toml         (gitignored)
$ omh repo set --shared account work    # → settings.toml               (committed)
$ omh repo unset carry_in
$ omh repo                              # what is effective here, and what decided it

# you, everywhere
$ omh config set idle_timeout 45m       # → ~/.omh/settings.toml
$ omh config unset idle_timeout         # let the layer beneath resurface
$ omh config edit                       # $EDITOR on your settings
$ omh config edit skills tdd            # $EDITOR on one catalogue entry
$ omh config                            # your defaults, and what the catalogue holds
```

**The two scopes want opposite defaults**, which is why one `--layer` flag could
not serve both:

| Command | Writes to | Why that default |
|---|---|---|
| `omh use` / `unuse` | `settings.toml`, **committed** | what a project uses is a fact about the project, and a teammate cloning should get it |
| `omh repo set` | `settings.local.toml`, **gitignored** | these carry `carry_in` paths and MCP env; a mistyped key must not be committable by accident |

Neither default can reach version control without asking:

```console
$ omh repo set --shared carry_in '[".env"]'
warning: the shared layer is COMMITTED — never put a secret here
```

**Two verb pairs, mirroring the two tables.** `use`/`unuse` for catalogue
entries, `enable`/`disable` for omh's features. The CLI teaches the file's
structure rather than flattening it: if `omh repo disable` took a skill name,
the difference between an entry you chose and a feature omh ships would exist
only here.

`unset` removes the value from one layer rather than forcing a value, which is
what lets the layer beneath take over again — the difference matters when you
are overriding a team default temporarily.

> **`--layer` is going away.** `omh config set --layer shared` still works and
> prints the `omh repo` form that replaces it. It is removed in the release
> after next.

## `[omh]` — omh's own features

`<repo>/.omh/settings.toml` says what this repo does with **omh's own
features**:

```toml
[omh]
codegraph = false     # the server, its four hooks and its section of the rules
```

Feature names only. `graph-first = false` is refused, naming the feature it
belongs to — keeping the graph while dropping one of the things that make it
used is taking a bundle apart, not changing a setting, and "graph on, refresher
off" is a graph that quietly stops tracking the code.

Disabling is not removal: your `mcp.json` is untouched, the server is left out
of the document *this* session is given, and the next repo gets it back.
**Removing the server is the other door** — `omh config mcp rm codegraph` takes
the feature with it, hooks and rules section included, because a hook nudging
the agent toward a server that is gone is worse than no hook.

It layers like every other setting — `~/.omh/settings.toml`, then this file,
then `<repo>/.omh/settings.local.toml`, which `omh init` adds to
`.omh/.gitignore`.

## Settings

Top-level keys of the same files:

| Key | Values | Meaning |
|---|---|---|
| `carry_in` | list of paths | untracked files copied into the worktree |
| `idle_timeout` | duration (`30m`, `2h`, `90s`) | stop a session nobody has used for this long. Unset means never |
| `runtime` | `auto` \| `docker` \| `sbx` | which backend to use; `auto` prefers `sbx` when present |
| `persistence` | `dtach` \| `none` | whether harnesses survive the terminal closing |
| `account` | account name | which captured login this project uses |

## MCP servers

```console
$ omh config mcp ls
$ omh config mcp add linear npx -- -y mcp-remote https://mcp.linear.app/sse
$ omh config mcp rm linear
```

MCP lives under `config` because MCP servers **are** configuration. They live in
your catalogue, and `omh config mcp add` writes there — the catalogue is not
committed, so nothing here reaches a teammate by `git clone`.

### A token for one repo

A server's environment applies in every repo you work in, which is the wrong
scope for a key scoped to one of them. So a repo overrides the environment
without redeclaring the server:

```toml
# <repo>/.omh/settings.local.toml
[mcp.linear.env]
LINEAR_API_KEY = "..."
```

Variable by variable, so overriding one does not drop the rest of an entry you
never saw. There is deliberately no `command` here: a repo configures a server
from your catalogue, it does not define one. An override naming a server the
catalogue does not have is an error — a token going nowhere is exactly the shape
of a setting somebody swears they configured.

### Importing what you already have

```console
$ omh config mcp import claude
```

Nobody retypes MCP servers they have already configured, so `import` is the
on-ramp. It is the exact inverse of the renderers, which forces a real
constraint: **every format that renders must also parse, and the pair must
round-trip.** Otherwise import silently drops fields. That is a test, not a hope
— see [Adapters](design/adapters.md#renderers).

Import never clobbers. Each server is added, recognised as already identical, or
reported as a conflict and left alone; re-running is a no-op.

Import paths expand against the **host**, deliberately using a different
expansion than everything else — the guest home would send import looking into a
filesystem that does not exist yet.

**Planned:** extending import beyond MCP to rules, skills, hooks and commands,
plus a `plugin` capability that reads Claude marketplace plugins and re-renders
them for other harnesses. See [roadmap](design/roadmap.md).

## `carry_in`

A git worktree contains only **tracked** files. No `.env`, no certs — so without
help both the agent and your IDE land somewhere that cannot run your app.

```toml
carry_in = [".env.local", "certs/"]
```

```console
$ omh claude
omh: carried .env.local
omh: carried certs/
omh: warning: carry_in lists .env.missing — not in this checkout
```

**This is the only path by which a secret reaches the agent.** That is why it is
an explicit allowlist, why omh prints what it carried, and why patterns are
validated: `carry_in` can be read from a *committed* file, so an entry like
`../../.ssh` would otherwise copy host secrets into a sandbox the agent controls.

A **missing** path is reported, never skipped — a `.env` you believe you are
carrying and are not is exactly what wastes an hour inside the sandbox.
Re-running copies only what changed; the checkout stays the source of truth.

**Copy, not symlink.** A symlink's target would have to resolve inside the
sandbox, which would mean mounting your main checkout — exposing the uncommitted
work the worktree model exists to protect.

`node_modules` is deliberately not carried. It is built in the sandbox, for the
sandbox's platform.

### Keeping the agent's `git status` clean

Carried files must not show up as untracked, or the agent is invited to commit
your `.env` onto the session branch.

omh's own `CLAUDE.md` / `AGENTS.md` are not written into the worktree at all —
they are **mounted read-only** over their declared filenames. Writing them there
made omh's staging indistinguishable from the agent's work, and `info/exclude`
could not help: gitignore semantics say nothing about a file git already tracks,
so a repo that commits its own `CLAUDE.md` saw a permanent modification nobody
made, and `omh s commit` published omh's rules over the project's conventions.

**The mount composes your project's rules rather than replacing them.** The
repo's own `AGENTS.md` is read before anything is mounted over it and joined
into the document the agent gets, after your catalogue's rules:

```
<!-- omh: tdd -->                ~/.omh/rules/tdd.md
<!-- omh: commit-style -->       ~/.omh/rules/commit-style.md
<!-- omh: <repo>/AGENTS.md -->   the project's own, tracked
<!-- omh: base:graph-rules -->   omh's own, from the base set
```

Your rules come first because they are how you work everywhere and the project's
file is the specific case that qualifies them. Within the catalogue they join in
filename order — a placeholder, honestly: rules build on each other, and the
only place an order can really come from is the list you wrote. `[use]` supplies
one when it lands.

**omh's own sections come last**, generated from the
[base set](design/base-set.md) rather than written into a file. They describe
the sandbox — what git does here, where notes go, which graph answers what — and
a convention the project wrote down should not have omh's account of the box
sitting in front of it. Each is an entry: `omh why git-rules` states what it
costs and how to switch it off.

Content comes from the worktree if the branch has a copy, otherwise from the
default branch — so a session that has just written its rules is governed by
them, and one that never had them still gets the project's. The marker says
which, because the two are not the same claim:

```
<!-- omh: <repo>/AGENTS.md -->   this branch's copy
<!-- omh: main:AGENTS.md -->     the branch has none; read from main
```

A **blank** rules file counts as no rules file. omh has to place an empty file
at each declared name for the mount to land on, so from a session's second
launch its own placeholder is sitting in the worktree — read as content it would
outrank the project's real rules under the canonical name.

A repo with `CLAUDE.md` and no `AGENTS.md` is composed anyway, and omh says
which file it read. Where both exist and differ, `AGENTS.md` wins and the other
is reported rather than silently dropped:

```console
$ omh claude
omh: composed CLAUDE.md — rename it to AGENTS.md
```

`carry_in` is for files git does **not** track. A tracked path is already in the
worktree, so listing one is a misconfiguration: omh says so at launch and does
not copy it, and `omh s commit` refuses to publish it if a running session got
one before the list was fixed.

Two traps here, both found by running it rather than reasoning about it:

- **`<worktree>/.git` is a *file***, not a directory — it points at the admin
  directory elsewhere. A test that builds a fake `.git/info` passes happily while
  the real thing does nothing.
- **git reads `info/exclude` from the *common* git dir**, not the per-worktree
  one. Checked empirically: a per-worktree exclude leaves `?? .env.local` in the
  status; the common one hides it. Worth naming the consequence — that file is
  shared with your main checkout. It is never committed, and carried paths are
  untracked there by definition, so the effect is invisible; but it is not
  scoped to the worktree, and that is a fact rather than an intention.
