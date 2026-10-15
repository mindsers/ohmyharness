//! A session omh started, for real, with a model the test controls.
//!
//! `omh new` brings the session's container up with everything omh gives the
//! harness — rules, MCP servers, hooks, the shadow repository — and then runs
//! the harness on a terminal. A test has no terminal, so that last step is
//! refused, and the container stays up with everything in place. The turn is
//! then run in it by `docker exec`, as the `agent` user in `/work`, which is
//! what omh's own exec does: the only difference is the `dtach` omh wraps the
//! harness in to keep it attachable, which needs a terminal too.

use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use crate::driver::Driver;

/// The fake model's source, copied into every session container.
const FAKE: &str = include_str!("fake-model.mjs");

/// Where the fake listens, inside the session container.
pub const FAKE_PORT: u16 = 8787;

pub struct Session {
    _dir: tempfile::TempDir,
    pub home: PathBuf,
    pub repo: PathBuf,
    pub id: String,
    pub container: String,
    pub harness: &'static Driver,
    /// What `omh new` said: the launch line names every hook it dropped.
    pub launched: String,
    fakes: std::cell::Cell<u16>,
}

impl Session {
    /// A repo holding `seed`, set up by `omh init`, with each of `hooks`
    /// selected by `omh use` and committed, and a session of `harness` started
    /// in it by `omh new`.
    pub fn start(harness: &'static Driver, seed: &[(&str, &str)], hooks: &[&str]) -> Session {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("home");
        let repo = dir.path().join("repo");
        std::fs::create_dir_all(&home).unwrap();
        std::fs::create_dir_all(&repo).unwrap();
        let mut session = Session {
            _dir: dir,
            home,
            repo,
            id: String::new(),
            container: String::new(),
            harness,
            launched: String::new(),
            fakes: std::cell::Cell::new(0),
        };
        session.git(&["init", "-q", "-b", "main"]);
        std::fs::write(
            session.repo.join("README.md"),
            "# a repo for omh's e2e suite\n",
        )
        .unwrap();
        for (file, contents) in seed {
            let path = session.repo.join(file);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, contents).unwrap();
        }
        session.git(&["add", "-A"]);
        session.git(&["commit", "-q", "-m", "init"]);

        let init = session.omh(&["init"]);
        assert!(init.status.success(), "omh init: {}", text(&init));
        session.id = repo_id(&session.omh(&["info", "--repo", "--json"]));
        for hook in hooks {
            let used = session.omh(&["use", "hooks", hook]);
            assert!(
                used.status.success(),
                "omh use hooks {hook}: {}",
                text(&used)
            );
        }
        // What `init` and `use` wrote is the project's, and a session's
        // worktree starts from what is committed.
        session.git(&["add", "-A"]);
        session.git(&["commit", "-q", "-m", "omh"]);

