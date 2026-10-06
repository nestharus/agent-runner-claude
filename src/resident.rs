//! Resident Claude Code sessions (`oulipoly.resident_session/v1`).
//!
//! `resident.prepare` admits the host's policy-evaluated launch template (the
//! Claude Code CLI argv `policy.evaluate` produced, its environment and
//! route), refuses session-selecting flags the resident endpoint owns, and
//! durably records it, content-addressed, under the provider's state. Its
//! result names the arguments the host appends to this same registered
//! provider executable: `resident.serve --config <path>`.
//!
//! `resident.serve` serves the SDK's resident ACP v2 endpoint on stdio. Each
//! turn runs the native Claude Code CLI once through the SDK's shared launch
//! lifecycle, in print mode with stream-JSON input and output and user-message
//! replay: the first turn creates the native session under an id this
//! provider chooses (`--session-id`), later turns `--resume` it. The native
//! `system/init` event names the session, the replayed user message is the
//! consumption evidence, each main-thread assistant message with text is one
//! agent message, and the `result` event decides success. Native tool and
//! transcript records stay in Claude Code's own session store. Never the
//! Claude Agent SDK.

use crate::launch::{self, gated_command, output_complete_marker};
use crate::{
    classify_terminal_signal, now_unix_ms, process_status_from_output, sha256_hex,
    terminal_signal_json, HostContext, ProcessStatus, ProviderFailure, RequestEnvelope,
};
use agent_provider_contract::resident_session::{self, ResidentPrepareResult};
use agent_provider_execution::custody::RequestCustody;
use agent_provider_execution::encoding::canonical_json_bytes;
use agent_provider_execution::lifecycle::{
    self, Channel, EventSink, LaunchAdapter, LaunchSpec, LifecycleTiming, NativeCommand,
    NativeOutcome, OutputFraming, Preparation, Terminal,
};
use agent_provider_execution::resident::{
    self as endpoint, ResidentTurns, TurnFailure, TurnFailureKind, TurnRequest,
};
use serde_json::{json, Value};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

pub const PREPARE: &str = resident_session::PREPARE_SUBCOMMAND;
pub const SERVE: &str = "resident.serve";
const MAX_CONFIG_BYTES: usize = 1024 * 1024;
const MAX_NATIVE_LINE: u64 = 32 * 1024 * 1024;
/// Flags that choose a native session; the resident endpoint owns that choice.
const SESSION_FLAGS: &[&str] = &[
    "--resume",
    "-r",
    "--continue",
    "-c",
    "--session-id",
    "--fork-session",
];
/// Transport flags the resident turn sets itself; a template's own values are
/// replaced, never combined.
const TRANSPORT_FLAGS: &[(&str, bool)] = &[
    ("-p", false),
    ("--print", false),
    ("--verbose", false),
    ("--replay-user-messages", false),
    ("--output-format", true),
    ("--input-format", true),
];

fn invalid(request_id: &str, code: &'static str, message: impl Into<String>) -> ProviderFailure {
    ProviderFailure::invalid_request(request_id.to_owned(), code, message)
}

fn resident_root(host: &HostContext) -> Result<PathBuf, ProviderFailure> {
    let data_root = match &host.data_root {
        Some(root) => PathBuf::from(root),
        None => host
            .env
            .as_ref()
            .and_then(|env| env.get("HOME").cloned())
            .or_else(|| std::env::var("HOME").ok())
            .filter(|home| !home.is_empty())
            .map(|home| Path::new(&home).join(".local/share/oulipoly-agent-runner"))
            .ok_or_else(|| {
                launch::failure(
                    "",
                    "resident_state_unavailable",
                    "resident state requires host.data_root or HOME",
                )
            })?,
    };
    Ok(data_root.join("provider-state/claude/resident"))
}

/// The template argv without transport flags, or why it cannot be resident.
fn base_argv(argv: &[String]) -> Result<Vec<String>, String> {
    let Some((program, rest)) = argv.split_first() else {
        return Err("launch.argv must name the Claude Code command".into());
    };
    let mut base = vec![program.clone()];
    let mut args = rest.iter();
    while let Some(arg) = args.next() {
        let flag = arg.split_once('=').map_or(arg.as_str(), |(flag, _)| flag);
        if SESSION_FLAGS.contains(&flag) {
            return Err(format!(
                "{flag} selects a native session; the resident endpoint chooses sessions"
            ));
        }
        match TRANSPORT_FLAGS.iter().find(|(name, _)| *name == flag) {
            Some((_, takes_value)) => {
                if *takes_value && !arg.contains('=') {
                    args.next();
                }
            }
            None => base.push(arg.clone()),
        }
    }
    Ok(base)
}

