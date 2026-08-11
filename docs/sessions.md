# Sessions

A session is not a launch. It is a **running container, a git worktree, and a
branch**, which many harnesses take turns inhabiting.

```
       omh claude ──┐
       omh opencode ┼── exec ──┐
       omh attach ──┘  (ssh)   │
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
index warm and makes switching harness instant. `omh claude`, then `omh opencode`
against the same session, and everything the graph learned is still there.

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
inside the sandbox that pointer leads nowhere and every git command fails with
`fatal: not a git repository`.

The agent is told this, and told not to try to repair it. Not because repairing
is dangerous — `git init` there refuses for the same reason and leaves the
pointer untouched, which was checked rather than assumed — but because it cannot
work, and an agent that thinks git is merely broken will offer to commit work it
has no way to commit.

The branch itself is in your checkout the whole time — worktrees share a ref
store — so once the work is committed, `git log omh/s01` and `git push` work
from your own repo without `~/.omh` entering into it.

Two things are writable — the worktree, because that is the work, and
credentials, because tokens refresh in place. **Nothing else is**, and a test
asserts exactly that rather than asserting a list of mount strings.

Worth being precise about what this buys: a sandbox protects your *host*. What
protects your *repo* is the worktree branch. That is why `omh s rm` never
deletes a branch.

### Worktrees live outside the repo

```
~/.omh/worktrees/<repo>/<session>/
```

Nested inside the repo, your IDE would index every session's full copy of the
codebase — three sessions, four indexes, and a machine that sounds like it is
about to take off.

## Persistence

A long-lived container is not a long-lived *session*. Plain `exec` ties the
harness's lifetime to your terminal: close the lid and the agent is hung up on
mid-task while the container keeps running around the corpse.

So every harness is wrapped:

```
dtach -A /omh/sock/<session>-<harness>  <harness> [args…]
```

Detaching is your terminal closing. Reattaching is running `omh <harness>` again
— `-A` attaches to a live session or creates one, so a second invocation never
starts a second agent. The socket path is a pure function of session and
harness; anything variable in it would silently fork a duplicate.

Some harnesses ship their own resume. Relying on that would be exactly the
per-harness behaviour omh exists to abstract, so persistence is uniform and
lives in the distribution. Turn it off with `persistence = "none"`.

### Why not tmux

tmux is a multiplexer **and** a persistence tool, and omh needs only the second
half — `omh attach` means SSH already gives you as many shells against a session
as you want.

Adopting tmux would buy one feature we need and one we already have, while
importing costs that land exactly where this project is most fragile: a prefix
key competing with harness TUI bindings, nested tmux for anyone already running
it on the host, and another translation layer over mouse, resize and paste.

`dtach` does persistence in about a thousand lines with no prefix key, no config
file, and nothing sitting between you and the harness.

**Open question.** Watching several things in one session — agent, dev server,
shell — is a real want that `dtach` cannot serve. Whether that calls for tmux
inside the sandbox, host-side panes over SSH, or a supervisor model (`omh run` /
`omh logs` for services you do not watch) is unresolved. Recorded rather than
answered.

## Lifecycle

```console
$ omh s ls                # sessions, branches, state
$ omh s down s01          # stop the container, keep worktree and branch
$ omh s rm s01            # remove the session — branch survives
```

Sessions idle longer than `policy.idle_timeout` are stopped on the next launch.
N sessions means N containers, so this is not a nicety — see
[risks](design/risks.md).

Only the **container** stops; the worktree and branch survive, so
`omh <harness>` resumes exactly where you left off. The clock measures when you
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
