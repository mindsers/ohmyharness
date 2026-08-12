//! The canonical hook format.
//!
//! `{ "event": "Stop", "matcher": "Edit|Write", "command": … }` is Claude
//! Code's vocabulary wearing a neutral-looking hat. Three separate things leak,
//! and they get harder in order: **event names** are Claude's words for moments
//! every harness has; **matchers** are Claude's *tool* names; and the **payload
//! protocol** — `jq -r '.tool_input.command'` in, `hookSpecificOutput` out — is
//! the one that matters, because translating every name in a hook file leaves a
//! body that still only works on Claude Code.
//!
//! All three are the same problem, so they get one answer: **omh has a closed
//! vocabulary, the adapter says how this harness spells each word, and the
//! translation happens when the hook is staged.** No part of it needs to happen
//! at runtime.
//!
//! Rejected: a runtime shim, with the harness calling `omh hook run <name>` and
//! omh normalizing the payload live. `omh` is already at `/usr/local/bin/omh`
//! in every sandbox so it would have worked, but it puts a process spawn in
//! front of every matching tool call — and `graph-read` matches `Read`, the most
//! frequent tool there is. Paying that per call, forever, to do work that can be
//! done once per launch is the wrong trade, and it would have moved a harness
//! difference out of adapter data and into omh's code.

use crate::adapter::Binding;
use anyhow::{Context, Result};
use serde::Deserialize;
use std::collections::BTreeSet;

/// A moment every harness has, in omh's words.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Event {
    SessionStart,
    TurnEnd,
    BeforeTool,
    AfterTool,
}

/// A class of thing an agent does, in omh's words. A harness spells each one
/// however its own tools are named.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Tool {
    Edit,
    Read,
    Shell,
    Search,
}

/// A piece of the tool call a hook can read.
///
/// `.tool_input.file_path` is Claude's spelling of a canonical field exactly as
/// `PreToolUse` is Claude's spelling of a canonical moment, which is what makes
/// the payload one mechanism with the other two rather than a second one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Field {
    ToolFile,
    ToolCommand,
}

impl Field {
    pub const ALL: [Field; 2] = [Self::ToolFile, Self::ToolCommand];

    /// The shell variable a hook body reads this field through. Binding the
    /// fields to `OMH_*` names is what lets a `when` predicate be written once
    /// and mean the same thing on every harness.
    pub fn var(&self) -> &'static str {
        match self {
            Self::ToolFile => "OMH_TOOL_FILE",
            Self::ToolCommand => "OMH_TOOL_COMMAND",
        }
    }
}

/// What `capture` binds its command's output to.
pub const CAPTURE_VAR: &str = "OMH_CAPTURE";

impl std::fmt::Display for Event {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::SessionStart => "session-start",
            Self::TurnEnd => "turn-end",
            Self::BeforeTool => "before-tool",
            Self::AfterTool => "after-tool",
        })
    }
}

impl std::fmt::Display for Tool {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Edit => "edit",
            Self::Read => "read",
            Self::Shell => "shell",
            Self::Search => "search",
        })
    }
}

impl std::fmt::Display for Field {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::ToolFile => "tool-file",
            Self::ToolCommand => "tool-command",
        })
    }
}

/// A hook declares what it wants, never how a harness spells it.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Hook {
    pub on: Event,
    /// Empty means every tool this moment has — which is the only sensible
    /// reading for `turn-end`, where there is no tool to narrow to.
    #[serde(default)]
    pub tools: Vec<Tool>,
    /// A predicate. Non-zero means this hook stays silent, which is how a hook
    /// degrades to a no-op rather than to an error.
    #[serde(default)]
    pub when: Option<String>,
    /// A command whose stdout binds to `$OMH_CAPTURE`, for the case `inject`
    /// alone cannot express: text that is not known until something has been
    /// asked. Evaluated **before** `when`, so a predicate can test it.
    #[serde(default)]
    pub capture: Option<String>,
    /// Executes; output ignored.
    #[serde(default)]
    pub run: Option<String>,
    /// Text into the agent's context, through whatever protocol this harness
    /// uses to accept one.
    #[serde(default)]
    pub inject: Option<String>,
}

