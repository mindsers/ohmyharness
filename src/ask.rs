//! The two questions of last resort.
//!
//! `omh init` derives what it can and asks what nothing could derive. After
//! stacks, provisioning, the hook catalogue and `derive`, exactly two things
//! can still be unknown, and neither is knowable from any file:
//!
//! - **how an ecosystem omh has never been taught is installed.** A repo with a
//!   `mix.exs` plainly *is* something; omh ships no elixir stack and cannot
//!   invent one. Only a person knows what puts `mix` on the PATH.
//! - **what command tests a project nothing else could speak for.** No stack,
//!   no lockfile, no runner, no declared script — and there is still, usually,
//!   a command.
//!
//! Both are tier 3 of `docs/design/adoption.md`'s table: nobody has encoded it,
//! so ask once and record the answer where it belongs. The recording is what
//! makes it one question rather than a wizard — a question re-asked every
//! `init` is exactly the thing omh sells itself as not having.
//!
//! ## Silence declines, and EOF stops
//!
//! Pressing Enter writes nothing. That has to be the safe answer, because it is
//! the one somebody gives when they do not know, are in a hurry, or are holding
//! the key down — and an answer omh invented from a blank line would be a stack
//! file in their repo, committed, describing an install command nobody chose.
//!
//! **A closed pipe stops the whole exchange**, rather than reading every
//! remaining question as declined. A CI runner with no terminal gets no
//! questions and no files, which is the same outcome; what must not happen is
//! omh recording answers on behalf of somebody who was never shown them. That
//! scar was earned once already by the question this module replaces.
//!
//! ## The terminal is handed in
//!
//! Reading `std::io::stdin()` directly would leave every rule above — what an
//! empty line means, what EOF means, what gets written — asserted by nothing.

use anyhow::{Context, Result};
use std::io::{BufRead, Write};
use std::path::Path;

/// What a question produced: a file to write, and what to say about it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Answer {
    /// Relative to `<repo>/.omh/`, so the caller cannot put it somewhere else.
    pub path: std::path::PathBuf,
    pub body: String,
    /// One line for `init`'s report.
    pub said: String,
}

/// Ask about an ecosystem omh recognises and cannot set up.
///
/// Two prompts, not one, and the second is not optional padding: `needs` is
/// what makes a provide verifiable at all. A stack whose outcome is unstated is
/// one nothing can check, so omh would install something, report success, and
/// have no way to notice it had not worked — which is the failure the whole
/// `install`/`needs` split exists to catch. Asking for a recipe and inventing
/// its outcome would be worse than not asking.
///
/// Writes `<repo>/.omh/stacks/<name>.toml`, which the loader reads beside the
/// shipped ones and which may not answer to a name omh ships.
pub fn how_is_it_installed(
    marker: &crate::stack::Marker,
    input: &mut dyn BufRead,
    out: &mut dyn Write,
) -> Result<Option<Answer>> {
    writeln!(
        out,
        "\n  {} is here, and omh has no {} stack.\n  \
         It can still build the sandbox — it just needs to be told how.",
        marker.file, marker.stack
    )?;
    let Some(install) = prompt(
        &format!("  what installs {}? (Enter to skip)", marker.stack),
        input,
        out,
    )?
    else {
        return Ok(None);
    };
    let Some(needs) = prompt(
        "  and what should then be on PATH? (space-separated)",
        input,
        out,
    )?
    else {
        // The recipe without its outcome is a provide nothing can verify, so
        // an exchange abandoned half-way writes nothing at all rather than a
        // file that would need editing before it worked.
        writeln!(out, "  nothing written — a stack needs both")?;
        return Ok(None);
    };

    let programs: Vec<&str> = needs.split_whitespace().collect();
    if programs
        .iter()
        .any(|p| crate::detect::program(p) != Some(*p))
    {
        writeln!(
            out,
            "  nothing written — `{needs}` is not a list of program names"
        )?;
        return Ok(None);
    }

    // Built as a value and serialised, never spliced: this is a command a human
    // just typed, so a quote in it would otherwise produce a file omh cannot
    // read back — and the first thing that reads it is omh.
    let body = toml::to_string_pretty(&toml::toml! {
        name = (marker.stack.clone())
        marker = (marker.file.clone())
        [[provide]]
        name = "toolchain"
        needs = (programs.iter().map(|p| p.to_string()).collect::<Vec<_>>())
        install = (install.clone())
        because = (format!("{} is what this project is written in", marker.stack))
    })
    .context("writing the stack you described")?;

    Ok(Some(Answer {
        path: Path::new("stacks").join(format!("{}.toml", marker.stack)),
        body,
        said: format!("stack      {} — from what you told it", marker.stack),
    }))
}

