# Troubleshooting

## `omh doctor`

```console
$ omh doctor
omh doctor: claude (in omh/claude:2133265d, account personal)

  ✓ rules       /work/CLAUDE.md
  ✓ skills      /home/agent/.claude/skills
  ✓ mcp         /work/.mcp.json
  ✓ mcp-loaded  /work (claude mcp list)
  ✓ commands    /home/agent/.claude/commands
  ✓ hooks       /home/agent/.claude/settings.json
  ✓ token       /home/agent/.claude/.credentials.json (atomic write)

  all 7 checks passed — claude's adapter paths are verified
```

Run it after changing an adapter, after upgrading a harness, and any time a
session behaves as though your profile is not there.

## Why it exists

**Factual correctness is not testable in process.**

Adapters assert things about *external software* — that Claude Code reads MCP
config from `/work/.mcp.json`, that skills live in `~/.claude/skills`. A green
unit suite proves omh mounts a path faithfully. It cannot prove anything reads
it, and that software ships weekly.

Almost every bug this project has shipped lived at that boundary, and **not one
was catchable by the test suite.** `doctor` is the only cure.

The `mcp` binding is the cautionary tale. It said `$HOME/.mcp.json` — a path
Claude Code does not read and never did — and omh rendered a correct document,
mounted it faithfully, and reported `✓ mcp` for as long as that lasted. Not one
session ever loaded an MCP server. Checking a document proves the document; only
the harness can say whether it read it, which is what `mcp-loaded` asks and why
its check runs the harness's own `mcp list` inside the sandbox.

## What it does

It launches the real image with the real mounts and inspects the **guest** paths
the adapter declares. Checking anything host-side would just re-test the staging
directory omh wrote a moment earlier, which is circular.

Capabilities the harness cannot express are **skipped, not failed** — they were
already reported as dropped at launch.

### The credential probe

This is the half no in-process test can reach: whether a token saved at a path
survives depends on how the runtime binds it, not on anything omh wrote. So the
probe attempts the temp-file-plus-rename that every token save performs.

It is **non-destructive by construction** — the file case copies the original
and renames byte-identical content back, so a successful probe changes nothing
and a failed one touches nothing. A health check that costs you your login would
be worse than no check.

Run against a file mount, it reports the real defect:

```
  ✗ token      /home/agent/.claude.json cannot be renamed over —
               a token saved here will not persist
```

### A silent probe is never a pass

If a probe produces no output, that means the sandbox never ran it. Calling that
success would make `doctor` worse than useless, so silence is a failure.

## Current status

Both shipped adapters are verified this way, hooks included on both. The
"unverified claim" caveat is retired for these two, and any third adapter
inherits the same bar.

The *number* of checks is not a property of the adapter: it counts the
capabilities your profile declares, plus a `token` check only when you have
captured a login for that harness, plus `memory` when the base set's server is
installed. Two harnesses showing different totals usually means you are logged
in to one of them.

---

## Common failures

### The harness starts but does not see my rules or skills

Run `omh doctor`. If a path fails there, the adapter is wrong — that is a bug,
and [`docs/design/adapters.md`](design/adapters.md) covers how to fix it.

If doctor passes, check whether the capability was dropped at launch:

```console
$ omh opencode
omh: opencode on omh/s01 — dropped hooks: graph-first (no `search` tool),
     graph-orient (no `session-start` moment),
     graph-read (no way to inject text before a tool runs)
```

That is the harness genuinely not supporting the feature, not omh losing it.

### I logged in, but the next session is logged out

The token was written somewhere that does not persist. `omh doctor` names it:

```
  ✗ token      /home/agent/.claude.json cannot be renamed over
```

