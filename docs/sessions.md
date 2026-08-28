# Sessions

A session is not a launch. It is a **running container, a git worktree, and a
branch**, which many harnesses take turns inhabiting.

```
       omh new claude ──┐
       omh new opencode ┼── exec ──┐
       omh s attach ────┘  (ssh)   │
                               ▼
 ┌──────────────────────────────────────────────────────┐
 │ SESSION  omh-<repo>-s01          detached, long-lived │
 │                                                       │
 │  sshd 127.0.0.1:49201 ──── your editor attaches here  │
 │  codebase-memory-mcp ──── daemon: the index stays warm │
 │                                                       │
 │  /work  ← worktree (host dir, bind-mounted)           │
 │  staged profile, read-only                            │
 │  /omh/cache ← volume keyed by REPO, not harness       │
 └──────────────────────────────────────────────────────┘
```

Making the session the unit of work — rather than the launch — is what keeps the
index warm. `omh new claude`, then `omh s01 resume opencode` against that same
session, and everything the graph learned is still there: the graph lives in a volume keyed by
repo, and the worktree and branch are on the host, so neither belongs to the
container.

**Switching harness is not free, though.** An image is built per harness, so a
session started by `omh new claude` is running the claude image and does not contain
`opencode` at all. Asking for the other one restarts the sandbox:

```console
$ omh s01 resume opencode
omh: restarting the sandbox for omh/s01 — image (omh/claude:b4ed… → omh/opencode:1e1a…), mounts (6 added, 9 removed)
```

