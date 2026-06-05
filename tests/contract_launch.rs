mod support;

use serde_json::{json, Value};
use std::fs;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use support::fixtures::{envelope, host_context, path_string, temp_roots, CONTRACT};
use support::invoke::{collect_launch_jsonl_lines, invoke, invoke_with_stdin_bytes};
use support::schema::assert_valid;

fn launch_request(roots: &support::fixtures::TempRoots, argv: Vec<String>, extra: Value) -> Value {
    let mut params = json!({
        "settings_id": "claude-primary",
        "mode": "proxy",
        "model": {
            "name": "claude-sonnet",
            "provider_args": [],
            "inputs": { "prompt": null, "named": {} }
        },
        "argv": argv,
        "working_directory": path_string(&roots.root)
    });
    for (key, value) in extra.as_object().expect("extra object") {
        params
            .as_object_mut()
            .unwrap()
            .insert(key.clone(), value.clone());
    }
    envelope(CONTRACT, host_context(roots), params)
}

fn write_script(path: &Path, body: &str) {
    fs::write(path, body).expect("write fixture script");
    #[cfg(unix)]
    {
        let mut permissions = fs::metadata(path).expect("script metadata").permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(path, permissions).expect("chmod fixture script");
    }
}

fn assert_launch_event_valid(event: &Value) {
    let schema = match event["kind"].as_str().expect("launch event kind") {
        "stdout" => "launch.schema.json#/$defs/LaunchStdoutEvent",
        "stderr" => "launch.schema.json#/$defs/LaunchStderrEvent",
        "marker" => "launch.schema.json#/$defs/LaunchMarkerEvent",
        "heartbeat" => "launch.schema.json#/$defs/LaunchHeartbeatEvent",
        "exit" => "launch.schema.json#/$defs/LaunchExitEvent",
        other => panic!("unknown launch event kind {other}: {event}"),
    };
    assert_valid(schema, event);
}

fn assert_single_pre_spawn_launch_exit(
    output: &support::invoke::Invocation,
    reason_fragment: &str,
) {
    assert_eq!(output.code, Some(0));
    assert!(output.stderr.is_empty());
    let events = collect_launch_jsonl_lines(output);
    assert_eq!(
        events.len(),
        1,
        "pre-spawn rejection must emit one exit event"
    );
    let event = &events[0];
    assert_launch_event_valid(event);
    assert_eq!(event["kind"], "exit");
    assert_eq!(event["status"]["kind"], "spawn_error");
    assert!(
        event["status"]["reason"]
            .as_str()
            .unwrap_or_default()
            .contains(reason_fragment),
        "unexpected pre-spawn rejection reason: {event}"
    );
    assert_eq!(event["terminal_signal"]["kind"], "spawn_error");
}

fn assert_seq_starts_at_one_and_monotonic(events: &[Value]) {
    assert!(!events.is_empty(), "launch stream must contain events");
    assert_eq!(events[0]["seq"], 1);
    let mut previous = 0u64;
    for event in events {
        let seq = event["seq"].as_u64().expect("numeric seq");
        assert!(seq > previous, "seq must be strictly monotonic: {events:?}");
        previous = seq;
    }
}

fn decode_base64(data: &str) -> Vec<u8> {
    let bytes = data.as_bytes();
    assert_eq!(bytes.len() % 4, 0, "base64 length must be padded");
    let mut out = Vec::new();
    for chunk in bytes.chunks(4) {
        let mut vals = [0u8; 4];
        let mut pad = 0;
        for (idx, byte) in chunk.iter().copied().enumerate() {
            vals[idx] = match byte {
                b'A'..=b'Z' => byte - b'A',
                b'a'..=b'z' => byte - b'a' + 26,
                b'0'..=b'9' => byte - b'0' + 52,
                b'+' => 62,
                b'/' => 63,
                b'=' => {
                    pad += 1;
                    0
                }
                _ => panic!("invalid base64 byte {byte}"),
            };
        }
        out.push((vals[0] << 2) | (vals[1] >> 4));
        if pad < 2 {
            out.push((vals[1] << 4) | (vals[2] >> 2));
        }
        if pad < 1 {
            out.push((vals[2] << 6) | vals[3]);
        }
    }
    out
}

fn encode_base64(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut encoded = String::new();
    for chunk in bytes.chunks(3) {
        let b0 = chunk[0];
        let b1 = *chunk.get(1).unwrap_or(&0);
        let b2 = *chunk.get(2).unwrap_or(&0);
        encoded.push(TABLE[(b0 >> 2) as usize] as char);
        encoded.push(TABLE[(((b0 & 0b0000_0011) << 4) | (b1 >> 4)) as usize] as char);
        encoded.push(if chunk.len() > 1 {
            TABLE[(((b1 & 0b0000_1111) << 2) | (b2 >> 6)) as usize] as char
        } else {
            '='
        });
        encoded.push(if chunk.len() > 2 {
            TABLE[(b2 & 0b0011_1111) as usize] as char
        } else {
            '='
        });
    }
    encoded
}