        // Refused at the last step, for want of a terminal — everything before
        // it is what the test is here for.
        let new = session.omh(&["new", harness.name]);
        assert!(
            !new.status.success() && text(&new).contains("not a terminal"),
            "omh new {} was expected to start the container and stop at the \
             terminal: {}",
            harness.name,
            text(&new)
        );
        session.launched = text(&new);
        session.container = format!("omh-{}-s01", session.id);
        let running = docker(&["inspect", "-f", "{{.State.Running}}", &session.container]);
        assert_eq!(
            String::from_utf8_lossy(&running.stdout).trim(),
            "true",
            "the session container is not running: {}",
            text(&running)
        );
        session.provide();
        session
    }

    /// The files the harness's driver needs in the container — omp's model
    /// provider — written again whenever the container is new.
    pub fn provide(&self) {
        for (file, contents) in self.harness.files {
            self.write(file, contents);
        }
    }

    /// Whether the session's container is running.
    pub fn running(&self) -> bool {
        let out = docker(&["inspect", "-f", "{{.State.Running}}", &self.container]);
        String::from_utf8_lossy(&out.stdout).trim() == "true"
    }

    /// Run omh, in the repo, as this sandbox's user.
    pub fn omh(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_omh"))
            .args(args)
            .current_dir(&self.repo)
            .env("HOME", &self.home)
            .envs(identity())
            .stdin(Stdio::null())
            .output()
            .expect("the binary under test must run")
    }

    pub fn git(&self, args: &[&str]) -> Output {
        let out = Command::new("git")
            .args(args)
            .current_dir(&self.repo)
            .env("HOME", &self.home)
            .envs(identity())
            .output()
            .expect("git must run");
        assert!(out.status.success(), "git {args:?}: {}", text(&out));
        out
    }

    /// Write a file in the container, as the agent. `~` is the agent's home,
    /// and a relative path is in the worktree.
    pub fn write(&self, file: &str, contents: &str) {
        let file = match file {
            f if f.starts_with('~') => f.replacen('~', "/home/agent", 1),
            f if f.starts_with('/') => f.to_string(),
            f => format!("/work/{f}"),
        };
        let parent = Path::new(&file).parent().unwrap().display().to_string();
        let out = self.exec_stdin(
            &[
                "sh",
                "-c",
                &format!("mkdir -p '{parent}' && cat > '{file}'"),
            ],
            contents,
        );
        assert!(out.status.success(), "writing {file}: {}", text(&out));
    }

    /// Run a command in the session container, as the agent, in `/work`.
    pub fn exec(&self, env: &[(String, String)], argv: &[String]) -> Output {
        let mut args: Vec<String> = vec![
            "exec".into(),
            "-u".into(),
            "agent".into(),
            "-w".into(),
            "/work".into(),
        ];
        for (k, v) in env {
            args.push("-e".into());
            args.push(format!("{k}={v}"));
        }
        args.push(self.container.clone());
        args.extend(argv.iter().cloned());
        Command::new("docker")
            .args(&args)
            .stdin(Stdio::null())
            .output()
            .expect("docker must run")
    }

    fn exec_stdin(&self, argv: &[&str], input: &str) -> Output {
        use std::io::Write;
        let mut child = Command::new("docker")
            .args(["exec", "-i", "-u", "agent", "-w", "/work", &self.container])
            .args(argv)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("docker must run");
        child
            .stdin
            .take()
            .unwrap()
            .write_all(input.as_bytes())
            .unwrap();
        child.wait_with_output().unwrap()
    }

    /// One turn of the harness, against a fake model playing `script`.
    pub fn turn(&self, prompt: &str, script: &[serde_json::Value]) -> Turn {
        // A directory per turn, so nothing the last fake logged can be taken
        // for this turn's. One port for all of them — omp's provider is a file
        // written once per session — and it is clear of the ports omp probes
        // for local providers (11434, 8080, 1234).
        let k = self.fakes.get() + 1;
        self.fakes.set(k);
        let dir = format!("/tmp/omh-fake-{k}");
        self.write(&format!("{dir}/fake-model.mjs"), FAKE);
        self.write(
            &format!("{dir}/script.json"),
            &serde_json::to_string(script).unwrap(),
        );
        let start = format!(
            "FAKE_PORT={FAKE_PORT} FAKE_DIR={dir} nohup node {dir}/fake-model.mjs > {dir}/fake.log 2>&1 & \
             echo $! > {dir}/pid; \
             for _ in $(seq 50); do [ -f {dir}/ready ] && exit 0; sleep 0.1; done; \
             cat {dir}/fake.log; exit 1"
        );
        let up = self.exec(&[], &["sh".into(), "-c".into(), start]);
        assert!(
            up.status.success(),
            "the fake model did not start: {}",
            text(&up)
        );

        let url = format!("http://127.0.0.1:{FAKE_PORT}");
        let mut env: Vec<(String, String)> =
            ["HTTPS_PROXY", "HTTP_PROXY", "https_proxy", "http_proxy"]
                .iter()
                .map(|k| (k.to_string(), url.clone()))
                .collect();
        env.push(("NO_PROXY".into(), "127.0.0.1,localhost".into()));
        env.push(("no_proxy".into(), "127.0.0.1,localhost".into()));
        for (k, v) in self.harness.env {
            env.push((k.to_string(), v.replace("{url}", &url)));
        }
        let argv = (self.harness.turn)(prompt, &url);
        let out = self.exec(&env, &argv);

        // Stopped, and gone, before the next turn wants the port.
        let stop = format!(
            "p=$(cat {dir}/pid); kill $p; for _ in $(seq 50); do kill -0 $p 2>/dev/null || exit 0; sleep 0.1; done; exit 1"
        );
        let stopped = self.exec(&[], &["sh".into(), "-c".into(), stop]);
        assert!(
            stopped.status.success(),
            "the fake model did not stop: {}",
            text(&stopped)
        );
        let log = self.exec(&[], &["cat".into(), format!("{dir}/requests.jsonl")]);
        let requests = String::from_utf8_lossy(&log.stdout)
            .lines()
            .map(|l| serde_json::from_str(l).expect("the fake writes JSON lines"))
            .collect();
        Turn { out, requests }
    }

    /// Whether `omh new` named `hook` as dropped for this harness: the
    /// launch line reads `— dropped hooks: graph-first (no `search` tool), …`.
    pub fn dropped(&self, hook: &str) -> bool {
        self.launched
            .lines()
            .filter(|l| l.contains("dropped hooks:"))
            .any(|l| l.contains(&format!("{hook} (")))
    }

    /// Run a shell command in the container, as the agent, and say whether
    /// it succeeded.
    pub fn sh(&self, command: &str) -> Output {
        self.exec(&[], &["sh".into(), "-c".into(), command.into()])
    }

    /// A file in the worktree, as the agent left it.
    pub fn read(&self, file: &str) -> Option<String> {
        std::fs::read_to_string(self.worktree().join(file)).ok()
    }

    pub fn worktree(&self) -> PathBuf {
        self.home.join(".omh/worktrees").join(&self.id).join("s01")
    }

    /// What omh rendered for this harness in this session: `rules.md`,
    /// `hooks.rendered`, and so on.
    pub fn rendered(&self, file: &str) -> String {
        let path = self
            .home
            .join(".omh/run")
            .join(&self.id)
            .join("s01")
            .join(self.harness.name)
            .join(file);
        std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
    }
}

