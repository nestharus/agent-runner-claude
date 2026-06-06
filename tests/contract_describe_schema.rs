// declared_role: validator

mod support;

use jsonschema::{Draft, JSONSchema};
use serde_json::json;
use support::fixtures::{envelope, host_context, temp_roots, CONTRACT};
use support::invoke::{invoke, parse_one_stdout_json};
use support::schema::{assert_valid, compile_arbitrary_schema};

#[test]
fn describe_advertises_claude_identity_contract_and_all_wave1_capabilities() {
    let roots = temp_roots("describe");
    let request = envelope(CONTRACT, host_context(&roots), json!({}));
    let output = invoke("describe", &request);
    assert_eq!(output.code, Some(0));
    assert!(output.stderr.is_empty());
    let response = parse_one_stdout_json(&output);
    assert_valid("describe.schema.json#/$defs/DescribeResponse", &response);

    let result = &response["result"];
    assert_eq!(result["provider_id"], "claude");
    assert_eq!(result["display_name"], "Claude Code");
    assert_eq!(result["preferred_contract"], CONTRACT);
    assert!(result["contract_versions"]
        .as_array()
        .unwrap()
        .contains(&json!(CONTRACT)));
    assert_eq!(result["settings_schema_id"], "claude.settings/v1");

    for capability in [
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
        assert_eq!(
            result["capabilities"][capability], true,
            "capability {capability}"
        );
    }
}

#[test]
fn schema_serves_only_claude_settings_v1_and_returned_schema_compiles() {
    let roots = temp_roots("schema-ok");
    let request = envelope(
        CONTRACT,
        host_context(&roots),
        json!({ "schema_id": "claude.settings/v1" }),
    );
    let output = invoke("schema", &request);
    assert_eq!(output.code, Some(0));
    assert!(output.stderr.is_empty());
    let response = parse_one_stdout_json(&output);
    assert_valid("schema.schema.json#/$defs/SchemaResponse", &response);

    assert_eq!(response["result"]["schema_id"], "claude.settings/v1");
    let schema = &response["result"]["schema"];
    compile_arbitrary_schema(schema).expect("settings schema must compile");
    let compiled = JSONSchema::options()
        .with_draft(Draft::Draft202012)
        .compile(schema)
        .expect("settings schema must compile for validation");
    assert!(compiled.validate(&json!({ "command": "claude" })).is_ok());
    for reopened_shape in [
        json!({}),
        json!({ "command": "claude", "unexpected": true }),
        json!({ "command": "claude", "tool_restrictions": { "kind": "claude", "extra": true } }),
        json!({ "command": "claude", "tool_restrictions": { "kind": "claude", "claude": { "allowed_tools": [""] } } }),
        json!({ "command": "claude", "session_storage": { "kind": "claude_code" } }),
        json!({ "command": "claude", "session_storage": { "kind": "script", "cwd_script": "cwd", "transcript_script": "transcript" } }),
    ] {
        assert!(
            compiled.validate(&reopened_shape).is_err(),
            "settings schema must reject reopened shape: {reopened_shape}"
        );
    }
    let properties = schema["properties"]
        .as_object()
        .expect("settings schema properties");
    for field in [
        "command",
        "args",
        "api_key",
        "auth_token",
        "tool_restrictions",
        "quota_script",
        "auth_refresh_command",
        "session_storage",
        "setup_brain_model",
    ] {
        assert!(
            properties.contains_key(field),
            "missing settings field {field}"
        );
    }
    for secret_field in ["api_key", "auth_token"] {
        let secret_schema = properties.get(secret_field).unwrap();
        assert_eq!(
            secret_schema["writeOnly"], true,
            "{secret_field} must be writeOnly"
        );
        assert!(
            secret_schema.get("default").is_none(),
            "{secret_field} must not declare a default"
        );
    }

    let unknown = envelope(
        CONTRACT,
        host_context(&roots),
        json!({ "schema_id": "unknown.settings/v1" }),
    );
    let output = invoke("schema", &unknown);
    assert_eq!(output.code, Some(3));
    assert!(output.stderr.is_empty());
    let response = parse_one_stdout_json(&output);
    assert_valid("schema.schema.json#/$defs/SchemaErrorResponse", &response);
    assert!(!response["ok"].as_bool().unwrap());
    assert_eq!(response["error"]["category"], "unsupported");
}

#[test]
fn describe_malformed_params_uses_capability_error_def() {
    let roots = temp_roots("describe-malformed");
    let request = envelope(
        CONTRACT,
        host_context(&roots),
        json!({ "unexpected": true }),
    );

    let output = invoke("describe", &request);

    assert_eq!(output.code, Some(2));
    assert!(output.stderr.is_empty());
    let response = parse_one_stdout_json(&output);
    assert_valid(
        "describe.schema.json#/$defs/DescribeErrorResponse",
        &response,
    );
    assert!(!response["ok"].as_bool().unwrap());
    assert_eq!(response["error"]["category"], "invalid_request");
}
