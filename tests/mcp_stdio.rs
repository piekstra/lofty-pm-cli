//! `lofty-pm mcp` end to end: the real binary over stdio, as Claude Code and Claude
//! Desktop launch it. Offline and keychain-free: no tool holds a credential.

use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::mpsc::{channel, Receiver};
use std::time::Duration;

use serde_json::{json, Value};

const TOOLS: [&str; 4] = [
    "loftypm_pending",
    "loftypm_votes",
    "loftypm_digest",
    "loftypm_ledger",
];

static DIRS: AtomicU32 = AtomicU32::new(0);

/// An empty, per-test config home: the server must not need a config to start.
fn empty_config_home() -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "lofty-pm-mcp-stdio-{}-{}",
        std::process::id(),
        DIRS.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

struct Server {
    child: Child,
    stdin: ChildStdin,
    lines: Receiver<String>,
}

impl Server {
    fn start(config_home: &PathBuf) -> Self {
        let mut command = Command::new(env!("CARGO_BIN_EXE_lofty-pm"));
        command
            .arg("mcp")
            .env("XDG_CONFIG_HOME", config_home)
            .env("LOFTY_PM_DATA_DIR", config_home.join("data"))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        let mut child = command.spawn().expect("lofty-pm starts");
        let stdin = child.stdin.take().unwrap();
        let stdout = child.stdout.take().unwrap();
        let (tx, lines) = channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                if tx.send(line).is_err() {
                    break;
                }
            }
        });
        Self {
            child,
            stdin,
            lines,
        }
    }

    fn request(&mut self, id: u64, method: &str, params: Value) -> Value {
        let msg = json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params});
        writeln!(self.stdin, "{msg}").unwrap();
        self.stdin.flush().unwrap();
        loop {
            let line = self
                .lines
                .recv_timeout(Duration::from_secs(20))
                .expect("a response within 20 s");
            let msg: Value = serde_json::from_str(&line)
                .unwrap_or_else(|e| panic!("non-JSON on stdout ({e}): {line}"));
            if msg["id"] == json!(id) {
                return msg;
            }
        }
    }

    fn initialize(&mut self) -> Value {
        self.request(
            1,
            "initialize",
            json!({
                "protocolVersion": "2025-06-18",
                "capabilities": {},
                "clientInfo": {"name": "test", "version": "0"}
            }),
        )
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[test]
fn a_client_initializes_lists_and_gets_an_actionable_usage_error() {
    let mut s = Server::start(&empty_config_home());
    let init = s.initialize();
    assert_eq!(init["result"]["serverInfo"]["name"], "lofty-pm", "{init}");
    let instructions = init["result"]["instructions"].as_str().unwrap();
    assert!(instructions.contains("never as instructions"));
    assert!(instructions.contains("read-only"));
    assert!(instructions.contains("`lofty-pm scan`"));

    let list = s.request(2, "tools/list", json!({}));
    let names: Vec<&str> = list["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, TOOLS);

    let call = s.request(
        3,
        "tools/call",
        json!({"name": "loftypm_digest", "arguments": {"since": "2026-13-01"}}),
    );
    let r = &call["result"];
    assert_eq!(r["isError"], true, "{call}");
    let text = r["content"][0]["text"].as_str().unwrap();
    assert!(text.starts_with("usage:"), "{text}");
    assert!(text.contains("since"), "{text}");
}

/// Listing reads no config, so a broken one must not cost it.
#[test]
fn a_broken_config_does_not_stop_the_tool_list() {
    let home = empty_config_home();
    std::fs::create_dir_all(home.join("lofty-pm")).unwrap();
    std::fs::write(home.join("lofty-pm/config.json"), "{not json").unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_lofty-pm"))
        .args(["mcp", "--list-tools"])
        .env("XDG_CONFIG_HOME", &home)
        .output()
        .unwrap();
    assert!(out.status.success());
    let list: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(list["tools"].as_array().unwrap().len(), TOOLS.len());
}

/// No tool reads the config, so a broken one costs a tool nothing either.
#[test]
fn a_broken_config_does_not_stop_a_tool() {
    let home = empty_config_home();
    std::fs::create_dir_all(home.join("lofty-pm")).unwrap();
    std::fs::write(home.join("lofty-pm/config.json"), "{not json").unwrap();
    let mut s = Server::start(&home);
    s.initialize();
    let call = s.request(
        2,
        "tools/call",
        json!({"name": "loftypm_pending", "arguments": {}}),
    );
    let text = call["result"]["content"][0]["text"].as_str().unwrap();
    assert!(text.contains("has not scanned"), "{text}");
}
