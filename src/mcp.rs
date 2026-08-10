//! An MCP server, hand-rolled.
//!
//! MCP over stdio is newline-delimited JSON-RPC 2.0: one message per line, no
//! `Content-Length` headers — that is LSP. So the whole transport is
//! `BufRead::lines()` plus `serde_json`, with no async runtime and no SDK,
//! which is the same trade this crate already makes for frontmatter, key
//! templates and every config format it renders.
//!
//! This module knows nothing about notes. It takes a [`Tools`] implementation
//! and moves JSON. That separation is load-bearing rather than tidy: it means
//! there is exactly one place a note can be turned into text an agent sees, so
//! the provenance envelope cannot be bypassed by a second formatting path.

use anyhow::Result;
use serde::Deserialize;
use serde_json::{json, Value};
use std::io::{BufRead, Write};

/// The version omh speaks. Sent when a client asks for something we do not
/// recognise, rather than echoing theirs — echoing claims compliance with a
/// specification we have never seen.
pub const PROTOCOL: &str = "2025-06-18";

/// Versions omh will speak if asked by name.
pub const SUPPORTED: [&str; 2] = ["2025-06-18", "2025-03-26"];

/// A JSON-RPC request or notification.
///
/// Deliberately **not** `deny_unknown_fields`, which is the opposite of the
/// rule this repo applies to adapters — and for the opposite reason. An
/// adapter is our data, so a misspelled key there must fail loudly. A request
/// is the client's: it carries `_meta`, `progressToken` and whatever the next
/// harness release adds, and refusing those is how a server stops working on
/// somebody else's upgrade.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Request {
    /// Absent on a notification. A string *or* a number — parsing it as `u64`
    /// answers a string id with `null`, and the client then waits forever.
    #[serde(default)]
    pub id: Option<Value>,
    pub method: String,
    #[serde(default)]
    pub params: Value,
}

#[derive(Debug, PartialEq)]
pub enum Framed {
    Message(Request),
    /// Kept as a value rather than an error: one junk line must not end the
    /// session. A server that exits mid-task is reported to the user as
    /// "MCP server crashed", with no cause attached.
    Malformed(String),
}

pub fn parse_line(line: &str) -> Framed {
    match serde_json::from_str::<Request>(line) {
        Ok(req) => Framed::Message(req),
        Err(e) => Framed::Malformed(e.to_string()),
    }
}

/// One tool, as the harness sees it.
pub struct Tool {
    pub name: String,
    /// Generated per call rather than fixed, which is what lets it carry facts
    /// about the store as it is right now.
    pub description: String,
    pub input_schema: Value,
}

/// The outcome of a tool call.
///
/// A refusal is **not** a JSON-RPC error. A protocol error means "your call
/// was malformed"; conflating the two makes a refused write look like a broken
/// server, and an agent that believes the server is broken stops calling it.
pub enum ToolResult {
    Text(String),
    Refused(String),
}

pub trait Tools {
    fn server_name(&self) -> &str;
    /// The harness introduced itself. `initialize` is the only place omh can
    /// learn which agent is on the other end, and provenance has to record it:
    /// one store is shared across harnesses by design, so "which one wrote
    /// this" is not answerable any other way.
    fn client_connected(&mut self, _name: &str) {}
    /// `&mut` on purpose: the description is computed from the store at the
    /// moment it is asked for, so a note written this session is advertised
    /// this session.
    fn list(&mut self) -> Vec<Tool>;
    fn call(&mut self, name: &str, args: &Value) -> ToolResult;
}

/// Answer with the client's version when we speak it, ours when we do not.
pub fn negotiate(client: Option<&str>) -> &'static str {
    client
        .and_then(|want| SUPPORTED.into_iter().find(|v| *v == want))
        .unwrap_or(PROTOCOL)
}

fn ok(id: &Value, result: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "result": result })
}

fn err(id: &Value, code: i64, message: &str) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}

