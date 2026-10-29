---
title: Troubleshooting
description: Find the message or behaviour you are seeing and fix it, starting with omh doctor.
---

Find what you are seeing and fix it — starting with `omh doctor`, the one check
that proves a session's setup is right.

## `omh doctor`

Run it first, in the repository whose sessions misbehave:

```console
$ omh doctor
checking claude in omh/claude:8eae0d5c1511fa89 — no account, so credentials go unchecked…
host
  ✓  container runtime   docker — answering, and every sandbox omh builds and runs uses it
  ✓  stacks detected     rust (from Cargo.toml)
  ✓  settings omh reads  every key set here is one omh reads
  ✓  leftovers           none — nothing orphaned on this machine
  ✓  seeded by           version 0.15.0, the one running now
  ✓  disk                76.6 GB free on the filesystem holding /Users/you/.omh — …
  ✓  git on the host     git version 2.55.0 — takes a `--keep` selection; syncs
  ✓  declared config     resolves
  ✓  use                 every name in [use] resolves in the catalogue
  ✓  repo hooks          every .json file in .omh/hooks parses
  ✓  carry_in            nothing declared

claude in omh/claude:8eae0d5c1511fa89
  ✓  starts      claude --version
  ✓  git         git merge-tree -h 2>&1 | grep -q merge-base || { git --version 2>&1; exit 1; }
  ✓  rules       /work/CLAUDE.md
  ✓  skills      /home/agent/.claude/skills
  …
```

**Run it** after changing an adapter, after upgrading a harness, and any time a
session behaves as though your profile is not there. A preview —
`omh doctor --dry-run` — builds nothing; it prints the probe script.

### Reading the report

- **`host`** rows are gathered before any container work — so on a machine that
  cannot build a sandbox they are still what you get, instead of a single
  error. The last four read what this checkout declares — that its settings
  resolve, that every `[use]` name is in the catalogue, that every file in
  `.omh/hooks` parses, that every `carry_in` path exists — which needs no
  container either. One more host row appears only when this repo has no commit
  for a session branch to fork from.
- **`starts`** comes first under the harness's heading: the harness printing its
  own version, in the image a launch uses.
- **`git`** is second, and is about the image rather than the adapter — whether
  its git can merge, which is what a repo's own turn-end check will need.
- **Every row after those two is an adapter path**, checked inside the sandbox.
  They are the reason the command exists.

