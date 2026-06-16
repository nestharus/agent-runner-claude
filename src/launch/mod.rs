// declared_role: orchestration, validator, predicate, mapper, accessor, formatter, filter
// intrinsic_surface_declarations:
//   - component: src/launch/mod.rs
//     role: intrinsic-surface
//     Domain: provider child launch and contract-event streaming
//     Owns:
//       - launch capability submodule declaration set
//       - launch request envelope decode plus params/env/argv/stdin extraction
//       - provider child process spawn, lifecycle, and descendant termination
//       - child stdout/stderr drain streaming over mpsc channels and worker threads
//       - launch contract event output (started/heartbeat/data/marker/exit) to process stdout
//       - deadline, heartbeat, and drain-grace timing
//       - terminal status to terminal-signal mapping plus spawn and pre-spawn error formatting

pub mod child;
pub mod drain;
pub mod events;
pub mod params;
pub mod session_marker;
pub mod stdin;
pub mod submitted_user_turn;

use serde_json::json;
use serde_json::Value;
use std::collections::BTreeMap;
use std::io::{self, Write};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use crate::envelope::decode::RequestEnvelope;
use crate::envelope::error::{ErrorCategory, ProviderFailure};

const DRAIN_POLL_INTERVAL: Duration = Duration::from_millis(10);
const FINAL_DRAIN_GRACE: Duration = Duration::from_millis(100);
const STDOUT_SESSION_ID_PREFIX_CAP: usize = 256 * 1024;
const CLAUDE_RESUME_FLAG: &str = "--resume";
const CLAUDE_SESSION_ID_FLAG: &str = "--session-id";

#[derive(Clone, Copy)]
enum DrainStatus {
    Open,
    Disconnected,
}

#[derive(Default)]
struct StdoutPrefixAccumulator {
    bytes: Vec<u8>,
}

impl StdoutPrefixAccumulator {
    // declared_role: mapper
    fn record_stdout_prefix(&mut self, bytes: &[u8]) {
        if self.bytes.len() >= STDOUT_SESSION_ID_PREFIX_CAP {
            return;
        }
        let remaining = STDOUT_SESSION_ID_PREFIX_CAP - self.bytes.len();
        let len = bytes.len().min(remaining);
        self.bytes.extend_from_slice(&bytes[..len]);
    }

    // declared_role: accessor
    fn stdout(&self) -> &[u8] {
        &self.bytes
    }
}

pub fn handle(_subcommand: &str, request: &RequestEnvelope) -> Result<Value, ProviderFailure> {
    launch(request)
}

pub fn stream_rejected_request_and_exit(request_id: &str, reason: &str) -> ! {
    stream_pre_spawn_exit(request_id, reason)
}

fn launch(request: &RequestEnvelope) -> Result<Value, ProviderFailure> {
    let params = params::params_value(&request.params);
    if let Err(message) = validate_required_params(params) {
        stream_pre_spawn_exit(&request.request_id, &message);
    }

    let argv = match params::argv(params) {
        Ok(argv) => argv,
        Err(message) => stream_pre_spawn_exit(&request.request_id, &message),
    };
    let argv = session_argv(params, argv);
    let cwd = match params::required_string(params, "working_directory") {
        Ok(cwd) => cwd,
        Err(message) => stream_pre_spawn_exit(&request.request_id, &message),
    };
    let stdin_bytes = match launch_stdin_bytes(params) {
        Ok(bytes) => bytes,
        Err(failure) => stream_pre_spawn_exit(&request.request_id, &failure.message),
    };
    let resume_confirmation = submitted_user_turn::resume_confirmation(params, &argv, &stdin_bytes);
    let env = match launch_env(request, params) {
        Ok(env) => env,
        Err(failure) => stream_pre_spawn_exit(&request.request_id, &failure.message),
    };
    let deadline = deadline_unix_ms(request);

    stream_launch_and_exit(
        &request.request_id,
        &argv,
        cwd,
        &env,
        stdin_bytes,
        deadline,
        resume_confirmation,
    );
}

