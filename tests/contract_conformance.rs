use jsonschema::{Draft, JSONSchema};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

const CONTRACT: &str = "oulipoly.provider/v1";

fn invoke(subcommand: &str, params: Value) -> Output {
    invoke_with_host(subcommand, params, json!({}))
}

fn invoke_with_host(subcommand: &str, params: Value, host_overrides: Value) -> Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_agent-runner-claude"))
        .arg(subcommand)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut host = json!({
        "app": "oulipoly-agent-runner",
        "app_version": "0.0.0",
        "platform": "linux-x86_64",
        "working_directory": "/tmp",
        "config_root": "/tmp/config",
        "data_root": "/tmp/data",
        "env": { "TERM": "xterm-256color" }
    });
    if let (Some(host), Some(overrides)) = (host.as_object_mut(), host_overrides.as_object()) {
        for (key, value) in overrides {
            host.insert(key.clone(), value.clone());
        }
    }
    let request = json!({
        "contract": CONTRACT,
        "request_id": format!("req-{subcommand}"),
        "provider_instance_id": "claude-primary",
        "host": host,
        "params": params
    });
    child
        .stdin
        .as_mut()
        .unwrap()
        .write_all(request.to_string().as_bytes())
        .unwrap();
    child.wait_with_output().unwrap()
}

