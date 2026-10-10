//! Intent: SDK provider/v1 common.schema.json requires nonempty (not trimmed)
//! IDs, forbids extra envelope/host keys and null optional host/instance fields.
//! describe.schema.json requires an empty object; operation params are bound
//! to their operation before any native effect. These are boundary assertions,
//! not expectations derived from the Claude parser.
use agent_runner_claude::write_invocation;
use serde_json::{json, Value};

fn request(params: Value) -> Value {
    json!({
        "contract": agent_provider_contract::CONTRACT_VERSION,
        "request_id": "common-admission",
        "host": { "app": "contract-test" },
        "params": params
    })
}

fn invoke(operation: &str, request: &Value) -> (i32, Value) {
    let mut output = Vec::new();
    let status = write_invocation(
        &["agent-runner-claude".into(), operation.into()],
        &request.to_string(),
        &mut output,
    );
    (
        status,
        serde_json::from_slice(&output).expect("one response"),
    )
}

fn invalid(operation: &str, request: &Value) -> Value {
    let (status, response) = invoke(operation, request);
    assert_eq!(status, 2, "{operation}: {response}");
    assert_eq!(response["ok"], false);
    assert_eq!(response["error"]["category"], "invalid_request");
    assert!(response.get("result").is_none());
    assert_eq!(
        response["request_id"],
        request["request_id"]
            .as_str()
            .filter(|id| !id.is_empty())
            .unwrap_or("unknown")
    );
    assert_eq!(response["error"]["retryable"], false);
    assert!(matches!(
        response["error"]["code"].as_str(),
        Some("invalid_envelope" | "invalid_params")
    ));
    response
}

#[test]
fn describe_requires_empty_object_params() {
    for params in [
        json!(null),
        json!(false),
        json!("text"),
        json!([]),
        json!({"ignored": true}),
    ] {
        invalid("describe", &request(params));
    }
    let (status, response) = invoke("describe", &request(json!({})));
    assert_eq!(status, 0);
    assert_eq!(response["ok"], true);
    assert_eq!(response["result"]["provider_id"], "claude");
}

#[test]
fn common_ids_have_length_not_trim_constraints() {
    for (id, app) in [(" ", " \t"), ("normal", "app")] {
        let mut value = request(json!({}));
        value["request_id"] = json!(id);
        value["host"]["app"] = json!(app);
        let (status, response) = invoke("describe", &value);
        assert_eq!(status, 0, "{response}");
        assert_eq!(response["request_id"], id, "do not rewrite valid identity");
    }
    for (id, app) in [("", "app"), ("id", "")] {
        let mut value = request(json!({}));
        value["request_id"] = json!(id);
        value["host"]["app"] = json!(app);
        invalid("describe", &value);
    }
}

#[test]
fn common_envelope_checks_presence_unknown_fields_and_optional_shapes() {
    let base = request(json!({}));
    for key in ["contract", "request_id", "host", "params"] {
        let mut value = base.clone();
        value.as_object_mut().unwrap().remove(key);
        invalid("describe", &value);
    }
    let mut values = Vec::new();
    let mut extra = base.clone();
    extra["private"] = json!(true);
    values.push(extra);
    let mut extra = base.clone();
    extra["host"]["private"] = json!(true);
    values.push(extra);
    for instance in [json!(null), json!(""), json!(3)] {
        let mut value = base.clone();
        value["provider_instance_id"] = instance;
        values.push(value);
    }
    for (key, malformed) in [
        ("env", json!(null)),
        ("env", json!({"KEY": 3})),
        ("platform", json!(null)),
        ("deadline_unix_ms", json!(-1)),
    ] {
        let mut value = base.clone();
        value["host"][key] = malformed;
        values.push(value);
    }
    for value in values {
        invalid("describe", &value);
    }
    let mut valid = base;
    valid["provider_instance_id"] = json!("instance");
    valid["host"]["env"] = json!({});
    valid["host"]["deadline_unix_ms"] = json!(0);
    assert_eq!(invoke("describe", &valid).0, 0);
}

#[test]
fn operation_admission_is_not_just_common_envelope_admission() {
    // These params are valid JSON and fit the common envelope, but not each
    // operation's required fields/types. No native/session/quota work is owed.
    for operation in [
        "schema",
        "policy.evaluate",
        "launch",
        "terminal.classify",
        "quota.source",
        "quota.probe",
        "quota.refresh_auth",
        "session.locate_transcript",
        "session.capture",
        "session.export",
        "session.replace",
    ] {
        assert_eq!(
            invalid(operation, &request(json!({})))["error"]["code"],
            "invalid_params"
        );
    }
    let (_, result) = invoke(
        "schema",
        &request(json!({"schema_id":"claude.settings/v1"})),
    );
    assert_eq!(result["ok"], true);
    invalid(
        "schema",
        &request(json!({"schema_id":"claude.settings/v1", "extra": 1})),
    );
    let (status, result) = invoke("schema", &request(json!({"schema_id":"other/v1"})));
    assert_eq!(status, 1); // structurally valid, adapter-owned schema selection
    assert_eq!(result["error"]["code"], "unknown_schema");
}

