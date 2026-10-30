---
title: Risks
description: What is weak, stated plainly.
---

This page lists what is still weak in omh — in security, correctness and
operations — stated plainly, because a tool that asks for your credentials and
runs an agent against your code should not make you go looking. Read it before
you trust omh with either.

**In short**

- **The two largest gaps are one piece of work.** On Docker, the agent can read
  its own token and reach the network freely; both close with a backend that
  has them, `sbx`. → [Risk 1](#1-the-credential-model-is-weaker-than-sbxs),
  [risk 5](#5-egress-is-unrestricted-and-on-docker-that-is-the-design)
- **A sandbox protects your host, not your repo**, and nothing in it contains a
  determined agent. The worktree branch is what protects your work.
  → [Risk 2](#2-a-sandbox-protects-the-host-not-the-repo)
- **The sandbox's git repository is the agent's to write**, so omh trusts
  nothing it asserts and checks what crosses back, on the host.
  → [Risk 2b](#2b-the-sandboxs-repository-is-agent-writable-its-push-wall-is-a-signpost)
- **Almost every bug lived where omh meets other software**, where the test
  suite cannot see — so `omh doctor` must be re-run, not trusted once.
  → [Risk 6](#6-adapter-facts-are-unverified-claims)
- **The risk most likely to kill the project is not technical**: curation is a
  recurring commitment. → [Risk 10](#10-curation-is-a-recurring-commitment-not-a-one-time-choice)

:::note[Status: current, and deliberately not tidied]
Closed risks stay on the page struck through, with the pull request that closed
them, because a risk that vanishes teaches nobody what it cost.
:::

## At a glance

| Risk | Where it stands |
|---|---|
| [1. The credential model is weaker than `sbx`'s](#1-the-credential-model-is-weaker-than-sbxs) | open — gated on the `sbx` backend |
| [1b. The account directory is writable, and every repo shares it](#1b-the-account-directory-is-writable-and-every-repo-shares-it) | narrowed — MCP servers, and Codex's hooks, are held to the launch's; Claude's settings are above what a session writes; omp's providers are the account's own, and its settings each session's |
| [2. A sandbox protects the host, not the repo](#2-a-sandbox-protects-the-host-not-the-repo) | inherent |
| [2b. The sandbox's repository is agent-writable](#2b-the-sandboxs-repository-is-agent-writable-its-push-wall-is-a-signpost) | narrowed |
| [2c. ~~`omh sNN rm` destroys the agent's own commits~~](#2c-omh-snn-rm-destroys-the-agents-own-commits) | closed in #58 |
| [2d. Host-side git in a session's worktree ran the agent's hooks](#2d-host-side-git-in-a-sessions-worktree-ran-the-agents-hooks) | narrowed |
| [3. sshd is an attack surface pointed at yourself](#3-sshd-is-an-attack-surface-pointed-at-yourself) | mitigated, and asserted by tests |
| [4. `carry_in` is read from a committed layer](#4-carry_in-is-read-from-a-committed-layer) | mitigated |
| [4b. ~~The carried-secret scan read its needle as a pattern~~](#4b-the-carried-secret-scan-read-its-needle-as-a-pattern) | closed |
| [4c. ~~The exclude list was frozen at the first launch~~](#4c-the-sandboxs-exclude-list-was-frozen-at-the-first-launch) | closed |
| [4d. What the carried-secret scan cannot see, it now says](#4d-what-the-carried-secret-scan-cannot-see-it-now-says) | narrowed, not closed |
| [4e. `ca_cert` widens what the sandbox trusts](#4e-ca_cert-widens-what-the-sandbox-trusts-on-purpose) | by design |
| [5. Egress is unrestricted](#5-egress-is-unrestricted-and-on-docker-that-is-the-design) | open — the design on Docker |
| [5b. `refs/omh/turn` is a place the guards do not look](#5b-refsomhturn-is-a-place-the-guards-do-not-look) | bounded |
| [6. Adapter facts are unverified claims](#6-adapter-facts-are-unverified-claims) | open — `omh doctor` must be re-run |
| [7. Auth is verified but was hard-won](#7-auth-is-verified-but-was-hard-won) | verified |
| [8. Concurrent edits](#8-concurrent-edits) | open |
| [8b. ~~Three defects on the host side of git~~](#8b-three-defects-on-the-host-side-of-git) | closed |
| [8c. ~~An orphaned sandbox repository is adopted~~](#8c-an-orphaned-sandbox-repository-is-adopted-by-the-next-session-that-takes-its-id) | closed in #59 |
| [8d. ~~Two checkouts with the same directory name share everything~~](#8d-two-checkouts-with-the-same-directory-name-share-everything) | closed in 0.8.0; leftovers pruned since 0.10.0 |
| [8e. The git omh runs on the host inherits your environment](#8e-the-git-omh-runs-on-the-host-inherits-the-environment-omh-was-run-in) | open — argued, not measured |
| [9. Sandbox sprawl](#9-sandbox-sprawl) | open — `idle_timeout` is unset by default |
| [10. Curation is a recurring commitment](#10-curation-is-a-recurring-commitment-not-a-one-time-choice) | open — not a technical risk |

## Security

### 1. The credential model is weaker than `sbx`'s

**omh mounts a credential volume, so a compromised agent can read and
exfiltrate its own token.** `sbx` injects secrets at the egress proxy and the
agent never holds one.

Adopting that is the single largest security improvement available, and it is
gated on the
[backend spike](/concepts/architecture/#declared-capabilities-and-honest-unknowns).
Several accounts widens this gap rather than narrowing it.

A Codex [provider file](/guides/accounts/#a-model-provider-of-your-own) is one
more token the agent can read: its key is in `/etc/codex/config.toml`, which
the session cannot write but Codex — and so the agent — must read. An
`env_key` that used to stay in your shell is, once `omh account login codex` resolves
it, a value in a `0600` file on disk.

### 1b. The account directory is writable, and every repo shares it

**Reading the token is not the only thing a writable account mount allows.**
omh mounts each `creds` path read-write, because OAuth tokens refresh in
place, and a captured account lives under `~/.omh/creds/<harness>/<name>` —
one directory for every repo that uses it. For Claude Code that is all of
`~/.claude/` and, until #197, `~/.claude.json` — each session now gets its own
copy of that one, below; for omp, all of `~/.omp/agent/`, where `config.yml`
is each session's own since #197; for Codex, `~/.codex/`, where `config.toml`
is too.

**Those directories hold more than credentials.** By the adapters' own
accounts of what each harness reads:

- `~/.claude/CLAUDE.md` is Claude Code's personal memory file, and
  `~/.claude.json` holds user-scope MCP servers nested per project — and every
  session's project is `/work` (each session's own copy since #197);
- `~/.codex/AGENTS.md` is Codex's user-scope instructions;
- omp discovers every module under `~/.omp/agent/hooks/pre/`, and omh's own
  `omh.ts` is only one file in that directory.

So an agent in one repo can leave instructions, a server or a hook there, and
every later session of every repo on that account loads it — where `carry_in`
may have put that repo's secrets in reach.

**Narrowed for MCP servers and Codex's hooks** (#197). On Claude Code and
Codex omh fills the harness's own policy layer on every launch — see
[`[policy]`](/reference/adapters/#policy-what-a-session-may-load) — and it
admits exactly the servers the launch staged, by command and arguments. A
server a session adds with `codex mcp add` stays in the account and loads
disabled in that session, and in every later one, since every launch writes
its policy afresh from what it stages. On Claude Code `claude mcp add` refuses
it (measured by hand on 2.1.300, at one scope), and one written into
`~/.claude.json` directly is left out — so
is one with omh's own command and one more argument. `tests/e2e` asserts the
Codex case and the direct writes. Codex also runs managed hooks only, so a hook
written into `~/.codex/config.toml` does not run either (read in Codex's
source; nothing in `tests/e2e` plants one).

**Closed across sessions on Claude Code** (#197): an entry in `~/.claude.json`
with the *same* command and arguments as one of omh's servers and an `env` of
its own passes the lock, since the allowlist compares no `env` (measured in
#197). So each session now gets its own copy of `~/.claude.json`, seeded from
the account's without any server, switch, permission grant or consent, and the
legacy `~/.claude/.config.json` Claude Code would read instead is masked — see
[`~/.claude.json` is each session's own](/guides/accounts/#claudejson-is-each-sessions-own).
Nothing a session writes there reaches the account but the preferences its
adapter declares, once someone keeps them in the review; a session's own
relaunch still reads what it wrote into its copy. `tests/e2e` launches with an account
holding such an entry — at the top, under `/work`, and in `.config.json` — with
omh-memory switched off and a permission granted, checks the entry is one the
lock admits, and asserts none of it reaches the session and nothing the
session writes reaches the account. The login, `omh account login`, which
mounts the account's own file, cleans it the same way first and admits no
server.

**omh's hooks are above every settings file a session writes, on Claude
Code** (#197). omh's settings reach Claude by `--settings`, from a read-only
file every `claude` in the sandbox is handed — see
[Claude's settings document](/reference/adapters/#claudes-settings-document)
— which ranks above user, project and local settings. A `disableAllHooks:
true` written into the worktree's `.claude/settings.local.json` switched omh's
hooks off for the session that wrote it, and `attribution` text there reached
the model; neither does now (`tests/e2e`, red before). `~/.claude/settings.json`
is each session's own copy, so a hook written there reaches neither the
account nor any other session (`tests/e2e`). A `claude` run by the real
binary's absolute path is not handed the document: that session's own
business, like any other file it can write.

**Preferences are reviewed before the account keeps them** (#197). A session's
preference changes — `/model`, a theme — reach the account only when someone
keeps them at a terminal, as values of keys the adapter declares, which never
name a program, grant a consent or carry text before the model. Nobody at the
terminal keeps nothing: `down` leaves them to review, `rm` refuses, and
`rm --yes` drops them (`tests/e2e`, red before — Codex's were kept without
asking). See [your preferences](/guides/accounts/#your-preferences-from-one-session-to-the-next).

**Closed across sessions on Codex** (#197): the requirements identity
compares neither `env` nor `cwd` (read in Codex's source), so an entry with
omh's command and arguments and an `env` of its own passes it. Each session
now gets its own `config.toml`: the account's is a link to the session's copy,
and the account keeps only the keys the adapter declares as preferences — see
[`~/.codex/config.toml` is each session's own](/guides/accounts/#codexconfigtoml-is-each-sessions-own).
A server, `developer_instructions`, a provider or a trust a session writes
there lasts that session. `tests/e2e` launches with an account whose shared
file holds `developer_instructions` beside a model, and asserts the turn runs
on the model and the model never sees the instructions — red with the shared
file mounted as before.

**Closed across sessions for omp's providers** (#197): omp reads custom
providers from `~/.omp/agent/models.yml` and trusts it at its next start — a
provider's `baseUrl` sends the turn, and the key it carries, where the file
says, and an `apiKey` or a header value starting with `!` runs through
`/bin/sh` (read in omp's source, 18.10.4). The account's providers now live in
`provider.yml` at its root, which no mount reaches, and each launch mounts
what omh renders from it over `models.yml`, read-only — see
[omp's providers](/guides/accounts/#omps-providers). omp reads neither
`models.yaml` nor `models.json` while `models.yml` is there. Its model cache,
`models.db`, is in the account too, and a cached model keeps its own address
(omp's source; not measured), so omh removes it before each launch.
`tests/e2e` has a session write a provider doing both into all three files and
leave a cache, and asserts that at the next start the cache is gone, the
command never ran and the turn reached the account's provider — red with the
render unmounted, and with the cache left.

**Closed across sessions for omp's settings** (#197): omp keeps its settings
in `~/.omp/agent/config.yml`, in the account, and what one session wrote
there every later one read: `enabledProviders`, `disabledProviders` and
`modelProviderOrder` choose among providers, `auth.broker.*` names a broker
omp fetches credentials through, and `extensions` lists extensions it loads
(read in omp's source, 18.10.4). Each session now gets its own `config.yml`:
the account's is a link to the session's copy, and the account keeps only the
keys the adapter declares as preferences — see
[`~/.omp/agent/config.yml` is each session's own](/guides/accounts/#ompagentconfigyml-is-each-sessions-own).
`tests/e2e` launches with an account whose shared file holds a setting that is
not a preference beside the model, and asserts the turn runs on the model and
omp reads the other setting as never set — red with the shared file as before.

**Still open:**

- an enterprise account's cloud bundle composes above omh's requirements: its
  allowlist is the union of both, and an organisation's
  `allow_managed_hooks_only = false` wins over omh's (read in Codex's source);
- the instruction files and omp's hook directory above. The mitigation for
  those is read-only overlays inside the account directory, the way omh
  already mounts its own files there;
- omp's logins, `agent.db` in the account: an API key stored there that
  starts with `!` runs through `/bin/sh` the way one in `models.yml` did (read
  in omp's source, `ai/src/auth/cascade.ts`; not measured). It reaches what the
  hook directory does — code one session leaves runs in the next — and closes
  with it;
- omp's model cache between two sessions running at once on one account:
  omh removes it only when a session starts.

Until then, an account used for untrusted repos is best kept separate from one
used for work — `omh account login <harness> -n <name>` captures as many as you like.

### 2. A sandbox protects the host, not the repo

**What protects the repo is the worktree branch** — which is why `omh sNN rm`
never deletes one holding work trunk does not already have. Conflating the two is how people end up surprised by what an
agent could reach.

### 2b. The sandbox's repository is agent-writable; its push wall is a signpost

**The agent commits into a gitdir omh mounts read-write**, so every file in it —
including the `pre-push` hook — is the agent's to change. That hook is
bypassable by `--no-verify` and by `core.hooksPath`, measured against git
2.55.0.

**It is not containment and is not meant to be**: with `curl` and
[risk 5](#5-egress-is-unrestricted-and-on-docker-that-is-the-design) below, a
push was never the narrow way out. What stops work reaching your remote is that
the repository has no remote and no commit of yours in it.

**omh does not trust anything that repository asserts** — authorship is stamped
and carried secrets are refused on the host, when work crosses back.

**Narrowed: its `config` is pinned.** The gitdir has to be writable because the
agent commits into it; its `config` does not. omh rewrites that file from its
own view at every launch and mounts it read-only, which stops the agent editing
that file — textconv drivers, `core.hooksPath`, `core.sshCommand`,
`protocol.file.allow` — at the mount rather than by omh remembering the right
`-c` flag at every call site forever.

**That pin holds one path, and this page said it closed the class.** It did
not, and #153 is what it cost to find out. Measured against git 2.56.0:

- a `commondir` file in the gitdir root sends git to a `config` the agent
  wrote, and the `status` behind `omh sNN log` ran a clean filter from it, on
  the host;
- a repository nested in the worktree and staged in the agent's index was
  entered by `status`, `diff` and `add`, under its own config;
- and the `git --git-dir=… checkout` omh printed for a detached HEAD ran a
  `post-checkout` hook the agent had left in the gitdir.

**What #153 changed.** Every host-side git over that repository now goes
through one constructor, which sets `GIT_COMMON_DIR`; `status` and `diff` pass
`--ignore-submodules=dirty`, staging refuses a nested repository by name, and
omh no longer prints a host-side `git --git-dir=` for anyone to run. Refs still
follow `commondir` — git's ref store does not ask the variable — which hands the
agent nothing it did not already own.

**Measured inside a real container:** `commit`, `checkout -b` and
`reset --hard` never write that file and do not notice it, while `git config`
and `git remote add` meet `could not write config file /omh/shadow/config:
Resource busy` with nothing half-written. It is `Resource busy` rather than
`Read-only file system` because git replaces the file by renaming a lock over
it, and what refuses is the rename onto a mount point.

**Taking `git remote add` away closes the route git's own error message walks
the agent down**, so the arrangement now says adding a remote will not work
either. The `pre-push` hook keeps its job on the other route:
`git push <url> <ref>` needs nothing in config and still meets it.

**What remains open is everything else in 2b** — the hook is still bypassable
by `--no-verify`, and risk 5 is untouched. Measured with `chflags uchg` standing
in for a read-only bind mount — both make a rename-over fail — so the claim owes
a check inside a real container, which is `doctor`'s job.

**Not the way to make the wall firmer: an `.invalid` remote.** Configuring one
so that the agent meets a refusal sooner does the opposite, because `pre-push`
runs *after* git checks the remote. Measured 2026-08-21: against an unreachable
remote the push dies on `Could not resolve host` and the hook never runs, while
the same hook fires normally against a reachable one. It also puts
`origin/<branch> [gone]` into every `git status`, and turns "attempts nothing"
into "attempts a DNS lookup", which is not nothing on a network whose resolver
answers for names that do not exist. Worth remembering only as a note that a
hostname is a delivery surface — the error quotes the URL verbatim — should a
remote ever have to exist for another reason.

### 2c. ~~`omh sNN rm` destroys the agent's own commits~~

**Closed in #58.** `omh sNN rm` refuses over checkpoints no branch has, naming
how many and what to do, before it takes anything down — on a terminal it asks
first, and anything but yes is the refusal; `--yes` is the way past. Everything
below is what it was answering.

**Before #58, it was partly addressed** — what has already been harvested is on
the branch, and `--keep` is repeatable now, so the window is what the agent has
done since the last landing rather than the whole session. `omh sNN log` (#54)
shows them and counts them; refusing over them is step 12.

**The risk, as it stood.** The worktree's content is already on disk and the
branch survives, but checkpoints the agent made and you did not harvest with
`omh sNN commit --keep` go with the session — and after a `git reset --hard` in
the sandbox, those were the only copies.

**What `rm` counts.** `rm` asks for the count now and stands on it — and the
count is deliberately wider than the numbered list `log` prints. `seed..HEAD`
cannot see work the agent threw away with `reset --hard`, which is the sentence
above; measured, `--all --reflog --not <last handover>` can. It also catches a
branch the sandbox wandered off, which `preflight` already refuses a *harvest*
over while `rm` was dropping it for good.

**When it cannot read the sandbox.** A sandbox that never ran removes quietly.
One omh cannot read does not: that is a third answer rather than a quiet yes,
because the states that produce it — a truncated replay record, a repository
with no seed — are ones where the work is demonstrably still there and only its
classification is missing. `--yes` covers every refusal, so the door is never
held shut; the user is asked once.

### 2d. Host-side git in a session's worktree ran the agent's hooks

**Narrowed.** git resolves a relative `core.hooksPath` against the worktree it
runs in, and a session's worktree is the sandbox's `/work`. husky v9 writes
exactly that, `.husky/_`, into a repository's config. Measured against git
2.56.0: `omh sNN commit -m` ran `pre-commit`, `commit-msg` and `post-commit`
from the agent's files, `push` ran `pre-push`, `sync` ran `pre-rebase` and
`post-rewrite` in a scratch checkout of the session branch, reopening a branch
`rm` had kept ran hooks the branch itself carried, and the `git status` behind
`omh s` ran `post-index-change` — all on the host, as you.

**What changed.** The git omh runs in a session's worktree, or in a checkout of
its branch, runs no hooks. Yours still run where you expect them — `commit`,
`push`, and the checkout that opens a session — when they live where the agent
cannot write: `core.hooksPath` unset, which is the repository's own
`.git/hooks`, or set to somewhere outside the worktree. When it points inside,
the landing runs none and says so:

```text
committed to omh/s01 (1 commit on the branch)
  hooks not run: core.hooksPath (.husky/_) is inside the session's worktree, which the agent writes
```

**What remains open.** A hook in `.git/hooks` is yours, but what it runs need
not be: a `pre-commit` that calls `lint-staged`, `make` or `npm test` reads its
configuration — `package.json`, a `Makefile` — from the worktree, and the agent
writes those. That is the general shape of running your own tools over the
agent's work on your machine, and no hook setting closes it. Review before you
land.

### 3. sshd is an attack surface pointed at yourself

**Loopback-only, per-repo keys, no password auth.** All three are asserted by
tests on what omh writes — the publish address, the key, and the
`-o PasswordAuthentication=no` family on the sshd line — because the failure
mode of getting the bind address wrong is publishing a shell inside your
sandbox to the local network.

**That sshd honours those flags is a fact about openssh**, and `omh doctor` is
where it gets proved, not the suite.

### 4. `carry_in` is read from a committed layer

**It is the only path by which a secret reaches the agent**, so patterns are
validated against escaping the repo. A teammate's commit should not be able to
copy your `~/.ssh` into a sandbox.

### 4b. ~~The carried-secret scan read its needle as a pattern~~

**Closed.** The carried-secret scan read its needle as a *pattern* rather than
as bytes. `--keep` searches the agent's commit messages for lines from the
files you carried in, and passed them to `git log --grep` without `-F` — and
which pattern language that is depends on the reader's `grep.patternType`.

**Measured 2026-08-21 against git 2.55.0**, on a subject quoting the secret
verbatim:

- a `*` in a secret was missed under **all three** settings;
- a `+` was missed under `extended` and `perl` (found under the `basic`
  default);
- and an unbalanced `[` was a fatal error everywhere — taking `--keep` down for
  that session entirely.

**A guard whose reach depends on the user's dotfiles is not a guard; `-F` pins
it.** The content half was always a literal pickaxe and was never affected.

### 4c. ~~The sandbox's exclude list was frozen at the first launch~~

**Closed.** The sandbox's exclude list was frozen at the first launch.

**What it let through.** omh derives what that repository must not track from
the mounts it is about to make — its rendered rules, `.mcp.json`, the files you
carried in — and wrote the list once, when the repository was created. A
capability switched on later, or a `carry_in` entry added later, mounted a
document the existing sandbox neither tracked nor excluded, so the agent's own
`git add -A` swept omh's rendered file — MCP environment included — into a
history `omh sNN commit --keep` replays onto your branch.

**The list is rewritten on every launch now**, which touches no commit.

### 4d. What the carried-secret scan cannot see, it now says

**The scan works from *needles*** — the lines of the files you carried in — and
three kinds of line never become one:

- **A file that is not UTF-8 yields no needles at all**, so a carried keystore,
  `.p12` or DER key is protected only by its path: copy it under another name
  and nothing catches it, and `certs/` is the second example `carry_in`'s own
  documentation gives.
- **A line shorter than 12 characters, or one starting with `#` or `//`,** is
  dropped.
- **A carried file deleted or renamed in your checkout between launch and
  harvest yields nothing either**, which is strictly worse than the rotation
  caveat already documented in the code: rotation leaves a needle at the new
  value, deletion leaves none.

**Narrowed, not closed.** All three still fail open, and that is deliberate:
refusing a harvest because omh could not read a file would be a worse trade
than the gap. What changed is that they no longer fail *silent*.

**`Shadow::needles` returns what it could not read alongside what it could, and
both moments report it** — at launch, where you can still carry the file under
a name the scan can read, and at harvest, where the count would otherwise be the
same sentence whether omh had looked or not. Each file is named with its cause,
directories resolve to the file inside them rather than to the entry, and the
warning says the path check still stands so nobody reads it as *unguarded*:

```console
omh: the carried-file scan could not read these, so it cannot tell you whether
     their contents reached a commit:
    certs/deploy.p12 — it is not text, so there are no lines to search for
    certs/short.env — every line in it is too short or is a comment
  the path itself is still checked — what is not is a copy under another name
```

**An empty carried file is deliberately not reported**: it yields no needles
like the other three, but there is nothing in it to have been copied anywhere,
and a warning that fires on every empty `.env` placeholder is one nobody reads.

**What remains is the gap itself.** A copy of `deploy.p12` under another name
still reaches the branch unremarked — omh now tells you it cannot see that,
rather than implying it looked.

### 4e. `ca_cert` widens what the sandbox trusts, on purpose

**Behind a TLS-inspecting proxy nothing in a container verifies**, so no image
gets built at all — the setting exists because the alternative is that omh does
not work there.

**What it costs is real**: the sandbox then trusts a root that, by
construction, can sign a certificate for any host. That is already true of the
machine omh runs on, so this hands the sandbox no authority the user's own
laptop lacks, and it is the reason the setting names **one** certificate rather
than copying the host trust store in — the difference between inheriting one
decision IT already made and inheriting all of them.

**Two properties keep it honest:**

- **The certificate is written into the recipe** rather than passed as a build
  argument, so the image tag is a digest of a text that includes it and a
  rotated root cannot be a cache hit on an image trusting the retired one.
- **A path omh cannot read is an error**: resolving a typo to "no certificate"
  would rebuild exactly the image that was already failing and report success.

**What is *not* claimed: that every toolchain needed telling.** Measured against
a server presenting a leaf signed by a private root, in three arms — no root
installed, the root in the system store with no variables set, and the full
recipe. The middle arm is the one that isolates: only node still fails there,
with `UNABLE_TO_VERIFY_LEAF_SIGNATURE`, while curl, git, python, pip, go and
cargo all verify off the store, Debian's pip included. So one of the seven
variables is load-bearing and six are belt and braces; they are set anyway, and
described that way rather than as each being what makes its tool work.

**The recipe splits a chain into one file per certificate** for
`update-ca-certificates`, then concatenates it back into the single file
`NODE_EXTRA_CA_CERTS` names — because node reads that file and nothing else. A
guard pins the concatenation to the whole chain: narrowing it to the first
certificate used to leave the suite green while an intermediate-signed leaf
failed in node alone.

### 5. Egress is unrestricted, and on Docker that is the design

**An agent in a session can reach the network freely.**

**It is not an unfinished omh feature.** This page used to read *"the allowlist
is designed and not wired"*, which implied an omh feature somebody had failed to
finish. [Decisions](/project/decisions/) has recorded egress as **inherited
from the runtime** throughout, and
[architecture](/concepts/architecture/#runtime-backends) puts egress policy in
the `sbx` backend beside its keychain-backed credential injection. Those two
were right and this one was out of step — worth naming, because a risk page
that misattributes a gap sends the reader looking for the wrong fix.

**So it is not on omh's list to wire.** It arrives with a backend that has it,
or it does not arrive. That also means it shares a fix with
**[risk 1](#1-the-credential-model-is-weaker-than-sbxs)** — `sbx` injects
secrets at the egress proxy, so the agent never holds its own token — and the
two are one piece of work rather than two, gated on the
[v0.5 spike](/project/roadmap/).

**Until then: a Docker session has unrestricted egress, and omh does not claim
otherwise.**

### 5b. `refs/omh/turn` is a place the guards do not look

**The sandbox's gitdir is a read-write mount, and since 2026.08 three of omh's
own queries skip that ref** so per-turn snapshots are not mistaken for the
agent's stranded work. Anything the agent writes there is skipped by all three,
so an agent could park commits there and a harvest would drop them without the
refusal that normally stops it.

**Bounded deliberately:**

- it is one fixed ref rather than a glob, so there is exactly one such place;
- `omh sNN rm` names what would go, counting the ref directly through
  `Shadow::turns` rather than through the excluded queries;
- and `omh sNN log --turns` prints its contents **including each subject**,
  which is constant for omh's own snapshots — so anything else parked there is
  visible as the odd row.

**What an agent gains is the ability to lose its own work quietly**, which is
not a boundary it could not already cross with `git reset --hard`.

**An earlier version of this note said `unkept` still counts the ref.** It does
not — it carries the same exclusion as the other two. The mitigation is real but
it runs through a different query, and saying so matters: a refactor of
`Shadow::turns` that did not know it was load-bearing here would remove it.

**One config line defeats the exclusion.** `core.logAllRefUpdates = always`
makes git write a reflog for every ref under `refs/`, and `unkept`'s `--reflog`
arm is not reachable by `--exclude`. The effect is noise rather than a breach —
`rm` refuses for the rest of the session and only `--yes` gets past — and
`write_config` drops the key on every launch, so git falls back to its default
and the setting does not survive.

## Correctness

### 6. Adapter facts are unverified claims

**They are claims about external software that ships weekly, and they break
*silently*.** `omh doctor` is the only cure, and it must be re-run rather than
trusted once. Almost every bug this project has shipped lived at this boundary,
and **not one was catchable by the test suite.**

**Git was the case nobody noticed**: it is the external binary omh leans on
hardest, and `doctor` used to probe it not at all — not a version, not a
behaviour. It does now, by capability rather than by version number. On the
host, where the harvest runs, it reports `git --version` and asks the binary
whether it takes `cherry-pick --empty` — which #56 made a dependency of
`--keep <selection>` without being able to measure the floor — and
`merge-tree --merge-base`. In the sandbox it asks the same of `merge-tree`.

**What the [work loop](/project/roadmap/#v15--the-work-loop--060) added is
checked the same way.** `sync` merges with `merge-tree --write-tree` (git
≥ 2.38) and `--merge-base`, and both checks ask for `--merge-base` — the option a
sandbox on git 2.39.5 turned out to lack.

### 7. Auth is verified but was hard-won

**A real login, `omh account login claude --name <account>`, completes and persists.** Five
bugs preceded that, every one at the boundary with a real harness.

### 8. Concurrent edits

**You and the agent can write the same file.** No worse than running `claude`
natively, but no better, and the worktree makes it easier to forget that you
are both in there.

### 8b. ~~Three defects on the host side of git~~

**Closed.** Three measured defects on the host side of git, all fixed:

- **`omh sNN rm` deleted a branch it could not count** — a count with no answer
  now keeps the branch and says so.
- **A session could be created on trunk itself**, where git's remote DWIM
  overrode `worktree add -b` — the start point is a resolved commit now, and
  `default_branch` returns a ref this checkout can answer about.
- **`omh sNN commit --keep` was not repeatable**, replaying from where the
  session started, so a second run offered commits already on the branch and
  died applying them — it replays from what it last handed over now, and
  refuses rather than guessing when the sandbox has rewound below that point.

### 8c. ~~An orphaned sandbox repository is adopted by the next session that takes its id~~

**Closed in #59:** `omh s` looks at them now, beside the containers and run
directories it already reported. `omh <id> rm` clears one, and since #58 says
what it would take with it first.

**The risk, as it stood.** Ids are the highest `sNN` plus one, so they come back
around, and `omh sNN rm` deletes the repository with the session precisely to
stop this. A worktree removed outside omh — which `omh s` already expects, since
it reports what a hand `git worktree remove` strands — leaves the gitdir and its
seed behind, and the next session with that id opens on the previous one's
history, against a seed naming a tree it never had.

**Why it mattered most.** `omh s` looked at containers and run directories and
not at these — the most valuable of the three, since a container is re-creatable
and a run directory holds a timestamp, while this holds every commit an agent
made.

### 8d. ~~Two checkouts with the same directory name share everything~~

**Closed in 0.8.0.** Worktrees, run state, ssh keys, sandbox repositories, the
note store, the cache volume, the network and container names were all keyed by
the checkout's basename, so `~/work/api` and `~/oss/api` were one repo as far
as omh was concerned — and the second one resumed into the first one's session:
a live container holding another project's code, reached by typing an ordinary
command in an ordinary checkout.

**The key is now the basename *and* a digest of the canonical path** —
`api-3f9a2c1b` — composed in `Paths::repo_id`, which every per-repo accessor
routes through. The name stays in front because people read these: `omh s` prints them
and `docker ps` lists them, and `omh-3f9a2c1b-s01` says nothing about which
checkout it is. `omh info --repo` reports it, so the question the collision used
to create has an answer.

**The digest is FNV-1a written out rather than `DefaultHasher`**, and the
reasoning is the same one `container::labels` already records: std does not
guarantee that hasher's output across releases, and this value names
directories holding an agent's commits. Drift would not break loudly, it would
strand every session and open an empty one where the work used to be. A test
pins it to FNV's published vectors rather than to its own output, so a rewrite
has something to fail against.

**The migration reads ownership rather than assuming it.** A worktree's `.git`
is a file naming the checkout it belongs to, so omh does not have to guess which
of two `api`s owns `~/.omh/worktrees/api`:

- **a pointer naming this checkout** moves everything;
- **a pointer naming another one** refuses, says whose it is, and leaves it for
  that checkout to claim on its next run;
- **no worktrees at all** moves the rest, since there is no session to collide
  over and stranding an `init`-only repo's notes would be worse;
- **a running sandbox** refuses outright — its mounts point at the directory
  being renamed.

And state under the old key that *cannot* move, because the new key is already
in use, is reported rather than passed over in silence: nothing reads it again,
and silence is what let this survive.

**What the migration left behind.** The cache volume and the network are
recreated rather than renamed — they are derivable and docker cannot rename
either — and **nothing reports the old pair.** An earlier draft of this page
said `omh s` did. It does not: `leftovers` reads the shadow directory, the run
directory and `docker ps -a` filtered by the *new* container prefix, and omh
issues no `volume ls` or `network ls` anywhere. A stopped container under the
old key is missed for the same reason — the migration only refuses over
*running* ones.

So after a migration `omh-cache-<basename>`, `omh-<basename>` and any stopped
`omh-<basename>-sNN` are orphaned and unmentioned. They cost disk and a name,
not correctness.

**Fixed in 0.10.0.** `omh init` records the checkout's path beside the id
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
`image::superseded` already implements, and `prune` does not call it: only the
reap after an image build does.

### 8e. The git omh runs on the host inherits the environment omh was run in

**Nothing clears `GIT_*` before a host-side call.** The calls over the sandbox's
repository name their gitdir, so an ambient `GIT_DIR` does not redirect those;
the ones in your checkout and in a session's worktree rely on the directory they
are run from, and a `GIT_DIR` or `GIT_WORK_TREE` already in the environment —
omh run from inside a hook, or from `rebase --exec` — would send them somewhere
else wholesale.

**Argued, not measured**: no report of it, and no test that sets one. The
environment is yours rather than the agent's, which is why this is here and not
under Security.

## Operational

### 9. Sandbox sprawl

**One container per session.** `idle_timeout` stops sessions nobody has used,
on the next launch — but it is **unset by default**, so sprawl is opt-out rather
than prevented, and a machine that never launches again never reaps.

**The [work loop](/project/roadmap/#v15--the-work-loop--060) narrows it from the
other end**: a dashboard saying which sessions hold nothing you have not already
taken is what makes deleting them safe, and an `rm` that refuses over
unharvested commits is what would make a stronger default policy safe to set.

### 10. Curation is a recurring commitment, not a one-time choice

**The base set goes stale.** Re-choosing quarterly is the real ongoing cost of a
distribution and the honest reason a solo one is hard. This is the risk most
likely to kill the project, and it is not a technical one.

## Known rough edges

- An agent can query **another session's graph** for the same repo. Mitigated by
  naming the project in three places, not prevented. See
  [code graph](/guides/code-graph/#what-the-agent-can-still-see).
- **`.claude.json` is a single-file mount** that cannot be atomically replaced.
  `doctor` reports it rather than pretending otherwise.
- **Only `claude` has been driven for real work.** `codex`, `opencode` and `omp`
  pass `doctor`, which proves their paths and nothing about their behaviour.
  `omp` carries one gap beyond that: its login probe greps for a field name read
  out of oh-my-pi's source, and no logged-in run has confirmed it.
- **Copies an older omh seeded stay until `omh upgrade`.** omh's servers are
  generated with their features now, but a `~/.omh/mcp.json` written before
  holds copies of them. Nothing reads those, `omh why` names them as not read,
  and `omh upgrade` takes them out — keeping the file as it was in
  `mcp.json.yours` when a copy was not what omh ships now.
- **`omh why` answers about one repo's profile.** It reads the layers resolved
  for the current repo, so the same entry can legitimately answer differently in
  two checkouts. That is correct and occasionally surprising.

## Next

<div class="cards">

- [Sessions](/concepts/sessions/#the-agent-has-git-and-it-is-not-yours)\
  The sandbox's own repository, and what crosses back to yours.
- [Architecture](/concepts/architecture/#runtime-backends)\
  The runtime backends, and why `sbx` is the fix for risks 1 and 5.
- [Troubleshooting](/guides/troubleshooting/#omh-doctor)\
  `omh doctor`, the check this page keeps sending you to.
- [Roadmap](/project/roadmap/)\
  Which of these is scheduled, and what decides it.

</div>