fn temp_config_root(label: &str) -> std::path::PathBuf {
    let root = std::env::temp_dir().join(format!(
        "agent-runner-claude-{label}-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    root
}

fn json_stdout(output: &Output) -> Value {
    assert!(
        output.stderr.is_empty(),
        "stderr must remain diagnostics-only and empty for these tests: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

fn compile_contract_ref(schema_file: &str, def_name: &str) -> JSONSchema {
    let common: Value =
        serde_json::from_str(include_str!("../contract/v1/common.schema.json")).unwrap();
    let schema_text = match schema_file {
        "describe.schema.json" => include_str!("../contract/v1/describe.schema.json"),
        "schema.schema.json" => include_str!("../contract/v1/schema.schema.json"),
        "policy.schema.json" => include_str!("../contract/v1/policy.schema.json"),
        "terminal.schema.json" => include_str!("../contract/v1/terminal.schema.json"),
        "quota.schema.json" => include_str!("../contract/v1/quota.schema.json"),
        "launch.schema.json" => include_str!("../contract/v1/launch.schema.json"),
        "session.schema.json" => include_str!("../contract/v1/session.schema.json"),
        "common.schema.json" => include_str!("../contract/v1/common.schema.json"),
        other => panic!("unhandled schema file: {other}"),
    };
    let schema_doc: Value = serde_json::from_str(schema_text).unwrap();
    let mut root = bundled_contract_schema(common, schema_doc, def_name);

    let mut options = JSONSchema::options();
    options.with_draft(Draft::Draft202012);
    rewrite_external_refs(&mut root);
    options.compile(&root).unwrap()
}

fn bundled_contract_schema(common: Value, schema_doc: Value, def_name: &str) -> Value {
    let mut defs = common["$defs"].as_object().unwrap().clone();
    for (key, value) in schema_doc["$defs"].as_object().unwrap() {
        defs.insert(key.clone(), value.clone());
    }

    json!({
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "$defs": defs,
        "$ref": format!("#/$defs/{def_name}")
    })
}

fn rewrite_external_refs(value: &mut Value) {
    match value {
        Value::Object(map) => {
            if let Some(Value::String(reference)) = map.get_mut("$ref") {
                if let Some((document, def_path)) = reference.split_once("#/$defs/") {
                    if document.ends_with(".schema.json") {
                        *reference = format!("#/$defs/{def_path}");
                    }
                }
            }
            for child in map.values_mut() {
                rewrite_external_refs(child);
            }
        }
        Value::Array(items) => {
            for item in items {
                rewrite_external_refs(item);
            }
        }
        _ => {}
    }
}

fn assert_valid(schema: &JSONSchema, value: &Value) {
    if let Err(errors) = schema.validate(value) {
        let details = errors
            .map(|err| err.to_string())
            .collect::<Vec<_>>()
            .join("\n");
        panic!("contract validation failed:\n{details}\nvalue:\n{value}");
    }
}

#[test]
fn describe_response_conforms_to_contract() {
    let output = invoke("describe", json!({}));
    assert!(output.status.success());
    let response = json_stdout(&output);
    let schema = compile_contract_ref("describe.schema.json", "DescribeResponse");
    assert_valid(&schema, &response);

    let result = &response["result"];
    assert_eq!(result["provider_id"], "claude");
    assert_eq!(result["display_name"], "Claude Code");
    assert_eq!(result["settings_schema_id"], "claude.settings/v1");
    for key in ["launch", "policy", "terminal", "quota", "session"] {
        assert_eq!(result["capabilities"][key], true, "capability {key}");
    }
    for key in [
        "rotation",
        "discovery",
        "settings",
        "setup_brain",
        "setup",
        "migration",
    ] {
        assert_eq!(result["capabilities"][key], false, "capability {key}");
    }
    assert_eq!(result["concurrency"]["safe_for_parallel_invocation"], true);
}

#[test]
fn schema_response_conforms_to_contract_and_returns_valid_settings_schema() {
    let output = invoke("schema", json!({ "schema_id": "claude.settings/v1" }));
    assert!(output.status.success());
    let response = json_stdout(&output);
    let schema = compile_contract_ref("schema.schema.json", "SchemaResponse");
    assert_valid(&schema, &response);

    let settings_schema = &response["result"]["schema"];
    assert_eq!(response["result"]["schema_id"], "claude.settings/v1");
    JSONSchema::options()
        .with_draft(Draft::Draft202012)
        .compile(settings_schema)
        .unwrap();
    assert_eq!(
        settings_schema["properties"]["command"]["default"],
        "env -u CLAUDECODE claude"
    );
    assert_eq!(
        settings_schema["properties"]["tool_restrictions"]["properties"]["kind"]["const"],
        "claude"
    );
}

#[test]
fn unknown_schema_returns_contract_error_envelope() {
    let output = invoke("schema", json!({ "schema_id": "unknown.settings/v1" }));
    assert_eq!(output.status.code(), Some(1));
    let response = json_stdout(&output);
    let schema = compile_contract_ref("common.schema.json", "ErrorResponseEnvelope");
    assert_valid(&schema, &response);
    assert_eq!(response["ok"], false);
    assert_eq!(response["error"]["category"], "unsupported");
    assert_eq!(response["error"]["code"], "unknown_schema");
}

#[test]
fn later_capability_returns_contract_error_envelope() {
    let output = invoke("rotation.assess", json!({}));
    assert_eq!(output.status.code(), Some(3));
    let response = json_stdout(&output);
    let schema = compile_contract_ref("common.schema.json", "ErrorResponseEnvelope");
    assert_valid(&schema, &response);
    assert_eq!(response["error"]["code"], "capability_not_implemented");
}

fn temp_session_root(label: &str) -> PathBuf {
    let root = temp_config_root(label);
    let project = root.join("-tmp-workspace");
    std::fs::create_dir_all(&project).unwrap();
    root
}

fn session_context(projects_dir: &Path) -> Value {
    json!({
        "session_storage": {
            "kind": "claude_code",
            "projects_dir": projects_dir.display().to_string()
        },
        "provider_name": "claude-primary"
    })
}

fn write_claude_transcript(projects_dir: &Path, session_id: &str) -> PathBuf {
    let path = projects_dir
        .join("-tmp-workspace")
        .join(format!("{session_id}.jsonl"));
    std::fs::write(
        &path,
        format!(
            "{}\n{}\n",
            json!({
                "type": "user",
                "uuid": "turn-user-1",
                "sessionId": session_id,
                "timestamp": "2026-06-02T12:00:00Z",
                "message": { "role": "user", "content": [{ "type": "input_text", "text": "hello" }] }
            }),
            json!({
                "type": "assistant",
                "uuid": "turn-assistant-1",
                "sessionId": session_id,
                "timestamp": "2026-06-02T12:00:01Z",
                "parentUuid": "turn-user-1",
                "message": { "role": "assistant", "content": [{ "type": "output_text", "text": "world" }] }
            })
        ),
    )
    .unwrap();
    path
}

fn session_params(projects_dir: &Path, session_id: &str) -> Value {
    json!({
        "settings_id": "claude-primary",
        "session_id": session_id,
        "context": session_context(projects_dir)
    })
}

fn decode_b64(data: &str) -> Vec<u8> {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = Vec::new();
    let bytes = data.as_bytes();
    for chunk in bytes.chunks(4) {
        let mut values = [0_u8; 4];
        for (idx, byte) in chunk.iter().enumerate() {
            values[idx] = if *byte == b'=' {
                0
            } else {
                TABLE.iter().position(|value| value == byte).unwrap() as u8
            };
        }
        out.push((values[0] << 2) | (values[1] >> 4));
        if chunk.get(2) != Some(&b'=') {
            out.push((values[1] << 4) | (values[2] >> 2));
        }
        if chunk.get(3) != Some(&b'=') {
            out.push((values[2] << 6) | values[3]);
        }
    }
    out
}

fn encode_b64(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut encoded = String::new();
    for chunk in bytes.chunks(3) {
        let b0 = chunk[0];
        let b1 = *chunk.get(1).unwrap_or(&0);
        let b2 = *chunk.get(2).unwrap_or(&0);
        encoded.push(TABLE[(b0 >> 2) as usize] as char);
        encoded.push(TABLE[(((b0 & 0b0000_0011) << 4) | (b1 >> 4)) as usize] as char);
        if chunk.len() > 1 {
            encoded.push(TABLE[(((b1 & 0b0000_1111) << 2) | (b2 >> 6)) as usize] as char);
        } else {
            encoded.push('=');
        }
        if chunk.len() > 2 {
            encoded.push(TABLE[(b2 & 0b0011_1111) as usize] as char);
        } else {
            encoded.push('=');
        }
    }
    encoded
}

fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[test]
fn session_locate_transcript_finds_claude_code_file_by_session_filename() {
    let projects_dir = temp_session_root("session-locate");
    let transcript = write_claude_transcript(&projects_dir, "sess-locate");
    let output = invoke(
        "session.locate_transcript",
        session_params(&projects_dir, "sess-locate"),
    );
    assert!(output.status.success());
    let response = json_stdout(&output);
    let schema = compile_contract_ref("session.schema.json", "SessionLocateTranscriptResponse");
    assert_valid(&schema, &response);
    assert_eq!(response["result"]["located"], true);
    assert_eq!(response["result"]["format_id"], "claude_code");
    assert_eq!(
        response["result"]["path"],
        transcript.canonicalize().unwrap().display().to_string()
    );
    let _ = std::fs::remove_dir_all(projects_dir);
}

#[test]
fn session_locate_transcript_falls_back_to_content_session_id() {
    let projects_dir = temp_session_root("session-locate-content");
    let path = projects_dir
        .join("-tmp-workspace")
        .join("renamed-session.jsonl");
    std::fs::write(
        &path,
        format!(
            "{}\n",
            json!({
                "type": "user",
                "uuid": "turn-user-1",
                "sessionId": "sess-content",
                "timestamp": "2026-06-02T12:00:00Z",
                "message": "hello"
            })
        ),
    )
    .unwrap();
    let output = invoke(
        "session.locate_transcript",
        session_params(&projects_dir, "sess-content"),
    );
    assert!(output.status.success());
    let response = json_stdout(&output);
    assert_eq!(
        response["result"]["path"],
        path.canonicalize().unwrap().display().to_string()
    );
    let _ = std::fs::remove_dir_all(projects_dir);
}

#[test]
fn session_read_turns_returns_stable_turns_and_zero_turn_completion() {
    let projects_dir = temp_session_root("session-read-turns");
    write_claude_transcript(&projects_dir, "sess-turns");
    let output = invoke(
        "session.read_turns",
        session_params(&projects_dir, "sess-turns"),
    );
    assert!(output.status.success());
    let response = json_stdout(&output);
    let schema = compile_contract_ref("session.schema.json", "SessionReadTurnsResponse");
    assert_valid(&schema, &response);
    assert_eq!(response["result"]["complete"], true);
    assert_eq!(response["result"]["turn_count"], 2);
    assert_eq!(response["result"]["turns"][0]["turn_id"], "turn-user-1");
    assert_eq!(response["result"]["turns"][0]["body"][0]["text"], "hello");

    let empty_path = projects_dir
        .join("-tmp-workspace")
        .join("empty-session.jsonl");
    std::fs::write(&empty_path, "").unwrap();
    let output = invoke(
        "session.read_turns",
        session_params(&projects_dir, "empty-session"),
    );
    assert!(output.status.success());
    let response = json_stdout(&output);
    assert_eq!(response["result"]["turn_count"], 0);
    assert_eq!(response["result"]["complete"], true);
    let _ = std::fs::remove_dir_all(projects_dir);
}

#[test]
fn session_export_returns_canonical_jsonl_bytes_and_hash() {
    let projects_dir = temp_session_root("session-export");
    write_claude_transcript(&projects_dir, "sess-export");
    let output = invoke(
        "session.export",
        session_params(&projects_dir, "sess-export"),
    );
    assert!(output.status.success());
    let response = json_stdout(&output);
    let schema = compile_contract_ref("session.schema.json", "SessionExportResponse");
    assert_valid(&schema, &response);
    let bytes = decode_b64(response["result"]["data_base64"].as_str().unwrap());
    let text = String::from_utf8(bytes.clone()).unwrap();
    let lines = text.lines().collect::<Vec<_>>();
    assert_eq!(lines.len(), 2);
    let first: Value = serde_json::from_str(lines[0]).unwrap();
    assert_eq!(first["session_id"], "sess-export");
    assert_eq!(first["provider_name"], "claude-primary");
    assert_eq!(first["content"][0]["type"], "text");
    assert_eq!(response["result"]["turn_count"], 2);
    assert_eq!(response["result"]["sha256"], sha256_hex(&bytes));
    let _ = std::fs::remove_dir_all(projects_dir);
}

#[test]
fn session_replace_validates_preimage_and_writes_claude_storage_atomically() {
    let projects_dir = temp_session_root("session-replace");
    let transcript = write_claude_transcript(&projects_dir, "sess-replace");
    let export = json_stdout(&invoke(
        "session.export",
        session_params(&projects_dir, "sess-replace"),
    ));
    let mut records = String::from_utf8(decode_b64(
        export["result"]["data_base64"].as_str().unwrap(),
    ))
    .unwrap()
    .lines()
    .map(|line| serde_json::from_str::<Value>(line).unwrap())
    .collect::<Vec<_>>();
    records[0]["content"][0]["text"] = json!("replacement user body");
    records[1]["content"][0]["text"] = json!("replacement assistant body");
    let replacement = records
        .iter()
        .map(Value::to_string)
        .collect::<Vec<_>>()
        .join("\n")
        + "\n";
    let replacement_bytes = replacement.as_bytes();
    let output = invoke(
        "session.replace",
        json!({
            "settings_id": "claude-primary",
            "session_id": "sess-replace",
            "provider_name": "claude-primary",
            "canonical_format": "oulipoly.canonical_transcript/v1",
            "data_base64": encode_b64(replacement_bytes),
            "preimage_sha256": export["result"]["sha256"],
            "context": session_context(&projects_dir)
        }),
    );
    assert!(output.status.success());
    let response = json_stdout(&output);
    let schema = compile_contract_ref("session.schema.json", "SessionReplaceResponse");
    assert_valid(&schema, &response);
    assert_eq!(response["result"]["changed"], true);
    assert_ne!(
        response["result"]["postimage_sha256"],
        sha256_hex(replacement_bytes)
    );
    assert_eq!(
        response["result"]["host_state_plan"]["records_sha256"],
        sha256_hex(replacement_bytes)
    );
    assert_eq!(
        response["result"]["host_state_plan"]["postimage_sha256"],
        response["result"]["postimage_sha256"]
    );
    let native = std::fs::read_to_string(&transcript).unwrap();
    assert!(native.contains("replacement user body"));
    assert!(native.contains("replacement assistant body"));

    let mismatch = invoke(
        "session.replace",
        json!({
            "settings_id": "claude-primary",
            "session_id": "sess-replace",
            "provider_name": "claude-primary",
            "canonical_format": "oulipoly.canonical_transcript/v1",
            "data_base64": encode_b64(replacement_bytes),
            "preimage_sha256": "0000000000000000000000000000000000000000000000000000000000000000",
            "context": session_context(&projects_dir)
        }),
    );
    assert_eq!(mismatch.status.code(), Some(1));
    let response = json_stdout(&mismatch);
    assert_eq!(response["error"]["code"], "preimage_mismatch");
    let _ = std::fs::remove_dir_all(projects_dir);
}

#[test]
fn session_replace_reports_no_change_for_identical_canonical_input() {
    let projects_dir = temp_session_root("session-replace-no-change");
    write_claude_transcript(&projects_dir, "sess-no-change");
    let export = json_stdout(&invoke(
        "session.export",
        session_params(&projects_dir, "sess-no-change"),
    ));
    let output = invoke(
        "session.replace",
        json!({
            "settings_id": "claude-primary",
            "session_id": "sess-no-change",
            "provider_name": "claude-primary",
            "canonical_format": "oulipoly.canonical_transcript/v1",
            "data_base64": export["result"]["data_base64"],
            "preimage_sha256": export["result"]["sha256"],
            "context": session_context(&projects_dir)
        }),
    );
    assert!(output.status.success());
    let response = json_stdout(&output);
    assert_eq!(response["result"]["changed"], false);
    assert!(response["result"]["host_state_plan"].is_null());
    let _ = std::fs::remove_dir_all(projects_dir);
}

#[test]
fn session_capture_extracts_stdout_json_event_session_id() {
    let stdout = br#"{"type":"system","session_id":"stdout-session"}"#;
    let output = invoke(
        "session.capture",
        json!({
            "settings_id": "claude-primary",
            "stdout_base64": encode_b64(stdout),
            "capture": {
                "kind": "stdout_json_event",
                "event_type": "system",
                "event_id_path": "session_id"
            }
        }),
    );
    assert!(output.status.success());
    let response = json_stdout(&output);
    let schema = compile_contract_ref("session.schema.json", "SessionCaptureResponse");
    assert_valid(&schema, &response);
    assert_eq!(response["result"]["provider_session_id"], "stdout-session");
}

#[test]
fn quota_source_reports_fresh_cached_script_without_running_probe() {
    let output = invoke(
        "quota.source",
        json!({
            "settings_id": "claude-primary",
            "model_name": "claude-sonnet",
            "context": {
                "settings": {
                    "quota_script": "exit 99"
                },
                "now_unix_ms": 1779991000200u64,
                "cached_checked_at_unix_ms": 1779990990200u64,
                "cached_windows": [
                    {
                        "name": "weekly",
                        "remaining_ratio": 0.42,
                        "resets_at_unix_ms": 1780595800200u64
                    }
                ]
            }
        }),
    );
    assert!(output.status.success());
    let response = json_stdout(&output);
    let schema = compile_contract_ref("quota.schema.json", "QuotaSourceResponse");
    assert_valid(&schema, &response);

    assert_eq!(response["result"]["has_source"], true);
    assert_eq!(
        response["result"]["source_id"],
        "claude:claude-primary:quota_script"
    );
    assert_eq!(response["result"]["freshness"], "fresh");
}

#[test]
fn quota_source_rejects_non_object_context() {
    let output = invoke(
        "quota.source",
        json!({
            "settings_id": "claude-primary",
            "context": []
        }),
    );
    assert_eq!(output.status.code(), Some(2));
    let response = json_stdout(&output);
    let schema = compile_contract_ref("common.schema.json", "ErrorResponseEnvelope");
    assert_valid(&schema, &response);
    assert_eq!(response["error"]["category"], "invalid_request");
    assert_eq!(response["error"]["code"], "invalid_quota_context");
}

#[test]
fn quota_source_uses_provider_toml_for_runtime_request_shape() {
    let config_root = temp_config_root("quota-source");
    std::fs::write(
        config_root.join("providers.toml"),
        r#"[claude-primary]
quota_script = "printf '%s' '{\"windows\":[]}'"
"#,
    )
    .unwrap();
    let output = invoke_with_host(
        "quota.source",
        json!({
            "settings_id": "provider-a-test",
            "model_name": "claude-sonnet",
            "context": {
                "provider_name": "claude-primary"
            }
        }),
        json!({ "config_root": config_root.display().to_string() }),
    );
    assert!(output.status.success());
    let response = json_stdout(&output);
    let schema = compile_contract_ref("quota.schema.json", "QuotaSourceResponse");
    assert_valid(&schema, &response);
    assert_eq!(response["result"]["has_source"], true);
    assert_eq!(response["result"]["freshness"], "probe_required");
    let _ = std::fs::remove_dir_all(config_root);
}

#[test]
fn quota_source_derives_anthropic_script_from_provider_session_storage() {
    let config_root = temp_config_root("quota-derived-provider-storage");
    std::fs::write(
        config_root.join("providers.toml"),
        r#"[claude-primary.session_storage]
kind = "script"
cwd_script = "claude-code-cwd /home/test/.claude/projects"
"#,
    )
    .unwrap();
    let output = invoke_with_host(
        "quota.source",
        json!({
            "settings_id": "provider-a-test",
            "model_name": "claude-sonnet",
            "context": {
                "provider_name": "claude-primary"
            }
        }),
        json!({ "config_root": config_root.display().to_string() }),
    );
    assert!(output.status.success());
    let response = json_stdout(&output);
    let schema = compile_contract_ref("quota.schema.json", "QuotaSourceResponse");
    assert_valid(&schema, &response);
    assert_eq!(response["result"]["has_source"], true);
    let _ = std::fs::remove_dir_all(config_root);
}

#[test]
fn quota_source_merges_top_level_auth_with_nested_derived_storage() {
    let config_root = temp_config_root("quota-auth-plus-derived-provider-storage");
    std::fs::write(
        config_root.join("providers.toml"),
        r#"[claude-primary]
auth_refresh_command = "claude auth status"

[claude-primary.session_storage]
kind = "script"
cwd_script = "claude-code-cwd /home/test/.claude/projects"
"#,
    )
    .unwrap();
    let output = invoke_with_host(
        "quota.source",
        json!({
            "settings_id": "provider-a-test",
            "model_name": "claude-sonnet",
            "context": {
                "provider_name": "claude-primary"
            }
        }),
        json!({ "config_root": config_root.display().to_string() }),
    );
    assert!(output.status.success());
    let response = json_stdout(&output);
    let schema = compile_contract_ref("quota.schema.json", "QuotaSourceResponse");
    assert_valid(&schema, &response);
    assert_eq!(response["result"]["has_source"], true);
    let _ = std::fs::remove_dir_all(config_root);
}

#[test]
fn quota_source_derives_from_claude_code_storage_kind() {
    let config_root = temp_config_root("quota-claude-code-storage-kind");
    std::fs::write(
        config_root.join("providers.toml"),
        r#"[claude-primary.session_storage]
kind = "claude_code"
projects_dir = "/home/test/.claude/projects"
"#,
    )
    .unwrap();
    let output = invoke_with_host(
        "quota.source",
        json!({
            "settings_id": "provider-a-test",
            "model_name": "claude-sonnet",
            "context": {
                "provider_name": "claude-primary"
            }
        }),
        json!({ "config_root": config_root.display().to_string() }),
    );
    assert!(output.status.success());
    let response = json_stdout(&output);
    let schema = compile_contract_ref("quota.schema.json", "QuotaSourceResponse");
    assert_valid(&schema, &response);
    assert_eq!(response["result"]["has_source"], true);
    let _ = std::fs::remove_dir_all(config_root);
}

#[test]
fn quota_source_ignores_blank_quota_script_and_falls_back_to_storage() {
    let config_root = temp_config_root("quota-blank-script-fallback");
    std::fs::write(
        config_root.join("providers.toml"),
        r#"[claude-primary]
quota_script = "   "

[claude-primary.session_storage]
kind = "script"
cwd_script = "claude-code-cwd /home/test/.claude/projects"
"#,
    )
    .unwrap();
    let output = invoke_with_host(
        "quota.source",
        json!({
            "settings_id": "provider-a-test",
            "model_name": "claude-sonnet",
            "context": {
                "provider_name": "claude-primary"
            }
        }),
        json!({ "config_root": config_root.display().to_string() }),
    );
    assert!(output.status.success());
    let response = json_stdout(&output);
    let schema = compile_contract_ref("quota.schema.json", "QuotaSourceResponse");
    assert_valid(&schema, &response);
    assert_eq!(response["result"]["has_source"], true);
    let _ = std::fs::remove_dir_all(config_root);
}

#[test]
fn quota_source_accepts_standard_toml_literal_strings_and_quoted_tables() {
    let config_root = temp_config_root("quota-standard-toml-forms");
    std::fs::write(
        config_root.join("providers.toml"),
        r#"["claude.primary"]
quota_script = 'printf "%s" "{\"windows\":[]}"'
"#,
    )
    .unwrap();
    let output = invoke_with_host(
        "quota.source",
        json!({
            "settings_id": "provider-a-test",
            "model_name": "claude-sonnet",
            "context": {
                "provider_name": "claude.primary"
            }
        }),
        json!({ "config_root": config_root.display().to_string() }),
    );
    assert!(output.status.success());
    let response = json_stdout(&output);
    let schema = compile_contract_ref("quota.schema.json", "QuotaSourceResponse");
    assert_valid(&schema, &response);
    assert_eq!(response["result"]["has_source"], true);
    let _ = std::fs::remove_dir_all(config_root);
}

#[test]
fn quota_refresh_auth_uses_top_level_auth_with_nested_derived_storage() {
    let config_root = temp_config_root("quota-refresh-auth-plus-derived-storage");
    let marker = config_root.join("refresh-marker");
    std::fs::write(
        config_root.join("providers.toml"),
        format!(
            "[claude-primary]\nauth_refresh_command = \"printf refreshed > {}\"\n\n[claude-primary.session_storage]\nkind = \"script\"\ncwd_script = \"claude-code-cwd /home/test/.claude/projects\"\n",
            marker.display()
        ),
    )
    .unwrap();
    let output = invoke_with_host(
        "quota.refresh_auth",
        json!({
            "settings_id": "provider-a-test",
            "force": false
        }),
        json!({ "config_root": config_root.display().to_string() }),
    );
    assert!(output.status.success());
    let response = json_stdout(&output);
    let schema = compile_contract_ref("quota.schema.json", "QuotaRefreshAuthResponse");
    assert_valid(&schema, &response);
    assert_eq!(std::fs::read_to_string(&marker).unwrap(), "refreshed");
    let _ = std::fs::remove_dir_all(config_root);
}

#[test]
fn quota_source_derives_anthropic_script_from_legacy_sessions_toml() {
    let config_root = temp_config_root("quota-derived-session-storage");
    std::fs::write(config_root.join("providers.toml"), "[claude-primary]\n").unwrap();
    std::fs::write(
        config_root.join("sessions.toml"),
        r#"[claude-primary]
turn_script = "claude-code-turns /home/test/.claude/projects"
"#,
    )
    .unwrap();
    let output = invoke_with_host(
        "quota.source",
        json!({
            "settings_id": "provider-a-test",
            "model_name": "claude-sonnet",
            "context": {
                "provider_name": "claude-primary"
            }
        }),
        json!({ "config_root": config_root.display().to_string() }),
    );
    assert!(output.status.success());
    let response = json_stdout(&output);
    let schema = compile_contract_ref("quota.schema.json", "QuotaSourceResponse");
    assert_valid(&schema, &response);
    assert_eq!(response["result"]["has_source"], true);
    let _ = std::fs::remove_dir_all(config_root);
}

#[test]
fn quota_probe_maps_anthropic_usage_windows_to_remaining_ratio_contract() {
    let output = invoke(
        "quota.probe",
        json!({
            "settings_id": "claude-primary",
            "context": {
                "quota_script": "printf '%s' '{\"windows\":[{\"label\":\"weekly\",\"used_percent\":45,\"resets_at\":\"2026-04-17T15:00:00Z\"},{\"label\":\"5h-burst\",\"used_percent\":23,\"resets_at\":\"2026-04-23T19:00:00Z\"}]}'",
                "now_unix_ms": 1779991000200u64
            }
        }),
    );
    assert!(output.status.success());
    let response = json_stdout(&output);
    let schema = compile_contract_ref("quota.schema.json", "QuotaProbeResponse");
    assert_valid(&schema, &response);

    let result = &response["result"];
    assert_eq!(result["available"], true);
    assert_eq!(result["checked_at_unix_ms"], 1779991000200u64);
    let windows = result["windows"].as_array().unwrap();
    assert_eq!(windows.len(), 2);
    assert_eq!(windows[0]["name"], "weekly");
    assert!((windows[0]["remaining_ratio"].as_f64().unwrap() - 0.55).abs() < 1e-9);
    assert_eq!(windows[0]["resets_at_unix_ms"], 1776438000000u64);
    assert_eq!(windows[1]["name"], "5h-burst");
    assert!((windows[1]["remaining_ratio"].as_f64().unwrap() - 0.77).abs() < 1e-9);
}

#[test]
fn quota_probe_uses_provider_toml_for_runtime_request_shape() {
    let config_root = temp_config_root("quota-probe");
    std::fs::write(
        config_root.join("providers.toml"),
        r#"[claude-primary]
quota_script = "printf '%s' '{\"used_percent\":12,\"resets_at\":\"2026-04-23T19:00:00Z\"}'"
"#,
    )
    .unwrap();
    let output = invoke_with_host(
        "quota.probe",
        json!({
            "settings_id": "provider-a-test",
            "model_name": "claude-sonnet",
            "context": {
                "provider_name": "claude-primary",
                "now_unix_ms": 1779991000200u64
            }
        }),
        json!({ "config_root": config_root.display().to_string() }),
    );
    assert!(output.status.success());
    let response = json_stdout(&output);
    let schema = compile_contract_ref("quota.schema.json", "QuotaProbeResponse");
    assert_valid(&schema, &response);
    let windows = response["result"]["windows"].as_array().unwrap();
    assert_eq!(windows.len(), 1);
    assert!((windows[0]["remaining_ratio"].as_f64().unwrap() - 0.88).abs() < 1e-9);
    let _ = std::fs::remove_dir_all(config_root);
}

#[test]
fn quota_probe_rejects_empty_windows_after_prior_populated_data() {
    let output = invoke(
        "quota.probe",
        json!({
            "settings_id": "claude-primary",
            "context": {
                "quota_script": "printf '%s' '{\"windows\":[]}'",
                "had_prior_windows": true
            }
        }),
    );
    assert_eq!(output.status.code(), Some(1));
    let response = json_stdout(&output);
    let schema = compile_contract_ref("common.schema.json", "ErrorResponseEnvelope");
    assert_valid(&schema, &response);
    assert_eq!(response["error"]["category"], "unavailable");
    assert_eq!(
        response["error"]["code"],
        "quota_probe_empty_after_prior_data"
    );
    assert_eq!(response["error"]["retryable"], true);
}

#[test]
fn quota_probe_reports_retryable_error_for_nonzero_script() {
    let output = invoke(
        "quota.probe",
        json!({
            "settings_id": "claude-primary",
            "context": {
                "quota_script": "printf 'expired token' >&2; exit 4"
            }
        }),
    );
    assert_eq!(output.status.code(), Some(1));
    let response = json_stdout(&output);
    let schema = compile_contract_ref("common.schema.json", "ErrorResponseEnvelope");
    assert_valid(&schema, &response);
    assert_eq!(response["error"]["category"], "unavailable");
    assert_eq!(response["error"]["code"], "quota_probe_failed");
    assert_eq!(response["error"]["retryable"], true);
    assert!(response["error"]["details"]["stderr"]
        .as_str()
        .unwrap()
        .contains("expired token"));
}

#[test]
fn quota_probe_rejects_invalid_json_and_bad_resets_at() {
    let invalid_json = invoke(
        "quota.probe",
        json!({
            "settings_id": "claude-primary",
            "context": {
                "quota_script": "printf '%s' 'not-json'"
            }
        }),
    );
    assert_eq!(invalid_json.status.code(), Some(1));
    let response = json_stdout(&invalid_json);
    let schema = compile_contract_ref("common.schema.json", "ErrorResponseEnvelope");
    assert_valid(&schema, &response);
    assert_eq!(response["error"]["code"], "quota_probe_parse_failed");

    let bad_resets_at = invoke(
        "quota.probe",
        json!({
            "settings_id": "claude-primary",
            "context": {
                "quota_script": "printf '%s' '{\"used_percent\":12,\"resets_at\":\"not-a-date\"}'"
            }
        }),
    );
    assert_eq!(bad_resets_at.status.code(), Some(1));
    let response = json_stdout(&bad_resets_at);
    assert_valid(&schema, &response);
    assert_eq!(response["error"]["code"], "quota_probe_parse_failed");
    assert!(response["error"]["message"]
        .as_str()
        .unwrap()
        .contains("Bad resets_at"));
}

#[test]
fn quota_refresh_auth_executes_provider_owned_command() {
    let marker = std::env::temp_dir().join(format!(
        "agent-runner-claude-refresh-auth-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_file(&marker);
    let command = format!("printf refreshed > {}", marker.display());
    let output = invoke(
        "quota.refresh_auth",
        json!({
            "settings_id": "claude-primary",
            "context": {
                "auth_refresh_command": command,
                "now_unix_ms": 1779991000200u64
            }
        }),
    );
    assert!(output.status.success());
    let response = json_stdout(&output);
    let schema = compile_contract_ref("quota.schema.json", "QuotaRefreshAuthResponse");
    assert_valid(&schema, &response);

    assert_eq!(response["result"]["refreshed"], true);
    assert_eq!(response["result"]["available"], true);
    assert_eq!(std::fs::read_to_string(&marker).unwrap(), "refreshed");
    let _ = std::fs::remove_file(marker);
}

#[test]
fn quota_refresh_auth_reports_retryable_error_for_nonzero_command() {
    let output = invoke(
        "quota.refresh_auth",
        json!({
            "settings_id": "claude-primary",
            "context": {
                "auth_refresh_command": "printf 'login failed' >&2; exit 7"
            }
        }),
    );
    assert_eq!(output.status.code(), Some(1));
    let response = json_stdout(&output);
    let schema = compile_contract_ref("common.schema.json", "ErrorResponseEnvelope");
    assert_valid(&schema, &response);
    assert_eq!(response["error"]["category"], "unavailable");
    assert_eq!(response["error"]["code"], "quota_refresh_auth_failed");
    assert_eq!(response["error"]["retryable"], true);
    assert!(response["error"]["details"]["stderr"]
        .as_str()
        .unwrap()
        .contains("login failed"));
}

#[test]
fn quota_refresh_auth_uses_provider_toml_when_runtime_context_is_empty() {
    let config_root = temp_config_root("quota-refresh-auth");
    let marker = config_root.join("refresh-marker");
    std::fs::write(
        config_root.join("providers.toml"),
        format!(
            "[claude-primary]\nauth_refresh_command = \"printf refreshed > {}\"\n",
            marker.display()
        ),
    )
    .unwrap();
    let output = invoke_with_host(
        "quota.refresh_auth",
        json!({
            "settings_id": "provider-a-test",
            "force": false
        }),
        json!({ "config_root": config_root.display().to_string() }),
    );
    assert!(output.status.success());
    let response = json_stdout(&output);
    let schema = compile_contract_ref("quota.schema.json", "QuotaRefreshAuthResponse");
    assert_valid(&schema, &response);
    assert_eq!(std::fs::read_to_string(&marker).unwrap(), "refreshed");
    let _ = std::fs::remove_dir_all(config_root);
}

#[test]
fn policy_evaluate_appends_claude_policy_and_reports_diagnostics() {
    let output = invoke(
        "policy.evaluate",
        json!({
            "settings_id": "claude-primary",
            "mode": "proxy",
            "model": {
                "name": "claude-sonnet",
                "provider_args": ["--model", "sonnet"],
                "inputs": { "prompt": "hello", "named": {} }
            },
            "launch": {
                "command": "claude --allowed-tools Bash",
                "args": ["--tools", "mcp__unsafe"],
                "prompt_mode": "arg",
                "invocation_mode": "proxy",
                "system_prompt_override": "system policy",
                "tool_restrictions": {
                    "kind": "claude",
                    "claude": {
                        "allowed_tools": ["Bash"],
                        "disable_slash_commands": true
                    }
                }
            }
        }),
    );
    assert!(output.status.success());
    let response = json_stdout(&output);
    let schema = compile_contract_ref("policy.schema.json", "PolicyEvaluateResponse");
    assert_valid(&schema, &response);

    let result = &response["result"];
    assert_eq!(result["accepted"], false);
    assert!(result["argv"]
        .as_array()
        .unwrap()
        .contains(&json!("--append-system-prompt")));
    assert!(result["argv"]
        .as_array()
        .unwrap()
        .contains(&json!("--allowed-tools")));
    assert!(result["argv"]
        .as_array()
        .unwrap()
        .contains(&json!("--disable-slash-commands")));
    assert_eq!(result["prompt"], "hello");
    let diagnostics = result["diagnostics"].as_array().unwrap();
    assert!(diagnostics
        .iter()
        .any(|item| item["code"] == "duplicate_claude_tool_filter"));
    assert!(diagnostics
        .iter()
        .any(|item| item["code"] == "unsafe_proxy_claude_tools_restrict"));
}

#[test]
fn terminal_classify_matches_claude_exit_vocabulary_without_quota_substrings() {
    let output = invoke(
        "terminal.classify",
        json!({
            "stdout_base64": "",
            "stderr_base64": "Q2xhdWRlIHVzYWdlIGxpbWl0IHJlYWNoZWQ=",
            "status": { "kind": "exited", "code": 0 },
            "observed_at_unix_ms": 1234
        }),
    );
    assert!(output.status.success());
    let response = json_stdout(&output);
    let schema = compile_contract_ref("terminal.schema.json", "TerminalClassifyResponse");
    assert_valid(&schema, &response);
    assert_eq!(response["result"]["terminal_signal"]["kind"], "clean_exit");
    assert_eq!(
        response["result"]["terminal_signal"]["evidence"],
        "exit_code=0"
    );
}

#[test]
fn launch_stream_preserves_stdout_and_stderr_bytes() {
    let output = invoke(
        "launch",
        json!({
            "settings_id": "claude-primary",
            "mode": "headless",
            "model": {
                "name": "claude-sonnet",
                "provider_args": [],
                "inputs": { "prompt": null, "named": {} }
            },
            "argv": [
                "/bin/sh",
                "-c",
                "printf '\\000\\001\\377'; printf 'err\\377\\376' >&2"
            ],
            "working_directory": "/tmp",
            "env": {}
        }),
    );
    assert!(output.status.success());
    assert!(
        output.stderr.is_empty(),
        "launch provider diagnostics must stay off stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let lines = String::from_utf8(output.stdout).unwrap();
    let events = lines
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).unwrap())
        .collect::<Vec<_>>();
    let stdout_schema = compile_contract_ref("launch.schema.json", "LaunchStdoutEvent");
    let stderr_schema = compile_contract_ref("launch.schema.json", "LaunchStderrEvent");
    let exit_schema = compile_contract_ref("launch.schema.json", "LaunchExitEvent");
    let stdout = events
        .iter()
        .find(|event| event["kind"] == "stdout")
        .expect("stdout event");
    let stderr = events
        .iter()
        .find(|event| event["kind"] == "stderr")
        .expect("stderr event");
    let exit = events.last().expect("exit event");
    assert_valid(&stdout_schema, stdout);
    assert_valid(&stderr_schema, stderr);
    assert_valid(&exit_schema, exit);

    assert_eq!(stdout["data_base64"], "AAH/");
    assert_eq!(stderr["data_base64"], "ZXJy//4=");
    assert_eq!(exit["status"], json!({ "kind": "exited", "code": 0 }));
    assert_eq!(exit["terminal_signal"]["kind"], "clean_exit");
}

#[test]
fn launch_stream_reports_session_marker_and_nonzero_exit() {
    let output = invoke(
        "launch",
        json!({
            "settings_id": "claude-primary",
            "mode": "headless",
            "model": {
                "name": "claude-sonnet",
                "provider_args": [],
                "inputs": { "prompt": null, "named": {} }
            },
            "argv": ["/bin/sh", "-c", "exit 7"],
            "working_directory": "/tmp",
            "env": {},
            "session": { "provider_session_id": "known-session" }
        }),
    );
    assert!(output.status.success());
    let lines = String::from_utf8(output.stdout).unwrap();
    let events = lines
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).unwrap())
        .collect::<Vec<_>>();

    let marker_schema = compile_contract_ref("launch.schema.json", "LaunchMarkerEvent");
    let exit_schema = compile_contract_ref("launch.schema.json", "LaunchExitEvent");
    assert_valid(&marker_schema, &events[0]);
    assert_valid(&exit_schema, &events[1]);
    assert_eq!(events[0]["kind"], "marker");
    assert_eq!(events[0]["name"], "provider_session_known");
    assert_eq!(events[1]["status"], json!({ "kind": "exited", "code": 7 }));
    assert_eq!(events[1]["terminal_signal"]["kind"], "nonzero_exit");
    assert_eq!(events[1]["session"]["provider_session_id"], "known-session");
}

#[test]
fn launch_stream_rejects_invalid_stdin_base64_before_spawn() {
    let output = invoke(
        "launch",
        json!({
            "settings_id": "claude-primary",
            "mode": "headless",
            "model": {
                "name": "claude-sonnet",
                "provider_args": [],
                "inputs": { "prompt": null, "named": {} }
            },
            "argv": ["/bin/sh", "-c", "printf should-not-run"],
            "working_directory": "/tmp",
            "env": {},
            "stdin": { "encoding": "base64", "data": "@@not-base64@@" }
        }),
    );
    assert!(output.status.success());
    let lines = String::from_utf8(output.stdout).unwrap();
    let events = lines
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).unwrap())
        .collect::<Vec<_>>();

    let exit_schema = compile_contract_ref("launch.schema.json", "LaunchExitEvent");
    assert_eq!(events.len(), 1);
    assert_valid(&exit_schema, &events[0]);
    assert_eq!(events[0]["status"]["kind"], "spawn_error");
    assert_eq!(events[0]["terminal_signal"]["kind"], "spawn_error");
}