impl Hook {
    /// Parse and validate together, so no caller can hold an unchecked hook.
    /// Validating where the value is minted is the rule `memory::expand_key`
    /// and `carry::validate_pattern` already follow.
    pub fn parse(raw: &str, whence: &str) -> Result<Self> {
        let hook: Self =
            serde_json::from_str(raw).with_context(|| format!("parsing hook {whence}"))?;
        hook.validate(whence)?;
        Ok(hook)
    }

    fn validate(&self, whence: &str) -> Result<()> {
        match (&self.run, &self.inject) {
            (Some(_), Some(_)) => anyhow::bail!(
                "{whence}: a hook either `run`s something or `inject`s text, not both. \
                 A command whose output should reach the agent is `capture` plus `inject`."
            ),
            (None, None) => {
                anyhow::bail!("{whence}: a hook does nothing without `run` or `inject`")
            }
            _ => {}
        }
        if self.capture.is_some() && self.inject.is_none() {
            anyhow::bail!(
                "{whence}: `capture` collects output for `inject` to carry. \
                 With `run` the output is ignored, so capturing it says nothing."
            );
        }
        if let Some(text) = &self.inject {
            check_interpolation(text, whence)?;
        }
        Ok(())
    }

    /// What this hook does, in one string, for the commands that report a hook
    /// rather than run one.
    ///
    /// `validate` guarantees exactly one of the two, so the fallback is
    /// unreachable rather than a default that could be mistaken for an answer.
    pub fn does(&self) -> &str {
        self.run
            .as_deref()
            .or(self.inject.as_deref())
            .unwrap_or_default()
    }

    /// The payload fields this hook actually reads, derived from the `$OMH_*`
    /// names its bodies mention.
    ///
    /// Derived rather than declared so there is nothing to keep in sync: a
    /// `when` that stops testing the filename stops paying for it in the same
    /// edit. `graph-first` reads no payload at all and must not be charged a
    /// `jq` on every search.
    pub fn fields(&self) -> BTreeSet<Field> {
        let bodies = [&self.when, &self.capture, &self.run, &self.inject];
        Field::ALL
            .into_iter()
            .filter(|f| {
                bodies
                    .iter()
                    .flat_map(|b| b.iter())
                    .any(|b| mentions(b, f.var()))
            })
            .collect()
    }
}

/// Does `body` reference `$var`? Checked with the following character so
/// `$OMH_TOOL_FILE_BACKUP` does not count as `$OMH_TOOL_FILE` — and `${VAR}`
/// counts, because that is the same reference written defensively.
fn mentions(body: &str, var: &str) -> bool {
    let boundary = |rest: &str| {
        !rest
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_alphanumeric() || c == '_')
    };
    body.match_indices(var).any(|(i, _)| {
        let before = &body[..i];
        let rest = &body[i + var.len()..];
        (before.ends_with('$') && boundary(rest))
            || (before.ends_with("${") && rest.starts_with('}'))
    })
}

/// `inject` is prose that reaches a shell, which is a combination that fails
/// quietly: a stray `$` expands to nothing and the sentence arrives with a hole
/// in it. So every `$` has to look like a reference, `$$` is how you write a
/// literal one, and command substitution is refused by name — that is what
/// `capture` is for, and running a command from inside a sentence is how a hook
/// body learns to be shell again.
fn check_interpolation(text: &str, whence: &str) -> Result<()> {
    let chars: Vec<char> = text.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] != '$' {
            i += 1;
            continue;
        }
        match chars.get(i + 1) {
            Some('$') => i += 2,
            Some('(') => anyhow::bail!(
                "{whence}: `$(` in `inject` runs a command from inside a sentence. \
                 Use `capture` and interpolate ${CAPTURE_VAR}."
            ),
            Some(c) if c.is_ascii_alphabetic() || *c == '_' || *c == '{' => i += 2,
            _ => anyhow::bail!(
                "{whence}: `$` in `inject` must name a variable — it is interpolated \
                 by a shell, so a bare one expands to nothing and the text arrives \
                 with a hole in it. Write `$$` for a literal dollar."
            ),
        }
    }
    Ok(())
}