/// The whole server, as a function. `None` means the message was a
/// notification and carries no reply.
pub fn dispatch(req: &Request, tools: &mut dyn Tools) -> Option<Value> {
    // A notification has no id, and answering one is a protocol violation that
    // strict clients respond to by dropping the connection.
    let id = req.id.clone()?;

    Some(match req.method.as_str() {
        "initialize" => {
            if let Some(client) = req
                .params
                .get("clientInfo")
                .and_then(|c| c.get("name"))
                .and_then(|n| n.as_str())
            {
                tools.client_connected(client);
            }
            let want = req.params.get("protocolVersion").and_then(|v| v.as_str());
            ok(
                &id,
                json!({
                    "protocolVersion": negotiate(want),
                    "capabilities": { "tools": {} },
                    "serverInfo": { "name": tools.server_name(), "version": env!("CARGO_PKG_VERSION") },
                }),
            )
        }
        "ping" => ok(&id, json!({})),
        "tools/list" => {
            let listed: Vec<Value> = tools
                .list()
                .into_iter()
                .map(|t| {
                    json!({
                        "name": t.name,
                        "description": t.description,
                        "inputSchema": t.input_schema,
                    })
                })
                .collect();
            ok(&id, json!({ "tools": listed }))
        }
        "tools/call" => {
            let Some(name) = req.params.get("name").and_then(|v| v.as_str()) else {
                return Some(err(&id, -32602, "tools/call needs a `name`"));
            };
            let args = req.params.get("arguments").cloned().unwrap_or(json!({}));
            let (text, failed) = match tools.call(name, &args) {
                ToolResult::Text(text) => (text, false),
                ToolResult::Refused(why) => (why, true),
            };
            ok(
                &id,
                json!({
                    "content": [{ "type": "text", "text": text }],
                    "isError": failed,
                }),
            )
        }
        // Answering an unrecognised method with `{}` tells capability
        // negotiation we support things we have never implemented.
        other => err(&id, -32601, &format!("unknown method `{other}`")),
    })
}