#[test]
fn extensions_and_unknown_commands_still_obey_the_common_envelope() {
    for operation in [
        "resident.prepare",
        "unknown.operation",
        "session.read_turns",
    ] {
        let mut value = request(json!({}));
        value["host"]["app"] = json!("");
        invalid(operation, &value);
    }
    assert_eq!(
        invoke("unknown.operation", &request(json!({}))).1["error"]["code"],
        "unsupported_subcommand"
    );
    let mut value = request(json!({}));
    value["contract"] = json!("oulipoly.provider/v2");
    let (status, response) = invoke("describe", &value);
    assert_eq!(status, 3);
    assert_eq!(response["error"]["code"], "unsupported_version");
}

#[test]
fn invalid_launch_has_no_effect_and_valid_launch_keeps_existing_identity() {
    let root = tempfile::tempdir().unwrap();
    let marker = root.path().join("effect");
    let params = json!({
        "settings_id": "fixture", "mode": "headless",
        "model": {"name":"fake", "provider_args":[], "inputs":{"named":{}}},
        "argv": ["/bin/sh", "-c", "printf done > \"$1\"", "fake", marker],
        "working_directory": root.path(), "env": {}
    });
    let mut value = request(params);
    value["host"]["data_root"] = json!(root.path());
    value["params"]["extra"] = json!(true);
    assert_eq!(invalid("launch", &value)["error"]["code"], "invalid_params");
    assert!(!marker.exists());
    assert!(!root.path().join("provider-state").exists());
    value["params"].as_object_mut().unwrap().remove("extra");

    let mut bytes = Vec::new();
    let args = ["agent-runner-claude".into(), "launch".into()];
    assert_eq!(write_invocation(&args, &value.to_string(), &mut bytes), 0);
    assert_eq!(std::fs::read_to_string(&marker).unwrap(), "done");
    let records: Vec<Value> = String::from_utf8(bytes.clone())
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(records.last().unwrap()["kind"], "exit");
    // Completed replay must not start the shell a second time.
    std::fs::remove_file(&marker).unwrap();
    let mut replay = Vec::new();
    assert_eq!(write_invocation(&args, &value.to_string(), &mut replay), 0);
    assert!(!marker.exists());
    assert_eq!(replay, bytes);
    // A changed, schema-valid request under the same ID cannot replay the old
    // result or perform a new native effect.
    value["params"]["env"] = json!({"CHANGED":"1"});
    let (status, response) = invoke("launch", &value);
    assert_eq!(status, 2);
    assert_eq!(response["error"]["code"], "request_changed");
    assert!(!marker.exists());
}

// Fake credential-adjacent values only. The independent oracle is the decision
// to omit submitted value bodies, not the validator's chosen prose or order.
#[test]
fn malformed_admission_errors_are_bounded_correlated_and_do_not_echo_values() {
    let root = tempfile::tempdir().unwrap();
    let sentinel = "MAKER_SENTINEL_NEVER_REAL";
    let large = format!("{sentinel}{}", "x".repeat(65536));
    let mut envelope = request(json!({}));
    envelope["host"]["env"] = json!({"FAKE_ENV": {"FAKE_VALUE": large}});
    let mut launch = request(json!({
        "settings_id":"fixture", "mode":"headless",
        "model":{"name":"fake", "provider_args":[], "inputs":{"named":{}}},
        "argv":[{"FAKE_ARGV":large}], "env":{"FAKE_ENV":large}
        // Also missing working_directory: a second violation must not expand
        // or value-echo the diagnostic.
    }));
    launch["host"]["data_root"] = json!(root.path());
    for (operation, value, code, boundary) in [
        ("describe", envelope, "invalid_envelope", "RequestEnvelope"),
        ("launch", launch, "invalid_params", "launch"),
    ] {
        let response = invalid(operation, &value);
        assert_eq!(response["error"]["code"], code);
        let message = response["error"]["message"].as_str().unwrap();
        assert!(message.len() <= 200, "bounded diagnostic");
        assert!(
            message.contains(boundary),
            "identify the trusted boundary: {message}"
        );
        assert!(!response.to_string().contains(sentinel));
        assert_eq!(response["error"]["details"], json!({}));
        // Launch is request-only/NDJSON; use the shared common error envelope,
        // not the nonexistent launch single-response binding (U534 limit).
        agent_provider_contract::SchemaRegistry::new()
            .validate_error_response("describe", &response)
            .unwrap();
    }
    assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 0);
}

#[test]
fn unsupported_version_and_json_diagnostics_omit_submitted_bodies() {
    let mut value = request(json!({}));
    value["contract"] = json!(format!("FAKE_VERSION_BODY{}", "x".repeat(65536)));
    let (status, response) = invoke("describe", &value);
    assert_eq!(status, 3);
    assert_eq!(response["request_id"], "common-admission");
    assert_eq!(response["error"]["code"], "unsupported_version");
    assert!(!response.to_string().contains("FAKE_VERSION_BODY"));
    assert!(response["error"]["message"].as_str().unwrap().len() <= 200);
    let mut output = Vec::new();
    assert_eq!(
        write_invocation(
            &["provider".into(), "describe".into()],
            "{\"FAKE_JSON_BODY\":invalid}",
            &mut output
        ),
        2
    );
    let response: Value = serde_json::from_slice(&output).unwrap();
    assert_eq!(response["request_id"], "unknown"); // no admitted JSON identity
    assert_eq!(response["error"]["code"], "invalid_json");
    assert!(!response.to_string().contains("FAKE_JSON_BODY"));
    assert!(response["error"]["message"]
        .as_str()
        .unwrap()
        .contains("column"));
}
