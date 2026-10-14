---
title: Stacks
description: What a stack file holds — the marker that names an ecosystem, what it provides, and the condition that decides whether a provide applies.
---

A stack tells omh what has to be installed in the sandbox before anyone can
work on one kind of project. It is a TOML file. omh ships four — `go`, `node`,
`python` and `rust` — and a repo can add its own.

You rarely write one. omh works out which stacks a repo is from the files it
holds, and records what it installed in
[`[provision]`](/reference/configuration/#provision--what-your-sandbox-is-built-with).
You read this page to correct what a stack does, or to teach omh an ecosystem
it does not know.

| I want to… | Do this |
|---|---|
| leave one provide out of my sandbox | set it to `false` in [`[provision]`](/reference/configuration/#provision--what-your-sandbox-is-built-with) |
| fix a stack omh ships | open a pull request — a local edit is saved aside and not read ([below](#where-they-are)) |
| teach omh an ecosystem it has no stack for | answer `omh init`'s question ([the two things omh will ask](/reference/configuration/#the-two-things-omh-will-ask)), or write `<repo>/.omh/stacks/<name>.toml` |
| find out why a hook says a program is missing | [When a provide is wrong](#when-a-provide-is-wrong) |

<details>
<summary>Why a stack is not a set of hooks</summary>

A hook is automation: what runs, on which event. A stack is the environment
the hook runs in, and a sandbox with no compiler is broken for you as well as
for the agent, whatever its hooks do.

</details>

## Where they are

A stack is read from one of two directories.

```
~/.omh/stacks/<name>.toml           the ones omh ships
<repo>/.omh/stacks/<name>.toml      committed: one this repo taught omh
```

**The shipped ones are omh's, and it replaces them.** They are refreshed from
the binary, so that a fix to one reaches everybody in a release. An edit you
made is not thrown away — it is saved beside the file as `<name>.toml.yours`,
and omh says so — but it is not read either. A stack that is wrong is a pull
request, not a local patch: fixing Elixir on one laptop leaves it broken for
every other Elixir user.

**A repo's own adds; it never shadows.** A file in `<repo>/.omh/stacks/` whose
`name` is one omh ships is an error naming both paths, not an override.

**`omh init` writes a repo's own stack for you** when it meets an ecosystem it
has never been taught — see
[the two things omh will ask](/reference/configuration/#the-two-things-omh-will-ask).

<details>
<summary>Why a repo cannot redefine a stack omh ships</summary>

A stack decides what goes into the image an agent runs in, so a repo that could
redefine `rust` could point its `install` at anything, and the only symptom
would be a sandbox that built successfully.

</details>

## The file

A stack names an ecosystem, the file that marks a repo as one, and one
`[[provide]]` for each thing it puts in the image.

```toml
name   = "<stack>"
marker = "<one filename>"

[[provide]]
name    = "<provide>"
needs   = ["<program>"]
when    = "<shell condition>"      # optional
install = "<shell>"                # optional
because = "<the case for it, in a sentence>"

  [[provide.measured]]             # optional, any number
  what  = "…"
  value = "…"
  how   = "…"
  on    = "YYYY-MM-DD"
```

| field | required | |
|---|---|---|
| `name` | yes | what the stack is called. It is the first half of every [`[provision]`](/reference/configuration/#provision--what-your-sandbox-is-built-with) key |
| `marker` | yes | one filename inside the repo. Its presence says the repo is one of these. Checked on the host, at every launch, so it stays a filename and nothing cleverer |
| `[[provide]]` | | one thing the stack puts in the image. A stack has as many as it needs |

And in each provide:

| field | required | |
|---|---|---|
| `name` | yes | the second half of the key: `node/pnpm`. No `/`, and no two alike in one stack |
| `needs` | yes | the programs that must resolve on `PATH` once this has run. Never empty, and program names only — it is the outcome omh checks, not a command it runs |
| `when` | no | a shell condition deciding whether this provide applies to this repo. Without one, it always does. See [`when`](#when) |
| `install` | no | the shell that puts it in the image. Without one, the provide is an assertion: *the base image already has this*, stated so that it is checked instead of assumed |
| `because` | yes | the case for it, in a sentence |
| `[[provide.measured]]` | no, any number | what it costs: `what`, `value`, `how` it was measured and `on` which day |

This is the `node` stack omh ships, without its comments:

```toml
name   = "node"
marker = "package.json"

[[provide]]
name    = "runtime"
needs   = ["node", "npm"]
because = "the base image already ships these; declaring them is how a change to it gets noticed"

[[provide]]
name    = "pnpm"
needs   = ["pnpm"]
when    = "test -f pnpm-lock.yaml || jq -e '(.packageManager // \"\") | startswith(\"pnpm\")' package.json >/dev/null"
install = "corepack enable pnpm"
because = "a repo whose lockfile is pnpm's cannot be installed or tested with anything else"

[[provide]]
name    = "yarn"
needs   = ["yarn"]
when    = "test -f yarn.lock || jq -e '(.packageManager // \"\") | startswith(\"yarn\")' package.json >/dev/null"
install = "corepack enable yarn"
because = "a repo whose lockfile is yarn's cannot be installed or tested with anything else"

[[provide]]
name    = "bun"
needs   = ["bun"]
when    = "test -f bun.lock || test -f bun.lockb || jq -e '(.packageManager // \"\") | startswith(\"bun\")' package.json >/dev/null"
install = "npm install -g bun"
because = "a repo whose lockfile is bun's cannot be installed or tested with anything else"
```

**A field omh does not know is refused**, by name.

**Order is file order.** `corepack enable pnpm` needs node first, and a list
says so without a dependency graph.

**`needs` and `install` are separate on purpose.** `install` is a recipe and
`needs` is the outcome, so a failure can be pinned on one provide: the `linker`
provide ran and `cc` still does not resolve.

<details>
<summary>Where the split between <code>needs</code> and <code>install</code> came from</summary>

The two came apart on the first stack omh had: installing rustup produced a
working `cargo` that could not link anything, because the image had no `cc`.
Paired per provide, the failure can be pinned on one.

</details>

## `when`

A provide's `when` is shell, and its exit code is read three ways:

| exit | |
|---|---|
| `0` | the provide applies |
| `1` | it does not |
| anything else | the condition could not answer. `omh init` reports the provide by name with the code, and does not apply it |

**It runs in the sandbox, with the repo mounted read-only.** Never on your
machine. It runs in the image omh has built by then, before the stack's own
layer exists, and it can read the repo and cannot write to it.

**What applied is written to `[provision]`** in the repo's committed settings,
and nothing asks again —
[Configuration](/reference/configuration/#provision--what-your-sandbox-is-built-with)
has what that table is and how to correct it.

<details>
<summary>Why three answers rather than two, why shell, and why never on the host</summary>

**Two values would not do.** *False* and *broken* both come back non-zero, and
a condition that could not run is not a reason to act either way. The split is
the one the commands a condition is written with already make: `test` exits 1
for false and 2 for misuse, `grep` 1 for no match and 2 for an error, `jq -e` 1
for a false or null result and higher for its own failures.

**It is shell rather than a list of filenames** because the first thing a real
ecosystem needs is to read a field: `packageManager` in `package.json` is how a
repo declares pnpm, and no "does this file exist" can see it.

**It never runs on your machine** because a stack file that ran shell on the
host during `init` would be the one thing omh exists to avoid, brought back by
the file that describes an environment.

</details>

## When a provide is wrong

Two different mistakes, caught by two different things.

| The mistake | What catches it |
|---|---|
| it applied and **did not deliver** — the recipe ran and the program is not there | measurement: omh asks the built image whether each of its `needs` resolves |
| it **should have applied and did not** | the hook that needs the program, [held back by name](/reference/configuration/#hooks-your-sandbox-cannot-run) |

The second is invisible to measurement, because nothing it would have
installed is in any `needs` list that was measured. What catches it is the
hook: the one that runs `pnpm test` is held back because `pnpm` does not
resolve.

## Next

<div class="cards">

- [`[provision]`](/reference/configuration/#provision--what-your-sandbox-is-built-with)\
  The table a stack's answers are written to, and how to correct it.
- [Hooks your sandbox cannot run](/reference/configuration/#hooks-your-sandbox-cannot-run)\
  What omh does when a hook needs a program the image lacks.
- [Architecture](/concepts/architecture/)\
  Images, runtime backends, and what lives where on disk.

</div>