/// The argv of one resident turn.
fn turn_argv(base: &[String], turn: &TurnRequest) -> Vec<String> {
    let mut argv = base.to_vec();
    argv.extend(
        [
            "-p",
            "--input-format",
            "stream-json",
            "--output-format",
            "stream-json",
            "--verbose",
            "--replay-user-messages",
        ]
        .map(String::from),
    );
    match (&turn.native_session_id, &turn.create_native_session_id) {
        (Some(id), _) => argv.extend(["--resume".into(), id.clone()]),
        (None, Some(id)) => argv.extend(["--session-id".into(), id.clone()]),
        (None, None) => {}
    }
    argv
}

/// `resident.prepare`.
pub(crate) fn prepare(request: &RequestEnvelope) -> Result<Value, ProviderFailure> {
    let id = &request.request_id;
    if !resident_session::FAMILY
        .advertised(
            resident_session::SUPPORTED_VERSIONS,
            request.host.env.as_ref(),
        )
        .contains(&1)
    {
        return Err(ProviderFailure::unsupported(
            id.clone(),
            "resident_session_not_selected",
            "resident.prepare requires host.env.OULIPOLY_HOST_RESIDENT_SESSION_V1=1",
            3,
        ));
    }
    let params = resident_session::decode_prepare_params(&request.params)
        .map_err(|error| invalid(id, "invalid_resident_prepare", error.to_string()))?;
    base_argv(&params.launch.argv)
        .map_err(|message| invalid(id, "invalid_resident_argv", message))?;
    let mut host = serde_json::to_value(&request.host).expect("host serializes");
    // A resident endpoint outlives this request: its deadline does not apply.
    host["deadline_unix_ms"] = Value::Null;
    let config = json!({"protocol": resident_session::PROTOCOL, "provider": "claude",
        "host": host, "launch": params.launch});
    let bytes = canonical_json_bytes(&config);
    let digest = sha256_hex(&bytes);
    let directory = resident_root(&request.host)?.join("configs");
    let io = |error: std::io::Error| launch::failure(id, "resident_io", error.to_string());
    agent_provider_execution::durable_fs::create_private_directories(&directory).map_err(io)?;
    let path = directory.join(format!("{digest}.json"));
    if !path.is_file() {
        let staged = directory.join(format!(".{digest}.{}.tmp", std::process::id()));
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&staged)
            .map_err(io)?;
        file.write_all(&bytes).map_err(io)?;
        file.sync_all().map_err(io)?;
        std::fs::rename(&staged, &path).map_err(io)?;
        agent_provider_execution::durable_fs::sync_directory(&directory).map_err(io)?;
    }
    let result = ResidentPrepareResult::v1(
        vec![SERVE.into(), "--config".into(), path.display().to_string()],
        digest,
    );
    Ok(serde_json::to_value(result).expect("prepare result serializes"))
}

struct ClaudeTurns {
    config: Value,
    base_argv: Vec<String>,
}

impl ResidentTurns for ClaudeTurns {
    fn implementation(&self) -> (String, String) {
        (
            "agent-runner-claude".into(),
            env!("CARGO_PKG_VERSION").into(),
        )
    }

    fn create_native_session_id(&self) -> Option<String> {
        endpoint::random_uuid().ok()
    }

    fn run_turn(
        &self,
        turn: &TurnRequest,
        stop: &AtomicBool,
        events: &mut dyn Write,
    ) -> Result<i32, TurnFailure> {
        let classify = |failure: ProviderFailure| TurnFailure {
            kind: match failure.code {
                "launch_cancelled" => TurnFailureKind::Cancelled,
                "launch_reconciliation_required" => TurnFailureKind::ReconciliationRequired,
                _ => TurnFailureKind::Failed,
            },
            code: failure.code.into(),
            message: failure.message,
        };
        let spec = LaunchSpec {
            contract: crate::CONTRACT,
            request_id: &turn.request_id,
            provider_instance_id: None,
            deadline_unix_ms: None,
            state_root: &turn.state_root,
            timing: LifecycleTiming::default(),
        };
        let mut adapter = ResidentTurn {
            turn,
            argv: turn_argv(&self.base_argv, turn),
            env: self.config["launch"]["env"].clone(),
            user_uuid: sha256_hex(turn.request_id.as_bytes())
                .chars()
                .take(32)
                .collect(),
            native_session: turn
                .native_session_id
                .clone()
                .or_else(|| turn.create_native_session_id.clone()),
            consumed: false,
            result: None,
            stdout: Vec::new(),
            stderr: Vec::new(),
        };
        let mut events = events;
        lifecycle::run_launch_until(&spec, stop, &mut adapter, &mut events).map_err(classify)
    }
}

