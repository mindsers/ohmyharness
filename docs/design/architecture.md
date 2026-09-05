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
| bind-mount a single **file** | yes | **unknown** | staging must mount dirs + symlink instead |
| choose the **guest path** | yes | **no** — workspaces mount at the host path | the `/work` convention breaks |
| SSH attach for IDE | yes (sshd in image) | **unknown** | [editors](../editors.md) need another mechanism |

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
