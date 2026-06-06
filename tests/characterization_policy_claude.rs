// declared_role: orchestration, accessor, filter, mapper, validator

mod support;

use serde_json::{json, Value};
use support::assertions::assert_successful_invocation;
use support::fixtures::temp_roots;
use support::invoke::{invoke, parse_one_stdout_json};
use support::requests::policy_evaluate_request;
use support::schema::assert_valid;

fn policy_with(model_prompt: Value, launch: Value) -> Value {
    let roots = temp_roots("policy-characterization");
    let request = policy_evaluate_request(&roots, &[], model_prompt, launch);
    let output = invoke("policy.evaluate", &request);
    assert_successful_invocation(&output);
    let response = parse_one_stdout_json(&output);
    assert_valid(
        "policy.schema.json#/$defs/PolicyEvaluateResponse",
        &response,
    );
    response
}

fn codes(response: &Value) -> Vec<String> {
    diagnostic_code_values(response)
        .into_iter()
        .map(owned_code)
        .collect()
}

fn diagnostic_code_values(response: &Value) -> Vec<&str> {
    diagnostics_array(response)
        .iter()
        .filter_map(diagnostic_code_value)
        .collect()
}

fn diagnostics_array(response: &Value) -> &Vec<Value> {
    response["result"]["diagnostics"].as_array().unwrap()
}

fn diagnostic_code_value(diagnostic: &Value) -> Option<&str> {
    diagnostic["code"].as_str()
}

fn owned_code(code: &str) -> String {
    code.to_string()
}

#[test]
fn claude_append_system_prompt_projects_to_exact_argv_pair() {
    let response = policy_with(
        json!("prompt"),
        json!({
            "command": "claude",
            "args": [],
            "prompt_mode": "arg",
            "invocation_mode": "proxy",
            "system_prompt_override": "follow host policy"
        }),
    );
    assert_append_system_prompt_response(&response);
}

fn assert_append_system_prompt_response(response: &Value) {
    assert!(response["result"]["accepted"].as_bool().unwrap());
    assert_eq!(
        response["result"]["argv"],
        json!([
            "claude",
            "--append-system-prompt",
            "follow host policy",
            "prompt"
        ])
    );
}

#[test]
fn claude_allowed_and_disallowed_tools_are_projected_as_comma_joined_flags() {
    let allowed = policy_with(
        json!("prompt"),
        json!({
            "command": "claude",
            "args": [],
            "prompt_mode": "arg",
            "invocation_mode": "proxy",
            "tool_restrictions": {
                "kind": "claude",
                "claude": { "allowed_tools": ["Read", "Bash", "mcp__github__search"] }
            }
        }),
    );
    assert_allowed_tools_response(&allowed);

    let disallowed = policy_with(
        json!("prompt"),
        json!({
            "command": "claude",
            "args": [],
            "prompt_mode": "arg",
            "invocation_mode": "proxy",
            "tool_restrictions": {
                "kind": "claude",
                "claude": { "disallowed_tools": ["Write", "WebFetch"] }
            }
        }),
    );
    assert_disallowed_tools_response(&disallowed);
}

fn assert_allowed_tools_response(response: &Value) {
    assert_eq!(
        response["result"]["argv"],
        json!([
            "claude",
            "--allowed-tools",
            "Read,Bash,mcp__github__search",
            "prompt"
        ])
    );
}

fn assert_disallowed_tools_response(response: &Value) {
    assert_eq!(
        response["result"]["argv"],
        json!(["claude", "--disallowed-tools", "Write,WebFetch", "prompt"])
    );
}

#[test]
fn claude_disable_slash_commands_projects_to_flag_without_value() {
    let response = policy_with(
        json!("prompt"),
        json!({
            "command": "claude",
            "args": [],
            "prompt_mode": "arg",
            "invocation_mode": "proxy",
            "tool_restrictions": {
                "kind": "claude",
                "claude": { "disable_slash_commands": true }
            }
        }),
    );
    assert_disable_slash_commands_response(&response);
}

fn assert_disable_slash_commands_response(response: &Value) {
    assert_eq!(
        response["result"]["argv"],
        json!(["claude", "--disable-slash-commands", "prompt"])
    );
}

#[test]
fn claude_duplicate_detection_reports_existing_cli_policy_flags() {
    let response = policy_with(
        json!("prompt"),
        json!({
            "command": "claude --append-system-prompt existing --allowed-tools Read --disable-slash-commands",
            "args": [],
            "prompt_mode": "arg",
            "invocation_mode": "proxy",
            "system_prompt_override": "new policy",
            "tool_restrictions": {
                "kind": "claude",
                "claude": { "allowed_tools": ["Bash"], "disable_slash_commands": true }
            }
        }),
    );
    assert_duplicate_policy_diagnostics(&response);
}

fn assert_duplicate_policy_diagnostics(response: &Value) {
    assert!(!response["result"]["accepted"].as_bool().unwrap());
    let codes = codes(response);
    assert!(codes.contains(&"duplicate_claude_append_system_prompt".to_string()));
    assert!(codes.contains(&"duplicate_claude_allowed_tools".to_string()));
    assert!(codes.contains(&"duplicate_claude_disable_slash_commands".to_string()));
}

#[test]
fn claude_proxy_rejects_raw_tools_mcp_restriction_but_not_allowed_tools_mcp_names() {
    let rejected = policy_with(
        json!("prompt"),
        json!({
            "command": "claude",
            "args": ["--tools", "mcp__danger__run"],
            "prompt_mode": "arg",
            "invocation_mode": "proxy"
        }),
    );
    assert_proxy_rejected_response(&rejected);

    let permitted = policy_with(
        json!("prompt"),
        json!({
            "command": "claude",
            "args": [],
            "prompt_mode": "arg",
            "invocation_mode": "proxy",
            "tool_restrictions": {
                "kind": "claude",
                "claude": { "allowed_tools": ["mcp__safe__read"] }
            }
        }),
    );
    assert_proxy_permitted_response(&permitted);
}

fn assert_proxy_rejected_response(response: &Value) {
    assert!(!response["result"]["accepted"].as_bool().unwrap());
    assert!(codes(response).contains(&"unsafe_proxy_claude_tools_restrict".to_string()));
}

fn assert_proxy_permitted_response(response: &Value) {
    assert!(response["result"]["accepted"].as_bool().unwrap());
}

#[test]
fn claude_prompt_and_stdin_projection_are_mutually_explicit() {
    let arg = policy_with(
        json!("hello arg"),
        json!({ "command": "claude", "args": [], "prompt_mode": "arg", "invocation_mode": "proxy" }),
    );
    assert_arg_prompt_response(&arg);

    let stdin = policy_with(
        json!("hello stdin"),
        json!({ "command": "claude --print", "args": [], "prompt_mode": "stdin", "invocation_mode": "headless" }),
    );
    assert_stdin_prompt_response(&stdin);
}

fn assert_arg_prompt_response(response: &Value) {
    assert_eq!(response["result"]["prompt"], "hello arg");
    assert!(response["result"]["stdin"].is_null());
    assert_eq!(response["result"]["argv"], json!(["claude", "hello arg"]));
}

fn assert_stdin_prompt_response(response: &Value) {
    assert!(response["result"]["prompt"].is_null());
    assert_eq!(response["result"]["stdin"], "hello stdin");
    assert_eq!(response["result"]["argv"], json!(["claude", "--print"]));
}