/// A hook translated into one harness's words, or the reason it could not be.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    Rendered(Rendered),
    /// Not an error: a harness that cannot express a moment is the same
    /// "absent key is graceful degradation" rule the capability map already
    /// uses, one level down. But the whole capability no longer goes with it,
    /// so the report has to name the hook and what it asked for.
    Dropped(Dropped),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rendered {
    pub event: String,
    pub matcher: String,
    pub command: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Dropped {
    pub name: String,
    /// What this harness could not spell, in omh's words.
    pub wanted: String,
}

impl std::fmt::Display for Dropped {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} (no {})", self.name, self.wanted)
    }
}

/// Translate one hook for one harness. All of it is string generation, done
/// once, at staging.
pub fn render(name: &str, hook: &Hook, binding: &Binding) -> Result<Outcome> {
    let dropped = |wanted: String| {
        Ok(Outcome::Dropped(Dropped {
            name: name.to_string(),
            wanted,
        }))
    };

    let Some(event) = binding.events.get(&hook.on) else {
        return dropped(format!("`{}` moment", hook.on));
    };

    let mut matchers = Vec::new();
    for tool in &hook.tools {
        match binding.tools.get(tool) {
            Some(spelling) => matchers.push(spelling.clone()),
            None => return dropped(format!("`{tool}` tool")),
        }
    }

    let mut command = String::new();

    // One `cat`, however many fields. stdin is consumable once, so a second
    // bare `jq` would read an empty payload and the field would silently be
    // blank — the shape of bug that satisfies every assertion about a hook's
    // text while the hook does nothing.
    let fields = hook.fields();
    if !fields.is_empty() {
        command.push_str("p=$(cat); ");
        for field in &fields {
            let Some(expr) = binding.fields.get(field) else {
                return dropped(format!("`{field}` field"));
            };
            command.push_str(&format!(
                "{}=$(printf '%s' \"$p\" | jq -r '{expr} // empty'); ",
                field.var()
            ));
        }
    }

    if let Some(capture) = &hook.capture {
        command.push_str(&format!("{CAPTURE_VAR}=$({capture}); "));
    }
    if let Some(when) = &hook.when {
        command.push_str(&format!("{when} || exit 0; "));
    }

    if let Some(run) = &hook.run {
        command.push_str(run);
    } else if let Some(text) = &hook.inject {
        let Some(inject) = &binding.inject else {
            return dropped("way to inject text".into());
        };
        command.push_str(
            &inject
                .template
                .replace("{{text}}", &interpolating(text))
                .replace("{{event}}", event),
        );
    }

    Ok(Outcome::Rendered(Rendered {
        event: event.clone(),
        matcher: matchers.join("|"),
        command,
    }))
}

