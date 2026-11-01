//! Which program a shell command runs.
//!
//! A leaf, so a hook's renderer and a stack's predicate can ask it without
//! depending on what `omh init` detects.

/// Which executable a command needs in order to run at all.
///
/// This is the generic kernel of toolchain detection, and it is deliberately
/// ignorant of every stack omh ships: omh is not a rust tool that also knows
/// npm. Give it a command string — one of ours or one somebody wrote by hand —
/// and it names the program that has to be on PATH.
///
/// `None` means *cannot tell*, never *nothing is needed*. The two failure
/// directions are not symmetric: missing a gap costs one confusing hook error,
/// which is the status quo, while inventing one makes omh drop a working hook
/// or interrogate somebody about a toolchain they already have. Everything
/// ambiguous therefore resolves to `None`, and no caller may act on it.
pub fn program(command: &str) -> Option<&str> {
    let word = command.split_whitespace().find(|w| !is_assignment(w))?;
    is_program_name(word).then_some(word)
}

/// Is this word a program name, or is it shell?
///
/// An **allowlist**. A list of metacharacters to reject says nothing about the
/// syntax it failed to think of, and the cost of missing one is not cosmetic:
/// `$(which cargo) test` yields a candidate of `$(which`, which resolves
/// nowhere, so omh reports a missing program in a repo whose cargo is fine,
/// asks about it, and records an answer that switches a working hook off for
/// everyone who clones. That is the expensive failure direction this module
/// opens by naming.
///
/// The set is what a program name is actually made of — a bare name, a version
/// suffix (`python3.11`), a path (`./scripts/test.sh`), a `+` or `-` in the
/// name (`g++`, `cargo-nextest`). Anything else is *cannot tell*.
fn is_program_name(word: &str) -> bool {
    !word.is_empty()
        && word
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.' | '/' | '+'))
}

/// `FOO=1 cargo test` runs cargo. A leading assignment is an ordinary shape for
/// a test command and names no executable, so reporting `FOO=1` missing would
/// be a gap omh invented. Recognising one word too many is safe — it costs a
/// `None` — so this stays permissive on purpose.
fn is_assignment(word: &str) -> bool {
    match word.split_once('=') {
        Some((name, _)) => {
            !name.is_empty()
                && !name.starts_with(|c: char| c.is_ascii_digit())
                && name.chars().all(|c| c.is_alphanumeric() || c == '_')
        }
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The generic kernel. omh is not a rust tool that also knows npm: it takes
    /// a command string it was given and reports which executable has to exist
    /// for that string to run. Every shipped stack goes through here, and so
    /// does a command somebody wrote by hand.
    #[test]
    fn the_program_a_command_needs_is_the_word_that_runs() {
        for (command, want) in [
            ("cargo test", "cargo"),
            ("npm run format", "npm"),
            ("pytest", "pytest"),
            ("ruff format .", "ruff"),
            ("go test ./...", "go"),
            ("gofmt -w .", "gofmt"),
        ] {
            assert_eq!(program(command), Some(want), "for {command:?}");
        }
    }

    /// The asymmetry that governs this whole feature. Failing to spot a
    /// missing program costs one confusing hook error — the status quo. Naming
    /// a program missing when it is not is worse: omh drops a hook that works,
    /// or interrogates somebody about a toolchain they already have. So when a
    /// command is not something we can read with confidence the answer is
    /// `None` — *cannot tell* — and no caller is entitled to act on it.
    #[test]
    fn a_command_we_cannot_read_names_no_program_rather_than_a_wrong_one() {
        assert_eq!(program(""), None, "an empty command runs nothing");
        assert_eq!(program("   "), None, "and neither does whitespace");
        // A leading environment assignment is an ordinary shape for a test
        // command, and it names no executable. Reporting `RUST_LOG=debug`
        // missing would be a gap omh invented.
        assert_eq!(program("RUST_LOG=debug cargo test"), Some("cargo"));
        assert_eq!(program("FOO=1 BAR=2 pytest"), Some("pytest"));
        assert_eq!(program("FOO=1"), None, "assignments alone run nothing");
    }

    /// A word carrying shell syntax is not a program name, and guessing one out
    /// of it fabricates a gap. `$(which cargo) test` otherwise reports a missing
    /// program called `$(which` — in a repo whose cargo is fine — and the
    /// recorded answer switches a working hook off for everyone who clones it.
    ///
    /// An allowlist, not a list of metacharacters to reject. A denylist says
    /// nothing about whatever syntax it failed to think of, which is the lesson
    /// `image::probe_args`' guard had to learn twice.
    #[test]
    fn a_word_carrying_shell_syntax_names_no_program() {
        for command in [
            "$(which cargo) test",
            "${CARGO:-cargo} test",
            "`which cargo` test",
            // Not a valid shell variable name, so `is_assignment` correctly
            // declines to skip it — which leaves it as the candidate, and it is
            // not a program either.
            "2FOO=1 cargo test",
            "cargo|tee test",
            "cargo&&true",
        ] {
            assert_eq!(program(command), None, "for {command:?}");
        }
    }

    /// And the ordinary shapes still resolve. An allowlist that refused real
    /// program names would be worse than the guess it replaced — it would
    /// invent a *silence* instead of a gap, and silence is what stops a real
    /// missing toolchain being reported at all.
    #[test]
    fn an_ordinary_program_name_still_resolves() {
        for (command, want) in [
            ("./scripts/test.sh", "./scripts/test.sh"),
            ("python3.11 -m pytest", "python3.11"),
            ("g++ -o x x.cc", "g++"),
            ("/usr/local/bin/cargo test", "/usr/local/bin/cargo"),
            ("cargo-nextest run", "cargo-nextest"),
        ] {
            assert_eq!(program(command), Some(want), "for {command:?}");
        }
    }
}
