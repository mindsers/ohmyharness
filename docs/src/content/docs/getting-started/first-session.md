---
title: Your first session
description: Set a repository up, start Claude Code in a sandbox that already has your rules, skills and MCP servers, and bring its work back as a branch you review.
sidebar:
  order: 2
---

Set a repository up, start Claude Code in a sandbox that already has your rules,
skills and MCP servers, and bring its work back as a branch you review. About
five minutes, most of it the first image build.

**You need** omh [installed](/getting-started/installation/), Docker running, a
git repository, and access to Claude Code.

<div class="steps">

1. Set the repository up.

   ```console
   $ cd ~/code/your-project
   $ omh init
   ```

   `init` reads your manifests, README and git history, decides, and prints what
   it decided. It asks nothing. The first run builds the sandbox image, which
   takes a while; later runs use the cache.

   ```text title="omh init"
   omh init — decided, asked nothing

     harness    claude  (found on your host)
     image      omh/claude:b4edd8fd15d4d669 (already built)
     image      omh/claude:8eae0d5c1511fa89 (this repo's toolchain)
     stack      rust (from Cargo.toml)
     hooks      2 selected  (6 more in your catalogue)
     provision  rust/linker
     provision  rust/toolchain
     memory     2 notes written, 0 already there
     graph      indexing in background → omh-cache-your-project-a8c4d1cd
   ```

   The report goes on to say where your catalogue and this repo's settings
   live, which [base set](/reference/base-set/) entries you got, and what to run
   next. [`omh why <name>`](/reference/commands/#omh-why-thing) explains any
   entry: what it costs, what was considered instead, how to remove it.

2. Log in to Claude.

   ```console
   $ omh auth claude --name personal
   ```

   This runs Claude Code's own login and keeps the result under
   `~/.omh/creds/claude/personal/`. You do it once per account — for a work and
   a personal account, see [Accounts](/guides/accounts/).

3. Start a session and give it a task.

   ```console
   $ omh new claude
   ```

   Claude Code opens in a container, on a git worktree with a branch of its
   own, with your configuration where it reads it. Your checkout is not mounted.
   Ask for something small to start with — fix a failing test, say.

   Close the terminal whenever you like: the agent keeps running, and
   `omh s01 resume` puts you back.

4. Read what it did.

   From a terminal on your host, in the same repository:

   ```console
   $ omh s              # every session and its state
   $ omh s01 diff -p    # the patch
   ```

   To look around yourself, open the same session in your editor:

   ```console
   $ omh s01 attach
   ```

5. Keep the work.

   ```console
   $ omh s01 commit -m "Fix the parser test"
   $ omh s01 push fix/parser-test
   ```

   `commit` runs the repository's own checks in the sandbox first, then lands
   the work as one commit on the session branch, `omh/s01`. `push` sends that
   branch to origin under a name a reviewer can read. Your checkout changes only
   when you merge it.

</div>

## Check that it is wired up

Do not trust the fact that it started:

```console
$ omh doctor
```

`doctor` launches the real image with the real mounts, checks the paths Claude
Code actually reads — rules, skills, MCP servers, hooks — and asks Claude Code
whether the MCP servers loaded. It is the only thing that can prove a setup is
right; [Troubleshooting](/guides/troubleshooting/#omh-doctor) says what each row
means.

## What you just used

- **A session** is a container, a git worktree and a branch. The agent works on
  its side of that line and cannot reach yours. → [Sessions](/concepts/sessions/)
- **Your configuration** — rules, skills, MCP servers, commands, subagents and
  hooks — lives once in `~/.omh`, and each repository names what it takes from
  it. → [Configuration](/reference/configuration/)
- **The code graph and project memory** were set up by `init` and are there in
  every session of this repository. → [Code graph](/guides/code-graph/),
  [Memory](/reference/memory/)

## Behind the scenes

### What it actually did

<details>
<summary>The seven things <code>omh init</code> does, in order</summary>

```text
1  repo check                     fail fast, before any work
2  ensure ~/.omh: the catalogue, bundled adapters, editors, the base set
3  detect the stack; read the host for a harness *preference*
4  write ~/.omh/mcp.json and <repo>/.omh: settings, [use], memory, a hook per stack
5  seed memory by derivation       README, manifests, git log, existing rules
6  ensure the image                ← the real blocker; nothing runs without it
7  index the code graph            background, resumable
```

**The report is the last step.** Every decision is collected and printed once,
which is why the image and graph lines — made at 6 and 7 — are in it, and why
`--json` can be one object rather than six.

**`[use]` is written expanded**, every catalogue entry named, because a list you
curate by deleting lines beats a wildcard you cannot review. What you add to
your catalogue later is not in it, so the launcher names what is not selected —
see [Configuration](/reference/configuration/#use--what-this-repo-takes-from-your-catalogue).

**Step 4 never overwrites what you wrote**, and writes no rules file: omh's own
sections are [base-set entries](/reference/base-set/), composed into every
session. Your `AGENTS.md` or `CLAUDE.md` is read and composed with them — see
[Configuration](/reference/configuration/#keeping-the-agents-git-status-clean).

**Step 6 is the slow one** — about 30 seconds the first time, cached afterwards.
`init` is not finished until `omh new <harness>` works, so it builds the image
rather than leaving that to your first launch.

**Step 3 reads your host only for a preference.** The harness that runs is the
one inside the sandbox, a different installation.

The full report, with its last sections:

```text
  catalogue  /Users/you/.omh
  this repo  /Users/you/code/your-project/.omh  (committed)

  base set (2026.08)
    omh-codegraph  structural queries instead of re-grepping the repo every task
    omh-memory     what a session learned outlives it — a removed session leaves no transcript to grep

  omh why <name>  what it costs, what was considered instead, how to remove it

  next
    omh new claude  start a session
    omh s           list sessions, each by its id
    omh sNN resume  rejoin one later
    omh sNN attach  open it in your editor
```

</details>

### Derive, never interrogate

`init` asks no questions, on purpose. Every question is hassle omh exists to
remove, and most answers are already in the repository: manifests name the
stack, git log names what you work on, the README names the project. Derived
facts also stay current as the repo changes, where an answer typed once goes
stale.

<details>
<summary>When omh would ask, and the four kinds of unknown</summary>

A question earns its place only if it is **not derivable**, **actionable** (omh
does something different with the answer), and **answerable well right now**.

| | Knowable | Example | What omh does |
|---|---|---|---|
| 1 | from the repo, on your machine | `Cargo.toml` means rust | derives it, for free |
| 2 | only in the sandbox | does `cargo` resolve; does this repo declare pnpm | asks the sandbox, once |
| 3 | nobody has written it down | what command tests this project | asks you once, and records the answer |
| 4 | only you know | "these hooks are mine, keep them" | `omh import` |

Every question is pushed up a row if it can be. The strongest permitted form is
*derive, then confirm* — state the guess and make it correctable:

```text
! 2 stacks detected; hooks were written for all of them.
  drop the ones you do not want: .omh/hooks/
```

That is not a questionnaire: `init` still decided, and showed its work.

</details>

## Next

<div class="cards">

- [Commands](/reference/commands/)\
  Everything you can do with a session, and what each command prints.
- [Configuration](/reference/configuration/)\
  Your catalogue, what a repository takes from it, and where settings go.
- [Accounts](/guides/accounts/)\
  Several logins, and keeping work and personal apart.
- [Editors](/guides/editors/)\
  Open the same session in VS Code, Zed, Cursor or Neovim.

</div>