fn stream_pre_spawn_exit(request_id: &str, reason: &str) -> ! {
    let stdout = io::stdout();
    let mut events = events::EventWriter::new(stdout.lock(), request_id);
    let status = pre_spawn_rejection_status(reason);
    let signal = terminal_signal(&status);
    let _ = events.exit(status, signal);
    std::process::exit(0);
}

fn pre_spawn_rejection_status(reason: &str) -> Value {
    let error = io::Error::new(io::ErrorKind::InvalidInput, reason);
    spawn_error_status(&error)
}

fn validate_required_params(params: &Value) -> Result<(), String> {
    let object = params
        .as_object()
        .ok_or_else(|| "launch params must be an object".to_string())?;
    for key in object.keys() {
        if !matches!(
            key.as_str(),
            "settings_id"
                | "mode"
                | "model"
                | "argv"
                | "working_directory"
                | "env"
                | "stdin"
                | "session"
        ) {
            return Err(format!("unsupported launch param field: {key}"));
        }
    }
    params::required_string(params, "settings_id")?;
    params::required_string(params, "mode")?;
    params::required_string(params, "working_directory")?;
    if !params.get("model").is_some_and(Value::is_object) {
        return Err("launch params missing model".to_string());
    }
    let _ = params::argv(params)?;
    Ok(())
}

fn session_argv(params: &Value, mut argv: Vec<String>) -> Vec<String> {
    let Some(session_id) = params::known_provider_session_id(params) else {
        return argv;
    };
    let Some(flag) = claude_session_flag(params) else {
        // Defensive fallback for malformed callers: never guess create vs resume.
        // With no flag, Claude can self-select and stdout session capture remains authoritative.
        return argv;
    };
    remove_session_args(&mut argv, opposite_session_flag(flag));
    let insert_at = session_arg_insert_index(params, &argv);
    upsert_session_arg(&mut argv, flag, session_id, insert_at);
    argv
}

fn claude_session_flag(params: &Value) -> Option<&'static str> {
    match params::known_provider_session_start_mode(params) {
        Some("create") => Some(CLAUDE_SESSION_ID_FLAG),
        Some("resume") => Some(CLAUDE_RESUME_FLAG),
        _ => None,
    }
}

fn opposite_session_flag(flag: &str) -> &'static str {
    match flag {
        CLAUDE_SESSION_ID_FLAG => CLAUDE_RESUME_FLAG,
        CLAUDE_RESUME_FLAG => CLAUDE_SESSION_ID_FLAG,
        _ => unreachable!("known Claude session flag"),
    }
}

fn session_arg_insert_index(params: &Value, argv: &[String]) -> usize {
    params::prompt_input(params)
        .and_then(|prompt| argv.iter().position(|arg| arg == prompt))
        .unwrap_or(argv.len())
}

fn upsert_session_arg(argv: &mut Vec<String>, flag: &str, session_id: &str, insert_at: usize) {
    if let Some(index) = argv.iter().position(|arg| arg == flag) {
        set_existing_session_arg(argv, index, session_id);
        remove_duplicate_session_args(argv, flag, index);
    } else {
        insert_session_arg(argv, flag, insert_at, session_id);
    }
}

fn set_existing_session_arg(argv: &mut Vec<String>, index: usize, session_id: &str) {
    if index + 1 < argv.len() {
        argv[index + 1] = session_id.to_string();
    } else {
        argv.insert(index + 1, session_id.to_string());
    }
}

fn remove_duplicate_session_args(argv: &mut Vec<String>, flag: &str, keep_index: usize) {
    let mut index = keep_index + 2;
    while index < argv.len() {
        if argv[index] == flag {
            argv.remove(index);
            if index < argv.len() {
                argv.remove(index);
            }
        } else {
            index += 1;
        }
    }
}

fn remove_session_args(argv: &mut Vec<String>, flag: &str) {
    let mut index = 0;
    while index < argv.len() {
        if argv[index] == flag {
            argv.remove(index);
            if index < argv.len() {
                argv.remove(index);
            }
        } else {
            index += 1;
        }
    }
}

