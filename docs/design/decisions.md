# Decisions

Every load-bearing choice, with the reason. If you are about to change one of
these, the reason is what you need to argue with.

| Decision | Choice | Why |
|---|---|---|
| Category | **distribution** | the parts exist; assembly and subtraction are the value |
| Runtime | **pluggable backend** | `sbx` where available, Docker as fallback; no vendor lock |
| Repo exposure | **git worktree, auto-branch** | the agent cannot reach your checkout or `main`; review is `git diff` |
| Git in the sandbox | **a repository of its own** | your checkout is never mounted, so the agent gets a gitdir holding one commit and none of your history |
| Getting work back | **`omh s commit`, squash or `--keep`** | you choose at commit time whether the agent's own commits and messages land |
| Code graph | **wire an existing MCP server** | distributions package, they don't reinvent |
| Language | **Rust** | single binary; `omh` wraps every invocation, so startup is felt |
| LLM routing | **not ours** | one env var in `settings.toml` |
| Unit of work | **long-lived session** | keeps the index warm, makes harness switching instant |
| Session persistence | **`dtach`, not tmux** | omh needs detach/reattach; SSH already provides multiplexing |
| IDE access | **SSH into the session** | one dependency tree, shared with the agent |
| Untracked files | **`carry_in` allowlist** | worktrees are pure git; secrets enter only by declaration |
| Capability floor | **superset, adapters degrade** | omh must never cost you a feature you already had |
| Content scope | **one personal catalogue** | "where is this skill" had three answers, and a union can add but never subtract |
| Repo content | **hooks, and nothing else** | a hook binds to a repo's commands; a skill is a way you work |
| Settings scope | **3 layers: personal / shared / local** | a machine-wide preference and a one-repo exception are both real |
| Selection | **an allowlist, `[use]`** | removing something is deleting its name; no `include`/`exclude` pair to reconcile |
| Absent selection | **means everything** | upgrading changes nothing, and a new checkout is useful before it is configured |
| Command scope | **`omh config` is you, `omh repo` is here** | the two want opposite write defaults, and one `--layer` flag cannot express both |
| Write default | **a value never lands in the committed file; a name may** | `omh repo set`/`omh config set` take a value you typed and default away from git. `omh use`/`unuse` and `omh repo enable`/`disable` write only names, and what a project uses is a fact about the project |
| Hook vocabulary | **closed, translated at staging** | `event`/`matcher`/payload are one harness's words; no runtime shim |
| Tool vocabulary | **one closed set, per-adapter map** | the one thing skills, subagents and hooks all leak |
| Naming a session | **`sNN` first, one form** | several sandboxes of one repo are reached from one place, so the selector leads and everything after it is unchanged — designed, see [git](git.md) |
| Staying current | **`omh sNN sync`, merged on the host** | no commit of yours may enter the sandbox, so it receives files and conflict markers rather than history — designed, see [git](git.md) |
| Portability | **store the standard where one exists** | skills and rules already travel; only hooks and subagents need omh |

Each is expanded where it applies: [architecture](architecture.md) for runtime
and images, [sessions](../sessions.md) for the session model and persistence,
[configuration](../configuration.md) for the layers, [adapters](adapters.md) for
the capability floor.

## The base set

This is the product. Everything else is a place to put it.

```
omh init             → base system. no questions.
                     → stack rules and hooks, derived from what init detected.
omh config mcp add … → the archive is still there, one command away,
                       and not in your face.
```

| Component | Status | Justification |
|---|---|---|
| sandbox + worktree branch | ✅ | safety; non-negotiable |
| `AGENTS.md` from detected stack | ✅ | the thing everyone writes badly |
| `omh attach` | ✅ | IDE attach |
| **`codegraph`** | ✅ | structural queries instead of re-grepping every task |
| test-on-stop + format-on-edit hooks | ✅ | `init` detects the commands and wires them |
| memory | ⬜ | survives harness switches |
| egress allowlist | ⬜ | inherited from the runtime |

**If an eighth entry needs a paragraph to justify, it belongs in a profile, not
the base set.**

The set lives in a versioned manifest rather than in the binary, and the
justification is enforced by a test rather than by convention — see
[the base set](base-set.md).

The base set is also the part of omh with the least evidence behind it. See
[the honest weak spot](distribution.md#the-honest-weak-spot).

## Decisions deliberately not made

- **Which model you use.** One env var. Routing is a solved problem with several
  good solutions and no reason for omh to have an opinion.
- **Which editor you use.** [Editors are data](../editors.md#editors-are-data),
  and the integration point is an SSH config include rather than a plugin, so
  editors omh has never heard of work anyway.
- **Whether to trust the sandbox over the branch.** Both. A sandbox protects the
  host; the worktree branch protects the repo. They are not substitutes, and
  conflating them is how people end up surprised.

## Reversals worth knowing about

Two decisions were made, shipped, and undone. Both are more instructive than the
choices that held.

**Per-session graph website → one per repo.** Every session's graph lives in one
volume, so a per-session server displayed every other session's graph anyway — N
identical websites on N ports, each running inside a container that held a
writable worktree and live credentials. Scoping the service to the repo removed
the duplication, removed the exposure, and deleted the `pgrep` guard, detached
`exec` and `pkill` that the per-session version needed — each of which had been a
bug before it worked. **Choosing the right scope removed them rather than fixing
them.**

**Credentials mounted at a private path → mounted where the harness reads.** The
first version mounted a credential volume at `$HOME/.omh-creds`, which no
harness has ever read. It tested green. See
[accounts](../accounts.md#mount-the-directory-never-the-token-file).
