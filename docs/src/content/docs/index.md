---
title: omh documentation
description: Set up, use and understand omh — the safe way to run a coding agent against your repo, with your setup already inside the sandbox and its work handed back as a branch.
template: splash
hero:
  title: Your agent's environment, already set up.
  tagline: omh runs Claude Code in a sandbox with your rules, skills, MCP servers, a code graph and shared project knowledge already in place — and hands its work back as a branch you review.
  actions:
    - text: Install omh
      link: /getting-started/installation/
      icon: right-arrow
    - text: Your first session
      link: /getting-started/first-session/
      variant: minimal
---

:::caution[Early project]
omh is at `0.15.0`. Claude Code on Docker is the path people have done real work
through; the other harnesses and runtimes pass `omh doctor` but have not been
driven for real. The [roadmap](/project/roadmap/) says what comes next.
:::

## Start here

<div class="cards">

- [Install omh](/getting-started/installation/)\
  Homebrew, the install script, or from source. About two minutes.
- [Your first session](/getting-started/first-session/)\
  From a repository to a sandboxed agent, and its work back as a branch.
- [Commands](/reference/commands/)\
  Every command and flag, and what each one prints.
- [Troubleshooting](/guides/troubleshooting/)\
  `omh doctor`, and what to do when something does not start.

</div>

## Common tasks

| I want to… | Read |
|---|---|
| bring the agent's work back onto my branch | [Getting work out of a session](/reference/commands/#getting-work-out-of-a-session) |
| use a work and a personal Claude account | [Accounts](/guides/accounts/) |
| open the session in VS Code, Zed, Cursor or Neovim | [Editors](/guides/editors/) |
| choose which skills and MCP servers a repo uses | [`[use]`](/reference/configuration/#use--what-this-repo-takes-from-your-catalogue) |
| give the sandbox a `.env` file git does not track | [`carry_in`](/reference/configuration/#carry_in) |
| find out why omh installed something | [`omh why`](/reference/commands/#omh-why-thing) |
| let the agent look up symbols and callers | [Code graph](/guides/code-graph/) |
| fix an image build that fails on a certificate | [An unknown issuer error](/guides/troubleshooting/#an-unknown-issuer-error-when-omh-builds-an-image) |

## Understanding omh

These pages explain *why* omh works the way it does. Read them before proposing
an architectural change: most of them record something that was tried, and what
it cost.

<div class="cards">

- [Sessions](/concepts/sessions/)\
  What a session is: a container, a git worktree and a branch.
- [Architecture](/concepts/architecture/)\
  Images, runtime backends, and what lives where on disk.
- [Why a distribution](/concepts/distribution/)\
  The thesis: curation and defaults, not another catalogue.
- [Trust](/concepts/trust/)\
  Provenance, measured costs, and a way out.
- [Risks](/concepts/risks/)\
  What is still weak, stated plainly.
- [Decisions](/project/decisions/)\
  Every load-bearing choice, with its reasoning.

</div>

[Retrospectives](/project/retrospectives/) cover what past work cost. One proposal
is kept beside the source rather than here:
[the sandbox's repository becomes a remote](https://github.com/mindsers/ohmyharness/blob/main/docs/design/sandbox-remote.md)
— proposed, not built.

## What's new in 0.15

- **Your catalogue on a screen.** At a terminal, `omh settings` shows your
  defaults and every rule, skill, server, command, subagent and hook, changes
  them in place, and says what a new repo gets. Each change is the command line
  you would have typed. See [`omh settings`](/reference/commands/#omh-settings).
- **The host stops running what the sandbox wrote.** Host-side git, note and
  transcript reads, and hooks resolved inside the agent's worktree no longer run
  or follow anything the agent left. See [risks](/concepts/risks/).
- **Fewer things that failed in silence.** Session removal, launches, `sync` and
  `attach` stop losing work or editing your files without saying so, and a
  checkout whose name has a space or an accent can launch.
- **Every harness at its latest release**, and `cargo install omh` builds on the
  Rust omh declares.

Earlier releases are on [GitHub](https://github.com/mindsers/ohmyharness/releases).

## Contributing

[CONTRIBUTING](https://github.com/mindsers/ohmyharness/blob/main/.github/CONTRIBUTING.md)
has the testing rules, the invariants that must keep holding, and the one thing
about this codebase that will mislead you if nobody tells you first.

:::note[How these pages are written]
Claims here are meant to be checkable. A number was measured on this repository
and says so; anything unverified is marked unverified. Almost every bug omh has
shipped lived where omh meets other software, where a confident sentence and a
green test suite are equally worthless — which is why
[`omh doctor`](/guides/troubleshooting/#omh-doctor) exists.
:::
