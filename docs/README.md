# omh documentation

> oh-my-zsh for agentic coding — the best agentic coding environment without the
> hassle of understanding, installing, and configuring everything.

**Status: early.** `0.8.0`. This release closes the two failures that were
silent: two checkouts with the same directory name no longer share sessions
([risks](design/risks.md) 8d), and the carried-file scan says which files it
could not read instead of reporting a clean harvest either way (4d). It adds
[`omh eject`](commands.md#omh-eject-harness---to-dir), the exit. The
[command surface](design/profile.md) landed in 0.7.0 and the
[work loop](design/git.md) in 0.6.0.
One harness (`claude`) has
been driven for real work; `opencode` passes `omh doctor` but has not. Docker is
the only verified runtime. Several design pages describe work that is **partly
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
