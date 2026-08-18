# Security

## Reporting a vulnerability

Use GitHub's [private vulnerability reporting](https://github.com/mindsers/ohmyharness/security/advisories/new).
It reaches the maintainer without the report being public first.

If that is unavailable to you, email **nathanael@cherrier.dev** with `omh
security` in the subject.

Please do not open a public issue for a vulnerability.

Expect an acknowledgement within a week. This is a one-maintainer project two
releases old; that is an honest estimate rather than a service level, and it is
better to say so than to publish a number nobody is on call to meet.

## What is in scope

omh's job is to be a boundary, so the interesting bugs are the ones that move
it. In rough order of how much they matter:

- **A mount that is writable and should not be.** The invariant is that nothing
  beyond the session worktree and the credential files is writable from inside
  the sandbox. A stray `rw` is the difference between a sandbox and a
  suggestion.
- **Credentials reaching somewhere they should not** — mounted outside the path
  the harness reads, exposed to another session, or captured into the wrong
  account. Note that an account name is required to be a single path component
  precisely because the alternative mounts the user's real `~` writable.
- **The sshd used by `omh attach` reachable off-host.** It is meant to publish
  on `127.0.0.1` only; anything that binds it wider hands a shell in the
  sandbox to the LAN.
- **Escape from the session worktree to the host checkout**, or to another
  session's worktree.
- **Egress that bypasses policy** once the allowlist is wired.
- **A supply-chain hole in the base set or an adapter** — anything that causes
  omh to fetch and run something the profile did not declare.

## What is out of scope

- **The agent itself doing something destructive inside its own worktree.**
  That is the design: the agent is untrusted, the worktree is disposable, and
  `git diff` is the review. Damage confined to the worktree and its branch is
  the sandbox working.
- Vulnerabilities in Docker, podman, the harnesses omh launches, or an MCP
  server you configured — please report those upstream. If omh *invokes* one of
  them in an unsafe way, that is in scope and worth reporting here.
- Anything that requires an attacker who already has code execution on your
  host as your user.

## Known weak points

Stated here rather than left to be discovered, because the project's credibility
rests on the difference between an unverified claim and a disclosed gap:

- The **code graph store is shared across sessions of one repo**, so one session
  can query another's graph. Mitigated, not prevented.
- **`.claude.json` is a file mount that cannot be atomically replaced.**
- The **egress allowlist is designed, not wired.** Assume a session has network
  access.
- Only **Docker** has been driven for real work. The `sbx` backend declares
  capabilities that have not been exercised.

## Supported versions

The latest release is supported, and only that one: `0.5.0` today. Fixes land
on `main` and reach you in the next tag — there are no backport branches before
1.0, because one maintainer cannot honestly promise to keep two lines patched.