/// One resident turn of the native Claude Code CLI in stream-JSON print mode.
struct ResidentTurn<'a> {
    turn: &'a TurnRequest,
    argv: Vec<String>,
    env: Value,
    /// Identity of the submitted user message, echoed by the replay.
    user_uuid: String,
    native_session: Option<String>,
    consumed: bool,
    result: Option<Value>,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
}

impl ResidentTurn<'_> {
    fn user_message(&self) -> Vec<u8> {
        let uuid = format!(
            "{}-{}-{}-{}-{}",
            &self.user_uuid[0..8],
            &self.user_uuid[8..12],
            &self.user_uuid[12..16],
            &self.user_uuid[16..20],
            &self.user_uuid[20..32]
        );
        let mut line = serde_json::to_vec(&json!({"type":"user","uuid":uuid,
            "session_id":self.native_session.clone().unwrap_or_default(),
            "parent_tool_use_id":null,
            "message":{"role":"user","content":[{"type":"text","text":self.turn.prompt}]}}))
        .expect("user message serializes");
        line.push(b'\n');
        line
    }

    fn echoes_input(&self, event: &Value) -> bool {
        let uuid = event["uuid"].as_str().unwrap_or_default().replace('-', "");
        event["type"] == json!("user")
            && (uuid == self.user_uuid || event["isReplay"] == json!(true))
    }
}

impl LaunchAdapter for ResidentTurn<'_> {
    type Failure = ProviderFailure;

    /// Everything that determines the native turn; never executable identity.
    fn request_digest(&mut self) -> Result<String, ProviderFailure> {
        let turn = self.turn;
        Ok(sha256_hex(
            &serde_json::to_vec(&json!({"argv":self.argv,"env":self.env,"cwd":turn.cwd,
                "prompt":turn.prompt,"native_session":turn.native_session_id,
                "create":turn.create_native_session_id}))
            .expect("digest input serializes"),
        ))
    }

    fn prepare(&mut self, _custody: &RequestCustody) -> Result<Preparation, ProviderFailure> {
        let id = &self.turn.request_id;
        let (program, args) = self
            .argv
            .split_first()
            .expect("resident argv has a program");
        let mut command = gated_command(id, program, args)?;
        command.command_mut().current_dir(&self.turn.cwd);
        if let Some(env) = self.env.as_object() {
            for (key, value) in env {
                if let Some(value) = value.as_str() {
                    command.command_mut().env(key, value);
                }
            }
        }
        Ok(Preparation::Native(NativeCommand {
            command,
            stdin: Some(self.user_message()),
            framing: OutputFraming::Lines {
                max_bytes: MAX_NATIVE_LINE,
            },
        }))
    }

    fn output<W: Write>(
        &mut self,
        channel: Channel,
        bytes: Vec<u8>,
        events: &mut EventSink<'_, W>,
    ) -> Result<(), ProviderFailure> {
        if channel == Channel::Stderr {
            self.stderr.extend_from_slice(&bytes);
            events.data(Channel::Stderr, &bytes)?;
            return Ok(());
        }
        self.stdout.extend_from_slice(&bytes);
        let Ok(event) = serde_json::from_slice::<Value>(&bytes) else {
            return Ok(());
        };
        match event["type"].as_str() {
            Some("system") if event["subtype"] == json!("init") => {
                if let Some(id) = event["session_id"].as_str() {
                    self.native_session = Some(id.to_owned());
                    events.marker(
                        endpoint::PROVIDER_SESSION_MARKER,
                        json!({"provider_session_id":id,"source":"claude.stream_json"}),
                    )?;
                }
            }
            Some("user") if !self.consumed && self.echoes_input(&event) => {
                self.consumed = true;
                events.marker(
                    endpoint::SUBMITTED_USER_TURN_MARKER,
                    json!({"provider_session_id":self.native_session,
                        "prompt_sha256":sha256_hex(self.turn.prompt.as_bytes()),
                        "source":"claude.stream_json.replay"}),
                )?;
            }
            Some("assistant") if event["parent_tool_use_id"].is_null() => {
                let text: String = event["message"]["content"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter(|block| block["type"] == json!("text"))
                    .filter_map(|block| block["text"].as_str())
                    .collect();
                if !text.is_empty() {
                    events.data(Channel::Stdout, format!("{text}\n").as_bytes())?;
                }
            }
            Some("result") => self.result = Some(event),
            _ => {}
        }
        Ok(())
    }

    fn finish<W: Write>(
        &mut self,
        outcome: NativeOutcome,
        events: &mut EventSink<'_, W>,
    ) -> Result<Terminal, ProviderFailure> {
        let mut status = match outcome.stopped {
            Some(_) => ProcessStatus::Cancelled,
            None => process_status_from_output(&outcome.status),
        };
        // A clean exit without a successful result is not a successful turn.
        let succeeded = self.result.as_ref().is_some_and(|result| {
            result["subtype"] == json!("success") && result["is_error"] != json!(true)
        });
        if matches!(status, ProcessStatus::Exited { code: 0 }) && !succeeded {
            status = ProcessStatus::Exited { code: 1 };
        }
        let signal = classify_terminal_signal(&self.stdout, &self.stderr, &status, now_unix_ms());
        events.event(output_complete_marker(events.accounting().to_json()))?;
        Ok(Terminal {
            status: serde_json::to_value(&status).expect("process status serializes"),
            terminal_signal: terminal_signal_json(&signal),
            session: Some(json!({"provider_session_id":self.native_session})),
            exit_code: 0,
        })
    }
}

