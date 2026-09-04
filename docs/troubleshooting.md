# Troubleshooting

## `omh doctor`

```console
$ omh doctor
checking claude in omh/claude:8eae0d5c1511fa89 — no account, so credentials go unchecked…
  ✓  container runtime   docker — answering, and every sandbox omh builds and runs uses it
  ✓  stacks detected     rust (from Cargo.toml)
  ✓  settings omh reads  every key set here is one omh reads
  ✓  leftovers           none — nothing orphaned on this machine
  ✓  seeded by           version 0.9.0, the one running now
  ✓  disk                76.6 GB free on the filesystem holding /Users/you/.omh — …
  ✓  git on the host     git version 2.55.0 — takes a `--keep` selection; syncs
  ✓  rules               /work/CLAUDE.md
  ✓  skills              /home/agent/.claude/skills
  …
```

The first seven rows are the **host's**, gathered before any container work — so
on a machine that cannot build a sandbox they are still what you get, instead of
a single error. The rest are the adapter paths, checked inside the sandbox, and
they are the reason the command exists. An eighth host row appears only when
this repo has no commit for a session branch to fork from.

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
$ omh new opencode
omh: opencode on omh/s01 — dropped hooks: git-note (no `session-start` moment),
     graph-first (no `search` tool),
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

### `up?` in `omh s`, or `omh could not tell whether the sandbox is running`

The container runtime is installed — omh checked before it asked — but it would
not answer. Almost always the daemon is not running: start Docker Desktop, or
`systemctl start docker`. `omh s` prints the runtime's own message above the
table, one line per session, and that message is the part worth reading.

**Nothing acts on the answer while it is unknown**, but what *that* means
differs by command, because the safe direction does:

- `sync`, `graph` and a launch **refuse**, naming what the runtime said.
- `down` leaves the session alone, reports it as a row rather than omitting it,
  and exits non-zero. It says *could not be asked*, not *would not stop* — omh
  never tried, so it cannot claim the container refused.
- `rm` removes the session anyway, and warns that the code graph's entry for it
  was left behind. Nothing else ever drops that entry.
- the idle reaper goes the other way on purpose: a session it cannot ask about
  is never stopped for being idle. Leaving a sandbox up costs a container;
  stopping a live one on a guess costs somebody's turn.

The refusals earn their keep. omh used to read *the runtime would not answer*
as *the container is not running*, so with the daemon down `omh s` showed
live sandboxes as `stopped` — and `omh sNN sync` believed there was nothing to
stop, which would have written trunk's files over the work of an agent
mid-turn.

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

### `omh could not tell whether s01's sandbox is still usable`

Before reusing a running sandbox, omh runs one command inside it — that command
answers both *can this be entered* and *what is running in it*. It acts on the
answer only when docker names the failure, and refuses otherwise.

Two failures it acts on. The worktree the container is bound to was deleted, or
deleted and recreated, so no `exec` will ever work again (the next section) —
the container has to go. Or the container is not there at all: removed, or
exited while omh was looking. Both mean nothing is alive inside to lose, so omh
replaces the sandbox and the launch carries on.

Anything else — the daemon restarting mid-launch, an image with no shell, a
fork that failed — is a question omh cannot answer, and the cost of answering
it wrongly is a `docker rm -f` on a container with an agent working inside. So
it stops and shows you what the runtime said, along with two ways on:

- run the launch again, which is enough if the runtime has come back;
- `omh sNN down`, which stops the container without needing to enter it, so the
  next launch builds a fresh one.

`omh sNN rm` is **not** the way out here, though an earlier version of this page
said so. It deletes the worktree as well as the container, and when omh cannot
read the sandbox it will not guess what would be lost: on a terminal it asks
before going ahead, and with nobody to ask it refuses. Either way the answer
that gets past it destroys the work you came here to keep.

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
`omh s01 down` if you want it gone, or `omh new <harness>` to leave it alone
and work somewhere else.

If you believe nothing is running, look at the sockets: `docker exec
omh-<repo>-s01 ls /omh/sock`. One per live harness, removed when it exits.

### A flag went to omh when you meant the harness

`omh new claude --dry-run` is a dry run *of omh* — everything before the `--`
is omh's. Put the flag after the separator to hand it to the harness:

```console
$ omh new claude -- --dry-run
```

This used to be an error rather than a rule. The bare-name form had no
separator, so omh guessed: it refused its own long flags typed after a harness
name and left short ones alone, since `-s` is a flag plenty of harnesses use.
`omh new` does not guess, so there is nothing left to refuse.

### `omh s rm` says the session "is not a working tree"

Worktree registration and the directory on disk disagreed. omh prunes before
adding and removes the directory outright when git will not, so this should no
longer reach you.

