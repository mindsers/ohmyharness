# Architecture

**Status: built, except the second backend.** The on-disk layout, the image build and the runtime trait are all shipping. `sbx` is selectable and **unverified** — `runtime = "sbx"` is a valid setting, and `auto` never picks it, because nobody has measured it — but the spike that resolves file mounts, guest paths and IDE attach has not run, so Docker is the only runtime anyone has confirmed works.

How omh is put together: what lives where on disk, how images are built, and how
the runtime backend is kept swappable.

## On-disk layout

```
~/.omh/
  rules/ skills/ commands/    the catalogue — the only place content lives
  subagents/ hooks/ mcp.json
  default.toml                what a new repo starts from
  adapters/*.toml             one file per harness (data, not code)
  editors/*.toml              one file per editor
  base/                       the curated base set, versioned
  creds/<harness>/<account>/  captured logins, one directory per account
  worktrees/<repo>/<session>/ the agent's working directory
  shadow/<repo>/<session>.git the repository the sandbox gets, and `.seed` beside it
  keys/<repo>/                per-repo ed25519 keypair
  run/<repo>/<session>/<harness>/  staged profile, regenerated per launch
  sessions.json               session → container, branch, port

<repo>/.omh/
  settings.toml               COMMITTED — settings, and `[omh]`
  settings.local.toml         GITIGNORED — overrides, and MCP env
  memory.toml                 COMMITTED — how the note store keys and expires
  hooks/                      COMMITTED — the one content kind a repo may declare
<repo>/AGENTS.md              the project's own rules, tracked
```

Two details that are load-bearing rather than incidental:

- **Staging is keyed by session *and* harness.** Without the harness in that
  path, launching a second harness overwrites the config the first one has
  mounted, live.
- **Worktrees live outside the repo.** Nested, your IDE would index every
  session's full copy of the codebase.

