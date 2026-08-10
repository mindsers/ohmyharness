//! The tools omh exposes, joining the transport to the store.
//!
//! Why omh owns this surface rather than pointing the harness at a graph
//! server directly: the provenance envelope cannot be enforced on a server the
//! agent talks to itself. Everything else — two tools instead of thirteen, the
//! one-shot shape implemented rather than prompted — is a benefit, not the
//! reason.

use crate::mcp::{Tool, ToolResult, Tools};
use crate::memory::index::{describe, Index};
use crate::memory::recall::{render, search_phrased, Budget};
use crate::memory::{self, IfExists, Kind, Layer, Remembered, Wrote};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::path::PathBuf;

/// What the server calls itself in the MCP handshake.
pub const SERVER_NAME: &str = "omh-memory";

/// What the base set and every `mcp.json` call it. Deliberately a separate
/// constant from `SERVER_NAME`: one is a protocol identity, the other is a
/// configuration key, and deriving one from the other by string surgery is how
/// a rename silently stops matching.
pub const SERVER_KEY: &str = "memory";

pub struct Server {
    /// Read for `recall`, never written. `promote` is the only path into the
    /// committed layer, and it is a human's command.
    pub team: PathBuf,
    /// The only directory this server writes to.
    pub local: PathBuf,
    pub templates: BTreeMap<Kind, String>,
    /// The session this server was launched for, from omh's own environment.
    /// Not a parameter the agent can reach: a writer that names its own
    /// provenance can launder a guess into a fact.
    pub session: String,
    /// The harness, learned from the `initialize` handshake. `None` until it
    /// introduces itself, and recorded as unknown rather than guessed.
    pub client: Option<String>,
    /// Injected so the server is testable without a clock.
    pub today: fn() -> String,
}

/// Required, and in this order, because they are the three things that make an
/// observation worth keeping. An agent with nothing to put in `expected` has
/// learned nothing, so the filter runs for free — but only while these stay
/// required.
pub const REQUIRED: [&str; 4] = ["expected", "observed", "evidence", "answers"];