fn insert_session_arg(argv: &mut Vec<String>, flag: &str, insert_at: usize, session_id: &str) {
    argv.insert(insert_at, flag.to_string());
    argv.insert(insert_at + 1, session_id.to_string());
}

fn launch_env(
    request: &RequestEnvelope,
    params: &Value,
) -> Result<BTreeMap<String, String>, ProviderFailure> {
    let mut env = BTreeMap::new();
    if let Some(host_env) = request.host.get("env") {
        let entries = validate_env_object(host_env)?;
        merge_env(&mut env, &entries);
    }
    if let Some(param_env) = params.get("env") {
        let entries = validate_env_object(param_env)?;
        merge_env(&mut env, &entries);
    }
    Ok(env)
}

fn launch_stdin_bytes(params: &Value) -> Result<Vec<u8>, ProviderFailure> {
    let Some(payload) = params.get("stdin") else {
        return Ok(Vec::new());
    };

    stdin::decode_stdin_payload(payload).map_err(invalid_launch_stdin)
}

fn deadline_unix_ms(request: &RequestEnvelope) -> Option<u64> {
    request.host.get("deadline_unix_ms").and_then(Value::as_u64)
}

fn validate_env_object(value: &Value) -> Result<Vec<(&String, &str)>, ProviderFailure> {
    let object = value
        .as_object()
        .ok_or_else(|| invalid_launch_params("launch env must be an object"))?;
    object
        .iter()
        .map(|(key, value)| {
            value
                .as_str()
                .map(|value| (key, value))
                .ok_or_else(|| invalid_launch_params("launch env values must be strings"))
        })
        .collect()
}

fn merge_env(env: &mut BTreeMap<String, String>, entries: &[(&String, &str)]) {
    for &(key, value) in entries {
        env.insert(key.clone(), value.to_string());
    }
}

fn stream_launch_and_exit(
    request_id: &str,
    argv: &[String],
    cwd: &str,
    env: &BTreeMap<String, String>,
    stdin_bytes: Vec<u8>,
    deadline: Option<u64>,
    resume_confirmation: Option<submitted_user_turn::ResumeConfirmation>,
) -> ! {
    let stdout = io::stdout();
    let mut events = events::EventWriter::new(stdout.lock(), request_id);
    let mut stdout_prefix = StdoutPrefixAccumulator::default();
    let status = launch_status(
        &mut events,
        argv,
        cwd,
        env,
        stdin_bytes,
        deadline,
        &mut stdout_prefix,
    );
    emit_submitted_user_turn_marker(&mut events, &status, resume_confirmation.as_ref());
    emit_terminal_exit(
        &mut events,
        status,
        resume_confirmation.as_ref(),
        stdout_prefix.stdout(),
    );
    std::process::exit(0);
}

// declared_role: orchestration
fn emit_submitted_user_turn_marker<W: Write>(
    events: &mut events::EventWriter<W>,
    status: &Value,
    confirmation: Option<&submitted_user_turn::ResumeConfirmation>,
) {
    if !is_clean_exit(status) {
        return;
    }
    let Some(confirmation) = confirmation else {
        return;
    };
    let _ = events.marker(
        submitted_user_turn::marker_name(),
        submitted_user_turn::marker_value(confirmation),
    );
}

fn launch_status<W: Write>(
    events: &mut events::EventWriter<W>,
    argv: &[String],
    cwd: &str,
    env: &BTreeMap<String, String>,
    stdin_bytes: Vec<u8>,
    deadline: Option<u64>,
    stdout_prefix: &mut StdoutPrefixAccumulator,
) -> Value {
    match child::RunningChild::spawn(argv, cwd, env, stdin_bytes) {
        Ok((mut child, pipes)) => {
            spawned_child_status(events, &mut child, pipes, deadline, stdout_prefix)
        }
        Err(error) => spawn_error_status(&error),
    }
}

