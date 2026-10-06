//! Claude launch through the shared SDK lifecycle, driven through the real
//! provider binary with `/bin/sh` standing in for the native CLI.

use serde_json::{json, Value};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const CONTRACT: &str = "oulipoly.provider/v1";

struct Fixture {
    root: tempfile::TempDir,
}

impl Fixture {
    fn new() -> Self {
        Self {
            root: tempfile::tempdir().unwrap(),
        }
    }

    fn path(&self, name: &str) -> PathBuf {
        self.root.path().join(name)
    }

    fn request(&self, script: &str) -> Value {
        json!({
            "contract": CONTRACT,
            "request_id": "req-lifecycle",
            "provider_instance_id": "claude-primary",
            "host": {
                "app": "oulipoly-agent-runner",
                "working_directory": self.root.path(),
                "data_root": self.path("data"),
            },
            "params": {
                "settings_id": "claude-primary",
                "mode": "headless",
                "model": {
                    "name": "claude-sonnet",
                    "provider_args": [],
                    "inputs": { "prompt": null, "named": {} }
                },
                "argv": ["/bin/sh", "-c", script],
                "working_directory": self.root.path(),
                "env": {},
                "session": { "provider_session_id": "known-session" }
            }
        })
    }

    fn spawn(&self, request: &Value) -> Child {
        let mut child = Command::new(env!("CARGO_BIN_EXE_agent-runner-claude"))
            .arg("launch")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(request.to_string().as_bytes())
            .unwrap();
        child
    }

    fn run(&self, request: &Value) -> Output {
        self.spawn(request).wait_with_output().unwrap()
    }

    fn calls(&self) -> usize {
        std::fs::read_to_string(self.path("calls"))
            .map(|text| text.lines().count())
            .unwrap_or(0)
    }
}

