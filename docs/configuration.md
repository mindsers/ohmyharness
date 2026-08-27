# Configuration

Your setup is declared once and rendered into whatever shape each harness reads.
This page covers where it lives, how it resolves, and how to change things.

## One catalogue, and it is personal

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

A project cannot declare a skill, an MCP server, a command or a subagent. It
names ones from your catalogue.

### Why hooks are the exception

A skill is a way *you* work — it travels with you across repos, which is why it
belongs to you. Some hooks are the same: `cargo test` is what a rust project
runs, not what *this* rust project runs, so omh ships one per ecosystem and they
live in your catalogue like everything else.

But a hook is also the one capability that can bind to a command only this repo
has — an integration suite behind a script, a linter with the project's own
config. Those have to be declarable where the project is, or the catalogue fills
with entries that are only ever right in one place.

So the rule is not "no content in the repo". It is **content lives where its
scope is**, and hooks are the one capability that can have either scope.

**A shipped hook names the ecosystem it belongs to**, and nothing else about it:

```json
{ "on": "turn-end", "stack": "rust", "run": "cargo test" }
```

That is a *reference*. The marker that decides whether a repo is a rust project
stays in the stack definition, so the two can never disagree — and a hook naming
an ecosystem you are not is simply not offered to you. `omh init` in a rust repo
does not put `go-test` in your `[use]` list, and the launcher does not report it
as something you are not using.

A hook that names no stack belongs everywhere, which is most of them.

**A project hook beats a catalogue hook of the same name**, which is how a repo
overrides your personal `format` hook with the one it actually needs, without
renaming anything. **Names from the base set are reserved** — a file answering
to one is an error naming both, because a repo that could replace
`graph-refresh` could make the graph lie while looking installed.

### Hooks omh works out for you

Some commands the catalogue cannot hold, because they are a property of the
project rather than of its ecosystem. `npm test` is only a real command if the
project declared a `test` script, and which manager runs it depends on the
lockfile. So `omh init` reads what the project already commits and writes the
hook into `<repo>/.omh/hooks/` — where you can edit it, and where it is
committed and travels:

| It reads | To decide |
|---|---|
| a lockfile, then `packageManager` | which package manager runs a script |
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
hook and you write one. A repo with no hook is a repo somebody adds one to; a
repo with the *wrong* hook is one where every turn ends in a red mark nobody can
explain, and the hook omh invented is the last place anyone looks.

The command is always spelled from omh's own vocabulary — `pnpm run test`,
`make fmt` — never from text in your files. And it is always `run`: `bun test`
is bun's own test runner and ignores your `test` script completely.

### The two things omh will ask

Everything above is derived, and most repos are asked nothing at all. Two things
are not derivable from any file, and `omh init` asks about them — once, on a
terminal, recording the answer so it never asks again.

**"How is this installed?"** — when the repo plainly *is* something omh has
never been taught. A `mix.exs` names elixir; omh ships no elixir stack and
cannot invent one. Answer it and omh writes `<repo>/.omh/stacks/elixir.toml`,
which is read beside the ones omh ships:

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
what goes into the image your agent runs in.

**"What command tests this?"** — when no stack, lockfile, runner or declared
script could say. Answer it and omh writes `<repo>/.omh/hooks/test.json`.

**Pressing Enter declines and writes nothing**, which is what it should mean:
it is what you press when you do not know. **A closed pipe stops the
questions** — a CI runner is asked nothing and gets no files, rather than having
its silence recorded as a set of answers.

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
carries that *executes*. It goes in `<repo>/.omh/hooks/<name>.json` if it binds
to a command only this project has, and in `~/.omh/hooks/` if it works anywhere
— which is where omh's own conventional ones live.

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
| `stack` | which ecosystem it belongs to; absent means every one |
| `tools` | `edit` · `read` · `shell` · `search`. Empty means every tool |
| `when` | a shell predicate; non-zero keeps the hook silent |
| `capture` | a command whose output binds to `$OMH_CAPTURE` |
| `run` | executes; output ignored |
| `inject` | **advisory** text into the agent's context; the call proceeds |
| `refuse` | **blocks** the call and tells the model why |

Exactly one of `run`, `inject` or `refuse`. `capture` needs `inject` —
collecting output nobody reads says nothing, and a refusal is a fixed reason.

### Advising is not blocking

The difference is invisible in the text and decisive in the translation. On
Claude Code both travel in the same field and differ by one key —
`additionalContext` advises, `permissionDecision` denies. On opencode they are
not the same mechanism at all: the only way to say anything before a tool runs
is to throw, which blocks, and advisory text has no channel there until the tool
has produced a result to append to.

So a hook says which it means, and a harness that cannot do the one it asked for
**drops it by name** rather than substituting the other. A nudge that quietly
became a wall would look exactly like working — and a wall that quietly became a
nudge would let through the call it existed to stop.