fn channel_bytes(events: &[Value], channel: &str) -> Vec<u8> {
    channel_events(events, channel)
        .into_iter()
        .flat_map(channel_event_bytes)
        .collect()
}

fn channel_events<'a>(events: &'a [Value], channel: &str) -> Vec<&'a Value> {
    events
        .iter()
        .filter(|event| event["kind"] == channel)
        .collect()
}

fn channel_event_bytes(event: &Value) -> Vec<u8> {
    decode_base64(channel_event_data_base64(event))
}

fn channel_event_data_base64(event: &Value) -> &str {
    event["data_base64"].as_str().unwrap()
}

fn now_unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock after epoch")
        .as_millis() as u64
}

fn process_exists(pid: u32) -> bool {
    PathBuf::from(format!("/proc/{pid}")).exists()
}

#[test]
fn launch_reports_signal_exit_event_and_provider_exits_zero() {
    let roots = temp_roots("launch-signal-exit");
    let script = roots.root.join("signal-child.sh");
    write_script(&script, "#!/bin/sh\nkill -TERM $$\n");

    let request = launch_request(&roots, vec![path_string(&script)], json!({}));
    let output = invoke("launch", &request);

    assert_eq!(
        output.code,
        Some(0),
        "provider exits zero after emitting a valid final launch event"
    );
    assert!(output.stderr.is_empty());
    let events = collect_launch_jsonl_lines(&output);
    for event in &events {
        assert_launch_event_valid(event);
    }
    assert_seq_starts_at_one_and_monotonic(&events);

    let final_event = events.last().expect("launch stream has final event");
    assert_eq!(final_event["kind"], "exit");
    assert_eq!(final_event["status"]["kind"], "signal_terminated");
    assert!(
        final_event["status"]["signal"].as_i64().unwrap_or_default() > 0,
        "signal exit must report the terminating Unix signal: {final_event}"
    );
    assert_eq!(final_event["terminal_signal"]["kind"], "signal_exit");
}

#[test]
fn launch_reports_spawn_error_for_non_executable_command_without_running_fixture() {
    let roots = temp_roots("launch-spawn-error");
    let marker = roots.root.join("non-executable-ran");
    let script = roots.root.join("not-executable.sh");
    fs::write(
        &script,
        format!("#!/bin/sh\nprintf ran > '{}'\n", marker.display()),
    )
    .expect("write non-executable fixture");
    #[cfg(unix)]
    {
        let mut permissions = fs::metadata(&script)
            .expect("script metadata")
            .permissions();
        permissions.set_mode(0o644);
        fs::set_permissions(&script, permissions).expect("chmod non-executable fixture");
    }

    let request = launch_request(&roots, vec![path_string(&script)], json!({}));
    let output = invoke("launch", &request);

    assert_eq!(
        output.code,
        Some(0),
        "spawn errors are reported as launch exit events, not provider failures"
    );
    assert!(output.stderr.is_empty());
    assert!(
        !marker.exists(),
        "non-executable fixture must not run or leave an orphaned child side effect"
    );
    let events = collect_launch_jsonl_lines(&output);
    for event in &events {
        assert_launch_event_valid(event);
    }
    assert_seq_starts_at_one_and_monotonic(&events);

    let final_event = events.last().expect("launch stream has final event");
    assert_eq!(final_event["kind"], "exit");
    assert_eq!(final_event["status"]["kind"], "spawn_error");
    assert!(
        !final_event["status"]["reason"]
            .as_str()
            .unwrap_or_default()
            .is_empty(),
        "spawn error status must include a reason: {final_event}"
    );
    assert_eq!(final_event["terminal_signal"]["kind"], "spawn_error");
}

#[test]
fn launch_rejects_invalid_stdin_bytepayload_before_spawning_child() {
    let roots = temp_roots("launch-invalid-base64");
    let marker = roots.root.join("child-spawned");
    let script = roots.root.join("child.sh");
    write_script(
        &script,
        &format!("#!/bin/sh\nprintf spawned > '{}'\n", marker.display()),
    );

    let request = launch_request(
        &roots,
        vec![path_string(&script)],
        json!({ "stdin": { "encoding": "base64", "data": "not@@base64" } }),
    );
    let output = invoke("launch", &request);
    assert!(
        !marker.exists(),
        "child must not spawn when stdin base64 is invalid"
    );
    assert_single_pre_spawn_launch_exit(&output, "invalid launch stdin");
}

#[test]
fn launch_rejects_invalid_utf8_stdin_payload_before_spawning_child() {
    let roots = temp_roots("launch-invalid-utf8");
    let marker = roots.root.join("child-spawned");
    let script = roots.root.join("child.sh");
    write_script(
        &script,
        &format!("#!/bin/sh\nprintf spawned > '{}'\n", marker.display()),
    );

    let request = launch_request(
        &roots,
        vec![path_string(&script)],
        json!({ "stdin": { "encoding": "utf8", "data": "" } }),
    );
    let request_text = request
        .to_string()
        .replace("\"data\":\"\"", "\"data\":\"\\uD800\"");
    let output = invoke_with_stdin_bytes(Some("launch"), request_text.as_bytes());
    assert!(
        !marker.exists(),
        "child must not spawn when stdin UTF-8 is invalid"
    );
    assert_single_pre_spawn_launch_exit(&output, "request envelope");
}

