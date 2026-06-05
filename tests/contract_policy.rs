mod support;

use serde_json::{json, Value};
use support::fixtures::{envelope, host_context, temp_roots, CONTRACT};
use support::invoke::{invoke, parse_one_stdout_json};
use support::schema::assert_valid;

fn evaluate(launch: Value) -> Value {
    let roots = temp_roots("policy");
    let request = envelope(
        CONTRACT,
        host_context(&roots),
        json!({
            "settings_id": "claude-primary",
            "mode": "proxy",
            "model": {
                "name": "claude-sonnet",
                "provider_args": ["--model", "sonnet"],
                "inputs": { "prompt": "hello", "named": {} }
            },
            "launch": launch
        }),
    );
    let output = invoke("policy.evaluate", &request);
    assert_eq!(output.code, Some(0));
    assert!(output.stderr.is_empty());
    let response = parse_one_stdout_json(&output);
    assert_valid(
        "policy.schema.json#/$defs/PolicyEvaluateResponse",
        &response,
    );
    response
}

fn launch_base(extra: Value) -> Value {
    let mut launch = json!({
        "command": "claude",
        "args": [],
        "prompt_mode": "arg",
        "invocation_mode": "proxy"
    });
    for (key, value) in extra.as_object().unwrap() {
        launch
            .as_object_mut()
            .unwrap()
            .insert(key.clone(), value.clone());
    }
    launch
}

fn diagnostic_codes(response: &Value) -> Vec<String> {
    response["result"]["diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|diagnostic| diagnostic["code"].as_str().map(str::to_string))
        .collect()
}

fn assert_policy_error(response: &Value, category: &str) {
    assert_valid(
        "policy.schema.json#/$defs/PolicyEvaluateErrorResponse",
        response,
    );
    assert!(!response["ok"].as_bool().unwrap());
    assert_eq!(response["error"]["category"], category);
}

#[test]
fn policy_evaluate_accepts_safe_path_and_preserves_exact_effective_argv_order() {
    let response = evaluate(launch_base(json!({
        "system_prompt_override": "system policy",
        "tool_restrictions": {
            "kind": "claude",
            "claude": {
                "allowed_tools": ["Read", "Bash", "mcp__github__search"],
                "disable_slash_commands": true
            }
        }
    })));
    let result = &response["result"];
    assert!(result["accepted"].as_bool().unwrap());
    assert!(result["diagnostics"].as_array().unwrap().is_empty());
    assert_eq!(
        result["argv"],
        json!([
            "claude",
            "--model",
            "sonnet",
            "--append-system-prompt",
            "system policy",
            "--allowed-tools",
            "Read,Bash,mcp__github__search",
            "--disable-slash-commands",
            "hello"
        ])
    );
    assert_eq!(result["prompt"], "hello");
    assert!(result["stdin"].is_null());
}

#[test]
fn policy_rejects_allowed_and_disallowed_tool_filters_together() {
    let response = evaluate(launch_base(json!({
        "tool_restrictions": {
            "kind": "claude",
            "claude": {
                "allowed_tools": ["Read"],
                "disallowed_tools": ["Write"]
            }
        }
    })));
    assert!(!response["result"]["accepted"].as_bool().unwrap());
    assert!(diagnostic_codes(&response).contains(&"claude_allowed_disallowed_xor".to_string()));
}

#[test]
fn policy_reports_duplicate_policy_and_config_flags_as_diagnostics() {
    let response = evaluate(launch_base(json!({
        "command": "claude --allowed-tools Read --disallowed-tools Write",
        "tool_restrictions": {
            "kind": "claude",
            "claude": { "allowed_tools": ["Bash"] }
        }
    })));
    assert!(!response["result"]["accepted"].as_bool().unwrap());
    let codes = diagnostic_codes(&response);
    assert!(codes.contains(&"duplicate_claude_allowed_tools".to_string()));
    assert!(codes.contains(&"duplicate_claude_disallowed_tools".to_string()));
}

#[test]
fn policy_rejects_unsafe_proxy_tools_mcp_restriction() {
    let response = evaluate(launch_base(json!({
        "args": ["--tools", "mcp__filesystem__read_file"]
    })));
    assert!(!response["result"]["accepted"].as_bool().unwrap());
    assert!(diagnostic_codes(&response).contains(&"unsafe_proxy_claude_tools_restrict".to_string()));
}

#[test]
fn policy_permits_allowed_tools_mcp_forms() {
    let response = evaluate(launch_base(json!({
        "tool_restrictions": {
            "kind": "claude",
            "claude": { "allowed_tools": ["mcp__filesystem__read_file", "mcp__github__search"] }
        }
    })));
    assert!(response["result"]["accepted"].as_bool().unwrap());
    assert!(response["result"]["diagnostics"]
        .as_array()
        .unwrap()
        .is_empty());
    let argv = response["result"]["argv"].as_array().unwrap();
    assert!(argv.contains(&json!("mcp__filesystem__read_file,mcp__github__search")));
}

#[test]
fn policy_accepted_is_false_when_any_diagnostic_is_emitted() {
    let response = evaluate(launch_base(json!({
        "args": ["--tools", "mcp__unsafe__tool"],
        "tool_restrictions": {
            "kind": "claude",
            "claude": { "allowed_tools": ["Read"] }
        }
    })));
    assert!(!response["result"]["diagnostics"]
        .as_array()
        .unwrap()
        .is_empty());
    assert!(!response["result"]["accepted"].as_bool().unwrap());
}

#[test]
fn policy_malformed_request_uses_error_def_while_policy_violations_stay_success_diagnostics() {
    let roots = temp_roots("policy-malformed-request");
    let request = envelope(
        CONTRACT,
        host_context(&roots),
        json!({ "settings_id": "claude-primary" }),
    );

    let output = invoke("policy.evaluate", &request);

    assert_eq!(output.code, Some(2));
    assert!(output.stderr.is_empty());
    let response = parse_one_stdout_json(&output);
    assert_policy_error(&response, "invalid_request");
}