/// Everything omh started for this checkout is named after its id — the
/// session's container and network, the graph's volume — and the id holds a
/// digest of a temporary path, so nothing of anyone else's can match.
/// Removed here, panicking or not: the address pool a session network comes
/// from is finite.
impl Drop for Session {
    fn drop(&mut self) {
        if self.id.is_empty() {
            return;
        }
        let ours =
            |name: &str| name.starts_with(&format!("omh-{}", self.id)) || name.contains(&self.id);
        let named = |listing: &[&str]| -> Vec<String> {
            let out = docker(listing);
            String::from_utf8_lossy(&out.stdout)
                .lines()
                .map(str::trim)
                .filter(|n| ours(n))
                .map(str::to_string)
                .collect()
        };
        for c in named(&["ps", "-a", "--format", "{{.Names}}"]) {
            docker(&["rm", "-f", &c]);
        }
        for n in named(&["network", "ls", "--format", "{{.Name}}"]) {
            docker(&["network", "rm", &n]);
        }
        for v in named(&["volume", "ls", "--format", "{{.Name}}"]) {
            docker(&["volume", "rm", &v]);
        }
    }
}

/// What the harness printed, and what the model was sent.
pub struct Turn {
    pub out: Output,
    pub requests: Vec<serde_json::Value>,
}

impl Turn {
    /// The requests that offered tools: the agent asking what to do next.
    pub fn asks(&self) -> Vec<&serde_json::Value> {
        self.requests
            .iter()
            .filter(|r| r["body"]["tools"].as_array().is_some_and(|t| !t.is_empty()))
            .collect()
    }