#[test]
fn launch_propagates_cwd_env_stdin_and_streams_byte_exact_events() {
    let roots = temp_roots("launch-propagation");
    let script = roots.root.join("child.sh");
    let stdin_capture = roots.root.join("stdin.bin");
    let pwd_capture = roots.root.join("pwd.txt");
    let env_capture = roots.root.join("env.txt");
    write_script(
        &script,
        &format!(
            "#!/bin/sh\npwd > '{}'\nprintf '%s' \"$CONTRACT_LAUNCH_TEST_ENV\" > '{}'\ndd of='{}' bs=1 status=none\nprintf 'out\\000\\377A\\n'\nprintf 'err\\001\\376Z\\n' >&2\nexit 7\n",
            pwd_capture.display(),
            env_capture.display(),
            stdin_capture.display()
        ),
    );

    let stdin_bytes = b"stdin\0\xffpayload";
    let request = launch_request(
        &roots,
        vec![path_string(&script)],
        json!({
            "env": { "CONTRACT_LAUNCH_TEST_ENV": "propagated-value" },
            "stdin": { "encoding": "base64", "data": encode_base64(stdin_bytes) }
        }),
    );
    let output = invoke("launch", &request);
    assert_eq!(
        output.code,
        Some(0),
        "provider exits zero after a valid exit event"
    );
    assert!(output.stderr.is_empty());
    let events = collect_launch_jsonl_lines(&output);
    for event in &events {
        assert_launch_event_valid(event);
    }
    assert_seq_starts_at_one_and_monotonic(&events);
    assert!(
        events.iter().any(|event| event["kind"] == "marker"),
        "launch stream must include a marker event"
    );
    for heartbeat in events.iter().filter(|event| event["kind"] == "heartbeat") {
        assert_launch_event_valid(heartbeat);
    }
    assert_eq!(
        events.last().unwrap()["kind"],
        "exit",
        "final event must be LaunchExitEvent"
    );
    assert_eq!(
        events.last().unwrap()["status"],
        json!({ "kind": "exited", "code": 7 })
    );
    assert_eq!(
        fs::read(stdin_capture).expect("captured stdin"),
        stdin_bytes
    );
    assert_eq!(
        fs::read_to_string(pwd_capture)
            .expect("captured cwd")
            .trim(),
        path_string(&roots.root)
    );
    assert_eq!(
        fs::read_to_string(env_capture).expect("captured env"),
        "propagated-value"
    );
    assert_eq!(channel_bytes(&events, "stdout"), b"out\0\xffA\n");
    assert_eq!(channel_bytes(&events, "stderr"), b"err\x01\xfeZ\n");
}

#[test]
fn launch_timeout_cancels_process_group_without_orphaning_grandchild() {
    let roots = temp_roots("launch-timeout-pgrp");
    let script = roots.root.join("child.sh");
    let pid_file = roots.root.join("grandchild.pid");
    write_script(
        &script,
        &format!(
            "#!/bin/sh\n(sleep 30) &\necho $! > '{}'\nwhile :; do sleep 1; done\n",
            pid_file.display()
        ),
    );

    let mut host = host_context(&roots);
    host.as_object_mut()
        .unwrap()
        .insert("deadline_unix_ms".to_string(), json!(now_unix_ms() + 500));
    let request = envelope(
        CONTRACT,
        host,
        json!({
            "settings_id": "claude-primary",
            "mode": "proxy",
            "model": {
                "name": "claude-sonnet",
                "provider_args": [],
                "inputs": { "prompt": null, "named": {} }
            },
            "argv": [path_string(&script)],
            "working_directory": path_string(&roots.root)
        }),
    );

    let output = invoke("launch", &request);
    assert_eq!(output.code, Some(0));
    let events = collect_launch_jsonl_lines(&output);
    for event in &events {
        assert_launch_event_valid(event);
    }
    assert_eq!(events.last().unwrap()["kind"], "exit");
    assert_ne!(
        events.last().unwrap()["status"],
        json!({ "kind": "exited", "code": 0 })
    );

    let pid_text =
        fs::read_to_string(&pid_file).expect("grandchild pid must be captured before timeout");
    let grandchild_pid = pid_text
        .trim()
        .parse::<u32>()
        .expect("numeric grandchild pid");
    for _ in 0..50 {
        if !process_exists(grandchild_pid) {
            return;
        }
        thread::sleep(Duration::from_millis(100));
    }
    assert!(
        !process_exists(grandchild_pid),
        "grandchild process {grandchild_pid} must not be orphaned"
    );
}
