// declared_role: orchestration, mapper, formatter, validator, predicate, parser

mod support;

use serde_json::{json, Value};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use support::fixtures::{envelope, host_context, temp_roots, TempRoots, CONTRACT};
use support::invoke::{invoke, parse_one_stdout_json, Invocation};
use support::schema::assert_valid;

const SECRET: &str = "DISCOVERY_SECRET_SENTINEL_W2C_DO_NOT_LEAK";

fn host_with_path(roots: &TempRoots, path: &Path) -> Value {
    let mut host = host_context(roots);
    let env = host["env"].as_object_mut().unwrap();
    env.insert("PATH".to_string(), json!(path.display().to_string()));
    host
}

fn call(
    roots: &TempRoots,
    subcommand: &str,
    params: Value,
    path: Option<&Path>,
) -> (Value, Vec<u8>, Vec<u8>) {
    let request = discovery_request(roots, params, path);
    let output = provider_invocation(subcommand, &request);
    assert_success_output(&output);
    let response = stdout_response(&output);
    call_result(response, output)
}

fn call_error(roots: &TempRoots, subcommand: &str, params: Value, schema: &str) -> Value {
    let request = discovery_request(roots, params, None);
    let output = provider_invocation(subcommand, &request);
    assert_error_output(&output);
    let response = stdout_response(&output);
    assert_error_response(schema, &response);
    response
}

fn discovery_request(roots: &TempRoots, params: Value, path: Option<&Path>) -> Value {
    envelope(CONTRACT, discovery_host(roots, path), params)
}

fn discovery_host(roots: &TempRoots, path: Option<&Path>) -> Value {
    path.map_or_else(|| host_context(roots), |path| host_with_path(roots, path))
}

fn provider_invocation(subcommand: &str, request: &Value) -> Invocation {
    invoke(subcommand, request)
}

fn stdout_response(output: &Invocation) -> Value {
    parse_one_stdout_json(output)
}

fn call_result(response: Value, output: Invocation) -> (Value, Vec<u8>, Vec<u8>) {
    (response, output.stdout, output.stderr)
}

fn assert_success_output(output: &Invocation) {
    assert_success_code(output);
    assert_empty_stderr(output);
}

fn assert_success_code(output: &Invocation) {
    assert_eq!(output.code, Some(0));
}

fn assert_error_output(output: &Invocation) {
    assert_error_code(output);
    assert_empty_stderr(output);
}

fn assert_error_code(output: &Invocation) {
    assert_eq!(output.code, Some(2));
}

fn assert_empty_stderr(output: &Invocation) {
    assert!(output.stderr.is_empty());
}

fn assert_error_response(schema: &str, response: &Value) {
    assert_valid(schema, response);
    assert_error_unsuccessful(response);
    assert_error_category(response, "invalid_request");
}

fn assert_error_unsuccessful(response: &Value) {
    assert!(!response["ok"].as_bool().unwrap());
}

fn assert_error_category(response: &Value, category: &str) {
    assert_eq!(response["error"]["category"], category);
}

fn make_probe_trap(bin_dir: &Path, log_path: &Path) {
    fs::create_dir_all(bin_dir).expect("create fake bin dir");
    let claude = bin_dir.join("claude");
    fs::write(&claude, probe_trap_script(log_path)).expect("write fake claude");
    make_executable(&claude);
}

fn probe_trap_script(log_path: &Path) -> String {
    format!(
        "#!/bin/sh\nprintf '%s\\n' \"$@\" > '{}'
exit 17\n",
        log_path.display()
    )
}

fn make_executable(path: &Path) {
    let mut perms = fs::metadata(path).unwrap().permissions();
    perms.set_mode(0o755);
    fs::set_permissions(path, perms).unwrap();
}

fn assert_no_secret(value: &Value, stdout: &[u8], stderr: &[u8]) {
    let value_text = value.to_string();
    assert!(
        !value_text.contains(SECRET),
        "secret leaked in response: {value_text}"
    );
    assert!(!String::from_utf8_lossy(stdout).contains(SECRET));
    assert!(!String::from_utf8_lossy(stderr).contains(SECRET));
}

fn has_string(value: &Value, needle: &str) -> bool {
    match value {
        Value::String(text) => text.contains(needle),
        Value::Array(items) => items.iter().any(|item| has_string(item, needle)),
        Value::Object(map) => map.values().any(|item| has_string(item, needle)),
        _ => false,
    }
}

