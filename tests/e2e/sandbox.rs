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
    /// Turns so far, naming each turn's fake. A `Cell`, so a scenario's
    /// `&Session` can count; which leaves `Session` not `Sync`, and right
    /// so — its scenarios run one after another, on one thread.
    fakes: std::cell::Cell<u16>,
}

impl Session {
    /// A repo holding `seed`, set up by `omh init`, with each of `hooks`
    /// selected by `omh use` and committed, and a session of `harness` started
    /// in it by `omh new`.
    pub fn start(harness: &'static Driver, seed: &[(&str, &str)], hooks: &[&str]) -> Session {
        // One start at a time. `omh init` builds the image a launch needs and
        // removes the ones it supersedes, and four of them at once after a
        // recipe changed removed each other's images: `No such image` on
        // whichever came last. The turns, after, run side by side.
        static STARTING: std::sync::Mutex<()> = std::sync::Mutex::new(());
        let _one = STARTING.lock().unwrap_or_else(|e| e.into_inner());
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

        // A captured account, for a driver that launches with one.
        if let Some(account) = harness.account {
            let dir = session.account();
            for (file, contents) in account() {
                let path = dir.join(file);
                std::fs::create_dir_all(path.parent().unwrap()).unwrap();
                std::fs::write(path, contents).unwrap();
            }
        }

        // Refused at the last step, for want of a terminal — everything before
        // it is what the test is here for. Docker words that refusal its own
        // way per platform ("the input device is not a TTY" on Linux, "stdin
        // is not a terminal" on Docker Desktop), so the check is the state it
        // leaves: omh failed, and the session's container is up.
        let new = session.omh(&["new", harness.name]);
        session.launched = text(&new);
        session.container = format!("omh-{}-s01", session.id);
        assert!(
            !new.status.success() && session.running(),
            "omh new {} was expected to start the container and stop at the \
             terminal: {}",
            harness.name,
            session.launched
        );
        session.provide();
        session
    }

    /// The files the harness's driver needs in the container — omp's model
    /// provider — written again whenever the container is new.
    pub fn provide(&self) {
        for (file, contents) in self.harness.files {
            self.write(file, &contents.replace("{url}", &fake_url()));
        }
    }

    /// `omh s01 resume`, and the new container provided for.
    pub fn resume(&self) {
        let resumed = self.omh(&["s01", "resume"]);
        assert!(
            self.running(),
            "resume did not bring the session back: {}",
            text(&resumed)
        );
        self.provide();
    }

    /// Whether the session's container is running. One that is not there is
    /// not running; a daemon that cannot be asked fails the test, rather than
    /// answer "stopped" to the scenario checking that `down` stopped it.
    pub fn running(&self) -> bool {
        let out = docker(&["ps", "-a", "--format", "{{.Names}} {{.State}}"]);
        assert!(out.status.success(), "listing containers: {}", text(&out));
        String::from_utf8_lossy(&out.stdout)
            .lines()
            .any(|l| l.trim() == format!("{} running", self.container))
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
        // naming it, written whenever the container is new — and it is clear
        // of the ports omp probes
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

        let url = fake_url();
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
        assert!(
            log.status.success(),
            "the fake model's log could not be read: {}",
            text(&log)
        );
        let requests: Vec<serde_json::Value> = String::from_utf8_lossy(&log.stdout)
            .lines()
            .map(|l| serde_json::from_str(l).expect("the fake writes JSON lines"))
            .collect();
        // Each line is one of the four things the fake writes. A key renamed
        // in `fake-model.mjs` would otherwise leave `unreadable` and
        // `elsewhere` counting nothing, and the checks on them passing.
        for r in &requests {
            let kinds = [
                r["method"] == "CONNECT",
                r["proxied"] == true,
                r["bad_json"] == true,
                r.get("plan").is_some() && r.get("body").is_some(),
            ];
            assert_eq!(
                kinds.iter().filter(|k| **k).count(),
                1,
                "the fake logged a line this suite does not read: {r}"
            );
        }
        let calls = script.iter().filter(|s| s.get("text").is_none()).count();
        Turn {
            out,
            requests,
            calls,
        }
    }

    /// Whether `omh new` named `hook` as dropped for this harness: the
    /// launch line reads `— dropped hooks: graph-first (no `search` tool), …`.
    pub fn dropped(&self, hook: &str) -> bool {
        self.launched
            .lines()
            .filter(|l| l.contains("dropped hooks:"))
            .any(|l| l.contains(&format!("{hook} (")))
    }

