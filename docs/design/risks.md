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

**2b. The sandbox's own git repository is agent-writable, and its push wall is a
signpost.** The agent commits into a gitdir omh mounts read-write, so every file
in it — including the `pre-push` hook — is the agent's to change. That hook is
bypassable by `--no-verify` and by `core.hooksPath`, measured against git
2.55.0. It is not containment and is not meant to be: with `curl` and risk 5
below, a push was never the narrow way out. What stops work reaching your remote
is that the repository has no remote and no commit of yours in it. omh does not
trust anything that repository asserts — authorship is stamped and carried
secrets are refused on the host, when work crosses back.

Narrowing it is cheap and is scheduled with the [git work](git.md). The gitdir
has to be writable because the agent commits into it; its `config` does not.
Measured 2026-08-21: `commit`, `checkout -b`, `stash`, `reset --hard`, `rebase`
and `gc` never write that file, and when it cannot be replaced `git config` and
`git remote add` fail cleanly with nothing half-written. Mounting it read-only
therefore closes the whole agent-writable-config class — `diff.external`,
textconv, `core.hooksPath`, `core.sshCommand`, `protocol.file.allow` — at the
mount, rather than by omh remembering the right `-c` flag at every call site
forever. That matters more than it used to, because `omh sNN log` and `diff`
will read this gitdir **on the host**, where a diff driver the agent named runs
as you. It also takes `git remote add` away, which is the route git's own
error message walks the agent to — though not the hook's job: measured
2026-08-21, `git push <url> <ref>` needs nothing in config and still meets it.
What changes on the `remote add` route is that the agent reads a filesystem
error rather than omh's sentence, which is a reason to put the fact in the
arrangement, not a reason to skip the mount. Measured with `chflags uchg` standing in for a
read-only bind mount — both make a rename-over fail — so the claim owes a check
inside a real container, which is `doctor`'s job.

**2c. `omh s rm` destroys the agent's own commits.** The worktree's content is
already on disk and the branch survives, but checkpoints the agent made and you
did not harvest with `omh s commit --keep` go with the session — and after a
`git reset --hard` in the sandbox, those were the only copies. Nothing counts
them either, so the removal cannot mention what it is about to take. Closing it
needs a way to see them and a way to refuse over them; both are designed in
[Git](git.md).

**3. sshd is an attack surface pointed at yourself.** Loopback-only, per-repo
keys, no password auth. All three are asserted by tests, because the failure
mode of getting the bind address wrong is publishing a shell inside your sandbox
to the local network.

**4. `carry_in` is read from a committed layer.** It is the only path by which a
secret reaches the agent, so patterns are validated against escaping the repo.
A teammate's commit should not be able to copy your `~/.ssh` into a sandbox.

**4b. Closed.** The carried-secret scan read its needle as a *pattern* rather
than as bytes. `--keep` searches the agent's commit messages for lines from the
files you carried in, and passed them to `git log --grep` without `-F` — and
which pattern language that is depends on the reader's `grep.patternType`.
Measured 2026-08-21 against git 2.55.0, on a subject quoting the secret
verbatim: a `*` in a secret was missed under **all three** settings, a `+` was
missed under `extended` and `perl` (found under the `basic` default), and an
unbalanced `[` was a fatal error everywhere — taking `--keep` down for that
session entirely. A guard whose reach depends on the user's dotfiles is not a
guard; `-F` pins it. The content half was always a literal pickaxe and was never
affected.

**4d. What the carried-secret scan still cannot see.** It works from *needles* —
the lines of the files you carried in — and three kinds of line never become
one. A file that is not UTF-8 yields **no needles at all**, so a carried
keystore, `.p12` or DER key is protected only by its path: copy it under another
name and nothing catches it, and `certs/` is the second example `carry_in`'s own
documentation gives. A line shorter than 12 characters, or one starting with `#`
or `//`, is dropped silently. And a carried file deleted or renamed in your
checkout between launch and harvest yields nothing either, which is strictly
worse than the rotation caveat already documented in the code: rotation leaves a
needle at the new value, deletion leaves none. All three fail open, none is
reported.

