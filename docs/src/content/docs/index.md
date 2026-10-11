---
title: omh documentation
description: The safe way to run a coding agent against your repo — a sandboxed session, your setup already in it, and a branch you can review.
---

> oh-my-zsh for agentic coding — the best agentic coding environment without the
> hassle of understanding, installing, and configuring everything.

**Status: early.** `0.14.1`. This release is about memory another session can
use, and a sandbox whose git can merge. One note store per repo at
`~/.omh/memory/<repo>`, shared the moment a note is written and never
committed — [`omh memory`](/reference/commands/#omh-memory-) — and a base image on
Debian 13, because Debian 12's git 2.39.5 failed any check that merges.
0.13.0 ran the harnesses their makers ship now: every pin moved to its current
release and each adapter was re-read against it; the two claims that broke were
fixed rather than carried. [`omh auth codex`](/guides/accounts/#a-login-that-cannot-finish-in-a-sandbox)
finishes — a device code, or `--import` of the login this machine has — Codex
runs omh's hooks, `omh sNN` reads its rollouts, and an
[`account`](/guides/accounts/#keyed-by-harness-not-by-provider) can name one
harness's login with `codex:work`. An unreadable `worktrees/` no longer reads as
no sessions. [`omh sNN rm`](/reference/commands/#omh-snn-rm--and-what-it-refuses-to-take-with-it)
that can tell a squash already landed a branch, and harness names that cannot
be paths, arrived in 0.12.0,
[which hooks fired](/reference/commands/#omh-snn-shows-which-hooks-fired) and the opt-in
[guards](/reference/configuration/#guards-and-their-bypass) in 0.11.0,
[`omh upgrade`](/reference/commands/#omh-upgrade) and the checks `omh sNN commit` runs
before landing in 0.10.0, the [silent failures](/project/retrospectives/#the-silent-failures--done-080-9192)
closed in 0.8.0, the [command surface](https://github.com/mindsers/ohmyharness/blob/main/docs/design/profile.md) landed in 0.7.0 and
the [work loop](https://github.com/mindsers/ohmyharness/blob/main/docs/design/git.md) in 0.6.0.

`0.14.1` is repairs: no editor could attach to a session, a fresh install's
[`omh doctor`](/guides/troubleshooting/#omh-doctor) ended red on a memory server it
had never built, and several session lifecycle paths could lose work or leave a
worktree unusable. 0.14.0 retired the committed note layer and `omh memory promote` with it, and
moves the store to `~/.omh/memory/<repo>` — a store under the old path moves
itself, once, and says so. `omh doctor` gained a `git` row that asks the image
rather than the host. 0.13.2 let Codex run commands and edit files: its own
sandbox cannot start inside omh's container, so omh turns it off. 0.13.1 fixed
two launches 0.13.0 broke: opencode died on start over a
directory the image left to root, and Codex could not save its own config.
[`omh doctor`](/guides/troubleshooting/#omh-doctor) asks first whether a harness
starts at all.

One harness (`claude`) has been driven for real work; `opencode`, `omp` and
`codex` pass `omh doctor`, which proves their paths and nothing about their
behaviour. Docker is the only end-to-end-verified runtime; `podman` and `sbx`
are opt-in. Several design pages describe work that is **partly
built** — each says which parts, at the top. See the [roadmap](/project/roadmap/).

## Start here

If you have five minutes and a repo, [install omh](/getting-started/installation/)
and follow [Your first session](/getting-started/first-session/). It goes from
nothing to a sandboxed agent with your setup already inside it.

## Using omh

| | |
|---|---|
| [Installation](/getting-started/installation/) | Homebrew, the install script, or from source |
| [Your first session](/getting-started/first-session/) | `omh init`, logging in, and running an agent |
| [Commands](/reference/commands/) | every command, what it does, what it prints |
| [Configuration](/reference/configuration/) | the catalogue, settings and their layers, provenance, `carry_in` |
| [Sessions](/concepts/sessions/) | what a session actually is, the git the agent gets, persistence, worktrees |
| [Accounts](/guides/accounts/) | `omh auth`, several logins per harness |
| [Editors](/guides/editors/) | attaching VS Code, Zed, Cursor or Neovim over SSH |
| [Code graph](/guides/code-graph/) | the graph, the four hooks, `omh graph` |
| [Troubleshooting](/guides/troubleshooting/) | `omh doctor`, and the failures it exists to catch |

## Understanding omh

These explain *why*, and are worth reading before proposing an architectural
change — most of them record something that was tried and cost something.

| | |
|---|---|
| [Why a distribution](/concepts/distribution/) | the thesis, why not an app store, who else is in this space |
| [Architecture](/concepts/architecture/) | images, runtime backends, on-disk layout |
| [Adapters](/reference/adapters/) | harnesses and editors as data, and how to add one |
| [The base set](/reference/base-set/) | omh's opinion as a versioned data file, and the test that makes an entry earn its place |
| [Trust](/concepts/trust/) | provenance, evidence, and a credible exit |
| [Risks](/concepts/risks/) | what is weak, stated plainly |
| [Decisions](/project/decisions/) | every load-bearing choice with its reasoning |
| [Roadmap](/project/roadmap/) | what ships when, and what gates what |
| [Retrospectives](/project/retrospectives/) | four bodies of work that shipped outside a milestone, and what each one cost |

### Design pages

Seven more are kept beside the source and read on GitHub. Each records how one
part of omh was designed, and says at the top how much of that is built.

| | |
|---|---|
| [The profile](https://github.com/mindsers/ohmyharness/blob/main/docs/design/profile.md) | built — one catalogue, `[use]` per repo, rules composed with the repo's own, omh's hooks and sections generated from the base set, and the command surface 0.7.0 settled on top of it |
| [Adoption](https://github.com/mindsers/ohmyharness/blob/main/docs/design/adoption.md) | partly built — what `init` decides when it meets a repo, the toolchain probe, and what `omh import` still has to migrate |
| [Git](https://github.com/mindsers/ohmyharness/blob/main/docs/design/git.md) | built in 0.6.0 — the loop around the sandbox's repository: reading a session's work, landing it in stages, staying current with trunk, and reaching several sessions from one place. Twelve steps, each naming the pull request that landed it |
| [Memory](https://github.com/mindsers/ohmyharness/blob/main/docs/design/memory.md) | the note graph, its guards, and the build order — the store, retrieval and staleness are built; hub pages are not, and the committed layer the spec describes was removed in 0.14 |
| [Memory: how the design got here](https://github.com/mindsers/ohmyharness/blob/main/docs/design/memory-rationale.md) | the survey, the benchmark that reversed six choices, and the alternatives not taken |
| [Memory M0: running iwe](https://github.com/mindsers/ohmyharness/blob/main/docs/design/memory-m0.md) | the blocking spike, and the four claims the design rested on that did not survive it |
| [Measuring retrieval](https://github.com/mindsers/ohmyharness/blob/main/docs/design/memory-benchmark.md) | the benchmark that decides retrieval questions, and why it cannot be tilted |

## Contributing

[Contributing](https://github.com/mindsers/ohmyharness/blob/main/.github/CONTRIBUTING.md) — the testing rules, the invariants that must
keep holding, and the one thing about this codebase that will mislead you if
nobody tells you first.

---

### A note on how these pages are written

Claims here are meant to be checkable. Where a number appears it was measured on
this repo and says so; where something is unverified it is marked unverified.

That is not modesty. Almost every bug this project has shipped lived at the
boundary between omh and external software, where a confident sentence in a
document and a green test suite are equally worthless — see
[Troubleshooting](/guides/troubleshooting/) for why `omh doctor` exists and what it
can prove that nothing else can.
