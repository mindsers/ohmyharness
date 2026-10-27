---
title: Configuration
description: Where omh's configuration lives, what a repository takes from your catalogue, every setting with its default, and how to write a hook.
---

Configure what every session gets: the rules, skills, MCP servers and hooks it
is offered, and the settings its sandbox runs with. You declare your setup once,
and omh renders it into whatever shape each harness reads.

| Where | What it holds |
|---|---|
| `~/.omh/` | **your catalogue** — every rule, skill, command, subagent, hook and MCP server you hold. Personal, and never committed |
| `<repo>/.omh/` | **this repository** — what it takes from your catalogue, its settings, and hooks bound to its own commands. Committed, except `settings.local.toml` |

Change this repository with `omh use` and `omh set`, and the defaults a new
repository starts from with `omh settings` — see
[Two scopes, two commands](#two-scopes-two-commands).

## At a glance

| I want to… | Run | Read |
|---|---|---|
| use one more skill in this repo | `omh use skills <name>` | [`[use]`](#use--what-this-repo-takes-from-your-catalogue) |
| stop using an MCP server here | `omh unuse mcp <name>` | [`[use]`](#use--what-this-repo-takes-from-your-catalogue) |
| switch the code graph off here | `omh set codegraph off` | [`[omh]`](#omh--omhs-own-features) |
| give the sandbox a `.env` git does not track | `omh set carry_in '[".env"]'` | [`carry_in`](#carry_in) |
| build behind a network that inspects TLS | `omh set --local ca_cert ~/corp-root.pem` | [`ca_cert`](#ca_cert) |
| add an MCP server to my catalogue | `omh settings mcp add <name> <command> …` | [MCP servers](#mcp-servers) |
| bring in the skills I already have in Claude Code | `omh import skills claude` | [Importing the rest](#importing-the-rest) |
| refuse edits to code whose test has not changed | `omh use hooks tdd-guard` | [Guards, and their bypass](#guards-and-their-bypass) |
| see what is in effect here, and what decided it | `omh info --repo` | [Settings, and their two layers](#settings-and-their-two-layers) |

The settings a repository holds, in `<repo>/.omh/settings.toml` or, for what
stays on your machine, `settings.local.toml`:

| Key | What it does |
|---|---|
| [`[use]`](#use--what-this-repo-takes-from-your-catalogue) | which of your catalogue's entries this repo uses, one list per capability |
| [`[omh]`](#omh--omhs-own-features) | switches omh's own features — `codegraph`, `git-notice`, `memory` — on or off here |
| [`[provision]`](#provision--what-your-sandbox-is-built-with) | which parts of each stack go into the sandbox image |
| [`[mcp.<name>.env]`](#a-token-for-one-repo) | environment for one of your MCP servers, in this repo only |
| [`carry_in`](#carry_in) | untracked files copied into the worktree |
| [`ca_cert`](#ca_cert) | a root certificate the sandbox trusts |
| [`runtime`](#settings) | docker, podman or Docker Sandboxes |
| [`idle_timeout`](#settings) | stop a session nobody has used for this long |
| [`persistence`](#settings) | whether harnesses survive the terminal closing |
| [`sandbox_memory`, `sandbox_cpus`](#settings) | how much memory and CPU a session's sandbox may use |
| [`account`](#settings) | which captured login this project uses |

And the files that are not settings:

| File | What it is |
|---|---|
| `<repo>/.omh/hooks/<name>.json` | a hook bound to a command only this repo has — [Writing a hook](#writing-a-hook) |
| `<repo>/.omh/stacks/<name>.toml` | an ecosystem you taught omh — [Stacks](/reference/stacks/) |
| `<repo>/.omh/memory.toml` | how a note's key is derived — [Memory](/reference/memory/#keys) |
| `<repo>/AGENTS.md` | the project's own rules, composed into every session — [How your rules reach the agent](#how-your-rules-reach-the-agent) |
| `~/.omh/default.toml` | what `omh init` seeds a new repo's settings from, and nothing more — [Settings, and their two layers](#settings-and-their-two-layers) |
| `~/.omh/mcp.json` | your MCP servers — [MCP servers](#mcp-servers) |

## One catalogue, and it is personal

Everything you hold lives once, in your catalogue, and a repository names what
it takes from it.

```
~/.omh/
  rules/  skills/  commands/  subagents/  hooks/   the only place these live
  mcp.json
  default.toml                                     what a new repo starts from
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
  hooks/               committed: hooks bound to commands only this repo has
  stacks/              committed: an ecosystem you taught omh, if you had to
<repo>/AGENTS.md       the project's own rules — tracked, and actually read
```

**A project cannot declare a skill, an MCP server, a command or a subagent.**
It names ones from your catalogue, in
[`[use]`](#use--what-this-repo-takes-from-your-catalogue). Hooks are the one
exception.

### Why hooks are the exception

**Content lives where its scope is**, and a hook is the one capability that can
have either scope. A hook that works anywhere lives in your catalogue, as the
ones omh ships do. A hook bound to a command only this repo has lives in
`<repo>/.omh/hooks/`.

**A shipped hook names the ecosystem it belongs to**, and nothing else about it:

```json
{ "on": "turn-end", "stack": "rust", "run": "cargo test" }
```

That is a *reference*. The marker that decides whether a repo is a rust project
stays in the stack definition, so the two can never disagree.

**A hook naming an ecosystem you are not is not offered to you.** `omh init` in
a rust repo does not put `go-test` in your `[use]` list, and the launcher does
not report it as something you are not using.

**A hook that names no stack belongs everywhere**, which is most of them.

**A project hook beats a catalogue hook of the same name**, which is how a repo
overrides your personal `format` hook with the one it actually needs, without
renaming anything. A name from the base set is the exception: it is reserved —
see [Names omh ships are omh's](#names-omh-ships-are-omhs).

<details>
<summary>Why a skill is yours and a hook can be the project's</summary>

A skill is a way *you* work — it travels with you across repos, which is why it
belongs to you. Some hooks are the same: `cargo test` is what a rust project
runs, not what *this* rust project runs, so omh ships a test and a format hook
for go, python and rust, and they live in your catalogue like everything else.

But a hook is also the one capability that can bind to a command only this repo
has — an integration suite behind a script, a linter with the project's own
config. Those have to be declarable where the project is, or the catalogue fills
with entries that are only ever right in one place.

So the rule is not "no content in the repo". It is **content lives where its
scope is**.

</details>

### What that costs

**A repo cannot ship a skill, an MCP server or a command to your teammates.**
What a repo still shares is its rules file — which for the first time actually
reaches the agent — its hooks, and its settings.

**A committed hook is executable content, and cloning a repo runs it.** So every
launch prints `this repo's hooks:` and their names, and under it
`new or changed since you last ran here:` with the ones that are. A hook runs
inside the sandbox, as the agent — it reaches what the agent reaches, and
nothing the agent cannot.

**There is no `<repo>/.omh/rules/`.** A project's rules are its `AGENTS.md`,
which is tracked, reviewed and shared, and anything of yours that applies here
is a rule in your catalogue that this repo selects.

<details>
<summary>What the committed layer used to do, and why rules did not earn a directory</summary>

The committed layer used to ship skills, MCP servers and commands to your
teammates. Recorded, not built: a catalogue entry could carry a `source` and
`omh sync` could fetch the missing ones, which restores team sharing without
putting content back in the repo.

Hooks are the one thing a repo can hand you that executes, which is why the
mechanism narrates itself at every launch.

Hooks earned a place in the repo by binding to commands only that repo has.
Rules did not.

</details>

## Settings, and their two layers

A setting has one value, and two files in the repository can give it one. Later
wins:

```
<repo>/.omh/settings.toml  →  <repo>/.omh/settings.local.toml
```

| File | In git | What belongs in it |
|---|---|---|
| `settings.toml` | committed | the rules for a project — what a teammate cloning it should get |
| `settings.local.toml` | gitignored | your own overrides, and a value that reaches a credential, like the API key that makes one of those rules work |

**Both are in the repo**, and that is the point: a repo's behaviour is explained
by files the repo contains — which is what a teammate cloning it can actually
see, and what `omh info --repo` can account for without pointing at a file they
do not have.

**`~/.omh/default.toml` is not a third layer.** It is the *template* `omh init`
seeds a new repo from — read once, at `init`, and never at launch. See
[`omh settings`](/reference/commands/#omh-settings).

**Every effective value reports where it came from and what it beat:**

```console
$ omh info --repo
this repo /Users/you/proj/.omh

settings
  carry_in  [".env.local"]  ← local (overrides shared)

omh's features
  codegraph   off here
  git-notice  on
  memory      on

using
  rules      commit-style, tdd · +3 from omh's features
  skills     review-diff                                      (1 more in your catalogue)
  mcp        +2 from omh's features
  commands   nothing
  subagents  nothing
  hooks      rust-format, rust-test · +6 from omh's features  (4 more in your catalogue)

1 catalogue entry is not selected here: skills/refactor

  omh use skills refactor    ·    omh use --all
```

<details>
<summary>Why settings keep layers when content does not, and what <code>default.toml</code> used to be</summary>

Content has one home; **settings** keep their layers, because a setting has one
value and the useful question is which file decided it.

`default.toml` was `settings.toml`, and a layer like the two above, until
0.7.0. A file that decides nothing should not be spelled like the two that
decide everything.

Layered settings are undebuggable without provenance — the standard complaint
about oh-my-zsh, and the thing [trust](/concepts/trust/) exists to prevent.
Hence every value says what decided it.

</details>

## Two scopes, two commands

`omh set`, `omh use` and their opposites change **this checkout**.
`omh settings` changes **you** — the defaults a new repo starts from.
`omh info --repo` shows the first, and `omh info` the second.

```console
# this repo
$ omh use skills tdd                    # → <repo>/.omh/settings.toml   (committed)
$ omh unuse mcp linear
$ omh use --all                         # resync every list to the catalogue
$ omh set codegraph off                 # → [omh] in settings.toml
$ omh set codegraph on
$ omh set carry_in '[".env"]'           # → settings.local.toml         (gitignored)
$ omh set idle_timeout 30m              # → settings.toml               (committed)
$ omh set --local idle_timeout 45m      # → settings.local.toml, because you said so
$ omh set --save carry_in '[".env"]'    # → settings.toml, because you said so
$ omh unset carry_in
$ omh info --repo                       # what is effective here, and what decided it

# you, everywhere
$ omh settings set idle_timeout 45m     # → ~/.omh/default.toml, seeds new repos
$ omh settings unset idle_timeout       # new repos are no longer seeded with it
$ omh settings edit                     # $EDITOR on your defaults
$ omh settings edit skills tdd          # $EDITOR on one catalogue entry
$ omh settings                          # your defaults
$ omh info                              # and what you have here
```

These commands ask *where the value already is* before they ask anything else:

| Command | Writes to | Why that default |
|---|---|---|
| `omh set` / `unset` | **where it already is**, else the key decides | one rule; the classification lives with the key rather than in your memory |
| … with `--save` | `settings.toml`, **committed** | you named the file, and writing a committed one is said out loud |
| … with `--local` | `settings.local.toml`, **gitignored** | the same, in the direction that keeps a value off your teammates' machines |
| `omh use` / `unuse` | `settings.toml`, **committed** — and `settings.local.toml` as well when it already declares that list | what a project uses is a fact about the project; a list left standing in the gitignored file would outrank the write |

**The key decides the file, so a credential stays out of git with no flag.**
`omh set` defaults to the committed file, because most settings — what runtime a
project wants, how long its sessions idle — are facts about the project that a
teammate cloning it should get. What keeps a credential out of git is
`src/key.rs`: a table saying, per key, whether a value there can name one.
`carry_in` is in it, so `omh set carry_in …` still writes the gitignored file
with no flag at all.

**Every write reaches every layer that already holds the key**, so one cannot
land under a value that outranks it. `--local` and `--save` step outside that on
purpose — you named the file — and say so when the result is a value you cannot
observe changing.

**Every write names the file, and whether git carries it:**

```console
$ omh set carry_in '[".env"]'
wrote → /Users/you/proj/.omh/settings.local.toml (gitignored)

$ omh set idle_timeout 30m
wrote → /Users/you/proj/.omh/settings.toml (committed)
```

The word in brackets is the point, and it is there because the path is not
enough: `settings.toml` and `settings.local.toml` do not read as opposites at a
glance in a line eighty columns wide.

**`omh set` makes its own premise true.** The gitignored file is only safe
because git ignores it, so `omh set` adds the ignore line when nothing is
ignoring the file:

```console
$ omh set carry_in '[".env"]'
omh: nothing was ignoring settings.local.toml — added it to /Users/you/proj/.omh/.gitignore
wrote → /Users/you/proj/.omh/settings.local.toml (gitignored)
```

**A key omh has never heard of is refused**, with the one it was likely meant
as. `--anyway` writes it — for a key only a newer omh reads — to the committed
file, and reports it twice: nothing reads it, and it went somewhere git carries:

```console
$ omh set carry_ins '[".env"]'
omh: nothing in omh reads `carry_ins` — did you mean `carry_in`? Nothing was written. For a key only a newer omh reads:
  omh set --anyway carry_ins <value>

$ omh set --anyway carry_ins '[".env"]'
omh: nothing in omh reads `carry_ins` — written anyway, and it will sit there
wrote → /Users/you/proj/.omh/settings.toml (committed)
omh: and the committed file is COMMITTED — `carry_ins` went into a file git carries
```

A test fails the build if a key omh's own code reads is missing from the table,
which is what stops omh's own keys ever taking that path.

**A value omh cannot read is refused**, where it was typed rather than at the
next launch, minutes later and in another command. `--anyway` writes it — for
a value only a newer omh takes — and still names it:

```console
$ omh set persistence tmux
omh: `persistence` does not take `tmux` — only dtach, none. Nothing was written. For a value only a newer omh reads:
  omh set --anyway persistence <value>

$ omh set --anyway persistence tmux
omh: `persistence` does not take `tmux` — only dtach, none — written anyway
wrote → /Users/you/proj/.omh/settings.toml (committed)
```

`omh settings set` is the same `set`, for your defaults. A list key takes a
list: `carry_in .env` is refused, and `carry_in '[".env"]'` is the spelling.

**Asking for the committed file still says what that means**, and names the key
when the key is one that reaches a credential:

```console
$ omh set --save carry_in '[".env"]'
wrote → /Users/you/proj/.omh/settings.toml (committed)
omh: the committed file is COMMITTED — never put a secret here
omh:   `carry_in` is one of those — it belongs in /Users/you/proj/.omh/settings.local.toml
```

**When the gitignored file already declares the `[use]` list or the `[omh]`
switch you are writing, the write reaches it** — it is the layer that decides,
and a command that reported success while that layer overruled it would be
lying. Never a layer that did not already declare it: a selection appearing in
a gitignored file is how a teammate stops getting what the repo says it uses.

**`unset` removes a value rather than forcing one**, which is what lets the
layer beneath, or omh's own default, take over again. Plain `omh unset` removes
the key from every repo layer that holds it; `omh unset --local` drops only your
own override, and leaves a team default standing.

<details>
<summary>Why the key decides, and what the command decided before 0.7.0</summary>

**The two scopes wanted opposite defaults**, which is why one `--layer` flag
could not serve both — and why these commands ask *where the value already is*
before they ask anything else.

**The protection moved from the command to the key.** Before 0.7.0 the
repo-scoped write sent every value to the gitignored file: the safety came from
the destination, and no value could reach git unasked — at the price that a
teammate cloning the repo got none of your settings.

The ignore line used to be written by `omh init` alone — so in a repo that had
never been `omh init`ed, `omh set carry_in` left a credential map that
`git add .` would stage.

</details>

### `omh unset` reaches every repo layer that holds the key

Plain `omh unset` removes the key from every repo file that holds it, so a
value you wrote to both files leaves both:

```console
$ omh unset carry_in
removed carry_in from the shared layer
removed carry_in from the local layer
```

`--save` and `--local` still mean that file alone, so anything still supplying
the value after a removal is named:

```console
$ omh set --local idle_timeout 15m
wrote → /Users/you/proj/.omh/settings.local.toml (gitignored)

$ omh unset --save idle_timeout
idle_timeout was not set in the shared layer
omh: `idle_timeout` is still set in the local layer — /Users/you/proj/.omh/settings.local.toml
```

<details>
<summary>The defect this rule was written after</summary>

`unset` reaches every layer holding the key — not the one `omh set` would have
written. Those are different questions, and answering the second one shipped a
defect worth recording. `omh set --save carry_in` followed by
`omh set --local carry_in` leaves the key in **both** repo files; omh wrote
both. An `unset` that consulted `set`'s rule removed the gitignored copy, said
so, exited 0, and left a map to a credential standing in the file git carries.
The command you run to get a secret out of git reported success and did not do
it.

</details>

### A write something outranks says so

`settings.local.toml` wins at read time, so writing the committed file
underneath a standing local value changes nothing you can observe. Without a
flag that cannot happen — the rule reaches every layer already holding the key.
`--save` walks past it on purpose, and no longer does so quietly:

```console
$ omh set --save idle_timeout 30m
wrote → /Users/you/proj/.omh/settings.toml (committed)
omh: the committed file is COMMITTED — never put a secret here
omh: `idle_timeout` is still 15m here — the local layer sets it, and that outranks what was written
```

### `omh why <key>` says what a key is for

The classification is a table in the binary, so a settings file cannot show it:

```console
$ omh why carry_in
`carry_in` is a setting omh reads.

  Untracked files a session needs — a worktree holds tracked files only. The one
  path by which a secret reaches the agent, so keep it short.
  takes  a TOML array of paths, e.g. [".env"]
  kept   /Users/you/proj/.omh/settings.local.toml (gitignored)

  A value here can name a credential, which is why omh keeps it
  out of the file git carries.
```

## Settings

The top-level keys of the same two files.

| Key | Values | What it does | Default |
|---|---|---|---|
| `carry_in` | list of paths | untracked files copied into the worktree — see [`carry_in`](#carry_in) | nothing is carried |
| `ca_cert` | a path to a PEM file | a root certificate the sandbox trusts — see [`ca_cert`](#ca_cert) | none |
| `idle_timeout` | duration (`30m`, `2h`, `90s`) | stop a session nobody has used for this long | never |
| `runtime` | `auto` \| `docker` \| `podman` \| `sbx` | which backend to use | `auto`, which means docker |
| `persistence` | `dtach` \| `none` | whether harnesses survive the terminal closing | `dtach` |
| `sandbox_memory` | as docker spells it: `4g`, `512m` | how much memory a session's sandbox may use | docker's default |
| `sandbox_cpus` | as docker spells it: `2`, `1.5` | how many CPUs a session's sandbox may use | docker's default |
| `account` | an account name, or an `[account]` table | which captured login this project uses | the one login captured for the harness |

```toml
# <repo>/.omh/settings.toml
idle_timeout   = "30m"
sandbox_memory = "4g"
```

**`auto` means docker**, the measured backend. `podman` speaks docker's CLI and
adds a rootless user-namespace mapping; `sbx` is
[Docker Sandboxes](#the-sbx-runtime-docker-sandboxes). Both `podman` and `sbx`
are explicit opt-ins, unmeasured in `auto` until `omh doctor` confirms them —
set them explicitly and run `omh doctor` first.

**Changing `sandbox_memory` or `sandbox_cpus` restarts the sandbox** on the next
launch.

**`account` becomes a table once one name does not fit every harness.** A
harness's own entry wins, at whatever layer it is set; `omh set account
codex:work` writes one:

```toml
[account]
default = "mine"
codex   = "work"
```

`omh unset account codex` takes codex's entry back out, leaving the rest. Your
defaults take the same two lines — `omh settings set account codex:work`,
`omh settings unset account codex` — and `omh init` seeds the table into a new
repo's `settings.toml`, where `omh set account` writes it. `omh settings` says
it as one line:

```text
your defaults ~/.omh/default.toml
  account       mine · claude:work
```

**With no `account` set**, a harness with one captured login uses it, one with
several refuses to launch until you pick one with `omh set account <name>`, and
one with none starts logged out. [Accounts](/guides/accounts/) has the rest.

## `[use]` — what this repo takes from your catalogue

The catalogue is everything you have. `[use]` is what *this* project uses — one
list per capability, naming entries from your catalogue.

<div class="steps">

1. See what this repo uses, and what it leaves out.

   `omh info --repo` lists each capability, and names every catalogue entry
   that is not selected here.

2. Add or drop an entry.

   `omh use skills review-diff` adds one, `omh unuse mcp linear` drops one, and
   `omh use --all` resyncs every list to the whole catalogue. Each writes
   `<repo>/.omh/settings.toml`, which is committed.

3. Or edit the lists yourself.

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

</div>

| A list that is… | Means |
|---|---|
| absent | everything in your catalogue |
| `[]` | nothing |
| `["*"]` | everything — "keep following the catalogue as it grows", written down |
| `["a", "b"]` | exactly these; for `rules`, in this order |

**One mechanism: an allowlist.** No `exclude`, no `include`/`exclude` pair.
Removing something is deleting its name, and there is one place to look to
answer "is this on here".

**Absent means everything; `[]` means nothing.** Those differ on purpose: a repo
that never configured a selection gets the whole catalogue, so upgrading changes
nothing and a new checkout is useful before it is configured — while a list you
emptied by deleting its last name means what it says.

**For `rules`, the list is the order.** Rules build on each other, and a general
one followed by its exception reads differently reversed. Without a list they
compose in filename order, which is the fallback rather than the plan.

**It layers like everything else in these files: later wins, per capability,
wholesale.** A list in `settings.local.toml` replaces the committed list for
that capability rather than adding to it. Your `~/.omh/default.toml` can carry a
default selection — a `[use]` list, or an `[unuse]` list of what a kind with no
list leaves out — which `omh init` writes into a new repo as its lists.

<details>
<summary>Why a list is replaced rather than merged</summary>

Merging would let a repo add to your list and never take anything off it, which
is the thing an allowlist exists to make possible.

</details>

### A feature is not selectable

`omh-codegraph` and `omh-memory` are omh's servers, generated at launch with
their features' hooks and rules sections — not entries of your catalogue. They
are not yours to select, under those names or under `codegraph` and `memory`,
the names they had before, which are also their features' names:

```console
$ omh use mcp codegraph
omh: mcp/codegraph is omh's — part of the `codegraph` feature. `[use]` names
     your entries; a feature is all or nothing, so `omh set codegraph on` and
     `omh set codegraph off` are its switches, and `omh unset codegraph` hands
     the decision back to omh's own default.
```

An empty `[use]` leaves every omh feature whole — server, hooks and rules
section together. Taking one apart is the state `[omh]` refuses to let anybody
express, and `[use]` does not get a second door to it.

**One command, two tables, and the refusal teaches the difference.**
`omh use`/`unuse` name catalogue entries; `omh set <feature> on|off` writes
`[omh]`. `omh set` reads the name first and refuses the wrong kind — a
catalogue entry is answered with `omh use <capability> <name>`, an entry that
belongs to a feature is answered with the feature — so the difference between
*an entry you chose* and *a feature omh ships* is stated where somebody just
guessed at it, rather than only here.

### What is not selected gets said out loud

`omh init` writes `[use]` with every entry named, because an explicit list is
editable and reviewable in a way `"*"` is not — you curate by deleting lines.
That has one failure mode, and it gets the treatment every silence in omh gets:

```console
$ omh new claude
omh: 1 catalogue entry is not selected here: skills/refactor
omh:   omh use skills refactor    ·    omh use --all
omh: warning: [use] names an entry nothing answers to: skills/reveiw-diff
```

Neither is fatal. A typo in a list is something to be told about, not a reason
to refuse to start work.

### Curation, not confinement

**`[use]` decides what the harness is offered; it is not a boundary**, and omh
does not claim it as one. The catalogue directory behind it is mounted into the
sandbox whole and read-only, so an unselected skill is not loaded but is still
readable at an internal path by an agent that goes looking for it. Selection is
for keeping a project's context to what the project needs. What is guaranteed is
the read-only part: the agent can read a selected skill and cannot write one.

## `[omh]` — omh's own features

`<repo>/.omh/settings.toml` says what this repo does with **omh's own
features**:

```toml
[omh]
codegraph = false     # the server, its four hooks, its section of the rules and its index volume
```

| Feature | What it brings | Default |
|---|---|---|
| `codegraph` | the code graph's MCP server; the hooks `graph-orient`, `graph-first`, `graph-read` and `graph-refresh`; the `graph-rules` section; the index volume's mount | on |
| `git-notice` | the hooks `git-note` and `git-turn`; the `git-rules` section | on |
| `memory` | the memory MCP server; the `memory-rules` section; the note store's mount and the server binary's | on |

There are three ways to take a feature away, at different reaches:

| To | Run | Reaches |
|---|---|---|
| switch it off in this repo | `omh set codegraph off` | this repo — the server, its hooks and its rules section together |
| hand the decision back to omh's own default | `omh unset codegraph` | this repo |
| switch it off in every repo `init` sets up after | `omh settings set codegraph off` | every new repo |

**Feature names only.** `graph-first = false` is refused, naming the feature it
belongs to — keeping the graph while dropping one of the things that make it
used is taking a bundle apart, not changing a setting, and "graph on, refresher
off" is a graph that quietly stops tracking the code.

**Disabling is not removal.** Nothing is uninstalled: the feature's parts are
left out of what *this* repo's sessions are given, and the next repo gets them
back.

**`omh set codegraph off` takes the hooks and rules section with the server**,
because a hook nudging the agent toward a server that is gone is worse than no
hook.

**It layers like every other setting** — this file, then
`<repo>/.omh/settings.local.toml`, which `omh set` adds to `.omh/.gitignore`
the first time it writes there. `~/.omh/default.toml` can carry an `[omh]`
table, which seeds a new repo; it decides nothing in a repo that already
exists.

## `[provision]` — what your sandbox is built with

`[provision]` records which parts of each stack go into this repo's sandbox
image.

`omh init` works out which ecosystems this repo is — a `Cargo.toml`, a
`package.json` — and then asks the **sandbox**, with your repo mounted
read-only, which parts of them apply here. A repo with a `pnpm-lock.yaml` gets
pnpm; the yarn and bun provides do not apply and are not installed. What it
decided is written into your committed settings:

```toml
# <repo>/.omh/settings.toml
[provision]
"rust/toolchain" = true
"rust/linker" = true
```

| Value | Written by | Means |
|---|---|---|
| `true` | `omh init` — omh only ever writes `true` | install this provide in the image |
| `false` | you — it can only have been typed, so it is treated as a decision and left alone | leave it out |

Keyed `"<stack>/<provide>"`; [Stacks](/reference/stacks/) has what a provide is
and how its condition is read. To leave one out:

```toml
[provision]
"rust/linker" = false   # this base image already has cc; do not spend 124 MB on it
```

**That table is the input to everything afterwards.** Your sessions run an image
built from exactly these, so a teammate who clones the repo gets the same
sandbox without being asked anything, and neither `omh new` nor `omh sNN
resume` re-evaluates a condition.

**An opt-out changes what goes into the image, not what omh says about it.** If
one of your hooks needs the program, omh still asks the sandbox whether it is
there, and still holds the hook back by name if it is not.

**A provide you want left out on your machine alone belongs in
`settings.local.toml`**, where it says nothing to anyone else — and omh will
never copy it into the committed file.

**A launch does not ask again; `omh upgrade` does.** So a table written in
April and a `yarn.lock` swapped for a `pnpm-lock.yaml` in May disagree until
something looks, and until then the table wins: the image still has yarn. What
tells you is a hook — `pnpm` does not resolve, so the hook that needs it is
[held back by name](#hooks-your-sandbox-cannot-run).
[`omh upgrade`](/reference/commands/#omh-upgrade) is what looks. It asks the
sandbox the same conditions `init` did, takes out the entry that stopped being
true, adds the one that started, and builds from the result — leaving every
`false` where it is, and changing nothing for a condition that could not
answer.

## Hooks your sandbox cannot run

A hook that needs a program the sandbox image does not have is held back, by
name, until the image has it.

omh works out which ecosystems this repo is from its manifests, and the hooks
for them are catalogue entries this repo turns on through `[use]`. Detection
runs on your machine; the hook runs in the sandbox, and those are different
computers. So omh provisions the toolchain into the image — that is what
[`[provision]`](#provision--what-your-sandbox-is-built-with) records — and then
**measures** what it got.

```
  held back  `rust-test` needs `cargo` — not installed in this repo's sandbox
             the hook file is written and travels; it runs as soon as the
             sandbox has it
```

**It is said at `omh init`, and again at every launch** — in the status line of
`omh new` and `omh sNN resume`, and named individually by `omh sNN attach` — in
the same list as a hook your harness cannot spell. A held-back hook is never
silently absent.

**It is re-decided from the measurement each time**, so a sandbox that gains
the tool gets its hook back with nothing to un-configure.

**A hook is selected either way.** What a missing program decides is whether the
hook **runs against this image**, and nobody is asked about it.

**Measurements are cached per image**, in `~/.omh/facts.json`, keyed by the tag
your sessions run. A repo whose hooks and stacks have not changed asks the
container nothing; add a hook naming a new program and that one program is
asked about, once. A missing or unreadable cache is measured again.

**A program omh could not measure is unknown, never assumed missing.** When the
sandbox cannot be asked, omh warns, and a hook whose program is unknown ships.

**Two things the measurement cannot see:**

- **It asks whether a program is installed, not whether what you run with it
  exists.** `npm` is in the base image, so a hook running `npm run format`
  passes whether or not the repo has a `format` script. The hooks omh
  [works out for you](#hooks-omh-works-out-for-you) are read from your scripts
  for that reason; one you wrote by hand is yours to get right.
- **It measures the image, not the session.** A tool that arrives at launch —
  mounted in, or installed by a hook that runs when the session starts — is
  there at run time and was not there when omh looked, so its hooks are held
  back wrongly. Nothing lets you say otherwise yet.

**`[toolchain]` was removed.** A repo that still has the table gets an error
naming it; delete it, and use `[provision] "<stack>/<name>" = false` if what you
want is a provide left out.

<details>
<summary>Why a missing program does not unselect a hook, why unknown is not missing, and what <code>[toolchain]</code> was</summary>

What omh ships and what your repo declares are both statements about the
*project* — committed, and the same for everybody who clones it. Whether `cargo`
is installed is a fact about one image, and it must not decide what the repo
says about itself: otherwise whoever ran `init` first imposes their machine on
the whole team.

An unknown program has to fall that way round: the other direction would switch
off every hook on the machine in a session that otherwise looks completely
normal.

`[toolchain]` recorded an answer to *"this sandbox lacks `cargo`, shall I switch
the hook off?"* — a question that asked you to configure around a broken
environment, and whose answer outlived the breakage. omh provisions the tool
instead.

</details>

## `carry_in`

`carry_in` copies files git does not track into the session's worktree. A git
worktree contains only **tracked** files — no `.env`, no certs — so without help
both the agent and your IDE land somewhere that cannot run your app.

<div class="steps">

1. List the untracked files a session needs.

   ```toml
   carry_in = [".env.local", "certs/"]
   ```

   `omh set carry_in '[".env.local", "certs/"]'` writes it to
   `settings.local.toml`, which is gitignored.

2. Start a session. omh says what it carried, and what it could not.

   ```console
   $ omh new claude
   omh: carried .env.local
   omh: carried certs/
   omh: warning: carry_in lists .env.missing — not in this checkout
   ```

</div>

**This is the only path by which a secret reaches the agent.** That is why it is
an explicit allowlist, why omh prints what it carried, and why patterns are
validated: `carry_in` can be read from a *committed* file, so an entry like
`../../.ssh` would otherwise copy host secrets into a sandbox the agent controls.

**A missing path is reported, never skipped** — a `.env` you believe you are
carrying and are not is exactly what wastes an hour inside the sandbox.

**Re-running copies only what changed**; the checkout stays the source of truth.

**A carried directory can still be removed by `git clean` in the sandbox**, and
omh warns when it carries one. Carrying the files individually keeps them.

**Copy, not symlink.** A symlink's target would have to resolve inside the
sandbox, which would mean mounting your main checkout — exposing the uncommitted
work the worktree model exists to protect.

**`node_modules` is deliberately not carried.** It is built in the sandbox, for
the sandbox's platform.

**A tracked path does not belong here** — see
[Keeping the agent's `git status` clean](#keeping-the-agents-git-status-clean).

### What stops a carried secret coming back

A carried file is not on the branch, so the agent can put it there — by
`git add -f`, by copying it under another name, or by pasting a value into
source. What omh checks depends on how you land the work:

| Landing | What is checked |
|---|---|
| [`omh sNN commit --keep`](/reference/commands/#omh-snn-commit---keep--the-agents-own-commits-curated), the agent's own commits | the **path**, and the **content**: omh searches the agent's commits for lines out of the files you carried, and refuses a harvest that did any of those three |
| `omh sNN commit`, one squashed commit | the **path**: a carried file that reached the index is refused. Copies under another name and pasted values are not searched for |

The content half works from lines, and four kinds of file never become any:

| | |
|---|---|
| not text | a keystore, a `.p12`, a DER key — there are no lines to search for |
| not there | deleted or renamed in your checkout since the launch |
| not readable | omh was not allowed to read it |
| nothing long enough | every line under twelve characters, or a comment |

**All four are reported rather than passed over.** They do not refuse a
harvest — omh will not lose an agent's work because it could not read a file —
but they no longer spell the same as *nothing carried reached the branch*:

```console
omh: the carried-file scan could not read these, so it cannot tell you whether
     their contents reached a commit:
    certs/deploy.p12 — it is not text, so there are no lines to search for
  the path itself is still checked — what is not is a copy under another name
```

`certs/` is the second example above, so this is the ordinary case rather than
the exotic one: a carried keystore is protected by its **path** and nothing
else. Copy it under another name in the sandbox and nothing catches it.

**The same warning appears at launch**, which is the moment you can still act on
it — carry the file in a form the scan can read, or accept that its path is the
whole guard.

### Keeping the agent's `git status` clean

Carried files must not show up as untracked, or the agent is invited to commit
your `.env` onto the session branch. So omh writes every `carry_in` pattern into
git's exclude list twice: for the worktree's own git, which runs on the
**host**, and for the different repository the agent reads inside the sandbox —
see [Sessions](/concepts/sessions/#the-agent-has-git-and-it-is-not-yours). Two
mechanisms, one list, so a `carry_in` entry keeps the file out of both
`git status`es.

**`carry_in` is for files git does not track.** A tracked path is already in the
worktree, so listing one is a misconfiguration: omh says so at launch and does
not copy it, and `omh sNN commit` refuses to publish it if a running session got
one before the list was fixed.

**The host's exclude list is shared with your main checkout.** git reads
`info/exclude` from the *common* git dir, not the per-worktree one, so your own
`git status` obeys it too: after a launch, a `carry_in` entry is ignored in your
checkout as well as in the session's worktree. omh only ever adds lines to that
file, and only the `carry_in` entries.

**Earlier releases also added `CLAUDE.md`, `AGENTS.md` and `.mcp.json`** — the
files omh mounts over — and with them there, a file by any of those names you
create in your checkout is invisible to `git status` and `git add -A`. omh
leaves those lines alone, because nothing marks them as omh's rather than
yours; if you did not write them, delete them from `.git/info/exclude`.

<details>
<summary>Two traps in hiding a file from git, both found by running it rather than reasoning about it</summary>

- **`<worktree>/.git` is a *file***, not a directory — it points at the admin
  directory elsewhere. A test that builds a fake `.git/info` passes happily while
  the real thing does nothing.
- **git reads `info/exclude` from the *common* git dir**, not the per-worktree
  one. Checked empirically: a per-worktree exclude leaves `?? .env.local` in the
  status; the common one hides it.

Both of those are about the worktree's own git, on the host. Inside the sandbox
the agent's exclude list is written separately, from the same `carry_in`
patterns, into the gitdir omh mounts.

</details>

#### How your rules reach the agent

omh's own `CLAUDE.md` / `AGENTS.md` are not written into the worktree at all —
they are **mounted read-only** over their declared filenames. **The mount
composes your project's rules rather than replacing them:** the repo's own
`AGENTS.md` is read before anything is mounted over it, and joined into the
document the agent gets, after your catalogue's rules:

```
<!-- omh: tdd -->                ~/.omh/rules/tdd.md
<!-- omh: commit-style -->       ~/.omh/rules/commit-style.md
<!-- omh: <repo>/AGENTS.md -->   the project's own, tracked
<!-- omh: base:graph-rules -->   omh's own, from the base set
```

**Your rules come first**, because they are how you work everywhere and the
project's file is the specific case that qualifies them. Within the catalogue
they join in the order `[use]` lists them, and in filename order when this repo
has no list.

**omh's own sections come last**, generated from the
[base set](/reference/base-set/) rather than written into a file. Each is an
entry: `omh why git-rules` states what it costs and how to switch it off.

**Content comes from the worktree if the branch has a copy, otherwise from the
default branch** — so a session that has just written its rules is governed by
them, and one that never had them still gets the project's. The marker says
which, because the two are not the same claim:

```
<!-- omh: <repo>/AGENTS.md -->   this branch's copy
<!-- omh: main:AGENTS.md -->     the branch has none; read from main
```

**A blank rules file counts as no rules file.** omh has to place an empty file
at each declared name for the mount to land on, so from a session's second
launch its own placeholder is sitting in the worktree — read as content it would
outrank the project's real rules under the canonical name.

**A repo with `CLAUDE.md` and no `AGENTS.md` is composed anyway**, and omh says
which file it read. Where both exist and differ, `AGENTS.md` wins and the other
is reported rather than silently dropped:

```console
$ omh new claude
omh: composed CLAUDE.md — rename it to AGENTS.md
```

<details>
<summary>Why the rules file is mounted rather than written, and why omh's sections come last</summary>

Writing omh's rules files into the worktree made omh's staging
indistinguishable from the agent's work, and `info/exclude` could not help:
gitignore semantics say nothing about a file git already tracks, so a repo that
commits its own `CLAUDE.md` saw a permanent modification nobody made, and
`omh sNN commit` published omh's rules over the project's conventions.

omh's sections describe the sandbox — what git does here, where notes go, which
graph answers what — and a convention the project wrote down should not have
omh's account of the box sitting in front of it.

</details>

## `ca_cert`

`ca_cert` adds one root certificate to what the sandbox trusts, for a network
that inspects TLS.

**You need it behind Zscaler, Netskope or a corporate MITM proxy.** They
terminate every HTTPS connection and re-sign it with the company's own root.
Your machine trusts that root because IT installed it. A container does not, so
fetches inside the sandbox fail on an unknown issuer — the graph download and
`npm install -g` for the harness are the two that stop an image being built at
all. The wording of the failure depends on which tool reaches the network first:

| Tool | Says |
|---|---|
| curl | `SSL certificate problem: unable to get local issuer certificate` |
| python | `CERTIFICATE_VERIFY_FAILED` |
| node | `UNABLE_TO_VERIFY_LEAF_SIGNATURE` |

<div class="steps">

1. Get the root as a PEM file.

   It is usually not sitting on disk. On macOS it is in the System keychain:

   ```console
   $ security find-certificate -a -c "Zscaler" -p > ~/corp-root.pem
   ```

   On Linux, IT has normally put it in `/usr/local/share/ca-certificates/` or
   `/etc/pki/ca-trust/source/anchors/`. A DER `.crt` converts with
   `openssl x509 -inform der -in corp.crt -out corp.pem`.

2. Point omh at it.

   ```console
   $ omh set --local ca_cert ~/corp-root.pem    # this repo, gitignored
   $ omh set ca_cert /etc/ssl/certs/corp.pem    # this repo, committed
   ```

   Use `--local` for a path that is only yours; it lands in
   `.omh/settings.local.toml`, which is gitignored. Plain `omh set` writes to
   the committed file, which is right when a team on managed machines all have
   the root at the same path — a teammate then inherits it.

3. Run `omh doctor`.

   It is the only thing that checks the root actually reached the store rather
   than the recipe merely naming it — `update-ca-certificates` exits 0 when it
   skips a certificate it cannot parse.

</div>

In the settings file it reads:

```toml
ca_cert = "/etc/ssl/certs/corp-root.pem"
```

:::note[Not omh settings set]
That writes `~/.omh/default.toml`, which is the template new repos are *seeded*
from — `init` copies it with `write_if_absent` and never revisits. In a repo
that already has `.omh/settings.toml`, and that is every repo where this problem
shows up, it would be written and never read.
:::

**It reaches node, which the system store does not.** Running
`update-ca-certificates` rebuilds `/etc/ssl/certs/ca-certificates.crt`, and on
the Debian base omh builds on that is enough for curl, git, python, pip, go
and cargo. Debian's pip vendors a `certifi` that points at the system bundle,
so it reads the store like everything else. node does not: it reads one named
file and nothing else, and it is the fetch most likely to be the reason you are
here — the harness arrives through `npm install -g`. So the image also sets:

| | |
|---|---|
| `NODE_EXTRA_CA_CERTS` | **node ignores the system store.** Measured: still fails with the root installed, succeeds with this set |
| `PIP_CERT`, `REQUESTS_CA_BUNDLE` | belt and braces — a pip that vendors its own `certifi` rather than Debian's |
| `CARGO_HTTP_CAINFO` | belt and braces — cargo read the system store here |
| `SSL_CERT_FILE`, `SSL_CERT_DIR`, `GIT_SSL_CAINFO` | belt and braces — openssl's own defaults already point here |

**Only the first row is load-bearing on today's base image** — see how that was
measured, below.

**They are set on the image rather than at launch**, so a stack layer building
on top of it gets them too — `pip3 install`, `corepack` and the `curl` that
fetches rustup all run at build time, which is where the failure usually
happens. They are also written to `/etc/environment`, because a session
reached over SSH from an editor does not inherit the image's environment.

**A bundle with several certificates is fine**: each one is written to its own
file, because `update-ca-certificates` skips a file holding more than one.

**Changing the certificate rebuilds the image.** The tag is a digest of the
recipe and the certificate is part of it, so a rotated root produces a new tag
rather than silently reusing an image that trusts the old one.

**The file must be PEM** — a `BEGIN CERTIFICATE` block — and nothing else. A
`pkcs12` export carrying `Bag Attributes` or `friendlyName:` preamble is
refused rather than rewritten, because omh will not edit a certificate to make
it fit the recipe.

**A path omh cannot read is an error** rather than a shrug: you set this
because builds were failing, and resolving a typo to "no certificate" would
rebuild exactly the image that was already failing and report success.

**It is one certificate, and explicit**, because it widens what the sandbox
trusts — rather than omh copying your whole host trust store in.

<details>
<summary>How the variables were measured, and why the rest are set anyway</summary>

That is measured, not assumed. Against an `openssl s_server` presenting a leaf
signed by a private root, three images were compared: one with no root
installed, where all seven tools refuse; one with the root in the system store
and **none** of the variables in the table set; and this recipe. The middle
image is the isolation — six of the seven verify there.

The other rows cost nothing and cover a base image that patches its tools
differently, which is why they are set rather than trusted to be unnecessary.

</details>

## The `sbx` runtime (Docker Sandboxes)

`runtime = "sbx"` runs each session in a **Docker Sandbox** — a microVM with its
own kernel — instead of a Docker container that shares the host's. It is an
explicit opt-in: the default is Docker, and `auto` never picks `sbx`.

:::note[Provisional]
The `sbx` backend is provisional. Its CLI and runtime behaviour are measured
(see [architecture](/concepts/architecture/#runtime-backends)), but omh's own
launch path on `sbx` is not yet wired end to end, so treat it as hardening
under test rather than a finished backend.
:::

### Why you might want it

Two things it does that the Docker backend does not:

- **Stronger isolation.** A microVM has its own kernel, so a container escape is
  not a host escape. If you run agents on code you do not fully trust, this is
  the difference that matters.
- **The agent never holds its own token.** `sbx` injects service credentials
  (Anthropic, OpenAI, GitHub) at an egress proxy, so the secret authenticates
  the request without ever entering the sandbox's filesystem. omh's Docker
  backend mounts a credential directory the agent *can* read; `sbx` closes that.

It also carries per-sandbox egress policy (`sbx policy`, `--deny-network`) if you
want to restrict where an agent can reach.

The cost is real, which is why it is opt-in: it needs a Docker account, it is
Apple-Silicon macOS today, and there is a one-time setup below. The Docker
backend stays first-class; reach for `sbx` when the isolation is worth the
setup, not by default.

### How to set it up

<div class="steps">

1. Install `sbx`, log in, and start its daemon.

   `sbx` is a separate CLI. One-time, before omh can use it:

   ```console
   $ brew trust docker/tap
   $ brew install docker/tap/sbx
   $ sbx login                                  # a Docker account; --username + --password-stdin for CI
   $ sbx daemon start
   $ sbx policy init balanced                   # or allow-all / deny-all
   ```

2. Point a repo at it, and check.

   ```console
   $ omh set runtime sbx
   $ omh doctor
   ```

   `omh settings set runtime sbx` puts it in your defaults instead, which seed
   the repos you set up from then on.

   Run `omh doctor` before your first launch on `sbx`. Its container-runtime
   row says whether the runtime you chose is installed and answering — the same
   row it gives docker. It does not check `sbx`'s login, daemon or policy one by
   one.

</div>

## What goes in the catalogue

Most of what you already have needs no translation, which is what makes
adopting the catalogue cheap.

| Capability | Its format | How it reaches the harness |
|---|---|---|
| skills | [Agent Skills](https://agentskills.io/specification): a `SKILL.md` in a directory named after the skill | mounted, unchanged |
| rules | markdown, `~/.omh/rules/*.md` — prose, and prose travels | composed into the rules file the harness reads |
| MCP servers | the MCP spec's own shape | re-rendered per harness — the one place omh has always translated |
| commands | markdown with frontmatter | mounted, unchanged. Claude Code, opencode and oh-my-pi read the same shape; Codex has no commands |
| hooks | omh's own vocabulary | translated at staging — see [Writing a hook](#writing-a-hook) |
| subagents | no cross-tool standard | mounted as written; nothing is translated |

**Content is mounted read-only, not copied**, which keeps it live: edit a skill
on the host and the running agent sees it.

**A skill needs `name` and `description`**; `license`, `compatibility`,
`metadata` and `allowed-tools` are optional. `metadata` is the spec's own
extension point for anything a client needs that it does not define. Claude
Code, opencode, Cursor and Codex CLI all read the format, so
`~/.omh/skills/<name>/SKILL.md` reaches whichever harness you launch unchanged,
and nothing is lost in the trip.

**Subagents are the exception**, and it is worth knowing before you fill that
directory. There is no cross-tool standard: `name`, `description`, `tools`,
`model` and `hidden` are common to Claude Code and opencode, but
`permissionMode`, `disallowedTools` and `skills` are Claude's alone, while
`temperature`, `maxSteps`, `mode` and `permissions` are opencode's. A subagent
authored for one harness reaches the other with fields it will ignore. Nothing
is lost, and nothing is translated.

**What does not travel is tool names** — `allowed-tools` in a skill, `tools` in
a subagent, a hook's matcher. That is why omh keeps one closed tool vocabulary
and one `[tools]` map per adapter.

## MCP servers

Your MCP servers live in your catalogue. `omh settings mcp` lists and adds
them, and `omh settings rm mcp` takes one out, as it takes out any other entry
— omh's own, `omh-codegraph` and `omh-memory`, are never in it: they are
generated with their features, switched with [`[omh]`](#omh--omhs-own-features),
and a repo cannot set their environment under `[mcp.<name>.env]`:

```console
$ omh settings mcp ls
$ omh settings mcp add linear npx -- -y mcp-remote https://mcp.linear.app/sse
$ omh settings rm mcp linear
```

MCP lives under `omh settings` because MCP servers **are** configuration. They
live in your catalogue, and `omh settings mcp add` writes there — the catalogue
is not committed, so nothing here reaches a teammate by `git clone`.

### A token for one repo

A server's environment applies in every repo you work in, which is the wrong
scope for a key scoped to one of them. So a repo overrides the environment
without redeclaring the server:

```toml
# <repo>/.omh/settings.local.toml
[mcp.linear.env]
LINEAR_API_KEY = "..."
```

**Variable by variable**, so overriding one does not drop the rest of an entry
you never saw.

**There is deliberately no `command` here**: a repo configures a server from
your catalogue, it does not define one.

**An override naming a server the catalogue does not have is an error** — a
token going nowhere is exactly the shape of a setting somebody swears they
configured.

### Importing what you already have

Nobody retypes MCP servers they have already configured, so `import` is the
on-ramp:

```console
$ omh settings mcp import claude
```

**Import does not clobber.** Each server is added, recognised as already
identical, or reported as a conflict and left alone; re-running is a no-op.
`--replace` overwrites a conflicting entry instead. A server under one of omh's
names — `omh-codegraph`, `omh-memory`, or the `codegraph` and `memory` they
had — is skipped either way, naming the feature: add it under another name.

**Import paths expand against the host**, deliberately using a different
expansion than everything else — the guest home would send import looking into
a filesystem that does not exist yet.

**It is the exact inverse of the renderers**, which forces a real constraint:
**every format that renders must also parse, and the pair must round-trip.**
Otherwise import silently drops fields. That is a test, not a hope — see
[Adapters](/reference/adapters/#renderers).

## Bringing in what a harness already has

`omh import` copies the rest of a setup you already have into omh, so adopting
it is not a rebuild:

| What | Command | Lands in |
|---|---|---|
| MCP servers | `omh settings mcp import <harness>` | your catalogue — [above](#importing-what-you-already-have) |
| hooks | `omh import hooks <harness>` | this repo, `<repo>/.omh/hooks/` |
| skills, rules, commands, subagents | `omh import <capability> <harness>` | your catalogue |

**`omh init` names what it can see, across all of these, and acts on none of
it.** Importing writes content — for hooks, executable content in your repo —
which is a decision you make rather than one `init` makes because it found a
file.

### Importing hooks

```console
$ omh import hooks claude
```

The same inverse, for the capability most people already have configured. It
reads the harness's own hook file and writes what it can say into
**`<repo>/.omh/hooks/`** — this repo, never your catalogue, because a catalogue
hook runs in every project you ever open and one project's formatter does not
belong in front of the others.

**It copies.** The harness keeps working exactly as it did; adopting omh is not
a migration you cannot back out of.

**Imported hooks are selected**, or they would sit on disk and never run — the
launcher reads `[use]`, so a file written without being named there is a hook
the report counted and no session ships.

**Nothing is imported half-way.** A handler carrying anything omh cannot say —
`args`, a `type` that is not a command, or an `if` permission gate — is left
where it is and named. So is a matcher omh cannot read as tools.

<details>
<summary>Why a handler omh cannot fully say is left behind rather than imported without the part</summary>

Importing the command without its `if` would turn a hook that fired on one
narrow case into one that fires on every call, which is not a smaller version of
what you wrote.

The same goes for a matcher: Claude's matchers are unanchored regexes, and
`Edit|Write` is deliberately narrower than omh's `edit` — importing it as `edit`
would widen your hook to fire where you had stopped it.

</details>

### Importing the rest

```console
$ omh import skills claude
$ omh import rules claude
$ omh import commands opencode
$ omh import subagents claude
```

These are **copied into your catalogue**, not into the repo — the opposite of
hooks, and for the reason the catalogue exists: a skill is a way you work and
travels with you, while a hook binds to one project's commands.

**Rules come from your own file**, `~/.claude/CLAUDE.md`, never the project's.
omh already composes this repo's `CLAUDE.md` into every session; importing that
one would hand the agent the same prose twice, and go on doing it in every other
repo you opened.

**A skill arrives whole** — it is a directory, and everything under it comes
across.

**A symlink is refused**, at any depth. Your catalogue is mounted into every
sandbox omh launches, so a link reaching outside a skill would become a file the
agent can read in every project, from a copy nobody had reason to inspect. The
entry is skipped and named; nothing partial is left behind.

**A name that is not a name is refused** — `..`, a separator, a dotfile — by the
same rule `[use]` applies, so a path cannot be smuggled in where an entry
belongs.

**Nothing is ever clobbered.** An entry already in your catalogue is left
exactly as it is, so re-running is a no-op and an import cannot replace
something you have since edited.

**Planned:** a `plugin` capability that reads Claude marketplace plugins and
re-renders them for other harnesses. See [roadmap](/project/roadmap/).

## Writing a hook

A hook runs a command, advises the agent, or refuses a call, at a moment in a
session. You write what you want; the adapter says how this harness spells it.

**It is the one kind of content a repo can declare, and the only thing omh
carries that *executes*.** It goes in `<repo>/.omh/hooks/<name>.json` if it
binds to a command only this project has, and in `~/.omh/hooks/` if it works
anywhere — which is where omh's own conventional ones live.

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
| `stack` | which ecosystem it belongs to; absent means every one |
| `tools` | `edit` · `read` · `shell` · `search`. Empty means every tool |
| `when` | a shell predicate; non-zero keeps the hook silent |
| `capture` | a command whose output binds to `$OMH_CAPTURE` |
| `run` | executes; output ignored |
| `inject` | **advisory** text into the agent's context; the call proceeds |
| `refuse` | **blocks** the call and tells the model why |

**Exactly one of `run`, `inject` or `refuse`.** `capture` needs `inject` —
collecting output nobody reads says nothing, and a refusal is a fixed reason.

**The file's name is the hook's name**, and it is one line of plain text: a
name holding a line break or a control character is refused, naming the file,
because the name labels the hook in the programs omh generates for opencode and
omp. A `# comment` at the end of `when` or `capture` is fine.

**Not every harness has a word for every tool.** No adapter spells `search`
today, so a hook narrowed to it is dropped by name wherever it runs, and Codex
has no `read` either. An adapter's `[tools]` map is what it can spell.

### Advising is not blocking

**A hook says which it means, and a harness that cannot do the one it asked for
drops it by name** rather than substituting the other. `inject` advises and the
call goes ahead; `refuse` blocks it.

**`refuse` belongs to `before-tool`.** It blocks a call, and after the tool has
run there is nothing left to block. Written at any other moment it is refused
when the file is read, rather than rendering a payload the harness then ignores.

**A moment with no call in it can express less, on some harnesses.** On opencode
and oh-my-pi, `turn-end` and `session-start` hand the hook no tool call, so a
hook there cannot read a payload field, narrow to a tool, or inject text — each
is dropped by name saying so; a `run` is what those moments can do. Claude Code
has no such limit — omh's own `graph-orient` and `git-note` inject at
`session-start` there — and Codex drops only an `inject` at `turn-end`.

**A hook wanting a moment, tool or field this harness has no word for is
dropped by name at launch**, saying what it asked for; the rest still ship.

<details>
<summary>Why advising and blocking cannot stand in for each other</summary>

The difference is invisible in the text and decisive in the translation. On
Claude Code both travel in the same field and differ by one key —
`additionalContext` advises, `permissionDecision` denies. On opencode they are
not the same mechanism at all: the only way to say anything before a tool runs
is to throw, which blocks, and advisory text has no channel there until the tool
has produced a result to append to.

A nudge that quietly became a wall would look exactly like working — and a wall
that quietly became a nudge would let through the call it existed to stop.

</details>

### What the payload gives you

`$OMH_TOOL_FILE` and `$OMH_TOOL_COMMAND` are the tool call, in omh's words.
Mention one in any body and omh reads it for you — `${OMH_TOOL_FILE:-none}`
counts too. Mention neither and your hook pays for nothing, which matters:
`before-tool` on `read` fires on the most frequent tool there is. What a hook
binds, `$OMH_CAPTURE` included, is in the environment of every program it
runs, on every harness — a script your `run` calls reads `$OMH_TOOL_FILE` too.

Three more are set in every sandbox, whatever the tool call:

| | |
|---|---|
| `$OMH_SESSION` | the session's id — `s01` |
| `$OMH_GRAPH_PROJECT` | the name this session's code graph is indexed under |
| `$OMH_IMAGE_RECIPE` | the digest of the base image's recipe, which is what a note pinned to `image:current` records. Empty when omh could not work it out |

**Parameterize through the environment, never through the file.**
`$OMH_GRAPH_PROJECT` and `/work` are what let omh's graph hooks be the same
text in every repo; a repo's name written into the hook would not be.

### Three rules about `inject` and `refuse`

Both are prose that reaches a shell, and that combination fails quietly, so all
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

`graph-orient`, `graph-first`, `graph-read`, `graph-refresh`, `git-note` and
`git-turn` are generated from the [base set](/reference/base-set/), not files.
A hook file answering to one of those names is an **error naming both** — it
would not override omh's, and it would not run. To be rid of omh's, switch the
feature off with [`[omh]`](#omh--omhs-own-features).

A rules file is held to the same rule. `graph-rules`, `git-rules` and
`memory-rules` are sections omh generates, so `~/.omh/rules/git-rules.md` is an
error naming the file and the feature to switch off — it is not composed, and
it does not override omh's.

<details>
<summary>Why a name omh ships is an error rather than an override</summary>

A hook that is committed, reviewed and silently inert is worse than one that
refuses to start. And a repo that could replace `graph-refresh` could make the
graph lie while looking installed.

</details>

### Guards, and their bypass

A **guard** is a catalogue hook whose action is `refuse` — a `before-tool`
hook that blocks the call rather than advising around it. omh ships two:

| Guard | Refuses | Covers |
|---|---|---|
| `tdd-guard` | editing a source file whose paired test has not moved since the last commit | go, python and node; silent on `.rs` |
| `config-guard` | editing this repo's own `.omh/` from inside a session | every repo |

**Guards are never in the default expansion.** `omh init` and `omh use --all`
turn on every other applicable catalogue hook, but never a guard — the same
way `graph-first` etc. are never a wall. A new repo gets one only where your
defaults' `[use]` names it. Turn one on by name:

```console
$ omh use hooks tdd-guard
```

**A guard is discipline, not containment — and this is its bypass.**
`tools: ["edit"]` matches Claude's `Edit|Write|MultiEdit` — a shell redirect or
`sed -i` reaches the same file through `Bash` and is never seen. The sandbox —
the worktree the agent cannot escape — is what actually contains a session; a
guard is a reminder at the moment its rule is broken, not a second containment.

**A guard fails open.** Each one refuses only when it can positively confirm
the rule is unmet; anything it cannot ask about — no git repository, `git`
erroring — allows the edit rather than blocking on a question it could not
answer.

**One `tdd-guard`, every language it knows.** It dispatches on extension inside
the one hook, the way `config-guard` dispatches on path with no `stack` field.
One name rather than one hook per language: a monorepo with go and node turns on
`tdd-guard` once, not `go-tdd` and `node-tdd` separately.

**There is no `rust-tdd`.** Rust's tests live inside the file under test
(`#[cfg(test)] mod tests`), so "the test file changed first" cannot be
expressed — the guard would refuse the edit that adds the test. `tdd-guard`
is silent on `.rs` files rather than pretending to cover them.

**`config-guard` guards what the next session gets.** This repo's `.omh/`
decides what the next session gets and what `omh sNN commit` runs before it
lands anything.

**Coverage is per harness**, like everything hooks-shaped. A dropped guard is
announced at launch, never silently downgraded to a nudge.

| Harness | `refuse` renders as | What it cannot see |
|---|---|---|
| claude | `permissionDecision: "deny"` | — |
| opencode | `throw` | — |
| oh-my-pi (omp) | `{ block: true, reason }` | a single `edit` call can batch edits across several files, and `$OMH_TOOL_FILE` only ever names the first one it touches |
| codex | `permissionDecision: "deny"` | `$OMH_TOOL_FILE` names only the first file in a patch, and a patch the model runs *through the shell* reaches hooks as `Bash`, which an `edit` guard does not see |

**Every hook's decision is logged**, not only a guard's. Whether a hook fired,
stayed silent or refused, on every harness with hooks (claude, codex,
opencode, omp), is read back per session — see `omh sNN` in
[Commands](/reference/commands/). `omh eject` carries none of it.

<details>
<summary>How omp and Codex find the file being edited, and why no guard closes the shell</summary>

omp needed one more thing first: its `edit` tool keeps its path inside one
`input` string (`[PATH#TAG]` sections) rather than a plain property, so
`$OMH_TOOL_FILE` needed a per-tool override —
`[capabilities.hooks.fields-by-tool.edit]` in `adapters/omp.toml` — instead
of the shared field map every other tool reads.

Codex is the same shape in reverse: `apply_patch` carries the path only in the
patch text, so `$OMH_TOOL_FILE` reads the first `*** Update File:`,
`*** Add File:` or `*** Delete File:` header — the same first-file gap. Codex's
hooks are rendered into `/etc/codex/config.toml`, the layer Codex runs without
asking for a trusted hash.

No guard adds a shell arm to close the `Bash` route: the
[base set](/reference/base-set/) already argues the identical point about
`git push`, and its own pattern for matching `git` at all shipped broken once by
missing a multi-line script.

</details>

## Hooks and stacks `omh init` writes for you

`omh init` writes what it can derive from the files a repo already commits, and
asks about the two things it cannot.

### Hooks omh works out for you

Some commands the catalogue cannot hold, because they are a property of the
project rather than of its ecosystem. `npm test` is only a real command if the
project declared a `test` script, and which manager runs it depends on the
lockfile. So `omh init` reads what the project already commits and writes the
hook into `<repo>/.omh/hooks/` — where you can edit it, and where it is
committed and travels:

| It reads | To decide |
|---|---|
| `[provision]`, then a lockfile, then `packageManager` | which package manager runs a script |
| `scripts` in `package.json` | whether there is a `test` or `format` to run |
| a `Makefile`, `justfile` or `Taskfile.yml` | whether the project has its own entry point |

**It executes nothing.** Not `make -qp`, not a shell. Every answer comes from
reading a file, because a derivation that ran something on your machine during
`init` would be the thing omh exists to avoid.

**It fills gaps and never competes.** A rust project already has `rust-test`
from the catalogue, so its `Makefile` earns nothing — otherwise every turn
would run the suite twice. A project that is both rust and node correctly gets
both.

**Anything ambiguous produces nothing.** Two lockfiles, a `Taskfile` using
`includes:` or YAML anchors, a `package.json` that will not parse: omh writes no
hook and you write one.

**The command is always spelled from omh's own vocabulary** — `pnpm run test`,
`make fmt` — never from text in your files. **And it is always `run`**:
`bun test` is bun's own test runner and ignores your `test` script completely.

<details>
<summary>Why ambiguity writes no hook, and why <code>[provision]</code> is read before the lockfile</summary>

A repo with no hook is a repo somebody adds one to; a repo with the *wrong* hook
is one where every turn ends in a red mark nobody can explain, and the hook omh
invented is the last place anyone looks.

`[provision]` records what the image was built with, so it is the one source
that describes the sandbox the hook will run in. A hook spelled for a package
manager the image does not have is worse than no hook.

</details>

### The two things omh will ask

Most repos are asked nothing at all. Two things are not derivable from any file,
and `omh init` asks about them — once, on a terminal, recording the answer so it
never asks again.

**`what installs elixir? (Enter to skip)`** — when the repo plainly *is*
something omh has never been taught. A `mix.exs` names elixir; omh ships no
elixir stack and cannot invent one. Answer it and omh writes
`<repo>/.omh/stacks/elixir.toml`, which is read beside the ones omh ships:

```toml
name   = "elixir"
marker = "mix.exs"

[[provide]]
name    = "toolchain"
needs   = ["mix", "elixir"]
install = "apt-get update && apt-get install -y elixir"
because = "elixir is what this project is written in"
```

It asks for the install command *and* what should then be on PATH, because a
recipe with no stated outcome is one nothing can check — omh would install
something, report success, and have no way to notice it had not worked.

**A repo's own stack adds; it never shadows.** A file answering to a name omh
ships is an error naming both paths, not a silent override — a stack decides
what goes into the image your agent runs in. [Stacks](/reference/stacks/) has
the file's fields.

**`what command runs the tests? (Enter to skip)`** — when no stack, lockfile,
runner or declared script could say. Answer it and omh writes
`<repo>/.omh/hooks/test.json`.

**Pressing Enter declines and writes nothing**, which is what it should mean:
it is what you press when you do not know. One decline stops the remaining
stack questions; the test question is still put, because it is the one most
repos reach.

**A closed pipe stops the stack questions** rather than having its silence
recorded as a set of answers. A CI runner, which is not a terminal, is asked
nothing and gets no files.

## Next

<div class="cards">

- [Commands](/reference/commands/)\
  `omh set`, `omh use`, `omh settings` and every other command, with what each prints.
- [Stacks](/reference/stacks/)\
  What a stack file holds, and the condition that decides whether a provide applies.
- [The base set](/reference/base-set/)\
  omh's own features, entry by entry, with what each one costs.
- [Troubleshooting](/guides/troubleshooting/)\
  `omh doctor`, and what to do when something does not start.

</div>
