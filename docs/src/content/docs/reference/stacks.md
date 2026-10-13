---
title: Stacks
description: What a stack file holds — the marker that names an ecosystem, what it provides, and the condition that decides whether a provide applies.
---

A stack answers one question about a kind of project: *what has to be installed
before anyone can work on it?* It is a TOML file. omh ships four — `go`, `node`,
`python` and `rust` — and a repo can add its own.

It is not a set of hooks. A hook is automation: what runs, on which event. A
stack is the environment the hook runs in, and a sandbox with no compiler is
broken for you as well as for the agent, whatever its hooks do.

## Where they are

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
`name` is one omh ships is an error naming both paths, not an override. A stack
decides what goes into the image an agent runs in, so a repo that could redefine
`rust` could point its `install` at anything, and the only symptom would be a
sandbox that built successfully.

`omh init` writes a repo's own stack for you when it meets an ecosystem it has
never been taught — see
[the two things omh will ask](/reference/configuration/#the-two-things-omh-will-ask).

## The file

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

| field | |
|---|---|
| `name` | what the stack is called. It is the first half of every [`[provision]`](/reference/configuration/#provision--what-your-sandbox-is-built-with) key |
| `marker` | one filename inside the repo. Its presence says the repo is one of these. Checked on the host, at every launch, so it stays a filename and nothing cleverer |
| `[[provide]]` | one thing the stack puts in the image. A stack has as many as it needs |

And in each provide:

| field | |
|---|---|
| `name` | the second half of the key: `node/pnpm`. No `/`, and no two alike in one stack |
| `needs` | the programs that must resolve on `PATH` once this has run. Never empty, and program names only — it is the outcome omh checks, not a command it runs |
| `when` | optional. A shell condition deciding whether this provide applies to this repo. Without one, it always does |
| `install` | optional. The shell that puts it in the image. Without one, the provide is an assertion: *the base image already has this*, stated so that it is checked instead of assumed |
| `because` | required. The case for it, in a sentence |
| `[[provide.measured]]` | optional, any number. What it costs: `what`, `value`, `how` it was measured and `on` which day |

A field omh does not know is refused, by name.

**`needs` and `install` are separate on purpose.** `install` is a recipe and
`needs` is the outcome, and the two came apart on the first stack omh had:
installing rustup produced a working `cargo` that could not link anything,
because the image had no `cc`. Paired per provide, the failure can be pinned on
one: the `linker` provide ran and `cc` still does not resolve.

**Order is file order.** `corepack enable pnpm` needs node first, and a list
says so without a dependency graph.

## `when`

A provide's `when` is shell, and its exit code is read three ways:

| exit | |
|---|---|
| `0` | the provide applies |
| `1` | it does not |
| anything else | the condition could not answer. The provide is reported by name with the code, and nothing changes for it: one that was not recorded is not applied, and one that was is left as it was |

Two values would not do. *False* and *broken* both come back non-zero, and a
condition that could not run is not a reason to act either way. The split is
the one the commands a condition is written with already make: `test` exits 1
for false and 2 for misuse, `grep` 1 for no match and 2 for an error, `jq -e` 1
for a false or null result and higher for its own failures.

**It runs in the sandbox, with the repo mounted read-only.** Never on your
machine: a stack file that ran shell on the host during `init` would be the one
thing omh exists to avoid, brought back by the file that describes an
environment. It runs in the image omh has built by then, before the stack's own
layer exists, and it can read the repo and cannot write to it.

**It is shell rather than a list of filenames** because the first thing a real
ecosystem needs is to read a field: `packageManager` in `package.json` is how a
repo declares pnpm, and no "does this file exist" can see it.

What applied is written to `[provision]` in the repo's committed settings. A
launch does not ask again; `omh upgrade` does —
[Configuration](/reference/configuration/#provision--what-your-sandbox-is-built-with)
has what that table is.

## When a provide is wrong

Two different mistakes, caught by two different things.

A provide that applied and **did not deliver** — the recipe ran and the program
is not there — is caught by measurement: omh asks the built image whether each
of its `needs` resolves.

A provide that **should have applied and did not** is invisible to that check,
because nothing it would have installed is in any `needs` list that was
measured. What catches it is the hook: the one that runs `pnpm test` is
[held back by name](/reference/configuration/#hooks-your-sandbox-cannot-run),
because `pnpm` does not resolve.
