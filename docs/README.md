# omh documentation

> oh-my-zsh for agentic coding — the best agentic coding environment without the
> hassle of understanding, installing, and configuring everything.

**Status: early.** `0.13.2`. This release is about running the harnesses
their makers ship now, and about Codex. Every pin moved to its current release
and each adapter was re-read against it; the two claims that broke were fixed
rather than carried. [`omh auth codex`](accounts.md#a-login-that-cannot-finish-in-a-sandbox)
finishes — a device code, or `--import` of the login this machine has — Codex
runs omh's hooks, `omh sNN` reads its rollouts, and an
[`account`](accounts.md#keyed-by-harness-not-by-provider) can name one
harness's login with `codex:work`. An unreadable `worktrees/` no longer reads as
no sessions. [`omh sNN rm`](commands.md#omh-snn-rm--and-what-it-refuses-to-take-with-it)
that can tell a squash already landed a branch, and harness names that cannot
be paths, arrived in 0.12.0,
[which hooks fired](commands.md#omh-snn-shows-which-hooks-fired) and the opt-in
[guards](configuration.md#guards-and-their-bypass) in 0.11.0,
[`omh upgrade`](commands.md#omh-upgrade) and the checks `omh sNN commit` runs
before landing in 0.10.0, the [silent failures](design/roadmap.md)
closed in 0.8.0, the [command surface](design/profile.md) landed in 0.7.0 and
the [work loop](design/git.md) in 0.6.0.

`0.13.2` lets Codex run commands and edit files: its own sandbox cannot start
inside omh's container, so omh turns it off. 0.13.1 fixed two launches 0.13.0
broke: opencode died on start over a
directory the image left to root, and Codex could not save its own config.
[`omh doctor`](troubleshooting.md#omh-doctor) now asks first whether a harness
starts at all.

One harness (`claude`) has been driven for real work; `opencode`, `omp` and
`codex` pass `omh doctor`, which proves their paths and nothing about their
behaviour. Docker is the only end-to-end-verified runtime; `podman` and `sbx`
are opt-in. Several design pages describe work that is **partly
built** — each says which parts, at the top. See the [roadmap](design/roadmap.md).

## Start here

If you have five minutes and a repo, read [Getting started](getting-started.md).
It goes from nothing to a sandboxed agent with your setup already inside it.

## Using omh

| | |
|---|---|
| [Getting started](getting-started.md) | install, `omh init`, your first session |
| [Commands](commands.md) | every command, what it does, what it prints |
| [Configuration](configuration.md) | the catalogue, settings and their layers, provenance, `carry_in` |
| [Sessions](sessions.md) | what a session actually is, the git the agent gets, persistence, worktrees |
| [Accounts](accounts.md) | `omh auth`, several logins per harness |
| [Editors](editors.md) | attaching VS Code, Zed, Cursor or Neovim over SSH |
| [Code graph](code-graph.md) | the graph, the four hooks, `omh graph` |
| [Troubleshooting](troubleshooting.md) | `omh doctor`, and the failures it exists to catch |

## Understanding omh

These explain *why*, and are worth reading before proposing an architectural
change — most of them record something that was tried and cost something.

| | |
|---|---|
| [Why a distribution](design/distribution.md) | the thesis, why not an app store, who else is in this space |
| [Decisions](design/decisions.md) | every load-bearing choice with its reasoning |
| [The base set](design/base-set.md) | omh's opinion as a versioned data file, and the test that makes an entry earn its place |
| [Architecture](design/architecture.md) | images, runtime backends, on-disk layout |
| [The profile](design/profile.md) | built — one catalogue, `[use]` per repo, rules composed with the repo's own, omh's hooks and sections generated from the base set, and the command surface 0.7.0 settled on top of it |
| [Adapters](design/adapters.md) | harnesses and editors as data, and how to add one |
| [Adoption](design/adoption.md) | partly built — what `init` decides when it meets a repo, the toolchain probe, and what `omh import` still has to migrate |
| [Git](design/git.md) | built in 0.6.0 — the loop around the sandbox's repository: reading a session's work, landing it in stages, staying current with trunk, and reaching several sessions from one place. Twelve steps, each naming the pull request that landed it |
| [Memory](design/memory.md) | the note graph, its guards, and the build order — the store, retrieval, the team layer and staleness are built; hub pages are not |
| [Memory: how the design got here](design/memory-rationale.md) | the survey, the benchmark that reversed six choices, and the alternatives not taken |
| [Memory M0: running iwe](design/memory-m0.md) | the blocking spike, and the four claims the design rested on that did not survive it |
| [Measuring retrieval](design/memory-benchmark.md) | the benchmark that decides retrieval questions, and why it cannot be tilted |
| [Trust](design/trust.md) | provenance, evidence, and a credible exit |
| [Risks](design/risks.md) | what is weak, stated plainly |
| [Roadmap](design/roadmap.md) | what ships when, and what gates what |

## Contributing

[Contributing](../.github/CONTRIBUTING.md) — the testing rules, the invariants that must
keep holding, and the one thing about this codebase that will mislead you if
nobody tells you first.

---

### A note on how these pages are written

Claims here are meant to be checkable. Where a number appears it was measured on
this repo and says so; where something is unverified it is marked unverified.

That is not modesty. Almost every bug this project has shipped lived at the
boundary between omh and external software, where a confident sentence in a
document and a green test suite are equally worthless — see
[Troubleshooting](troubleshooting.md) for why `omh doctor` exists and what it
can prove that nothing else can.
