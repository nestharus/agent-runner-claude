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
    for key in [
        "launch",
        "policy",
        "quota",
        "session",
        "terminal",
        "rotation",
        "discovery",
        "settings",
        "setup_brain",
        "setup",
        "migration",
    ] {
        assert_eq!(result["capabilities"][key], true, "capability {key}");
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
    let output = invoke("policy.evaluate", json!({}));
    assert_eq!(output.status.code(), Some(3));
    let response = json_stdout(&output);
    let schema = compile_contract_ref("common.schema.json", "ErrorResponseEnvelope");
    assert_valid(&schema, &response);
    assert_eq!(response["error"]["code"], "capability_not_implemented");
}