Where things live is in [Configuration](../configuration.md#one-catalogue-and-it-is-personal).

## Images

Two layers: a base every session shares, and a thin per-harness layer that runs
the adapter's `install` command.

```dockerfile
# omh/base
FROM node:22-bookworm-slim
RUN apt-get install -y … git ripgrep dtach sudo curl jq socat
RUN usermod -l agent -d /home/agent -m node        # node:slim holds UID 1000
RUN test "$(id -u agent)" = "1000"                 # assert, do not assume
RUN mkdir -p /work /omh/sock /omh/cache /omh/layers
USER agent
```

```dockerfile
# omh/<harness>
FROM omh/base:<recipe-digest>
USER root
RUN <adapter.install>
USER agent
```

**The base satisfies the `sbx` kit contract on purpose** — `agent` at UID 1000,
passwordless sudo, `/home/agent`, proxy env forwarding. One image works on
either backend, and an sbx kit becomes a two-line file rather than a port.

Three properties, each with a test:

- **The contract is asserted at build time**, not assumed. If a future base image
  moves UID 1000, the build fails there instead of failing mysteriously inside a
  sandbox weeks later.
- **Images end unprivileged.** Installing needs root; running must not have it.
  An image that ends as root hands the agent the sandbox's own escape hatch.
- **Dockerfiles arrive on stdin** with an empty build context, so nothing is
  written to disk and no context is uploaded.

### Tags are recipe digests, never `:latest`

A mutable `:latest` on the base meant `ensure` skipped rebuilding it, so base
recipe changes had **never** shipped — adding `socat` silently did nothing.
Harness layers now pin an exact base digest, so a recipe change actually
propagates.

`init` builds these, because init is not finished until `omh new <harness>` works.
About 30 seconds on first run, cached after.

### A plan must be runnable, not merely well-formed

The network a plan names — one per session, named like its container, so two
sessions of one repo cannot reach each other's services — has to be *created*,
too. That gap made every real launch die at `network omh-<repo> not found`
(the per-project network of the time) while every unit test
passed — the archetypal case for [`omh doctor`](../troubleshooting.md).

### Verified end to end

Inside a real container, with the real mounts:

```
user:   agent uid=1000    home: /home/agent    cwd: /work
tools:  claude=ok dtach=ok git=ok rg=ok node=ok
  rules      # tdd / <repo>/AGENTS.md    (your catalogue, then the project's)
  skills     graphify, review-diff       (your catalogue)
  mcp        codegraph, filesystem, github, linear, omh-memory, sentry
  subagents  explorer.md
  hooks      rust-test, graph-refresh …  (translated into settings.json)
  version    2.1.222 (Claude Code)
```

Every capability class arrives, resolved from the catalogue, in the harness's own
format. That is the thesis demonstrated rather than argued.

## Runtime backends

Isolation is not ours to build. It is also not one vendor's to own.

The backend sits behind a narrow trait. `Plan` is already a pure description — a
mount list, env, and argv — so a backend is just a translation of that into one
process invocation.

```
        Plan  (pure: mounts, env, argv, workdir)
          │
    ┌─────┴─────┐
    ▼           ▼
  Docker       Sbx
  shared       microVM: own kernel, own dockerd,
  kernel       egress policy, keychain-backed
               credential injection
```

Selection is `runtime = "auto" | "docker" | "sbx"` in `settings.toml`. `auto`
selects only `docker`; `sbx` is an explicit opt-in until the spike below has
measured it, and `omh doctor` says so when it is chosen.

### Why not simply adopt `sbx`

It is better on every security axis — hypervisor isolation instead of a shared
kernel, and secrets injected at the egress proxy so **a compromised agent never
holds its own token.** omh currently mounts a credential volume, which means the
agent *can* read and exfiltrate it. That is a real flaw and `sbx`'s model is the
fix.

But a distribution whose opinion cannot be escaped is not trustworthy. Backends
stay plural.

### Is `sbx` harness-agnostic?

Yes — verified, not assumed. Docker's own docs
[demonstrate building a kit for Amp](https://docs.docker.com/ai/sandboxes/customize/build-an-agent/),
a third-party agent they do not ship. The four named agents are shipped *kits*,
not a constraint. A kit is barely more than an omh adapter:

```yaml
entrypoint:
  run: [amp, --dangerously-allow-all]
```

Base image requirements are four: a non-root `agent` user at UID 1000,
passwordless sudo, `/home/agent/` as home, and HTTP proxy env forwarding. omh's
`GUEST_HOME` is already `/home/agent`.

**An omh adapter can compile to an sbx kit.** Agnosticism expressed once.

### The seam

From the sbx FAQ:

> "Sandboxes don't import your complete user-level agent configuration. Hooks,
> settings, and other files under directories such as `~/.claude` remain on the
> host."

Docker drew its boundary exactly where omh's product begins. They isolate; they
deliberately do not carry your setup in. The two **compose**: `sbx` provides the
microVM, omh gets your profile inside it.

### The seam in the code

`runtime::Runtime` is pure: a `Plan` in, an argv out, and nothing in it ever
runs a process. `runtime::Backend` is the one place an argv becomes a process.
`runtime::select` returns a `Backend`, every command that shells out to the
runtime goes through `Backend::output`, and only the two things that need a
`Child` — an interactive attach and a build fed its Dockerfile on stdin — take
`program()` and spawn their own.

That split is what makes the launch path testable on a machine with no
container runtime. `Backend::scripted` answers each argv from a table and logs
what was asked, so `cmd::session`'s launch decisions — join the running
container, refuse to touch one the daemon will not describe, refuse to restart
one with a live harness inside, clear a stopped one before `run --name` — each
have a unit test, where before the seam they had a manual check against Docker
or nothing at all.

### Declared capabilities, and honest unknowns

Backends differ in ways that break a naive plan, so each **declares** what it can
do rather than failing mysteriously.

| Capability | Docker | `sbx` | Consequence if absent |
|---|---|---|---|
| bind-mount a single **file** | yes | **no** (measured 0.39.0) — a workspace must be a directory | staging must write into a workspace dir + symlink, not mount single files |
| choose the **guest path** | yes | **no** (measured) — a workspace mounts at its host path | the `/work` convention breaks; stage + symlink from inside |
| SSH attach for IDE | yes (sshd in image) | **no native SSH**; `exec` is the entry, and a published loopback port reaches a service inside (measured) | an sshd in omh's template on a published port should work; else `exec` ([#109](https://github.com/mindsers/ohmyharness/issues/109)) |

The unknowns are not hand-waved. A `Plan` is validated against the selected
backend's declared capabilities and fails **loudly** if it needs something the
backend lacks. Running that validation against `sbx`'s conservative capabilities
today is instructive — **every single mount omh makes is rejected**:

```
Error: the selected runtime cannot honour this plan:
  /work would have to mount at its host path /Users/…/worktrees/s01
  /home/agent/.claude/skills would have to mount at its host path …
  /home/agent/.mcp.json is a single-file mount
  …
```

That is the real blast radius. If `sbx` genuinely forces host-path mounts, the
staging model needs rework rather than a tweak: staged content would have to be
written into the workspace and symlinked from inside, not mounted onto chosen
guest paths.

Better to know that from a failing validation than from an agent that starts
fine and cannot see its own profile.

Resolving it is a one-afternoon spike — build an opencode kit, try a single-file
mount, try attaching an IDE — and it gates whether `sbx` becomes the default or
stays opt-in hardening. See [roadmap](roadmap.md).

### Spike: `sbx` 0.39.0, measured 2026-09-06

The section above was written against the `docker sandbox` Desktop plugin, which
is **removed** (`docker sandbox` now prints a migration notice). Docker
Sandboxes ships as a standalone CLI, `sbx`, installed with
`brew trust docker/tap && brew install docker/tap/sbx`. This spike measured
`sbx v0.39.0` on macOS 26 (Apple Silicon). What runs a *sandbox* needs
`sbx login` (interactive Docker OAuth) and `sbx daemon start`; the CLI surface
below was read without a login, and the runtime behaviours it raises are marked
as still needing one.

**Measured from the CLI (no login required):**

- **Agent-aware, not command-aware.** `sbx create AGENT PATH [PATH...]` and
  `sbx run AGENT …` take a *named* agent — `claude`, `codex`, `copilot`,
  `cursor`, `gemini`, `opencode`, `shell`, and others — not an arbitrary argv.
  `shell` is the bare sandbox omh would build on.
- **Workspaces are positional, `:ro` for read-only**, and extra workspaces are
  extra arguments (`sbx create claude . /docs:ro`). The guest path is not
  chosen — this confirms `free_guest_paths: false`.
- **`-t, --template IMAGE`** runs a chosen container image, and
  `sbx template save|load|ls|rm` manages them (`sbx template load` reads a tar).
  This is the enabler the old design lacked: **omh can hand sbx its own base
  image** — the one that already carries sshd, the memory server and the graph
  cache — rather than generating a kit. It removes most of the kit machinery the
  plan sketched.
- **`-p, --publish [[HOST_IP:]HOST_PORT:]SANDBOX_PORT[/PROTOCOL]`** on
  `create`/`run`, and `sbx ports SANDBOX --publish …` on a running one.
  Loopback by default (127.0.0.1), ephemeral host port when omitted — the same
  loopback-only posture omh's Docker `up_args` keep by hand.
- **`sbx exec [flags] SANDBOX COMMAND`** mirrors `docker exec` exactly: `-it`,
  `-d`, `-u root`, `-e/--env`. omh's `exec_args` maps onto it with only the
  program name changed.
- **`-e/--env` and `--env-file`** carry environment in, so `OMH_SESSION` and
  `OMH_GRAPH_PROJECT` ride along; **`-m/--memory` and `--cpus`** are the limits
  omh's `sandbox_memory`/`sandbox_cpus` already express.
- **Egress and credentials are first-class.** `--deny-network`, `--profile` and
  `sbx policy` govern egress; `sbx secret set|import` stores *service* secrets
  (github/anthropic/openai) that a proxy injects into API requests so **the
  secret never enters the sandbox filesystem** — the exact "a compromised agent
  never holds its own token" property this section wants, delivered by sbx
  rather than built by omh. Registry secrets (`sbx secret set --registry`) pull
  a private `--template` image without the credential entering the sandbox.
- **`--clone`** runs the agent on an in-container git clone of the host repo
  (mounted read-only), with the agent's commits reachable through a
  `sandbox-<name>` git remote on the host. This is a second, sbx-native answer
  to the isolation omh gets from a worktree plus harvest, and worth weighing
  against omh's own model rather than layering on top of it.
- Lifecycle verbs are all present: `ls`, `stop`, `rm`, `prune`, `cp`, `reset`,
  `diagnose`. `sbx diagnose` runs without a login and confirmed virtualization
  support and the daemon socket path.

**The candidate omh-on-sbx model, under these answers:**

1. `sbx template load` omh's base-image tar once, then
   `sbx create shell -t omh/base:<tag> -p <port>:22 -e OMH_SESSION=sNN <worktree>`
   — omh's own image, its own sshd, the worktree at its host path.
2. Credentials for an API-key login go through `sbx secret set`; an OAuth login,
   which is a file, rides a read-only host-path workspace as the weaker option,
   documented as such.
3. Egress is a `sbx policy` / `--deny-network` set seeded from the harness's
   known hosts plus a `sandbox_egress` setting.
4. The profile — skills, MCP, rules — is staged into the worktree and symlinked
   from inside at startup, because the guest path cannot be chosen.

**Measured with a login, sbx 0.39.0 (2026-09-06):**

- **A workspace mounts at its exact host path** — `sbx create shell <dir>` puts
  the files at `<dir>` inside the sandbox, `pwd` and all. The guest path is not
  chosen (`free_guest_paths: false`).
- **A workspace must be a directory** — a single-file path is refused with
  *"workspace path exists but is not a directory"* (`file_mounts: false`). So
  omh's per-file mounts (rules, `.mcp.json`) cannot be workspaces; they must be
  staged into a directory and symlinked from inside.
- **The default sandbox already presents `agent` at UID 1000, passwordless
  sudo, and `/home/agent`** — omh's base-image contract, met out of the box.
- **A published loopback port reaches a service inside.** `sbx ports NAME
  --publish 8099` bound `127.0.0.1:<ephemeral>`, and a listener on 8099 inside
  answered on the host — the same loopback bridge omh's Docker `up_args` build,
  so publishing sshd's 22 works the same way.
- **The home directory persists across `stop`/`run`** — a file written to
  `~/` survived a stop and the next exec's auto-start.
- **`-e/--env` carries in** — `OMH_SESSION=s01` read back inside.
- **`--template <image>` runs a custom image as-is** — plain `debian` came up as
  root with no sudo, so sbx does *not* add the agent/sudo layer; the image
  provides it. omh's base does, so it is a valid template: the omh-on-sbx model
  is `sbx create shell -t omh/base:<tag> -p <port>:22 <worktree>`.
- **First-run prerequisites**, beyond install: `sbx login` (a Docker account),
  `sbx daemon start`, and `sbx policy init <allow-all|balanced|deny-all>`
  before the first `create`. These are what `omh doctor` should detect and hand
  the user the exact next command for, rather than letting sbx's own errors
  surface.

**What remains** is the end-to-end: build omh's base, `sbx template load` it,
and run `omh doctor --harness claude` with `runtime = "sbx"` — the acceptance
test. The design is no longer guessed; `Sbx` in `runtime.rs` still needs its
argv rewritten to `create`/`exec`/`ports` and `Plan::validate` taught the
stage-and-symlink model, and until that ships `auto` never selects it.

**The rewrite is architectural, not an argv swap (measured 2026-09-06).**
Trying to map omh's Docker-shaped `Sbx` onto the real CLI surfaced four
mismatches that are the actual work, each verified end to end with a throwaway
image loaded into sbx:

1. **The image reaches sbx by `docker save | sbx template load`.** omh builds
   `omh/base:<tag>` in Docker; sbx runs in its own runtime and takes an image
   with `-t`. `sbx template load <tar>` loaded a locally-built image, and
   `sbx create shell -t <it>` **ran its ENTRYPOINT** — the stub wrote its marker
   and `sleep infinity` was PID 1, so an sshd started before that exec would
   persist. So omh's launch gains a save-and-load step before `create`.
2. **sbx has no labels.** `create`/`run` take no `--label`, so omh's
   `Plan::labels` drift-and-reuse — read back with `container_stamp` to decide
   attach-vs-restart — has no equivalent. A first cut recreates the sandbox each
   launch (`rm` then `create`); a fuller one records the plan stamp under
   `runs/<id>` and compares there.
3. **The workspace is at its host path, and `/work` must be a symlink.** `-e
   OMH_WORKTREE=<host path>` carries the path in (env carrying is confirmed),
   and the image entrypoint does `ln -s "$OMH_WORKTREE" /work`. The profile's
   single-file and chosen-guest-path mounts (rules, `.mcp.json`, skills) cannot
   be mounts at all — they stage into the worktree (or a second `:ro`
   workspace) and are symlinked from the entrypoint, which is the
   `stage-and-symlink` model `Plan::validate` must express for a backend with
   `file_mounts: false, free_guest_paths: false`.
4. **The running check is `sbx ls --json`, filtered by status.** `sbx ls -q`
   lists *all* sandboxes, running or stopped, so it cannot stand in for Docker's
   `ps --format {{.Names}}` (running only) — a stopped sandbox would read as up.
   `--json` returns `{"sandboxes":[…]}` with a `status`; the sbx path filters
   `status == "running"`. `sbx rm --force`, `sbx stop` map cleanly; `sbx exec
   [-it] NAME ARGV` is the harness entry.

So the `Sbx` rewrite touches `runtime.rs` (argv + a JSON running-check), the
base image (the symlink entrypoint, one rebuild), `image` (save-and-load
delivery), `container`/`Plan::validate` (stage-and-symlink, and reuse without
labels), and its acceptance is a real launch: `omh doctor --harness claude` with
`runtime = "sbx"`. The design is fully measured; none of it is guessed.

**Implemented (2026-09-06).** All four mismatches are now built and tested:

1. **Delivery** is `provide`/`provide_to_sbx` in `image`: build with docker,
   `docker save -o` to a tar, `sbx template load` it, idempotent through
   `Sbx::template_has` reading `sbx template ls --json`. The measured store row
   is registry-prefixed (`docker.io/omh/base`), which the parser matches on a
   path boundary.
2. **No labels** — the stamp is recorded in `runs/<id>/stamp.json` at create and
   read back by `stamp_recorded`; `Runtime::carries_labels` is the seam, and the
   reuse semantics (attach on no drift, restart on drift) are identical to the
   label path. The fuller option won over recreate-each-launch, so a live sbx
   agent is not killed on the next `resume`.
3. **Stage-and-symlink** — every guest path travels in `OMH_LINKS` (`guest host`
   per line) and the entrypoint symlinks each, `sudo` for a root-owned parent.
   `sbx_staging` turns mounts into host-path workspaces: a file mount stages its
   parent directory, a docker named volume (the graph cache) is dropped because
   sbx home persists across stop/run, and a workspace nested in another is
   dropped as redundant. `Plan::validate_for` skips the native-mount refusal for
   a staging backend.
4. **Running check** and `remove`/`network` are the runtime's own:
   `running_names` parses the JSON by status, `remove_args` is `rm … --force`,
   and `ensure_network` is a no-op because sbx isolates in a microVM.

Verified live against sbx 0.39.0: the `OMH_LINKS` entrypoint symlinks `/work`
and a root-owned path (through the sudo fallback) and reads the host content
through them; `docker save | sbx template load` lists the image and the parser
recognises it. The remaining acceptance is a full `omh new`/`omh doctor` launch,
which rebuilds the base image once (the entrypoint changed). `auto` still never
selects `sbx` — it stays an explicit opt-in behind the login and account gate.


