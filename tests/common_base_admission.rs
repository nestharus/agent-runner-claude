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

fn invalid(operation: &str, request: &Value) {
    let (status, response) = invoke(operation, request);
    assert_eq!(status, 2, "{operation}: {response}");
    assert_eq!(response["ok"], false);
    assert_eq!(response["error"]["category"], "invalid_request");
    assert!(response.get("result").is_none());
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
        invalid(operation, &request(json!({})));
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
    invalid("launch", &value);
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