/// The only code here that touches I/O.
pub fn serve<R: BufRead, W: Write>(input: R, mut output: W, tools: &mut dyn Tools) -> Result<()> {
    for line in input.lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let reply = match parse_line(&line) {
            Framed::Message(req) => dispatch(&req, tools),
            // No id to answer against, so there is nobody to tell. Diagnostics
            // go to stderr; stdout carries protocol and nothing else.
            Framed::Malformed(why) => {
                eprintln!("omh-mcp: ignoring unparseable line: {why}");
                None
            }
        };
        if let Some(reply) = reply {
            // `to_string`, never `to_string_pretty` — this crate's habit
            // elsewhere. A pretty response spans several lines, and every line
            // is a frame, so it corrupts the whole rest of the session.
            writeln!(output, "{}", serde_json::to_string(&reply)?)?;
            output.flush()?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A stand-in with no store behind it, so transport failures cannot be
    /// mistaken for store failures.
    struct Fake {
        listed: usize,
        called: Vec<(String, Value)>,
        refuse: bool,
        client: Option<String>,
    }

    impl Fake {
        fn new() -> Self {
            Self {
                listed: 0,
                called: Vec::new(),
                refuse: false,
                client: None,
            }
        }
    }

    impl Tools for Fake {
        fn server_name(&self) -> &str {
            "omh-memory"
        }
        fn client_connected(&mut self, name: &str) {
            self.client = Some(name.to_string());
        }
        fn list(&mut self) -> Vec<Tool> {
            self.listed += 1;
            vec![Tool {
                name: "recall".into(),
                description: format!("asked {} time(s)", self.listed),
                input_schema: json!({
                    "type": "object",
                    "properties": { "question": { "type": "string" } },
                    "required": ["question"],
                }),
            }]
        }
        fn call(&mut self, name: &str, args: &Value) -> ToolResult {
            self.called.push((name.to_string(), args.clone()));
            if self.refuse {
                ToolResult::Refused("no".into())
            } else {
                ToolResult::Text("yes".into())
            }
        }
    }

    fn request(raw: &str) -> Request {
        match parse_line(raw) {
            Framed::Message(req) => req,
            Framed::Malformed(why) => panic!("{why}"),
        }
    }

    fn answer(raw: &str) -> Value {
        dispatch(&request(raw), &mut Fake::new()).expect("expected a reply")
    }

    /// The harness names itself in the handshake, and that is the only place
    /// omh can learn which one is writing. Provenance has to say *which agent*
    /// recorded a note, and a server told nothing can only guess.
    #[test]
    fn the_harness_names_itself_and_the_server_is_told() {
        let mut tools = Fake::new();
        let req = request(
            r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","clientInfo":{"name":"claude","version":"9"}}}"#,
        );
        dispatch(&req, &mut tools);
        assert_eq!(tools.client.as_deref(), Some("claude"));
    }

    /// A client that names nothing must not be recorded as one that did.
    #[test]
    fn a_handshake_with_no_client_name_leaves_provenance_alone() {
        let mut tools = Fake::new();
        let req = request(r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}"#);
        dispatch(&req, &mut tools);
        assert_eq!(tools.client, None);
    }

    /// An id is a string *or* a number, and it must come back exactly as it
    /// arrived. Parsing it as `u64` answers a string id with `null`, and the
    /// client waits for a reply that will never match.
    #[test]
    fn a_request_is_answered_with_the_id_it_arrived_with() {
        for id in ["1", "\"abc\"", "\"0\"", "-4"] {
            let raw = format!(r#"{{"jsonrpc":"2.0","id":{id},"method":"ping"}}"#);
            let got = answer(&raw);
            assert_eq!(
                got["id"],
                serde_json::from_str::<Value>(id).unwrap(),
                "id {id} came back as {}",
                got["id"]
            );
        }
    }

    /// A notification carries no id and gets no reply. An unsolicited response
    /// is a protocol violation, and strict clients drop the connection.
    #[test]
    fn a_notification_is_never_answered() {
        let req = request(r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#);
        assert_eq!(dispatch(&req, &mut Fake::new()), None);
    }

    /// Every frame is one line. `to_string_pretty` — which this crate uses
    /// almost everywhere else — turns one response into several frames and
    /// corrupts every message after it.
    #[test]
    fn no_response_ever_spans_more_than_one_line() {
        let mut out = Vec::new();
        let input = concat!(
            r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}"#,
            "\n",
            r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
            "\n",
            r#"{"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}"#,
            "\n",
        );
        serve(input.as_bytes(), &mut out, &mut Fake::new()).unwrap();

        let text = String::from_utf8(out).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 2, "two requests, two frames: {text:?}");
        for line in lines {
            serde_json::from_str::<Value>(line)
                .unwrap_or_else(|e| panic!("a frame must be one whole JSON value: {e}\n{line}"));
        }
    }

    /// One junk line must not end the session. `?` on the parse inside the
    /// loop kills the server mid-task, and the harness reports it as a crash
    /// with no cause.
    #[test]
    fn a_malformed_line_does_not_end_the_session() {
        let mut out = Vec::new();
        let input = concat!(
            "not json at all\n",
            r#"{"jsonrpc":"2.0","id":7,"method":"ping"}"#,
            "\n",
        );
        serve(input.as_bytes(), &mut out, &mut Fake::new()).unwrap();

        let text = String::from_utf8(out).unwrap();
        assert_eq!(text.lines().count(), 1, "the junk line is not answered");
        assert_eq!(
            serde_json::from_str::<Value>(text.trim()).unwrap()["id"],
            json!(7),
            "the request after it still is"
        );
    }

    /// The adapter rule, inverted on purpose. Clients send `_meta` and
    /// `progressToken` today and something else next release; refusing them is
    /// how this server breaks on somebody else's upgrade, silently, for
    /// everyone at once.
    #[test]
    fn an_unknown_field_in_a_request_is_not_a_failure() {
        let raw =
            r#"{"jsonrpc":"2.0","id":1,"method":"ping","params":{},"_meta":{"progressToken":"x"}}"#;
        assert_eq!(answer(raw)["id"], json!(1));
    }

    /// Returning `{}` for anything unrecognised tells capability negotiation
    /// we implement things we do not.
    #[test]
    fn an_unknown_method_is_an_error_not_a_silent_success() {
        let got = answer(r#"{"jsonrpc":"2.0","id":1,"method":"resources/list"}"#);
        assert_eq!(got["error"]["code"], json!(-32601));
        assert!(got.get("result").is_none(), "an error carries no result");
        assert!(
            got["error"]["message"]
                .as_str()
                .unwrap()
                .contains("resources/list"),
            "say which method: {got}"
        );
    }

    /// Echoing whatever arrives claims compliance with a specification we have
    /// never seen.
    #[test]
    fn an_unknown_protocol_version_is_answered_with_ours_not_echoed() {
        assert_eq!(negotiate(Some("1999-01-01")), PROTOCOL);
        assert_eq!(negotiate(None), PROTOCOL);
        for known in SUPPORTED {
            assert_eq!(negotiate(Some(known)), known, "we said we speak {known}");
        }

        let raw = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"1999-01-01"}}"#;
        assert_eq!(answer(raw)["result"]["protocolVersion"], json!(PROTOCOL));
    }

    /// Harnesses differ on what they do with a malformed `inputSchema` —
    /// some ignore it, some hide the tool — so this fails as "the tool does
    /// not exist", which is the hardest possible thing to diagnose.
    #[test]
    fn every_tool_declares_a_schema_the_harness_can_validate() {
        let listed = answer(r#"{"jsonrpc":"2.0","id":1,"method":"tools/list","params":{}}"#);
        let tools = listed["result"]["tools"].as_array().unwrap();
        assert!(!tools.is_empty());
        for tool in tools {
            assert!(tool["name"].is_string(), "{tool}");
            assert!(!tool["description"].as_str().unwrap().is_empty(), "{tool}");
            assert_eq!(tool["inputSchema"]["type"], json!("object"), "{tool}");
            assert!(
                tool["inputSchema"]["required"].is_array(),
                "a tool with nothing required accepts an empty call: {tool}"
            );
        }
    }

    /// Caching the description in the server means a note written at 10:00 is
    /// still unadvertised at 10:05, in the same session that wrote it.
    #[test]
    fn the_tool_description_is_computed_per_call_not_once() {
        let mut tools = Fake::new();
        let req = request(r#"{"jsonrpc":"2.0","id":1,"method":"tools/list","params":{}}"#);
        let first = dispatch(&req, &mut tools).unwrap();
        let second = dispatch(&req, &mut tools).unwrap();
        assert_ne!(
            first["result"]["tools"][0]["description"], second["result"]["tools"][0]["description"],
            "the store is asked again, not remembered"
        );
    }

    /// A refusal is a successful call that reports failure, never a transport
    /// error. `-32603` reads to the agent as "this server is broken", and an
    /// agent that believes that stops calling the tool at all.
    #[test]
    fn a_refused_call_is_a_tool_error_not_a_transport_error() {
        let mut tools = Fake::new();
        tools.refuse = true;
        let req = request(
            r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"remember","arguments":{}}}"#,
        );
        let got = dispatch(&req, &mut tools).unwrap();

        assert!(got.get("error").is_none(), "not a protocol error: {got}");
        assert_eq!(got["result"]["isError"], json!(true));
        assert_eq!(got["result"]["content"][0]["text"], json!("no"));
    }

    #[test]
    fn a_successful_call_returns_its_text_and_says_it_did_not_fail() {
        let mut tools = Fake::new();
        let req = request(
            r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"recall","arguments":{"question":"q"}}}"#,
        );
        let got = dispatch(&req, &mut tools).unwrap();

        assert_eq!(got["result"]["isError"], json!(false));
        assert_eq!(got["result"]["content"][0]["text"], json!("yes"));
        assert_eq!(
            tools.called,
            vec![("recall".into(), json!({"question": "q"}))]
        );
    }

    /// A call with no `name` is genuinely malformed, and that *is* a protocol
    /// error — the distinction the refusal test guards, seen from the other
    /// side.
    #[test]
    fn a_call_naming_no_tool_is_a_protocol_error() {
        let got = answer(r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{}}"#);
        assert_eq!(got["error"]["code"], json!(-32602));
    }
}
