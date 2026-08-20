# Accounts

```console
$ omh auth claude personal
$ omh auth claude work
$ omh -a work claude              # or, per project: omh repo set account work
```

An **account is a captured snapshot of a harness's own credential files.**
`omh auth` runs the harness's real login flow and keeps whatever it wrote.

## Keyed by harness, not by provider

A harness is what can actually be captured. Two harnesses talking to the same
provider still each need their own login, and there is no useful sense in which
they share one.

Which account a session uses is a **project-level setting**, resolved through
the [usual three layers](configuration.md#settings-and-their-three-layers) — because that is
how it genuinely varies. This repo is work; that one is personal.

```
~/.omh/creds/<harness>/<account>/
```

## Mount the directory, never the token file

```toml
creds = ["$HOME/.claude/", "$HOME/.claude.json"]   # trailing slash = directory
token = ["$HOME/.claude/.credentials.json"]        # what proves a login
```

**You cannot `rename()` over a bind-mounted file.** It is a mount point, so the
kernel returns `EBUSY`:

```
mv: cannot move '.tmp1' to '.credentials.json': Device or resource busy
```

Every tool saves a token by writing a temp file and renaming over it. Mounting
the token *file* therefore means the login succeeds and silently never persists
— you log in, it says success, and the next launch is logged out. Mounting the
enclosing **directory** makes that rename an ordinary operation inside it.

That collides with capability mounts living in the same directory, because
Docker refuses to create a mountpoint inside a bind-mounted host directory
("is outside of rootfs"). `omh auth` therefore prepares those mountpoints on the
host first: `.claude/skills`, `.claude/commands`, `.claude/agents`,
`.claude/settings.json`.

Docker also creates any missing mount parent as **root**, which leaves the agent
unable to write beside its own config — transcripts fail with `EACCES` and
atomic writes have nowhere to put their temp file. The harness image therefore
creates and owns every directory it will mount into, derived from the adapter.

None of that is guessable from documentation. All of it came from running the
thing and reading the error.

## `token` is what proves a login

A harness fills its config directory just by starting: Claude Code writes
`statsig/`, `projects/` and `todos/` on boot. Inferring a login from "does this
directory contain anything" therefore reports success for a session with no
token at all — which is exactly the bug it replaced.

So the adapter names the file, and `is_captured` is *defined* as
`unfilled(...).is_empty()`, so the two answers can never drift apart.

Placeholders must be **parseable**, not merely present. `prepare` writes `{}`
into JSON files, because an empty file is valid TOML and valid YAML but not
valid JSON, and a harness that parses its config on startup refuses to run:

```
Claude configuration file at /home/agent/.claude.json is corrupted:
JSON Parse error: Unexpected EOF
```

A placeholder is recognised as one, so it never counts as a login.

## What stops a launch

- **Not being logged in is fine.** The harness prompts, which is what you want
  before your first `omh auth`.
- **An account you named and do not have** stops it. Running with no credentials
  produces a session that is logged out for reasons nothing explains.
- **Two identities and no stated preference** stops it too. Guessing would send
  work traffic through a personal account and never mention it.
- **Account names are validated as single path components.** Credentials mount
  writable, so `omh auth claude ../../..` would otherwise resolve to `~` and
  hand the agent your real credential store.
- **The runtime's exit status is checked before the filesystem.** A container
  that failed to start leaves the previous credentials in place, which would
  otherwise read as a successful re-authentication.

## The invariant this bends

> The writable mounts are a short named set — the worktree, credentials,
> carried files, the sandbox's own git repository, and two caches.
> **Nothing else is.**

This is the weakest part of the current security model, and it is weaker than
what `sbx` offers: there, secrets are injected at the egress proxy and the agent
never holds its own token at all. omh mounts a credential volume, so a
compromised agent **can** read and exfiltrate its own token.

Several accounts widens that gap rather than narrowing it. See
[risks](design/risks.md) and [runtime backends](design/architecture.md#runtime-backends).

## Verifying

`omh doctor` probes the token path by attempting the temp-file-plus-rename that
every token save performs:

```
  ✓ token      /home/agent/.claude/.credentials.json (atomic write)
```

It is non-destructive by construction. See [Troubleshooting](troubleshooting.md).