/// `resident.serve --config <path>`: serves one ACP v2 connection on stdio.
pub fn serve(args: &[String]) -> i32 {
    let usage = || {
        eprintln!("usage: agent-runner-claude resident.serve --config <path>");
        2
    };
    let [_, _, flag, path] = args else {
        return usage();
    };
    if flag != "--config" {
        return usage();
    }
    let config = match load_config(Path::new(path)) {
        Ok(config) => config,
        Err(message) => {
            eprintln!("resident configuration refused: {message}");
            return 2;
        }
    };
    let host: HostContext = match serde_json::from_value(config["host"].clone()) {
        Ok(host) => host,
        Err(error) => {
            eprintln!("resident configuration refused: {error}");
            return 2;
        }
    };
    let state_root = match resident_root(&host) {
        Ok(root) => root,
        Err(failure) => {
            eprintln!("resident state unavailable: {}", failure.message);
            return 2;
        }
    };
    let argv: Vec<String> =
        serde_json::from_value(config["launch"]["argv"].clone()).unwrap_or_default();
    let base_argv = match base_argv(&argv) {
        Ok(base) => base,
        Err(message) => {
            eprintln!("resident configuration refused: {message}");
            return 2;
        }
    };
    let stdin = std::io::BufReader::new(std::io::stdin());
    match endpoint::serve(
        Arc::new(ClaudeTurns { config, base_argv }),
        &state_root,
        stdin,
        std::io::stdout(),
    ) {
        Ok(_) => 0,
        Err(error) => {
            eprintln!("resident endpoint failed: {error}");
            1
        }
    }
}

/// Reads a recorded configuration whose content still matches its name.
fn load_config(path: &Path) -> Result<Value, String> {
    let bytes = agent_provider_execution::durable_fs::read_file_bounded(path, MAX_CONFIG_BYTES)
        .map_err(|error| error.to_string())?;
    let name = path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or_default();
    if sha256_hex(&bytes) != name {
        return Err("configuration content does not match its digest".into());
    }
    let config: Value = serde_json::from_slice(&bytes).map_err(|error| error.to_string())?;
    if config["protocol"] != json!(resident_session::PROTOCOL)
        || config["provider"] != json!("claude")
    {
        return Err("not a Claude resident-session/v1 configuration".into());
    }
    Ok(config)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn template_transport_flags_are_replaced_and_session_flags_refused() {
        let argv: Vec<String> = [
            "claude",
            "-p",
            "--output-format",
            "json",
            "--model",
            "opus",
            "--verbose",
        ]
        .map(String::from)
        .to_vec();
        assert_eq!(base_argv(&argv).unwrap(), ["claude", "--model", "opus"]);
        let argv: Vec<String> = [
            "claude",
            "--output-format=json",
            "--append-system-prompt",
            "x",
        ]
        .map(String::from)
        .to_vec();
        assert_eq!(
            base_argv(&argv).unwrap(),
            ["claude", "--append-system-prompt", "x"]
        );
        for flag in SESSION_FLAGS {
            let argv: Vec<String> = ["claude", flag, "id"].map(String::from).to_vec();
            assert!(base_argv(&argv).is_err(), "{flag}");
        }
        assert!(base_argv(&[]).is_err());
    }
}
