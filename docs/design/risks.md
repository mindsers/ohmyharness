# Risks

Stated plainly, because a tool that asks for your credentials and runs an agent
against your code should not make you go looking.

## Security

**1. The credential model is weaker than `sbx`'s.** omh mounts a credential
volume, so a compromised agent can read and exfiltrate its own token. `sbx`
injects secrets at the egress proxy and the agent never holds one. Adopting that
is the single largest security improvement available, and it is gated on the
[backend spike](architecture.md#declared-capabilities-and-honest-unknowns).
Several accounts widens this gap rather than narrowing it.

**2. A sandbox protects the host, not the repo.** What protects the repo is the
worktree branch — which is why `omh s rm` never deletes one. Conflating the two
is how people end up surprised by what an agent could reach.

**3. sshd is an attack surface pointed at yourself.** Loopback-only, per-repo
keys, no password auth. All three are asserted by tests, because the failure
mode of getting the bind address wrong is publishing a shell inside your sandbox
to the local network.

**4. `carry_in` is read from a committed layer.** It is the only path by which a
secret reaches the agent, so patterns are validated against escaping the repo.
A teammate's commit should not be able to copy your `~/.ssh` into a sandbox.

**5. Egress is unrestricted.** The allowlist is designed and not wired. An agent
in a session can reach the network freely.

## Correctness

**6. Adapter facts are unverified claims** about external software that ships
weekly, and they break *silently*. `omh doctor` is the only cure, and it must be
re-run rather than trusted once. Almost every bug this project has shipped lived
at this boundary, and **not one was catchable by the test suite.**

**7. Auth is verified but was hard-won.** A real `omh auth claude <account>`
login completes and persists. Five bugs preceded that, every one at the boundary
with a real harness.

**8. Concurrent edits.** You and the agent can write the same file. No worse than
running `claude` natively, but no better, and the worktree makes it easier to
forget that you are both in there.

## Operational

**9. Sandbox sprawl.** One container per session. Idle auto-stop is not a nicety.

**10. Curation is a recurring commitment, not a one-time choice.** The base set
goes stale; re-choosing quarterly is the real ongoing cost of a distribution and
the honest reason a solo one is hard. This is the risk most likely to kill the
project, and it is not a technical one.

## Known rough edges

- An agent can query **another session's graph** for the same repo. Mitigated by
  naming the project in three places, not prevented. See
  [code graph](../code-graph.md#what-the-agent-can-still-see).
- **`.claude.json` is a single-file mount** that cannot be atomically replaced.
  `doctor` reports it rather than pretending otherwise.
- **`omh s rm` keeps a branch even when the session produced no commits**, which
  litters the namespace. Erring toward keeping unreviewed work was deliberate;
  keeping empty branches was not.
- **Only `claude` has been driven for real work.** `opencode` passes `doctor`,
  which proves its paths and nothing about its behaviour.
- **A seeded profile and the shipped base set drift by design.** `init` copies
  the [base set](base-set.md) into your profile once and never rewrites it, so
  your edits survive — but an upgrade moves the manifest and not your copy.
  `omh why` reports the difference and deliberately does **not** say who caused
  it, because it cannot tell an edit from an upgrade. Recording the version a
  profile was seeded from would close this; nothing does yet.
- **`omh why` answers about one repo's profile.** It reads the layers resolved
  for the current repo, so the same entry can legitimately answer differently in
  two checkouts. That is correct and occasionally surprising.
