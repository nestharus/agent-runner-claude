// declared_role: mapper, orchestration, formatter, validator

use agent_runner_claude::encoding::encode_base64;
use serde_json::{json, Value};
use std::path::Path;

use super::fixtures::{envelope, host_context, path_string, TempRoots, CONTRACT};

pub const TERMINAL_OBSERVED_AT_UNIX_MS: u64 = 1_779_991_000_200;

pub fn contract_request(roots: &TempRoots, params: Value) -> Value {
    envelope(CONTRACT, host_context(roots), params)
}

pub fn policy_evaluate_request(
    roots: &TempRoots,
    provider_args: &[&str],
    model_prompt: Value,
    launch: Value,
) -> Value {
    contract_request(
        roots,
        policy_evaluate_params(provider_args, model_prompt, launch),
    )
}

fn policy_evaluate_params(provider_args: &[&str], model_prompt: Value, launch: Value) -> Value {
    json!({
        "settings_id": "claude-primary",
        "mode": "proxy",
        "model": {
            "name": "claude-sonnet",
            "provider_args": provider_args,
            "inputs": { "prompt": model_prompt, "named": {} }
        },
        "launch": launch
    })
}

pub fn terminal_classify_request(
    roots: &TempRoots,
    stdout: &[u8],
    stderr: &[u8],
    status: Value,
) -> Value {
    terminal_classify_request_from_base64(
        roots,
        encode_base64(stdout),
        encode_base64(stderr),
        status,
    )
}

pub fn terminal_classify_request_from_base64(
    roots: &TempRoots,
    stdout_base64: String,
    stderr_base64: String,
    status: Value,
) -> Value {
    contract_request(
        roots,
        terminal_classify_params(
            stdout_base64,
            stderr_base64,
            status,
            TERMINAL_OBSERVED_AT_UNIX_MS,
        ),
    )
}

fn terminal_classify_params(
    stdout_base64: String,
    stderr_base64: String,
    status: Value,
    observed_at_unix_ms: u64,
) -> Value {
    json!({
        "stdout_base64": stdout_base64,
        "stderr_base64": stderr_base64,
        "status": status,
        "observed_at_unix_ms": observed_at_unix_ms
    })
}

pub fn launch_request(roots: &TempRoots, argv: Vec<String>, extra: Value) -> Value {
    let mut params = launch_base_params(argv, path_string(&roots.root));
    merge_object_fields(&mut params, &extra, "extra object");
    contract_request(roots, params)
}

fn launch_base_params(argv: Vec<String>, working_directory: String) -> Value {
    json!({
        "settings_id": "claude-primary",
        "mode": "proxy",
        "model": {
            "name": "claude-sonnet",
            "provider_args": [],
            "inputs": { "prompt": null, "named": {} }
        },
        "argv": argv,
        "working_directory": working_directory
    })
}

fn merge_object_fields(target: &mut Value, extra: &Value, message: &str) {
    let fields = object_fields(extra, message);
    let target = object_fields_mut(target);
    copy_object_fields(target, fields);
}

fn object_fields<'a>(value: &'a Value, message: &str) -> &'a serde_json::Map<String, Value> {
    value.as_object().expect(message)
}

fn object_fields_mut(value: &mut Value) -> &mut serde_json::Map<String, Value> {
    value.as_object_mut().unwrap()
}

fn copy_object_fields(
    target: &mut serde_json::Map<String, Value>,
    fields: &serde_json::Map<String, Value>,
) {
    for (key, value) in fields {
        target.insert(key.clone(), value.clone());
    }
}

pub fn launch_timeout_request(
    roots: &TempRoots,
    deadline_unix_ms: u64,
    argv: Vec<String>,
) -> Value {
    let mut host = host_context(roots);
    host.as_object_mut()
        .unwrap()
        .insert("deadline_unix_ms".to_string(), json!(deadline_unix_ms));
    envelope(
        CONTRACT,
        host,
        launch_base_params(argv, path_string(&roots.root)),
    )
}

pub fn session_replace_params(
    session_id: &str,
    path: &Path,
    canonical: &[u8],
    preimage: Option<String>,
) -> Value {
    let mut params = session_replace_base_params(session_id, path, encode_base64(canonical));
    if let Some(preimage) = preimage {
        insert_preimage_sha256(&mut params, preimage);
    }
    params
}

fn session_replace_base_params(session_id: &str, path: &Path, canonical_base64: String) -> Value {
    json!({
        "settings_id": "claude-primary",
        "session_id": session_id,
        "path": path.display().to_string(),
        "canonical_format": "oulipoly.canonical_transcript/v1",
        "canonical_transcript": { "kind": "bytes", "data_base64": canonical_base64 }
    })
}

fn insert_preimage_sha256(params: &mut Value, preimage: String) {
    params["preimage_sha256"] = json!(preimage);
}
