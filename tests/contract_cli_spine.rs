// declared_role: orchestration, mapper, parser, validator
// intrinsic_surface_declarations:
//   - component: tests/contract_cli_spine.rs
//     role: intrinsic-surface
//     Domain: contract_cli_spine_proof_surface
//     Owns:
//       - CLI spine contract scenarios
//       - support harness dependencies for provider invocation/schema proof

mod support;

use serde_json::{json, Value};
use support::fixtures::{envelope, host_context, temp_roots, CONTRACT};
use support::invoke::{
    invoke, invoke_with_args_and_stdin_bytes, invoke_with_stdin_bytes, parse_one_stdout_json,
};
use support::schema::assert_valid;

fn assert_error_response(value: &Value, expected_category: &str) {
    assert_valid("common.schema.json#/$defs/ErrorResponseEnvelope", value);
    assert!(!value["ok"].as_bool().unwrap());
    assert_eq!(value["error"]["category"], expected_category);
    assert!(!value["error"]["message"].as_str().unwrap().is_empty());
}

#[test]
fn invalid_stdin_json_returns_schema_valid_invalid_request_error() {
    let output = invoke_with_stdin_bytes(Some("describe"), b"{");
    assert_eq!(output.code, Some(2));
    assert!(output.stderr.is_empty());
    let response = parse_one_stdout_json(&output);
    assert_error_response(&response, "invalid_request");
}

#[test]
fn invalid_utf8_stdin_returns_schema_valid_invalid_request_error() {
    let output = invoke_with_stdin_bytes(Some("describe"), &[0xff, 0xfe, b'{']);
    assert_eq!(output.code, Some(2));
    assert!(output.stderr.is_empty());
    let response = parse_one_stdout_json(&output);
    assert_error_response(&response, "invalid_request");
}

#[test]
fn wrong_contract_returns_schema_valid_unsupported_error() {
    let roots = temp_roots("wrong-contract");
    let request = envelope("oulipoly.provider/v0", host_context(&roots), json!({}));
    let output = invoke("describe", &request);
    assert_eq!(output.code, Some(3));
    assert!(output.stderr.is_empty());
    let response = parse_one_stdout_json(&output);
    assert_error_response(&response, "unsupported");
}

#[test]
fn omitted_host_returns_schema_valid_invalid_request_error() {
    let request = json!({
        "contract": CONTRACT,
        "request_id": "missing-host-req",
        "provider_instance_id": "claude-primary",
        "params": {}
    });
    let output = invoke("describe", &request);
    assert_eq!(output.code, Some(2));
    assert!(output.stderr.is_empty());
    let response = parse_one_stdout_json(&output);
    assert_error_response(&response, "invalid_request");
}

#[test]
fn omitted_params_request_returns_schema_valid_invalid_request_error() {
    let roots = temp_roots("missing-params-request");
    let request = json!({
        "contract": CONTRACT,
        "request_id": "missing-params-req",
        "provider_instance_id": "claude-primary",
        "host": host_context(&roots)
    });
    let output = invoke("describe", &request);
    assert_eq!(output.code, Some(2));
    assert!(output.stderr.is_empty());
    let response = parse_one_stdout_json(&output);
    assert_error_response(&response, "invalid_request");
}

#[test]
fn missing_argv_subcommand_returns_schema_valid_unsupported_error() {
    let roots = temp_roots("missing-subcommand");
    let request = envelope(CONTRACT, host_context(&roots), json!({}));
    let output = invoke_with_args_and_stdin_bytes(&[], request.to_string().as_bytes());
    assert_eq!(output.code, Some(3));
    assert!(output.stderr.is_empty());
    let response = parse_one_stdout_json(&output);
    assert_error_response(&response, "unsupported");
}

#[test]
fn extra_argv_selector_returns_schema_valid_unsupported_error() {
    let roots = temp_roots("extra-argv");
    let request = envelope(CONTRACT, host_context(&roots), json!({}));
    let output =
        invoke_with_args_and_stdin_bytes(&["describe", "extra"], request.to_string().as_bytes());
    assert_eq!(output.code, Some(3));
    assert!(output.stderr.is_empty());
    let response = parse_one_stdout_json(&output);
    assert_error_response(&response, "unsupported");
}

#[test]
fn unknown_subcommand_returns_schema_valid_unsupported_error() {
    let roots = temp_roots("unknown-subcommand");
    let request = envelope(CONTRACT, host_context(&roots), json!({}));
    let output = invoke("no.such_capability", &request);
    assert_eq!(output.code, Some(3));
    assert!(output.stderr.is_empty());
    let response = parse_one_stdout_json(&output);
    assert_error_response(&response, "unsupported");
}

#[test]
fn malformed_params_returns_schema_valid_invalid_request_error() {
    let roots = temp_roots("malformed-params");
    let request = envelope(
        CONTRACT,
        host_context(&roots),
        json!({ "unexpected": true }),
    );
    let output = invoke("describe", &request);
    assert_eq!(output.code, Some(2));
    assert!(output.stderr.is_empty());
    let response = parse_one_stdout_json(&output);
    assert_error_response(&response, "invalid_request");
}

#[test]
fn valid_non_launch_command_returns_one_json_value_exit_zero_empty_stderr() {
    let roots = temp_roots("valid-describe");
    let request = envelope(CONTRACT, host_context(&roots), json!({}));
    let output = invoke("describe", &request);
    assert_eq!(output.code, Some(0));
    assert!(output.stderr.is_empty());
    let response = parse_one_stdout_json(&output);
    assert_valid("describe.schema.json#/$defs/DescribeResponse", &response);
    assert!(response["ok"].as_bool().unwrap());
}
