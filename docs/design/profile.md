# The profile — catalogue, selection, composition

> **Status: mostly built.** P1, P2 and P3 of the [build order](#build-order)
> have landed — the project's own rules are composed rather than replaced, omh's
> hooks and rules sections are generated from the base manifest, `[omh]` switches
> a feature off per repo, there is one catalogue, and hooks are authored in omh's
> own vocabulary and translated per harness at staging.
> [Configuration](../configuration.md) describes the storage model as it now is.
> Selection — `[use]`, `omh use` / `unuse`, `omh repo` — is P4.

## The three things wrong with what exists

**Content lives in three places.** `~/.omh/profile`, `<repo>/.omh/profile` and
`<repo>/.omh/local` have identical shapes, so a skill can be declared in any of
them and the question "where is this one" has three answers.

**A layer can add, never subtract.** `Profile::sources` is a union. A later
layer can shadow a same-named entry, but nothing turns one off — so "these are
my twelve MCP servers, this project uses three" is unexpressible. The only lever
is not installing it globally, which is the opposite of a catalogue.

**The project's own rules are hidden, not composed.** This one is a bug, not a
gap. `Render::Concat` merges the profile layers only, and the result is mounted
read-only over `/work/CLAUDE.md`. A repo that tracks its own `CLAUDE.md` has it
covered for the length of the session — the agent never sees the conventions the
project actually wrote down. `carry.rs` documents the mount as keeping omh's
rules out of the user's commit, which it does; the cost nobody wrote down is
that it also discards the repo's.

## Where things are stored

**One catalogue, and it is personal.**

```
~/.omh/
  rules/               \
  skills/               |  the only place these live
  commands/             |
  subagents/           /
  mcp.json
  hooks/               yours — and the one kind a repo may also declare
  settings.toml        your defaults
```

**Rules are a directory of named files, not one `AGENTS.md`.** That is what makes
`[use] rules` mean something: `tdd.md`, `commit-style.md` and `rust-idiom.md` are
separate things you hold, and a repo takes the ones that apply to it. It also
makes the catalogue uniform — every capability is now a directory of named
entries selected the same way, with `mcp.json` the lone exception because a server
is a record rather than a file.

**A repo holds configuration, and one kind of content.**

```
<repo>/.omh/
  settings.toml        committed: which catalogue entries this project uses, plus policy
  settings.local.toml  gitignored: your overrides, and the secrets settings.toml must not hold
  memory.toml          committed: how the note store keys and expires
  hooks/               committed: hooks that only make sense in this repo
<repo>/AGENTS.md         the project's own rules — tracked, and now actually read
```

There is no `<repo>/.omh/profile/` and no `<repo>/.omh/local/`. A project cannot
declare a skill, an MCP server, a command or a subagent; it names ones from your
catalogue.

**Hooks are the exception, and the reason is in the capability itself.** A skill
is a way *you* work — it travels with you across repos, which is why it belongs
to you. A hook binds to a repo's own commands, and the stack hooks are the proof:
`cargo test` here, `pnpm test` next door, one name, two bodies. A capability that
is project-specific by nature has to be declarable where the project is, or the
catalogue fills up with entries that are only ever right in one place.

So the rule is not "no content in the repo". It is **content lives where its
scope is**, and hooks are the one capability whose scope is the repo.

`settings.toml` / `settings.local.toml` is the pair Claude Code already
established, and it says what the file is rather than what one of its tables
holds. `memory.toml` replaces `keys.toml` for the same reason: key templates are
one table of the memory subsystem's configuration, not the whole of it —
[expiry](memory.md) has settings of its own coming.

**The `keys.toml` rename needs a check, not a fallback.** `templates()` treats a
missing file as "use the shipped defaults", so renaming without one would take an
edited `keys.toml`, fail to find `memory.toml`, silently revert to the shipped
templates and re-key every note written from then on — the exact failure the
"seeded once and never refreshed" rule exists to prevent. **Built:** a `keys.toml`
still present is a loud error naming both paths, never a silent fallback. A check
rather than a move, because moving somebody's file behind their back is the
larger of the two surprises.

### What that costs

**A repo can no longer ship a skill, an MCP server or a command to your
teammates.** Today the committed layer does exactly that, and this model gives it
up. What a repo still shares is its rules file — which for the first time
actually reaches the agent — its hooks, its selection, and its policy.

**A committed hook is executable content, and cloning a repo runs it.** That is
true today and stays true; what changes is that hooks become the *only* thing a
repo can hand you that executes, which makes it worth saying plainly rather than
leaving implied. A hook fires on tool calls, inside the sandbox, with whatever
the image has. So the launcher **names the repo's hooks at launch, and calls out
any that are new or changed since you last ran here** — the same treatment
`carry_in` gets, for the same reason: the mechanism by which somebody else's
content reaches your agent is the one that has to narrate itself.

The sandbox is what makes this a disclosure rather than a hole. A repo hook
cannot reach your checkout, your home directory or your credentials — the
worktree model and the read-only mounts were already load-bearing for exactly
this.

A name the repo selects and your catalogue does not have is **reported at
launch, by name, with what would install it**. Never silently missing: silent
total degradation is the failure this codebase refuses everywhere else, and it
is the exact shape a teammate hits after cloning.

It is a **warning, not a hard error**. A teammate who clones must still be able
to launch a session; refusing to start because they lack a skill they never
asked for would make the committed file a liability.

Recorded, not built: catalogue entries could carry a `source` and `omh sync`
could fetch the missing ones. That restores team sharing without putting content
back in the repo, and it is worth nothing until somebody other than the author
hits the problem.

### Hooks come from three places

Hooks are the one capability with several sources, because they are the one
capability whose scope genuinely varies.

| Tier | Where | Editable | Switched off by |
|---|---|---|---|
| omh's own | generated from the base manifest | no | `[omh]`, by feature |
| the catalogue | `~/.omh/hooks/` | yes — add, edit, delete | `[use]` |
| the project's | `<repo>/.omh/hooks/` | yes | `[use]` |

Only the first is omh's. The stack hooks `init` writes from `detect.rs` are
**project hooks that omh seeded** — omh guessed them, but they exist because your
repo has a `Cargo.toml`, and once written they are yours like any other file in
your repo.

All three are merged and injected together; the harness receives one hooks
configuration and cannot tell which tier a hook came from.

**Precedence, when two tiers use one name:** project beats catalogue. That is how
a repo overrides your personal `format` hook with the one this project actually
needs, without renaming anything.

**Names from the base manifest are reserved** — a collision with one is an error
naming both rather than an override, because a repo that could replace
`graph-refresh` could make the graph lie while looking installed. Everything else
is a file, and files are yours to overwrite.

**A hook shipped in the base manifest is omh's, whether or not disabling it
breaks anything.** `graph-orient`, `graph-first` and `graph-read` do not keep the
graph correct — that is `graph-refresh`'s job, and `omh graph` reads the index
without consulting any of the three. What they do is make the graph *used* rather
than merely installed, which is what the manifest's own section header says. But
ownership does not follow from consequence: they are authored, versioned and
cost-measured entries, so the reason omh's hooks are generated rather than seeded
applies to them identically — **a hook you can edit is a hook omh can never ship a
fix to.**

That leaves the catalogue tier **empty on a fresh install**, and that is the
honest state of it: the five hooks omh ships belong to a feature, and the other
two are this repo's. `~/.omh/hooks/` is where *your* hooks go, and where any
genuinely optional hook omh ships later would land — a turn-end notification, a
secret scanner — the kind that serves no feature and is pure preference.

Applying the seven this repo has today:

| | | |
|---|---|---|
| `graph-refresh` | omh's | the graph does not vanish, it **lies** — `search_graph` keeps answering about code the session has changed |
| `git-unavailable` | omh's | the agent meets `fatal: not a git repository`, decides something is broken, and spends turns repairing what cannot be repaired |
| `graph-orient`, `graph-first`, `graph-read` | omh's | the graph stays correct and goes unused — the agent greps what it could have queried |
| `rust-test`, `rust-format` | the project's | no test at turn end, no format on edit |

Only omh's five are absent from every directory and uneditable, generated from
the manifest at launch. The other two are files, and that is the difference the
next section is about.

### omh's hooks belong to features, not to a flat tier

`graph-orient`, `graph-first`, `graph-read` and `graph-refresh` are not four
hooks that happen to mention the graph. They **are** the graph feature, as much
as the MCP server is — the server makes it queryable, they make it queried.

Today that grouping exists as a comment header in `base/2026.08.toml` and nowhere
else. A comment is the one claim in that file no test can check, while every other
claim an entry makes — `because`, `remove`, `measured`, `instead_of` — is a field
with a test that says it must be filled. So ownership becomes a field:

```toml
[[entry]]
name    = "graph-refresh"
kind    = "hook"
feature = "codegraph"
```

| Feature | What it is | Removed by |
|---|---|---|
| `codegraph` | the MCP server, plus `graph-orient`, `graph-first`, `graph-read`, `graph-refresh` | `omh config mcp rm codegraph` — takes all five |
| `git-notice` | the `git-unavailable` hook, plus the rules section saying the same thing | nothing to uninstall; `[omh]` or not at all |
| `memory` | the MCP server, plus the note-taking rules section | `omh config mcp rm memory` |

**A feature is not a group of hooks. It is a group of entries across kinds** — a
server, some hooks, a section of the rules — and that is why it is the unit that
matters. The manifest already says so about the git notice: the hook and the
`AGENTS.md` section both ship, "read from one string so they cannot drift", the
section stopping the plan being made and the hook catching the call. Half of that
is not a smaller version of it.

Three things follow that were previously convention.

**Removal is symmetric with installation, and feature-level.**
`omh config mcp rm codegraph` takes the server and all four of its hooks
together; installing it brings all four back. Today removing the server leaves
four hooks nudging the agent toward something that is gone. There is deliberately
no way to delete `graph-refresh` while keeping the graph, because that is the one
combination that manufactures confident wrong answers.

**Disabling is per-project, and `[omh]` takes feature names — only feature
names**:

```toml
[omh]
codegraph = false     # the server and all four of its hooks, in this repo
```

`graph-first = false` is not a thing you can write. A key that is not a feature
is an error naming the feature it belongs to, which is also how you discover the
grouping without reading the manifest.

**All or nothing, and that is what removes the hardest question in this
section.** An earlier draft let you disable one hook of a feature, which made
`codegraph` on with `graph-refresh` off reachable — a graph that quietly stops
tracking the code. It answered that with a warning at every launch, plus a
manifest field declaring whether disabling an entry costs correctness, plus the
rule that a nudge switches off silently while correctness does not. All of it
existed to guard a state the granularity itself created. Take the granularity
away and the guard, the field and the rule go with it.

A table of its own rather than a line in `[use]`, because these are not entries
you chose and switching one off is not the same act as dropping a skill. It
layers like every other setting — personal, then project, then local — so a
machine-wide preference and a one-repo exception are both expressible.

**What it costs**, and it is a real thing somebody will want: you cannot keep the
graph and stop the nudging. `graph-first` costs 243 B per grep and `graph-orient`
2,300 B per context rebuild — measured numbers, in the manifest, that a person
with a tight context budget might reasonably want to shed while keeping
`search_graph` and a fresh index. Under this rule that means losing the graph.

The answer for now is that the base set curates bundles, and taking one apart is
disagreeing with the curation — which `omh why` exists to let you do out loud,
by removing the feature. If somebody actually wants graph-without-nudges, that is
a real report and the design should change then, not in anticipation of it.

**`omh why graph-first` can answer "part of `codegraph`"**, and `omh why codegraph`
can list what it brought. Neither is expressible while the link is a comment.

**Not editable is not the same as not switchable.** The base set's rule —
*a default nobody can leave is a cage* — holds through those two different acts,
and conflating them is what made the first draft of this section wrong.

A feature off in this repo needs no warning, because nothing is left half-working
to warn about: `omh repo` reports it off and which file said so, and that is
the whole of it. Silence is only dangerous when something is still running while
believing something else is too.

`omh why graph-refresh` still answers for it: what it costs to have on, which
feature it belongs to, and whether that feature is on here.

This also repairs something the current model cannot do. `init` seeds these with
`write_if_absent`, so a hook you edited or deleted never returns and **omh can
never ship a fix to its own machinery**. `git-unavailable` has already been
rewritten once, after an earlier pattern missed the newline-separated scripts
agents most often emit; every repo initialised before that fix still carries the
broken one. Generated-at-launch means the fix arrives with the upgrade.

What remains in `~/.omh/hooks/` is unambiguously yours.

### The canonical hook format

> **Built.** One correction fell out of writing it, recorded below: two kinds do
> *not* cover all seven hooks.

`{ "event": "Stop", "matcher": "Edit|Write", "command": … }` is Claude Code's
vocabulary wearing a neutral-looking hat. It has survived because opencode
declares no hooks capability, so nothing has ever had to translate one. Three
separate things leak, and they get harder in order:

**Event names.** `Stop`, `PreToolUse`, `SessionStart` are Claude's words for
moments every harness has.

**Matchers.** `Edit|Write` and `Bash` are Claude's *tool* names. Another harness
has the same moments and different names for the things that happen in them.

**The payload protocol**, which is the one that matters. Read what the shipped
hooks actually do: `git-unavailable` runs `jq -r '.tool_input.command'` — parsing
Claude's stdin schema — and emits
`{"hookSpecificOutput":{"hookEventName":"PreToolUse","additionalContext":…}}`,
which is Claude's output protocol. Translate every name in that file and the
**body** still only works on Claude Code.

All three are the same problem: **omh has a closed vocabulary, the adapter says
how this harness spells each word, and the translation happens when the hook is
staged.** No part of it needs to happen at runtime.

**Names become adapter data**, like every other harness difference:

```toml
[capabilities.hooks.events]
session-start = "SessionStart"
turn-end      = "Stop"
before-tool   = "PreToolUse"
after-tool    = "PostToolUse"

[capabilities.hooks.tools]
edit   = "Edit|Write|MultiEdit"
read   = "Read"
shell  = "Bash"
search = "Grep|Glob"
```

An absent entry means this harness has no such moment — the same "absent key is
graceful degradation" rule the capability map already uses, one level down. It
does mean `Plan::dropped` grows granularity: today a whole capability is dropped,
and now a single hook can be, so the report has to name the hook and the event it
wanted.

**A hook declares what it wants, not how a harness spells it:**

```json
{ "on": "turn-end", "run": "cargo nextest run" }

{ "on": "before-tool", "tools": ["read"],
  "when": "[ \"$(wc -c < \"$OMH_TOOL_FILE\")\" -gt 8000 ]",
  "inject": "…query the graph for one symbol instead…" }
```

Two kinds: **`run`** executes and its output is ignored (`graph-refresh`,
`rust-test`, `rust-format`), **`inject`** puts text in the agent's context
(`graph-first`, `graph-read`, `git-unavailable`). The renderer owns the protocol
that delivers each, which is exactly the knowledge that was hand-written into
every hook body.

**They did not cover all seven, and this draft said they did.** `graph-orient`
runs `get_architecture` and injects that command's *output*, which is neither.
So there is a third field, not a third kind: **`capture`** runs a command and
binds its stdout to `$OMH_CAPTURE`, which `when` and `inject` may interpolate —
evaluated before `when`, so a predicate can test it, which is how `graph-orient`
stays silent when the graph answered nothing.

```json
{ "on": "session-start",
  "capture": "codebase-memory-mcp cli get_architecture …",
  "when": "[ -n \"$OMH_CAPTURE\" ]",
  "inject": "Code graph for project $OMH_GRAPH_PROJECT …\n$OMH_CAPTURE" }
```

The alternative was letting `inject` hold `$(…)`, which needs no new field and
puts shell back in the hook body — the thing the format exists to take out.

Two consequences of `inject` being prose that reaches a shell, both of which
fail quietly and so are refused at parse: every `$` must name a variable, since
a bare one expands to nothing and the sentence arrives with a hole in it while
every assertion about the text still passes; and `$$` is how a literal dollar is
written. **Which payload fields a hook wants is derived**, not declared — from
the `$OMH_*` names its bodies mention, so a `when` that stops testing the
filename stops paying for the `jq` in the same edit. The payload is read once
however many fields are wanted: stdin is consumable once, and a second bare `jq`
would bind the field to blank.

**The payload is names too**, which is what makes the whole thing one mechanism
instead of two. `.tool_input.file_path` is Claude's spelling of a canonical
field, exactly as `PreToolUse` is Claude's spelling of a canonical moment. So it
is a third map:

```toml
[capabilities.hooks.fields]
tool-file    = ".tool_input.file_path"
tool-command = ".tool_input.command"

[capabilities.hooks.inject]
template = """jq -nc --arg m {{text}} \
'{"hookSpecificOutput":{"hookEventName":"{{event}}","additionalContext":$m}}'"""
```

And then **the translation happens once, at staging, as string generation.** The
rendered command for `graph-read` is omh-written shell: read the payload, bind
the declared fields to `OMH_*`, evaluate `when`, emit `inject` through the
harness's own template. The hook as authored never sees a schema; the harness
receives something indistinguishable from the hand-written file it has today.

Three maps and a renderer — no new runtime component, nothing added to the launch
path, and adding a harness stays what it already is: filling in data.

**Rejected: a runtime shim.** An earlier draft had the harness call
`omh hook run <name>`, with omh normalizing the payload live. `omh` is already at
`/usr/local/bin/omh` in every sandbox, so it would have worked — but it puts a
process spawn in front of every matching tool call, and `graph-read` matches
`Read`, the most frequent tool there is. Paying that per call, forever, to do
work that can be done once per launch is the wrong trade, and it would have moved
a harness difference out of adapter data and into omh's code — against the one
rule the adapter layer exists to keep.

**Recorded, not built:** an escape hatch for a harness-specific hook with no
canonical spelling — a `raw` body declared per harness and injected only into
that one. The closed vocabulary is the point, so this waits until something real
cannot be said.

### Authoring a hook

A **catalogue** hook runs in repos it was never tested against, so it carries two
obligations a project hook does not. Both are already visible in omh's own, which
face the same problem and solve it the same way:

**Parameterize through the environment, never through the file.** `$OMH_GRAPH_PROJECT`
and `/work` are what let the graph hooks be identical everywhere; a repo name in
the text would not be.

**Degrade to a no-op, not to an error.** `when` carries most of this once the
canonical format lands — `graph-read`'s size gate is a predicate rather than
shell buried in a body — but not all of it. `graph-refresh` ends in `|| true` so
a missing `codebase-memory-mcp` cannot fail a turn, and `graph-orient` produces
nothing when the graph answers nothing. Those stay the author's job: `when`
decides whether to fire, and the body still has to survive its dependencies
being absent.

A **project** hook is free of both. It knows exactly which repo it is in, so it
can name commands and paths directly — that is the whole reason the tier exists.
Which is also the test for where a hook belongs: **if it needs to know which repo
it is in, it is a project hook.** If it works anywhere, it is yours and belongs in
the catalogue where every repo can reach it.

Selection matters more for hooks than for anything else. A skill is inert until
invoked; a hook fires on every matching tool call, in every repo that has it.

### The detected stack is hooks, and only hooks

`omh init` today writes two things from what `detect.rs` found: the `rust-test`
and `rust-format` hooks, and a `## rust` section of `AGENTS.md` saying
*test: `cargo test`, format: `cargo fmt`*.

**The prose goes away.** A hook already runs the command at the right moment; a
paragraph telling the agent the same command exists is the same fact stored
twice, in a place that cannot be checked against the repo and is read on every
turn whether or not it is needed. Detection produces hooks. Nothing detected ends
up in the rules file.

That is subtraction, which the base set is supposed to be capable of — an
argument for adding is always available, and this is what the other direction
looks like. What is lost: an agent that wanted to run the tests *by hand* no
longer reads the command in its context. It can read `Cargo.toml`, and the hook
runs anyway.

**The hooks stay files, written by `init` into `<repo>/.omh/hooks/`.**

```json
<repo>/.omh/hooks/rust-test.json
{ "on": "turn-end", "run": "cargo test" }
```

A toolset does not change weekly. When it does — `cargo test` becomes
`cargo nextest run` — the thing you want is a file you can open and edit, in the
repo where the change belongs, reviewed with the commit that made it. A hook
computed at launch is correct by construction and unreachable: to change it you
would have to learn a second mechanism, and to see what it does you would have to
trust a document. Seeding a file trades a guarantee for a handle, and the handle
is worth more for something you tune a few times a year.

It also means the hook is **committed**, so a teammate cloning gets the project's
test command with the project. That is the hooks-are-shareable rule paying for
itself on the most ordinary case there is.

An earlier draft computed them at launch instead, and gave stack commands a
`[stack]` table in `settings.toml` before that. Both are gone: the file above is
the one mechanism, and it shows the moment it fires on rather than hiding that
behind a convention.

**What this costs is real, and gets a guard rather than a denial.** `init` writes
with `write_if_absent` and never revisits, so:

- **a stack added later gets nothing.** Add a `package.json` in six months and no
  node hooks appear — `init` already ran, and re-running skips what exists.
- **a stack removed later leaves its hooks behind**, calling a command the repo
  no longer has.

Neither is acceptable silently, and neither is a reason to take the file away. So
the launcher compares what `detect.rs` finds against what `<repo>/.omh/hooks/`
holds, and says so:

```console
$ omh claude
omh: node detected (package.json), no hook for it — omh init --hooks
omh: rust-test runs `cargo test`, but no Cargo.toml is here any more
```

Detection stops being a one-time write and becomes a **continuous check on a file
you own** — which is the arrangement this codebase reaches for everywhere else:
omh does not silently correct you, it tells you what it noticed.

## What is customizable, and how

`<repo>/.omh/settings.toml`:

```toml
idle_timeout = "30m"
carry_in     = [".env.local"]

[use]
rules     = ["tdd", "commit-style"]           # ~/.omh/rules/*.md, in this order
mcp       = ["codegraph"]
skills    = ["review-diff"]
hooks     = ["notify-on-stop", "rust-test"]   # yours and this repo's
commands  = []
subagents = ["*"]

[omh]
codegraph = false          # an omh feature — server and hooks, off in this repo
```

Settings stay top-level, exactly as `policy.toml` has them today, so
`set` keeps writing one key at one depth on either command. That has a consequence in
`config::policy`, which iterates every top-level key and stringifies whatever it
finds: `use`, `omh` and `mcp` would be listed as settings whose value is an inline
table. It skips table values instead — one guard, one test, and the alternative is
`omh repo` reporting a curated skill list as though it were a duration.

**One mechanism: an allowlist.** No `exclude`, no `include`/`exclude` pair.
Removing something is deleting its name, and there is one place to look to
answer "is this on".

**Absent means everything.** A repo with no `settings.toml` gets the full catalogue —
upgrading changes nothing, and a new repo is useful before it is configured.
`"*"` is the same thing said explicitly, for a capability you want to keep
following the catalogue as it grows.

**`omh init` writes the file expanded**, with every catalogue entry named. An
explicit list is editable and reviewable in a way `"*"` is not; you curate by
deleting lines.

That has a failure mode and it needs its own guard: a catalogue entry added
*after* `init` is not in the list, so it is off, and the reason is invisible.
So the launcher reports it —

```console
$ omh claude
omh: 2 catalogue entries are not selected here: skills/refactor, mcp/linear
omh:   omh use skills refactor    ·    omh use --all
```

— which is the same principle as the missing-entry warning: the tool says what
it is not doing, by name.

### The commands

Two scopes, so two commands. `omh config` narrows to mean **you** — your
catalogue and your defaults. `omh repo` means **this checkout**.

```console
# this repo
$ omh use skills tdd                  # select from the catalogue → settings.toml
$ omh unuse mcp linear
$ omh use --all                       # resync the list to the whole catalogue
$ omh repo disable codegraph          # an omh feature, off here → [omh]
$ omh repo enable codegraph
$ omh repo set carry_in '[".env"]'    # → settings.local.toml
$ omh repo                            # what is effective here, and what decided it

# you, everywhere
$ omh config set idle_timeout 30m     # → ~/.omh/settings.toml
$ omh config mcp add linear npx -- -y mcp-remote https://…
$ omh config                          # your defaults, and what the catalogue holds
$ omh config edit                     # $EDITOR on the catalogue
```

**Both commands show when given no verb**, which is the pattern `Config` and
`Memory` already follow — `Option<subcommand>`, bare means report.
`omh repo`, `omh config` and `omh memory` then read the same way, and `omh s`
stays the one command that demands a verb, as it already does.

**`--layer` disappears.** The command already says where the write lands, and
that matters because the two scopes want **opposite defaults**:

| Command | Writes to | Why that default |
|---|---|---|
| `omh use` / `unuse` | `settings.toml`, **committed** | what a project uses is a fact about the project, and a teammate cloning should get it |
| `omh repo set` | `settings.local.toml`, **gitignored** | these carry `carry_in` paths and MCP env; a mistyped key must not be committable by accident |

One flag cannot express two opposite defaults, which is why today's single
`omh config --layer` strains. `omh repo set --shared` still writes the committed
file and says so, the way `--layer shared` does today.

**Two verb pairs, mirroring the two tables.** `use` / `unuse` for catalogue
entries, `enable` / `disable` for omh's features. The CLI teaches the file's
structure rather than flattening it: if `omh repo disable` accepted a skill name,
the distinction between *an entry you chose* and *a feature omh ships* would exist
only in the docs.

Bare `omh repo` is the provenance view, and it is where the reporting this design
keeps promising actually surfaces — every entry on or off, every
setting and which file decided it, omh's features and their state, plus the
unselected entries and missing names the launcher warns about. With a curated
list the useful question stops being "what is this set to" and becomes "why is
this skill not here".

**This renames a shipped command.** `omh config set --layer shared` exists today
and would break. It gets the treatment `keys.toml` gets: `--layer` is accepted
for one release, printing the `omh repo` form it maps to, then removed.

**`edit` validates its argument; it does not confine the editor.** Once `$EDITOR`
is spawned it is a full program running as you, and any fence omh drew around it
would be decorative — there is no trust boundary between omh and the person whose
home directory this is. The boundary that matters is the one that already exists
structurally: `~/.omh` is not mounted into the sandbox, only the staged
capability directories are, and those are read-only, so the agent can read a
selected skill and cannot write one.

What does need a guard is the **name**, the moment `edit` takes one and joins it
to a directory: `omh config edit skills ../../../.ssh/id_rsa` is traversal, and
it is the shape `memory::validate_key` and `carry::validate_pattern` already
refuse. Same rule, same place — validate where the name is minted, not where it
is used, so every future caller inherits the guard instead of remembering it.

### Precedence

Content has no layers any more — there is one catalogue. **Settings** keep
theirs, and the order is unchanged:

```
~/.omh/settings.toml  →  <repo>/.omh/settings.toml  →  <repo>/.omh/settings.local.toml
```

`settings.local.toml` is the write target for `omh repo set`, for the reason it
always was: a mistyped API key must not be committable by accident. It is also
where per-project MCP secrets go —

```toml
[mcp.linear.env]
LINEAR_API_KEY = "..."
```

— an override of a catalogue entry's environment, which is configuration. It
cannot define a server the catalogue does not have.

## How it is injected

Unchanged where it was already right: staged into `~/.omh/run/<repo>/<session>/<harness>/`,
bind-mounted **read-only** onto whatever path the adapter declares, never copied
into a real config location. Nothing drifts and there is nothing to clean up.

What changes is one step in the middle:

```
catalogue  →  selection  →  render  →  adapter binding  →  mount
 ~/.omh/      [use]         canonical   path + also        read-only
                            per cap     per harness
```

**Selection resolves before the adapter is consulted**, which is what makes it
harness-agnostic by construction rather than by one code path per capability.
`skills`, `mcp`, `commands` and `subagents` resolve identically — a name is in
the list or it is not.

Hooks resolve the same way and then have more to do, because they are the only
capability with three sources and two tables. That is not a special case in the
launcher: `[use]` names entries in the two tiers that are yours, `[omh]` names
features in the one that is omh's, and both hand the same shape to the renderer.

Selecting something the chosen harness cannot express keeps the shape it has
today — an absent key in the adapter and a line in `Plan::dropped` — but the
granularity grows. A harness with no `after-tool` event drops the hooks that
wanted it, not the whole hooks capability, so the report names the hook and the
event it asked for.

## Rules: composition, not replacement

The rules file is assembled on the host, in this order, and then mounted:

```
1.  ~/.omh/rules/<selected>    you — the ones this repo uses, in the order [use] lists them
2.  <repo>/AGENTS.md           the project
3.  omh's generated section    the sandbox — always last
```

Each section carries a provenance marker, so the agent and `omh repo` can both
answer whose rule is whose.

### Concatenating is the fallback, not the plan

Those three are *sources*. How they reach the harness is the adapter's business,
and `rules` stops being hardcoded to one answer:

| The harness has | The adapter declares | What omh does |
|---|---|---|
| a rules **directory** | `render = "dir"`, `path` = that directory | mounts the selected rules there, one file per rule |
| a rules **file** | `render = "concat"`, `path` = that file, `also` = other names it answers to | joins them into it, in order |
| no way to reach it | the `rules` key omitted | drops the capability and reports it, as today |

**Every path is adapter data — there is no filename in omh's code.** The middle
row is what Claude Code already uses: `path = "/work/CLAUDE.md"` with
`also = ["/work/AGENTS.md"]`, so the composed document arrives under the name
that harness reads, and under the neutral one as well.

That row also absorbs the case where a harness documents no rules feature at all.
The adapter still names a file — `/work/AGENTS.md` for a harness that follows the
convention, something else for one that follows a different one — and the only
difference from a documented feature is how confident the claim is. A bet on a
convention is still a bet, which is what `omh doctor` exists to settle. The last
row stays available for a harness where even that is untrue: a capability dropped
by name beats a file nobody reads.

Both renders already exist in `Render`, so this is the `rules` capability
ceasing to be the exception that only ever concatenates. **Native first,
concatenation when there is nowhere else to put them**, because a harness with a
rules directory usually loads those files on its own terms, and flattening them
into one blob throws that away.

The two paths differ in one more place than the mount:

**On a `dir` harness the project's `AGENTS.md` is left alone.** The harness reads
it natively — that is what `also = ["/work/AGENTS.md"]` records — so omh writes
only the catalogue rules and its own generated section into the directory.
Nothing is mounted over the repo's file, nothing is duplicated in the context,
and the original bug does not arise: there is no shadowing because there is no
overlap.

**On a `concat` harness there is one slot, so all three sources go into it** —
which is the composition this section is about, and the only case where hiding
the project's file is a risk to manage.

**Ordering.** On `concat` the rules join in the order `[use]` names them, not
alphabetically: rules build on each other, a general one followed by its
exception reads differently reversed, and the list you wrote is the only place
that ordering can come from. On `dir` the harness decides load order and omh
cannot make it obey a list — so staged filenames are prefixed (`01-tdd.md`) to
express the intent, and it is stated as intent rather than a guarantee.

**Which render each adapter uses is a claim about external software**, exactly
like every `path` in an adapter, and no test in this repo can settle it. A green
suite proves omh mounted a directory faithfully, never that a harness read it.
That is `omh doctor`'s job, and until doctor has run against a harness the
`dir` binding for it is unverified.

**On the project's side omh reads one filename: `AGENTS.md`** — the
harness-neutral one. Where it lands is the adapter's business, per the table
above. **Reading is canonical, writing is per-harness**, and neither half has to
know the other's names.

That leaves two cases that must not be silent, because both end with a project's
rules not reaching the agent — the bug this section exists to fix. Both are
`concat` problems; on a `dir` harness the repo's file is never touched:

- **`CLAUDE.md` but no `AGENTS.md`**, which is most repos that have used Claude
  Code. omh composes it anyway and says which file it read, once, with the
  suggestion to rename. A fallback that announces itself is not a silent
  fallback; refusing outright would leave the agent running with no project
  rules until somebody notices, which is strictly worse than the status quo.
- **Both present, and different.** `AGENTS.md` wins — it is the canonical name —
  and the launcher reports that `CLAUDE.md` was not composed. Many repos carry
  both with one pointing at the other, so this is common and harmless; the ones
  where it is not harmless are exactly the ones that need telling.

**Content is the branch's copy if the worktree has one, otherwise the default
branch's**, via `git show <default>:AGENTS.md`; `session::default_branch`
already resolves the name and already refuses to trust a stale `origin/HEAD`.
Branch-first, because a session that has just written an `AGENTS.md` the default
branch does not have yet should be governed by it. The accepted risk is the
other direction: a session editing that file changes the rules it runs under
next launch. That is visible rather than hidden — the composed file names where
each section came from — and the alternative silently ignores work in progress.

**omh's own section stops being prose somebody pasted.** The "git does not work
in this session" notice and the note-taking protocol are written by hand into
`.omh/profile/AGENTS.md` today, which means they exist in the repos where
somebody remembered and nowhere else. They become base-set entries with
`kind = "rules"` — a rule section belonging to a feature, exactly as its hooks do
— so they reach every project, `omh why git-notice` can answer for them, and they
land in the cost rollup [v1](roadmap.md#v1--accountability) wants instead of
being invisible weight. On a `dir` harness they are one more staged file; on a
`concat` one they are the last section. Same entries, same feature switch, two
renders.

The mount stays read-only and `place_destination` / `hide_staged_rules` are
untouched: the project's tracked rules file must still be byte-for-byte intact on
disk under every name the adapter mounts over, and omh's staging must still be
absent from the agent's `git status`. `carry::STAGED_RULES` stays the list of
names to hide, because it is about what gets mounted, not about what gets read.

All of that is **`concat` machinery only**. A `dir` binding mounts into the
harness's own config directory, nothing lands in `/work`, and so there is no
placeholder to create and nothing for `git status` to see. The awkward part of
the current design turns out to be the fallback path rather than the main one.

**Deliberately absent: per-project personal rules.** There is no gitignored
`<repo>/.omh/rules/`. Hooks earned their repo tier by being project-scoped by
nature; rules did not — a project's rules are its `AGENTS.md`, which is tracked,
reviewed and shared, and anything of yours that applies here is a catalogue rule
you select. The demand for a private fourth section is speculative, and if it
arrives it arrives with a reason.

## Build order

Each phase is independently useful and independently shippable, which is the
only reason to have five of them.

**One ordering constraint overrides the rest: the canonical hook *format* has to
land with P3.** P3 is where `<repo>/.omh/hooks/` opens and people start writing
files. A format you can change before anyone has written against it costs
nothing; the same change after costs a migration and somebody's afternoon. So
`on` / `run` / `inject` / `tools` ship as the format from the first commit that
reads that directory, with the three adapter maps behind them.

P5 is not more machinery — the maps are complete when P3 lands. It is the
admission that **a translation with one harness on the far side is untested**. A
map from `turn-end` to `Stop` proves nothing while `Stop` is the only word omh
has ever emitted, and this repo already knows that adapter paths are claims about
external software no unit test can settle. It is blocked on a second harness that
has hooks at all, which is a fact about the ecosystem rather than a task.

| | | |
|---|---|---|
| **P1** | compose the rules on a `concat` binding | **landed** — fixed a bug on its own; no storage change |
| **P2** | `kind = "rules"` and `feature` in the base set, omh's own hooks **generated** rather than seeded, `remove` moved to the feature level | **landed**, plus `[omh]` read-only, brought forward so `remove` names something that works |
| **P3** | catalogue move, the canonical hook format and its three maps | **landed**. No migration: omh had no users but its author, so the one repo and the one home directory holding the old layout were moved by hand |
| **P4** | `[use]`, `omh use` / `unuse`, `omh repo`, `init` writing it expanded, the unselected report | |
| **P5** | the three maps exercised by a **second** harness | the only thing that can prove the translation |

**Generation came before the move**, and an earlier draft had it the other way
round. Deleting omh's five hooks is only safe once the manifest produces them —
run the other way, a repo spends a phase without them and no way to notice.
Whatever replaces a file has to exist before the file is removed.

**No migration was written, and that was a decision rather than an omission.**
omh had no users but its author, so the one repo and the one home directory in
the world holding the old layout were moved by hand, in the same diff, reviewable
— `git mv`, not machinery for leaving a state nobody is in. What a migration
would have cost is real (it has to prove the five hooks reproduced before
deleting them, name by name, and splitting `AGENTS.md` by heading is a guess that
must leave the original on disk), and none of it would ever have run.

Two guards survive from it, because each is one branch and each protects
something unrecoverable:

- **a `keys.toml` still present is a loud error naming both paths**, per above.
- **a file answering to a manifest hook name is an error naming both.** Silently
  skipping it was right while the only such files were leftovers omh had seeded;
  `<repo>/.omh/hooks/` is somewhere people write on purpose.

What the old layout held, and where it went by hand:

| Was | Went to |
|---|---|
| the five omh ships, in `.omh/profile/hooks/` | nowhere; generated from the manifest |
| `rust-test`, `rust-format` | `<repo>/.omh/hooks/`, rewritten into the canonical format |
| the code-graph, memory and git sections of `AGENTS.md` | nowhere; base-set entries with `kind = "rules"` |
| the detected-stack section | nowhere; deleted, per above |
| the rest of `.omh/profile/AGENTS.md` | `<repo>/AGENTS.md` |
| `.omh/profile/mcp.json`, `policy.toml` | `~/.omh/mcp.json`, `<repo>/.omh/settings.toml` |

P2 rewrote five `remove` fields in the manifest. Each currently reads
`rm .omh/profile/hooks/<name>.json`, which will name a path that no longer
exists — and a `remove` instruction that silently does nothing is worse than
none, because `omh why` presents it as the way out. The four graph hooks point at
`omh config mcp rm codegraph`; `git-unavailable` points at `[omh] git-notice`,
since there is nothing to uninstall.

`feature` lands in the same phase, and it needs the test the other fields have:
**every entry names the feature it serves**, hooks and MCP servers and rules
sections alike, so nothing can be added without saying what it is part of. That
is the same guard as `every_base_set_entry_states_its_case`, extended one field —
and with `[omh]` keyed on features it is load-bearing rather than documentary: an
entry with no feature is an entry nobody can switch off.

Test-first, as everything here is. The regressions worth naming in advance,
because they are the ones that would ship silently:

- a repo whose tracked `AGENTS.md` reaches the agent — the bug this started from
- a repo with `CLAUDE.md` and no `AGENTS.md` is composed, and told
- selected rules concatenate in the order `[use]` lists them, not alphabetically
- on a `dir` binding the repo's `AGENTS.md` is **not** mounted over and **not**
  copied into the rules directory — no shadowing, and no paying for it twice
- the migration never deletes a section of `AGENTS.md` it did not generate
- a selected name absent from the catalogue is **reported**, and the session
  still launches
- a catalogue entry absent from an explicit `[use]` list is reported, not
  quietly dropped
- `[use]` cannot switch off omh's own hooks — `[omh]` is the only door
- `[omh]` rejects a hook name, naming the feature it belongs to — the state
  "graph on, refresher off" must be unrepresentable, not merely warned about
- a project hook shadows a catalogue hook of the same name, and a collision with
  a **manifest** name is an **error naming both**
- a stack detected with no hook for it is reported, and a hook whose stack is
  gone is reported — `init` writes once, so the launcher is what keeps noticing
- `omh config mcp rm codegraph` leaves **no** graph hooks behind — today it
  leaves four, nudging the agent toward a server that is gone
- the repo's hooks are named at launch, and a new or changed one is called out
- `edit` refuses a name that escapes its directory — `..`, a leading `/`, a
  backslash — the way `validate_key` and `validate_pattern` already do
- a hook whose event the chosen harness cannot express is **dropped by name**,
  saying which event it wanted — not dropped with the whole capability
- every shipped hook survives a round trip through the canonical format: the
  rendered Claude settings must match what the hand-written JSON produces today,
  or the translation silently changed omh's own behaviour

## Alternatives not taken

- **Keep content in the repo as well as the catalogue.** It is what exists, and
  it is why "where is this skill" has three answers. Team sharing was the reason
  and `source` + `omh sync` is a better one.
- **Opt-in selection** — a project sees only what it names, absent means nothing.
  Tighter context budgets and no surprise MCP servers, but it breaks every
  existing setup on upgrade and makes a fresh repo useless until configured.
  Against "your setup is already there".
- **`include` / `exclude` pairs.** Two mechanisms to express one thing, and a
  resolution order to explain. An allowlist you edit is smaller.
- **Reading the default branch's rules always.** Stable, and every session on a
  repo composes identical text — but a session working on the rules file cannot
  see its own work, which is the case somebody hits on day one.
- **Concatenating always, even where the harness has a rules directory.** One
  code path and one output to reason about, and the composed document is easy to
  print. But it discards whatever the harness does with separate rules — loading
  on demand, scoping by glob, showing the user which fired — and replaces it with
  a blob omh assembled. A distribution that flattens what its harnesses can
  already do is subtracting rather than adding.
