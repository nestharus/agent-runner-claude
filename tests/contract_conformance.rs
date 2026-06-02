use jsonschema::{Draft, JSONSchema};
use serde_json::{json, Value};
use std::io::Write;
use std::process::{Command, Output, Stdio};

const CONTRACT: &str = "oulipoly.provider/v1";

fn invoke(subcommand: &str, params: Value) -> Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_agent-runner-claude"))
        .arg(subcommand)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let request = json!({
        "contract": CONTRACT,
        "request_id": format!("req-{subcommand}"),
        "provider_instance_id": "claude-primary",
        "host": {
            "app": "oulipoly-agent-runner",
            "app_version": "0.0.0",
            "platform": "linux-x86_64",
            "working_directory": "/tmp",
            "config_root": "/tmp/config",
            "data_root": "/tmp/data",
            "env": { "TERM": "xterm-256color" }
        },
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
        "launch.schema.json" => include_str!("../contract/v1/launch.schema.json"),
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
    for key in ["launch", "policy", "terminal"] {
        assert_eq!(result["capabilities"][key], true, "capability {key}");
    }
    for key in [
        "quota",
        "session",
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
    let output = invoke("quota.source", json!({}));
    assert_eq!(output.status.code(), Some(3));
    let response = json_stdout(&output);
    let schema = compile_contract_ref("common.schema.json", "ErrorResponseEnvelope");
    assert_valid(&schema, &response);
    assert_eq!(response["error"]["code"], "capability_not_implemented");
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