/// A double-quoted shell word: `$` stays live so a hook can name what it is
/// talking about, everything that could end the word or start a command does
/// not. `$$` is the literal dollar, resolved here rather than left for the
/// shell, which reads `$$` as its own process id.
fn interpolating(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '$' if chars.peek() == Some(&'$') => {
                chars.next();
                out.push_str("\\$");
            }
            '\\' | '"' | '`' => {
                out.push('\\');
                out.push(c);
            }
            _ => out.push(c),
        }
    }
    out.push('"');
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapter::{Adapter, Capability};
    use std::path::Path;

    const ADAPTERS: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/adapters");

    fn claude() -> Adapter {
        Adapter::find(Path::new(ADAPTERS), "claude").unwrap()
    }

    /// A binding built from TOML rather than a struct literal: an adapter always
    /// arrives as a file, and a hand-built one can be right in exactly the way
    /// the code is wrong.
    fn binding(toml: &str) -> Binding {
        toml::from_str(toml).unwrap()
    }

    fn rendered(name: &str, hook: &Hook, b: &Binding) -> Rendered {
        match render(name, hook, b).unwrap() {
            Outcome::Rendered(r) => r,
            Outcome::Dropped(d) => panic!("unexpectedly dropped: {d}"),
        }
    }

    fn dropped(name: &str, hook: &Hook, b: &Binding) -> Dropped {
        match render(name, hook, b).unwrap() {
            Outcome::Rendered(r) => panic!("unexpectedly rendered: {r:?}"),
            Outcome::Dropped(d) => d,
        }
    }

    // ── the format ──────────────────────────────────────────────────────────

    /// The whole reason the format exists. `event` and `matcher` are one
    /// harness's words, and a file omh presents as its own cannot be written in
    /// them — otherwise every hook anybody writes is a hook that works on
    /// exactly one harness, and nothing says so.
    #[test]
    fn a_hook_declares_a_moment_not_a_harness_event() {
        let err = Hook::parse(r#"{"event":"Stop","command":"cargo test"}"#, "h.json")
            .expect_err("Claude's vocabulary is not omh's");
        let err = format!("{err:#}");
        assert!(err.contains("h.json"), "must name the file: {err}");
        assert!(err.contains("`on`"), "and the field to use: {err}");
    }

    #[test]
    fn a_hook_that_neither_runs_nor_injects_is_refused() {
        let err = Hook::parse(r#"{"on":"turn-end"}"#, "h.json").unwrap_err();
        assert!(err.to_string().contains("run"), "got: {err}");
    }

    /// `run` ignores output and `inject` is text, so a hook asking for both is
    /// asking for something with no meaning — most likely it wanted a command
    /// whose output reaches the agent, which is `capture` plus `inject`.
    #[test]
    fn a_hook_that_both_runs_and_injects_is_refused_with_what_it_meant() {
        let err = Hook::parse(r#"{"on":"turn-end","run":"x","inject":"y"}"#, "h.json").unwrap_err();
        assert!(err.to_string().contains("capture"), "got: {err}");
    }

    #[test]
    fn capture_without_inject_is_refused() {
        let err =
            Hook::parse(r#"{"on":"turn-end","capture":"date","run":"x"}"#, "h.json").unwrap_err();
        assert!(err.to_string().contains("capture"), "got: {err}");
    }

    /// `inject` is prose that reaches a shell, and that combination fails
    /// quietly: the sentence arrives with a hole where the `$` was, and every
    /// assertion about the hook's text still passes.
    #[test]
    fn a_dollar_that_names_no_variable_is_refused() {
        let err =
            Hook::parse(r#"{"on":"turn-end","inject":"costs $5 a month"}"#, "h.json").unwrap_err();
        assert!(
            err.to_string().contains("$$"),
            "must say how to write one: {err}"
        );
    }

    #[test]
    fn a_doubled_dollar_is_a_literal_one() {
        let h = Hook::parse(r#"{"on":"turn-end","inject":"costs $$5"}"#, "h.json").unwrap();
        assert_eq!(
            interpolating(h.inject.as_deref().unwrap()),
            r#""costs \$5""#
        );
    }

    /// Running a command from inside a sentence is how a hook body learns to be
    /// shell again, which is the thing the format exists to undo.
    #[test]
    fn command_substitution_in_prose_points_at_capture() {
        let err = Hook::parse(r#"{"on":"turn-end","inject":"now $(date)"}"#, "h.json").unwrap_err();
        assert!(err.to_string().contains("capture"), "got: {err}");
    }

    /// `graph-first` fires on every search and reads no payload at all. A
    /// preamble it does not need is a `jq` process per search, forever.
    #[test]
    fn only_the_fields_a_hook_mentions_are_bound() {
        let quiet = Hook::parse(r#"{"on":"before-tool","inject":"a nudge"}"#, "h.json").unwrap();
        assert!(quiet.fields().is_empty());
        assert!(!rendered("q", &quiet, hooks_binding())
            .command
            .contains("jq -r"));

        let reads = Hook::parse(
            r#"{"on":"before-tool","when":"[ -f \"$OMH_TOOL_FILE\" ]","run":"x"}"#,
            "h.json",
        )
        .unwrap();
        assert_eq!(reads.fields(), BTreeSet::from([Field::ToolFile]));
    }

    /// A name that merely starts with a binding's is not that binding.
    #[test]
    fn a_longer_name_is_not_a_field_reference() {
        let h = Hook::parse(
            r#"{"on":"before-tool","run":"echo $OMH_TOOL_FILENAME"}"#,
            "h.json",
        )
        .unwrap();
        assert!(
            h.fields().is_empty(),
            "OMH_TOOL_FILENAME is its own variable"
        );
    }

    /// stdin is consumable once. A second bare `jq` would read an empty payload
    /// and the field would silently be blank — a hook that satisfies every
    /// assertion about its text while doing nothing.
    #[test]
    fn the_payload_is_read_once_however_many_fields() {
        let h = Hook::parse(
            r#"{"on":"before-tool","when":"[ -n \"$OMH_TOOL_COMMAND\" ]",
                "inject":"about $OMH_TOOL_FILE"}"#,
            "h.json",
        )
        .unwrap();
        let cmd = rendered("two", &h, hooks_binding()).command;
        assert_eq!(cmd.matches("$(cat)").count(), 1, "got: {cmd}");
        assert_eq!(cmd.matches("jq -r").count(), 2, "one per field: {cmd}");
    }

    // ── the translation ─────────────────────────────────────────────────────

    fn hooks_binding() -> &'static Binding {
        // Leaked deliberately: the shipped adapter is read once and every test
        // below wants the same maps a launch uses.
        static CELL: std::sync::OnceLock<Binding> = std::sync::OnceLock::new();
        CELL.get_or_init(|| {
            let a = claude();
            let b = a.supports(Capability::Hooks).expect("claude has hooks");
            binding(&format!(
                "path = {p:?}\nrender = \"claude-settings\"\n\
                 [events]\n{events}\n[tools]\n{tools}\n[fields]\n{fields}\n\
                 [inject]\ntemplate = {t:?}\n",
                p = b.path,
                events = b
                    .events
                    .iter()
                    .map(|(k, v)| format!("{k} = {v:?}"))
                    .collect::<Vec<_>>()
                    .join("\n"),
                tools = b
                    .tools
                    .iter()
                    .map(|(k, v)| format!("{k} = {v:?}"))
                    .collect::<Vec<_>>()
                    .join("\n"),
                fields = b
                    .fields
                    .iter()
                    .map(|(k, v)| format!("{k} = {v:?}"))
                    .collect::<Vec<_>>()
                    .join("\n"),
                t = b.inject.as_ref().unwrap().template,
            ))
        })
    }

    /// An absent map entry means this harness has no such moment — the same
    /// "absent key is graceful degradation" rule the capability map uses, one
    /// level down. What changes is the granularity: the capability survives, so
    /// the report has to name the hook and what it wanted, or a hook that is
    /// simply not there looks exactly like a hook that is working.
    #[test]
    fn an_event_this_harness_cannot_express_drops_the_hook_by_name() {
        let b = binding(
            "path = \"/x\"\nrender = \"claude-settings\"\n\
             [events]\nturn-end = \"Stop\"\n[inject]\ntemplate = \"echo {{text}}\"\n",
        );
        let h = Hook::parse(r#"{"on":"after-tool","run":"cargo fmt"}"#, "h.json").unwrap();
        let d = dropped("rust-format", &h, &b);
        assert_eq!(d.name, "rust-format");
        assert!(d.wanted.contains("after-tool"), "got: {}", d.wanted);

        // And the sibling still ships, which is the half a capability-level
        // count could never say.
        let sibling = Hook::parse(r#"{"on":"turn-end","run":"cargo test"}"#, "h.json").unwrap();
        assert_eq!(rendered("rust-test", &sibling, &b).command, "cargo test");
    }

    #[test]
    fn an_unmapped_tool_drops_the_hook_saying_which_tool() {
        let b = binding(
            "path = \"/x\"\nrender = \"claude-settings\"\n\
             [events]\nbefore-tool = \"PreToolUse\"\n[tools]\nshell = \"Bash\"\n\
             [inject]\ntemplate = \"echo {{text}}\"\n",
        );
        let h = Hook::parse(
            r#"{"on":"before-tool","tools":["read"],"run":"x"}"#,
            "h.json",
        )
        .unwrap();
        assert!(dropped("peek", &h, &b).wanted.contains("read"));
    }

    /// The payload is names too, so it degrades the same way. A harness whose
    /// stdin schema has no filename cannot answer `graph-read`'s question, and
    /// rendering it anyway would produce a hook comparing an empty string.
    #[test]
    fn an_unmapped_field_drops_the_hook_saying_which_field() {
        let b = binding(
            "path = \"/x\"\nrender = \"claude-settings\"\n\
             [events]\nbefore-tool = \"PreToolUse\"\n\
             [fields]\ntool-command = \".cmd\"\n[inject]\ntemplate = \"echo {{text}}\"\n",
        );
        let h = Hook::parse(
            r#"{"on":"before-tool","when":"[ -f \"$OMH_TOOL_FILE\" ]","run":"x"}"#,
            "h.json",
        )
        .unwrap();
        assert!(dropped("graph-read", &h, &b).wanted.contains("tool-file"));
    }

    /// A harness with the moment but no way to accept text drops the injectors
    /// and keeps the runners.
    #[test]
    fn a_harness_that_cannot_take_text_still_runs_commands() {
        let b =
            binding("path = \"/x\"\nrender = \"claude-settings\"\n[events]\nturn-end = \"Stop\"\n");
        let say = Hook::parse(r#"{"on":"turn-end","inject":"hello"}"#, "h.json").unwrap();
        assert!(dropped("say", &say, &b).wanted.contains("inject"));

        let run = Hook::parse(r#"{"on":"turn-end","run":"cargo test"}"#, "h.json").unwrap();
        assert_eq!(rendered("run", &run, &b).command, "cargo test");
    }

    /// Prose reaches the agent through a shell, and the characters that end a
    /// shell word are exactly the ones prose is full of. Asserting on the
    /// command string proves the sentence is embedded, never that a shell will
    /// emit it — so this runs the thing.
    #[test]
    fn injected_prose_survives_the_shell() {
        let prose = "a \"quoted\" word, a \\ backslash, a `backtick`, and $$5 — it's fine";
        let h = Hook::parse(
            &serde_json::json!({ "on": "turn-end", "inject": prose }).to_string(),
            "h.json",
        )
        .unwrap();

        let out = std::process::Command::new("sh")
            .arg("-c")
            .arg(&rendered("prose", &h, hooks_binding()).command)
            .stdin(std::process::Stdio::null())
            .output()
            .expect("sh must run");
        assert!(
            out.stderr.is_empty(),
            "the harness shows the user stderr: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        let doc: serde_json::Value = serde_json::from_slice(&out.stdout)
            .unwrap_or_else(|e| panic!("not JSON: {} ({e})", String::from_utf8_lossy(&out.stdout)));
        assert_eq!(
            doc["hookSpecificOutput"]["additionalContext"]
                .as_str()
                .unwrap(),
            prose.replace("$$", "$"),
            "the prose has to survive shell quoting intact"
        );
    }

    /// `capture` runs before `when`, which is the only order that lets a
    /// predicate test what was captured — `graph-orient` stays silent when the
    /// graph answered nothing, and that is the whole of its degradation.
    #[test]
    fn capture_is_evaluated_before_the_predicate_that_tests_it() {
        let h = Hook::parse(
            r#"{"on":"session-start","capture":"echo hi",
                "when":"[ -n \"$OMH_CAPTURE\" ]","inject":"got $OMH_CAPTURE"}"#,
            "h.json",
        )
        .unwrap();
        let cmd = rendered("orient", &h, hooks_binding()).command;
        assert!(
            cmd.find("OMH_CAPTURE=$(").unwrap() < cmd.find("|| exit 0").unwrap(),
            "got: {cmd}"
        );
    }
}