### omh is using more disk than I expected

`omh doctor`'s **leftovers** row is the inventory, and `omh prune` is what acts
on it. Between them they cover the six keyed things omh writes: cache volumes,
containers, networks, the per-checkout directories under `~/.omh`, and the
`tmp.*` remnants of operations that did not finish.

`omh --dry-run prune` shows what would go without removing anything.

Expect the first run on an established machine to report far more **left** than
removed, and to say `omh could not attribute` about most of it. That is not the
command failing. Until 0.9.0 omh keyed a checkout's state by a one-way digest
of its path and recorded the path nowhere, so for anything created before that
there is nothing to compare against. omh recovers what it can from evidence it
already wrote — a worktree's `.git` pointer, an image's build label — and
refuses to guess about the rest.

The rest is not stuck. `omh prune --dangerously-include-unsafe` names every one
of them with its reason and asks before removing any.

### `sNN is partly removed — its worktree is still there`

`rm` asks the disk whether the worktree actually went rather than trusting
git's exit code, and this is it saying no. The command exits non-zero and lists
what it did observe going, so nothing is claimed that was not watched.

Something is holding the directory: a shell still `cd`'d into it, an editor or
a file watcher with it open, a mount that has gone read-only, or a parent whose
permissions changed. Close whatever it is and run `omh sNN rm` again — the
parts that already went are gone, and re-running finishes the rest.

`--force` does not help here. It answers the question about unreviewed work; it
does not make the removal try harder, and `git worktree remove --force` was
passed either way.

### `moved this checkout's … off ` — upgrading to 0.8.0

Expected, once, and nothing is lost.

Before 0.8.0 omh keyed a checkout's state by its **directory name**, so
`~/work/api` and `~/oss/api` were one repo: they shared worktrees, sandbox
repositories, the note store, the cache volume and container names, and the
second one's `omh new` resumed into the first one's session. That is
[risk 8d](design/risks.md), and 0.8.0 closes it by keying on the name *and* a
digest of where the checkout is.

The first omh command you run in each checkout after upgrading moves its state
onto the new key and says so:

```console
$ omh s
omh: moved this checkout's worktrees off `proj` — keyed by checkout now, so two projects of the same name no longer share them
  s01  omh/s01  stopped
```

`omh info --repo` shows the key it chose, as `keyed as proj-561662ee`. That is
also the answer to *which `docker ps` row belongs to this project*.

The cache volume and network are **not** moved — they are derivable, docker
cannot rename either, and the old pair is left behind. Nothing reports them;
`docker volume ls | grep omh-cache-` finds them if you want the disk back.

### `omh will not move … — it cannot establish that it is this checkout's`

Two checkouts with the same directory name shared one worktrees directory, and
omh will not decide which of them gets it:

```console
$ omh s
omh: omh will not move `~/.omh/worktrees/api` — it cannot establish that it is this checkout's:
    ~/oss/api owns one
    ~/work/api owns one
  Two checkouts named `api` shared one directory before this version, and sessions from both can be in there. Move the ones you want by hand, or remove what you do not.
no sessions
```

This is the normal shape of the collision rather than an exotic one: session
ids were handed out by scanning that shared directory, so the first checkout
took `s01` and the second took `s02` — **in the same directory**. Whichever
checkout omh sampled would have taken the other's sessions with it, including
work no branch has.

Each session's `.git` file names the checkout it belongs to. Read them, move
the ones you want into `~/.omh/worktrees/<name>-<digest>/` for the checkout
that owns them — `omh info --repo` prints that name — and delete the rest.
The same message appears if a pointer cannot be read at all, naming the file:
omh treats *cannot look* as a refusal rather than as permission.

### `a sandbox from before this version is still running`

The move renames the directory a live container has mounted. `omh s down`
first, then run any omh command again.

### `… are from before omh keyed them by checkout, and it already has newer ones`

State under the old key that cannot move, because this checkout already has
state under the new one. Nothing reads it again. omh will not merge two sets
of sessions together, so it names them and leaves them in `~/.omh` for you to
keep or delete.

### `the carried-file scan could not read these`