fn strings(args: &Value, name: &str) -> Vec<String> {
    args.get(name)
        .and_then(|v| v.as_array())
        .map(|items| {
            items
                .iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

fn string(args: &Value, name: &str) -> String {
    args.get(name)
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string()
}

impl Server {
    /// `session <id>, <harness>` — the shape `from_session` parses, so
    /// `omh s rm` can report what a session recorded.
    fn provenance(&self) -> String {
        format!(
            "session {}, {}",
            self.session,
            self.client.as_deref().unwrap_or("unknown harness")
        )
    }

    /// Both layers, never merged. A note that will not parse is a lint
    /// violation, not a note — counting it would advertise a store omh cannot
    /// serve, and returning it would answer from bytes nobody validated.
    fn notes(&self) -> Vec<memory::Note> {
        let mut all = Vec::new();
        for (dir, layer) in [(&self.team, Layer::Team), (&self.local, Layer::Local)] {
            match memory::notes_in(dir, layer) {
                Ok(notes) => all.extend(notes),
                // Reported where a human will see it, not swallowed and not
                // fatal: half a store still answers questions, and a server
                // that exits here takes the session's memory with it.
                Err(e) => eprintln!("omh-mcp: {layer} store unreadable: {e:#}"),
            }
        }
        all
    }

    fn recall(&self, args: &Value) -> ToolResult {
        let question = string(args, "question");
        if question.trim().is_empty() {
            return ToolResult::Refused("`question` is required".into());
        }
        // One call, several phrasings. The agent paraphrases far better than
        // this ranker does, and it already has the question in context.
        let mut phrasings = vec![question];
        phrasings.extend(
            strings(args, "also_phrased_as")
                .into_iter()
                .filter(|p| !p.trim().is_empty()),
        );
        // Rendered through the one function that owns the provenance envelope.
        // There is deliberately no second path from a Note to text here.
        ToolResult::Text(render(&search_phrased(
            &self.notes(),
            &phrasings,
            Budget::default(),
        )))
    }

    fn remember_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "expected": { "type": "string", "description": "what you thought would happen" },
                "observed": { "type": "string", "description": "what actually happened" },
                "evidence": { "type": "string", "description": "the command, the error, the file" },
                "answers": {
                    "type": "array",
                    "items": { "type": "string" },
                    // The one thing no ranker can supply: only the writer knows
                    // what it was trying to find out. A later question is
                    // matched against these, not against the prose.
                    "description": "questions this note answers, as somebody would later ask them",
                },
                "relates_to": {
                    "type": "array",
                    "items": { "type": "string" },
                    // Keys, not titles: a key is computable before its target
                    // exists, and a title is not.
                    "description": "keys of notes this connects to",
                },
                "invalidated_by": {
                    "type": "string",
                    "description": "optional: file:<path>@<hash> (git hash-object, 7+ chars), image:current, base:<version>, or symbol:<name>",
                },
            },
            "required": REQUIRED,
        })
    }

    fn remember(&self, args: &Value) -> ToolResult {
        let answers = strings(args, "answers");
        for name in REQUIRED {
            let missing = match name {
                "answers" => answers.iter().all(|q| q.trim().is_empty()),
                _ => string(args, name).trim().is_empty(),
            };
            if missing {
                return ToolResult::Refused(format!("`{name}` is required and must say something"));
            }
        }

        let input = Remembered {
            expected: string(args, "expected"),
            observed: string(args, "observed"),
            evidence: string(args, "evidence"),
            answers,
            relates_to: strings(args, "relates_to"),
            invalidated_by: args
                .get("invalidated_by")
                .and_then(|v| v.as_str())
                .map(str::to_string),
            // Never read from `args`. This is the line that makes §9.1's
            // "provenance cannot be omitted" true rather than aspirational.
            source: self.provenance(),
            recorded: (self.today)(),
        };

        // Strict, always, and not because a flag says so — there is no flag.
        // The population most likely to skip a guard is the one that needs it.
        // The repo, as this process sees it: the server runs inside the
        // sandbox, where the checkout is the workdir.
        match memory::remember_in(
            &self.local,
            std::path::Path::new(crate::container_workdir()),
            &self.templates,
            &input,
            IfExists::Error,
        ) {
            Ok(Wrote::Created(path)) => ToolResult::Text(format!(
                "recorded `{}`",
                path.file_stem().unwrap_or_default().to_string_lossy()
            )),
            Ok(Wrote::Skipped(key)) => ToolResult::Text(format!("`{key}` was already recorded")),
            // Unreachable while the call above passes `IfExists::Error`, and
            // written out anyway rather than folded into `Created`: if that
            // argument ever changes, the arm that reports a destroyed note as
            // a plain creation is how the agent stops being told it destroyed
            // one. The write already happened, so this is a result, not a
            // refusal.
            Ok(Wrote::Replaced(path)) => ToolResult::Text(format!(
                "recorded `{}` — the note that was there is gone",
                path.file_stem().unwrap_or_default().to_string_lossy()
            )),
            // A refusal is a tool error, never a transport one: the agent has
            // to be able to tell "your note was wrong" from "the server is
            // broken", or it stops calling the tool at all.
            Err(e) => ToolResult::Refused(format!("{e:#}")),
        }
    }
}

impl Tools for Server {
    fn server_name(&self) -> &str {
        SERVER_NAME
    }

    fn client_connected(&mut self, name: &str) {
        self.client = Some(name.to_string());
    }