    /// Every hook `omh new` named as dropped.
    pub fn dropped_all(&self) -> std::collections::BTreeSet<String> {
        let mut out = std::collections::BTreeSet::new();
        for line in self
            .launched
            .lines()
            .filter(|l| l.contains("dropped hooks:"))
        {
            let list = line.split("dropped hooks:").nth(1).unwrap_or("");
            // `name (why), name (why)`: a name is what comes before ` (` at
            // the start of the list or after `), `.
            for part in list.split("), ") {
                if let Some((name, _)) = part.trim().split_once(" (") {
                    out.insert(name.trim().to_string());
                }
            }
        }
        out
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

    /// The account a driver with one launches with, on the host.
    pub fn account(&self) -> PathBuf {
        self.home
            .join(".omh/creds")
            .join(self.harness.name)
            .join("e2e")
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
            if !out.status.success() {
                eprintln!("e2e: could not list what to remove: {}", text(&out));
            }
            String::from_utf8_lossy(&out.stdout)
                .lines()
                .map(str::trim)
                .filter(|n| ours(n))
                .map(str::to_string)
                .collect()
        };
        // Said, never asserted: a panic in `drop` while a scenario's panic
        // unwinds aborts the run, and a leak is worth a line, not that.
        let remove = |what: &str, args: &[&str]| {
            let out = docker(args);
            if !out.status.success() {
                eprintln!("e2e: could not remove {what}: {}", text(&out));
            }
        };
        for c in named(&["ps", "-a", "--format", "{{.Names}}"]) {
            remove(&c, &["rm", "-f", &c]);
        }
        for n in named(&["network", "ls", "--format", "{{.Name}}"]) {
            remove(&n, &["network", "rm", &n]);
        }
        for v in named(&["volume", "ls", "--format", "{{.Name}}"]) {
            remove(&v, &["volume", "rm", &v]);
        }
    }
}

/// What the harness printed, and what the model was sent.
pub struct Turn {
    pub out: Output,
    pub requests: Vec<serde_json::Value>,
    /// The script's calls: every step but plain text.
    pub calls: usize,
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

    /// Every tool result in the last request, one item each: what the model
    /// was told its calls returned, in each protocol's own shape.
    fn outputs(&self) -> Vec<serde_json::Value> {
        let Some(last) = self.asks().last().copied() else {
            return Vec::new();
        };
        let body = &last["body"];
        let history = body["messages"].as_array().or(body["input"].as_array());
        let mut out = Vec::new();
        for m in history.into_iter().flatten() {
            match (m["type"].as_str(), m["role"].as_str()) {
                // Responses: a call's output is an item of its own.
                (Some(t), _) if t.ends_with("_call_output") || t == "tool_search_output" => {
                    out.push(m.clone())
                }
                // Chat Completions: a message from the `tool` role.
                (_, Some("tool")) => out.push(m["content"].clone()),
                // Anthropic: `tool_result` blocks inside a user message.
                (_, Some("user")) => out.extend(
                    m["content"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter(|c| c["type"] == "tool_result")
                        .cloned(),
                ),
                _ => {}
            }
        }
        out
    }

    /// The text of every tool result in the last request.
    pub fn results(&self) -> Vec<String> {
        self.outputs()
            .into_iter()
            .map(|p| Turn::seen(&serde_json::json!({ "body": p })))
            .filter(|text| !text.trim().is_empty())
            .collect()
    }

    /// Whether the turn played its whole script: the fake reached the end of
    /// it, and the model was told what every call returned. Reaching the end
    /// alone is not enough — a harness that ended the turn on a call it did
    /// not make, and asked again for a hook of its own, was answered "done".
    pub fn played(&self) -> bool {
        self.requests
            .iter()
            .any(|r| r["plan"]["text"] == "FAKE_DONE")
            && self.outputs().len() >= self.calls
    }

    /// Requests the fake could not parse, which it refused rather than played.
    pub fn unreadable(&self) -> usize {
        self.requests
            .iter()
            .filter(|r| r["bad_json"] == true)
            .count()
    }

    /// Hosts the harness tried to reach other than the model.
    pub fn elsewhere(&self) -> Vec<String> {
        self.requests
            .iter()
            .filter(|r| r["method"] == "CONNECT" || r["proxied"] == true)
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

/// Where a turn's fake model listens, in the container.
pub fn fake_url() -> String {
    format!("http://127.0.0.1:{FAKE_PORT}")
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