At launch and at harvest, omh searches the agent's commits for lines from the
files you [carried in](configuration.md#carry_in) — and this says which of
those files it could not turn into lines:

```console
omh: the carried-file scan could not read these, so it cannot tell you whether
     their contents reached a commit:
    certs/deploy.p12 — it is not text, so there are no lines to search for
    certs/short.env — every line in it is too short or is a comment
  the path itself is still checked — what is not is a copy under another name
```

Not an error, and the harvest still lands. It is the difference between *no
carried secret reached the branch* and *omh could not check*, which used to be
the same sentence. The path check still applies; what it cannot see is the
same content copied to a different name.

Nothing to do about it in most cases. If a file matters, carrying it in a form
the scan can read — text, lines of twelve characters or more — is what buys
the content check.

### `is inside your checkout, and eject will not write there`

`omh eject` renders `AGENTS.md` and `CLAUDE.md`, which are files it *reads* to
compose the rules document — so writing into the checkout would overwrite its
own input. Point `--to` somewhere outside and copy in what you want.

### an unknown-issuer error when omh builds an image

**omh now says this for you, from both ends.** A build that dies on an
unverifiable certificate ends by naming `ca_cert` and the command that sets it.
Doctor's *guest-side* checks cannot answer that one — they work by launching
the image and inspecting it from the inside, and behind an inspecting proxy
there is no image to launch — so the diagnosis lives in the build itself.

And `omh doctor` now says it *before* anything fails. With no `ca_cert` set, it
asks whether a container would accept the certificates this network serves, by
verifying against the roots the platform ships rather than the ones your
machine trusts — which is the same question a container asks. That catches the
case the build cannot: an image cached from before the proxy appeared still
builds, and only the sessions fail.

It asks about two of the hosts a build fetches from — `github.com`, where the
graph binary comes from in the base layer, and `registry.npmjs.org`, where the
harness does — sampled rather than exhaustive, because a proxy can inspect
selectively and one host is a coin flip. Either one being re-signed is enough,
and the warning names which.

It needs an `openssl` that restricts verification to the file it is given.
Stock macOS ships LibreSSL, which does not, so omh checks that first and says
it cannot tell rather than reporting a clean network it did not measure.

It reports only when the answer is yes. Offline, or anywhere omh cannot tell a
shipped root from an installed one, it says nothing rather than guessing —
being told to install a corporate root on a plane is worse than silence.

The wording depends on which tool reaches the network first — `unable to get
local issuer certificate` from curl, `CERTIFICATE_VERIFY_FAILED` from python,
`UNABLE_TO_VERIFY_LEAF_SIGNATURE` from node — but the cause is one thing: your
network inspects TLS. Zscaler, Netskope and corporate MITM proxies terminate
every HTTPS connection and re-sign it with the company's own root. Your machine
trusts it because IT installed it; a container does not. The graph download and
`npm install -g` for the harness both fail, so no image gets built.

Get the root as a PEM — on macOS it lives in the keychain rather than on disk —
and point omh at it:

```console
$ security find-certificate -a -c "Zscaler" -p > ~/corp-root.pem
$ omh set --local ca_cert ~/corp-root.pem
$ omh init
$ omh doctor
```

`--local` puts it in `.omh/settings.local.toml`, which is gitignored. **Not
`omh settings set`** — that writes the template new repos are seeded from, and
a repo that already exists never re-reads it.

`omh doctor` is the step worth not skipping: it checks the root reached the
sandbox's trust store, which is the one thing a successful build does not
prove. `update-ca-certificates` exits 0 when it skips a certificate it cannot
parse.

[`ca_cert`](configuration.md#ca_cert) has the detail, including which
toolchains need telling separately from the system store.

If the file you have is a DER `.crt` rather than PEM, omh says so and gives you
the `openssl` line that converts it. If it carries `Bag Attributes` or
`friendlyName:` preamble from a `pkcs12` export, omh refuses it rather than
editing your certificate — strip it to the `BEGIN`/`END` block.

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
$ omh new claude
omh: 1 catalogue entry is not selected here: skills/refactor
omh:   omh use skills refactor    ·    omh use --all
```

`omh init` writes `[use]` with every entry named, so anything added to your
catalogue *afterwards* is off here until you say otherwise. That is the trade an
explicit list makes, and this line is what stops it being silent. `omh info --repo`
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

or `omh set codegraph off`. Nothing is uninstalled and the next repo gets
it back.

### `--dry-run is not something this command can answer yet`

`--dry-run` runs everything and withholds the writes, so it only means anything
on a command that writes. `init`, `auth`, `graph`, `settings edit`,
`memory promote` and the session verbs except `resume` refuse it instead of
running: each would have to compute what it *would* do — which container to
stop, which commits to replant, which worktree to delete — and a preview that
guessed would be worse than none.

Read-only commands refuse it too, for the opposite reason. `omh info` is its own
dry run; accepting the flag would promise a preview it never gives.

Run the command without the flag, or read what it would touch with
`omh info --repo`.

### `unexpected argument '-a' found`

`-a`/`--account` was removed in 0.7.0. It overrode the account for one
invocation and recorded nothing — so a session started with it could not be
resumed without repeating it, and forgetting meant the account mount no longer
matched the container's stamp, which either blocked the resume or brought the
container back as a different account.

The account is one thing with one spelling now: `omh auth <harness> -n <name>`
captures it, `omh set account <name>` chooses it, and every command that
launches or probes reads that. `omh set account` refuses a name no captured
login answers to, so a typo is caught where you type it rather than as a failed
login inside a sandbox.

### `unexpected argument '--layer' found`

`--layer` was removed in 0.7.0 along with the command that carried it. You no
longer pick the file — the key does. `carry_in` is kept out of git because a
value there can name a credential; everything else is committed, because it is
a fact about the project a teammate cloning should get. `omh why <key>` says
which a key is, and `--save` or `--local` overrides it for one write. See
[Configuration](configuration.md#two-scopes-two-commands).

### `your catalogue has no skills called …`

`omh use` names an entry that has to exist, so a typo is refused rather than
written and reported at the next launch. `omh settings edit skills <name>` creates
one.

The mirror of it: `omh unuse` refuses a name this repo was not using, instead of
writing the list back unchanged and reporting success.

### `a repo names servers from your catalogue, it cannot declare one`

There is an `mcp.json` in `<repo>/.omh/`. That was where servers lived before
the catalogue, and nothing reads it now. `omh settings mcp add` puts one in your
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
`git clean` *and* leave `sed -i` working — but a tracked file is in the tree of
every commit that follows it, and the harvest fetches that repository into
yours, so the secret would be copied into your real repository by every harvest
that gets that far. Fetching less does not help: `--depth=1` still carries the
blob, because it is in the tree of the commit it does fetch.
On a harvest that then *fails*, omh keeps the fetched ref on purpose so nothing
is lost — which would leave the secret reachable from a live ref until someone
noticed. A test pins it so nobody fixes the visible half.

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

### `the sandbox's history no longer reaches …`

`omh s commit --keep` replays from the point it last handed over. An agent that
`reset --hard`s below that point — or rebases across it — leaves a record the
sandbox's history no longer contains, and omh cannot tell which commits are new
from what is left. Replaying from the start of the session would offer the
branch work it already has.

The branch is untouched. Take the files as they stand with `omh s commit -m`,
which does not read the sandbox's repository at all.

### `omh will not rewrite your history to hide a secret`

`--keep` found something you listed in `carry_in` inside a commit the agent
made — added with `git add -f`, copied under another name, pasted into source,
or written into a commit message. omh knows the bytes it carried in, so it can
tell, and it stops rather than quietly rewriting the agent's work.

Drop that commit in the sandbox and harvest again, or take the files without the
history with `omh s commit -m`. The branch is untouched either way.

### Sessions are piling up

```console
$ omh s
$ omh s01 down
```

N sessions is N containers. `idle_timeout` stops unused ones; see
[Sessions](sessions.md#lifecycle).

### `omh: removed omh/claude:… this build replaced`

A build removes the images it supersedes. omh tags an image with a hash of its
recipe, so once a recipe changes the previous tag of the same kind is dead and
nothing will ask for it again. Before this, nothing removed them.

"Of the same kind" is the load-bearing part, and it is decided by labels omh
stamps at build time — `omh.kind`, plus the adapter, plus the checkout for a
stack layer — not by the image name. `omh/claude` legitimately holds the
harness layer *and* one toolchain layer per checkout, all of them current, so
the name alone cannot tell you which are dead.

Never removed: `:latest`, and any tag a container still references.

Two limits worth knowing. Images built before omh stamped labels carry none, so
they belong to no class and are never collected — `docker images 'omh/*'` will
show them and they are yours to remove. And a superseded tag usually frees far
less than its `SIZE` column suggests, because it shares almost every layer with
the tag that replaced it; the space is returned when a whole chain dies.

### `docker image rm` is not what filled my disk

Buildkit's cache is separate and `omh` never touches it. `docker system df`
shows it; `docker buildx du` breaks it down. On the machine that prompted this
feature it was 8.8 GB against 14.86 GB of images. Only `docker builder prune`
reclaims it, and it takes every cached layer with it, so omh leaves that
decision to you.

### The graph shows a project I do not recognise

Graphs are shared per repo across sessions, so `list_projects` shows all of
them. Expected, and [documented](code-graph.md#what-the-agent-can-still-see) —
your session's own project name is in `$OMH_GRAPH_PROJECT`.

### Something else

The things most likely to be wrong are the things omh asserts about other
people's software. If you are debugging one of those, start with `omh doctor`,
then read the relevant page in [design](design/adapters.md) — several of them
record a failure that looked exactly like the one you are probably having.