**`refuse` belongs to `before-tool`.** It blocks a call, and after the tool has
run there is nothing left to block. Written at any other moment it is refused
when the file is read, rather than rendering a payload the harness then ignores.

**A moment with no call in it can express less.** `turn-end` and `session-start`
hand the hook no tool call, so a hook there cannot read a payload field, narrow
to a tool, or inject text — each is dropped by name saying so. A `run` is the
thing those moments can do. A hook wanting a moment, tool or field this harness
has no word for is **dropped by name at launch**, saying what it asked for; the
rest still ship.

### What the payload gives you

`$OMH_TOOL_FILE` and `$OMH_TOOL_COMMAND` are the tool call, in omh's words.
Mention one in any body and omh reads it for you — `${OMH_TOOL_FILE:-none}`
counts too. Mention neither and your hook pays for nothing, which matters:
`before-tool` on `read` fires on the most frequent tool there is.

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

`graph-orient`, `graph-first`, `graph-read` and `graph-refresh` are generated
from the [base set](design/base-set.md), not files. A hook file answering to one of those names is an **error naming both**
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
wholesale**. Your `~/.omh/default.toml` can carry a default selection, which
`omh init` seeds into a new repo; a repo naming `skills` replaces it outright. Merging would let a repo
add to your list and never take anything off it, which is the thing an allowlist
exists to make possible.

### A feature is not selectable

`codegraph` and `memory` sit in `~/.omh/mcp.json` looking exactly like servers
you added, because `omh init` seeded them there. They are not yours to select:

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

`[use]` decides what the harness is **offered**. The catalogue directory behind
it is mounted into the sandbox whole and read-only, so an unselected skill is
not loaded but is still readable at an internal path by an agent that goes
looking for it. Selection is for keeping a project's context to what the project
needs — it is not a boundary, and omh does not claim it as one. What is
guaranteed is the read-only part: the agent can read a selected skill and cannot
write one.

## `[provision]` — what your sandbox is built with

`omh init` works out which ecosystems this repo is — a `Cargo.toml`, a
`package.json` — and then asks the **sandbox**, with your repo mounted
read-only, which parts of them apply here. A repo with a `pnpm-lock.yaml` gets
pnpm; the yarn and bun provides do not apply and are not installed.

What it decided is written into your committed settings:

```toml
# <repo>/.omh/settings.toml
[provision]
"rust/toolchain" = true
"rust/linker" = true
```

That table is the input to everything afterwards. Your sessions run an image
built from exactly these, so a teammate who clones the repo gets the same
sandbox without being asked anything, and neither `omh new` nor `omh sNN
resume` re-evaluates a condition.

**omh only ever writes `true`.** A `false` can only have been typed, so it is
treated as a decision and left alone:

```toml
[provision]
"rust/linker" = false   # this base image already has cc; do not spend 124 MB on it
```

An opt-out changes what goes **into** the image. It does not change what omh
says **about** it: if one of your hooks needs the program, omh still asks the
sandbox whether it is there, and still holds the hook back by name if it is not.

Keyed `"<stack>/<provide>"`. `omh why` names what each one buys and what it
costs. Re-running `omh init` is the honest fix for drift — swap a `yarn.lock`
for a `pnpm-lock.yaml` and the yarn entry goes, because the table describes what
is true now.

These layer like every other setting, so a provide you want left out on **your**
machine belongs in `settings.local.toml`, where it says nothing to anyone else —
and omh will never copy it into the committed file.

## Hooks your sandbox cannot run

omh works out which ecosystems this repo is from its manifests, and the hooks
for them are catalogue entries this repo turns on through `[use]`. Detection
runs on your machine; the hook runs in the sandbox, and those are different
computers. So omh provisions the toolchain into the image — that is what
`[provision]` above records — and then **measures** what it got.

**A hook is selected either way.** What omh ships and what your repo declares
are both statements about the *project* — committed, and the same for everybody
who clones it. Whether `cargo` is installed is a fact about one image, and it
must not decide what the repo says about itself: otherwise whoever ran `init`
first imposes their machine on the whole team.

What a missing program decides is whether the hook **runs against this image**,
and nobody is asked about it:

```
  held back  `rust-test` needs `cargo` — not installed in this repo's sandbox
             the hook file is written and travels; it runs as soon as the
             sandbox has it
```

That line appears at `omh init`, and again at every launch — in the status line
of `omh new` and `omh sNN resume`, and named individually by `omh s attach` — in
the same list as a hook your harness cannot spell — a held-back hook is never silently absent. It
is re-decided from the measurement each time, so a sandbox that gains the tool
gets its hook back with nothing to un-configure.

