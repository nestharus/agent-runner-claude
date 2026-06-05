// declared_role: filter, formatter, orchestration, predicate, validator

use serde_json::{json, Value};

use super::restrictions::ClaudeRestrictions;

pub fn policy_diagnostics(
    argv: &[String],
    restrictions: &ClaudeRestrictions,
    proxy_mode: bool,
) -> Vec<Value> {
    let mut diagnostics = duplicate_flag_diagnostics(argv);
    if restrictions.has_tool_conflict() {
        diagnostics.push(diagnostic(
            "claude_allowed_disallowed_xor",
            "allowed_tools and disallowed_tools are mutually exclusive",
        ));
    }
    if proxy_mode {
        diagnostics.extend(unsafe_proxy_diagnostics(argv));
    }
    diagnostics
}

fn diagnostic(code: &str, message: &str) -> Value {
    json!({
        "severity": "error",
        "code": code,
        "message": message
    })
}

fn duplicate_flag_diagnostics(argv: &[String]) -> Vec<Value> {
    let mut diagnostics = Vec::new();
    if contains_flag(argv, "--append-system-prompt") {
        diagnostics.push(diagnostic(
            "duplicate_claude_append_system_prompt",
            "argv already contains --append-system-prompt",
        ));
    }
    if contains_flag(argv, "--allowed-tools") {
        diagnostics.push(diagnostic(
            "duplicate_claude_allowed_tools",
            "argv already contains --allowed-tools",
        ));
    }
    if contains_flag(argv, "--disallowed-tools") {
        diagnostics.push(diagnostic(
            "duplicate_claude_disallowed_tools",
            "argv already contains --disallowed-tools",
        ));
    }
    if contains_flag(argv, "--disable-slash-commands") {
        diagnostics.push(diagnostic(
            "duplicate_claude_disable_slash_commands",
            "argv already contains --disable-slash-commands",
        ));
    }
    diagnostics
}

fn unsafe_proxy_diagnostics(argv: &[String]) -> Vec<Value> {
    let mut diagnostics = Vec::new();
    for (idx, arg) in argv.iter().enumerate() {
        if unsafe_proxy_tools_arg(arg, argv.get(idx + 1)) {
            diagnostics.push(diagnostic(
                "unsafe_proxy_claude_tools_restrict",
                "raw --tools mcp restrictions are unsafe in proxy mode; use --allowed-tools forms",
            ));
            break;
        }
    }
    diagnostics
}

fn unsafe_proxy_tools_arg(arg: &str, next_arg: Option<&String>) -> bool {
    if arg == "--tools" {
        return next_arg.is_some_and(|value| tools_value_contains_mcp(value));
    }
    arg.strip_prefix("--tools=")
        .is_some_and(tools_value_contains_mcp)
}

fn tools_value_contains_mcp(value: &str) -> bool {
    value.split(',').any(|tool| tool.starts_with("mcp__"))
}

fn contains_flag(argv: &[String], flag: &str) -> bool {
    argv.iter().any(|arg| {
        arg == flag
            || arg
                .strip_prefix(flag)
                .is_some_and(|rest| rest.starts_with('='))
    })
}
