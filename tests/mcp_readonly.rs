//! Every tool through the real binary over a copy of the fixture store: the data
//! directory is byte for byte what it was after every call, a directory that does
//! not exist stays that way, and nothing runs `gro` or `lofty`.

use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{channel, Receiver};
use std::time::Duration;

use serde_json::{json, Value};

fn fixture_store() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/store")
}

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.path().is_dir() {
            copy_dir(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), target).unwrap();
        }
    }
}

/// Every path under `dir` and what it holds.
fn snapshot(dir: &Path, into: &mut BTreeMap<PathBuf, Vec<u8>>) {
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            into.insert(path.clone(), Vec::new());
            snapshot(&path, into);
        } else {
            into.insert(path.clone(), std::fs::read(path).unwrap());
        }
    }
}

fn state_of(dir: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    let mut all = BTreeMap::new();
    snapshot(dir, &mut all);
    all
}

/// A directory for `PATH` with a `gro` and a `lofty` that mark `ran` if run.
fn stubs_in(dir: &Path) -> PathBuf {
    let stubs = dir.join("stubs");
    std::fs::create_dir_all(&stubs).unwrap();
    for name in ["gro", "lofty"] {
        let path = stubs.join(name);
        let script = format!("#!/bin/sh\necho x > {}\n", dir.join("ran").display());
        std::fs::write(&path, script).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
    }
    stubs
}

struct Server {
    child: Child,
    stdin: ChildStdin,
    lines: Receiver<String>,
    next_id: u64,
}

impl Server {
    fn start(data: &Path, stubs: &Path) -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_lofty-pm"))
            .arg("mcp")
            .env("XDG_CONFIG_HOME", stubs.join("config"))
            .env("LOFTY_PM_DATA_DIR", data)
            // `gro` and `lofty` here leave a mark if anything runs them.
            .env("PATH", stubs)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("lofty-pm starts");
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
        let mut server = Self {
            child,
            stdin,
            lines,
            next_id: 1,
        };
        server.request(
            "initialize",
            json!({
                "protocolVersion": "2025-06-18",
                "capabilities": {},
                "clientInfo": {"name": "test", "version": "0"}
            }),
        );
        server
    }

    fn request(&mut self, method: &str, params: Value) -> Value {
        let id = self.next_id;
        self.next_id += 1;
        let msg = json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params});
        writeln!(self.stdin, "{msg}").unwrap();
        self.stdin.flush().unwrap();
        loop {
            let line = self
                .lines
                .recv_timeout(Duration::from_secs(30))
                .expect("a response within 30 s");
            let msg: Value = serde_json::from_str(&line).unwrap();
            if msg["id"] == json!(id) {
                return msg;
            }
        }
    }

    fn call(&mut self, name: &str, arguments: Value) -> Value {
        self.request("tools/call", json!({"name": name, "arguments": arguments}))["result"].clone()
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn every_tool_call() -> Vec<(&'static str, Value)> {
    vec![
        ("loftypm_pending", json!({})),
        ("loftypm_pending", json!({"response_format": "detailed"})),
        ("loftypm_votes", json!({})),
        ("loftypm_votes", json!({"response_format": "detailed"})),
        ("loftypm_votes", json!({"vote": "v-lapsed"})),
        ("loftypm_votes", json!({"state": "missed"})),
        ("loftypm_votes", json!({"property": "elm"})),
        ("loftypm_digest", json!({})),
        (
            "loftypm_digest",
            json!({"since": "2030-02-01", "property": "oak"}),
        ),
        ("loftypm_ledger", json!({})),
        ("loftypm_ledger", json!({"vote": "v-decide"})),
    ]
}

#[test]
fn every_tool_leaves_the_data_directory_exactly_as_it_was() {
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path().join("data");
    copy_dir(&fixture_store(), &data);
    let stubs = stubs_in(dir.path());
    let before = state_of(&data);
    let mut server = Server::start(&data, &stubs);

    let listed = server.request("tools/list", json!({}));
    let mut unreached: Vec<String> = Vec::new();
    for tool in listed["result"]["tools"].as_array().unwrap() {
        unreached.push(tool["name"].as_str().unwrap().to_string());
    }
    for (name, _) in every_tool_call() {
        unreached.retain(|c| c != name);
    }
    assert!(unreached.is_empty(), "no call below reaches {unreached:?}");

    for (name, args) in every_tool_call() {
        let result = server.call(name, args.clone());
        assert_eq!(result["isError"], false, "{name} {args}: {result}");
        assert_eq!(state_of(&data), before, "{name} {args} changed the store");
    }
    assert!(!dir.path().join("ran").exists(), "a tool ran a subprocess");
}

#[test]
fn no_tool_brings_a_missing_data_directory_into_being() {
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path().join("never-created");
    let stubs = stubs_in(dir.path());
    let mut server = Server::start(&data, &stubs);
    for (name, args) in every_tool_call() {
        let result = server.call(name, args);
        assert_eq!(result["isError"], true, "{name}: {result}");
        assert!(!data.exists(), "{name} created the data directory");
    }
    assert!(!dir.path().join("ran").exists(), "a tool ran a subprocess");
}
