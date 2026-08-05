# ohmyharness (`omh`)

> Launch any coding harness, in a sandbox, with your setup already there.

```
$ omh claude       # or: omh opencode, omh codex, omh aider
```

That is the entire user-facing surface. Same rules, same skills, same MCP servers,
same memory — regardless of which harness you typed.

---

## 1. Thesis

Four problems looked separate. They collapse into one.

| Problem | Naive solution | What we do instead |
|---|---|---|
| Reconfiguring every harness | sync daemon copying files into `~` | **mount** one profile onto each harness's expected path |
| Sandboxing the agent | per-harness native sandbox flags | container — which is also what makes mounting possible |
| Portable rules/skills | write a translator | AGENTS.md + SKILL.md are already open standards; just place them |
| Memory & token efficiency | build a knowledge graph | one MCP server, registered once, inherited by every harness |

**Containerization is not a fourth feature bolted on. It is the mechanism that
makes the other three trivial.** There is no drift to fight because nothing is
ever copied into your home directory. Mounts are recreated on every launch and
vanish when the container exits.

### What is genuinely new here

Rules and skills portability is solved by standards (AGENTS.md under the Linux
Foundation since 2025-12; Agent Skills open standard since 2025-12-18). Code
knowledge graphs are solved by existing MCP servers. Sandboxing is solved by
Docker.

The thing nobody has: **memory that survives a harness switch.** Move from Claude
Code to opencode mid-project and the agent still knows what it learned. That
falls out of this architecture for free, because memory lives in a volume keyed
by *repo*, not by *harness*.

---

## 2. Decisions

| Decision | Choice | Why |
|---|---|---|
| Repo exposure | **git worktree, auto-branch** | agent physically cannot reach your working tree or `main`; review is `git diff` |
| Code knowledge graph | **wire an existing MCP server** | `codebase-memory-mcp` / CodeGraph already do this well; omh owns lifecycle + cache volume only |
| Language | **Rust** | single static binary, no runtime; `omh` wraps every invocation so startup latency is felt |
| Sandbox runtime | **Docker** | present on the target machine; microVM sandboxes are a later swap behind the same trait |
| LLM routing | **not ours** | `ANTHROPIC_BASE_URL` / `OPENAI_BASE_URL` in `policy.toml`, pointed at LiteLLM or OpenRouter |

---

## 3. Layout

```
~/.omh/
  profile/                    global — applies to every project
    AGENTS.md                 canonical rules
    skills/                   SKILL.md open standard, one dir per skill
    mcp.json                  canonical MCP server list
    policy.toml               egress allowlist, env, resource limits
  adapters/*.toml             one file per harness (data, not code)
  creds/<harness>/            credential volume, seeded by `omh auth`
  cache/<repo-id>/            code-graph index + memory graph — keyed by REPO

<repo>/
  .omh/
    profile/                  project overrides, layered over global
    worktrees/<session>/      agent's actual working directory
```

Profile resolution is a two-layer merge: project overrides global. `AGENTS.md`
concatenates (global first). `skills/` unions by directory name. `mcp.json`
merges by server name. `policy.toml` overrides key by key.

---

## 4. Adapters are data

An adapter declares four facts. That is the whole reason adding a harness is
cheap.

```toml
# ~/.omh/adapters/claude.toml
name    = "claude"
bin     = "claude"
install = "npm install -g @anthropic-ai/claude-code"

rules  = { path = "/work/CLAUDE.md", also = ["/work/AGENTS.md"] }
skills = { path = "$HOME/.claude/skills" }
mcp    = { path = "$HOME/.claude.json", format = "claude-json" }
creds  = ["$HOME/.claude/.credentials.json"]
```

Only `mcp.format` involves real work — rendering the canonical list into each
harness's schema (`claude-json`, `codex-toml`, `mcp-json`, `opencode-json`).
Everything else is a bind mount or a symlink.

Adding a harness = one TOML file. No recompile.

---

## 5. What happens on `omh claude`

```
1. resolve profile          global ⊕ project
2. ensure session           git worktree add .omh/worktrees/<id> -b omh/<id>
3. ensure image             omh/base + harness layer + project stack
4. render MCP config        canonical mcp.json → claude-json, into a tmpfs
5. docker run
     -v <worktree>:/work                          rw   ← agent's world
     -v ~/.omh/profile/skills:$HOME/.claude/skills:ro
     -v ~/.omh/creds/claude:$HOME/.claude:ro
     -v omh-cache-<repo>:/omh/cache               rw   ← graph + memory
     --network omh-<repo>                              ← egress-filtered
     -it  claude "$@"
6. on exit                  report `git diff main..omh/<id>`
```

Your host repo is never mounted. Only the worktree is.

---

## 6. Risks, named

1. **Auth is the whole UX risk.** If `omh claude` makes you log in again, the
   promise is dead. `omh auth <harness>` runs the harness's login flow once in a
   throwaway container and captures the credential dir into a volume. This is v0
   scope, not v2.
2. **Docker Desktop on macOS is a VM.** Bind-mount I/O is slow on large repos.
   Mitigate with VirtioFS; escalate to Docker's microVM sandboxes if it hurts.
3. **A container protects the host, not the repo.** The worktree branch is what
   makes this actually safe — that is why it is not optional.
4. **Graph staleness.** The index must update incrementally on file change, or
   it is worse than `ripgrep`. Owned by the wired MCP server; omh only guarantees
   it is running and its cache volume persists.
5. **TTY fidelity.** Harnesses are TUIs. Signals, resize, paste, and image input
   must survive `docker exec -it`. This is the most likely source of papercuts.

---

## 7. Milestones

**v0 — prove the thesis.** `omh init`, `omh auth`, `omh <harness>`, `omh diff`,
`omh rm`. Two adapters: `claude`, `opencode` (both already installed locally).
No knowledge graph at all. Success criterion: launch both harnesses in the same
repo and confirm they see identical rules, skills, and MCP servers with zero
per-harness setup.

**v1 — memory.** `omh-mcp` serving `memory.*`, registered once in the canonical
`mcp.json`, persisted in the repo-keyed cache volume. Success criterion: teach
something in Claude Code, switch to opencode, it still knows.

**v2 — graph.** Wire `codebase-memory-mcp`. Benchmark honestly:
tokens-to-first-correct-edit, with and without.

**v3 — egress policy.** iptables allowlist init (registries + model API only),
per-project docker network.
