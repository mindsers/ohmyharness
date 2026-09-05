# Risks

**Status: current, and deliberately not tidied.** Closed risks stay on the page struck through, with the pull request that closed them, because a risk that vanishes teaches nobody what it cost.

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

**Narrowed.** The gitdir has to be writable because the agent commits into it;
its `config` does not. omh rewrites that file from its own view at every launch
and mounts it read-only, which closes the agent-writable-config class —
textconv drivers, `core.hooksPath`, `core.sshCommand`, `protocol.file.allow` —
at the mount rather than by omh remembering the right `-c` flag at every call
site forever.

Measured inside a real container: `commit`, `checkout -b` and `reset --hard`
never write that file and do not notice it, while `git config` and
`git remote add` meet `could not write config file /omh/shadow/config: Resource
busy` with nothing half-written. It is `Resource busy` rather than `Read-only
file system` because git replaces the file by renaming a lock over it, and what
refuses is the rename onto a mount point.

Taking `git remote add` away closes the route git's own error message walks the
agent down, so the arrangement now says adding a remote will not work either.
The `pre-push` hook keeps its job on the other route: `git push <url> <ref>`
needs nothing in config and still meets it. What remains open is everything
else in 2b — the hook is still bypassable by `--no-verify`, and risk 5 is
untouched. Measured with `chflags uchg` standing in for a
read-only bind mount — both make a rename-over fail — so the claim owes a check
inside a real container, which is `doctor`'s job.