**The first `omh doctor` on a machine cross-builds the memory server**, the same
Linux binary a launch mounts, and says `first run only` while it does. How many
rows you get depends on your setup — [Current status](#current-status) says
what moves the count.

<details>
<summary>Why <code>starts</code> comes first, and why the first run builds a server</summary>

No path row can notice a harness that dies before reading any of them — which
is how opencode 1.18 failed every launch while every path row passed.

Before the first run built the memory server, `doctor` asked a server it had
not put in the container and reported it as not answering.

</details>

## Find your problem

Look for the message or the behaviour, and follow the link. If nothing matches,
see [Something else](#something-else).

| What you see | What to do |
|---|---|
| **Building and starting a session** | |
| `the sandbox cannot see package.json, which is in this repo` from `omh init` or `omh upgrade` | [Let the container runtime see your checkout](#the-sandbox-cannot-see--which-is-in-this-repo) |
| `unable to get local issuer certificate`, `CERTIFICATE_VERIFY_FAILED` or `UNABLE_TO_VERIFY_LEAF_SIGNATURE` while omh builds an image | [Give omh your network's root certificate](#an-unknown-issuer-error-when-omh-builds-an-image) |
| `up?` in `omh s`, or `could not tell whether s01's sandbox is running` | [Start the container runtime](#up-in-omh-s-or-could-not-tell-whether-s01s-sandbox-is-running) |
| `network omh-<repo>-<session> not found` | [A launch plan that could not run](#network-omh-repo-session-not-found) |
| `create mountpoint for /work/AGENTS.md mount: ... is outside of rootfs` | [Update omh, or launch again](#create-mountpoint-for-workagentsmd-mount--is-outside-of-rootfs) |
| A flag you meant for the harness went to omh | [Put it after `--`](#a-flag-went-to-omh-when-you-meant-the-harness) |
| **Reusing a session's sandbox** | |
| `omh could not tell whether s01's sandbox is still usable` | [Launch again, or take the session down](#omh-could-not-tell-whether-s01s-sandbox-is-still-usable) |
| `current working directory is outside of container mount namespace root`, or `possible container breakout detected` | [Replace the container](#current-working-directory-is-outside-of-container-mount-namespace-root) |
| `restarting the sandbox for omh/s01 — …` | [Not an error](#restarting-the-sandbox-for-omhs01--) |
| `session s01 is running opencode and cannot be reused for this launch` | [Stop it, or work somewhere else](#session-s01-is-running-opencode-and-cannot-be-reused-for-this-launch) |
| **Logging in** | |
| You logged in, and the next session is logged out | [Find where the token was written](#i-logged-in-but-the-next-session-is-logged-out) |
| Codex says "Login using `codex login` and then run this command again" | [Upgrade, then log in with a device code](#codex-says-login-using-codex-login-and-then-run-this-command-again) |
| **Codex** | |
| `Failed to set trust for /work … failed to persist config.toml` | [Update omh and upgrade](#codex-says-failed-to-set-trust-for-work--failed-to-persist-configtoml) |
| `could not find bubblewrap on PATH`, or `bwrap: No permissions to create a new namespace` | [Update omh and upgrade](#codex-says-could-not-find-bubblewrap-on-path-and-commands-fail-with-bwrap-no-permissions-to-create-a-new-namespace) |
| A server is `disabled` with `requirements (/etc/codex/requirements.toml)`, or a repo's `.codex/config.toml` hooks do not run | [Put it in your catalogue](#a-server-is-disabled-by-requirements-etccodexrequirementstoml) |
| Codex does not start offline on an enterprise account | [Expected](#codex-does-not-start-offline-on-an-enterprise-account) |
| A launch stops on `provider.toml`, or `omh doctor`'s `model provider` row is `✗` | [Fix the account's provider file](#a-launch-stops-on-providertoml) |
| A launch says `… selects model_provider = …, and sessions no longer read it from there` | [Keep it as the account's provider](#a-launch-says-sessions-no-longer-read-model_provider-from-there) |
| A launch says `… is in use by omh-…, started before codex's config was per session` | [Stop that session, then launch](#a-launch-says-the-account-is-in-use-by-a-session-started-before) |
| A Codex setting from one session is gone in the next | [Expected, unless it is a preference](#a-codex-setting-from-one-session-is-gone-in-the-next) |
| **Claude Code** | |
| `claude mcp add` says "not allowed by enterprise policy" | [Put it in your catalogue](#claude-mcp-add-says-not-allowed-by-enterprise-policy) |
| claude.ai connectors are missing in a session | [Expected](#claudeai-connectors-are-missing-in-a-session) |
| A `/mcp` switch from one session is gone in the next, and a `/login` keeps only its tokens | [Expected](#a-mcp-switch-from-one-session-is-gone-in-the-next-and-a-login-keeps-only-its-tokens) |
| **omp** | |
| A launch says `… models.yml defines … omh now hides it behind the account's provider file` | [Keep them, or remove the file](#a-launch-says-the-accounts-own-models-file-defines-providers) |
| A launch stops on `provider.yml`, or `omh doctor`'s `model provider` row is `✗` | [Fix the account's provider file](#a-launch-stops-on-provideryml) |
| `omh auth --provider` says a file `is under … .omh, which a session can write` | [Check it, copy it out, name the copy](#a-launch-says-the-accounts-own-models-file-defines-providers) |
| A session cannot write `~/.omp/agent/models.yml` (`Read-only file system`) | [Expected](#a-launch-says-the-accounts-own-models-file-defines-providers) |
| **Your setup in the session** | |
| The agent does not see your rules or skills | [Run `omh doctor`, then read the launch line](#the-harness-starts-but-does-not-see-my-rules-or-skills) |
| One skill is missing, and the launch says `1 catalogue entry is not selected here` | [Select it with `omh use`](#a-skill-i-have-is-not-reaching-the-agent) |
| `mcp/codegraph is omh's` | [Switch the whole feature instead](#mcpcodegraph-is-omhs) |
| `list_projects` shows a project you do not recognise | [Expected](#the-graph-shows-a-project-i-do-not-recognise) |
| **Attaching an editor** | |
| `nvim did not open the session`, or any other editor | [Recreate the session's container](#nvim-did-not-open-the-session--and-neither-does-any-other-editor) |
| **Carried files** | |
| `the carried-file scan could not read these` | [Not an error](#the-carried-file-scan-could-not-read-these) |
| `Device or resource busy` from `sed -i` or `mv` on a carried file | [Edit it in place](#device-or-resource-busy-editing-a-file-omh-carried-in) |
| **Getting work out of a session** | |
| `omh will not move a branch the session is not on` | [Put the worktree back on its branch](#omh-will-not-move-a-branch-the-session-is-not-on) |
| `… is a repository of its own inside this session's worktree` | [Remove its `.git`, or ignore it](#-is-a-repository-of-its-own-inside-this-sessions-worktree) |
| `the sandbox's history no longer reaches …` | [Take the files with `commit -m`](#the-sandboxs-history-no-longer-reaches-) |
| `omh will not rewrite your history to hide a secret` | [Drop the commit, or take the files](#omh-will-not-rewrite-your-history-to-hide-a-secret) |
| **Disk and cleanup** | |
| Sessions are piling up | [Stop the ones you are not using](#sessions-are-piling-up) |
| omh is using more disk than you expected | [Read the inventory, then prune](#omh-is-using-more-disk-than-i-expected) |
| Removing images did not give the disk back | [Buildkit's cache is separate](#docker-image-rm-is-not-what-filled-my-disk) |
| `omh: removed omh/claude:… this build replaced` | [Not an error](#omh-removed-omhclaude-this-build-replaced) |
| `omh sNN rm` says the session "is not a working tree" | [What it meant](#omh-snn-rm-says-the-session-is-not-a-working-tree) |
| `sNN is partly removed — its worktree is still there` | [Close what holds it, then remove again](#snn-is-partly-removed--its-worktree-is-still-there) |
| **Your catalogue and settings** | |
| `no usable base manifest`, or `declares no base-set entries` | [Run `omh init`](#no-usable-base-manifest-or-declares-no-base-set-entries) |
| `` a hook does nothing without `run`, `inject` or `refuse` ``, or ``unknown field `event` `` | [Write the hook in omh's words](#a-hook-does-nothing-without-run-inject-or-refuse) |
| `` `graph-refresh` is a name omh ships `` | [Rename your hook, or switch the feature off](#graph-refresh-is-a-name-omh-ships) |
| `your catalogue has no skills called …` | [Check the name, or create the entry](#your-catalogue-has-no-skills-called-) |
| `a repo names servers from your catalogue, it cannot declare one` | [Move the servers to your catalogue](#a-repo-names-servers-from-your-catalogue-it-cannot-declare-one) |
| `keys.toml is where key templates used to live` | [Rename it to `memory.toml`](#keystoml-is-where-key-templates-used-to-live) |
| `omh why` fails naming a file it cannot read | [Fix the file it names](#omh-why-fails-naming-a-file-it-cannot-read) |
| `is inside your checkout, and eject will not write there` | [Eject somewhere outside it](#is-inside-your-checkout-and-eject-will-not-write-there) |
| **Command-line errors** | |
| `--dry-run is not something this command can answer yet` | [Run the command without it](#--dry-run-is-not-something-this-command-can-answer-yet) |
| `unexpected argument '-a' found` | [Set the account instead](#unexpected-argument--a-found) |
| `unexpected argument '--layer' found` | [Drop the flag](#unexpected-argument---layer-found) |
| **After upgrading from an older omh** | |
| `moved this checkout's … off` | [Expected, once](#moved-this-checkouts--off---upgrading-to-080) |
| `omh will not move … — it cannot establish that it is this checkout's` | [Sort the sessions out by hand](#omh-will-not-move---it-cannot-establish-that-it-is-this-checkouts) |
| `a sandbox from before this version is still running` | [Stop the sandboxes, then retry](#a-sandbox-from-before-this-version-is-still-running) |
| `… are from before omh keyed them by checkout, and it already has newer ones` | [Keep them or delete them](#-are-from-before-omh-keyed-them-by-checkout-and-it-already-has-newer-ones) |

## Common failures

### Building and starting a session

#### `the sandbox cannot see …, which is in this repo`

```text
[provision] left as it was — the sandbox could not be asked (exit status: 96): the sandbox cannot see package.json, which is in this repo — the repo is not mounted where its conditions run
```

**Your container runtime cannot read this checkout's path.** omh asks the
sandbox which of a stack's provides apply with your repo mounted into it, and
the first thing it checks there is the file the stack was detected by —
`package.json` for node, `Cargo.toml` for rust. It was not there, so the
container was looking at an empty directory. A remote `DOCKER_HOST` cannot see
your files, and nor can a runtime in a VM that shares some of your directories
with it and not this one.

<div class="steps">

1. Make the checkout's path visible to the container runtime — share it with
   the VM, or work from a path the runtime already sees.

2. Run `omh upgrade` again.

</div>

<details>
<summary>Why omh records nothing rather than an answer</summary>

Run there, every condition would have answered "does not apply", and a second
look would have taken every provide out of your committed settings. omh records
nothing instead. `omh init` says the same in its own words — `the sandbox could
not be asked (exit status: 96) — nothing recorded`, followed by the line naming
the file.

</details>

#### an unknown-issuer error when omh builds an image

The wording depends on which tool reaches the network first — `unable to get
local issuer certificate` from curl, `CERTIFICATE_VERIFY_FAILED` from python,
`UNABLE_TO_VERIFY_LEAF_SIGNATURE` from node. A build that dies this way ends by
naming `ca_cert` and the command that sets it. `omh doctor` can warn before
anything fails — which is how you find out when an image cached from before the
proxy appeared still builds, and only the sessions fail.

**The cause is one thing: your network inspects TLS.** Zscaler, Netskope and
corporate MITM proxies terminate every HTTPS connection and re-sign it with the
company's own root. Your machine trusts it because IT installed it; a container
does not. The graph download and `npm install -g` for the harness both fail, so
no image gets built.

<div class="steps">

1. Get the root as a PEM.

   On macOS it lives in the keychain rather than on disk:

   ```console
   $ security find-certificate -a -c "Zscaler" -p > ~/corp-root.pem
   ```

   If the file you have is a DER `.crt` rather than PEM, omh says so and gives
   you the `openssl` line that converts it. If it carries `Bag Attributes` or
   `friendlyName:` preamble from a `pkcs12` export, omh refuses it rather than
   editing your certificate — strip it to the `BEGIN`/`END` block.

2. Point omh at it.

   ```console
   $ omh set --local ca_cert ~/corp-root.pem
   ```

   `--local` puts it in `.omh/settings.local.toml`, which is gitignored. **Not
   `omh settings set`** — that writes the template new repos are seeded from,
   and a repo that already exists never re-reads it.

3. Build the image again.

   ```console
   $ omh init
   ```

4. Check that the root reached the sandbox.

   ```console
   $ omh doctor
   ```

   This is the step worth not skipping: it checks the root reached the
   sandbox's trust store, which is the one thing a successful build does not
   prove. `update-ca-certificates` exits 0 when it skips a certificate it cannot
   parse.

</div>

[`ca_cert`](/reference/configuration/#ca_cert) has the detail, including which
toolchains need telling separately from the system store.

<details>
<summary>How omh spots it — in the build, and in <code>omh doctor</code> before anything fails</summary>

**omh says this for you, from both ends.** The build names `ca_cert` itself
because doctor's *guest-side* checks cannot answer this one — they work by
launching the image and inspecting it from the inside, and behind an inspecting
proxy there is no image to launch — so the diagnosis lives in the build itself.

**`omh doctor` says it *before* anything fails.** With no `ca_cert` set, it asks
whether a container would accept the certificates this network serves, by
verifying against the roots the platform ships rather than the ones your
machine trusts — which is the same question a container asks. That catches the
case the build cannot: the cached image that still builds.

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

</details>

#### `up?` in `omh s`, or `could not tell whether s01's sandbox is running`

The container runtime is installed — omh checked before it asked — but it would
not answer. Almost always the daemon is not running.

<div class="steps">

1. Read what the runtime said.

   `omh s` prints the runtime's own message above the table, one line per
   session, and that message is the part worth reading.

2. Start the daemon.

   Start Docker Desktop, or `systemctl start docker`.

</div>

**Nothing acts on the answer while it is unknown**, but what *that* means
differs by command, because the safe direction does.

<details>
<summary>What each command does while the runtime cannot be asked, and why</summary>

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

</details>

#### `network omh-<repo>-<session> not found`

The plan named a per-session network that was never created.

<details>
<summary>Why this one is the case for <code>omh doctor</code></summary>

A plan must be *runnable*, not merely well-formed — this gap made every real
launch die while every unit test passed, and it is the archetypal case for why
`doctor` exists.

</details>

#### `create mountpoint for /work/AGENTS.md mount: ... is outside of rootfs`

Fixed in `v0.2.1`. omh mounts its rules onto `/work/CLAUDE.md` and
`/work/AGENTS.md` — paths inside the worktree mount — and left creating those
destinations to the runtime, which Docker Desktop will not do.

**Update omh.** On an older version, launching a second time is the workaround.

<details>
<summary>Why it looked intermittent</summary>

`/work` is the host worktree, so Docker Desktop resolves the destination back to
a host path and refuses to create a mountpoint outside the container's rootfs.

Docker creates the empty file on the host on its way out, which is why this read
as intermittent: the first launch of a session died, the second found the
leftover and worked. omh places those files itself now, before docker sees the
plan.

</details>

#### A flag went to omh when you meant the harness

`omh new claude --dry-run` is a dry run *of omh* — everything before the `--`
is omh's. Put the flag after the separator to hand it to the harness:

```console
$ omh new claude -- --dry-run
```

<details>
<summary>Why omh no longer refuses the other spelling</summary>

This used to be an error rather than a rule. The bare-name form had no
separator, so omh guessed: it refused its own long flags typed after a harness
name and left short ones alone, since `-s` is a flag plenty of harnesses use.
`omh new` does not guess, so there is nothing left to refuse.

</details>

### Reusing a session's sandbox

#### `omh could not tell whether s01's sandbox is still usable`

Before reusing a running sandbox, omh runs one command inside it — that command
answers both *can this be entered* and *what is running in it*. It acts on the
answer only when docker names the failure, and here docker did not, so omh
stopped and showed you what the runtime said.

<div class="steps">

1. Run the launch again.

   That is enough if the runtime has come back.

2. If it still stops, take the session down.

   `omh sNN down` stops the container without needing to enter it, so the next
   launch builds a fresh one.

</div>

:::caution[Not `omh sNN rm`]
`omh sNN rm` is **not** the way out here, though an earlier version of this page
said so. It deletes the worktree as well as the container, and when omh cannot
read the sandbox it will not guess what would be lost: on a terminal it asks
before going ahead, and with nobody to ask it refuses. Either way the answer
that gets past it destroys the work you came here to keep.
:::

<details>
<summary>Which failures omh repairs on its own, and why it stops on the rest</summary>

Two failures it acts on. The worktree the container is bound to was deleted, or
deleted and recreated, so no `exec` will ever work again
([below](#current-working-directory-is-outside-of-container-mount-namespace-root))
— the container has to go. Or the container is not there at all: removed, or
exited while omh was looking. Both mean nothing is alive inside to lose, so omh
replaces the sandbox and the launch carries on.

Anything else — the daemon restarting mid-launch, an image with no shell, a
fork that failed — is a question omh cannot answer, and the cost of answering
it wrongly is a `docker rm -f` on a container with an agent working inside. So
it stops and shows you what the runtime said, along with the two ways on above.

</details>

#### `current working directory is outside of container mount namespace root`

Docker's full wording is `OCI runtime exec failed: ... -- possible container
breakout detected`, which is alarming and misleading: nothing broke out. The
session container is running with `/work` bound to a worktree directory that no
longer exists, so every command into that container fails the same way.

Fixed in `v0.3.1`: a launch that finds a container it cannot enter replaces it
instead of exec'ing into it, and says so:

```console
omh: restarting the sandbox for omh/s01 — it can no longer reach its worktree
```

The worktree and branch are on the host, so the restart costs nothing.

On an older version, replace the container yourself:

<div class="steps">

1. Remove the container.

   `docker rm -f omh-<repo>-<session>`

2. Launch the session again.

</div>

<details>
<summary>Why recreating the directory does not help, and what <code>v0.3.1</code> changed</summary>

Recreating the directory does not help — a bind mount follows the inode, not the
path.

`v0.3.1` fixed it from both ends: `omh sNN rm` now takes the container down with
the worktree, which is what created the mismatch, and a launch replaces the
container rather than exec'ing into it, as above.

</details>

#### `restarting the sandbox for omh/s01 — …`

**Not an error.** The container under that session id was not built from the
plan you just asked for — a different harness, a different account, a changed
mount set — and no `exec` can retrofit any of those. The line names what moved.

Nothing to do: the worktree and branch are on the host and the graph is in a
volume, so the restart costs seconds and loses nothing.

`it predates this check` means the container was started by a version of omh that
did not stamp its plan, so nothing about it can be verified. It happens once per
session after upgrading.

#### `session s01 is running opencode and cannot be reused for this launch`

The same mismatch as [`restarting the sandbox`](#restarting-the-sandbox-for-omhs01--),
but something is live inside and restarting would kill it. Choose one:

- **To stop it:** `omh s01 down`, if you want it gone.
- **To leave it alone:** `omh new <harness>`, and work somewhere else.

If you believe nothing is running, look at the sockets: `docker exec
omh-<repo>-s01 ls /omh/sock`. One per live harness, removed when it exits.

### Logging in

#### I logged in, but the next session is logged out

The token was written somewhere that does not persist. `omh doctor` names it:

```
  ✗ token      /home/agent/.claude.json cannot be renamed over —
               a token saved here will not persist
```

Why a token can fail to persist, and how omh mounts credentials so that it
does: [Accounts](/guides/accounts/#mount-the-directory-never-the-token-file).

#### Codex says "Login using `codex login` and then run this command again"

That line is Codex 0.5.0's, the pin omh shipped before 0.154.0. Its
`codex login` signs in through a browser redirect that waits inside the
sandbox, where your browser never arrives, so `omh auth codex` could not
finish that way and a launch started logged out.

<div class="steps">

1. [Update omh](/getting-started/installation/#keep-it-up-to-date).

2. Move the pin.

   Run `omh upgrade`.

3. Log in again, one of two ways.

   ```console
   $ omh auth codex                     # choose "Sign in with Device Code"
   $ omh auth codex --import            # or copy the login this machine has
   ```

   With `account` set for the repo, use that name: `omh auth codex --name <it>`.

</div>

See [Accounts](/guides/accounts/#a-login-that-cannot-finish-in-a-sandbox) for
both ways in.

### Codex

#### Codex says "Failed to set trust for /work … failed to persist config.toml"

omh 0.13.0 mounted its MCP servers over `~/.codex/config.toml`, and Codex
rewrites that file itself — here to save that it trusts `/work` — by renaming a
temporary file over it, which a mounted file refuses.

<div class="steps">

1. [Update omh](/getting-started/installation/#keep-it-up-to-date).

2. Run `omh upgrade`.

</div>

The servers move to `/etc/codex/managed_config.toml`, `~/.codex/config.toml` is
the account's own again, and `/etc/codex/config.toml` already trusts `/work`, so
Codex does not ask.

#### Codex says "could not find bubblewrap on PATH", and commands fail with `bwrap: No permissions to create a new namespace`

Codex runs each command inside its own bubblewrap sandbox, which cannot start
inside omh's container. omh 0.13.1 and earlier left it on, so every shell
command and every file edit failed.

<div class="steps">

1. [Update omh](/getting-started/installation/#keep-it-up-to-date).

2. Run `omh upgrade`.

</div>

Codex's sandbox is then turned off in `/etc/codex/config.toml`, and omh's
container is the sandbox.

#### A server is disabled by `requirements (/etc/codex/requirements.toml)`

omh writes Codex's requirements on every launch, and they admit exactly the MCP
servers the launch staged: yours that this repo selects, and omh's own. A
server added inside a session with `codex mcp add`, one declared in a repo's
`.codex/config.toml`, or one left in your account's `~/.codex/config.toml` by
another repo's session is not one this launch staged, so it loads disabled — in
every later session too, since each launch writes its requirements afresh from
what it stages. (A server in your catalogue that this repo does not select is
not handed to Codex at all.) The same file runs managed hooks
only: omh's, from `/etc/codex/config.toml`. A repo's `.codex/config.toml`
hooks and the ones in your account do not run (read in Codex's source). omh
names what a repo's `.codex/config.toml` declares at launch and in `omh doctor`.

To use a server in sessions, put it in your catalogue and select it — see
[`[use]`](/reference/configuration/#use--what-this-repo-takes-from-your-catalogue).
A hook goes in `.omh/hooks/` or your catalogue, in omh's words.

#### Codex does not start offline on an enterprise account

An enterprise account (a ChatGPT login on a Business, Enterprise or Edu plan)
caches its organisation's cloud bundle in `~/.codex`. A session could forge
that cache, so omh hides it behind a read-only file: Codex fetches the bundle
at every start instead, and with no network it does not start. That is what
Codex's source says it does; no enterprise account has been run to measure it.
omh names a cached bundle at launch and in `omh doctor`.

#### A launch stops on `provider.toml`

The account's [provider file](/guides/accounts/#a-model-provider-of-your-own)
holds something omh will not put in Codex's System layer, or is not TOML:

```
omh: /Users/you/.omh/creds/codex/work/provider.toml
  because `notify` is not a provider key — a provider file holds `model_provider` and `[model_providers.<id>]` only, since omh renders it into the layer Codex treats as managed
```

Every launch on the account stops until it is fixed, `omh doctor` among them.
Write it again from a file that holds the provider:

```console
$ omh auth codex --name work --provider ~/.codex/config.toml
```

A field that reads the environment (`env_key`), runs a command (`auth`) or
reads an AWS profile has to be settled on the host, which `--provider` does —
or, for a command, refuses: put the token it produces in the file by hand.

#### A launch says sessions no longer read `model_provider` from there

The account's shared `~/.codex/config.toml` selects a model provider, and the
first launch after upgrading is about to make that file
[each session's own](/guides/accounts/#codexconfigtoml-is-each-sessions-own),
keeping only preferences. A provider is not one, and nothing else on the
account holds it, so every later session would send its turns to OpenAI.
Nothing has changed yet:

```
omh: /Users/you/.omh/creds/codex/work/.codex/config.toml selects `model_provider = "azure"`, at https://example.openai.azure.com/openai, and sessions no longer read it from there — this launch would go to OpenAI instead. Keep it as the account's provider:

    omh auth codex --name work --provider /Users/you/.omh/creds/codex/work/.codex/config.toml

or remove `model_provider` from that file to use OpenAI
```

Run the command it names: it reads the provider out of that same file into the
account's [provider file](/guides/accounts/#a-model-provider-of-your-own), and
the next launch migrates. Read the `base_url` first — a session could have
written that file. For the same reason nothing in it is read from your
environment: an `env_key` there is refused. Check the file, copy it out of
`~/.omh`, and name the copy.

#### A launch says the account is in use by a session started before

A session an older omh started still mounts the account, and still reads the
shared `config.toml` — the migration would leave it reading a link it cannot
follow, and its next save would rename over the link:

```
omh: /Users/you/.omh/creds/codex/work is in use by omh-api-3f9a2c1b-s02, started before codex's config was per session — stop it first (`omh sNN down`), and launch again
```

Stop it — `omh s02 down` in the repo it belongs to, or `docker rm -f` the name
— and launch again. Once an account has migrated this is never asked again.
With a runtime that keeps no labels on its sandboxes (`sbx`) omh cannot tell
which are running, and says so instead: codex on `sbx` waits for #199.

#### A Codex setting from one session is gone in the next

Expected, unless it is a preference. A session's `config.toml` is its own: a
server added with `codex mcp add`, a feature toggled, `developer_instructions`,
a `[profiles]` table last that session. Your
[preferences](/guides/accounts/#codexconfigtoml-is-each-sessions-own) — the
model, its reasoning effort, the `tui` settings — reach the account when Codex
exits in your terminal and at `omh sNN down` and `rm`, and omh says which. A
server you want in every session goes in your catalogue
([`[use]`](/reference/configuration/#use--what-this-repo-takes-from-your-catalogue)).

### Claude Code

#### `claude mcp add` says "not allowed by enterprise policy"

omh writes a managed-settings drop-in on every launch,
`/etc/claude-code/managed-settings.d/omh.json`, and it admits exactly the MCP
servers the launch staged, by command and arguments. Inside a session
`claude mcp add` refuses a server not on that list (measured by hand on
2.1.300, at one scope), and one written into `~/.claude.json` by hand is left
out of `claude mcp list`. To use a server in sessions, put it in your catalogue
and select it.

An organisation that delivers its own managed `allowedMcpServers` replaces
omh's list (Claude Code's docs; not measured). If it leaves out one of omh's
servers, that omh feature does not load; `omh doctor`'s `mcp-loaded` row
names it.

#### claude.ai connectors are missing in a session

Expected: they are switched off in sessions, deliberately (read in the 2.1.300
binary; not measured). Claude Code matches a connector by its URL, and allowing
that URL would let a session put its own server there.

#### A `/mcp` switch from one session is gone in the next, and a `/login` keeps only its tokens

Expected: each session has its own `~/.claude.json`, seeded from the account's
and never written back — see
[`~/.claude.json` is each session's own](/guides/accounts/#claudejson-is-each-sessions-own).
To turn a server off for good, `omh unuse mcp <name>` — or `omh set memory off`
and `omh set codegraph off` for omh's own servers. A `/login` inside a session
replaces the account's shared tokens, but the identity it writes stays in that
session: to change who an account is, `omh auth claude --name <account>`; to
launch with another account, `omh set account claude:<name>`. If your account
carried `env` or `apiKeyHelper` in `.claude.json` (which 2.1.300 reads as a
legacy fallback: read in the binary, not measured), a session no longer gets
them.

### omp

#### A launch says the account's own models file defines providers

omp used to read its providers straight from the account's `models.yml`,
which every session on the account can rewrite. omh now
[keeps them in a file of the account's own](/guides/accounts/#omps-providers)
and mounts what it renders from that file over `models.yml`, read-only. The
account defines providers that file does not hold, so the launch stops rather
than hide them, and nothing has changed yet:

```
omh: /Users/you/.omh/creds/omp/work/.omp/agent/models.yml defines corp (https://llm.example.com/v1) — omp read it from the account, which any session can write, and omh now hides it behind the account's provider file, which holds none. If they are yours, keep them: `omh auth omp --name work --provider /Users/you/.omh/creds/omp/work/.omp/agent/models.yml`; if not, remove the file
```

Read the addresses first — a session could have written that file. If they
are yours, run the command it names. A key there that names an environment
variable, or one that runs a command, is refused, since nothing in a file
under `~/.omh` is read from your environment:

```
omh: /Users/you/.omh/creds/omp/work/.omp/agent/models.yml
  because it is under /Users/you/.omh, which a session can write, so nothing in it is read from your environment — check it, copy it out, and name the copy
  because `providers.corp.apiKey` reads `CORP_KEY` from the environment, and it is not set here — set it and import again, or write the value into a provider file and name it
```

A session cannot write `models.yml` — `Read-only file system` — so a provider
added there lasts nowhere. Add it to a `models.yml` of your own and run
`omh auth omp --provider` with it.

#### A launch stops on `provider.yml`

The account's provider file holds something omp's `models.yml` does not, a
command, or is not YAML omh reads:

```
omh: /Users/you/.omh/creds/omp/work/provider.yml
  because `providers.corp.apiKey` is a command (`!…`), which omp would run at every request — a provider file holds the value it prints instead
```

Every launch on the account stops until it is fixed, `omh doctor` among them.
Write it again from a `models.yml` that holds the providers:

```console
$ omh auth omp --name work --provider ~/.omp/agent/models.yml
```

### Your setup in the session

#### The harness starts but does not see my rules or skills

Either the adapter names a path the harness does not read, or the harness cannot
express that capability and omh dropped it at launch.

<div class="steps">

1. Run `omh doctor`.

   If a path fails there, the adapter is wrong — that is a bug, and
   [Adapters](/reference/adapters/) covers how to fix it.

2. If doctor passes, check whether the capability was dropped at launch.

   ```console
   $ omh new opencode
   omh: opencode on omh/s01 — dropped hooks: git-note (no `session-start` moment),
        graph-first (no `search` tool),
        graph-orient (no `session-start` moment),
        graph-read (no way to inject text before a tool runs)
   ```

   That is the harness genuinely not supporting the feature, not omh losing it.

</div>

If one entry is missing rather than a whole kind, see
[A skill I have is not reaching the agent](#a-skill-i-have-is-not-reaching-the-agent).

#### A skill I have is not reaching the agent

Check the launch output. If it names the entry, this repo has a `[use]` list and
that entry is not in it:

```console
$ omh new claude
omh: 1 catalogue entry is not selected here: skills/refactor
omh:   omh use skills refactor    ·    omh use --all
```

**Select it** with the line omh printed: `omh use skills refactor` for that
entry, or `omh use --all` for everything in your catalogue. `omh info --repo`
shows the same thing without launching.

`omh init` writes `[use]` with every entry named, so anything added to your
catalogue *afterwards* is off here until you say otherwise. That is the trade an
explicit list makes, and this line is what stops it being silent.

#### `mcp/codegraph is omh's`

`omh-codegraph` and `omh-memory` are omh's servers, generated at launch with
their features — not entries of your catalogue. They are not selectable in
either direction, and neither are `codegraph` and `memory`, the names they
had: a feature is its server, its hooks and its rules section together, and
keeping half of it is the one combination that manufactures confident wrong
answers.

**Switch the whole feature instead:**

```toml
# <repo>/.omh/settings.toml
[omh]
codegraph = false
```

or `omh set codegraph off`. Nothing is uninstalled and the next repo gets
it back.

#### The graph shows a project I do not recognise

**Expected.** Graphs are shared per repo across sessions, so `list_projects`
shows all of them — [documented](/guides/code-graph/#what-the-agent-can-still-see).
Your session's own project name is in `$OMH_GRAPH_PROJECT`.

### Attaching an editor

#### `nvim did not open the session` — and neither does any other editor

```console
$ omh s06 attach
omh: nvim did not open the session
session s06 is up

  ssh://omh-ohmyharness-c8a80c16-s06/work
  ssh omh-ohmyharness-c8a80c16-s06
```

Every editor `attach` offers is an ssh URL, so this is never really about the
editor: the container is refusing the connection, and each of the commands
printed under it fails the same way. omh's container dropped a capability sshd
needs, and `v0.14.1` gives it back.

<div class="steps">

1. [Update omh](/getting-started/installation/#keep-it-up-to-date) to `v0.14.1`
   or later.

2. Take the session down.

   `omh s06 down`. **A container created by an older omh keeps the old
   capability set**, because capabilities are fixed when a container is created
   and not when it starts.

3. Start it again.

   `omh s06 resume <harness>`. The worktree and everything in it are untouched.

</div>

<details>
<summary>How to confirm it, and which capability it was</summary>

`ssh -v` shows the connection closing right after `SSH2_MSG_KEXINIT sent`, and
the reason is in a log inside the container:

```console
$ docker exec -u root omh-<repo>-s06 tail -3 /omh/sshd.log
chroot("/run/sshd"): Operation not permitted [preauth]
```

sshd's unprivileged pre-auth process chroots to `/run/sshd`, and omh's
container dropped `SYS_CHROOT` — a capability its own list described as having
no caller. It was missing since the runtime seam landed, so every omh that
could attach at all had this.

</details>

### Carried files

#### `the carried-file scan could not read these`

```console
omh: the carried-file scan could not read these, so it cannot tell you whether
     their contents reached a commit:
    certs/deploy.p12 — it is not text, so there are no lines to search for
    certs/short.env — every line in it is too short or is a comment
  the path itself is still checked — what is not is a copy under another name
```

**Not an error, and the harvest still lands.** At launch and at harvest, omh
searches the agent's commits for lines from the files you
[carried in](/reference/configuration/#carry_in) — and this says which of those
files it could not turn into lines. The path check still applies; what it
cannot see is the same content copied to a different name.

Nothing to do about it in most cases. If a file matters, carrying it in a form
the scan can read — text, lines of twelve characters or more — is what buys
the content check.

<details>
<summary>Why omh says this at all</summary>

It is the difference between *no carried secret reached the branch* and *omh
could not check*, which used to be the same sentence.

</details>

#### `Device or resource busy` editing a file omh carried in

```console
$ sed -i s/OLD/NEW/ .env
sed: cannot rename ./sedXXXXXX: Device or resource busy
```

`carry_in` files are bind-mounted rather than copied, so `git clean -fdx` in the
sandbox cannot delete them — a mountpoint cannot be unlinked. The same property
blocks every write-temp-then-rename edit, which is what `sed -i` and `mv` do.

**Edit the file in place instead.** Appending works (`echo LINE >> .env`), and so
does anything that writes the file in place. Edits land on omh's staged copy,
never on the file in your checkout.

A carried **directory** is a plain copy and has none of this — it is removable
by `git clean -fdx`. omh warns when it carries one, though only when it actually
copies: relaunch a session whose carried directory has not changed and the
warning does not repeat.

<details>
<summary>Why omh mounts a carried file rather than tracking it</summary>

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

</details>

### Getting work out of a session

#### `omh will not move a branch the session is not on`

`omh sNN commit --keep` refuses when the session's worktree has left its branch —
a `git checkout` to look at something, or an abandoned bisect. Put it back:

```console
$ git -C ~/.omh/worktrees/<repo>/<session> checkout -q omh/<session>
```

Keep the `-q`. Without it `checkout` finishes by reporting what is still
changed, and for a directory the agent turned into a repository of its own that
report is git run inside it.

<details>
<summary>Related refusals from the same command</summary>

All of them mean "a harvest here would silently drop work", and all are about
the sandbox's **own** repository: a detached HEAD, an interrupted rebase or
merge, and commits no branch there can reach.

The detached and stranded cases each print a `git` command to run **inside the
sandbox** — one to put it back, one to show you what it found. Not on the host:
that repository's hooks are the agent's to write. The interrupted case names the
marker it saw and leaves finishing or aborting to you.

</details>

#### `… is a repository of its own inside this session's worktree`

`omh sNN commit`, `commit --keep` and `sync` refuse while the worktree holds a
directory with a `.git` of its own — the agent ran `git clone` or `git init`
inside `/work`, or did so in the empty directory of one of your submodules.

<div class="steps">

1. Inside the sandbox, delete its `.git`, or ignore it.

   Deleting the `.git` leaves the files as ordinary files; adding the path to
   `.gitignore` leaves it out. For one of your own submodules only the first
   applies.

2. Run the command again.

</div>

`omh s` and `omh sNN diff` keep working in the meantime. `omh s` lists the
directory as changed, and neither looks inside it.

<details>
<summary>Why omh will not commit a nested repository, or ask git about one</summary>

Two reasons, and the second is the one that matters. A commit cannot carry it:
git records a nested repository as a commit id and none of its files, and that
id exists nowhere but the sandbox. And git answers *is that repository dirty*
by running `git status` **inside it**, under its own config — a file the agent
wrote. So omh neither stages one nor asks about one.

`omh sNN commit` never stages a change to one of your submodule pointers either
way — the sandbox's repository has no submodules to update, so one that moved in
there moved to a commit only the agent has.

</details>

#### `the sandbox's history no longer reaches …`

`omh sNN commit --keep` replays from the point it last handed over. An agent that
`reset --hard`s below that point — or rebases across it — leaves a record the
sandbox's history no longer contains, and omh cannot tell which commits are new
from what is left. Replaying from the start of the session would offer the
branch work it already has.

**The branch is untouched.** Take the files as they stand with
`omh sNN commit -m`, which does not read the sandbox's repository at all.

#### `omh will not rewrite your history to hide a secret`

`--keep` found something you listed in `carry_in` inside a commit the agent
made — added with `git add -f`, copied under another name, pasted into source,
or written into a commit message. omh knows the bytes it carried in, so it can
tell, and it stops rather than quietly rewriting the agent's work.

The branch is untouched either way. Choose one:

- **Keep the history:** drop that commit in the sandbox and harvest again.
- **Take the files without the history:** `omh sNN commit -m`.

### Disk and cleanup

#### Sessions are piling up

N sessions is N containers.

<div class="steps">

1. List them.

   ```console
   $ omh s
   ```

2. Stop the ones you are not using.

   ```console
   $ omh s01 down
   ```

3. Let `idle_timeout` stop unused ones from now on.

   See [Sessions](/concepts/sessions/#lifecycle).

</div>

#### omh is using more disk than I expected

`omh doctor`'s **leftovers** row is the inventory, and `omh prune` is what acts
on it. Between them they cover everything omh keys by a checkout: cache volumes,
containers, networks, and the seven per-checkout directories under `~/.omh`
(`worktrees`, `shadow`, `memory`, `notes`, `run`, `keys`, `scratch`).

<div class="steps">

1. See what would go.

   `omh --dry-run prune` shows what would go without removing anything.

2. Remove it.

   `omh prune`.

3. Decide about what omh could not attribute.

   `omh prune --dangerously-include-unsafe` names every one of them with its
   reason and asks before removing any.

</div>

Expect the first run on an established machine to report far more **left** than
removed, and to say `omh could not attribute` about most of it. That is not the
command failing, and the rest is not stuck.

<details>
<summary>Why omh cannot attribute older state</summary>

Until 0.10.0 omh keyed a checkout's state by a one-way digest of its path and
recorded the path nowhere, so for anything created before that there is nothing
to compare against. omh recovers what it can from evidence it already wrote — a
worktree's `.git` pointer, an image's build label — and refuses to guess about
the rest.

</details>

#### `docker image rm` is not what filled my disk

Buildkit's cache is separate and `omh` never touches it. On the machine that
prompted this feature it was 8.8 GB against 14.86 GB of images.

<div class="steps">

1. See it.

   `docker system df` shows it; `docker buildx du` breaks it down.

2. Reclaim it, if you want to.

   Only `docker builder prune` reclaims it, and it takes every cached layer with
   it, so omh leaves that decision to you.

</div>

#### `omh: removed omh/claude:… this build replaced`

**Not an error.** A build removes the images it supersedes. omh tags an image
with a hash of its recipe, so once a recipe changes the previous tag of the same
kind is dead and nothing will ask for it again. Before this, nothing removed
them.

Never removed: `:latest`, and any tag a container still references.

**Images built before omh stamped labels are yours to remove.** They carry none,
so they belong to no class and are never collected — `docker images 'omh/*'`
will show them.

<details>
<summary>How omh decides an image is superseded, and why the space freed is small</summary>

"Of the same kind" is the load-bearing part, and it is decided by labels omh
stamps at build time — `omh.kind`, plus the adapter, plus the checkout for a
stack layer — not by the image name. `omh/claude` legitimately holds the
harness layer *and* one toolchain layer per checkout, all of them current, so
the name alone cannot tell you which are dead.

A superseded tag usually frees far less than its `SIZE` column suggests,
because it shares almost every layer with the tag that replaced it; the space is
returned when a whole chain dies.

</details>

#### `omh sNN rm` says the session "is not a working tree"

Worktree registration and the directory on disk disagreed. omh prunes before
adding and removes the directory outright when git will not, so this should no
longer reach you.

#### `sNN is partly removed — its worktree is still there`

`rm` asks the disk whether the worktree actually went rather than trusting
git's exit code, and this is it saying no. The command exits non-zero and lists
what it did observe going, so nothing is claimed that was not watched.

<div class="steps">

1. Close whatever is holding the directory.

   A shell still `cd`'d into it, an editor or a file watcher with it open, a
   mount that has gone read-only, or a parent whose permissions changed.

2. Run `omh sNN rm` again.

   The parts that already went are gone, and re-running finishes the rest.

</div>

`--yes` does not help here. It answers the question about unreviewed work; it
does not make the removal try harder, and `git worktree remove --force` was
passed either way.

### Your catalogue and settings

#### `no usable base manifest` or `declares no base-set entries`

omh could not find a readable [base set](/reference/base-set/) in `~/.omh/base`,
or the newest one names nothing. **Run `omh init`**: it reinstalls it.

<details>
<summary>Why this is loud</summary>

This is deliberately loud. It used to be silent: a stray `.toml` in that directory became the base set,
`init` seeded no MCP servers and reported success anyway, and every session
came up running hooks that pointed at a server which was not installed.

</details>

#### `` a hook does nothing without `run`, `inject` or `refuse` ``

A file in a `hooks/` directory is not a usable hook: truncated, half-written, or
written in a harness's words rather than omh's. Related messages from the same
check: ``unknown field `event` `` (that is Claude Code's vocabulary),
``a hook does one thing — `run` a command, `inject` advisory text, or `refuse` the call``,
`` `$(` in `inject` runs a command from inside a sentence ``, and
`… is not a variable name`.

**Rewrite the file in omh's words** — [writing a hook](/reference/configuration/#writing-a-hook)
has them — or remove it.

<details>
<summary>Why it is checked when the file is read</summary>

Loud on purpose, and checked when the file is read rather than at runtime. An
unparseable hook used to be reported as *"modified by you"* with a blank value —
a false accusation from the one command whose job is telling authorship
straight — while a launch failed hard on the same file.

</details>

#### `` `graph-refresh` is a name omh ships ``

You have a hook file answering to one of omh's own names. It does not override
omh's and it does not run, so it is refused rather than left inert. Choose one:

- **To keep your hook:** rename it.
- **To have omh's version gone:** switch the feature off.

  ```toml
  # <repo>/.omh/settings.toml
  [omh]
  codegraph = false
  ```

#### `your catalogue has no skills called …`

`omh use` names an entry that has to exist, so a typo is refused rather than
written and reported at the next launch. Check the name, or create the entry:
`omh settings edit skills <name>` creates one.

The mirror of it: `omh unuse` refuses a name this repo was not using, instead of
writing the list back unchanged and reporting success.

#### `a repo names servers from your catalogue, it cannot declare one`

There is an `mcp.json` in `<repo>/.omh/`. That was where servers lived before
the catalogue, and nothing reads it now.

<div class="steps">

1. Add each server to your catalogue.

   `omh settings mcp add` puts one there.

2. Move any token for this repo alone.

   It goes under `[mcp.<name>.env]` in `.omh/settings.local.toml`.

3. Delete `<repo>/.omh/mcp.json`.

   omh refuses for as long as the file is there.

</div>

#### `keys.toml is where key templates used to live`

**Rename it to `memory.toml`.**

<details>
<summary>Why omh refuses instead of reading it</summary>

omh refuses rather than falling back, because the fallback is the disaster: the
shipped defaults would silently re-key every note written from then on, and
every existing key would stop being derivable from anything.

</details>

#### `omh why` fails naming a file it cannot read

A layer that cannot be **read** — wrong permissions, a broken symlink — is an
error naming the file, not an empty layer. Fix the file it names.

<details>
<summary>On an older build</summary>

An older build read such a layer as empty and answered "not installed here"
about something that was, advising `omh init` — which did nothing, because it
saw the file existed and left it alone, so the advice and the problem never
met.

</details>

#### `is inside your checkout, and eject will not write there`

`omh eject` renders `AGENTS.md` and `CLAUDE.md`, which are files it *reads* to
compose the rules document — so writing into the checkout would overwrite its
own input.

<div class="steps">

1. Point `--to` somewhere outside the checkout.

2. Copy in what you want.

</div>

### Command-line errors

#### `--dry-run is not something this command can answer yet`

`--dry-run` runs everything and withholds the writes, so it only means anything
on a command that writes. `init`, `auth`, `graph`, `settings edit` and the
session verbs except `resume` refuse it instead of running, and so do read-only
commands.

**Run the command without the flag**, or read what it would touch with
`omh info --repo`.

<details>
<summary>Why these commands refuse rather than preview</summary>

Each of the writing commands would have to compute what it *would* do — which
container to stop, which commits to replant, which worktree to delete — and a
preview that guessed would be worse than none.

Read-only commands refuse it for the opposite reason. `omh info` is its own dry
run; accepting the flag would promise a preview it never gives.

</details>

#### `unexpected argument '-a' found`

`-a`/`--account` was removed in 0.7.0. The account is one thing with one
spelling now, and every command that launches or probes reads it:

<div class="steps">

1. Capture it.

   `omh auth <harness> -n <name>`

2. Choose it for this project.

   `omh set account <name>`. It refuses a name no captured login answers to, so
   a typo is caught where you type it rather than as a failed login inside a
   sandbox.

</div>

<details>
<summary>Why <code>-a</code> went</summary>

It overrode the account for one invocation and recorded nothing — so a session
started with it could not be resumed without repeating it, and forgetting meant
the account mount no longer matched the container's stamp, which either blocked
the resume or brought the container back as a different account.

</details>

#### `unexpected argument '--layer' found`

`--layer` was removed in 0.7.0 along with the command that carried it. You no
longer pick the file — the key does. (`omh memory rm --layer` is gone too, in
0.14: notes are one store per repo, so a key names one note.)

**Drop the flag.** `omh why <key>` says which file a key goes to, and `--save` or
`--local` overrides it for one write. See
[Configuration](/reference/configuration/#two-scopes-two-commands).

<details>
<summary>How the key decides</summary>

`carry_in` is kept out of git because a value there can name a credential;
everything else is committed, because it is a fact about the project a teammate
cloning should get.

</details>

### After upgrading from an older omh

#### `moved this checkout's … off ` — upgrading to 0.8.0

**Expected, once, and nothing is lost.** The first omh command you run in each
checkout after upgrading moves its state onto the new key and says so:

```console
$ omh s
omh: moved this checkout's worktrees off `proj` — keyed by checkout now, so two projects of the same name no longer share them
  s01  omh/s01  stopped
```

`omh info --repo` shows the key it chose, as `keyed as proj-561662ee`. That is
also the answer to *which `docker ps` row belongs to this project*.

**To get the disk back:** the cache volume and network are **not** moved — they
are derivable, docker cannot rename either, and the old pair is left behind.
Nothing reports them; `docker volume ls | grep omh-cache-` finds them.

<details>
<summary>Why the key changed</summary>

Before 0.8.0 omh keyed a checkout's state by its **directory name**, so
`~/work/api` and `~/oss/api` were one repo: they shared worktrees, sandbox
repositories, the note store, the cache volume and container names, and the
second one's `omh new` resumed into the first one's session. That is
[risk 8d](/concepts/risks/), and 0.8.0 closes it by keying on the name *and* a
digest of where the checkout is.

</details>

#### `omh will not move … — it cannot establish that it is this checkout's`

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

<div class="steps">

1. Find which checkout owns each session.

   Each session's `.git` file names the checkout it belongs to.

2. Find each checkout's new directory name.

   `omh info --repo`, run in that checkout, prints it.

3. Move the sessions you want.

   Into `~/.omh/worktrees/<name>-<digest>/` for the checkout that owns them.

4. Delete the rest.

</div>

The same message appears if a pointer cannot be read at all, naming the file:
omh treats *cannot look* as a refusal rather than as permission.

<details>
<summary>Why both checkouts' sessions are in one directory</summary>

This is the normal shape of the collision rather than an exotic one: session
ids were handed out by scanning that shared directory, so the first checkout
took `s01` and the second took `s02` — **in the same directory**. Whichever
checkout omh sampled would have taken the other's sessions with it, including
work no branch has.

</details>

#### `a sandbox from before this version is still running`

The move renames the directory a live container has mounted.

<div class="steps">

1. Stop the sandboxes.

   `omh s down`

2. Run any omh command again.

</div>

#### `… are from before omh keyed them by checkout, and it already has newer ones`

State under the old key that cannot move, because this checkout already has
state under the new one. Nothing reads it again. omh will not merge two sets
of sessions together, so it names them and leaves them in `~/.omh` for you to
keep or delete.

### Something else

The things most likely to be wrong are the things omh asserts about other
people's software. If you are debugging one of those, start with `omh doctor`,
then read the relevant reference page, starting with
[Adapters](/reference/adapters/) — several of them record a failure that looked
exactly like the one you are probably having.

## How `omh doctor` works

### Why it exists

**Factual correctness is not testable in process.**

Adapters assert things about *external software* — that Claude Code reads MCP
config from `/work/.mcp.json`, that skills live in `~/.claude/skills`. A green
unit suite proves omh mounts a path faithfully. It cannot prove anything reads
it, and that software ships weekly.

Almost every bug this project has shipped lived at that boundary, and **not one
was catchable by the test suite.** `doctor` is the only cure.

<details>
<summary>The <code>mcp</code> binding that no session ever loaded</summary>

The `mcp` binding is the cautionary tale. It said `$HOME/.mcp.json` — a path
Claude Code does not read and never did — and omh rendered a correct document,
mounted it faithfully, and reported `✓ mcp` for as long as that lasted. Not one
session ever loaded an MCP server. Checking a document proves the document; only
the harness can say whether it read it, which is what `mcp-loaded` asks and why
its check runs the harness's own `mcp list` inside the sandbox.

</details>

### What it does

It launches the real image with the real mounts and inspects the **guest** paths
the adapter declares. Checking anything host-side would just re-test the staging
directory omh wrote a moment earlier, which is circular.

Capabilities the harness cannot express are **skipped, not failed** — they were
already reported as dropped at launch.

#### The credential probe

This is the half no in-process test can reach: whether a token saved at a path
survives depends on how the runtime binds it, not on anything omh wrote. So the
probe attempts the temp-file-plus-rename that every token save performs.

It is **non-destructive by construction** — the file case copies the original
and renames byte-identical content back, so a successful probe changes nothing
and a failed one touches nothing. A health check that costs you your login would
be worse than no check.

Run against a file mount, it reports the real defect — the `✗ token` line under
[I logged in, but the next session is logged out](#i-logged-in-but-the-next-session-is-logged-out).

#### A silent probe is never a pass

If a probe produces no output, that means the sandbox never ran it. Calling that
success would make `doctor` worse than useless, so silence is a failure.

### Current status

All four shipped adapters — `claude`, `codex`, `opencode` and `omp` — are
verified this way, hooks included on each. The "unverified claim" caveat is
retired for them, and any further adapter inherits the same bar. Verified here
means the paths are right: only `claude` has been driven for real work — see
[Adapters](/reference/adapters/#the-bar-for-shipping-one).

The *number* of checks is not a property of the adapter: it counts the
capabilities your profile declares, plus a `token` check only when you have
captured a login for that harness, plus `memory` when the base set's server is
installed. Two harnesses showing different totals usually means you are logged
in to one of them.