Measurements are cached per **image**, in `~/.omh/facts.json`, keyed by the tag
your sessions run. A repo whose hooks and stacks have not changed asks the
container nothing; add a hook naming a new program and that one program is
asked about, once.

A program nobody has measured is **unknown**, never assumed missing — so a first
run, a deleted cache or an unreadable one holds nothing back, and every hook
ships. The failure has to fall that way round: the other direction would switch
off every hook on the machine in a session that otherwise looks completely
normal.

> **`[toolchain]` was removed.** It recorded an answer to *"this sandbox lacks
> `cargo`, shall I switch the hook off?"* — a question that asked you to
> configure around a broken environment, and whose answer outlived the breakage.
> omh provisions the tool instead. A repo that still has the table gets an error
> naming it; delete it, and use `[provision] "<stack>/<name>" = false` if what
> you want is a provide left out.

## Settings, and their two layers

Content has one home; **settings** keep their layers, because a setting has one
value and the useful question is which file decided it:

```
<repo>/.omh/settings.toml  →  <repo>/.omh/settings.local.toml
```

Later wins. Both are in the repo, and that is the point: a repo's behaviour is
explained by files the repo contains — which is what a teammate cloning it can
actually see, and what `omh info --repo` can account for without pointing at a file
they do not have. The rules for a project belong in the committed file; the API
key that makes one of them work belongs in the gitignored one.