fn spawned_child_status<W: Write>(
    events: &mut events::EventWriter<W>,
    child: &mut child::RunningChild,
    pipes: child::ChildPipes,
    deadline: Option<u64>,
    stdout_prefix: &mut StdoutPrefixAccumulator,
) -> Value {
    emit_child_started(events);
    let (receiver, stdout_thread, stderr_thread) = spawn_pipe_drains(pipes);
    let status = wait_child_while_draining(events, child, &receiver, deadline, stdout_prefix);
    if !drain_for(events, &receiver, FINAL_DRAIN_GRACE, stdout_prefix) {
        child.terminate_descendants();
        let _ = drain_for(events, &receiver, FINAL_DRAIN_GRACE, stdout_prefix);
    }
    drop(stdout_thread);
    drop(stderr_thread);
    status
}

fn emit_child_started<W: Write>(events: &mut events::EventWriter<W>) {
    let _ = events.marker(session_marker::initial_marker_name(), json!(true));
    let _ = events.heartbeat("child_spawned");
}

fn spawn_pipe_drains(
    pipes: child::ChildPipes,
) -> (
    mpsc::Receiver<drain::DrainEvent>,
    std::thread::JoinHandle<()>,
    std::thread::JoinHandle<()>,
) {
    let (sender, receiver) = mpsc::channel();
    let stdout_thread = drain::spawn_drain("stdout", pipes.stdout, sender.clone());
    let stderr_thread = drain::spawn_drain("stderr", pipes.stderr, sender);
    (receiver, stdout_thread, stderr_thread)
}

fn wait_child_while_draining<W: Write>(
    events: &mut events::EventWriter<W>,
    child: &mut child::RunningChild,
    receiver: &mpsc::Receiver<drain::DrainEvent>,
    deadline: Option<u64>,
    stdout_prefix: &mut StdoutPrefixAccumulator,
) -> Value {
    loop {
        drain_once(events, receiver, DRAIN_POLL_INTERVAL, stdout_prefix);
        if let Some(status) = child.poll_status() {
            return status;
        }
        if deadline_elapsed(deadline) {
            return child.cancel_for_deadline();
        }
    }
}

fn drain_for<W: Write>(
    events: &mut events::EventWriter<W>,
    receiver: &mpsc::Receiver<drain::DrainEvent>,
    duration: Duration,
    stdout_prefix: &mut StdoutPrefixAccumulator,
) -> bool {
    drain_completed(drain_for_status(events, receiver, duration, stdout_prefix))
}

fn drain_for_status<W: Write>(
    events: &mut events::EventWriter<W>,
    receiver: &mpsc::Receiver<drain::DrainEvent>,
    duration: Duration,
    stdout_prefix: &mut StdoutPrefixAccumulator,
) -> DrainStatus {
    let started = Instant::now();
    while started.elapsed() < duration {
        let remaining = duration.saturating_sub(started.elapsed());
        if drain_completed(drain_once(events, receiver, remaining, stdout_prefix)) {
            return DrainStatus::Disconnected;
        }
    }
    DrainStatus::Open
}

fn drain_once<W: Write>(
    events: &mut events::EventWriter<W>,
    receiver: &mpsc::Receiver<drain::DrainEvent>,
    timeout: Duration,
    stdout_prefix: &mut StdoutPrefixAccumulator,
) -> DrainStatus {
    match receive_drain_event(receiver, timeout) {
        Ok(event) => drain_received_event(events, event, stdout_prefix),
        Err(error) => drain_error_status(error),
    }
}

fn receive_drain_event(
    receiver: &mpsc::Receiver<drain::DrainEvent>,
    timeout: Duration,
) -> Result<drain::DrainEvent, mpsc::RecvTimeoutError> {
    receiver.recv_timeout(timeout)
}

fn drain_received_event<W: Write>(
    events: &mut events::EventWriter<W>,
    event: drain::DrainEvent,
    stdout_prefix: &mut StdoutPrefixAccumulator,
) -> DrainStatus {
    emit_drain_event(events, event, stdout_prefix);
    DrainStatus::Open
}

fn drain_error_status(error: mpsc::RecvTimeoutError) -> DrainStatus {
    match error {
        mpsc::RecvTimeoutError::Timeout => DrainStatus::Open,
        mpsc::RecvTimeoutError::Disconnected => DrainStatus::Disconnected,
    }
}

fn drain_completed(status: DrainStatus) -> bool {
    matches!(status, DrainStatus::Disconnected)
}

