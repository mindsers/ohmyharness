---
title: Decisions
description: Every load-bearing choice omh has made, with its reasoning.
---

Every load-bearing choice omh has made, and the reason for it. If you are about
to change one of these, the reason is what you need to argue with.

**Status: a ledger, not a design.** Every entry records a choice already made and the reason it was made. The Status column in [the base set](#the-base-set) is the component's; the *design* for each lives on its own page.

## At a glance

| Decision | Choice |
|---|---|
| [Category](#category-a-distribution) | **a distribution** |
| [Code graph](#code-graph-wire-an-existing-mcp-server) | **wire an existing MCP server** |
| [LLM routing](#llm-routing-not-ours) | **not ours** |
| [Language](#language-rust) | **Rust** |
| [Runtime](#runtime-a-pluggable-backend) | **a pluggable backend** |
| [Repo exposure](#repo-exposure-a-git-worktree-auto-branched) | **a git worktree, auto-branched** |
| [Git in the sandbox](#git-in-the-sandbox-a-repository-of-its-own) | **a repository of its own** |
| [Untracked files](#untracked-files-a-carry_in-allowlist) | **a `carry_in` allowlist** |
| [Unit of work](#unit-of-work-a-long-lived-session) | **a long-lived session** |
| [Session persistence](#session-persistence-dtach-not-tmux) | **`dtach`, not tmux** |
| [IDE access](#ide-access-ssh-into-the-session) | **SSH into the session** |
| [Naming a session](#naming-a-session-snn-first-one-form) | **`sNN` first, one form** |
| [Getting work back](#getting-work-back-omh-snn-commit) | **`omh sNN commit`, squash or `--keep`** |
| [Staying current](#staying-current-omh-snn-sync-merged-on-the-host) | **`omh sNN sync`, merged on the host** |
| [Content scope](#content-scope-one-personal-catalogue) | **one personal catalogue** |
| [Repo content](#repo-content-hooks-and-nothing-else) | **hooks, and nothing else** |
| [Settings scope](#settings-scope-two-layers-in-the-repo-plus-a-template) | **2 layers in the repo, plus a template** |
| [Selection](#selection-an-allowlist-use) | **an allowlist, `[use]`** |
| [Absent selection](#absent-selection-means-everything) | **means everything** |
| [Command scope](#command-scope-omh-settings-is-you) | **`omh settings` is you, this checkout is the rest** |
| [Write default](#write-default-the-key-decides) | **the key decides, not the command** |
| [Capability floor](#capability-floor-a-superset-adapters-degrade) | **superset, adapters degrade** |
| [Hook vocabulary](#hook-vocabulary-closed-translated-at-staging) | **closed, translated at staging** |
| [Tool vocabulary](#tool-vocabulary-one-closed-set) | **one closed set, per-adapter map** |
| [Portability](#portability-store-the-standard-where-one-exists) | **store the standard where one exists** |

Also on this page: [the base set](#the-base-set) as a decision,
[what was deliberately not decided](#decisions-deliberately-not-made),
[what the work loop refuses to do](#what-the-work-loop-refuses-to-do),
[a ratified deviation](#deviations-from-a-written-design-ratified) and
[three reversals](#reversals-worth-knowing-about).

## What omh is

### Category: a distribution

**omh is a distribution.** The parts exist already; assembly and subtraction are
the value. → [Why a distribution](/concepts/distribution/)

### Code graph: wire an existing MCP server

**omh wires an existing MCP server rather than writing a graph.** Distributions
package; they don't reinvent. Its browsable website is scoped to the repo, not
the session — see [the first reversal](#reversals-worth-knowing-about).

### LLM routing: not ours

**omh does not route models.** The harness logs in as itself and reaches
whatever it is configured to; omh passes no model and proxies nothing. See also
[which model you use](#decisions-deliberately-not-made).

### Language: Rust

**omh is written in Rust.** It ships as a single binary, and `omh` wraps every
invocation, so startup is felt.

## The sandbox

### Runtime: a pluggable backend

**The runtime is a pluggable backend: Docker unless you say otherwise.**
`podman` and `sbx` are opt-in through `runtime`. `sbx`'s capabilities were
measured against sbx 0.39.0, but no full `omh doctor` run has accepted it yet. No
vendor lock. → [Runtime backends](/concepts/architecture/#runtime-backends)

### Repo exposure: a git worktree, auto-branched

**Each session gets a git worktree on a branch of its own.** The agent cannot
reach your checkout or `main`, and review is `git diff`. →
[Sessions](/concepts/sessions/)

### Git in the sandbox: a repository of its own

**The sandbox has a repository of its own.** Your checkout is never mounted, so
the agent gets a gitdir holding one commit and none of your history.

### Untracked files: a `carry_in` allowlist

**Untracked files enter only through `carry_in`.** Worktrees are pure git, so
secrets enter only by declaration. → [`carry_in`](/reference/configuration/#carry_in)

## Sessions

### Unit of work: a long-lived session

**The unit of work is a long-lived session.** It keeps the index warm and makes
harness switching instant. → [Sessions](/concepts/sessions/)

### Session persistence: `dtach`, not tmux

**Sessions persist through `dtach`, not tmux.** omh needs detach and reattach,
and SSH already provides multiplexing. → [Sessions](/concepts/sessions/)

### IDE access: SSH into the session

**Your editor reaches the session over SSH.** One dependency tree, shared with
the agent. → [Editors](/guides/editors/)

### Naming a session: `sNN` first, one form

**The session selector comes first, in one form.** Several sandboxes of one
repo are reached from one place, so the selector leads and everything after it
is unchanged. → [The shape of the CLI](/reference/commands/#the-shape-of-the-cli)

### Getting work back: `omh sNN commit`

**Work comes back through `omh sNN commit`, squashed or with `--keep`.** You
choose at commit time whether the agent's own commits and messages land. How
`--keep` lands them departs from its design — see
[the ratified deviation](#deviations-from-a-written-design-ratified).

### Staying current: `omh sNN sync`, merged on the host

**Trunk reaches a session through `omh sNN sync`, merged on the host.** No
commit of yours may enter the sandbox, so it receives files and conflict markers
rather than history. →
[`omh sNN sync`](/reference/commands/#omh-snn-sync--trunk-moved-and-the-session-catches-up)

## Configuration

### Content scope: one personal catalogue

**Your configuration lives in one personal catalogue.** "Where is this
skill" had three answers, and a union can add but never subtract. →
[Configuration](/reference/configuration/)

### Repo content: hooks, and nothing else

**A repository carries hooks and nothing else.** A hook binds to a repo's
commands; a skill is a way you work.

### Settings scope: two layers in the repo, plus a template

**A repo's settings are two layers in the repo, plus a template.** A repo's
behaviour has to be explained by files the repo contains. `~/.omh/default.toml`
seeds a new one and decides nothing after that. →
[Configuration](/reference/configuration/)

### Selection: an allowlist, `[use]`

**What a repo takes from your catalogue is an allowlist, `[use]`.** Removing
something is deleting its name, with no `include`/`exclude` pair to reconcile. →
[`[use]`](/reference/configuration/#use--what-this-repo-takes-from-your-catalogue)

### Absent selection: means everything

**No `[use]` means everything.** Upgrading changes nothing, and a new checkout
is useful before it is configured.

### Command scope: `omh settings` is you

**`omh settings` is you; this checkout is the rest.** It was *`config` is you,
`repo` is here*, on the argument that the two wanted opposite write defaults,
and one `--layer` flag cannot express both. →
[The command surface](/project/retrospectives/#the-command-surface--done-070-6990)

### Write default: the key decides

**Where a write lands is decided by the key, not the command.** `src/key.rs`
records, per key, whether a value there can name a credential. Committed is the
default now — most settings are facts about the project — and that table is
what keeps a secret out of it.

<details>
<summary>What the rule was, and why it moved</summary>

It was *a value never lands in the committed file; a name may*, which held while
`repo set` sent every value to the gitignored file. `omh set` serves every key
from one command, so the judgement moved into `src/key.rs`. The full story, and
the second reversal it forced, is under
[Reversals](#reversals-worth-knowing-about).

</details>

## Harnesses

### Capability floor: a superset, adapters degrade

**The catalogue is the superset of what any harness can express; an adapter
degrades by leaving keys out.** omh must never cost you a feature you already
had. → [Adapters](/reference/adapters/#capabilities-are-optional-keys)

### Hook vocabulary: closed, translated at staging

**Hooks are written in omh's closed vocabulary and translated when they are
staged.** `event`, `matcher` and the payload are one harness's words, and there
is no runtime shim. → [Hooks](/reference/adapters/#hooks-three-maps-and-a-template)

### Tool vocabulary: one closed set

**Tool names are one closed set, mapped per adapter.** They are the one thing
skills, subagents and hooks all leak.

### Portability: store the standard where one exists

**omh stores the standard format where one exists.** Skills and rules already
travel; only hooks and subagents need omh.

## The base set

This is the product. Everything else is a place to put it.

```
omh init             → base system. no questions.
                     → a toolchain and hooks, derived from the stack init detected.
omh settings mcp add …
                     → the archive is still there, one command away,
                       and not in your face.
```

| Component | Status | Justification |
|---|---|---|
| sandbox + worktree branch | ✅ | safety; non-negotiable |
| rules, composed with the repo's own | ✅ | omh's sections are base-set entries composed into every session; it writes no `AGENTS.md` |
| `omh sNN attach` | ✅ | IDE attach |
| **`codegraph`** | ✅ | structural queries instead of re-grepping every task |
| test-on-stop + format-on-edit hooks | ✅ | `init` detects the commands and wires them |
| memory | ✅ | survives harness switches — one store per repo, shared by its sessions and never committed; hub pages do not ship, and the committed layer was removed in 0.14 |
| egress allowlist | ⬜ | inherited from the runtime |

**If a further entry needs a paragraph to justify, it belongs in your
catalogue, not the base set.**

The set lives in a versioned manifest rather than in the binary, and the
justification is enforced by a test rather than by convention — see
[the base set](/reference/base-set/). It is also the part of omh with the least
evidence behind it; see
[the honest weak spot](/concepts/distribution/#the-honest-weak-spot).

## Decisions deliberately not made

- **Which model you use.** The harness's own setting. Routing is a solved
  problem with several good solutions and no reason for omh to have an opinion.
- **Which editor you use.** [Editors are data](/guides/editors/#editors-are-data),
  and the integration point is an SSH config include rather than a plugin, so
  editors omh has never heard of work anyway.
- **Whether to trust the sandbox over the branch.** Both. A sandbox protects the
  host; the worktree branch protects the repo. They are not substitutes, and
  conflating them is how people end up surprised.

## What the work loop refuses to do

Reading a session's work, landing it and staying current with trunk are omh's.
These are not:

- **No omh verb for anything you could run on the host.** No `omh sNN rebase`,
  `cherry-pick`, `stash` or `blame`. The moment you want real git you are on the
  host with real git; omh's job is to make that always possible and never
  dangerous.
- **No merge UI.** Conflicts are markers in files. The agent fixes them, or you
  do, with your own tools.

Considered and not built, with the reason:

- **Moving work between sessions** (`omh s01 take --into s03`). A real workflow,
  and omh is well placed for it — but it is a second way to land work, and one
  landing path is why `-m` and `--keep` are mutually exclusive.
- **Forking a session from another.** Cheap, and useful for parallel
  exploration, but it is a launch flag rather than a git feature.
- **Comparing two sessions** (`omh s01 diff --against s03`). The payoff of
  parallel exploration, and the one worth revisiting soonest. Held back while
  `diff` was growing a patch and a checkpoint argument.

And one that is open rather than refused:

- **Naming sessions.** `omh sNN push <name>` refuses to invent a branch name,
  correctly. A session that carried a name from the start would answer that
  once instead of at the end — but `omh new claude` asking a question at launch
  would cost the thing `init` exists to protect.

## Deviations from a written design, ratified

**A `--keep` selection is a `cherry-pick`, not a generated rebase todo (#56).**
[The design](https://github.com/mindsers/ohmyharness/blob/afad736a609fd6e3921e09f784c420e05caca115/docs/design/git.md)
specified the todo, delivered through `GIT_SEQUENCE_EDITOR` pointed at omh's own
binary. The two are user-visibly identical, and `cherry-pick` is the smaller
mechanism, so it was kept — on measurement.

**The one real difference is a raised git floor.** `cherry-pick --empty=` is
newer than `rebase --empty=`, so a selection fails on an older git where the
todo would have worked. The exact version is unmeasured — only 2.55 was
available — and pinning it is part of `omh doctor` learning git.

<details>
<summary>The justification that failed, and the measurement that replaced it</summary>

The first justification given for `cherry-pick` was wrong: it claimed the
designed mechanism could not be reached by a test, when `tests/cli.rs` runs the
real binary and `memory::deliver` in this repo already injects `current_exe` for
exactly that reason.

It was kept anyway, on measurement rather than on the argument that failed. Both
mechanisms were run against the same history: `--keep 3,1` lands `three` then
`one` under each, and a selected merge is refused by each — `'pick' does not
accept merge commits` from the todo, `is a merge but no -m option was given`
from `cherry-pick` — so omh's own up-front refusal is correct either way and the
user never sees either message.

`cherry-pick` is the smaller mechanism: no editor, no `sh -c` quoting, no second
entry point into omh, no `hide = true` subcommand for `RESERVED` to carry.

</details>

## Reversals worth knowing about

Three decisions were made, shipped, and undone. All are more instructive than
the choices that held.

**Per-session graph website → one per repo.** Every session's graph lives in one
volume, so a per-session server displayed every other session's graph anyway — N
identical websites on N ports, each running inside a container that held a
writable worktree and live credentials. Scoping the service to the repo removed
the duplication, removed the exposure, and deleted the `pgrep` guard, detached
`exec` and `pkill` that the per-session version needed — each of which had been a
bug before it worked. **Choosing the right scope removed them rather than fixing
them.**

**Credentials mounted at a private path → mounted where the harness reads.** The
first version mounted a credential volume at `$HOME/.omh-creds`, which no
harness has ever read. It tested green. See
[accounts](/guides/accounts/#mount-the-directory-never-the-token-file).

**The write default lived in the command → it lives in the key.** `config set`
and `repo set` had opposite defaults and one flag could not express both, so
the safety came from the destination: `repo set` sent *every* value to the
gitignored file and nothing could reach git unasked. That was also why a
teammate cloning a repo got none of the settings it had chosen. One command
serving every key cannot work that way, so the judgement moved into
`src/key.rs`, per key, and the committed file became the default.

**The rule is now one function that four commands call** — every repo layer
that already holds the key, else the committed file, except a key that can name
a credential.

<details>
<summary>The second reversal the write default forced, inside one release</summary>

`omh set` first shipped with a rule of its own — the committed file, unless the
gitignored one already held the key — while `omh use`, `omh unuse` and the
feature switch each answered differently. Three answers to one question, and
every difference was a place for a command to report success over something it
had not changed: `omh unset carry_in` left a map to a credential standing in a
committed file while saying it had removed it.

Each of the three original answers was defensible alone; only asking whether
they were answers to the *same question* exposed them.

</details>

## Next

<div class="cards">

- [Architecture](/concepts/architecture/)\
  Images, runtime backends, and what lives where on disk.
- [Sessions](/concepts/sessions/)\
  The session model, and how it persists.
- [Configuration](/reference/configuration/)\
  The layers, the catalogue and `[use]`.
- [Roadmap](/project/roadmap/)\
  What comes next, and what gates it.

</div>
