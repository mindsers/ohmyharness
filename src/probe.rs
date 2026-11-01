//! The probe protocol: a script run inside a sandbox prints one line per
//! check, `ok` or `fail`, its name and a detail, tab-separated; [`parse`]
//! reads them back. `omh doctor`'s probes speak it, and so do a stack's
//! predicates and the check of which programs a sandbox has, whose answers
//! the facts store keeps.

use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Expect {
    /// The file exists and is not empty.
    NonEmptyFile,
    /// The file mentions each of these.
    Mentions(Vec<String>),
    /// The directory holds an entry for each of these.
    Entries(Vec<String>),
    /// `guest` is a JavaScript module that parses, and names each of these.
    ///
    /// The one render that emits a **program** rather than a configuration
    /// file, so it is the one that can be well-formed bytes and still be
    /// nonsense. `NonEmptyFile` passed for a module with a syntax error, for
    /// one where every hook had been dropped, and for one that threw on every
    /// event — while CONTRIBUTING puts this command above the suite as the only
    /// thing that verifies an adapter.
    Parses(Vec<String>),
    /// A temp file can be renamed over this path.
    ///
    /// The one failure omh cannot see from the host: a bind-mounted *file* is a
    /// mount point, so `rename()` onto it returns EBUSY. Every tool saves a
    /// token that way, so this decides whether a login can persist at all.
    AtomicWrite,
    /// `guest` answers an MCP handshake and names each of these tools.
    ///
    /// The other thing invisible from the host: whether a server omh
    /// *configured* can actually start where the harness will spawn it. Every
    /// host-side test proves the tool list is right about a host directory,
    /// which is circular in exactly the way this module exists to break.
    ///
    /// It does **not** prove that a harness reads the tool description each
    /// session. `doctor` replaces the launch command
    /// with this probe, so no harness ever runs, and a tool description is
    /// consumed by a model rather than written anywhere inspectable. What this
    /// proves is the precondition.
    Speaks(Vec<String>),
    /// The **harness's own** listing names each of these, on a line that also
    /// says it is running.
    ///
    /// `Speaks` asks omh's server whether it works; `Mentions` asks whether the
    /// document says what it should. Both passed for a year against a binding
    /// that pointed at a path Claude Code does not read, because neither one
    /// asks the only question that matters: did the harness load it. This is
    /// the check that can answer, and the only one that goes red when a harness
    /// changes where it looks.
    ///
    /// `ready` is matched on the same line as the name rather than anywhere in
    /// the output, because every other line of a listing is another server that
    /// may well be fine.
    ///
    /// `guest` is the **directory** the document lives in, and the probe runs
    /// `command` from there. A harness that finds its config by project root
    /// answers about whatever project it was asked from, so a probe run in the
    /// wrong directory is a confident answer to a question nobody asked.
    Loaded {
        command: String,
        names: Vec<String>,
        ready: String,
    },
    /// The harness's own answer to "are you logged in" says `ready`.
    ///
    /// `Loaded` without the names: there is nothing to match *per item*, so
    /// `ready` is looked for anywhere in the output rather than on a line with
    /// something else. The distinction `Loaded` draws — every other line is
    /// another server that may well be fine — has no analogue here, because a
    /// login is one fact.
    ///
    /// This exists because `AtomicWrite` cannot be asked of a harness that
    /// keeps no token file. omp keeps credentials in SQLite, and the database
    /// is created on first start by settings and telemetry, so the strongest
    /// host-side statement available is "a file exists that would exist
    /// anyway".
    Answers { command: String, ready: String },
    /// The harness starts: `command` exits 0.
    Starts { command: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Check {
    /// Label shown in the report.
    pub name: String,
    /// Path **inside the sandbox**, never on the host.
    pub guest: PathBuf,
    pub expect: Expect,
    /// Whether `guest` is a directory. Decides how the probe writes.
    pub dir: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Outcome {
    pub name: String,
    pub ok: bool,
    pub detail: String,
}

/// Shell run inside the sandbox. Emits one `ok|fail<TAB>name<TAB>detail` line
/// per check.
/// A probe that reports, for each program, whether it resolves where the script
/// runs.
///
/// A second builder rather than a fifth `Expect`: a `Check` is path-shaped —
/// `guest` is documented as a path inside the sandbox — and a toolchain has no
/// path, only a name. Widening `Check` to carry either would touch every check
/// that already works, to express a subject the existing ones never have.
///
/// What is shared is the thing that matters: the wire protocol. These lines go
/// through the same [`parse`] as every other probe, so there is one format and
/// one reader, and `doctor` can concatenate this script with its own.
///
/// `command -v` rather than `which`: it is POSIX, it is a shell builtin so it
/// needs nothing installed to answer, and it resolves builtins and functions
/// as well as files on PATH. `which` is not in POSIX and is absent from some
/// minimal images — a probe that needs a package installed to report a missing
/// package is a probe that reports on itself.
///
/// **This must run where the hook will run.** Whether `cargo` resolves is a
/// fact about one machine, and the machine that matters is the sandbox — not
/// the host, and not a login shell whose profile has added to PATH.
pub fn probe_programs(programs: &[&str]) -> String {
    let mut out = String::from("#!/bin/sh\n");
    for p in programs {
        let q = crate::quote::sh(p);
        out.push_str(&format!(
            "if command -v {q} >/dev/null 2>&1; then printf 'ok\\t%s\\tresolves\\n' {q}; \
             else printf 'fail\\t%s\\tnot installed in the sandbox\\n' {q}; fi\n"
        ));
    }
    out
}

/// Run a generated probe through a real `/bin/sh`, in `cwd`, and hand back its
/// stdout.
///
/// The older probes are asserted by searching their source text for a
/// substring, which cannot distinguish a script that works from one that merely
/// mentions the right word. These are POSIX `sh` and need no container, so they
/// can be *run* — and a probe is a program, so running it is the only assertion
/// that means anything.
///
/// Shared with `stack`'s predicate tests rather than copied, so both halves of
/// the wire format are exercised by the same runner.
#[cfg(test)]
pub(crate) fn run_probe_in(script: &str, cwd: &std::path::Path) -> String {
    let out = std::process::Command::new("/bin/sh")
        .arg("-c")
        .arg(script)
        .current_dir(cwd)
        .output()
        .expect("a probe must be a script /bin/sh can run");
    String::from_utf8_lossy(&out.stdout).into_owned()
}

pub fn probe_script(checks: &[Check]) -> String {
    let mut out = String::from("#!/bin/sh\n");
    for check in checks {
        // Every value as one quoted word, and never inside a `printf` format:
        // a path sat in bare single quotes, so a `'` in it broke the script,
        // and in the format, so a `%` was read as a directive.
        let p = crate::quote::sh(&check.guest.display().to_string());
        let n = crate::quote::sh(&check.name);
        match &check.expect {
            Expect::NonEmptyFile => out.push_str(&format!(
                "if [ -s {p} ]; then printf 'ok\\t%s\\t%s\\n' {n} {p}; \
                 else printf 'fail\\t%s\\t%s missing or empty\\n' {n} {p}; fi\n"
            )),
            // `node --check` inside the sandbox, for the same reason
            // `Expect::Speaks` runs a handshake there: the question is whether
            // the thing omh generated will load where it has to load.
            //
            // Copied to `.mjs` first, and that is load-bearing rather than
            // tidy: `node --check` on a `.ts` path **accepts anything** — it
            // took `export default (async () => ({ oops` without complaint —
            // so the obvious spelling of this probe would have been one more
            // check that cannot fail. The staged file keeps its `.ts` name
            // because that is what opencode loads.
            Expect::Parses(names) => out.push_str(&format!(
                "cp {p} /tmp/omh-probe.mjs 2>/dev/null; \
                 if ! err=$(node --check /tmp/omh-probe.mjs 2>&1); then \
                   printf 'fail\\t%s\\tdoes not parse: %s\\n' {n} \"$err\"; \
                 else missing=''; for w in {}; do grep -qF -- \"$w\" {p} || missing=\"$missing $w\"; done; \
                   if [ -z \"$missing\" ]; then printf 'ok\\t%s\\t%s\\n' {n} {p}; \
                   else printf 'fail\\t%s\\tmissing:%s\\n' {n} \"$missing\"; fi; fi\n",
                crate::quote::sh_words(names)
            )),
            // Preserve what is there: a probe that costs the user their token
            // is worse than no probe. The directory case writes a scratch file
            // and removes it; the file case renames byte-identical content back.
            Expect::AtomicWrite if check.dir => out.push_str(&format!(
                "if ( echo probe > {p}/.omh-probe.tmp && mv {p}/.omh-probe.tmp {p}/.omh-probe ) 2>/dev/null; \
                 then printf 'ok\\t%s\\t%s (atomic write)\\n' {n} {p}; \
                 else printf 'fail\\t%s\\t%s cannot be renamed over (EBUSY?)\\n' {n} {p}; fi; \
                 rm -f {p}/.omh-probe {p}/.omh-probe.tmp 2>/dev/null\n"
            )),
            Expect::AtomicWrite => out.push_str(&format!(
                "if ( cp {p} {p}.omh-probe && mv {p}.omh-probe {p} ) 2>/dev/null; \
                 then printf 'ok\\t%s\\t%s (atomic write)\\n' {n} {p}; \
                 else printf 'fail\\t%s\\t%s cannot be renamed over — a token saved here will not persist\\n' {n} {p}; fi; \
                 rm -f {p}.omh-probe 2>/dev/null\n"
            )),
            Expect::Entries(names) => out.push_str(&format!(
                "missing=''; for w in {}; do [ -e {p}/\"$w\" ] || missing=\"$missing $w\"; done; \
                 if [ -z \"$missing\" ]; then printf 'ok\\t%s\\t%s\\n' {n} {p}; \
                 else printf 'fail\\t%s\\tmissing:%s\\n' {n} \"$missing\"; fi\n",
                crate::quote::sh_words(names)
            )),
            // Three frames down a pipe: initialize, the notification the
            // protocol requires after it, then tools/list. Reading the reply
            // with grep rather than a parser keeps the probe a shell script,
            // which is the only thing that can run in there.
            Expect::Speaks(names) => out.push_str(&format!(
                "out=$( {{ printf '%s\\n' \
                  '{{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"initialize\",\"params\":{{\"protocolVersion\":\"2025-06-18\",\"capabilities\":{{}},\"clientInfo\":{{\"name\":\"omh-doctor\",\"version\":\"0\"}}}}}}' \
                  '{{\"jsonrpc\":\"2.0\",\"method\":\"notifications/initialized\"}}' \
                  '{{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"tools/list\",\"params\":{{}}}}'; }} | {path} 2>/dev/null ); \
                  missing=''; for w in {}; do printf '%s' \"$out\" | grep -qF -- \"$w\" || missing=\"$missing $w\"; done; \
                  if [ -z \"$missing\" ]; then printf 'ok\\t%s\\t%s\\n' {n} \"$(printf '%s' \"$out\" | grep -o 'The store [^.]*' | head -1)\"; \
                  else printf 'fail\\t%s\\tno reply naming:%s\\n' {n} \"$missing\"; fi\n",
                crate::quote::sh_words(names),
                // Not quoted: this "path" is the server's command line, its
                // arguments included, and runs as one.
                path = check.guest.display(),
            )),
            // Run from the document's own directory, which is the check rather
            // than incidental: a harness that finds its config by project root
            // answers about whatever root it was asked from.
            //
            // `grep` twice down a pipe rather than one pattern, because the
            // name and the ready word share a line in an order omh does not get
            // to decide. Line-wise rather than over the whole output for the
            // reason a note in this repo already records: a listing that names
            // every server means `contains` cannot tell *this* server is
            // running from *another* one being fine.
            Expect::Loaded {
                command,
                names,
                ready,
            //
            // The command and the ready word reach `printf` as arguments: in
            // the format, as they were, a `%` was a directive, and the ready
            // word's quotes were deleted to keep the format's quoting whole.
            } => out.push_str(&format!(
                "out=$( cd {p} 2>/dev/null && {command} 2>&1 ); missing=''; \
                 for w in {}; do printf '%s\\n' \"$out\" | grep -F -- \"$w\" | grep -qF -- {ready} || missing=\"$missing $w\"; done; \
                 if [ -z \"$missing\" ]; then printf 'ok\\t%s\\t%s (%s: %s)\\n' {n} {p} {cmd} {ready}; \
                 else printf 'fail\\t%s\\t%s in %s does not report as %s:%s\\n' {n} {cmd} {p} {ready} \"$missing\"; fi\n",
                crate::quote::sh_words(names),
                cmd = crate::quote::sh(command),
                ready = crate::quote::sh(ready),
            )),
            // `grep -F --`: a name is a name, not a pattern — read as one,
            // `a.b` was found in a file holding `axb`.
            Expect::Mentions(names) => out.push_str(&format!(
                "missing=''; for w in {}; do grep -qF -- \"$w\" {p} 2>/dev/null || missing=\"$missing $w\"; done; \
                 if [ -z \"$missing\" ]; then printf 'ok\\t%s\\t%s\\n' {n} {p}; \
                 else printf 'fail\\t%s\\tmissing:%s\\n' {n} \"$missing\"; fi\n",
                crate::quote::sh_words(names)
            )),
            // One fact, so `ready` is looked for anywhere in stdout rather than
            // on a line with a name beside it.
            //
            // **A login is the exit status and the marker, never the marker
            // alone.** The first version asked only whether `ready` appeared
            // anywhere in the command's combined output, and a harness that
            // errored out was then judged by the words its error happened to
            // contain: `harness usage --json` exiting 1 with "run /login to
            // obtain an accountId" on stderr reported a successful login. That
            // is the exact false positive `token-probe` exists to remove,
            // arriving through the check meant to replace it.
            //
            // stdout and stderr are kept apart for the same reason. The marker
            // has to come from the answer, not from a warning printed beside
            // it; both are still quoted back on failure, because "no account"
            // and "no such subcommand" are indistinguishable from an exit code
            // and decide whether the user runs `omh account login` or fixes the adapter.
            //
            // `grep -F`: `ready` is a marker an adapter author wrote, not a
            // pattern. Read as a basic regex, `usage: omp [options]` matches
            // any line holding one of `o i t p n s`, which is a far looser
            // check than anything anyone typed.
            //
            // `command` reaches `printf` as an **argument**, never inside the
            // format string. Interpolated into the format, a `%` in a command
            // was read as a directive and a `'` closed the quote and broke the
            // whole concatenated probe — taking every other check with it.
            // The exit status, and the harness's own words when it is not 0 —
            // a permission error, a missing library, a config it refused.
            // `command` reaches `printf` as an argument, as in `Answers`.
            Expect::Starts { command } => out.push_str(&format!(
                "out=$( cd {p} 2>/dev/null && {command} 2>&1 ); code=$?; \
                 if [ \"$code\" -eq 0 ]; then printf 'ok\\t%s\\t%s\\n' {n} {cmd}; \
                 else printf 'fail\\t%s\\t%s exited %s: %s\\n' {n} {cmd} \"$code\" \
                   \"$(printf '%s' \"$out\" | head -c 300 | tr '\\n' ' ')\"; fi\n",
                cmd = crate::quote::sh(command),
            )),
            Expect::Answers { command, ready } => out.push_str(&format!(
                "e=$(mktemp 2>/dev/null || echo /tmp/omh-login.$$); \
                 out=$( cd {p} 2>/dev/null && {command} 2>\"$e\" ); code=$?; \
                 err=$(cat \"$e\" 2>/dev/null); rm -f \"$e\"; \
                 if [ \"$code\" -eq 0 ] && printf '%s' \"$out\" | grep -qF -- {ready}; \
                 then printf 'ok\\t%s\\t%s reports %s\\n' {n} {cmd} {ready}; \
                 else printf 'fail\\t%s\\t%s exited %s without %s: %s\\n' {n} \
                   {cmd} \"$code\" {ready} \
                   \"$(printf '%s %s' \"$out\" \"$err\" | head -c 200 | tr '\\n' ' ')\"; fi\n",
                cmd = crate::quote::sh(command),
                ready = crate::quote::sh(ready),
            )),
        }
    }
    out
}

pub fn parse(output: &str) -> Vec<Outcome> {
    output
        .lines()
        .filter_map(|line| {
            // Anything that is not our protocol is runtime or harness noise,
            // and guessing at it would invent results.
            let mut parts = line.splitn(3, '\t');
            let status = parts.next()?;
            let name = parts.next()?;
            let detail = parts.next().unwrap_or("");
            let ok = match status {
                "ok" => true,
                "fail" => false,
                _ => return None,
            };
            Some(Outcome {
                name: name.to_string(),
                ok,
                detail: detail.to_string(),
            })
        })
        .collect()
}

pub fn passed(outcomes: &[Outcome]) -> bool {
    !outcomes.is_empty() && outcomes.iter().all(|o| o.ok)
}