#[test]
fn discovery_models_are_hardcoded_claude_aliases_with_metadata_and_no_cli_probe() {
    let roots = temp_roots("discovery-models");
    let bin_dir = roots.root.join("bin");
    let probe_log = roots.root.join("claude-probe.log");
    make_probe_trap(&bin_dir, &probe_log);

    let (response, stdout, stderr) = call(
        &roots,
        "discovery.models",
        json!({ "provider": "claude", "include_availability": true }),
        Some(&bin_dir),
    );
    assert_valid(
        "discovery.schema.json#/$defs/DiscoveryModelsResponse",
        &response,
    );
    assert!(response["ok"].as_bool().unwrap());

    let result = &response["result"];
    let models = result["models"].as_array().unwrap();
    assert!(!models.is_empty());
    for model in models {
        assert!(
            has_string(model, "hardcoded") || model.get("source").is_some(),
            "model missing source metadata: {model}"
        );
        assert!(
            !has_string(model, "available:true"),
            "model availability must not be CLI-probed: {model}"
        );
    }
    assert!(!result["warnings"].as_array().unwrap().is_empty());
    assert!(
        has_string(&result["warnings"], "not") || has_string(&result["warnings"], "availability")
    );
    assert!(
        !probe_log.exists(),
        "discovery.models must not invoke claude CLI for availability"
    );
    assert_no_secret(&response, &stdout, &stderr);
}

#[test]
fn discovery_models_malformed_request_uses_capability_error_def() {
    let roots = temp_roots("discovery-models-malformed-request");

    call_error(
        &roots,
        "discovery.models",
        json!(null),
        "discovery.schema.json#/$defs/DiscoveryModelsErrorResponse",
    );
}

#[test]
fn discovery_accounts_reports_auth_state_and_display_metadata_without_secret_material() {
    let roots = temp_roots("discovery-accounts");
    install_discovery_account_fixtures(&roots);

    let (response, stdout, stderr) = call(&roots, "discovery.accounts", json!({}), None);
    assert_valid(
        "discovery.schema.json#/$defs/DiscoveryAccountsResponse",
        &response,
    );
    assert!(response["ok"].as_bool().unwrap());
    assert_no_secret(&response, &stdout, &stderr);

    assert_discovery_accounts_metadata(&response["result"]);
}

fn install_discovery_account_fixtures(roots: &TempRoots) {
    let claude_home = roots.home.join(".claude");
    fs::create_dir_all(&claude_home).expect("create claude home");
    fs::write(claude_home.join("credentials.json"), credentials_fixture())
        .expect("write credentials fixture");
    fs::write(roots.home.join(".claude.json"), profile_fixture()).expect("write profile fixture");
}

fn credentials_fixture() -> String {
    format!(
        r#"{{"account_id":"acct-work","display_name":"Work Claude","email":"work@example.test","api_key":"{SECRET}","refresh_token":"{SECRET}"}}"#
    )
}

fn profile_fixture() -> String {
    format!(r#"{{"profiles":{{"work":{{"display_name":"Work Profile","token":"{SECRET}"}}}}}}"#)
}

fn assert_discovery_accounts_metadata(result: &Value) {
    assert!(!result["accounts"].as_array().unwrap().is_empty());
    assert!(
        has_string(&result["accounts"], "authenticated") || has_string(&result["accounts"], "auth")
    );
    assert!(
        has_string(&result["accounts"], "Work Claude")
            || has_string(&result["accounts"], "work@example.test")
    );
    assert!(!has_string(&result["accounts"], "api_key"));
    assert!(!has_string(&result["accounts"], "refresh_token"));
    assert!(!has_string(&result["accounts"], "token"));
}

#[test]
fn discovery_accounts_without_metadata_does_not_fabricate_default_account() {
    let roots = temp_roots("discovery-accounts-no-metadata");

    let (response, stdout, stderr) = call(&roots, "discovery.accounts", json!({}), None);

    assert_valid(
        "discovery.schema.json#/$defs/DiscoveryAccountsResponse",
        &response,
    );
    assert!(response["ok"].as_bool().unwrap());
    assert_no_secret(&response, &stdout, &stderr);
    assert!(response["result"]["accounts"]
        .as_array()
        .unwrap()
        .is_empty());
}

#[test]
fn discovery_accounts_malformed_request_uses_capability_error_def() {
    let roots = temp_roots("discovery-accounts-malformed-request");

    call_error(
        &roots,
        "discovery.accounts",
        json!(null),
        "discovery.schema.json#/$defs/DiscoveryAccountsErrorResponse",
    );
}
