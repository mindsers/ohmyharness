# Meeting a repo that already exists

**Status: Part 1 is built. Part 3 — import — is built for hooks; the rest is designed and not built.**

Built and shipping: stack definitions as data (`stacks/*.toml`), the stack image
layer and the tag that keys it, `[provision]` as the recorded resolution, the
predicates that produce it, one sandbox probe read two ways — verifying a stack's
`needs` and deciding which hooks the image can run — and the per-image
measurement cache in `~/.omh/facts.json`. `[toolchain]` is **deleted**, as §1.8
argues it should be; a repo that still has the table gets an error naming it and
pointing at `[provision]`.

Also built: hooks decoupled from stacks. `hooks/*.json` is a fourth data kind,
each naming the ecosystem it belongs to as a reference; a hook for an ecosystem
a repo is not is never offered to it.

Also built: deriving how *this* project spells its commands. `src/derive.rs`
reads a lockfile, a `packageManager`, a `package.json`'s scripts or a
runner's targets and writes the hooks the catalogue cannot hold — for
ecosystems the catalogue does not already cover, executing nothing.

Also built: the two questions of last resort. A marker omh recognises and has no
stack for is asked about once and the answer written as
`<repo>/.omh/stacks/<name>.toml`; a project nothing can test is asked what tests
it. Silence declines, a closed pipe stops, and a repo-local stack may add an
ecosystem but never answer to a name omh ships.

Also built: `omh import hooks <harness>`. It reads a harness's own hook file back
through the same vocabulary that renders to it, writes what it can say into
`<repo>/.omh/hooks/` and into `[use]`, and leaves anything it cannot say whole
where it is — named. `init` reports what it can see and acts on nothing.

