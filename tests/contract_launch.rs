// declared_role: orchestration, validator, accessor, parser, formatter, predicate
// intrinsic_surface_declarations:
//   - component: tests/contract_launch.rs
//     role: intrinsic-surface
//     Domain: contract_launch_proof_surface
//     Owns:
//       - launch contract and byte-preservation scenarios
//       - support harness dependencies for launch invoke/schema/script proof

mod support;

use agent_runner_claude::encoding::{encode_base64, sha256_hex};
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

const LAUNCH_HEARTBEAT_INTERVAL_MS_ENV: &str = "AGENT_RUNNER_CLAUDE_LAUNCH_HEARTBEAT_INTERVAL_MS";

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
    launch_provider_command()
        .spawn()
        .expect("spawn launch provider")
}

// declared_role: orchestration
fn spawn_launch_provider_with_heartbeat_interval(interval: Duration) -> Child {
    let mut command = launch_provider_command();
    command.env(
        LAUNCH_HEARTBEAT_INTERVAL_MS_ENV,
        interval.as_millis().to_string(),
    );
    command.spawn().expect("spawn launch provider")
}

// declared_role: formatter
fn launch_provider_command() -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_agent-runner-claude"));
    command
        .arg("launch")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    command
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

// declared_role: orchestration
fn invoke_launch_with_heartbeat_interval(
    envelope: &Value,
    heartbeat_interval: Duration,
    timeout: Duration,
) -> Result<support::invoke::Invocation, String> {
    let mut child = spawn_launch_provider_with_heartbeat_interval(heartbeat_interval);
    write_child_stdin(&mut child, envelope.to_string().as_bytes());
    wait_for_launch_provider(child, timeout)
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

struct ArgvCaptureFixture {
    script: PathBuf,
    argv_path: PathBuf,
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

fn argv_capture_stdout_session_fixture(roots: &support::fixtures::TempRoots) -> ArgvCaptureFixture {
    let fixture = ArgvCaptureFixture {
        script: roots.root.join("argv-capture-child.sh"),
        argv_path: roots.root.join("argv.txt"),
    };
    write_executable(
        &fixture.script,
        &argv_capture_stdout_session_script(&fixture.argv_path),
    );
    fixture
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

fn argv_capture_stdout_session_script(argv_path: &std::path::Path) -> String {
    format!(
        "#!/bin/sh\n: > '{}'\nfor arg in \"$@\"; do printf '%s\n' \"$arg\" >> '{}'; done\n{}exit 0\n",
        argv_path.display(),
        argv_path.display(),
        stdout_session_line_script("child-reported-session")
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

// declared_role: validator
#[test]
fn launch_emits_periodic_heartbeats_while_child_is_silent() {
    let roots = temp_roots("launch-periodic-heartbeat");
    let script = silent_child_fixture(&roots);
    let heartbeat_interval = Duration::from_millis(200);

    let request = launch_request(&roots, vec![path_string(&script)], json!({}));
    let output =
        invoke_launch_with_heartbeat_interval(&request, heartbeat_interval, Duration::from_secs(5))
            .expect("silent launch provider exits after emitting periodic heartbeats");
    assert_periodic_heartbeat_invocation(output, Duration::from_millis(500));
}

// declared_role: orchestration
fn silent_child_fixture(roots: &support::fixtures::TempRoots) -> PathBuf {
    let script = roots.root.join("silent-child.sh");
    write_executable(&script, "#!/bin/sh\nsleep 1\nexit 0\n");
    script
}

// declared_role: validator
fn assert_periodic_heartbeat_invocation(
    output: support::invoke::Invocation,
    max_event_gap: Duration,
) {
    assert_eq!(output.code, Some(0));
    assert!(output.stderr.is_empty());
    let events = collect_launch_jsonl_lines(&output);
    for event in &events {
        assert_launch_event_valid(event);
    }
    assert_seq_starts_at_one_and_monotonic(&events);
    assert_single_terminal_exit(&events);
    assert_alive_heartbeats_emitted(&events, 3);
    assert_adjacent_event_gaps_under(&events, max_event_gap);
}

// declared_role: validator
fn assert_single_terminal_exit(events: &[Value]) {
    assert_eq!(
        events
            .iter()
            .filter(|event| event["kind"] == "exit")
            .count(),
        1
    );
    assert_eq!(
        events.last().expect("launch stream final event")["kind"],
        "exit"
    );
}

// declared_role: validator
fn assert_alive_heartbeats_emitted(events: &[Value], minimum: usize) {
    let alive_heartbeats = events
        .iter()
        .filter(|event| event["kind"] == "heartbeat")
        .filter(|event| event["detail"] == "alive")
        .count();
    assert!(
        alive_heartbeats >= minimum,
        "expected at least {minimum} periodic heartbeats: {events:?}"
    );
}

// declared_role: validator
fn assert_adjacent_event_gaps_under(events: &[Value], max_gap: Duration) {
    let max_gap_ms = max_gap.as_millis() as u64;
    for pair in events.windows(2) {
        let before = event_time_unix_ms(&pair[0]);
        let after = event_time_unix_ms(&pair[1]);
        assert!(
            after.saturating_sub(before) <= max_gap_ms,
            "adjacent launch events exceeded {max_gap:?}: {events:?}"
        );
    }
}

// declared_role: accessor
fn event_time_unix_ms(event: &Value) -> u64 {
    event["time_unix_ms"]
        .as_u64()
        .expect("launch event timestamp")
}

#[test]
fn launch_clean_resume_emits_submitted_user_turn_marker() {
    let roots = temp_roots("launch-submitted-user-turn-clean-resume");
    let script = stdin_sink_fixture(&roots, 0);
    let prompt =
        "Notifications delivered:\n[OULIPOLY-DELIVERY 5169694d-de0f-40d1-890c-6e28e55bab27]\n";

    let request = resume_stdin_request(&roots, &script, prompt, Some("claude-session-123"));
    let output = invoke("launch", &request);
    assert_clean_resume_submitted_user_turn_marker(output, prompt);
}

#[test]
fn launch_non_resume_emits_no_submitted_user_turn_marker() {
    let roots = temp_roots("launch-submitted-user-turn-non-resume");
    let script = stdin_sink_fixture(&roots, 0);
    let prompt = "Notifications delivered:\n[OULIPOLY-DELIVERY nonce-non-resume]\n";

    let request = resume_stdin_request(&roots, &script, prompt, None);
    let output = invoke("launch", &request);
    assert_no_submitted_user_turn_marker(output);
}

#[test]
fn launch_non_clean_resume_emits_no_submitted_user_turn_marker() {
    let roots = temp_roots("launch-submitted-user-turn-non-clean");
    let script = stdin_sink_fixture(&roots, 7);
    let prompt = "Notifications delivered:\n[OULIPOLY-DELIVERY nonce-non-clean]\n";

    let request = resume_stdin_request(&roots, &script, prompt, Some("claude-session-123"));
    let output = invoke("launch", &request);
    assert_no_submitted_user_turn_marker(output);
}

#[test]
fn launch_fresh_exit_reports_stdout_provider_session_id() {
    let roots = temp_roots("launch-fresh-provider-session-id");
    let script = stdout_session_fixture(&roots, "claude-fresh-session-123");

    let request = launch_request(&roots, vec![path_string(&script)], json!({}));
    let output = invoke("launch", &request);
    let events = assert_valid_launch_invocation(output);
    assert_exit_provider_session_id(&events, "claude-fresh-session-123");
}

#[test]
fn launch_resume_exit_reports_known_provider_session_id() {
    let roots = temp_roots("launch-resume-provider-session-id");
    let script = stdout_session_stdin_sink_fixture(&roots, "stdout-should-not-win");
    let prompt = "Notifications delivered:\n[OULIPOLY-DELIVERY nonce-resume-session]\n";

    let request = resume_stdin_request(&roots, &script, prompt, Some("known-resume-session-456"));
    let output = invoke("launch", &request);
    let events = assert_valid_launch_invocation(output);
    assert_exit_provider_session_id(&events, "known-resume-session-456");
}

#[test]
fn launch_resume_injects_resume_arg_before_prompt_arg() {
    let roots = temp_roots("launch-resume-argv-insert");
    let fixture = argv_capture_stdout_session_fixture(&roots);
    let prompt = "resume prompt payload";

    let request = prompt_arg_request(
        &roots,
        &fixture.script,
        &["-p", prompt],
        prompt,
        Some("known-resume-session-789"),
    );
    let output = invoke("launch", &request);
    let events = assert_valid_launch_invocation(output);

    assert_exit_provider_session_id(&events, "known-resume-session-789");
    assert_eq!(
        captured_argv(&fixture.argv_path),
        ["-p", "--resume", "known-resume-session-789", prompt]
    );
}

#[test]
fn launch_create_injects_session_id_arg_before_prompt_arg() {
    let roots = temp_roots("launch-create-argv-insert");
    let fixture = argv_capture_stdout_session_fixture(&roots);
    let prompt = "create prompt payload";

    let request = prompt_arg_request_with_session(
        &roots,
        &fixture.script,
        &["-p", prompt],
        prompt,
        Some(("known-create-session-123", Some("create"))),
    );
    let output = invoke("launch", &request);
    let events = assert_valid_launch_invocation(output);

    assert_exit_provider_session_id(&events, "known-create-session-123");
    assert_eq!(
        captured_argv(&fixture.argv_path),
        ["-p", "--session-id", "known-create-session-123", prompt]
    );
}

#[test]
fn launch_fresh_does_not_inject_resume_arg() {
    let roots = temp_roots("launch-fresh-no-resume-argv");
    let fixture = argv_capture_stdout_session_fixture(&roots);
    let prompt = "fresh prompt payload";

    let request = prompt_arg_request(&roots, &fixture.script, &["-p", prompt], prompt, None);
    let output = invoke("launch", &request);
    assert_valid_launch_invocation(output);

    assert_eq!(captured_argv(&fixture.argv_path), ["-p", prompt]);
}

#[test]
fn launch_resume_replaces_existing_resume_arg_without_duplicate() {
    let roots = temp_roots("launch-resume-argv-replace");
    let fixture = argv_capture_stdout_session_fixture(&roots);
    let prompt = "replacement prompt payload";

    let request = prompt_arg_request(
        &roots,
        &fixture.script,
        &[
            "-p",
            "--resume",
            "stale-session",
            "--resume",
            "duplicate-session",
            prompt,
        ],
        prompt,
        Some("known-resume-session-abc"),
    );
    let output = invoke("launch", &request);
    let events = assert_valid_launch_invocation(output);

    assert_exit_provider_session_id(&events, "known-resume-session-abc");
    assert_eq!(
        captured_argv(&fixture.argv_path),
        ["-p", "--resume", "known-resume-session-abc", prompt]
    );
}

#[test]
fn launch_create_replaces_existing_session_id_arg_without_duplicate() {
    let roots = temp_roots("launch-create-argv-replace");
    let fixture = argv_capture_stdout_session_fixture(&roots);
    let prompt = "create replacement prompt payload";

    let request = prompt_arg_request_with_session(
        &roots,
        &fixture.script,
        &[
            "-p",
            "--session-id",
            "stale-session",
            "--session-id",
            "duplicate-session",
            prompt,
        ],
        prompt,
        Some(("known-create-session-abc", Some("create"))),
    );
    let output = invoke("launch", &request);
    let events = assert_valid_launch_invocation(output);

    assert_exit_provider_session_id(&events, "known-create-session-abc");
    assert_eq!(
        captured_argv(&fixture.argv_path),
        ["-p", "--session-id", "known-create-session-abc", prompt]
    );
}

#[test]
fn launch_resume_removes_existing_session_id_arg_when_switching_modes() {
    let roots = temp_roots("launch-resume-removes-session-id");
    let fixture = argv_capture_stdout_session_fixture(&roots);
    let prompt = "resume switch prompt payload";

    let request = prompt_arg_request(
        &roots,
        &fixture.script,
        &["-p", "--session-id", "stale-session", prompt],
        prompt,
        Some("known-resume-session-switch"),
    );
    let output = invoke("launch", &request);
    let events = assert_valid_launch_invocation(output);

    assert_exit_provider_session_id(&events, "known-resume-session-switch");
    assert_eq!(
        captured_argv(&fixture.argv_path),
        ["-p", "--resume", "known-resume-session-switch", prompt]
    );
}

#[test]
fn launch_create_removes_existing_resume_arg_when_switching_modes() {
    let roots = temp_roots("launch-create-removes-resume");
    let fixture = argv_capture_stdout_session_fixture(&roots);
    let prompt = "create switch prompt payload";

    let request = prompt_arg_request_with_session(
        &roots,
        &fixture.script,
        &["-p", "--resume", "stale-session", prompt],
        prompt,
        Some(("known-create-session-switch", Some("create"))),
    );
    let output = invoke("launch", &request);
    let events = assert_valid_launch_invocation(output);

    assert_exit_provider_session_id(&events, "known-create-session-switch");
    assert_eq!(
        captured_argv(&fixture.argv_path),
        ["-p", "--session-id", "known-create-session-switch", prompt]
    );
}

#[test]
fn launch_known_session_without_start_mode_injects_no_session_flag_and_uses_stdout_session() {
    let roots = temp_roots("launch-session-missing-start-mode");
    let fixture = argv_capture_stdout_session_fixture(&roots);
    let prompt = "missing start mode prompt payload";

    let request = prompt_arg_request_with_session(
        &roots,
        &fixture.script,
        &["-p", prompt],
        prompt,
        Some(("known-session-without-mode", None)),
    );
    let output = invoke("launch", &request);
    let events = assert_valid_launch_invocation(output);

    assert_exit_provider_session_id(&events, "child-reported-session");
    assert_eq!(captured_argv(&fixture.argv_path), ["-p", prompt]);
}

#[test]
fn launch_known_session_with_unknown_start_mode_injects_no_session_flag_and_uses_stdout_session() {
    let roots = temp_roots("launch-session-unknown-start-mode");
    let fixture = argv_capture_stdout_session_fixture(&roots);
    let prompt = "unknown start mode prompt payload";

    let request = prompt_arg_request_with_session(
        &roots,
        &fixture.script,
        &["-p", prompt],
        prompt,
        Some(("known-session-unknown-mode", Some("bogus"))),
    );
    let output = invoke("launch", &request);
    let events = assert_valid_launch_invocation(output);

    assert_exit_provider_session_id(&events, "child-reported-session");
    assert_eq!(captured_argv(&fixture.argv_path), ["-p", prompt]);
}

#[test]
fn launch_exit_omits_session_when_no_provider_session_id_resolves() {
    let roots = temp_roots("launch-no-provider-session-id");
    let script = no_session_stdout_fixture(&roots);

    let request = launch_request(&roots, vec![path_string(&script)], json!({}));
    let output = invoke("launch", &request);
    let events = assert_valid_launch_invocation(output);
    assert_exit_has_no_session(&events);
}

// declared_role: orchestration
fn stdin_sink_fixture(roots: &support::fixtures::TempRoots, exit_code: i32) -> PathBuf {
    let script = roots.root.join(format!("stdin-sink-{exit_code}.sh"));
    write_executable(&script, &stdin_sink_script(exit_code));
    script
}

// declared_role: orchestration
fn stdout_session_fixture(roots: &support::fixtures::TempRoots, session_id: &str) -> PathBuf {
    let script = roots.root.join("stdout-session.sh");
    write_executable(&script, &stdout_session_script(session_id));
    script
}

// declared_role: orchestration
fn stdout_session_stdin_sink_fixture(
    roots: &support::fixtures::TempRoots,
    session_id: &str,
) -> PathBuf {
    let script = roots.root.join("stdout-session-stdin-sink.sh");
    write_executable(&script, &stdout_session_stdin_sink_script(session_id));
    script
}

// declared_role: orchestration
fn no_session_stdout_fixture(roots: &support::fixtures::TempRoots) -> PathBuf {
    let script = roots.root.join("no-session-stdout.sh");
    write_executable(&script, no_session_stdout_script());
    script
}

// declared_role: formatter
fn stdin_sink_script(exit_code: i32) -> String {
    format!("#!/bin/sh\n/bin/cat >/dev/null\nexit {exit_code}\n")
}

// declared_role: formatter
fn stdout_session_script(session_id: &str) -> String {
    format!(
        "#!/bin/sh\n{}exit 0\n",
        stdout_session_line_script(session_id)
    )
}

// declared_role: formatter
fn stdout_session_stdin_sink_script(session_id: &str) -> String {
    format!(
        "#!/bin/sh\n{}/bin/cat >/dev/null\nexit 0\n",
        stdout_session_line_script(session_id)
    )
}

// declared_role: formatter
fn stdout_session_line_script(session_id: &str) -> String {
    format!(
        "cat <<'JSON'\n{{\"type\":\"system\",\"subtype\":\"init\",\"session_id\":\"{session_id}\"}}\nJSON\n"
    )
}

// declared_role: formatter
fn no_session_stdout_script() -> &'static str {
    "#!/bin/sh\nprintf 'not a claude session event\\n'\nexit 0\n"
}

// declared_role: formatter
fn resume_stdin_request(
    roots: &support::fixtures::TempRoots,
    script: &std::path::Path,
    prompt: &str,
    session_id: Option<&str>,
) -> Value {
    let mut extra = json!({ "stdin": { "encoding": "utf8", "data": prompt } });
    if let Some(session_id) = session_id {
        extra["session"] = json!({
            "known_provider_session_id": session_id,
            "start_mode": "resume"
        });
    }
    launch_request(roots, vec![path_string(script)], extra)
}

fn prompt_arg_request(
    roots: &support::fixtures::TempRoots,
    script: &std::path::Path,
    child_args: &[&str],
    prompt: &str,
    session_id: Option<&str>,
) -> Value {
    prompt_arg_request_with_session(
        roots,
        script,
        child_args,
        prompt,
        session_id.map(|session_id| (session_id, Some("resume"))),
    )
}

fn prompt_arg_request_with_session(
    roots: &support::fixtures::TempRoots,
    script: &std::path::Path,
    child_args: &[&str],
    prompt: &str,
    session: Option<(&str, Option<&str>)>,
) -> Value {
    let mut argv = vec![path_string(script)];
    argv.extend(child_args.iter().map(|arg| (*arg).to_string()));
    let mut extra = json!({
        "model": {
            "name": "claude-sonnet",
            "provider_args": [],
            "inputs": { "prompt": prompt, "named": {} }
        }
    });
    if let Some((session_id, start_mode)) = session {
        extra["session"] = json!({ "known_provider_session_id": session_id });
        if let Some(start_mode) = start_mode {
            extra["session"]["start_mode"] = json!(start_mode);
        }
    }
    launch_request(roots, argv, extra)
}

// declared_role: validator
fn assert_clean_resume_submitted_user_turn_marker(
    output: support::invoke::Invocation,
    prompt: &str,
) {
    let events = assert_valid_launch_invocation(output);
    let markers = submitted_user_turn_markers(&events);
    assert_eq!(
        markers.len(),
        1,
        "expected exactly one submitted marker: {events:?}"
    );
    let marker = markers[0];
    assert_eq!(marker["value"]["provider_session_id"], "claude-session-123");
    assert_eq!(
        marker["value"]["prompt_sha256"],
        sha256_hex(prompt.as_bytes())
    );
    assert_eq!(marker["value"]["source"], "claudecode.launch");
    assert_eq!(
        marker["value"]["delivery_nonce"],
        "5169694d-de0f-40d1-890c-6e28e55bab27"
    );
    assert!(
        marker["value"].get("message_id").is_none(),
        "claude submitted marker must omit message_id: {marker}"
    );
    assert_eq!(events.last().unwrap()["kind"], "exit");
}

// declared_role: validator
fn assert_no_submitted_user_turn_marker(output: support::invoke::Invocation) {
    let events = assert_valid_launch_invocation(output);
    let markers = submitted_user_turn_markers(&events);
    assert!(
        markers.is_empty(),
        "submitted marker must not be emitted: {events:?}"
    );
}

// declared_role: validator
fn assert_exit_provider_session_id(events: &[Value], expected_session_id: &str) {
    let event = events.last().expect("launch stream has final event");
    assert_eq!(event["kind"], "exit");
    assert_eq!(
        event["session"]["provider_session_id"], expected_session_id,
        "exit event must report provider session id: {event}"
    );
}

// declared_role: validator
fn assert_exit_has_no_session(events: &[Value]) {
    let event = events.last().expect("launch stream has final event");
    assert_eq!(event["kind"], "exit");
    assert!(
        event.get("session").is_none(),
        "exit event must omit session when no id resolves: {event}"
    );
}

// declared_role: validator
fn assert_valid_launch_invocation(output: support::invoke::Invocation) -> Vec<Value> {
    assert_eq!(output.code, Some(0));
    assert!(output.stderr.is_empty());
    let events = collect_launch_jsonl_lines(&output);
    for event in &events {
        assert_launch_event_valid(event);
    }
    assert_seq_starts_at_one_and_monotonic(&events);
    events
}

// declared_role: accessor
fn submitted_user_turn_markers(events: &[Value]) -> Vec<&Value> {
    events
        .iter()
        .filter(|event| event["kind"] == "marker")
        .filter(|event| event["name"] == "oulipoly.submitted_user_turn")
        .collect()
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

fn captured_argv(path: &std::path::Path) -> Vec<String> {
    captured_text(path).lines().map(str::to_string).collect()
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