/// Ask what tests a project nothing else could speak for.
///
/// Fires only where the catalogue and `derive` both produced nothing for this
/// moment — **one** notion of covered, computed by the caller, rather than two
/// that could disagree about whether a repo already has a test hook.
pub fn what_tests_it(input: &mut dyn BufRead, out: &mut dyn Write) -> Result<Option<Answer>> {
    writeln!(
        out,
        "\n  omh found no way to test this project — no stack it knows, no \
         lockfile, no runner.\n  With one, the agent can check its own work \
         before handing it back."
    )?;
    let Some(command) = prompt("  what command runs the tests? (Enter to skip)", input, out)?
    else {
        return Ok(None);
    };

    let hook = crate::hook::Hook {
        on: crate::hook::Event::TurnEnd,
        stack: None,
        tools: Vec::new(),
        when: None,
        action: crate::hook::Action::Run(command.clone()),
    };
    let body = serde_json::to_string_pretty(&hook).context("writing the hook you described")?;
    // Through the real parser before it reaches disk. `Hook`'s fields are
    // public, so constructing one bypasses every rule `from_raw` enforces, and
    // this is the one place a hook body is text a person just typed.
    //
    // For a `run` that is only non-emptiness today, which `prompt` has already
    // ruled out — so this is belt to that braces rather than a check that
    // currently fires. Deliberately not more: `$FOO` in a `run` is ordinary
    // shell and the user may well set it, which is why `check_interpolation`
    // applies to injected *prose* and not to commands. What this does buy is
    // that a rule added to the format later applies here, in front of the
    // person who typed it, rather than at somebody else's next launch.
    if let Err(e) = crate::hook::Hook::parse(&body, "the hook you described") {
        writeln!(out, "  nothing written — {e:#}")?;
        return Ok(None);
    }

    Ok(Some(Answer {
        path: Path::new("hooks").join("test.json"),
        body: format!("{body}\n"),
        said: format!("hook       test — `{command}`, from what you told it"),
    }))
}

