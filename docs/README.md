# omh documentation

> oh-my-zsh for agentic coding — the best agentic coding environment without the
> hassle of understanding, installing, and configuring everything.

**Status: early.** `0.1.0` (milestone v0). One harness (`claude`) has been driven for real work;
`opencode` passes `omh doctor` but has not. Docker is the only verified runtime.
Several things described in the design pages are **designed and not built** —
each says so at the top. See the [roadmap](design/roadmap.md).

## Start here

If you have five minutes and a repo, read [Getting started](getting-started.md).
It goes from nothing to a sandboxed agent with your setup already inside it.

## Using omh

| | |
|---|---|
| [Getting started](getting-started.md) | install, `omh init`, your first session |
| [Commands](commands.md) | every command, what it does, what it prints |
| [Configuration](configuration.md) | the three profile layers, provenance, `policy.toml`, `carry_in` |
| [Sessions](sessions.md) | what a session actually is, persistence, worktrees |
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
| [Adapters](design/adapters.md) | harnesses and editors as data, and how to add one |
| [Memory](design/memory.md) | the note graph, its guards, and the build order — the store, retrieval and the team layer are built; staleness is not |
| [Memory: how the design got here](design/memory-rationale.md) | the survey, the benchmark that reversed six choices, and the alternatives not taken |
| [Memory M0: running iwe](design/memory-m0.md) | the blocking spike, and the four claims the design rested on that did not survive it |
| [Measuring retrieval](design/memory-benchmark.md) | the benchmark that decides retrieval questions, and why it cannot be tilted |
| [Trust](design/trust.md) | provenance, evidence, and a credible exit |
| [Risks](design/risks.md) | what is weak, stated plainly |
| [Roadmap](design/roadmap.md) | what ships when, and what gates what |

## Contributing

[Contributing](contributing.md) — the testing rules, the invariants that must
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
