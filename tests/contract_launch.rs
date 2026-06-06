// declared_role: orchestration, validator, accessor, parser, formatter, predicate
// intrinsic_surface_declarations:
//   - component: tests/contract_launch.rs
//     role: intrinsic-surface
//     Domain: contract_launch_proof_surface
//     Owns:
//       - launch contract and byte-preservation scenarios
//       - support harness dependencies for launch invoke/schema/script proof

mod support;

use agent_runner_claude::encoding::encode_base64;
use serde_json::{json, Value};
use std::fs;
use std::io::{Read, Write};
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use support::assertions::{
    assert_pre_spawn_launch_exit_fields, assert_single_launch_event_count,
    assert_successful_invocation, first_launch_event,
};
use support::fixtures::{path_string, temp_roots};
use support::invoke::{collect_launch_jsonl_lines, invoke, invoke_with_stdin_bytes};
use support::requests::{launch_request, launch_timeout_request};
use support::schema::assert_valid;
use support::scripts::write_executable;

fn assert_launch_event_valid(event: &Value) {
    assert_launch_event_schema(event_schema_id(event), event);
}

fn event_schema_id(event: &Value) -> &'static str {
    let kind = event_kind(event);
    assert_known_event_kind(kind, event);
    schema_id_for_event_kind(kind)
}

fn schema_id_for_event_kind(kind: &str) -> &'static str {
    match kind {
        "stdout" => "launch.schema.json#/$defs/LaunchStdoutEvent",
        "stderr" => "launch.schema.json#/$defs/LaunchStderrEvent",
        "marker" => "launch.schema.json#/$defs/LaunchMarkerEvent",
        "heartbeat" => "launch.schema.json#/$defs/LaunchHeartbeatEvent",
        "exit" => "launch.schema.json#/$defs/LaunchExitEvent",
        _ => unreachable!("launch event kind validated"),
    }
}

fn assert_known_event_kind(kind: &str, event: &Value) {
    assert!(
        matches!(kind, "stdout" | "stderr" | "marker" | "heartbeat" | "exit"),
        "unknown launch event kind {kind}: {event}"
    );
}

fn event_kind(event: &Value) -> &str {
    event["kind"].as_str().expect("launch event kind")
}

fn assert_launch_event_schema(schema: &str, event: &Value) {
    assert_valid(schema, event);
}

fn assert_single_pre_spawn_launch_exit(
    output: &support::invoke::Invocation,
    reason_fragment: &str,
) {
    assert_successful_invocation(output);
    let events = collect_launch_jsonl_lines(output);
    assert_single_launch_event_count(&events);
    let event = first_launch_event(&events);
    assert_launch_event_valid(event);
    assert_pre_spawn_launch_exit_fields(event, reason_fragment);
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

fn channel_data_base64_values<'a>(events: &'a [Value], channel: &str) -> Vec<&'a str> {
    channel_events(events, channel)
        .into_iter()
        .map(channel_event_data_base64)
        .collect()
}

fn channel_events<'a>(events: &'a [Value], channel: &str) -> Vec<&'a Value> {
    events
        .iter()
        .filter(|event| event["kind"] == channel)
        .collect()
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

fn invoke_launch_with_timeout(
    envelope: &Value,
    timeout: Duration,
) -> Result<support::invoke::Invocation, String> {
    let mut child = spawn_launch_provider();
    write_child_stdin(&mut child, envelope.to_string().as_bytes());
    wait_for_launch_provider(child, timeout)
}

fn spawn_launch_provider() -> Child {
    Command::new(env!("CARGO_BIN_EXE_agent-runner-claude"))
        .arg("launch")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn launch provider")
}

fn write_child_stdin(child: &mut Child, stdin: &[u8]) {
    let mut pipe = child.stdin.take().expect("launch provider stdin");
    pipe.write_all(stdin).expect("write launch provider stdin");
}

fn wait_for_launch_provider(
    mut child: Child,
    timeout: Duration,
) -> Result<support::invoke::Invocation, String> {
    let started = Instant::now();
    loop {
        if let Some(status) = child.try_wait().expect("poll launch provider") {
            return Ok(invocation_from_exited_child(child, status));
        }
        if launch_provider_timed_out(started, timeout) {
            stop_timed_out_provider(&mut child);
            return Err(launch_provider_timeout_message(timeout));
        }
        thread::sleep(Duration::from_millis(10));
    }
}

fn launch_provider_timed_out(started: Instant, timeout: Duration) -> bool {
    started.elapsed() >= timeout
}

fn launch_provider_timeout_message(timeout: Duration) -> String {
    format!("launch provider did not exit within {timeout:?}")
}

