// declared_role: orchestration, validator, predicate, mapper, accessor, formatter
// intrinsic_surface_declarations:
//   - component: src/launch/mod.rs
//     role: intrinsic-surface
//     Domain: launch_capability_module_index
//     Owns:
//       - launch capability submodule declaration set
//       - launch request dispatch and pre-spawn stream surface

pub mod child;
pub mod drain;
pub mod events;
pub mod params;
pub mod session_marker;
pub mod stdin;

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

#[derive(Clone, Copy)]
enum DrainStatus {
    Open,
    Disconnected,
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
    let cwd = match params::required_string(params, "working_directory") {
        Ok(cwd) => cwd,
        Err(message) => stream_pre_spawn_exit(&request.request_id, &message),
    };
    let stdin_bytes = match launch_stdin_bytes(params) {
        Ok(bytes) => bytes,
        Err(failure) => stream_pre_spawn_exit(&request.request_id, &failure.message),
    };
    let env = match launch_env(request, params) {
        Ok(env) => env,
        Err(failure) => stream_pre_spawn_exit(&request.request_id, &failure.message),
    };
    let deadline = deadline_unix_ms(request);

    stream_launch_and_exit(&request.request_id, &argv, cwd, &env, stdin_bytes, deadline);
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
) -> ! {
    let stdout = io::stdout();
    let mut events = events::EventWriter::new(stdout.lock(), request_id);
    let status = launch_status(&mut events, argv, cwd, env, stdin_bytes, deadline);
    emit_terminal_exit(&mut events, status);
    std::process::exit(0);
}

fn launch_status<W: Write>(
    events: &mut events::EventWriter<W>,
    argv: &[String],
    cwd: &str,
    env: &BTreeMap<String, String>,
    stdin_bytes: Vec<u8>,
    deadline: Option<u64>,
) -> Value {
    match child::RunningChild::spawn(argv, cwd, env, stdin_bytes) {
        Ok((mut child, pipes)) => spawned_child_status(events, &mut child, pipes, deadline),
        Err(error) => spawn_error_status(&error),
    }
}

fn spawned_child_status<W: Write>(
    events: &mut events::EventWriter<W>,
    child: &mut child::RunningChild,
    pipes: child::ChildPipes,
    deadline: Option<u64>,
) -> Value {
    emit_child_started(events);
    let (receiver, stdout_thread, stderr_thread) = spawn_pipe_drains(pipes);
    let status = wait_child_while_draining(events, child, &receiver, deadline);
    if !drain_for(events, &receiver, FINAL_DRAIN_GRACE) {
        child.terminate_descendants();
        let _ = drain_for(events, &receiver, FINAL_DRAIN_GRACE);
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
) -> Value {
    loop {
        drain_once(events, receiver, DRAIN_POLL_INTERVAL);
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
) -> bool {
    drain_completed(drain_for_status(events, receiver, duration))
}

fn drain_for_status<W: Write>(
    events: &mut events::EventWriter<W>,
    receiver: &mpsc::Receiver<drain::DrainEvent>,
    duration: Duration,
) -> DrainStatus {
    let started = Instant::now();
    while started.elapsed() < duration {
        let remaining = duration.saturating_sub(started.elapsed());
        if drain_completed(drain_once(events, receiver, remaining)) {
            return DrainStatus::Disconnected;
        }
    }
    DrainStatus::Open
}

fn drain_once<W: Write>(
    events: &mut events::EventWriter<W>,
    receiver: &mpsc::Receiver<drain::DrainEvent>,
    timeout: Duration,
) -> DrainStatus {
    match receive_drain_event(receiver, timeout) {
        Ok(event) => drain_received_event(events, event),
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
) -> DrainStatus {
    emit_drain_event(events, event);
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

fn emit_drain_event<W: Write>(events: &mut events::EventWriter<W>, event: drain::DrainEvent) {
    let (channel, bytes) = drain_event_data(event);
    emit_stream_data(events, channel, &bytes);
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

fn emit_terminal_exit<W: Write>(events: &mut events::EventWriter<W>, status: Value) {
    let signal = terminal_signal(&status);
    let _ = events.exit(status, signal);
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