**2c. ~~`omh s rm` destroys the agent's own commits.~~** *(Closed in #58.
`omh sNN rm` refuses over checkpoints no branch has, naming how many and what
to do, before it takes anything down; `--force` is the way past. Everything
below is what it was answering.)* *(Previously: partly addressed — what
has already been harvested is on the branch, and `--keep` is repeatable now, so
the window is what the agent has done since the last landing rather than the
whole session. `omh sNN log` (#54) shows them and counts them; refusing over
them is step 12.)* The worktree's content is
already on disk and the branch survives, but checkpoints the agent made and you
did not harvest with `omh s commit --keep` go with the session — and after a
`git reset --hard` in the sandbox, those were the only copies. `rm` asks for the count now and stands on it — and the count is deliberately
wider than the numbered list `log` prints. `seed..HEAD` cannot see work the
agent threw away with `reset --hard`, which is the sentence above; measured,
`--all --reflog --not <last handover>` can. It also catches a branch the
sandbox wandered off, which `preflight` already refuses a *harvest* over while
`rm` was dropping it for good.

A sandbox that never ran removes quietly. One omh cannot read does not: that is
a third answer rather than a quiet yes, because the states that produce it — a
truncated replay record, a repository with no seed — are ones where the work is
demonstrably still there and only its classification is missing. `--force`
covers every refusal, so the door is never held shut; the user is asked once.

**3. sshd is an attack surface pointed at yourself.** Loopback-only, per-repo
keys, no password auth. All three are asserted by tests on what omh writes — the
publish address, the key, and the `-o PasswordAuthentication=no` family on the
sshd line — because the failure mode of getting the bind address wrong is
publishing a shell inside your sandbox to the local network. That sshd honours
those flags is a fact about openssh, and `omh doctor` is where it gets proved,
not the suite.

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

**4d. What the carried-secret scan cannot see, it now says.** It works from
*needles* — the lines of the files you carried in — and three kinds of line
never become one. A file that is not UTF-8 yields **no needles at all**, so a
carried keystore, `.p12` or DER key is protected only by its path: copy it under
another name and nothing catches it, and `certs/` is the second example
`carry_in`'s own documentation gives. A line shorter than 12 characters, or one
starting with `#` or `//`, is dropped. And a carried file deleted or renamed in
your checkout between launch and harvest yields nothing either, which is
strictly worse than the rotation caveat already documented in the code:
rotation leaves a needle at the new value, deletion leaves none.

**Narrowed, not closed.** All three still fail open, and that is deliberate:
refusing a harvest because omh could not read a file would be a worse trade
than the gap. What changed is that they no longer fail *silent*. `Shadow::needles`
returns what it could not read alongside what it could, and both moments report
it — at launch, where you can still carry the file under a name the scan can
read, and at harvest, where the count would otherwise be the same sentence
whether omh had looked or not. Each file is named with its cause, directories
resolve to the file inside them rather than to the entry, and the warning says
the path check still stands so nobody reads it as *unguarded*:

```console
omh: the carried-file scan could not read these, so it cannot tell you whether
     their contents reached a commit:
    certs/deploy.p12 — it is not text, so there are no lines to search for
    certs/short.env — every line in it is too short or is a comment
  the path itself is still checked — what is not is a copy under another name
```

An empty carried file is deliberately not reported: it yields no needles like
the other three, but there is nothing in it to have been copied anywhere, and a
warning that fires on every empty `.env` placeholder is one nobody reads.

What remains is the gap itself. A copy of `deploy.p12` under another name still
reaches the branch unremarked — omh now tells you it cannot see that, rather
than implying it looked.

**4c. Closed.** The sandbox's exclude list was frozen at the first launch. omh
derives what that repository must not track from the mounts it is about to
make — its rendered rules, `.mcp.json`, the files you carried in — and wrote the
list once, when the repository was created. A capability switched on later, or a
`carry_in` entry added later, mounted a document the existing sandbox neither
tracked nor excluded, so the agent's own `git add -A` swept omh's rendered file —
MCP environment included — into a history `omh s commit --keep` replays onto your
branch. The list is rewritten on every launch now, which touches no commit.

**4e. `ca_cert` widens what the sandbox trusts, on purpose.** Behind a
TLS-inspecting proxy nothing in a container verifies, so no image gets built at
all — the setting exists because the alternative is that omh does not work
there. What it costs is real: the sandbox then trusts a root that, by
construction, can sign a certificate for any host. That is already true of the
machine omh runs on, so this hands the sandbox no authority the user's own
laptop lacks, and it is the reason the setting names **one** certificate rather
than copying the host trust store in — the difference between inheriting one
decision IT already made and inheriting all of them.

Two properties keep it honest. The certificate is written into the recipe
rather than passed as a build argument, so the image tag is a digest of a text
that includes it and a rotated root cannot be a cache hit on an image trusting
the retired one. And a path omh cannot read is an error: resolving a typo to
"no certificate" would rebuild exactly the image that was already failing and
report success.

What is *not* claimed: that every toolchain needed telling. Measured against a
server presenting a leaf signed by a private root, in three arms — no root
installed, the root in the system store with no variables set, and the full
recipe. The middle arm is the one that isolates: only node still fails there,
with `UNABLE_TO_VERIFY_LEAF_SIGNATURE`, while curl, git, python, pip, go and
cargo all verify off the store, Debian's pip included. So one of the seven
variables is load-bearing and six are belt and braces; they are set anyway,
and described that way rather than as each being what makes its tool work.

The recipe splits a chain into one file per certificate for
`update-ca-certificates`, then concatenates it back into the single file
`NODE_EXTRA_CA_CERTS` names — because node reads that file and nothing else.
A guard pins the concatenation to the whole chain: narrowing it to the first
certificate used to leave the suite green while an intermediate-signed leaf
failed in node alone.

**5. Egress is unrestricted, and on Docker that is the design.** An agent in a
session can reach the network freely.

This page used to read *"the allowlist is designed and not wired"*, which
implied an omh feature somebody had failed to finish.
[Decisions](decisions.md) has recorded egress as **inherited from the runtime**
throughout, and [architecture](architecture.md#runtime-backends) puts egress
policy in the `sbx` backend beside its keychain-backed credential injection.
Those two were right and this one was out of step — worth naming, because a
risk page that misattributes a gap sends the reader looking for the wrong fix.

So it is not on omh's list to wire. It arrives with a backend that has it, or
it does not arrive. That also means it shares a fix with **risk 1** — `sbx`
injects secrets at the egress proxy, so the agent never holds its own token —
and the two are one piece of work rather than two, gated on the
[v0.5 spike](roadmap.md).

Until then: a Docker session has unrestricted egress, and omh does not claim
otherwise.

**5b. `refs/omh/turn` is a place the guards do not look.** The sandbox's gitdir
is a read-write mount, and since 2026.08 three of omh's own queries skip that
ref so per-turn snapshots are not mistaken for the agent's stranded work.
Anything the agent writes there is skipped by all three, so an agent could park
commits there and a harvest would drop them without the refusal that normally
stops it.

Bounded deliberately. It is one fixed ref rather than a glob, so there is
exactly one such place; `omh sNN rm` names what would go, counting the ref
directly through `Shadow::turns` rather than through the excluded queries; and
`omh sNN log --turns` prints its contents **including each subject**, which is
constant for omh's own snapshots — so anything else parked there is visible as
the odd row. What an agent gains is the ability to lose its own work quietly,
which is not a boundary it could not already cross with `git reset --hard`.

An earlier version of this note said `unkept` still counts the ref. It does
not — it carries the same exclusion as the other two. The mitigation is real
but it runs through a different query, and saying so matters: a refactor of
`Shadow::turns` that did not know it was load-bearing here would remove it.

**One config line defeats the exclusion.** `core.logAllRefUpdates = always`
makes git write a reflog for every ref under `refs/`, and `unkept`'s
`--reflog` arm is not reachable by `--exclude`. The effect is noise rather than
a breach — `rm` refuses for the rest of the session and only `--force` gets
past — and `write_config` pins the key on every launch so it does not survive.

## Correctness

**6. Adapter facts are unverified claims** about external software that ships
weekly, and they break *silently*. `omh doctor` is the only cure, and it must be
re-run rather than trusted once. Almost every bug this project has shipped lived
at this boundary, and **not one was catchable by the test suite.**

Git is the case nobody noticed: it is the external binary omh leans on hardest,
and `doctor` has never probed it — not a version, not a behaviour. The [git
work](git.md) adds a version-dependent claim (`merge-tree --write-tree`, git
≥ 2.38), which is the occasion to close both — and to pin what
`cherry-pick --empty=` needs, which #56 made a dependency of `--keep
<selection>` without being able to measure the floor.

**7. Auth is verified but was hard-won.** A real `omh auth claude --name
<account>` login completes and persists. Five bugs preceded that, every one
at the boundary with a real harness.

**8. Concurrent edits.** You and the agent can write the same file. No worse than
running `claude` natively, but no better, and the worktree makes it easier to
forget that you are both in there.

**8b. Closed.** Three measured defects on the host side of git, all fixed.
`omh s rm` deleted a branch it could not count — a count with no answer now
keeps the branch and says so. A session could be created on trunk itself, where
git's remote DWIM overrode `worktree add -b` — the start point is a resolved
commit now, and `default_branch` returns a ref this checkout can answer about.
And `omh s commit --keep` was not repeatable, replaying from where the session
started, so a second run offered commits already on the branch and died applying
them — it replays from what it last handed over now, and refuses rather than
guessing when the sandbox has rewound below that point.

**8c. ~~An orphaned sandbox repository is adopted by the next session that
takes its id.~~** *(Closed in #59: `omh s` looks at them now, beside the
containers and run directories it already reported. `omh <id> rm` clears one,
and since #58 says what it would take with it first.)* Ids are the highest `sNN` plus one, so they come back around, and
`omh s rm` deletes the repository with the session precisely to stop this. A
worktree removed outside omh — which `omh s` already expects, since it reports
what a hand `git worktree remove` strands — leaves the gitdir and its seed
behind, and the next session with that id opens on the previous one's history,
against a seed naming a tree it never had. `omh s` looked at containers and run
directories and not at these — the most valuable of the three, since a
container is re-creatable and a run directory holds a timestamp, while this
holds every commit an agent made.

**8d. ~~Two checkouts with the same directory name share everything.~~**
*(Closed in 0.8.0.)* Worktrees, run state, ssh keys, sandbox repositories, the
note store, the cache volume, the network and container names were all keyed by
the checkout's basename, so `~/work/api` and `~/oss/api` were one repo as far
as omh was concerned — and the second one resumed into the first one's session:
a live container holding another project's code, reached by typing an ordinary
command in an ordinary checkout.

The key is now the basename **and** a digest of the canonical path —
`api-3f9a2c1b` — composed in `Paths::repo_id`, which all nine accessors route
through. The name stays in front because people read these: `omh s` prints them
and `docker ps` lists them, and `omh-3f9a2c1b-s01` says nothing about which
checkout it is. `omh info --repo` reports it, so the question the collision used
to create has an answer.

The digest is FNV-1a written out rather than `DefaultHasher`, and the reasoning
is the same one `container::labels` already records: std does not guarantee that
hasher's output across releases, and this value names directories holding an
agent's commits. Drift would not break loudly, it would strand every session and
open an empty one where the work used to be. A test pins it to FNV's published
vectors rather than to its own output, so a rewrite has something to fail
against.

**The migration reads ownership rather than assuming it.** A worktree's `.git`
is a file naming the checkout it belongs to, so omh does not have to guess which
of two `api`s owns `~/.omh/worktrees/api`. A pointer naming this checkout moves
everything; a pointer naming another one refuses, says whose it is, and leaves
it for that checkout to claim on its next run; no worktrees at all moves the
rest, since there is no session to collide over and stranding an `init`-only
repo's notes would be worse. A running sandbox refuses outright — its mounts
point at the directory being renamed. And state under the old key that *cannot*
move, because the new key is already in use, is reported rather than passed over
in silence: nothing reads it again, and silence is what let this survive.

What remains is that the cache volume and the network are recreated rather than
renamed — they are derivable and docker cannot rename either — and **nothing
reports the old pair.** An earlier draft of this page said `omh s` did.
It does not: `leftovers` reads the shadow directory, the run directory and
`docker ps -a` filtered by the *new* container prefix, and omh issues no
`volume ls` or `network ls` anywhere. A stopped container under the old key
is missed for the same reason — the migration only refuses over *running*
ones.

So after a migration `omh-cache-<basename>`, `omh-<basename>` and any stopped
`omh-<basename>-sNN` are orphaned and unmentioned. They cost disk and a name,
not correctness.

*(Fixed in 0.10.0.)* `omh init` records the checkout's path beside the id
everything else is keyed by, which is the half that was missing: the id is a
one-way digest, so omh could see these existed and never whose they were.
`omh prune` acts on it, machine-wide, across volumes, containers, networks, the
per-checkout directories and the `tmp.*` remnants of operations that did not
finish — removing only what it can prove belongs to a checkout that is gone.

**What it does not close.** Ids created before 0.8.0 carry no path digest, so
there is nothing to compare and they stay unattributable unless a worktree's
`.git` pointer still names their checkout. On a machine with history that is
most of them, and `prune` says so rather than guessing. Nor are images pruned:
an image is content-addressed and shared, so the checkout that built it being
gone says nothing about who is running it — that needs the in-use check
`image::superseded` already implements, and it has not been wired up.

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