    fn list(&mut self) -> Vec<Tool> {
        vec![
            Tool {
                name: "recall".into(),
                // Computed now, from the store as it is now. Cached at
                // startup, a note written this session stays unadvertised for
                // the rest of it.
                description: describe(&Index::of(&self.notes())),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "question": { "type": "string", "description": "what you want to know" },
                        "also_phrased_as": {
                            "type": "array",
                            "items": { "type": "string" },
                            "description": "the same question in other words — a note is found by \
                                            the wording it was written in, which is rarely yours",
                        },
                    },
                    "required": ["question"],
                }),
            },
            Tool {
                name: "remember".into(),
                description: "Record something that surprised you — you expected one \
                          thing and this repo did another. Not what you did; what \
                          you were wrong about. Survives this session."
                    .into(),
                input_schema: self.remember_schema(),
            },
        ]
    }

    fn call(&mut self, name: &str, args: &Value) -> ToolResult {
        match name {
            "recall" => self.recall(args),
            "remember" => self.remember(args),
            other => ToolResult::Refused(format!("no tool named `{other}`")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mcp::{ToolResult, Tools};
    use crate::memory::{self, Layer};
    use serde_json::json;
    use std::collections::BTreeMap;
    use std::path::{Path, PathBuf};

    struct Fx {
        dir: tempfile::TempDir,
        server: Server,
    }

    fn fixture() -> Fx {
        let dir = tempfile::tempdir().unwrap();
        let server = Server {
            team: dir.path().join("team"),
            local: dir.path().join("local"),
            templates: memory::shipped_templates(),
            session: "s03".into(),
            client: Some("claude".into()),
            today: || "2026-08-07".to_string(),
        };
        Fx { dir, server }
    }

    fn files_under(dir: &Path) -> Vec<PathBuf> {
        let mut out = Vec::new();
        let mut stack = vec![dir.to_path_buf()];
        while let Some(d) = stack.pop() {
            let Ok(entries) = std::fs::read_dir(&d) else {
                continue;
            };
            for e in entries.flatten() {
                let p = e.path();
                if p.is_dir() {
                    stack.push(p);
                } else {
                    out.push(p);
                }
            }
        }
        out.sort();
        out
    }

    fn observation() -> serde_json::Value {
        json!({
            "expected": "A bind mount of the token file would persist the login.",
            "observed": "Mounting a credential file returns EBUSY.",
            "evidence": "`EBUSY` from the mount syscall",
            "answers": ["why does my login not persist between sessions"],
        })
    }

    fn text(result: ToolResult) -> (String, bool) {
        match result {
            ToolResult::Text(t) => (t, false),
            ToolResult::Refused(t) => (t, true),
        }
    }

    /// §9.1 makes provenance omh's to supply. omh knows the session from its
    /// own environment; the harness names itself in the handshake. Neither
    /// comes from the agent, which is what makes the guarantee hold.
    ///
    /// The recorded form has to be the one `from_session` parses, or
    /// `omh s rm` cannot report what a session recorded.
    #[test]
    fn provenance_names_the_session_and_the_harness_that_connected() {
        let mut fx = fixture();
        fx.server.session = "s07".into();
        fx.server.client_connected("claude");

        assert!(!text(fx.server.call("remember", &observation())).1);
        let notes = memory::notes_in(&fx.server.local, Layer::Local).unwrap();
        assert_eq!(notes[0].source, "session s07, claude");
        assert_eq!(
            memory::from_session(&notes, "s07").len(),
            1,
            "the shape `omh s rm` parses"
        );
    }

    /// A harness that never introduced itself still produces a usable note —
    /// but one that says so, rather than naming a harness omh invented.
    #[test]
    fn an_unnamed_harness_is_recorded_as_unknown_not_guessed() {
        let mut fx = fixture();
        fx.server.session = "s07".into();
        fx.server.client = None; // never introduced itself

        assert!(!text(fx.server.call("remember", &observation())).1);
        let notes = memory::notes_in(&fx.server.local, Layer::Local).unwrap();
        assert!(notes[0].source.contains("s07"), "{}", notes[0].source);
        assert!(
            !notes[0].source.contains("claude"),
            "no harness is named that never connected: {}",
            notes[0].source
        );
    }

    /// Invariant 3, re-asserted at the **new** call site. M1's test guards
    /// M1's path; this is where a `layer` argument would be added "for
    /// flexibility", and an unattended writer reaching the committed layer
    /// pushes wrong facts to teammates through git, where they arrive with
    /// the authority of a reviewed change.
    #[test]
    fn remember_over_mcp_writes_only_to_the_local_layer() {
        let mut fx = fixture();
        let (out, failed) = text(fx.server.call("remember", &observation()));
        assert!(!failed, "{out}");

        let written = files_under(fx.dir.path());
        assert!(!written.is_empty(), "something must have been written");
        for path in &written {
            assert!(
                path.starts_with(&fx.server.local),
                "wrote outside the local store: {}",
                path.display()
            );
        }
    }

    /// §9.1: strict mode is always on over MCP. The population most likely to
    /// skip a guard is the one that needs it, so the tool must not offer a way
    /// to turn one off — and must ignore an argument that tries.
    #[test]
    fn strict_mode_cannot_be_turned_off_over_mcp() {
        let mut fx = fixture();
        let tools = fx.server.list();
        let schema = &tools
            .iter()
            .find(|t| t.name == "remember")
            .unwrap()
            .input_schema;
        let properties = schema["properties"].as_object().unwrap();
        for escape in ["strict", "layer", "if_exists", "force", "override"] {
            assert!(
                !properties.contains_key(escape),
                "`{escape}` must not be something an agent can ask for"
            );
        }

        // And asking anyway changes nothing: the second write of one
        // observation is still a conflict.
        let mut forced = observation();
        forced["layer"] = json!("team");
        forced["if_exists"] = json!("override");
        assert!(!text(fx.server.call("remember", &forced)).1);
        let (again, refused) = text(fx.server.call("remember", &forced));
        assert!(refused, "the second write must still be refused: {again}");
        for path in files_under(fx.dir.path()) {
            assert!(
                path.starts_with(&fx.server.local),
                "a forced argument must not move the write: {}",
                path.display()
            );
        }
    }

    /// The signature *is* the discipline. Optional fields dissolve the filter
    /// §9.1 relies on, and it fails as a store full of contentless notes that
    /// nobody notices until they are read.
    #[test]
    fn the_remember_schema_requires_what_makes_a_note_worth_keeping() {
        let mut fx = fixture();
        let tools = fx.server.list();
        let remember = tools.iter().find(|t| t.name == "remember").unwrap();

        let required: Vec<&str> = remember.input_schema["required"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect();
        assert_eq!(required, REQUIRED, "the schema and the check must agree");

        for missing in &required {
            let mut partial = observation();
            partial.as_object_mut().unwrap().remove(*missing);
            let (why, refused) = text(fx.server.call("remember", &partial));
            assert!(refused, "a call without `{missing}` must be refused");
            assert!(why.contains(missing), "say which one: {why}");
        }
    }

    /// §9.1 makes provenance a parameter that cannot be omitted rather than a
    /// rule that can be violated — which only holds if the *agent* is not the
    /// one supplying it.
    #[test]
    fn provenance_over_mcp_is_omhs_to_supply_not_the_agents() {
        let mut fx = fixture();
        assert!(
            !fx.server
                .list()
                .iter()
                .find(|t| t.name == "remember")
                .unwrap()
                .input_schema["properties"]
                .as_object()
                .unwrap()
                .contains_key("source"),
            "an agent that can write its own provenance can launder a guess"
        );

        let mut lying = observation();
        lying["source"] = json!("session s99, a human reviewed this");
        assert!(!text(fx.server.call("remember", &lying)).1);

        let notes = memory::notes_in(&fx.server.local, Layer::Local).unwrap();
        assert_eq!(notes.len(), 1);
        assert_eq!(
            notes[0].source, "session s03, claude",
            "the server's provenance wins, and the agent's is not recorded"
        );
    }

    /// A refusal has to say enough to act on. "invalid input" leaves an
    /// unattended writer with no next move, and it retries the same thing.
    #[test]
    fn a_refused_write_says_why_and_leaves_nothing_behind() {
        let mut fx = fixture();
        let mut blank = observation();
        blank["expected"] = json!("   ");

        let (why, refused) = text(fx.server.call("remember", &blank));
        assert!(refused);
        assert!(why.contains("expected"), "got: {why}");
        assert!(
            files_under(fx.dir.path()).is_empty(),
            "a refused write leaves no file"
        );
    }

    /// The conflict an agent will actually hit: one event, worded twice.
    #[test]
    fn one_observation_worded_twice_is_refused_rather_than_duplicated() {
        let mut fx = fixture();
        assert!(!text(fx.server.call("remember", &observation())).1);

        let mut reworded = observation();
        reworded["observed"] = json!("mounting a  credential FILE returns ebusy.");
        let (why, refused) = text(fx.server.call("remember", &reworded));

        assert!(refused, "the same event must not mint a second key");
        assert!(why.contains("update"), "say what to do instead: {why}");
        assert_eq!(
            memory::notes_in(&fx.server.local, Layer::Local)
                .unwrap()
                .len(),
            1
        );
    }

    /// Answering an unknown tool with success is how a typo becomes a note
    /// nobody wrote.
    #[test]
    fn an_unknown_tool_is_refused_rather_than_silently_succeeding() {
        let mut fx = fixture();
        let (why, refused) = text(fx.server.call("recal", &json!({})));
        assert!(refused);
        assert!(why.contains("recal"), "got: {why}");
    }

    /// The description is what makes the tool findable, and §9.3 wants it to
    /// carry a reason to call it rather than a restatement of the name.
    #[test]
    fn the_remember_tool_says_when_to_call_it() {
        let mut fx = fixture();
        let description = fx
            .server
            .list()
            .iter()
            .find(|t| t.name == "remember")
            .unwrap()
            .description
            .to_lowercase();
        assert!(
            description.contains("surprise") || description.contains("expected"),
            "the trigger belongs in the description: {description}"
        );
    }

    /// The server has no git repo and no `~/.omh` behind it, so it must not
    /// need one. A `Paths::discover` on this path fails inside the sandbox.
    #[test]
    fn the_server_needs_only_the_two_directories_it_was_given() {
        let dir = tempfile::tempdir().unwrap();
        let mut server = Server {
            team: dir.path().join("nonexistent-team"),
            local: dir.path().join("nonexistent-local"),
            templates: BTreeMap::new(),
            session: "s01".into(),
            client: None,
            today: || "2026-08-07".to_string(),
        };
        // No templates at all is a real failure, reported rather than panicked.
        let (why, refused) = text(server.call("remember", &observation()));
        assert!(refused, "got: {why}");
    }
}