/// One line, trimmed. `None` for an empty answer **and** for a closed pipe, and
/// the caller must treat the second as a reason to stop asking.
fn prompt(question: &str, input: &mut dyn BufRead, out: &mut dyn Write) -> Result<Option<String>> {
    write!(out, "{question}\n  > ")?;
    out.flush()?;
    let mut line = String::new();
    if input.read_line(&mut line)? == 0 {
        return Ok(None);
    }
    let line = line.trim();
    Ok((!line.is_empty()).then(|| line.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn marker() -> crate::stack::Marker {
        crate::stack::Marker {
            file: "mix.exs".into(),
            stack: "elixir".into(),
        }
    }

    fn asked_about_stack(typed: &str) -> (Option<Answer>, String) {
        let mut out = Vec::new();
        let answer = how_is_it_installed(
            &marker(),
            &mut std::io::BufReader::new(typed.as_bytes()),
            &mut out,
        )
        .unwrap();
        (answer, String::from_utf8(out).unwrap())
    }

    fn asked_about_tests(typed: &str) -> (Option<Answer>, String) {
        let mut out = Vec::new();
        let answer =
            what_tests_it(&mut std::io::BufReader::new(typed.as_bytes()), &mut out).unwrap();
        (answer, String::from_utf8(out).unwrap())
    }

    /// The answer becomes a stack file omh can actually read back — which is
    /// the only thing that makes asking worth anything.
    #[test]
    fn what_somebody_types_becomes_a_stack_omh_can_load() {
        let (answer, said) = asked_about_stack("apt-get install -y elixir\nmix elixir\n");
        let answer = answer.expect("both questions answered");

        assert_eq!(answer.path, Path::new("stacks/elixir.toml"));
        assert!(
            said.contains("mix.exs"),
            "the question names its evidence: {said}"
        );

        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("elixir.toml"), &answer.body).unwrap();
        let loaded = crate::stack::load_dir(dir.path()).expect("must load");
        assert_eq!(loaded.len(), 1, "got: {loaded:?}");
        assert_eq!(loaded[0].name, "elixir");
        assert_eq!(loaded[0].marker, "mix.exs");
        assert_eq!(
            loaded[0].provides[0].install.as_deref(),
            Some("apt-get install -y elixir")
        );
        assert_eq!(loaded[0].provides[0].needs, ["mix", "elixir"]);
    }

    /// **Silence declines.** Enter is what somebody presses when they do not
    /// know, and an install command omh invented from a blank line would be a
    /// committed file describing a decision nobody made.
    #[test]
    fn an_empty_answer_writes_nothing() {
        assert_eq!(asked_about_stack("\n").0, None);
        assert_eq!(asked_about_tests("\n").0, None);
    }

    /// **EOF stops rather than declining every remaining question.** A CI
    /// runner gets the same outcome either way; what must not happen is omh
    /// recording an answer on behalf of somebody who was never shown it.
    #[test]
    fn a_closed_pipe_records_nothing() {
        assert_eq!(asked_about_stack("").0, None);
        assert_eq!(asked_about_tests("").0, None);

        // Including half-way through: a recipe with no outcome is a provide
        // nothing can verify, so the exchange writes nothing rather than a
        // file that would need editing before it worked.
        let (answer, said) = asked_about_stack("apt-get install -y elixir\n");
        assert_eq!(answer, None);
        assert!(said.contains("nothing written"), "and says so: {said}");
    }

    /// A `needs` that is not a list of program names is refused, because it is
    /// the half omh checks the sandbox against. `apt-get install elixir` typed
    /// into the second prompt would make the check permanently fail, and the
    /// report would name a program the user does have.
    #[test]
    fn a_needs_that_is_not_program_names_is_refused() {
        let (answer, said) = asked_about_stack("apt-get install -y elixir\nmix; rm -rf $HOME\n");
        assert_eq!(answer, None);
        assert!(said.contains("not a list of program names"), "got: {said}");
    }

    /// The command reaches the file, and the file is one the launcher reads.
    #[test]
    fn what_somebody_types_becomes_a_hook_omh_can_render() {
        let (answer, _) = asked_about_tests("mix test\n");
        let answer = answer.expect("answered");
        assert_eq!(answer.path, Path::new("hooks/test.json"));

        let parsed = crate::hook::Hook::parse(&answer.body, "test.json").expect("must parse");
        assert_eq!(parsed.on, crate::hook::Event::TurnEnd);
        assert_eq!(
            parsed.action,
            crate::hook::Action::Run("mix test".into()),
            "the command reaches the hook"
        );
    }

    /// A command with a quote in it is why this is **serialised**, never
    /// spliced into a JSON literal. It is text a person just typed, and the
    /// first thing that reads it back is omh.
    #[test]
    fn a_command_with_a_quote_survives_being_written() {
        let (answer, said) = asked_about_tests("sh -c \"mix test\"\n");
        let answer = answer.unwrap_or_else(|| panic!("a quoted command is a command: {said}"));
        assert_eq!(
            crate::hook::Hook::parse(&answer.body, "test.json")
                .expect("must parse back")
                .action,
            crate::hook::Action::Run("sh -c \"mix test\"".into())
        );
    }

    /// A `$` in a command is **not** refused, and writing that down is the
    /// point: `check_interpolation` guards injected *prose*, where a stray `$`
    /// is a hole in a sentence, and a `run` is shell where `$FOO` is a value
    /// the user may well set themselves.
    ///
    /// Kept as a test because the first version of this module asserted the
    /// opposite — a rule invented to make a guard look thorough, which would
    /// have refused a perfectly ordinary test command.
    #[test]
    fn a_variable_in_a_command_is_shell_not_a_hole_in_a_sentence() {
        let (answer, said) = asked_about_tests("mix test $MIX_ENV\n");
        let answer = answer.unwrap_or_else(|| panic!("got: {said}"));
        assert_eq!(
            crate::hook::Hook::parse(&answer.body, "test.json")
                .expect("must parse")
                .action,
            crate::hook::Action::Run("mix test $MIX_ENV".into())
        );
    }
}