**`~/.omh/default.toml` is not a third layer.** It is the *template* `omh init`
seeds a new repo from — read once, at `init`, and never at launch. It was
`settings.toml` and a layer like the two above until 0.7.0; a file that decides
nothing should not be spelled like the two that decide everything. See
[`omh settings`](commands.md#omh-settings).

Undebuggable without provenance — the standard complaint about oh-my-zsh, and
the thing [trust](design/trust.md) exists to prevent. So every effective value
reports where it came from and what it beat:

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
  rules      commit-style, tdd
  skills     review-diff             (1 not selected: refactor)
  mcp        everything
  commands   everything
  subagents  everything
  hooks      rust-format, rust-test

1 catalogue entry is not selected here: skills/refactor

  omh use skills refactor    ·    omh use --all
```

## Two scopes, two commands

`omh settings` means **you** — the defaults a new repo starts from. `omh info --repo` means
**this checkout**.

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
$ omh settings unset idle_timeout       # let the layer beneath resurface
$ omh settings edit                     # $EDITOR on your defaults
$ omh settings edit skills tdd          # $EDITOR on one catalogue entry
$ omh settings                          # your defaults
$ omh info                              # and what you have here
```

**The two scopes wanted opposite defaults**, which is why one `--layer` flag
could not serve both — and why these commands ask *where the value already is*
before they ask anything else:

| Command | Writes to | Why that default |
|---|---|---|
| `omh set` / `unset` / `use` / `unuse` | **where it already is**, else the key decides | one rule, four commands; the classification lives with the key rather than in your memory |
| … with `--save` | `settings.toml`, **committed** | you named the file, and writing a committed one is said out loud |
| … with `--local` | `settings.local.toml`, **gitignored** | the same, in the direction that keeps a value off your teammates' machines |

**The protection moved from the command to the key.** Before 0.7.0 the
repo-scoped write sent every value to the gitignored file: the safety came from
the destination, and no value could reach git unasked — at the price that a
teammate cloning the repo got none of your settings. `omh set` defaults to the
*committed* file now, because most settings — what runtime a project wants, how
long its sessions idle — are facts about the project that a teammate cloning it
should get. What keeps a credential out of git is `src/key.rs`: a table saying,
per key, whether a value there can name one. `carry_in` is in it, so
`omh set carry_in …` still writes the gitignored file with no flag at all.

**And every write reaches every layer that already holds the key**, so one
cannot land under a value that outranks it. `--local` and `--save` step outside
that on purpose — you named the file — and say so when the result is a value
you cannot observe changing.

```console
$ omh set carry_in '[".env"]'
wrote → /Users/you/proj/.omh/settings.local.toml (gitignored)

$ omh set idle_timeout 30m
wrote → /Users/you/proj/.omh/settings.toml (committed)
```

The word in brackets is the point, and it is there because the path is not
enough: `settings.toml` and `settings.local.toml` do not read as opposites at a
glance in a line eighty columns wide.

`omh set` also makes its own premise true. The gitignored file is only safe
because git ignores it, and the ignore line used to be written by `omh init`
alone — so in a repo that had never been `omh init`ed, `omh set carry_in` left
a credential map that `git add .` would stage:

```console
$ omh set carry_in '[".env"]'
omh: nothing was ignoring settings.local.toml — added it to /Users/you/proj/.omh/.gitignore
wrote → /Users/you/proj/.omh/settings.local.toml (gitignored)
```

A key omh has never heard of is written to the committed file and reported
twice — nothing reads it, and it went somewhere git carries:

```console
$ omh set carry_ins '[".env"]'
omh: nothing in omh reads `carry_ins` — it is written, and it will sit there
wrote → /Users/you/proj/.omh/settings.toml (committed)
omh: and the committed file is COMMITTED — `carry_ins` went into a file git carries
```

A test fails the build if a key omh's own code reads is missing from the table,
which is what stops omh's own keys ever taking that path.

Asking for the committed file still says what that means, and names the key when
the key is one that reaches a credential:

```console
$ omh set --save carry_in '[".env"]'
wrote → /Users/you/proj/.omh/settings.toml (committed)
omh: the committed file is COMMITTED — never put a secret here
omh:   `carry_in` is one of those — it belongs in /Users/you/proj/.omh/settings.local.toml
```

### `omh unset` reaches every repo layer that holds the key

Not the one `omh set` would have written — those are different questions, and
answering the second one shipped a defect worth recording. `omh set --save
carry_in` followed by `omh set --local carry_in` leaves the key in **both** repo
files; omh wrote both. An `unset` that consulted `set`'s rule removed the
gitignored copy, said so, exited 0, and left a map to a credential standing in
the file git carries. The command you run to get a secret out of git reported
success and did not do it.

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

  Files a session gets that git does not carry — see `src/carry.rs`.
  takes  a TOML array of paths, e.g. [".env"]
  kept   /Users/you/proj/.omh/settings.local.toml (gitignored)

  A value here can name a credential, which is why omh keeps it
  out of the file git carries.
```

Where a repo already carries a `[use]` or `[omh]` table in its **gitignored**
file, the write reaches that too — it is the layer that decides, and a command
that reported success while the layer beneath overruled it would be lying.
Never a layer that did not already declare the key: a selection appearing in a
gitignored file is how a teammate stops getting what the repo says it uses.

**One command, two tables, and the refusal teaches the difference.**
`omh use`/`unuse` name catalogue entries; `omh set <feature> on|off` writes
`[omh]`. `omh set` reads the name first and refuses the wrong kind — a
catalogue entry is answered with `omh use <capability> <name>`, an entry that
belongs to a feature is answered with the feature — so the difference between
*an entry you chose* and *a feature omh ships* is stated where somebody just
guessed at it, rather than only here.

`unset` removes the value from one layer rather than forcing a value, which is
what lets the layer beneath take over again — the difference matters when you
are overriding a team default temporarily.

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
**Removing the server is the other door** — `omh settings mcp rm codegraph` takes
the feature with it, hooks and rules section included, because a hook nudging
the agent toward a server that is gone is worse than no hook.

It layers like every other setting — this file, then
`<repo>/.omh/settings.local.toml`, which `omh set` adds to `.omh/.gitignore`
the first time it writes there. `~/.omh/default.toml` can carry an `[omh]`
table, which seeds a new repo; it decides nothing in a repo that already
exists.

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
$ omh settings mcp ls
$ omh settings mcp add linear npx -- -y mcp-remote https://mcp.linear.app/sse
$ omh settings mcp rm linear
```

MCP lives under `omh settings` because MCP servers **are** configuration. They live in
your catalogue, and `omh settings mcp add` writes there — the catalogue is not
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
$ omh settings mcp import claude
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
where it is and named. Importing the command without its `if` would turn a hook
that fired on one narrow case into one that fires on every call, which is not a
smaller version of what you wrote. The same goes for a matcher omh cannot read
as tools: Claude's matchers are unanchored regexes, and `Edit|Write` is
deliberately narrower than omh's `edit` — importing it as `edit` would widen
your hook to fire where you had stopped it.

`omh init` mentions what it can see and does nothing about it. Importing writes
executable content into your repo, which is a decision you make rather than one
`init` makes because it found a file.

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

`omh init` names what it can see across all of these and acts on none of it.

**Planned:** a `plugin` capability that reads Claude marketplace plugins and
re-renders them for other harnesses. See [roadmap](design/roadmap.md).

## `carry_in`

A git worktree contains only **tracked** files. No `.env`, no certs — so without
help both the agent and your IDE land somewhere that cannot run your app.

```toml
carry_in = [".env.local", "certs/"]
```

```console
$ omh new claude
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
$ omh new claude
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

Both of those are about the worktree's own git, which is what runs on the
**host**. Inside the sandbox the agent reads a different repository entirely —
see [Sessions](sessions.md#the-agent-has-git-and-it-is-not-yours) — and its
exclude list is written separately, from the same `carry_in` patterns, into the
gitdir omh mounts. Two mechanisms, one list, so a `carry_in` entry keeps the
file out of both `git status`es.