// declared_role: orchestration
fn emit_drain_event<W: Write>(
    events: &mut events::EventWriter<W>,
    event: drain::DrainEvent,
    stdout_prefix: &mut StdoutPrefixAccumulator,
) {
    let (channel, bytes) = drain_event_data(event);
    record_stdout_channel_prefix(stdout_prefix, channel, &bytes);
    emit_stream_data(events, channel, &bytes);
}

// declared_role: filter
fn record_stdout_channel_prefix(
    stdout_prefix: &mut StdoutPrefixAccumulator,
    channel: &str,
    bytes: &[u8],
) {
    if channel == "stdout" {
        stdout_prefix.record_stdout_prefix(bytes);
    }
}

fn drain_event_data(event: drain::DrainEvent) -> (&'static str, Vec<u8>) {
    (event.channel, event.bytes)
}

fn emit_stream_data<W: Write>(
    events: &mut events::EventWriter<W>,
    channel: &'static str,
    bytes: &[u8],
) {
    let _ = events.data(channel, bytes);
}

// declared_role: orchestration
fn emit_terminal_exit<W: Write>(
    events: &mut events::EventWriter<W>,
    status: Value,
    resume_confirmation: Option<&submitted_user_turn::ResumeConfirmation>,
    stdout: &[u8],
) {
    let signal = terminal_signal(&status);
    let session = exit_session_object(launch_session_provider_id(resume_confirmation, stdout));
    let _ = events.exit_with_session(status, signal, session);
}

// declared_role: formatter
fn exit_session_object(provider_session_id: Option<String>) -> Option<Value> {
    provider_session_id
        .map(|provider_session_id| json!({ "provider_session_id": provider_session_id }))
}

// declared_role: mapper
fn launch_session_provider_id(
    resume: Option<&submitted_user_turn::ResumeConfirmation>,
    stdout: &[u8],
) -> Option<String> {
    resume_provider_session_id(resume)
        .or_else(|| crate::session::stdout_session_id::extract_stdout_session_id(stdout))
}

// declared_role: accessor
fn resume_provider_session_id(
    resume: Option<&submitted_user_turn::ResumeConfirmation>,
) -> Option<String> {
    resume.map(|confirmation| confirmation.session_id().to_string())
}

fn deadline_elapsed(deadline: Option<u64>) -> bool {
    deadline.is_some_and(|deadline| crate::encoding::now_unix_ms() >= deadline)
}

fn terminal_signal(status: &Value) -> Value {
    terminal_signal_value(terminal_signal_kind(status))
}

fn terminal_signal_kind(status: &Value) -> &'static str {
    match status_kind(status) {
        Some("exited") if is_clean_exit(status) => "clean_exit",
        Some("exited") => "nonzero_exit",
        Some("signal_terminated") => "signal_exit",
        Some("spawn_error") => "spawn_error",
        Some("cancelled") => "cancelled",
        Some("prolonged_silence") => "prolonged_silence",
        _ => "unknown",
    }
}

fn status_kind(status: &Value) -> Option<&str> {
    status.get("kind").and_then(Value::as_str)
}

fn is_clean_exit(status: &Value) -> bool {
    status_exit_code(status) == Some(0)
}

fn status_exit_code(status: &Value) -> Option<i64> {
    status.get("code").and_then(Value::as_i64)
}

fn terminal_signal_value(kind: &str) -> Value {
    let now = crate::encoding::now_unix_ms();
    json!({ "kind": kind, "observed_at_unix_ms": now })
}

fn spawn_error_status(error: &io::Error) -> Value {
    json!({ "kind": "spawn_error", "reason": error.to_string() })
}

fn invalid_launch_stdin(error: String) -> ProviderFailure {
    ProviderFailure::invalid_request(
        "invalid_launch_stdin",
        format!("invalid launch stdin: {error}"),
    )
}

fn invalid_launch_params(message: impl Into<String>) -> ProviderFailure {
    ProviderFailure::new(
        ErrorCategory::InvalidRequest,
        "invalid_launch_params",
        message.into(),
        false,
    )
}