A few seconds, and nothing is lost. It used to be instant and wrong — omh execed
`opencode` into the claude image, which does not have it, and the harness's whole
staged profile stayed unmounted. See
[what a container is compared against](#what-a-container-is-compared-against).

A session with a harness still running in it is **not** restarted; see the same
section.

## What the agent can reach

Only the worktree. **Your checkout is never mounted**, so the agent cannot see
your uncommitted work, cannot touch `main`, and cannot reach anything outside
the branch it was given.

Review is therefore just git:

```console
$ omh s diff
$ git log omh/s01
```

That git runs on the **host**, and so does the way work leaves a session:

```console
$ omh s commit -m "Fix the tap guard"
$ omh s push fix/tap-guard
```

Not a stylistic choice. A worktree's `.git` is a file holding an absolute path
to an admin directory in your checkout, and omh mounts only the worktree — so
that pointer leads nowhere from inside a container, and pushing to your remote
is not something the sandbox is in a position to do.

The branch itself is in your checkout the whole time — worktrees share a ref
store — so once the work is committed, `git log omh/s01` and `git push` work
from your own repo without `~/.omh` entering into it.

What is writable is a short list, and it grew: the worktree, because that is the
work; credentials, because tokens refresh in place; the sandbox's own repository
(below); each file `carry_in` hands over, so the agent can edit config it was
given; and two caches outside `/work` — the code graph and the local note store.

**Nothing else is**, and a test asserts the set by name rather than asserting a
list of mount strings, so adding one is a decision somebody has to make on
purpose.

### Worktrees live outside the repo

```
~/.omh/worktrees/<repo>/<session>/
```

Nested inside the repo, your IDE would index every session's full copy of the
codebase — three sessions, four indexes, and a machine that sounds like it is
about to take off.

## The agent has git, and it is not yours

Until 2026.08 it had none: the dangling pointer meant every command failed with
`fatal: not a git repository`, so omh refused the call outright rather than let
an agent spend turns repairing what could not be repaired. That cost the agent
`status`, `diff`, `log`, `stash` and `reset --hard` — and cost an attached
editor its source control panel, since the editor runs *inside* the container —
for the editors that have one; `omh s attach nvim` never did.

So omh gives it a repository of its own. A gitdir under `~/.omh/shadow/`, seeded
with a single commit of the tree the session started from — minus everything omh
put there, so your carried `.env` and omh's own staged documents are not in it
either — mounted at `/omh/shadow`, and named by a `.git` file mounted over
`/work/.git`. That last
mount is the whole trick: it shadows the pointer for the container's view only,
so your own file is never written and everything on this page still works.

**What makes it safe is what is not in it.** It starts at one commit on one
branch, gains whatever the agent does, and never has a remote or a commit from
your checkout — an agent reading its own history learns
nothing about yours, and there is no `main` in there to move. Shared *blobs* are
unavoidable, since a file whose content matches yours hashes the same; history
is the thing that must not cross.

It lives at `~/.omh/shadow/<repo>/<session>.git`, on the branch
`<session>-scratch` — keyed by the checkout, like every other per-repo path omh
keeps — so you can read it from the host if you want to:

```console
$ git --git-dir=~/.omh/shadow/myrepo/s01.git log --oneline
```

Reading it directly is no longer necessary: `omh s01 log` numbers those commits
and `omh s01 diff 4 -p` reads one. [Git](design/git.md) is the design.

**Trunk can still reach it, one direction only.** `omh s01 sync` merges the
new base into the session *on the host* and writes the resulting files in, so
the sandbox receives a tree and never a commit of yours — the rule above holds
through a sync. The agent finds a commit saying `base moved to <sha>` and, if
anything conflicted, the files sitting there with their markers for it to
resolve — and, at its next start, one sentence saying so, delivered once and
then gone. That last part is Claude Code only; opencode and omp cannot speak at
a session's start, and say so when they launch. It refuses while the sandbox is running, because what the agent
believes the tree holds is in its conversation and no file on disk reaches
that.

**Its commits are not yours until you ask for them.** `omh s commit` squashes the
files into one commit of your own and never looks at that repository;
`omh s commit --keep` replants the agent's commits, messages and all — every one
since the last handover, or the ones you name by their number in `omh s01 log`:
`--keep 1,3-4`. Whichever you use, `omh s rm` takes the repository with the session
— so checkpoints you did not harvest go with it, and after a `git reset --hard`
in the sandbox those are the only copies there were.

Push is the one thing still walled, by git's own `pre-push` hook inside that
repository. Be clear about what that is: a signpost, not a control. There is no
remote, so a push fails anyway; the hook is what meets the agent after git's own
error suggests `git remote add`, and it is bypassable by `--no-verify`. Checked
against git 2.55.0 rather than assumed, both directions.

Nothing here contains a determined agent, and nothing ever did — the container
has `curl` and outbound network. See [Risks](design/risks.md).

Worth being precise about what this buys: a sandbox protects your *host*. What
protects your *repo* is the worktree branch. That is why `omh s rm` never
deletes a branch.

## What a container is compared against

A launch plan is a pure description — image, mounts, network, environment — and a
session container is one plan materialized. Everything in that list is fixed the
moment the container starts: no later `exec` adds a mount or changes an image.

So omh stamps the plan onto the container as labels at launch, and compares
before reusing one. If they disagree the container is replaced, naming what
moved. Two real failures came from never asking:

- `omh s01 resume opencode` on a session started by `omh new claude` execed a binary that image
  does not contain.
- changing the account between launches went on quietly using the old one — the
  exact thing `omh auth` refuses to guess about elsewhere. It was `--account
  work` then; the account is `omh set account <name>` now, and the mount it
  resolves to is part of the stamp either way.

The harness's own arguments are deliberately **not** part of the comparison.
`claude --resume x` is the same session as `claude`, and a stamp that moved would
rebuild the sandbox on every flag.

### It will not restart over running work

Replacing a container kills whatever is inside it, so a session with a live
harness is reported instead:

```console
$ omh s01 resume claude
Error: session s01 is running opencode and cannot be reused for this launch (image (…))
  stop it with        omh s01 down
  or start a fresh one  omh new claude
```

Liveness is read from the `dtach` sockets, which exist only while their harness
does — a socket *is* a harness name, where the process table needs one parsed
out of a command line.

It also used to be the only signal that worked. PID 1 in the sandbox was `sleep
infinity`, which reaps nothing, so an exited `dtach` lingered as
`[dtach] <defunct>` and `pgrep` would have answered "still running" for the life
of the session. The container runs under a real init now, so those no longer
accumulate — but a session started by an older omh keeps its zombies until it is
next restarted.

## Persistence

A long-lived container is not a long-lived *session*. Plain `exec` ties the
harness's lifetime to your terminal: close the lid and the agent is hung up on
mid-task while the container keeps running around the corpse.

So every harness is wrapped:

```
dtach -A /omh/sock/<session>-<harness>  <harness> [args…]
```

Detaching is your terminal closing. Reattaching is `omh sNN resume` — `-A`
attaches to a live session or creates one, so a second invocation never starts a
second agent. The socket path is a pure function of session and
harness; anything variable in it would silently fork a duplicate.

Some harnesses ship their own resume. Relying on that would be exactly the
per-harness behaviour omh exists to abstract, so persistence is uniform and
lives in the distribution. Turn it off with `persistence = "none"`.

### Why not tmux

tmux is a multiplexer **and** a persistence tool, and omh needs only the second
half — `omh s attach` means SSH already gives you as many shells against a session
as you want.

Adopting tmux would buy one feature we need and one we already have, while
importing costs that land exactly where this project is most fragile: a prefix
key competing with harness TUI bindings, nested tmux for anyone already running
it on the host, and another translation layer over mouse, resize and paste.

`dtach` does persistence in about a thousand lines with no prefix key, no config
file, and nothing sitting between you and the harness.

**Open question.** Watching several things in one session — agent, dev server,
shell — is a real want that `dtach` cannot serve. Whether that calls for tmux
inside the sandbox, host-side panes over SSH, or a supervisor model — a verb
that starts a service and another that shows its output, for the things you do
not watch — is unresolved. Recorded rather than answered.

## Lifecycle

```console
$ omh s                   # sessions, branches, state
$ omh s01 down            # stop the container, keep worktree and branch
$ omh s01 rm              # remove the session — branch survives
```

`omh s` reports state as English for you and as fields for a script: `omh s
--json` gives each session's `running`, `behind` and `work.state` without
anyone parsing the table — see [what every command
prints](commands.md#what-every-command-prints).

Sessions idle longer than `idle_timeout` are stopped on the next launch.
N sessions means N containers, so this is not a nicety — see
[risks](design/risks.md).

Only the **container** stops; the worktree and branch survive, so
`omh sNN resume` puts you back exactly where you left off. The clock measures when you
last *launched into* or *attached to* a session, not the agent's own writes — a
session left running after you walked away is what this exists to reap, and one
where an agent is working unattended is one you started recently.

Unset by default: nothing is stopped unless you ask for it. A session that has
no recorded use — from before this existed, or after clearing `~/.omh/run` — is
left alone rather than stopped on a guess.

`down` and `rm` differ in what they leave behind, and both leave the branch. To
actually discard agent work you delete the branch yourself, deliberately, with
git.

## Several sessions at once

Normal, and the reason sessions are numbered. Each gets its own container,
worktree and branch.

One thing they **share** is the code graph store, keyed by repo. That is what
keeps the index warm across a harness switch, and it means an agent in one
session can query another session's graph. Mitigated, not prevented — see
[Code graph](code-graph.md#what-the-agent-can-still-see).
