# Editors

```console
$ omh s attach              # $OMH_EDITOR, then $EDITOR, then print every recipe
$ omh s attach zed
$ omh s attach code
```

## Why this exists

The worktree is a plain host directory, so this was never about file access —
you could open it in anything.

It is about **where the language server runs.** Dependencies are installed
Linux-side inside the sandbox while your host is probably macOS/arm64. A host
LSP means a second dependency tree that silently diverges from the one the agent
is actually building against, and the divergence shows up as your editor
confidently underlining code that compiles fine.

sshd runs in the session; the editor attaches to it. One dependency tree, shared
with the agent.

## The integration point is an SSH config include

Not an IDE plugin. omh manages one file:

```
# ~/.ssh/config.d/omh
Host omh-s01
  HostName 127.0.0.1
  Port 49201
  User agent
  IdentityFile ~/.omh/keys/ohmyharness/id_ed25519
  HostKeyAlias omh-s01
  StrictHostKeyChecking yes
  UserKnownHostsFile ~/.omh/keys/ohmyharness/known_hosts
```

Everything downstream then works without omh knowing it exists:

```console
$ code --remote ssh-remote+omh-ohmyharness-3f9a2c1b-s01 /work
$ zed ssh://omh-ohmyharness-3f9a2c1b-s01/work
$ ssh omh-ohmyharness-3f9a2c1b-s01
```

JetBrains Gateway works through the same alias.

The alias is `omh-<repo id>-<session>`, and the repo id is the checkout's
directory name plus a short digest of where it is —
`omh info --repo` prints it. The digest is there because two checkouts called
`api` would otherwise be one host: before 0.8.0 they shared an alias, and
attaching from either landed you in whichever session existed. It is stable
for a given checkout, so a bookmark keeps working.

Choosing an include over a plugin is what makes "any editor" true rather than
aspirational — the editors omh ships descriptions for are a convenience, not the
boundary of what works.

## Editors are data

`omh s attach <name>` means *attach this tool to the session*. Adding one is a
TOML file, not a code change, for the same reason [adapters](design/adapters.md)
are data:

```toml
# ~/.omh/editors/zed.toml
name = "zed"
bin  = "zed"
args = ["$URL"]
```

```toml
# ~/.omh/editors/code.toml
args = ["--remote", "ssh-remote+$ALIAS", "/work"]
```

Four shipped: `code`, `cursor`, `nvim`, `zed`.

An editor that is not installed is **not an error** — omh says so and prints the
URL, because launching nothing silently would be worse. An editor omh has never
heard of is also not an error, and omh will not guess a flag for it:

```console
$ omh s attach emacs
omh: no editor named `emacs` — see `omh info`
session s01 is up

  ssh://omh-ohmyharness-3f9a2c1b-s01/work
  ssh omh-ohmyharness-3f9a2c1b-s01

  code    code --remote ssh-remote+omh-ohmyharness-3f9a2c1b-s01 /work
  cursor  cursor --remote ssh-remote+omh-ohmyharness-3f9a2c1b-s01 /work
  nvim    ssh -t omh-ohmyharness-3f9a2c1b-s01 cd /work && nvim
  zed     zed ssh://omh-ohmyharness-3f9a2c1b-s01/work
```

Guessing would launch the wrong thing, which is harder to diagnose than being
told no.

## Three details that are load-bearing

- **The public key arrives as an environment variable, not a mount.** A
  bind-mounted `authorized_keys` lands with host ownership, and sshd silently
  refuses to read one it does not trust — a failure that presents as a wrong key
  and sends you looking in entirely the wrong place.
- **Ports are derived, not assigned.** An IDE bookmark points at the alias, which
  resolves through the port. A port that moved between restarts would break every
  saved window you have.
- **The `Include` is prepended** to `~/.ssh/config`, never appended. ssh applies
  the first matching block, so an include placed below someone's `Host *` would
  never win. Everything else in that file is left untouched.

## Security

**sshd binds `127.0.0.1` only.** On `0.0.0.0` you have published a shell inside
your sandbox to the local network, which inverts the entire point of the
project. There is a test for this.

Keys are per-repo, and password auth is off — sshd is started with
`PasswordAuthentication=no` and `PermitRootLogin=no`, not left on the package
default.

**The sandbox's host key is pinned.** omh generates one host key per repo,
mounts it into the session read-only, and the entrypoint installs it into
`/etc/ssh` as root. Each `Host` block says `StrictHostKeyChecking yes` against a
known_hosts omh writes beside the keys, keyed by `HostKeyAlias`. The reason is
the port: it is a hash of the repo and session names, so any local process can
listen on it, and the previous `StrictHostKeyChecking no` trusted whatever did.

## Verified

`ssh omh-ohmyharness-3f9a2c1b-s01` lands as `agent` (uid 1000) inside the sandbox, with
the worktree at `/work` and the profile in place.