**4c. The sandbox's exclude list is frozen at the first launch.** omh derives
what the sandbox's repository must not track from the mounts it is about to
make — its rendered rules, `.mcp.json`, the files you carried in — and writes
that list once, when the repository is created. A capability switched on later
adds a mount inside `/work` which the existing sandbox neither tracks nor
excludes, so the agent's own `git add -A` commits omh's rendered document, MCP
environment included, into a history `omh s commit --keep` replays onto your
branch. Rewriting the list on every launch closes it and touches no commit;
scheduled with the [git work](git.md).

**5. Egress is unrestricted.** The allowlist is designed and not wired. An agent
in a session can reach the network freely.

## Correctness

**6. Adapter facts are unverified claims** about external software that ships
weekly, and they break *silently*. `omh doctor` is the only cure, and it must be
re-run rather than trusted once. Almost every bug this project has shipped lived
at this boundary, and **not one was catchable by the test suite.**

Git is the case nobody noticed: it is the external binary omh leans on hardest,
and `doctor` has never probed it — not a version, not a behaviour. The [git
work](git.md) adds a version-dependent claim (`merge-tree --write-tree`, git
≥ 2.38), which is the occasion to close both.

**7. Auth is verified but was hard-won.** A real `omh auth claude <account>`
login completes and persists. Five bugs preceded that, every one at the boundary
with a real harness.

**8. Concurrent edits.** You and the agent can write the same file. No worse than
running `claude` natively, but no better, and the worktree makes it easier to
forget that you are both in there.

**8b. One measured defect left on the host side of git.** Measured 2026-08-21
against git 2.55.0, with its test owed before the fix, and ordered in
[Git](git.md#order). Two others are closed: `omh s rm` deleting a branch it
could not count — a count with no answer now keeps the branch and says so — and
a session being created on trunk itself, where git's remote DWIM overrode
`worktree add -b`. The start point is a resolved commit now, and `default_branch`
returns a ref this checkout can actually answer about.

- **`omh s commit --keep` is not repeatable.** It replays from where the session
  started, so a second run offers commits already on your branch and fails
  applying them.

**8c. An orphaned sandbox repository is adopted by the next session that takes
its id.** Ids are the highest `sNN` plus one, so they come back around, and
`omh s rm` deletes the repository with the session precisely to stop this. A
worktree removed outside omh — which `s ls` already expects, since it reports
what a hand `git worktree remove` strands — leaves the gitdir and its seed
behind, and the next session with that id opens on the previous one's history,
against a seed naming a tree it never had. `s ls` looks at containers and run
directories, not at these.

**8d. Two checkouts with the same directory name share everything.** Worktrees,
sandbox repositories, container names, the cache volume and the network are all
keyed by the checkout's basename, so `~/work/api` and `~/oss/api` are one repo
as far as omh is concerned — and the second one resumes into the first one's
session. Fixing it changes a path scheme with live sessions underneath it, so it
needs a migration rather than a patch and is scheduled on its own.

## Operational

**9. Sandbox sprawl.** One container per session. `idle_timeout` stops
sessions nobody has used, on the next launch — but it is **unset by default**,
so sprawl is opt-out rather than prevented, and a machine that never launches
again never reaps. The [git work](git.md) narrows it from the other end: a
dashboard saying which sessions hold nothing you have not already taken is what
makes deleting them safe, and an `rm` that refuses over unharvested commits is
what would make a stronger default policy safe to set.

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
- **Only `claude` has been driven for real work.** `opencode` and `omp` pass
  `doctor`, which proves their paths and nothing about their behaviour. `omp`
  carries one gap beyond that: its login probe greps for a field name read out
  of oh-my-pi's source, and no logged-in run has confirmed it.
- **A seeded profile and the shipped base set drift by design.** `init` copies
  the [base set](base-set.md) into your profile once and never rewrites it, so
  your edits survive — but an upgrade moves the manifest and not your copy.
  `omh why` reports the difference and deliberately does **not** say who caused
  it, because it cannot tell an edit from an upgrade. Recording the version a
  profile was seeded from would close this; nothing does yet.
- **`omh why` answers about one repo's profile.** It reads the layers resolved
  for the current repo, so the same entry can legitimately answer differently in
  two checkouts. That is correct and occasionally surprising.
