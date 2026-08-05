# ohmyharness (`omh`)

> oh-my-zsh for agentic coding.

```
$ omh init         # detects your stack, decides, reports. no questions.
$ omh claude       # sandboxed, curated, your setup already inside
$ omh code         # attach any IDE to the same session
```

The promise is **the best agentic coding environment without the hassle of
understanding, installing, and configuring everything.**

---

## 1. What omh is

A **distribution**. Not a framework, not an abstraction layer, not a marketplace.

Debian didn't write the kernel. Homebrew didn't write the software. oh-my-zsh
didn't write zsh — and its genius was never the plugin catalog, it was that
installing it gave you a good shell *immediately*.

omh assembles parts that already exist into a working, opinionated whole:

| Part | Who does it | omh's job |
|---|---|---|
| Isolation | `sbx`, Docker | choose one, wire it |
| Rules & skills portability | AGENTS.md, SKILL.md standards | place them correctly |
| Code graph | `codebase-memory-mcp` et al | pick one, index on init |
| Model routing | LiteLLM, OpenRouter | one env var |
| Curation | Claude marketplace, community | **subtract from it** |

**"It already exists" is the precondition for a distribution, not a refutation
of one.**

## 2. What omh is not

Anthropic's marketplace ships 200+ curated plugins into an ecosystem of
[23,600+ skills and 12,700+ MCP servers](https://claudemarketplaces.com/). That
is an **app store**, and an app store is structurally incapable of being
opinionated — the moment it picks winners, every excluded publisher becomes a
business problem.

| | App store | Distribution |
|---|---|---|
| Optimizes for | catalog size | working defaults |
| Success metric | items listed | **decisions removed** |
| Can it pick winners? | no | yes — that *is* the product |

23,600 skills is not an opinion. It is the problem statement. And it grows every
month, which means the case for a distro strengthens over time rather than
eroding.

**The product is subtraction.** 12 MCP servers, not 12,700. The metric is
decisions-to-productive, and the target is zero.

Corollary: "we support everything" is an anti-feature. Mechanisms may be broad;
the UX must not expose that breadth as choice.

---

## 3. Competitive position

| | curated | isolated | harness-neutral |
|---|---|---|---|
| Anthropic marketplace | ✓✓ | ✗ | ✗ |
| [Docker `sbx`](https://docs.docker.com/reference/cli/sbx/) | ✗ | ✓✓ | ✓ |
| [Sculptor](https://nimbalyst.com/compare/sculptor/) | ✗ | ✓✓ | ~ |
| [Conductor](https://nimbalyst.com/blog/best-agent-management-tools-2026/), Nimbalyst | ✗ | ✓ | ~ |
| Plexus, agent-rules-sync | ✗ | ✗ | ✓ |
| **omh** | — | ✓ | ✓✓ |

**No tool is both curated and harness-neutral.** That is the gap.

It sharpens the pitch: omh is not "a curated setup" — Anthropic will always beat
us at that on their own harness. omh is **your curated setup, anywhere.**
Isolation we don't build, curation we inherit and port, provenance so it stays
debuggable.

Sobering note: Vibe Kanban's company shut down in April 2026. This category has
already produced a casualty.

---

## 4. Decisions

| Decision | Choice | Why |
|---|---|---|
| Category | **distribution** | the parts exist; assembly and subtraction are the value |
| Runtime | **pluggable backend** | `sbx` where available, Docker as fallback; no vendor lock |
| Repo exposure | **git worktree, auto-branch** | agent cannot reach your checkout or `main`; review is `git diff` |
| Code graph | **wire an existing MCP server** | distros package, they don't reinvent |
| Language | **Rust** | single binary; `omh` wraps every invocation so startup is felt |
| LLM routing | **not ours** | one env var in `policy.toml` |
| Unit of work | **long-lived session** | keeps the index warm, makes harness switching instant |
| Session persistence | **`dtach`, not tmux** | omh needs detach/reattach; SSH already provides multiplexing |
| IDE access | **SSH into the session** | one dependency tree, shared with the agent |
| Untracked files | **`carry_in` allowlist** | worktrees are pure git; secrets enter only by declaration |
| Capability floor | **superset, adapters degrade** | omh must never cost you a feature you already had |
| Profile scope | **3 layers: personal / shared / local** | team sharing without leaking secrets |
| Write default | **the gitignored layer** | a mistyped key must not be committable |

---

## 5. The base set

This is the product. Everything else in this document is a place to put it.

`omh init` **decides and reports** — it never asks. Every question is hassle we
promised to remove; an unavoidable question means a missing default.

```
omh init          → base system. no questions.
omh add rust      → stack profile. a small curated delta.
omh mcp add …     → the archive is still there, one command away, not in your face.
```

Straw-man base — deliberately small enough to argue about:

| Component | Justification owed |
|---|---|
| sandbox + worktree branch | safety; non-negotiable |
| one code-graph MCP | must show a token win on `omh bench` |
| `omh-mcp memory` | survives harness switches |
| `AGENTS.md` from detected stack | the thing everyone writes badly |
| test-on-stop + format-on-edit hooks | catches the most common agent failure |
| `omh code` | IDE attach |
| egress allowlist | inherited from the runtime |

Seven. If an eighth needs a paragraph to justify, it belongs in a profile.

### What `omh init` does

```
1  repo check                     fail fast, before any work
2  ensure ~/.omh + bundled adapters + layer 1
3  detect stack; read host for a harness *preference* (it runs in the sandbox)
4  write layer 2: AGENTS.md, hooks, mcp.json, policy — never overwriting yours
5  ensure the image                ← the actual blocker; nothing runs without it
6  index the code graph            background, resumable
7  seed memory by derivation       README, manifests, git log, existing rules
8  report every decision
```

**Derive, never interrogate.** Every question init would ask is hassle we
promised to remove, and most answers are already lying around: manifests name the
stack, git log names what you work on, the README names the project. Derived
facts also refresh when the repo changes instead of going stale in a config file.

A question earns its place only if it is **not derivable**, **actionable** (omh
does something different with the answer), and **answerable well right now**.
"What is your job" fails all three. The strongest permitted form is *derive, then
confirm* — state the hypothesis and make it correctable:

```
! 2 stacks detected; hooks were written for all of them.
  drop the ones you do not want: .omh/profile/hooks/
```

That is not a questionnaire: init still decided, it just showed its work.

**The shortlist must be earned.** Every entry needs a sentence like *"cut
tokens-to-first-correct-edit by N% across the task suite."* Anything that can't
say that is taste pretending to be curation — see §12.

**The shortlist expires.** A distro's real work is re-choosing quarterly as the
catalog churns. The base set is therefore versioned (`omh 2026.08`) and
`omh upgrade` shows a changelog of what entered, left, and why.

---

## 6. Runtime backends

Isolation is not ours to build. But it is also not one vendor's to own.

The backend is pluggable behind a narrow trait. `Plan` is already a pure
description — a mount list, env, and argv — so a backend is just a translation of
that into one process invocation.

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

### Why not just pick `sbx`

It is better on every security axis — hypervisor isolation instead of a shared
kernel, and secrets injected at the egress proxy so **a compromised agent never
holds its own token**. The current omh design mounts a credential volume, which
means the agent *can* read and exfiltrate it. That is a real flaw and `sbx`'s
model is the fix.

But a distribution whose opinion cannot be escaped is not trustworthy. Backends
stay plural.

### Is `sbx` harness-agnostic?

Yes — verified, not assumed. Docker's own docs
[demonstrate building a kit for Amp](https://docs.docker.com/ai/sandboxes/customize/build-an-agent/),
a third-party agent they don't ship. The four named agents are shipped *kits*,
not a constraint. A kit is barely more than an omh adapter:

```yaml
entrypoint:
  run: [amp, --dangerously-allow-all]
```

Base image requirements are four: non-root `agent` user at UID 1000, passwordless
sudo, `/home/agent/` home, HTTP proxy env forwarding. omh's `GUEST_HOME` is
already `/home/agent`.

**An omh adapter can compile to an sbx kit.** Agnosticism expressed once.

### The seam

From the sbx FAQ:

> "Sandboxes don't import your complete user-level agent configuration. Hooks,
> settings, and other files under directories such as `~/.claude` remain on the
> host."

Docker drew its boundary exactly where omh's product begins. They isolate; they
deliberately do not carry your setup in. The two **compose**: `sbx` provides the
microVM, omh gets your profile inside it.

### Backend capabilities, and honest unknowns

Backends differ in ways that break a naive plan, so each **declares** what it can
do rather than failing mysteriously:

| Capability | Docker | `sbx` | Consequence if absent |
|---|---|---|---|
| bind-mount a single **file** | yes | **unknown** | staging must mount dirs + symlink instead |
| choose the **guest path** | yes | **no** — workspaces mount at the host path | `/work` convention breaks |
| SSH attach for IDE | yes (sshd in image) | **unknown** | §10 needs a different mechanism |

The two unknowns are not hand-waved: a `Plan` is validated against the selected
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
staging model needs rework rather than a tweak — staged content would have to be
written into the workspace and symlinked from inside, not mounted onto chosen
guest paths. Better to know that from a failing validation than from an agent
that starts fine and cannot see its own profile. Resolving them is a one-afternoon spike (build an opencode kit,
try a single-file mount, try attaching an IDE) and gates whether `sbx` becomes
the default or stays opt-in hardening.

Selection: `runtime = "auto" | "docker" | "sbx"` in `policy.toml`. `auto` prefers
`sbx` when present.

---

## 6b. Images

Two layers: a base every session shares, and a thin per-harness layer running the
adapter's `install` command.

```dockerfile
# omh/base:latest
FROM node:22-bookworm-slim
RUN apt-get install -y … git ripgrep dtach sudo curl jq
RUN usermod -l agent -d /home/agent -m node        # node:slim holds UID 1000
RUN test "$(id -u agent)" = "1000"                 # assert, do not assume
RUN mkdir -p /work /omh/sock /omh/cache /omh/layers
USER agent
```

```dockerfile
# omh/<harness>:latest
FROM omh/base:latest
USER root
RUN <adapter.install>
USER agent
```

**The base satisfies the `sbx` kit contract on purpose** — `agent` at UID 1000,
passwordless sudo, `/home/agent`, proxy env forwarding. One image works on either
backend, and an sbx kit becomes a two-line file rather than a port.

Three properties worth stating, each with a test:

- **The contract is asserted at build time**, not assumed. If a future base image
  moves UID 1000, the build fails there instead of failing mysteriously inside a
  sandbox.
- **Images end unprivileged.** Installing needs root; running must not have it. An
  image that ends as root hands the agent the sandbox's own escape hatch.
- **Dockerfiles arrive on stdin** with an empty build context, so nothing is
  written to disk and no context is uploaded.

`init` builds them, because init is not finished until `omh <harness>` works.
Roughly 30s on first run, cached after.

**A plan must be runnable, not merely well-formed.** The per-project network the
plan names has to be created too — that gap made every real launch die at
`network omh-<repo> not found` while every unit test passed. That class of
failure is precisely what `omh doctor` exists to catch.

### Verified end to end

Inside a real container, with the real mounts:

```
user:   agent uid=1000    home: /home/agent    cwd: /work
tools:  claude=ok dtach=ok git=ok rg=ok node=ok
  rules      # Global rules              (layers 1+2 concatenated)
  skills     graphify project-only       (unioned across layers)
  mcp        codegraph, filesystem, github, linear, omh-memory, sentry
  subagents  explorer.md
  hooks      PostToolUse, Stop           (rendered to claude settings.json)
  version    2.1.222 (Claude Code)
```

Every capability class arrives, merged across three layers, in the harness's own
format. That is the thesis, demonstrated rather than argued.

---

## 7. The session

A session is not a launch. It is a running sandbox, a worktree, and a branch,
which many harnesses take turns inhabiting.

```
       omh claude ──┐
       omh opencode ┼── exec ──┐
       omh code ────┘  (ssh)   │
                               ▼
 ┌──────────────────────────────────────────────────────┐
 │ SESSION  omh-<repo>-s01          detached, long-lived │
 │                                                       │
 │  sshd 127.0.0.1:49201 ──── IDE attaches here          │
 │  omh-mcp memory      ┐                                │
 │  codebase-memory-mcp ┴─ daemons: index stays warm     │
 │                                                       │
 │  /work  ← worktree (host dir, bind-mounted)           │
 │  staged profile, read-only                            │
 │  /omh/cache ← volume keyed by REPO, not harness       │
 └──────────────────────────────────────────────────────┘
```

| Command | Effect |
|---|---|
| `omh <harness>` | ensure the session is up, then exec the harness into it |
| `omh code [s]` | ensure up, open `ssh://omh-s01/work` |
| `omh fwd [s] 3000` | forward a port to the host |
| `omh ls` | sessions, branches, state, ports |
| `omh diff [s]` | `git diff main...omh/s01` |
| `omh down [s]` | stop; worktree and branch survive |
| `omh rm [s]` | remove the worktree; **the branch is always kept** |

Idle sessions auto-stop after `policy.idle_timeout`. N sessions means N sandboxes.

Memory lives in a volume keyed by **repo**, not harness — which is why switching
from Claude Code to opencode mid-project keeps everything the agent learned.

### Session persistence

A long-lived sandbox is not a long-lived *session*. `exec`ing a harness ties its
lifetime to your terminal: close the lid and the agent is hung up on mid-task
while the container keeps running around the corpse. Every harness is wrapped:

```
dtach -A /omh/sock/<session>-<harness>  <harness> [args…]
```

Detach is the terminal closing. Reattach is running `omh <harness>` again — `-A`
attaches to a live session or creates one, so a second invocation never starts a
second agent. The socket is a pure function of session and harness; anything
variable in that path would silently fork a duplicate.

**Why not tmux.** tmux is a multiplexer *and* a persistence tool, and omh needs
only the second half — `omh code` means SSH already gives you as many shells
against a session as you want. Adopting tmux buys one feature we need and one we
have, while importing costs that land exactly where this project is most fragile:
a prefix key competing with harness TUI bindings, nested tmux for anyone running
it on the host, and another translation layer over mouse, resize, and paste.
`dtach` does persistence in ~1000 lines with no prefix, no config, and nothing
between you and the harness.

Some harnesses ship their own resume. Relying on it would be precisely the
per-harness behaviour omh exists to abstract, so persistence is uniform and
lives in the distro. `persistence = "dtach" | "none"` in `policy.toml`.

**Open:** watching several things in one session — agent, dev server, shell —
is a real want that `dtach` cannot serve. Whether that calls for tmux inside the
sandbox, host-side panes over SSH, or a supervisor model (`omh run` / `omh logs`
for services you do not watch) is unresolved. Recorded rather than answered.

---

## 8. Layout

```
~/.omh/
  profile/                    layer 1 — personal, every project
  adapters/*.toml             one file per harness (data, not code)
  base/                       the curated base set, versioned
  creds/<harness>/<account>/  captured logins, one directory per account
  worktrees/<repo>/<session>/ agent's working directory
  keys/<repo>/                per-repo ed25519 keypair
  run/<session>/<harness>/    staged profile, regenerated per launch
  sessions.json               session → sandbox, branch, port

<repo>/.omh/
  profile/                    layer 2 — project, COMMITTED, shared with the team
  local/                      layer 3 — project, GITIGNORED, yours alone
```

Every layer has the same shape:

```
  AGENTS.md   skills/   mcp.json   commands/   hooks/   subagents/   policy.toml
```

Merge order 1 → 2 → 3, later winning. `AGENTS.md` concatenates; directory
capabilities union by entry name; `mcp.json` merges by server name;
`policy.toml` overrides key by key.

Layer 2 is committed, so **it must never contain a secret** — that is what layer
3 and `carry_in` are for.

**Worktrees live outside the repo on purpose.** Nested, your IDE would index every
session's full copy of the codebase — three sessions, four indexes.

---

## 9. Adapters are data; capabilities are optional keys

The profile carries the superset. An adapter declares which parts its harness can
express, and where.

```toml
name    = "claude"
bin     = "claude"
install = "npm install -g @anthropic-ai/claude-code"

[capabilities.rules]
path   = "/work/CLAUDE.md"
also   = ["/work/AGENTS.md"]
render = "concat"

[capabilities.mcp]
path   = "$HOME/.mcp.json"
render = "mcp-json"
import = "$REPO/.mcp.json"     # host-side, for `omh mcp import`
```

**An absent key means the harness cannot do it.** Degradation is a missing map
entry, not special-case logic, and it is announced once:

```
$ omh codex
omh: codex on omh/s01 — dropped 2 hooks, 3 subagents (unsupported)
```

| Render | Used by | Effect |
|---|---|---|
| `dir` | skills, commands, subagents | union layers by entry name, mount read-only |
| `concat` | rules | join layers, write into the worktree |
| `mcp-json`, `codex-toml`, `opencode-json` | mcp | reshape the canonical server list |
| `claude-settings` | hooks | reshape the canonical hook list |

Adapters parse with `deny_unknown_fields`. Without it a stale adapter parses
cleanly with zero capabilities and every harness silently degrades to nothing —
the worst possible failure for a tool promising your setup is already there.

**Roadmap note:** adapter *breadth* is capped at two harnesses until the base set
exists. Breadth before depth is how distributions die.

---

## 10. IDE access

The worktree is a plain host directory, so this was never about file access. It is
about where the language server runs: dependencies are installed Linux-side while
the host is macOS/arm64, so a host LSP means a second dependency tree that
silently diverges.

sshd runs in the session; the IDE attaches. One dependency tree, shared with the
agent.

**The integration point is a managed SSH config include**, not an IDE plugin:

```
# ~/.ssh/config.d/omh
Host omh-s01
  HostName 127.0.0.1
  Port 49201
  User agent
  IdentityFile ~/.omh/keys/ohmyharness/id_ed25519
```

Everything downstream then works without omh knowing it exists — VS Code
(`code --remote ssh-remote+omh-s01 /work`), Zed (`zed ssh://omh-s01/work`),
JetBrains Gateway, plain `ssh`. Dev servers come free: `omh fwd s01 3000`.

**Bind sshd to `127.0.0.1` only.** On `0.0.0.0` you have published a shell inside
your sandbox to the local network, inverting the point of the project.

```
$ omh code
session s01 is up

  ssh://omh-ohmyharness-s01/work
  ssh omh-ohmyharness-s01

  VS Code / Cursor   code --remote ssh-remote+omh-ohmyharness-s01 /work
  Zed                zed ssh://omh-ohmyharness-s01/work
  JetBrains          Gateway → SSH → omh-ohmyharness-s01
```

### Editors are data, like adapters

`omh <name>` means *attach this tool to the session*. A harness runs inside it;
an editor attaches from outside over SSH. Same gesture, so the same dispatch —
and adding an editor stays a TOML file rather than a match arm, for exactly the
reason adapters are data.

```toml
# ~/.omh/editors/zed.toml
name = "zed"
bin  = "zed"
args = ["$URL"]

# ~/.omh/editors/code.toml
args = ["--remote", "ssh-remote+$ALIAS", "/work"]
```

```
$ omh zed          # opens Zed on the session
$ omh claude       # runs Claude Code inside that same session
$ omh emacs
Error: unknown tool `emacs`
  harnesses: claude, opencode
  editors:   code, cursor, nvim, zed
```

An editor that is not installed is not an error — omh says so and prints the
URL. Launching nothing silently would be worse, and guessing a flag for an
unknown editor would launch the wrong thing.

`omh code` with no argument resolves `$OMH_EDITOR` / `$EDITOR` against the same
registry, falling back to printing every attach recipe.

Verified: `ssh omh-ohmyharness-s01` lands as `agent` (uid 1000) inside the
sandbox with the worktree at `/work` and the profile in place. `$OMH_EDITOR` or
`$EDITOR` opens it directly; an unknown editor is not an error, since guessing a
flag would launch something wrong.

Three details that are load-bearing:

- **The public key arrives as an environment variable, not a mount.** A
  bind-mounted `authorized_keys` lands with host ownership and sshd silently
  refuses to read one it does not trust — a failure that looks like a wrong key.
- **Ports are derived, not assigned.** An IDE bookmark points at the alias, which
  resolves through the port; a port that moved between restarts would break every
  saved window.
- **The `Include` is prepended** to `~/.ssh/config`, never appended: ssh applies
  the first matching block, so an include below someone's `Host *` would never
  win. Everything else in that file is left untouched.

---

## 10b. Accounts

```
omh auth claude personal
omh auth claude work
omh -a work claude              # or: omh config set account work
```

An **account is a captured snapshot of a harness's own credential files**. It is
keyed by *harness*, not by provider, because a harness is what can actually be
captured — two harnesses talking to the same provider still each need their own
login.

Which account a session uses is a **project-level setting**, resolved through the
usual three layers, because that is how it actually varies: this repo is work,
that one is personal.

```
~/.omh/creds/<harness>/<account>/.claude/.credentials.json
```

Storage mirrors the guest path, so an account directory is legible rather than a
pile of mangled names.

### There is no capture step

Credentials mount **writable at the paths the harness reads**, so the login
writes straight through to the host. `omh auth` prepares and runs; nothing is
copied afterwards.

Two consequences that are easy to get wrong:

- **Docker turns a mount of a non-existent host path into a directory**, so a
  first login would write its token into a folder the harness cannot read.
  `prepare` lays down empty placeholder files first.
- **A placeholder is not a login.** `is_captured` requires non-empty files, or an
  interrupted `omh auth` would leave something that reports as authenticated.

### Ambiguity is refused, never guessed

```
$ omh claude
Error: claude has several accounts: personal, work
  pick one with `omh config set account <name>` or `-a <name>`
```

Not being logged in at all is fine — the harness prompts, which is what you want
before your first `omh auth`. But an account you **named** and do not have stops
the launch: silently running with no credentials produces a session that is
logged out for reasons nothing explains.

Two identities and no stated preference is exactly when guessing is most
expensive — you would send work traffic through a personal account and never
notice.

### The invariant this bends

Credentials are writable, which the sandbox contract previously forbade. The
contract is now stated precisely rather than quietly widened:

> The worktree is writable because that is the work. Credentials are writable
> because OAuth tokens refresh in place, and a read-only mount would discard
> every refreshed token. **Nothing else is.**

This remains weaker than `sbx`'s model, where secrets are injected at the egress
proxy and the agent never holds its own token — see §14.2. Several accounts makes
that gap wider, not narrower.

---

## 11. Carry-in

A worktree contains only tracked files — no `.env`, no certs. Without help both
the agent *and* your IDE land somewhere that cannot run your app.

```toml
carry_in = [".env.local", "certs/"]
```

Copied at session creation and added to `.git/info/exclude`. `node_modules` is
deliberately not carried; it is built in the sandbox, for the sandbox's platform.

Copy, not symlink: a symlink's target would have to resolve inside the sandbox,
which would mean mounting your main checkout — exposing the uncommitted work the
worktree model exists to protect.

**`carry_in` is the only path by which a secret reaches the agent.** That is why
it is an explicit allowlist and why omh prints what it carried.

---

## 11b. Memory scope

Memory is **layered like the profile**, not stored in one bucket. Global-only and
per-project-only are both wrong, for opposite reasons.

| Scope | Location | Holds | Example |
|---|---|---|---|
| personal | `~/.omh/memory` | facts about **you** | prefers TDD; dislikes defensive comments |
| project | repo-keyed volume | facts about **this codebase** | the sbx spike is unresolved; hooks live in layer 2 |
| team *(later)* | committed in the repo | facts the whole team should share | deploys need the VPN |

A query merges the layers and reports which one answered, exactly as
`omh config` does for settings.

**Why not global-only.** A single store accumulates thousands of repo-specific
facts and retrieval degrades into noise — you pay tokens loading facts about
repo B while working in repo A. It also carries one client's context into
another's session, which is a confidentiality problem before it is a quality one.

**Why not project-only.** You are one person across every repo. How you work, what
you have already learned the hard way, and what you keep correcting do not reset
when you `cd`. Re-teaching those per project is the hassle omh exists to remove.

**Writes default to the narrower scope.** Project unless promoted, mirroring
`omh set` defaulting to the gitignored layer: a fact that should have been global
is a mild annoyance, a client detail that should not have been global is not.
Promotion is deliberate — `omh memory promote <fact>`.

**Surviving a harness switch is orthogonal to all of this.** Both layers survive,
because neither is keyed by harness. That property comes from what memory is *not*
keyed by, not from being repo-keyed.

A wrong global fact poisons every project, so global memory needs the expiry and
`omh why` story from §12 more urgently than project memory does.

---

## 12. Trust: the anti-oh-my-zsh-criticism

The standard complaint about oh-my-zsh is opacity — a slow shell nobody can
diagnose. "Without the hassle of understanding" curdles into "unable to
understand." Four commands exist to prevent that.

**`omh config` — provenance.** Every value says where it came from and what it
beat. No competitor does this.

```
carry_in   [".env.local"]   ← local (overrides shared)
```

**`omh why <thing>` — justification.** Provenance extended from *where* to *why*.

```
$ omh why codegraph
  in the base set since 2026.06
  cut tokens-to-first-correct-edit 41% across 12 tasks
  alternatives considered: CodeGraph, custom tree-sitter
  remove with: omh mcp rm codegraph
```

**`omh bench` — evidence.** A fixed task suite measuring tokens-to-first-correct-
edit with each component on and off. This is what makes "opinionated" mean
something other than "arbitrary", it is how base-set entries are earned and
retired, and it is a claim no app store can make about its own catalog.

**`omh eject` — the exit.** Write out the raw per-harness config and step aside.
For an opinionated tool, a credible exit is what makes adoption safe. Nearly free,
since omh already generates exactly these files.

Together these make the opinion a **default, not a cage**. An app store cannot be
overridden because it never decided anything.

---

## 13. Settings

```
omh config [policy|mcp]        effective settings, with provenance
omh set <key> <value>          → local layer (gitignored) by default
omh unset <key> [--layer]      lets the layer beneath resurface
omh edit [--layer]             $EDITOR escape hatch

omh mcp ls
omh mcp add <name> <cmd> [args…] [--env K=V]
omh mcp rm <name> [--layer]
omh mcp import <harness> [--file] [--force]
```

Writes default to the gitignored layer; writing to the committed one says so:

```
$ omh set carry_in '[".env"]' --layer shared
warning: the shared layer is COMMITTED — never put a secret here
```

`import` is the on-ramp — nobody retypes MCP servers they already configured. It
is the inverse of the renderers, so **every format that renders must also parse,
and the pair must round-trip**; otherwise import silently drops fields. That is a
test, not a hope.

Import never clobbers: each server is added, recognised as identical, or reported
as a conflict and left alone. Re-running is a no-op.

Import paths expand against the **host** — `expand_host` is deliberately separate
from `expand`, since the guest home would send import into a filesystem that does
not exist yet.

**Planned:** extend import from MCP to rules, skills, hooks, and commands, and add
a `plugin` capability that imports Claude marketplace plugins and re-renders them
for other harnesses. That last one is the capability nothing else can have — and
it makes curation nearly free, since we inherit Anthropic's taste and port it.

---

## 14. Risks, named

1. **Auth is done, and the OAuth flow is still unverified.** Everything in §10b is
   tested against fixtures; no real login has been completed end to end. That is
   the one part of omh that cannot be checked without a terminal.
2. **The current credential model is weaker than `sbx`'s.** Mounting a creds
   volume lets a compromised agent read its own token. Adopt proxy injection.
3. **A sandbox protects the host, not the repo.** The worktree branch is what makes
   this safe — which is why `omh rm` never deletes a branch.
4. **sshd is an attack surface pointed at yourself.** Loopback-only, per-repo keys,
   no password auth.
5. **Curation is a recurring commitment, not a one-time choice.** The base set goes
   stale; re-choosing quarterly is the real ongoing cost of a distro and the honest
   reason a solo one is hard.
6. **Sandbox sprawl.** One per session. Idle auto-stop is not a nicety.
7. **Adapter facts are unverified claims** about external software that ships
   weekly, and they break *silently*. `omh doctor` is the only cure.
8. **Concurrent edits.** You and the agent can write the same file. No worse than
   running `claude` natively, but no better.

---

## 15. Testing

**TDD, always** — see `.omh/profile/AGENTS.md`. Write the failing test, watch it
fail, then implement. For a bug fix the regression test must go red before the fix
lands. A green suite is not evidence on its own: reintroduce the bug and confirm
the guarding test turns red, or the test is decoration.

The architecture favours this. `document()` is pure; `plan()` and the backends'
argument construction are pure given a temp filesystem — so the places a mistake
is invisible are the places that test cheaply.

Load-bearing invariants, each with a failing-without-it test:

| Invariant | Why |
|---|---|
| nothing beyond the worktree and credentials is writable | a stray `rw` is the difference between a sandbox and a suggestion |
| credentials mount where the harness reads, writable | anywhere else and the session is logged out; read-only and refreshed tokens vanish |
| a named-but-missing account stops the launch | otherwise the session runs logged out and says nothing |
| staged links resolve under `/omh/layers/…` | host paths don't exist in the sandbox; skills silently vanish |
| staging is keyed by session **and** harness | else a second harness overwrites the first's mounted config |
| unknown adapter fields are rejected | else a stale adapter degrades everything, silently |
| `rm` keeps the branch; `ensure` reattaches | unreviewed agent work must be unloseable |
| each MCP format emits its harness's real schema | a wrong shape means zero servers and no complaint |
| every renderer round-trips through its parser | else import silently drops data |
| a probe with no output is never a pass | silence means the sandbox never ran it |
| sshd publishes on 127.0.0.1 only | 0.0.0.0 exposes a shell in the sandbox to the LAN |
| a session runs detached, never `--rm` | it must outlive the terminal, or nothing can attach |
| a plan is rejected when the backend lacks a capability | else `sbx` fails mysteriously instead of loudly |

**Factual correctness is not testable in process.** Adapters assert things about
external software. A green suite proves omh mounts a path faithfully, never that
anything reads it.

That gap is what `omh doctor` closes. It launches the real image with the real
mounts and inspects the **guest** paths the adapter declares — checking anything
host-side would re-test the staging directory omh wrote a moment earlier, which
is circular.

```
$ omh doctor
omh doctor: claude (in omh/claude:latest)

  ✓ AGENTS     /work/CLAUDE.md
  ✓ skills     /home/agent/.claude/skills
  ✓ mcp        /home/agent/.mcp.json
  ✓ commands   /home/agent/.claude/commands
  ✓ hooks      /home/agent/.claude/settings.json

  all 5 checks passed — claude's adapter paths are verified
```

Capabilities the harness cannot express are skipped, not failed — they were
already reported as dropped at launch. **A probe that produces no output is never
a pass**: silence means the sandbox never ran it, and calling that success would
make doctor worse than useless.

Both shipped adapters are verified this way — `claude` passes 5 checks,
`opencode` 4 (subagents and hooks correctly skipped). The "unverified claim"
caveat is retired for these two, and any third adapter inherits the same bar.

---

## 16. Milestones

**v0 — the base set, one harness.** ✅ `omh init` that decides · ✅ images ·
✅ sandbox + worktree · ✅ persistence · ✅ `omh auth` · ⬜ code graph · ⬜ memory
· ✅ `omh code` · ✅ `omh doctor`.
Success criterion: `omh init && omh claude` is visibly better than raw `claude`,
with zero questions asked.

*Not in v0: a second adapter, memory, the capability superset breadth.* Breadth
before depth is how distributions die.

**v0.5 — backends.** Runtime trait with Docker and `sbx`; the spike that resolves
file-mounts, guest paths, and IDE attach.

**v1 — evidence.** `omh bench`, then `omh why` reading from it. This gates every
subsequent base-set decision, which is why it comes before more features.

**v2 — portability.** Second adapter, `omh eject`, full `omh import`.

**v3 — the unique capability.** Marketplace plugin port: Claude Code plugins
re-rendered for other harnesses.

**v4 — memory and graph**, justified by `bench` or dropped.