Background in [Accounts](accounts.md#mount-the-directory-never-the-token-file).

### `network omh-<repo> not found`

The plan named a per-project network that was never created. A plan must be
*runnable*, not merely well-formed — this gap made every real launch die while
every unit test passed, and it is the archetypal case for why `doctor` exists.

### `create mountpoint for /work/AGENTS.md mount: ... is outside of rootfs`

Fixed in `v0.2.1`. omh mounts its rules onto `/work/CLAUDE.md` and
`/work/AGENTS.md` — paths inside the worktree mount — and left creating those
destinations to the runtime. Docker Desktop will not: `/work` is the host
worktree, so it resolves the destination back to a host path and refuses to
create a mountpoint outside the container's rootfs.

Docker creates the empty file on the host on its way out, which is why this read
as intermittent: the first launch of a session died, the second found the
leftover and worked. omh places those files itself now, before docker sees the
plan. On an older version, launching a second time is the workaround.

### `current working directory is outside of container mount namespace root`

Docker's full wording is `OCI runtime exec failed: ... -- possible container
breakout detected`, which is alarming and misleading: nothing broke out. The
session container is running with `/work` bound to a worktree directory that no
longer exists. Recreating the directory does not help — a bind mount follows the
inode, not the path — so every command into that container fails the same way.

Fixed in `v0.3.1`, from both ends. `omh s rm` now takes the container down with
the worktree, which is what created the mismatch, and a launch that finds a
container it cannot enter replaces it instead of exec'ing into it:

```console
omh: restarting the sandbox for omh/s01 — it can no longer reach its worktree
```

The worktree and branch are on the host, so the restart costs nothing.

On an older version, `docker rm -f omh-<repo>-<session>` and relaunch.

### `restarting the sandbox for omh/s01 — …`

Not an error. The container under that session id was not built from the plan
you just asked for — a different harness, a different account, a changed mount
set — and no `exec` can retrofit any of those. The line names what moved. The
worktree and branch are on the host and the graph is in a volume, so the restart
costs seconds and loses nothing.

`it predates this check` means the container was started by a version of omh that
did not stamp its plan, so nothing about it can be verified. It happens once per
session after upgrading.

### `session s01 is running opencode and cannot be reused for this launch`

The same mismatch, but something is live inside and restarting would kill it. Use
`omh s down s01` if you want it gone, or `omh --new <harness>` to leave it alone
and work somewhere else.

If you believe nothing is running, look at the sockets: `docker exec
omh-<repo>-s01 ls /omh/sock`. One per live harness, removed when it exits.

### `` `--dry-run` is omh's flag, not claude's ``

Everything after a harness name is the harness's argv, so `omh claude --dry-run`
handed omh's flag to claude and launched for real. omh's own flags go first:
`omh --dry-run claude`. If the harness genuinely has a flag of the same name,
`omh claude -- --dry-run` passes it on.

Long forms only. `-s` and `-a` are left alone — plenty of harnesses use them, and
refusing those would break launches that work.

### `omh s rm` says the session "is not a working tree"

Worktree registration and the directory on disk disagreed. omh prunes before
adding and falls back to removing the directory outright; if you hit this,
`git worktree prune` in the main checkout clears it.

### `no usable base manifest` or `declares no base-set entries`

omh could not find a readable [base set](design/base-set.md) in `~/.omh/base`,
or the newest one names nothing. `omh init` reinstalls it.

This is deliberately loud. It used to be silent: a stray `.toml` in that
directory became the base set, `init` seeded no MCP servers and reported success
anyway, and every session came up running hooks that pointed at a server which
was not installed.

### `a hook does nothing without `run` or `inject``

A file in a `hooks/` directory is not a usable hook: truncated, half-written, or
written in a harness's words rather than omh's. Related messages from the same
check: ``unknown field `event` `` (that is Claude Code's vocabulary — see
[writing a hook](configuration.md#writing-a-hook)), ``a hook either `run`s
something or `inject`s text, not both``, and ``$` in `inject` must name a
variable`.

Loud on purpose, and checked when the file is read rather than at runtime. An
unparseable hook used to be reported as *"modified by you"* with a blank value —
a false accusation from the one command whose job is telling authorship
straight — while a launch failed hard on the same file.

### `` `graph-refresh` is a name omh ships ``

You have a hook file answering to one of omh's own names. It does not override
omh's and it does not run, so it is refused rather than left inert. Rename it —
or, if what you want is omh's version gone, switch the feature off:

```toml
# <repo>/.omh/settings.toml
[omh]
codegraph = false
```

### A skill I have is not reaching the agent

Check the launch output. If it names the entry, this repo has a `[use]` list and
that entry is not in it:

```console
$ omh claude
omh: 1 catalogue entry is not selected here: skills/refactor
omh:   omh use skills refactor    ·    omh use --all
```

`omh init` writes `[use]` with every entry named, so anything added to your
catalogue *afterwards* is off here until you say otherwise. That is the trade an
explicit list makes, and this line is what stops it being silent. `omh repo`
shows the same thing without launching.

### `mcp/codegraph is omh's`

`codegraph` and `memory` are in `~/.omh/mcp.json` because `omh init` seeded them
there, so they look exactly like servers you added. They are not selectable in
either direction: a feature is its server, its hooks and its rules section
together, and keeping half of it is the one combination that manufactures
confident wrong answers.

```toml
# <repo>/.omh/settings.toml
[omh]
codegraph = false
```

or `omh repo disable codegraph`. Nothing is uninstalled and the next repo gets
it back.

### `--layer is going away`

`omh config set --layer shared` still works and prints the form that replaced
it. Two scopes, two commands: `omh config` is you, `omh repo` is this checkout,
and they want opposite defaults — what a project *uses* is committed, what it
*overrides* is not. See [Configuration](configuration.md#two-scopes-two-commands).

### `your catalogue has no skills called …`

`omh use` names an entry that has to exist, so a typo is refused rather than
written and reported at the next launch. `omh config edit skills <name>` creates
one.

The mirror of it: `omh unuse` refuses a name this repo was not using, instead of
writing the list back unchanged and reporting success.

### `a repo names servers from your catalogue, it cannot declare one`

There is an `mcp.json` in `<repo>/.omh/`. That was where servers lived before
the catalogue, and nothing reads it now. `omh config mcp add` puts one in your
catalogue; a token for this repo alone goes under `[mcp.<name>.env]` in
`.omh/settings.local.toml`.

### `keys.toml is where key templates used to live`

Rename it to `memory.toml`. omh refuses rather than falling back, because the
fallback is the disaster: the shipped defaults would silently re-key every note
written from then on, and every existing key would stop being derivable from
anything.

### `omh why` says something is not installed when it is

Check the message for a path. A layer that cannot be **read** — wrong
permissions, a broken symlink — is now an error naming the file rather than an
empty layer.

If you hit the old behaviour on an older build, the tell is that `omh init`
appears to do nothing: it uses `write_if_absent`, sees the file exists, and
leaves it alone, so the advice and the problem never meet.

### `Device or resource busy` editing a file omh carried in

`carry_in` files are bind-mounted rather than copied, so `git clean -fdx` in the
sandbox cannot delete them — a mountpoint cannot be unlinked. The same property
blocks every write-temp-then-rename edit, which is what `sed -i` and `mv` do:

```console
$ sed -i s/OLD/NEW/ .env
sed: cannot rename ./sedXXXXXX: Device or resource busy
```

Appending works (`echo LINE >> .env`), and so does anything that writes the file
in place. Edits land on omh's staged copy, never on the file in your checkout.

This is a trade, and the obvious way out was measured and rejected. Tracking the
carried file in the sandbox's repository instead of mounting it would survive
`git clean` *and* leave `sed -i` working — but the harvest fetches that
repository into yours, and a fetch takes every reachable object, so the secret
would be copied into your real repository on every `omh s commit --keep`.
A test pins it so nobody fixes the visible half.

A carried **directory** is a plain copy and has none of this — it is removable
by `git clean -fdx`. omh warns when it carries one, though only when it actually
copies: relaunch a session whose carried directory has not changed and the
warning does not repeat.

### `omh will not move a branch the session is not on`

`omh s commit --keep` refuses when the session's worktree has left its branch —
a `git checkout` to look at something, or an abandoned bisect. Put it back:

```console
$ git -C ~/.omh/worktrees/<repo>/<session> checkout omh/<session>
```

Related refusals from the same command, all meaning "a harvest here would
silently drop work", all about the sandbox's **own** repository: a detached
HEAD, an interrupted rebase or merge, and commits no branch there can reach.
The detached and stranded cases each print a `git --git-dir=…` command — one to
put it back, one to show you what it found. The interrupted case names the
marker it saw and leaves finishing or aborting to you.

### `omh will not rewrite your history to hide a secret`

`--keep` found something you listed in `carry_in` inside a commit the agent
made — added with `git add -f`, copied under another name, pasted into source,
or written into a commit message. omh knows the bytes it carried in, so it can
tell, and it stops rather than quietly rewriting the agent's work.

Drop that commit in the sandbox and harvest again, or take the files without the
history with `omh s commit -m`. The branch is untouched either way.

### Sessions are piling up

```console
$ omh s ls
$ omh s down s01
```

N sessions is N containers. `idle_timeout` stops unused ones; see
[Sessions](sessions.md#lifecycle).

### The graph shows a project I do not recognise

Graphs are shared per repo across sessions, so `list_projects` shows all of
them. Expected, and [documented](code-graph.md#what-the-agent-can-still-see) —
your session's own project name is in `$OMH_GRAPH_PROJECT`.

### Something else

The things most likely to be wrong are the things omh asserts about other
people's software. If you are debugging one of those, start with `omh doctor`,
then read the relevant page in [design](design/adapters.md) — several of them
record a failure that looked exactly like the one you are probably having.