fn lines(output: &Output) -> Vec<Value> {
    String::from_utf8(output.stdout.clone())
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

fn decode_base64(text: &str) -> Vec<u8> {
    const TABLE: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut bits = 0_u32;
    let mut count = 0;
    let mut bytes = Vec::new();
    for byte in text.bytes().filter(|byte| *byte != b'=') {
        bits = (bits << 6) | TABLE.iter().position(|c| *c == byte).unwrap() as u32;
        count += 6;
        if count >= 8 {
            count -= 8;
            bytes.push((bits >> count) as u8);
        }
    }
    bytes
}

fn data(events: &[Value], kind: &str) -> String {
    let bytes = events
        .iter()
        .filter(|event| event["kind"] == kind)
        .flat_map(|event| decode_base64(event["data_base64"].as_str().unwrap()))
        .collect();
    String::from_utf8(bytes).unwrap()
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64
}

fn wait_for(path: &Path) {
    let started = Instant::now();
    while !path.exists() {
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "{path:?} never appeared"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn pid_from(path: &Path) -> i32 {
    let started = Instant::now();
    loop {
        if let Some(pid) = std::fs::read_to_string(path)
            .ok()
            .and_then(|text| text.trim().parse().ok())
        {
            return pid;
        }
        assert!(started.elapsed() < Duration::from_secs(10));
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn alive(pid: i32) -> bool {
    let signalable = unsafe { libc::kill(pid, 0) } == 0;
    signalable
        && std::fs::read_to_string(format!("/proc/{pid}/stat"))
            .map(|stat| {
                !stat
                    .rsplit(')')
                    .next()
                    .unwrap_or("")
                    .trim_start()
                    .starts_with('Z')
            })
            .unwrap_or(false)
}

fn assert_dies(pid: i32) {
    let started = Instant::now();
    while alive(pid) {
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "process {pid} survived"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn identical_launch_replays_exact_events_and_changed_inputs_conflict() {
    let fixture = Fixture::new();
    let mut request = fixture.request("echo call >> calls; cat; echo diag >&2; exit 4");
    request["params"]["stdin"] = json!({"encoding": "utf8", "data": "prompt bytes\n"});
    let first = fixture.run(&request);
    assert!(first.status.success(), "{first:?}");
    let events = lines(&first);
    assert_eq!(events[0]["name"], "provider_session_known");
    assert_eq!(data(&events, "stdout"), "prompt bytes\n");
    assert_eq!(data(&events, "stderr"), "diag\n");
    let exit = events.last().unwrap();
    assert_eq!(exit["status"], json!({"kind": "exited", "code": 4}));
    assert_eq!(exit["terminal_signal"]["kind"], "nonzero_exit");
    assert_eq!(exit["session"]["provider_session_id"], "known-session");

    let replay = fixture.run(&request);
    assert!(replay.status.success());
    assert_eq!(replay.stdout, first.stdout, "replay is byte-identical");
    assert_eq!(fixture.calls(), 1, "replay did not run the native command");

    request["params"]["env"] = json!({"CHANGED": "1"});
    let changed = fixture.run(&request);
    assert_eq!(changed.status.code(), Some(2));
    let error: Value = serde_json::from_slice(&changed.stdout).unwrap();
    assert_eq!(error["error"]["code"], "request_changed");
    assert_eq!(error["error"]["category"], "conflict");
    assert_eq!(fixture.calls(), 1);
}

#[test]
fn host_deadline_cancels_the_native_process_group() {
    let fixture = Fixture::new();
    let mut request =
        fixture.request("sleep 60 & echo $! > descendant; echo started; exec sleep 60");
    request["host"]["deadline_unix_ms"] = json!(now_ms() + 800);
    let started = Instant::now();
    let output = fixture.run(&request);
    assert!(started.elapsed() < Duration::from_secs(10));
    assert!(output.status.success(), "{output:?}");
    let events = lines(&output);
    assert_eq!(data(&events, "stdout"), "started\n");
    let exit = events.last().unwrap();
    assert_eq!(exit["status"], json!({"kind": "cancelled"}));
    assert_eq!(exit["terminal_signal"]["kind"], "cancelled");
    assert_dies(pid_from(&fixture.path("descendant")));
}

#[test]
fn elapsed_deadline_refuses_launch_before_native_effects() {
    let fixture = Fixture::new();
    let mut request = fixture.request("echo call >> calls");
    request["host"]["deadline_unix_ms"] = json!(1);
    let output = fixture.run(&request);
    assert_eq!(output.status.code(), Some(1));
    let error: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(error["request_id"], "req-lifecycle");
    assert_eq!(error["error"]["code"], "launch_deadline");
    assert_eq!(error["error"]["category"], "timeout");
    assert_eq!(fixture.calls(), 0);
}

#[test]
fn termination_signal_cancels_native_and_delivers_the_exit_event() {
    let fixture = Fixture::new();
    let request = fixture.request("sleep 60 & echo $! > descendant; touch ready; exec sleep 60");
    let child = fixture.spawn(&request);
    wait_for(&fixture.path("ready"));
    assert_eq!(unsafe { libc::kill(child.id() as i32, libc::SIGTERM) }, 0);
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success(), "{output:?}");
    let exit = lines(&output).pop().unwrap();
    assert_eq!(exit["kind"], "exit");
    assert_eq!(exit["terminal_signal"]["kind"], "cancelled");
    assert_dies(pid_from(&fixture.path("descendant")));
}

#[test]
fn lost_provider_is_reconciled_without_a_second_native_turn() {
    let fixture = Fixture::new();
    let request = fixture
        .request("echo call >> calls; sleep 60 & echo $! > descendant; touch ready; exec sleep 60");
    let mut child = fixture.spawn(&request);
    wait_for(&fixture.path("ready"));
    let descendant = pid_from(&fixture.path("descendant"));
    child.kill().unwrap();
    child.wait().unwrap();
    assert!(
        alive(descendant),
        "orphaned descendant keeps the native group"
    );

    let retry = fixture.run(&request);
    assert_eq!(retry.status.code(), Some(2));
    let error: Value = serde_json::from_slice(&retry.stdout).unwrap();
    assert_eq!(error["error"]["code"], "launch_reconciliation_required");
    assert_dies(descendant);
    assert_eq!(fixture.calls(), 1);
}

#[test]
fn descendant_holding_output_does_not_strand_completion() {
    let fixture = Fixture::new();
    let request = fixture.request("sleep 60 & echo $! > descendant; echo done; exit 0");
    let started = Instant::now();
    let output = fixture.run(&request);
    assert!(started.elapsed() < Duration::from_secs(10));
    let events = lines(&output);
    assert_eq!(data(&events, "stdout"), "done\n");
    assert_eq!(
        events.last().unwrap()["status"],
        json!({"kind": "exited", "code": 0})
    );
    assert_dies(pid_from(&fixture.path("descendant")));
}

#[test]
fn unspawnable_command_is_a_spawn_error_exit_and_replays() {
    let fixture = Fixture::new();
    let mut request = fixture.request("unused");
    request["params"]["argv"] = json!(["definitely-not-a-claude-binary-u89"]);
    let first = fixture.run(&request);
    assert!(first.status.success(), "{first:?}");
    let events = lines(&first);
    assert_eq!(events.len(), 2);
    assert_eq!(events[0]["name"], "provider_session_known");
    assert_eq!(events[1]["status"]["kind"], "spawn_error");
    assert!(events[1]["status"]["reason"]
        .as_str()
        .unwrap()
        .contains("No such file or directory"));
    assert_eq!(events[1]["terminal_signal"]["kind"], "spawn_error");
    assert_eq!(events[1]["session"]["provider_session_id"], "known-session");
    assert_eq!(fixture.run(&request).stdout, first.stdout);
}
