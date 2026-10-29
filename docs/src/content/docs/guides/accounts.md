---
title: Accounts
description: Log in to a harness with omh auth, keep several logins side by side, and choose which one each project uses.
---

Log in to a harness once, keep a work and a personal login side by side, and
choose which one each project uses.

**You need** omh [installed](/getting-started/installation/), Docker running,
and a git repository.

## Log in and choose an account

<div class="steps">

1. Log in once for each account.

   ```console
   $ omh auth claude --name personal
   $ omh auth claude --name work
   ```

   `omh auth` runs the harness's real login flow and keeps whatever it wrote.
   An **account is a captured snapshot of a harness's own credential files**,
   one directory per harness and name — see
   [Keyed by harness](#keyed-by-harness-not-by-provider).

2. Choose the one this project uses.

   ```console
   $ omh set account work            # which one this project uses
   ```

   The account is a project setting, so another repository can stay on
   `personal`.

3. Start a session.

   ```console
   $ omh new claude
   ```

</div>

To check that the login will last past this session, run `omh doctor` — see
[Verifying](#verifying). Codex's default sign-in cannot finish in a sandbox:
[log in with a device code, or copy a login](#a-login-that-cannot-finish-in-a-sandbox).

## A login that cannot finish in a sandbox

Codex's default sign-in opens a browser and waits for the redirect on
`127.0.0.1:1455` — inside the container, where the host's browser never
arrives. Two ways round it:

- **Device code.** In `omh auth codex`, choose *Sign in with Device Code*:
  Codex prints a URL and a one-time code to enter in any browser, so no
  redirect has to reach the sandbox.
- **Import.** If you are already logged in on this machine, copy that login:

  ```console
  $ omh auth codex --name work --import
  `work` captured for codex
    copied from /Users/you/.codex/auth.json
  ```

**It is a copy of one login, not a second one.** An API key is a key, and two
copies of it work side by side. An OAuth login is a refresh chain: when the
provider replaces the refresh token on each use, whichever copy refreshes
second is holding a spent one and is logged out. If the host and the sandbox
both need to stay logged in, give the sandbox its own login with a device
code. A later login on the host does not reach the account either way.

<details>
<summary>What <code>--import</code> copies, and when it finds nothing</summary>

`--import` copies the adapter's `token` files and nothing else — not the config
directory around them, which also holds your config, skills and history, and
omh stages those separately. Every file is read before anything is written;
each is staged at `0600` and renamed into place only once all are staged, so a
file omh cannot read, or a write that fails, leaves the account as it was. A
harness whose login is not a file (omp keeps its credentials in SQLite) is
refused.

It only finds a login kept in that file. Claude Code on macOS keeps its login
in the Keychain, and a harness can be pointed at another home by its own
environment variable (`CODEX_HOME`); in either case `--import` says what it
looked at and finds nothing. That is also why a launch offers `--import` only
when there is a login there to copy.

</details>

## A model provider of your own

A harness talks to its vendor's models unless its config names another
provider — Azure, a gateway, a local server. Where the harness keeps that
choice is a file in the account, which every session on it can rewrite. omh
keeps it with the account instead, in a file at the account's root that no
session reaches, in the harness's own schema. Codex and omp take one so far.

### Codex's provider

Codex talks to OpenAI unless its config names another provider. omh keeps
that choice in a file of the account's own:

```console
$ omh auth codex --name work --provider ~/.codex/config.toml
`work` captured for codex
  model provider azure kept in /Users/you/.omh/creds/codex/work/provider.toml
```

`--provider` takes a Codex `config.toml` or a file holding only the provider,
and logs nothing in. `--import` brings the provider in `~/.codex/config.toml`
along with the login. An account that holds only a provider counts as
captured — a provider with an API key needs no login.

**Only the provider is kept.** omh takes `model_provider` and the
`[model_providers.*]` tables, in Codex's own words, and nothing else from the
file — `model`, `[mcp_servers]`, hooks stay out. Each launch writes them into
`/etc/codex/config.toml`, Codex's System layer, which the session cannot write.
The file sits at the account's root, `~/.omh/creds/codex/<account>/`, which is
never mounted, so no session can change which provider later sessions use.

**What the host supplies is settled once, at `omh auth`.** A sandbox has none
of the host's environment, commands or loopback, so:

| In your config | What omh does |
|---|---|
| `env_key = "AZURE_KEY"` | reads `AZURE_KEY` from the shell running `omh auth` and keeps its value as `experimental_bearer_token`; refused by name if it is unset |
| `env_http_headers` | the same, into `http_headers` |
| `env_key_instructions` | dropped |
| `auth` (a command), `gateway_oauth`, `aws` | refused: a sandbox cannot run your command or read your AWS profile. Put the token the command produces in a provider file and name that |
| a `base_url` on `127.0.0.1` or `localhost` | refused: in a sandbox that is the sandbox itself. Name an address it can reach — on Docker Desktop, `host.docker.internal` |

A refusal keeps nothing. A file you write by hand is held to the same
allowlist at every launch: a key Codex does not read as a provider field, or
anything outside the provider tables — `notify`, `[hooks]`,
`developer_instructions`, which in the System layer would run as managed — is
refused and named, and the launch stops. So does a file that is not TOML,
named by line and column.

**The values are secrets, and treated as such.** `provider.toml` is `0600`,
written so from its first byte, and so is the System layer omh stages with it.
omh prints provider names, never values. `omh eject` writes the provider with
its token, headers and query parameters as `<redacted>`, and a URL's userinfo
and query too.

**Nothing in a session's own config outranks it.** Codex ranks its System
layer below the user's `config.toml` (read in Codex's source, 0.162.1), but a
session's `config.toml` is its own copy and holds only your preferences —
`model_provider` is never one — so the provider file is what selects. See
[`~/.codex/config.toml` is each session's own](#codexconfigtoml-is-each-sessions-own).

`omh doctor` has a row for it:

```
  ✓ model provider   `azure` selected — defines azure, from /Users/you/.omh/creds/codex/work/provider.toml
```

A `✗` there is a file Codex would refuse to start with, and doctor stops
before building anything. An account with a provider and no login has no
token to probe, so doctor leaves credentials unchecked and says so.

### omp's providers

omp reads custom providers — a gateway, a local server, a built-in
provider's address overridden — from `~/.omp/agent/models.yml`, and trusts
what it finds there at its next start: a provider's `baseUrl` sends the turn,
and the key it carries, wherever the file says, and an `apiKey` or a header
value starting with `!` runs through `/bin/sh` (read in omp's source,
18.10.4). In the account that file is every session's to rewrite. So omh keeps
your providers in a file of the account's own:

```console
$ omh auth omp --name work --provider ~/.omp/agent/models.yml
`work` captured for omp
  model provider corp kept in /Users/you/.omh/creds/omp/work/provider.yml
```

`--provider` takes an omp `models.yml` and logs nothing in. `--import` reads
the one in `~/.omp/agent/`, or the `models.yaml` beside it, along with the
login. An account that holds only providers counts as captured.

**Only the providers are kept, in omp's own words.** `provider.yml` is a
`models.yml`: `providers:` and nothing else, each provider's fields from omp's
own list. Each launch renders it over the `models.yml` omp reads, read-only —
`{}` when the account has none — so no session can change the providers later
sessions use. With `models.yml` there, omp reads neither `models.yaml` nor
`models.json`.

**What the host supplies is settled once, at `omh auth`:**

| In your `models.yml` | What omh does |
|---|---|
| `apiKey: CORP_KEY`, or a header value naming a variable | reads it from the shell running `omh auth` and keeps its value, as omp would; a name in capitals that is unset is refused — omp would send it as the key |
| an `apiKey` or a header value starting with `!` | refused: a sandbox cannot run your command. Put the value it prints in the file |
| a `baseUrl`, a model's or a provider's, or a `remoteCompaction` endpoint on `127.0.0.1` or `localhost` | refused, as for Codex |
| `enabledProviders`, `disabledProviders`, `modelProviderOrder`, `auth` | refused in a provider file: they are `config.yml` keys, which omp still reads from the account |

A file under `~/.omh` is read with nothing from your environment: a session
could have written it, and a variable resolved for it would send your secret
wherever the file's address says. Check it, copy it out, and name the copy.

**The account's own `models.yml`.** omp read it straight from the account
until now, and a launch that finds providers defined there — or in
`models.yaml` or `models.json` — with no provider file stops and names them,
with the command that keeps them. A session could have written them, so they
are never adopted unasked:
[A launch says the account's own models file defines providers](/guides/troubleshooting/#a-launch-says-the-accounts-own-models-file-defines-providers).

**The model cache goes before each launch.** omp keeps the model lists it
fetches in `models.db`, in the account, and a fresh cached model keeps its own
address (omp's source; not measured). omh removes `models.db` and its `-wal`
and `-shm` from the account before each launch, and omp rebuilds it at its
next start. Two sessions running at once on one account still share it.

The values are secrets, as for Codex: `provider.yml` and the `models.yml` omh
stages from it are `0600`, output names providers and never values, and
`omh eject` writes `models.yml` with every key and header value as
`<redacted>`. `omh doctor` has a row:

```
  ✓ model provider   defines corp, from /Users/you/.omh/creds/omp/work/provider.yml
```

An account whose providers are its login has no omp login to probe, so doctor
leaves credentials unchecked.

## Verifying

`omh doctor` probes the token path by attempting the temp-file-plus-rename that
every token save performs:

```
  ✓ token      /home/agent/.claude/.credentials.json (atomic write)
```

It is non-destructive by construction — see
[the credential probe](/guides/troubleshooting/#the-credential-probe). A `✗`
here is a login the next session will not have:
[Troubleshooting](/guides/troubleshooting/#i-logged-in-but-the-next-session-is-logged-out)
says what to do.

## Keyed by harness, not by provider

A harness is what can actually be captured. Two harnesses talking to the same
provider still each need their own login, and there is no useful sense in which
they share one. Each account is a directory of its own:

```
~/.omh/creds/<harness>/<account>/
```

Which account a session uses is a **project-level setting**, resolved through
the [usual two layers](/reference/configuration/#settings-and-their-two-layers) — because that is
how it genuinely varies. This repo is work; that one is personal.

### A different account for one harness

One name usually fits every harness. When it does not — accounts are captured
per harness, so a name one has may be one another lacks — name an account for
one harness with `<harness>:<name>`:

```console
$ omh set account mine
omh: `mine` is captured for claude
$ omh new codex
omh: no account `mine` for codex
  captured: work
  omh set account codex:<name>   one for codex alone
$ omh set account codex:work
omh: `work` is captured for codex
$ omh info --repo
settings
  account  mine        ← shared
  account  codex:work  ← shared
```

`account` stays one string until a harness differs, then becomes a table whose
`default` is the name for every other harness:

```toml
[account]
default = "mine"
codex = "work"
```

A harness's own entry wins at whatever layer it is set. A plain
`omh set account <name>` sets `default`; `omh unset account codex` takes back
codex's entry, and the table goes back to one string once no harness differs.

## What stops a launch

- **Not being logged in is fine.** The harness prompts, which is what you want
  before your first `omh auth`. omh says so first, because the harness's prompt
  names the harness's own login, and that login may be one that cannot finish
  in a sandbox:

  ```
  omh: no codex account — starting logged out. `omh auth codex` to log in, or `omh auth codex --import` to copy this machine's login
  ```
- **An account you named and do not have** stops it. Running with no credentials
  produces a session that is logged out for reasons nothing explains.
- **Two identities and no stated preference** stops it too. Guessing would send
  work traffic through a personal account and never mention it.
- **Account names are validated as single path components.** Credentials mount
  writable, so `omh auth claude --name ../../..` would otherwise resolve to
  `~` and hand the agent your real credential store.
- **The runtime's exit status is checked before the filesystem.** A container
  that failed to start leaves the previous credentials in place, which would
  otherwise read as a successful re-authentication.

## How accounts work

### Mount the directory, never the token file

omh mounts the harness's whole credential directory. The token file inside it
is named separately, as what proves a login:

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

<details>
<summary>What mounting the directory broke, and how omh works round it</summary>

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

</details>

### `~/.claude.json` is each session's own

One credential file is not the account's in a session: Claude Code's
`~/.claude.json`. It holds MCP servers, the `/mcp` switches and permissions
granted per project, and a session that wrote one there would hand it to every
later session on the account — including a server with omh's own command and
arguments and an `env.PATH` of its own, which the policy's allowlist passes. So
each session gets a copy, seeded from the account's when it first launches:

- **kept:** your identity (`oauthAccount`, `userID`), onboarding, the account's
  own API key, and everything else omh has no opinion on. Tokens stay in
  `.credentials.json`, which is still the account's, shared and refreshed in
  place;
- **dropped:** every MCP server and switch on one, `allowedTools`, the legacy
  `env` and `apiKeyHelper`, and the consents a session could leave — so an
  interactive session should ask again about bypass-permissions mode, an API
  key from the environment, and a `CLAUDE.md` that includes files from outside
  the project (inferred from the keys dropped; not measured). omh trusts
  `/work` itself (inferred to skip the trust dialog; not measured);
- **never written back:** nothing a session changes there reaches the account
  or another session. A `/mcp` switch lasts that session; to turn a server off
  for good, `omh unuse mcp <name>` — or `omh set memory off` and
  `omh set codegraph off` for omh's own `omh-memory` and `omh-codegraph`.

A `/login` inside a session replaces the account's shared tokens for every
session on it, but the identity it writes stays in that session's copy — later
sessions carry the account's old `oauthAccount` beside the new tokens.
`omh auth claude --name <account>` is how an account's identity changes: it
writes the account's own file, after cleaning it the same way, and setting it
aside if no session could be seeded from it.

The copy lives at `~/.omh/run/<repo>/<session>/.home/claude/.claude.json` and
goes with the session at `omh sNN rm`. It is seeded again — and what the
session made of it discarded — when the session's account changes, when that
account's own file does (after `omh auth claude` captures it again), or when the
copy is gone or no longer parses.

Claude Code 2.1.300 also reads `~/.claude/.config.json` *instead of*
`~/.claude.json` when it is not empty, and that one sits in the account's
shared `~/.claude/`; omh hides it behind an empty read-only file in every
session (read in the binary; a server planted there loaded in `tests/e2e`
until it was masked).

### `~/.codex/config.toml` is each session's own

Codex keeps its config in the account, at `~/.codex/config.toml`, and saves
into it as it runs. A session that wrote a server with an `env` of its own,
`developer_instructions`, a provider or a trust there would hand it to every
later session on the account. So the account keeps only your preferences, and
each session gets a copy of its own:

- **kept:** the keys the codex adapter declares under
  [`[preferences]`](/reference/adapters/#preferences-what-a-session-keeps) —
  `model`, `model_reasoning_effort`, `plan_mode_reasoning_effort`,
  `service_tier` and the `tui` settings (theme, keymap, status line, terminal
  title, session picker, transcript, agents overview, pet) — and the notices
  Codex records once you dismiss one;
- **not kept:** everything else — `[mcp_servers]`, `developer_instructions`,
  `model_provider`, `[features]`, `profiles`, `[projects]`, hooks. They last
  the session that wrote them. A feature you toggle lasts that session, and
  `-- -p <name>` finds no `[profiles.<name>]` table (a
  `~/.codex/<name>.config.toml` profile file still loads);
- **how:** the account's `config.toml` is a link to `/omh/prefs/config.toml`,
  and each session mounts its own copy there. Codex resolves the link before
  it saves, and saves into the session's copy (`tests/e2e`: `codex mcp add`
  writes there and leaves the link).

**What a session changed among its preferences reaches the account without
asking** — when Codex exits in your terminal, or you detach from it, at
`omh sNN down` and at `omh sNN rm`; changed keys only, and a value another
session kept meanwhile is named as replaced. omh says what it kept, and names
what the session wrote that is not a preference. A session that ended with
nobody watching — the idle reap, a reboot — keeps its changes at its next
`down` or `rm`; `omh prune` of a checkout that is gone does not. A later omh
asks before keeping (#197).

A session relaunched on another account keeps what it changed into the old
one first, then starts again from the new one. The account's kept values are
at `~/.omh/creds/codex/<account>/preferences/config.toml`; a session's copy at
`~/.omh/run/<repo>/<session>/.preferences/codex/live/config.toml`, and it goes
with the session at `omh sNN rm`.

<details>
<summary>The first launch after upgrading: the migration</summary>

An account captured by an older omh holds a real, shared `config.toml`. The
first launch on it:

1. backs it up, `0600`, to
   `~/.omh/creds/codex/<account>/preferences/config.toml.before-omh` — never as
   a `*.config.toml`, which Codex would load as a profile;
2. keeps its preferences as the account's;
3. lays the link, and records that it did.

It says what it kept and what it dropped, by key, never by value:

```
omh: codex's config.toml is per session from now on — kept model, tui.theme, dropped developer_instructions, mcp_servers; the file as it was is at /Users/you/.omh/creds/codex/work/preferences/config.toml.before-omh
```

`omh new --dry-run` and `omh doctor` say what it will do, and do none of it.
Each step can be interrupted: the next launch finishes the job.

**It refuses, and changes nothing:**

- while a session started by an older omh still mounts the account — it would
  rename over the link, or keep reading the shared file. `omh sNN down` it
  first. `omh auth codex` asks the same, once per account;
- when the file selects a model provider other than OpenAI and the account
  has no [provider file](#a-model-provider-of-your-own): every later session
  would quietly send its turns to OpenAI instead. The refusal names the fix,
  which reads the provider out of that same file:

  ```console
  $ omh auth codex --name work --provider ~/.omh/creds/codex/work/.codex/config.toml
  ```

After the migration, a real file where the link goes was put there by a
session: it is linked again, said, and nothing in it is kept.

</details>

**Codex on `sbx` waits for #199.** Laying the link — the first launch of any
codex account, new or not — asks which sandboxes still mount the account,
and `sbx` keeps no labels to answer from, so that launch refuses. `sbx` does
not run omh's links yet either.

### `token` is what proves a login

The adapter names the file that proves a login, and nothing else counts. A
placeholder omh writes is recognised as one, so it never counts as a login
either.

<details>
<summary>Why a full config directory is not a login, and why placeholders must parse</summary>

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

</details>

### The invariant this bends

> The writable mounts are a short named set — the worktree, credentials,
> carried files, the sandbox's own git repository, the harness's transcript
> directory, and two caches. **Nothing else is.**

This is the weakest part of the current security model, and it is weaker than
what `sbx` offers: there, secrets are injected at the egress proxy and the agent
never holds its own token at all. omh mounts a credential volume, so a
compromised agent **can** read and exfiltrate its own token.

Several accounts widens that gap rather than narrowing it. See
[risks](/concepts/risks/) and [runtime backends](/concepts/architecture/#runtime-backends).

## Next

<div class="cards">

- [Commands](/reference/commands/)\
  `omh auth`, `omh set` and every other command, with what each prints.
- [Configuration](/reference/configuration/)\
  Where `account` and every other setting lives, and how the two layers resolve.
- [Troubleshooting](/guides/troubleshooting/)\
  A login that does not last, and Codex's sign-in messages.

</div>