fn invocation_from_exited_child(
    mut child: Child,
    status: ExitStatus,
) -> support::invoke::Invocation {
    support::invoke::Invocation {
        stdout: read_child_pipe(child.stdout.take()),
        stderr: read_child_pipe(child.stderr.take()),
        code: status.code(),
    }
}

fn read_child_pipe<R: Read>(pipe: Option<R>) -> Vec<u8> {
    let mut bytes = Vec::new();
    if let Some(mut pipe) = pipe {
        pipe.read_to_end(&mut bytes).expect("read child pipe");
    }
    bytes
}

fn stop_timed_out_provider(child: &mut Child) {
    let _ = child.kill();
    let _ = child.wait();
}

fn cleanup_grandchild(pid_file: &PathBuf) {
    if let Some(pid) = pid_file_pid(pid_file) {
        terminate_pid(pid);
    }
}

fn pid_file_pid(pid_file: &PathBuf) -> Option<u32> {
    fs::read_to_string(pid_file)
        .ok()
        .and_then(|pid| pid.trim().parse::<u32>().ok())
}

fn terminate_pid(pid: u32) {
    let _ = Command::new("kill")
        .arg("-TERM")
        .arg(pid.to_string())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

#[test]
fn launch_reports_signal_exit_event_and_provider_exits_zero() {
    let roots = temp_roots("launch-signal-exit");
    let script = signal_exit_fixture(&roots);

    let request = launch_request(&roots, vec![path_string(&script)], json!({}));
    let output = invoke("launch", &request);
    assert_signal_exit_invocation(output);
}

fn signal_exit_fixture(roots: &support::fixtures::TempRoots) -> PathBuf {
    let script = signal_exit_fixture_path(roots);
    publish_signal_exit_fixture(&script);
    script
}

fn signal_exit_fixture_path(roots: &support::fixtures::TempRoots) -> PathBuf {
    roots.root.join("signal-child.sh")
}

fn publish_signal_exit_fixture(script: &std::path::Path) {
    write_executable(script, "#!/bin/sh\nkill -TERM $$\n");
}

fn assert_signal_exit_invocation(output: support::invoke::Invocation) {
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
    let fixture = non_executable_fixture(&roots);

    let request = launch_request(&roots, vec![path_string(&fixture.script)], json!({}));
    let output = invoke("launch", &request);
    assert_spawn_error_invocation(output, &fixture.marker);
}

struct NonExecutableFixture {
    script: PathBuf,
    marker: PathBuf,
}

fn non_executable_fixture(roots: &support::fixtures::TempRoots) -> NonExecutableFixture {
    let fixture = non_executable_fixture_record(roots);
    install_non_executable_fixture(&fixture);
    fixture
}

fn non_executable_fixture_record(roots: &support::fixtures::TempRoots) -> NonExecutableFixture {
    NonExecutableFixture {
        script: roots.root.join("not-executable.sh"),
        marker: roots.root.join("non-executable-ran"),
    }
}

fn install_non_executable_fixture(fixture: &NonExecutableFixture) {
    fs::write(&fixture.script, non_executable_script(&fixture.marker))
        .expect("write non-executable fixture");
    make_non_executable(&fixture.script);
}

fn non_executable_script(marker: &std::path::Path) -> String {
    format!("#!/bin/sh\nprintf ran > '{}'\n", marker.display())
}

fn make_non_executable(script: &std::path::Path) {
    #[cfg(unix)]
    {
        let mut permissions = fs::metadata(script).expect("script metadata").permissions();
        permissions.set_mode(0o644);
        fs::set_permissions(script, permissions).expect("chmod non-executable fixture");
    }
}

fn assert_spawn_error_invocation(output: support::invoke::Invocation, marker: &std::path::Path) {
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
fn launch_emits_exit_when_descendant_holds_stdio_after_child_exits() {
    let roots = temp_roots("launch-descendant-stdio");
    let fixture = descendant_stdio_fixture(&roots);

    let request = launch_request(&roots, vec![path_string(&fixture.script)], json!({}));
    let output = invoke_launch_with_timeout(&request, Duration::from_secs(2));
    cleanup_grandchild(&fixture.pid_file);
    let output = output.expect("launch provider exits promptly after direct child exits");
    assert_descendant_stdio_output(output);
}

struct DescendantStdioFixture {
    script: PathBuf,
    pid_file: PathBuf,
}

fn descendant_stdio_fixture(roots: &support::fixtures::TempRoots) -> DescendantStdioFixture {
    let fixture = descendant_stdio_fixture_record(roots);
    install_descendant_stdio_fixture(&fixture);
    fixture
}

fn descendant_stdio_fixture_record(roots: &support::fixtures::TempRoots) -> DescendantStdioFixture {
    DescendantStdioFixture {
        script: roots.root.join("child.sh"),
        pid_file: roots.root.join("descendant.pid"),
    }
}

fn install_descendant_stdio_fixture(fixture: &DescendantStdioFixture) {
    write_executable(&fixture.script, &descendant_stdio_script(&fixture.pid_file));
}

fn descendant_stdio_script(pid_file: &std::path::Path) -> String {
    format!(
        "#!/bin/sh\n(sleep 30) &\necho $! > '{}'\nexit 0\n",
        pid_file.display()
    )
}

fn assert_descendant_stdio_output(output: support::invoke::Invocation) {
    assert_eq!(output.code, Some(0));
    assert!(output.stderr.is_empty());
    let events = collect_launch_jsonl_lines(&output);
    for event in &events {
        assert_launch_event_valid(event);
    }
    assert_seq_starts_at_one_and_monotonic(&events);
    let final_event = events.last().expect("launch stream has final event");
    assert_eq!(final_event["kind"], "exit");
    assert_eq!(
        final_event["status"],
        json!({ "kind": "exited", "code": 0 })
    );
    assert_eq!(final_event["terminal_signal"]["kind"], "clean_exit");
}

#[test]
fn launch_rejects_invalid_stdin_bytepayload_before_spawning_child() {
    let roots = temp_roots("launch-invalid-base64");
    let fixture = spawn_marker_fixture(&roots);

    let request = invalid_base64_stdin_request(&roots, &fixture.script);
    let output = invoke("launch", &request);
    assert_invalid_stdin_pre_spawn(&output, &fixture.marker, "invalid launch stdin");
}

#[test]
fn launch_rejects_invalid_utf8_stdin_payload_before_spawning_child() {
    let roots = temp_roots("launch-invalid-utf8");
    let fixture = spawn_marker_fixture(&roots);

    let request_text = invalid_utf8_stdin_request_text(&roots, &fixture.script);
    let output = invoke_with_stdin_bytes(Some("launch"), request_text.as_bytes());
    assert_invalid_stdin_pre_spawn(&output, &fixture.marker, "request envelope");
}

struct SpawnMarkerFixture {
    script: PathBuf,
    marker: PathBuf,
}

fn spawn_marker_fixture(roots: &support::fixtures::TempRoots) -> SpawnMarkerFixture {
    let fixture = spawn_marker_fixture_record(roots);
    publish_spawn_marker_fixture(&fixture);
    fixture
}

fn spawn_marker_fixture_record(roots: &support::fixtures::TempRoots) -> SpawnMarkerFixture {
    SpawnMarkerFixture {
        script: roots.root.join("child.sh"),
        marker: roots.root.join("child-spawned"),
    }
}

fn publish_spawn_marker_fixture(fixture: &SpawnMarkerFixture) {
    write_executable(&fixture.script, &spawn_marker_script(&fixture.marker));
}

fn spawn_marker_script(marker: &std::path::Path) -> String {
    format!("#!/bin/sh\nprintf spawned > '{}'\n", marker.display())
}

fn invalid_base64_stdin_request(
    roots: &support::fixtures::TempRoots,
    script: &std::path::Path,
) -> Value {
    launch_request(
        roots,
        vec![path_string(script)],
        json!({ "stdin": { "encoding": "base64", "data": "not@@base64" } }),
    )
}

fn invalid_utf8_stdin_request_text(
    roots: &support::fixtures::TempRoots,
    script: &std::path::Path,
) -> String {
    invalid_utf8_stdin_request(roots, script)
        .to_string()
        .replace("\"data\":\"\"", "\"data\":\"\\uD800\"")
}

fn invalid_utf8_stdin_request(
    roots: &support::fixtures::TempRoots,
    script: &std::path::Path,
) -> Value {
    launch_request(
        roots,
        vec![path_string(script)],
        json!({ "stdin": { "encoding": "utf8", "data": "" } }),
    )
}

fn assert_invalid_stdin_pre_spawn(
    output: &support::invoke::Invocation,
    marker: &std::path::Path,
    reason_fragment: &str,
) {
    assert!(
        !marker.exists(),
        "child must not spawn when stdin payload is invalid"
    );
    assert_single_pre_spawn_launch_exit(output, reason_fragment);
}

#[test]
fn launch_propagates_cwd_env_stdin_and_streams_byte_exact_events() {
    let roots = temp_roots("launch-propagation");
    let fixture = propagation_fixture(&roots);

    let stdin_bytes = b"stdin\0\xffpayload";
    let request = propagation_request(&roots, &fixture.script, stdin_bytes);
    let output = invoke("launch", &request);
    assert_propagation_invocation(output, &roots, &fixture, stdin_bytes);
}

struct PropagationFixture {
    script: PathBuf,
    stdin_capture: PathBuf,
    pwd_capture: PathBuf,
    env_capture: PathBuf,
}

fn propagation_fixture(roots: &support::fixtures::TempRoots) -> PropagationFixture {
    let fixture = propagation_fixture_record(roots);
    install_propagation_fixture(&fixture);
    fixture
}

fn propagation_fixture_record(roots: &support::fixtures::TempRoots) -> PropagationFixture {
    PropagationFixture {
        script: roots.root.join("child.sh"),
        stdin_capture: roots.root.join("stdin.bin"),
        pwd_capture: roots.root.join("pwd.txt"),
        env_capture: roots.root.join("env.txt"),
    }
}

fn install_propagation_fixture(fixture: &PropagationFixture) {
    write_executable(&fixture.script, &propagation_script(fixture));
}

fn propagation_script(fixture: &PropagationFixture) -> String {
    format!(
        "#!/bin/sh\npwd > '{}'\nprintf '%s' \"$CONTRACT_LAUNCH_TEST_ENV\" > '{}'\ndd of='{}' bs=1 status=none\nprintf 'out\\000\\377A\\n'\nprintf 'err\\001\\376Z\\n' >&2\nexit 7\n",
        fixture.pwd_capture.display(),
        fixture.env_capture.display(),
        fixture.stdin_capture.display()
    )
}

fn propagation_request(
    roots: &support::fixtures::TempRoots,
    script: &std::path::Path,
    stdin_bytes: &[u8],
) -> Value {
    launch_request(
        roots,
        vec![path_string(script)],
        json!({
            "env": { "CONTRACT_LAUNCH_TEST_ENV": "propagated-value" },
            "stdin": { "encoding": "base64", "data": encode_base64(stdin_bytes) }
        }),
    )
}

fn assert_propagation_invocation(
    output: support::invoke::Invocation,
    roots: &support::fixtures::TempRoots,
    fixture: &PropagationFixture,
    stdin_bytes: &[u8],
) {
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
    assert_eq!(captured_bytes(&fixture.stdin_capture), stdin_bytes);
    assert_eq!(
        captured_text(&fixture.pwd_capture).trim(),
        path_string(&roots.root)
    );
    assert_eq!(captured_text(&fixture.env_capture), "propagated-value");
    assert_eq!(
        channel_data_base64_values(&events, "stdout"),
        ["b3V0AP9BCg=="]
    );
    assert_eq!(
        channel_data_base64_values(&events, "stderr"),
        ["ZXJyAf5aCg=="]
    );
}

fn captured_bytes(path: &std::path::Path) -> Vec<u8> {
    fs::read(path).expect("captured bytes")
}

fn captured_text(path: &std::path::Path) -> String {
    fs::read_to_string(path).expect("captured text")
}

#[test]
fn launch_timeout_cancels_process_group_without_orphaning_grandchild() {
    let roots = temp_roots("launch-timeout-pgrp");
    let fixture = timeout_fixture(&roots);

    let request = timeout_request(&roots, &fixture.script);

    let output = invoke("launch", &request);
    assert_timeout_invocation(output);
    assert_grandchild_terminated(&fixture.pid_file);
}

struct TimeoutFixture {
    script: PathBuf,
    pid_file: PathBuf,
}

fn timeout_fixture(roots: &support::fixtures::TempRoots) -> TimeoutFixture {
    let fixture = timeout_fixture_record(roots);
    setup_timeout_fixture(&fixture);
    fixture
}

fn timeout_fixture_record(roots: &support::fixtures::TempRoots) -> TimeoutFixture {
    TimeoutFixture {
        script: roots.root.join("child.sh"),
        pid_file: roots.root.join("grandchild.pid"),
    }
}

fn setup_timeout_fixture(fixture: &TimeoutFixture) {
    write_executable(&fixture.script, &timeout_script(&fixture.pid_file));
}

fn timeout_script(pid_file: &std::path::Path) -> String {
    format!(
        "#!/bin/sh\n(sleep 30) &\necho $! > '{}'\nwhile :; do sleep 1; done\n",
        pid_file.display()
    )
}

fn timeout_request(roots: &support::fixtures::TempRoots, script: &std::path::Path) -> Value {
    launch_timeout_request(roots, now_unix_ms() + 500, vec![path_string(script)])
}

fn assert_timeout_invocation(output: support::invoke::Invocation) {
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
}

fn assert_grandchild_terminated(pid_file: &std::path::Path) {
    let grandchild_pid = grandchild_pid(pid_file);
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

fn grandchild_pid(pid_file: &std::path::Path) -> u32 {
    fs::read_to_string(pid_file)
        .expect("grandchild pid must be captured before timeout")
        .trim()
        .parse::<u32>()
        .expect("numeric grandchild pid")
}