    /// Every string in a request's body, joined: what the model read, without
    /// the JSON escaping between the test and the text.
    pub fn seen(request: &serde_json::Value) -> String {
        fn walk(v: &serde_json::Value, out: &mut String) {
            match v {
                serde_json::Value::String(s) => {
                    out.push_str(s);
                    out.push('\n');
                }
                serde_json::Value::Array(a) => a.iter().for_each(|x| walk(x, out)),
                serde_json::Value::Object(o) => o.values().for_each(|x| walk(x, out)),
                _ => {}
            }
        }
        let mut out = String::new();
        walk(&request["body"], &mut out);
        out
    }

    /// The text of every tool result in the last request: what the model was
    /// told its calls returned, in each protocol's own shape.
    pub fn results(&self) -> Vec<String> {
        let Some(last) = self.asks().last().copied() else {
            return Vec::new();
        };
        let body = &last["body"];
        let history = body["messages"].as_array().or(body["input"].as_array());
        let mut out = Vec::new();
        for m in history.into_iter().flatten() {
            let piece = match (m["type"].as_str(), m["role"].as_str()) {
                // Responses: a call's output is an item of its own.
                (Some(t), _) if t.ends_with("_call_output") || t == "tool_search_output" => {
                    Some(m.clone())
                }
                // Chat Completions: a message from the `tool` role.
                (_, Some("tool")) => Some(m["content"].clone()),
                // Anthropic: `tool_result` blocks inside a user message.
                (_, Some("user")) => Some(serde_json::Value::Array(
                    m["content"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter(|c| c["type"] == "tool_result")
                        .cloned()
                        .collect(),
                )),
                _ => None,
            };
            if let Some(p) = piece {
                let text = Turn::seen(&serde_json::json!({ "body": p }));
                if !text.trim().is_empty() {
                    out.push(text);
                }
            }
        }
        out
    }

    /// Hosts the harness tried to reach other than the model.
    pub fn elsewhere(&self) -> Vec<String> {
        self.requests
            .iter()
            .filter(|r| r["method"] == "CONNECT")
            .filter_map(|r| r["url"].as_str().map(str::to_string))
            .collect()
    }

    /// What the harness printed, and the last two messages the model was sent:
    /// the last call and its result, which is what a failure needs to show.
    pub fn printed(&self) -> String {
        let last = self
            .asks()
            .last()
            .map(|r| {
                let body = &r["body"];
                let history = body["messages"].as_array().or(body["input"].as_array());
                let tail: Vec<&serde_json::Value> = history
                    .map(|h| h.iter().rev().take(2).rev().collect())
                    .unwrap_or_default();
                serde_json::to_string_pretty(&tail).unwrap()
            })
            .unwrap_or_default();
        format!(
            "{}\n--- the last two messages the model was sent\n{last}",
            text(&self.out)
        )
    }
}

pub fn docker(args: &[&str]) -> Output {
    Command::new("docker")
        .args(args)
        .output()
        .expect("docker must run")
}

pub fn text(out: &Output) -> String {
    format!(
        "exit {:?}\n--- stdout\n{}\n--- stderr\n{}",
        out.status.code(),
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

fn identity() -> [(&'static str, &'static str); 4] {
    [
        ("GIT_AUTHOR_NAME", "omh e2e"),
        ("GIT_AUTHOR_EMAIL", "e2e@omh.invalid"),
        ("GIT_COMMITTER_NAME", "omh e2e"),
        ("GIT_COMMITTER_EMAIL", "e2e@omh.invalid"),
    ]
}

fn repo_id(info: &Output) -> String {
    serde_json::from_slice::<serde_json::Value>(&info.stdout)
        .ok()
        .and_then(|v| v["repo_id"].as_str().map(str::to_string))
        .unwrap_or_else(|| panic!("omh info --repo --json names no repo_id: {}", text(info)))
}