**Part 1 is complete.** Still designed and not built: importing rules, skills,
commands and subagents (Part 3's remainder).

Two things happen when `omh init` meets a repo that is not empty. It has to
build an environment the project can actually be worked in, and it has to not
throw away the agent setup somebody already has. This page covers both.

## The failure this starts from

A rust repo's sandbox had no rust toolchain. `init` detected `Cargo.toml`, wrote
a turn-end hook running `cargo test`, and every turn ended in:

```
Stop hook error: /bin/sh: 1: cargo: not found
```

Three gaps were stacked behind that one message, and only the first is obvious:

1. no Rust toolchain in the sandbox at all
2. no C toolchain either — `cargo` alone compiles and then fails at link
3. rustup installs to `$HOME/.cargo/bin`, which is **not** on the `PATH` that
   `/bin/sh` hands a hook. A correctly installed toolchain still reports
   `cargo: not found` from inside a hook while working in a login shell

The third generalises: **detection runs on the host, the work runs in the
sandbox.** Those are different computers, and a check performed on the friendlier
of the two passes while the sandbox stays broken. `detect::preferred_harness`
already states the rule for harnesses — *"Host evidence is only a hint: the
harness itself runs in the sandbox"* — and it holds unchanged for toolchains.

The first correction, though, is about what kind of failure this is. It reads
like a hook problem and is not one. A human opening a shell in that sandbox and
typing `cargo test` gets the same error. **The environment was missing a tool the
project needs**, and no arrangement of hooks fixes that.

## Principles

**The environment comes first.** omh exists to hand somebody a secure,
preconfigured place to work with an agent. If the tools that project needs are
not in it, nothing else omh does matters — the human cannot build, the agent
cannot check its own work, and both are reduced to guessing. Every other rule
here is subordinate to that one.

**The enemy is not configuration, it is a broken first turn.** omh asked nothing
and turn one failed. Zero config cost more than one question would have. The goal
is fewest questions *subject to the environment working*, never fewest questions.

Four kinds of unknown, told apart by **where and when the answer is knowable**:

| Tier | Knowable | Example | Mechanism | Cost |
|---|---|---|---|---|
| 1 | from the repo, on the host | `Cargo.toml` → rust | derive | free |
| 2 | only in the sandbox | does `cargo` resolve in a hook's `PATH`; does this repo declare pnpm | probe | a container run |
| 3 | nobody has encoded it | what command tests this project | ask once, record | one question |
| 4 | only the person knows | "these six hooks are mine, keep them" | import | one command |

Push every question up a tier if you can. Most things that feel like they need
asking are derivable from a file the repo already commits.

**The two failure directions are not symmetric.** Missing a gap costs one
confusing error — the status quo. Inventing one makes omh withhold working
behaviour or interrogate somebody about a toolchain they have, which costs trust
in every answer omh gives. So everything ambiguous resolves to *cannot tell*, and
*cannot tell* is never a licence to act.

**Degrade to nothing, never to wrong.** An unrecognised project getting no hook
is a correct outcome. A wrong command is not.

---

# Part 1 — The stack is the environment

## 1.1 What a stack is, and what it is not

A stack answers one question: **what does this project need in order to be worked
on?** If the agent has just changed something and wants to check it, what tool
does it reach for, and is that tool here?

A stack is therefore *not* a set of hooks. Hooks are automation — when something
runs, on which events, matching which tools. That is a separate axis, covered in
Part 2. Conflating the two produced the original wrong fix: treating a missing
compiler as a hook that should be suppressed, which hides the symptom and leaves
the environment as broken as it was.

What a stack declares, and nothing else:

- **`name`** — what to call it, in a report and in `omh why`
- **`marker`** — how to tell this is one
- **a list of things it provides**, each with what must resolve, when it applies,
  how the image gets it, and what that cost (§1.4)

It carries no commands. A command belongs to a hook, and hooks already have two
homes with a defined precedence: `~/.omh/hooks/` for the ones you want
everywhere, `<repo>/.omh/hooks/` for the ones that belong to a project.
`render::merge_hooks` unions them — *"the repo's shadow yours"*. A third copy in
the stack file would be the same string in a second place, free to disagree with
the first.

## 1.2 Stack definitions as data

Compare what is data and what is code:

| | Where it lives | To add or fix one |
|---|---|---|
| Harnesses | `adapters/*.toml`, embedded in the binary | edit a TOML file, release |
| The base set | `base/*.toml`, embedded in the binary | edit a TOML file, release |
| **Stacks** | `const KNOWN` in `detect.rs` | **write Rust, release** |

The difference is the skill required, not the release. Both columns ship in a
version; only one of them needs somebody to know Rust.

**Data does not mean locally editable.** Adapters and the base set land in
`~/.omh` so the opinion imposed on somebody is reviewable by them — the
[trust](trust.md) argument — but they are *managed* files. `install_bundled`
overwrites them on every `init`, saving any edit as `<name>.toml.yours` and
warning on stderr, precisely so a shipped fix always lands. omh's own hooks go
further and are never written to disk at all, being generated from the manifest
at launch, *"the only arrangement in which omh can ship a fix to them"*.

That is deliberate and stacks must inherit it. A local edit that fixes Elixir on
one laptop leaves omh broken for every other Elixir user and removes the pressure
that would have produced a real fix. **A problem solved by changing a stack
definition should require a new version of omh**, so that solving it once solves
it for everyone.

What moving stacks out of the `const` buys is a lower barrier to *contributing*,
not to diverging. Somebody who writes Elixir opens a pull request adding
`stacks/elixir.toml`; they never touch Rust, omh's author never learns Elixir,
and the result reaches every user in a release. The contribution surface is the
repository, not `~/.omh`.

Local specifics that will never be upstreamed — a proprietary internal toolchain
— are served by §1.7, which records the answer in the *repo's* `.omh/`, scoped to
the project that needs it. Shared opinion stays versioned; one project's
peculiarity stays in that project.

So stacks become `stacks/*.toml`, embedded through the same `bundled::Shipped` +
`install_bundled` path adapters already use, with the same overwrite semantics:

```toml
name   = "rust"
marker = "Cargo.toml"

[[provide]]
name    = "toolchain"
needs   = ["cargo", "rustc"]
install = "curl -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal"
because = "cargo is how a rust project is built, tested and formatted"

  [[provide.measured]]
  what  = "on disk, minimal profile"
  value = "~600 MB"
  how   = "a default-profile install less its 904 MB of docs"
  on    = "2026-08-14"

[[provide]]
name    = "linker"
needs   = ["cc", "ld", "ar"]
install = "apt-get install -y --no-install-recommends gcc libc6-dev"
because = "rustc compiles without a linker and then cannot produce a binary"

  [[provide.measured]]
  what  = "on disk"
  value = "124 MB"
  how   = "dpkg installed-size of gcc, libc6-dev and binutils on arm64"
  on    = "2026-08-14"
```

`because` and `measured` are not decoration. A provide installs something on
somebody's machine without asking (§1.6), and the base set already settled what
that obliges: every entry states its case, and
`every_base_set_entry_states_its_case` fails the build when one does not. Stacks
inherit the rule *and the test* — otherwise §1.6's promise that `omh why <stack>`
reports what it cost has no file to read it out of.

Costs are **measured**, benefits are **argued**. The base set keeps those apart
because they are different kinds of claim, and so does this.

A few lines and a case, written by somebody who works in that ecosystem, reviewed
as a pull request, shipped to everybody in a release.

## 1.3 The stack layer

The mechanism exists. `image::harness_dockerfile` already builds a layer from a
command declared in adapter data:

```rust
format!("FROM {}\nUSER root\nRUN {}\n", base_tag(), adapter.install)
```

with `adapters/claude.toml` supplying `install = "npm install -g …"`.

A stack layer is the same shape, one layer further out — with one `RUN` per
provide that applied (§1.4), in file order:

```
node:22-bookworm-slim + graph + tools      base
  → RUN adapter.install                    harness
    → RUN <provide 1>.install              stack
      → RUN <provide 2>.install
```

Same root-then-drop-privilege discipline the harness layer already documents: an
image that ends privileged hands the agent the sandbox's own escape hatch.

Tagging keys on the harness, the detected stacks, **and which provides fired** —
so a rust repo and a node repo do not fight over one tag, a pnpm repo and a yarn
repo do not share one, and a polyglot repo composes every applicable layer.
Layer once, reuse across every session in that repo.

## 1.4 A stack provides several things, conditionally

One `install` per stack does not survive contact with a real ecosystem. A
TypeScript project needs a node runtime *and* a package manager, and those are
different commands — `corepack enable pnpm` is nothing like installing bun. Worse,
they are **mutually exclusive**: a repo using npm must not have pnpm installed on
the strength of being a node project.

So the unit is not the stack, it is each thing the stack provides:

```toml
name   = "node"
marker = "package.json"

[[provide]]
name  = "runtime"
needs = ["node", "npm"]
# no install: the base image already ships these. Stated anyway, so the
# assumption is checked rather than assumed, and a change to the base
# image is caught here rather than at somebody's turn one.

[[provide]]
name    = "pnpm"
needs   = ["pnpm"]
when    = "test -f pnpm-lock.yaml"
install = "corepack enable pnpm"

[[provide]]
name    = "yarn"
needs   = ["yarn"]
when    = "test -f yarn.lock"
install = "corepack enable yarn"

[[provide]]
name    = "bun"
needs   = ["bun"]
when    = "test -f bun.lock"
install = "npm install -g bun"
```

`when` is a shell predicate — exit zero means this provide applies — evaluated in
the sandbox rather than on the host. Both halves of that matter, and §1.4's last
subsections give the reasons.

Four properties follow, and each of them is a reason the shape is worth the
extra nesting:

**`needs` and `install` stay separate, per unit.** `install` is a recipe;
`needs` is the outcome. This is not theoretical: installing rustup in this
sandbox produced a working `cargo` and still could not link anything, because
the image had no `cc`, `ld` or `ar`. The recipe succeeded and the environment did
not work. Keeping them paired *per provide* makes the failure attributable — *"the
`linker` provide ran and `cc` still does not resolve"* — rather than a flat list
of names with no idea which recipe was supposed to deliver them.

**A provide with no `install` is an assertion.** `node` and `npm` come from the
base image. Declaring them anyway costs nothing and turns an unwritten assumption
into a checked one.

**`when` makes §1.5's derivation data rather than code.** "Read the lockfile to
know the package manager" stops being Rust that omh's author had to write and
becomes a line in a file contributed by somebody who works in that ecosystem —
which is the whole point of §1.2.

**Order is file order.** `corepack enable pnpm` needs node to exist first. An
ordered list expresses that without a dependency graph, and a graph would be a
second way to say what the order already says.

### Verification

After building the layer, omh **probes the sandbox for every `needs` entry of
every provide that fired** — in a hook's environment, not a login shell, because
that distinction is what made gap 3 invisible. A name that does not resolve is a
stack definition making a claim its recipe did not deliver, and is reported as
exactly that rather than as a mystery at turn one.

This is `omh doctor`'s territory as much as `init`'s, and it is the line CLAUDE.md
draws: a green suite proves omh built the right recipe, never that external
software did what it said.

### The tag has to key on what fired

A pnpm repo and a yarn repo are the same stack and **not** the same image. The
layer tag keys on the harness plus the set of provides that actually applied, or
one of them silently gets the other's package manager and the cache is worse than
useless.

### `when` is a shell predicate, and it runs in the sandbox

File existence is not enough, and the counter-example is the first one anybody
meets: `packageManager` in `package.json` is a **field**, and it is the corepack
standard. A predicate that can only ask *does this file exist* cannot read it.

The alternative to shell is a small query language — a path expression, a match
operator, a way to negate. That is a language, it grows every time somebody's
ecosystem does not fit, and a language inside a data file is how data files stop
being reviewable. omh already refused this once: hooks take a shell predicate,
and `hook::when` is documented as *"non-zero means this hook stays silent"*.

So `when` is shell, with the same exit-code meaning omh already uses:

```toml
[[provide]]
name    = "pnpm"
needs   = ["pnpm"]
when    = "test -f pnpm-lock.yaml || jq -e '(.packageManager // \"\") \
| startswith(\"pnpm\")' package.json"
install = "corepack enable pnpm"
```

Both witnesses, in the order §1.5 argues for: the lockfile first because it
records what was actually used, `packageManager` second because it records what
was declared. One line of shell says *"declared pnpm, or has a pnpm lockfile"*,
which no file-existence check could and no small query language should have to.

File existence alone stays expressible and stays the common case — `when = "test
-f pnpm-lock.yaml"` — without needing its own syntax.

**Predicates run in the sandbox, against the repo mounted read-only.** This is
the whole reason the question matters, and it is not a detail. `install` already
runs arbitrary shell, but it runs *inside a container as root*, which is
contained by construction. A predicate evaluated on the **host** would mean a
stack definition executing shell on somebody's laptop during `init` — the one
thing omh exists to avoid, reintroduced by the file that is supposed to describe
an environment.

Nothing new is needed to avoid it. `base::index_args` already mounts the repo
read-only into the image to run the graph indexer, for a reason that applies here
verbatim: *"Read-only: indexing reads code, and an indexer that can write into the
checkout is a sandbox hole for no benefit."* Predicates get the same treatment,
in the base-plus-harness image that has to be built anyway, and they may rely on
what that image ships — `jq`, `git`, `ripgrep`.

They report through the same protocol as the toolchain probe, so there is one
wire format and one parser.

### Order at `init`

1. **markers**, on the host — file existence, cheap, decides whether to bother
2. **build base + harness** — needed regardless
3. **evaluate every `when`** in that image, repo read-only → which provides fire
4. **record the result** in the repo's `[provision]` (§1.9), so no launch repeats
   steps 2–3
5. **build the stack layer** from those, in file order
6. **probe** every `needs` of every provide that fired → verify, and cache
   against the image tag

A launch still repeats step 1 — markers are a filename check on the host, cost
nothing, and are how drift is noticed (§1.9). What it never repeats is steps 2
and 3, which are the ones that need a container.

`marker` deliberately stays a plain filename. It is the coarse first cut — *is
this ecosystem here at all* — and making it executable would mean building an
image before knowing whether any stack applies. `when` is the fine-grained
question, asked only once a stack is already in play, and by then the image
exists.

**Two** container runs per `init`, not one, and they cannot be merged: predicates
must run before the layer is built to decide what goes in it, and `needs` must
run after to check what came out. Both are cheap against building the layer they
bracket, and a launch pays neither.

### A predicate that cannot answer does not fire

*Cannot tell* is never a licence to act — but a shell command reports one exit
code, and "false" and "broken" both come back non-zero. A two-valued mechanism
cannot express a three-valued rule, so the exit code has to be read more finely:

```
0    applies
1    does not apply
>1   could not answer  → reported, with the code, and does not fire
```

This is not invented. It is what the commands predicates will actually be
written with already do: `grep` answers 0 match, 1 no match, 2 error; `jq -e`
answers 1 for a false-or-null result and higher for its errors; `test` answers 1
for false and 2 for misuse.

It is a **convention rather than a guarantee**, and that is affordable here for
a reason specific to this design: stack files ship with omh and are reviewed
(§1.2), so the convention is enforceable where it is written, and a test can
assert every shipped predicate honours it. No arbitrary local predicate is in
play. The same does not hold for a repo-local stack file (§1.7), where a
misclassified error is the author's own to find.

Getting it wrong stays loud rather than silent: an error read as "does not
apply" skips the provide, and suppression then drops the hook that needed it, by
name.

The consequence is caught rather than absorbed. A provide that fired but did not
deliver is caught by step 6. A provide that wrongly did *not* fire is invisible to
step 6 — nothing it declared is in any `needs` list — and is caught instead by
suppression: the hook needing `pnpm` is dropped by name because `pnpm` does not
resolve. Two different mechanisms for two different failures, and neither is
silent.

## 1.5 Deriving what to provision

One marker covers projects using npm, pnpm, yarn and bun. Which one is a property
of the **project**, not of the developer, and the project already records it — in
`packageManager`, in a lockfile, or both. That is what a provide's `when` reads
(§1.4). Two developers using different package managers on one repo already have
fighting lockfiles; that is a broken project, not a case to model. Asking
somebody to retype what their repo already states is the *"hassle we promised to
remove"* that `detect.rs` opens by naming.

Where the two disagree, the lockfile is the better witness: it records what was
actually used, `packageManager` records what was declared. A stack file can say
so, because a predicate can express *"declared pnpm, or has a pnpm lockfile"* in
one line of shell.

The same reading answers two questions. Which provides fire — `pnpm` has to be
*in the image* — is settled declaratively by `when`. How the commands are spelled
is settled here, and the result is written as a repo hook (§2.2), because it is a
fact about one project.

**Runners before languages.** A large share of repos declare how to test
themselves in a language-neutral way — `Makefile`, `justfile`, `Taskfile.yml`,
`package.json` scripts. `make test` is the test command for a C, Haskell, Ruby or
Rust project alike. Detecting *runners* covers many ecosystems with one
definition each and needs no language knowledge at all.

For node, each command comes from `scripts`, and what is derived is written as a
hook file in `<repo>/.omh/hooks/` (§2.2) — a fact about one project, stored with
that project. A script that is not declared produces **no hook**, rather than one
that fails on every turn.

CI config (`.github/workflows/*.yml`) is the most authoritative statement a repo
makes about how it tests itself, but matrices, composite actions and container
steps make it unreliable to read. Use it to **suggest** an answer to §1.7. Never
to decide silently.

## 1.6 Cost, stated

Provisioning is not free and the number belongs in front of the user, the way
the base set already requires of every entry it installs.

Measured in this sandbox, on `aarch64`:

| | |
|---|---|
| `~/.rustup`, default profile | 1.5 G — **of which 904 M is `share/doc` alone** |
| `~/.cargo` | 75 M |
| `gcc` + `libc6-dev` + binutils | 124 M |
| minimal profile + rustfmt + clippy + C toolchain | **~700 M, estimated** |

The 904 M is the whole argument for `--profile minimal`: most of the default
install is documentation nobody reads inside a sandbox. Stack definitions should
install minimally and say what it cost.

`omh why <stack>` explains it from the same file, exactly as it does for base-set
entries — what it buys, what it costs, how to remove it.

**This should not be a question.** "Do you want the tools your project needs?"
has one answer. Provision, state the cost, make it removable — the base set's
existing contract. The first `init` on a rust repo will take minutes and cost
several hundred megabytes, and that is the price of the environment working; it
must be said out loud rather than discovered.

## 1.7 The two questions of last resort

A question fires only when omh does not know, and there are exactly two things it
can fail to know. They have different answers and different homes, and running
them together is what makes a wizard out of a prompt:

| | Asked when | Answer | Written to |
|---|---|---|---|
| **How is this installed?** | a marker matched no stack definition, or none matched at all | an install recipe | `<repo>/.omh/stacks/<name>.toml` |
| **What command tests this?** | a stack is known but §1.5 derived no command | a command | `<repo>/.omh/hooks/<name>.json` |

Both are asked. Refusing to ask the first — telling somebody to contribute a
stack definition upstream and wait for a release — leaves them unable to work
today, and a tool that answers *"not yet"* to the only question standing between
you and a working sandbox does not get used. §1.2 already anticipated this: local
specifics that will never be upstreamed are served here.

It belongs in `init` rather than `doctor` because `init` is the command you know
gets run; `doctor` may never be. `init` builds the image and probes it, then asks
with the measurement in hand — which is not guessing from the host.

When a question is declined, or there is no terminal, the outcome is **nothing
written, and a sentence saying where to write it** — never a guess.

### A repo-local stack adds; it never shadows

The recorded recipe is a stack file in the same format §1.2 defines, living in
the repo rather than in the catalogue. That is not a hole in §1.2's rule, and the
distinction is exact:

- **Defining a stack omh does not ship** forks nothing. There is no shared
  opinion about Elixir to diverge from, and a proprietary internal toolchain will
  never have one.
- **Patching a stack omh does ship** forks the shared opinion, leaves everybody
  else broken, and removes the pressure for the real fix. That is what §1.2
  refuses, and `install_bundled` enforces by overwriting.

So a repo-local stack file may only *add*. A name that omh already ships is **an
error naming both**, not a silent override — the rule `Own::reserved` already
applies to hooks, for the same reason and with the same wording.

### Every answer is asked once too often

After recording either answer, omh says where it could go:

```
  recorded  .omh/stacks/elixir.toml
            if this is not private, contributing it upstream means nobody
            answers this question again
```

That is the whole reconciliation with §1.2. A local answer unblocks somebody
today; the line after it is how the answer stops being local. Encouragement,
never a refusal — a refusal was the version of this section that got deleted.

### What makes a question clever

1. **It only fires on a real gap.** Zero gaps → zero questions, which is most
   repos most of the time. That is the difference between this and a wizard.
2. **It carries its evidence.** Not *"what is your test command?"* but *"`mix.exs`
   is here and omh ships no stack that claims it — nothing was installed for
   this project."*
3. **Options are actions, not concepts.**
4. **Enter is right**, so holding it through a first run never produces
   something broken.
5. **Asked once, ever**, persisted where teammates inherit it.
6. **Actionable** — omh does something with it rather than recording a taste.

## 1.8 Suppression is derived, not configured — `[toolchain]` goes

A first version of this shipped a `[toolchain]` table where somebody answered
`skip` or `assume` per program, and `init` asked. Provisioning removes the
question it was answering, and every case it covered now has a better home:

| Case | Answer |
|---|---|
| the sandbox lacks the tool | provisioning supplies it — not a decision anybody should be asked to make |
| provisioning failed | a **bug**. `skip` hides it, and a local silencer removes the pressure for the fix everyone else also needs (§1.2) |
| no stack definition exists | §1.7 asks. Answered, the tool gets installed; declined, nothing was written to suppress |
| I maintain my own image | the probe runs against *that* image and finds the tool. Nothing to assume |
| I do not want this hook | `[use]` and `omh unuse` already do this, by name |

`assume` was only meaningful while the probe was a *prediction* about some future
sandbox. It now runs against the image the session will actually use, so there is
nothing left to assume — the probe knows. And `skip` turns out to be a second way
to switch off a hook, keyed on a program name, which is a strange axis to
disable hooks on when `unuse` exists and says exactly what it means.

But something must still stop `cargo test` going red at the end of every turn
when cargo genuinely is not there. That was the original complaint, and it is
real. **The answer is that suppression is derived from measurement rather than
configured.**

A probe already runs, to verify `needs` (§1.4). But `needs` is not enough on its
own, and the gap is easy to miss: it lists what a **stack** declared, while
suppression has to cover every program a **hook** names. A hand-written or
imported hook running `shellcheck` appears in no `needs` list anywhere, so a
probe built only from `needs` is blind to exactly the hooks omh did not write.

So one probe, over the **union**:

- every `needs` of every provide that fired → read by verification
- every program named by a hook that will render → read by suppression

One container run after the layer is built, one program list, two readings.
Cached as `image tag → {program: resolves}` (§1.9), which is the right key
because it is a fact about an image.

A hook added later can name a program the cache has never seen. Then omh probes
**just the unseen ones**, once, and the cache grows. That costs a container only
when the hook set changes — never on an ordinary launch, and never in proportion
to how often you work.

With that, the rule is:

> A hook whose command names a program that does not resolve in the sandbox is
> dropped for that session, by name, with the reason — automatically, from the
> probe, with no answer asked for and none recorded.

("No answer recorded" — the probe *result* is cached against the image tag, per
§1.9. The distinction is the point: a cache is re-derived when the thing it
describes changes, and an answer is not.)

This is strictly better than the setting it replaces:

- **It self-heals.** Install the toolchain, or fix the stack definition, and the
  hook comes back on the next launch. There is no stale answer to remember and
  no file to go and edit.
- **A provisioning bug cannot be permanently hidden.** The report fires every
  launch until the environment is actually fixed, which is what makes it get
  fixed.
- **It cannot be wrong about the current machine**, because it *is* the current
  machine. A recorded answer is a claim about a sandbox that may since have
  changed.
- **It removes a question, a table, and a migration.** `init` stops asking what
  to do about a missing tool. It still asks the questions in §1.7 — how to
  install an ecosystem omh does not ship, and what command tests this project —
  but those are questions omh cannot answer for itself, not questions about a
  measurement it has already taken.

What survives from the built version, unchanged:

- **Hook files are written either way** — scoped, since §2.2 splits where they
  live. A *derived or answered* hook is written into `<repo>/.omh/hooks/`
  whatever the sandbox turned out to hold: that file is the repo's statement
  about itself, committed and identical for everybody who clones, and letting one
  computer's toolchain decide whether it exists lets whoever ran `init` first
  impose their machine on the team — permanently, since `write_if_absent` never
  revisits. A *conventional* hook needs no such rule: it ships with omh and
  exists unconditionally, so no toolchain state could have stopped it.
- **Keyed by program**, never by stack or by hook: `go test ./...` needs `go`
  while `gofmt -w .` needs `gofmt`, so one stack can be half-served.
- **Suppression happens in `render`, on both hook paths.** A rule applied on one
  harness and not the other is not a rule about the repo.
- **Dropped hooks are reported** through `hook::Dropped`, the channel a hook a
  harness cannot spell already uses. A hook that vanishes without a word is not
  an improvement on one that fails loudly.

Only the *input* changes: the probe result rather than a table somebody filled in.

### The one setting that is still worth having

Not `skip` on a program — an opt-out from a **provide**, for cost or policy,
written in the same `[provision]` table §1.9 defines:

```toml
[provision]
"rust/linker" = false   # I supply my own; do not put it in the image
```

That is a different claim from `skip`. It changes what goes into the image and
changes nothing about the truth: the probe still runs, and if `cc` is absent omh
still says so and still drops the hooks that need it. **Nobody can use this to
tell omh something false**, which is exactly what `assume` allowed.

Layered like every other setting, so *"not on this laptop"* lives in
`settings.local.toml` and never reaches the team.

### The case that would need an escape hatch, and does not have one yet

A tool that arrives at **launch** rather than at build — mounted in, or installed
by a session-start hook — is present at run time and absent when the probe looked.
omh would drop its hooks wrongly.

This is exotic, and speculative configuration is how config surfaces grow, so
nothing is built for it. If it turns up, the escape is a per-repo list of
programs to treat as present regardless — which is `assume` under a name that
admits what it is, and it should be added when somebody actually needs it and not
before.

### Consequence for what is built

The probe, `detect::program`, and `render::suppressed_by_toolchain` survive with
a different input. `settings::Toolchain`, the `[toolchain]` table, the `init`
question and the answer-recording that writes it are **deleted** — roughly half
of what shipped this cycle, removed because provisioning made the question it
answered the wrong question.

## 1.9 The resolution is written down, and launch reads it

Stack definitions ship with omh (§1.2). What a *particular repo* resolves to —
which stacks apply, which provides fired — is written into that repo's
`.omh/settings.toml`:

```toml
[provision]
"rust/toolchain" = true
"rust/linker"    = true
"node/pnpm"      = false   # yours: I supply pnpm myself
```

**Because otherwise every launch re-evaluates `when`.** Predicates are shell and
run in a container against the repo (§1.4); paying for that once per `omh init` is
nothing, paying for it on every session start is a container run standing between
somebody and their agent.

The resolution belongs in the repo's committed settings rather than in a cache
directory for three reasons that are not about speed:

- **It is a fact about the project**, like the lockfile it was derived from.
  Every teammate should get the same environment, not each re-derive one and
  possibly differ.
- **It is reviewable.** What omh concluded about your repo is a line you can
  read, in a file you already read, rather than state in a directory you do not
  know about.
- **It is the override.** A wrong resolution is corrected by editing it — the
  same table, the same shape, `false` where omh put `true`. That is why the
  opt-out in §1.8 and this cache are one table and not two: they are the same
  claim, written by different authors.

### Two caches, two keys

| What | Cached as | Valid while |
|---|---|---|
| which provides apply | `[provision]` in the repo's settings | the repo's manifests have not changed |
| what actually resolves in the image (`needs`) | against the image tag | that image is the one being run |

`needs` is a fact about an **image**, not about a repo, so it belongs with the
image and not in settings — and `image::recipe_digest` already exists to pin
exactly this kind of claim. Between them, **a launch runs no container to decide
either question.**

### Re-resolution, and the one edit that must survive it

`omh init` re-resolves and rewrites. Launch reads and does not. That is the same
split the rest of this page uses: `init` is the command that looks at the repo
and decides, launch is the command that uses what was decided.

One rule protects the human: **re-resolution may add entries and may flip an
entry to `true`, but must never clear a `false`.** Only a person writes `false` —
omh writing one would mean it had decided against something it detected — so a
`false` is a decision, and re-running `init` is not consent to discard it.

### Drift, stated

A resolution recorded in April and a `yarn.lock` committed in May disagree, and
nothing in a launch re-reads the repo to notice.

The launcher can check the cheap half on the host: the set of markers present
against the set recorded. A `package.json` that appeared since is caught, and
omh says so. What it cannot catch without re-running predicates is a change
*within* a stack — swapping pnpm for yarn leaves `package.json` exactly where it
was.

That case needs `omh init` re-run, and this is the cost of not paying a container
per launch. It is worth naming rather than hiding — and worth being exact about
which mechanism catches it. **Not `needs` verification**: yarn's provide never
fired, so `yarn` appears in no `needs` list and there is nothing there to fail.
What catches it is suppression — the hook runs `yarn test`, `yarn` does not
resolve, and the hook is dropped by name with the reason. So the failure is
visible, but the *fix* is a command somebody has to know to run.

---

# Part 2 — Hooks

## 2.1 Hooks are not the stack

A stack says what must be installed. A hook says what runs, and when. They are
independent, and a hook is not part of a stack definition — conflating them
produced the original wrong fix, treating a missing compiler as a hook to
suppress.

## 2.2 A hook command has exactly two homes

Both already exist, and `render::merge_hooks` already unions them with the repo
winning:

| | Holds | Reaches a teammate by |
|---|---|---|
| `~/.omh/hooks/` | hooks you want in every repo, and the ones omh ships | installing the same omh |
| `<repo>/.omh/hooks/` | hooks belonging to this project | being committed |

That layering answers the convention-versus-fact question without a new concept:

- **A conventional command is a shipped catalogue hook.** `cargo test` is true of
  Rust everywhere, so it is a hook file omh ships into `~/.omh/hooks/`, managed
  and refreshed like every other shipped file, and a repo turns it on through
  `[use]` when the marker is present. A fix to it ships with a version of omh
  and reaches everyone — the same rule §1.2 applies to stacks.
- **A derived or answered command is a repo hook.** `pnpm test` is a fact about
  one project, read from its lockfile and `scripts`, so `init` writes it into
  `<repo>/.omh/hooks/` where it is committed and travels with the project that
  is true of.

Neither path needs the stack to know a command, and a repo that disagrees with
omh's convention simply shadows it — which is what the union rule is for.

## 2.3 The arity is currently two, and hardcoded

`detect::Stack` has exactly two command fields, and five places depend on it: the
struct, `KNOWN`, `stack_hooks` returning `[StackHook; 2]`,
`notice::stack_hook_names` returning `[String; 2]`, and the curation test
asserting both are non-empty.

That is this repo's shape imposed on everybody else's. A TypeScript project may
want `tsc --noEmit` at turn end; a Python one `mypy`; a repo with codegen may
want to regenerate after edits.

Note that **per-project hooks are already unlimited**: `.omh/hooks/*.json` is a
directory anyone writes into, any moment, any command, and several hooks sharing
a moment already compose in a stable order. The limit is only in what omh can
*seed*, never in what a project can run.

---

# Part 3 — `omh import`

## 3.1 The adoption cliff

Somebody adopting omh already has an agent configured — hooks, rules, MCP
servers, skills, commands, subagents. They will not trade a working setup for
omh's opinion. So `init` meeting a configured repo decides whether omh can be
adopted at all, and it is currently the weakest path there is.

There are six capabilities: Rules, Skills, Mcp, Commands, Subagents, Hooks.
**Exactly one can be imported.** `Capability::import` is declared only under
`[capabilities.mcp]` in both shipped adapters, and `config::mcp_import` is its
only reader. Everything else has to be re-authored by hand.

The pattern exists and is tested — `shipped_adapters_declare_where_to_import_mcp_from`
requires every adapter to say where to look. It just stops at one capability.

## 3.2 Hooks invert from tables that already exist

The adapter already declares, as data, how omh's vocabulary maps onto a
harness's:

```toml
[capabilities.hooks.events]
turn-end    = "Stop"
before-tool = "PreToolUse"
after-tool  = "PostToolUse"

[tools]
edit  = "Edit|Write|MultiEdit"
shell = "Bash"
```

omh uses this to **write** the harness's format. It inverts. An existing
`{"matcher": "Bash", "command": "…"}` on `PreToolUse` becomes
`{"on":"before-tool","tools":["shell"],"run":"…"}`, from data already shipped and
already tested — and it works for any adapter declaring the same tables, with no
second implementation.

## 3.3 What does not invert

- **`inject` and `refuse`** render to jq one-liners built from adapter templates.
  Recognising an arbitrary user's jq blob as "this was an inject" is parsing, not
  inversion. Plain `run` hooks — the overwhelming majority — are trivial.
- **Matchers outside omh's vocabulary.** omh knows `edit`, `read`, `shell`,
  `search`. A matcher of `"Read|Grep"` is not one of them.
- **Harnesses whose hooks are not a config file.** opencode renders hooks to a
  TypeScript plugin. Reading a generated program back into declarations is not
  realistic. `Capability::import` being `Option` already expresses this: absent
  means this capability cannot be imported from this harness. Feasibility is per
  capability *and* per harness, and the adapter is where that is said.

The residue must be **reported by name, never silently dropped** — the rule
`hook::Dropped` already embodies. An import that quietly loses two of somebody's
six hooks is worse than one that refuses them loudly.

## 3.4 Copy, never move

omh generates the sandbox's config from the catalogue; the host's
`.claude/settings.json` is not consulted inside the sandbox. So import copies and
leaves the original untouched. Their existing setup keeps working if they run the
harness directly, and nothing is at risk if omh turns out not to be for them.

A tool that asks you to dismantle your working setup before you can evaluate it
does not get evaluated.

## 3.5 `init` notices; `import` acts

- **`omh init`** reports what it found: *"6 hooks in `.claude/settings.json` —
  `omh import` brings them in."* `init` is the touchpoint you know happens.
- **`omh import [harness]`** does it, is re-runnable, and reports both what it
  took and what it could not express.

Not silently inside `init`. Importing somebody's hooks is a real change to their
repo and should be a decision, not a side effect of setup.

---

# Part 4 — Known limitations

- **The probe checks launchers.** It asks *is this program installed*. For
  `cargo test` that is the right question. For `npm run format` it is not: `npm`
  is present in the base image, so the probe passes while `npm run format` fails
  on every edit unless a `format` script happens to exist. Fixed by §1.5, not by
  a better probe — deriving the command from `scripts` makes it right in the
  first place, and probing a launcher can never tell whether the script behind it
  exists.

As it ships today — the `Question fires?` column describes the `[toolchain]`
prompt §1.8 removes, and is here because it is what shows the weakness:

| Detected | Probes for | In the base image? | Question fires? | Meaningful? |
|---|---|---|---|---|
| rust | `cargo` | no | yes | yes |
| go | `go`, `gofmt` | no | yes | yes |
| python | `pytest`, `ruff` | no | yes | yes |
| **node** | `npm` | **yes** | **no** | **no** |

- **Selection resync.** `[use]` is written once, after the hooks. A hook that
  appears later is not added to the list, so it is off, and `omh use --all` is
  the fix. The launcher reports entries that are off for this reason, so it
  surfaces rather than failing silently, but it is not obvious.
- **omh's own suite cannot pass in an omh sandbox.** Four tests digest the image
  recipe with `git hash-object`, and git does not work in the sandbox by design.
  They are `#[ignore]`d with the reason recorded; CI runs them on the host. A
  turn-end `cargo test` hook in this repo is therefore red for a reason omh
  itself imposes.

---

# Part 5 — Testing

From [CONTRIBUTING](../../.github/CONTRIBUTING.md) and the project rules, restated
because this work has already produced several tests that were wrong in ways a
green suite would not have shown — one that failed against correct code, one
whose escape clause would have hidden a regression, and one asserting a fact
about somebody else's container image:

- **Watch every test fail first**, then implement. A guard written after its bug
  was fixed is tuned to a mutation you chose.
- **Confirm the guard by mutation.** Reintroduce a plausible wrong
  implementation and check exactly the intended test goes red.
- **Assert invariants, not shapes.** *"every hook init wrote is named in the
  selection"* survives refactoring; `contains("rust-test")` asserts that the
  harness image ships a rust toolchain, which is a fact about somebody else's
  software.
- **Iterate the whole set.** Guards run over every known stack, never this repo's
  own — a rust-shaped implementation passes a rust-only guard.
- **Execute probes, do not pattern-match them.** A probe is a program. The
  toolchain probe is POSIX `sh` and needs no container, so tests run it. One
  injection test using `contains` failed against *correct* code because the probe
  echoes the hostile name back as data; only a line-wise assertion tells
  execution from an echo.
- **Green proves construction, never behaviour.** A passing suite shows omh
  builds the right image recipe and the right command. That the toolchain landed
  and works needs `omh doctor`, and `needs` (§1.4) is what makes that checkable.

# Build order

1. **Stack definitions as data — `marker` plus conditional `[[provide]]`
   entries** — the stack image layer, tagged on the provides that fired, and the
   resolution recorded in `[provision]` so launch re-evaluates nothing (§1.9).
   This is the environment working, which everything else is subordinate to.
2. **One probe over the union** of every fired provide's `needs` and every
   program a hook names, in a hook's environment, cached against the image tag —
   feeding verification and suppression from one run (§1.8). Mostly built: the
   probe exists, gains a second consumer, and loses its dependence on a recorded
   answer.
3. **Delete `[toolchain]`** — the table, `settings::Toolchain`, the `init`
   question and the answer-recording. Follows 2, because 2 is what makes it
   unnecessary, and doing it in the other order leaves a window where nothing
   stops a broken hook (§1.8).
4. **Import: hooks.** The translation table is ready to invert, and this decides
   whether a configured repo can adopt omh at all.
5. **Derive what to provision and how commands are spelled** — package manager,
   then runners, then `package.json` scripts.
6. **The two questions of last resort** (§1.7) — how to install an unencoded
   ecosystem, and what command tests a project omh could derive nothing from,
   with repo-local stack files that add and never shadow.
7. **Decouple hooks from stacks** — conventional hooks become shipped catalogue
   hook files a repo turns on through `[use]`; the two-slot arity goes with them.
8. **Import: the remaining capabilities** — rules, skills, commands, subagents.

1 and 4 are independent and can proceed in parallel. 2 follows 1 immediately — a
layer whose outcome is never verified is a claim, not an environment — and 3
follows 2 for the reason given above.
