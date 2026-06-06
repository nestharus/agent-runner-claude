// declared_role: filter, formatter, orchestration, predicate, validator

use serde_json::{json, Value};

use super::restrictions::ClaudeRestrictions;

pub fn policy_diagnostics(
    argv: &[String],
    restrictions: &ClaudeRestrictions,
    proxy_mode: bool,
) -> Vec<Value> {
    let mut diagnostics = duplicate_flag_diagnostics(argv);
    diagnostics.extend(tool_conflict_diagnostics(restrictions));
    if proxy_mode {
        diagnostics.extend(unsafe_proxy_diagnostics(argv));
    }
    diagnostics
}

fn tool_conflict_diagnostics(restrictions: &ClaudeRestrictions) -> Vec<Value> {
    if has_tool_conflict(restrictions) {
        vec![tool_conflict_diagnostic()]
    } else {
        Vec::new()
    }
}

fn has_tool_conflict(restrictions: &ClaudeRestrictions) -> bool {
    restrictions.has_tool_conflict()
}

fn tool_conflict_diagnostic() -> Value {
    diagnostic(
        "claude_allowed_disallowed_xor",
        "allowed_tools and disallowed_tools are mutually exclusive",
    )
}

fn diagnostic(code: &str, message: &str) -> Value {
    json!({
        "severity": "error",
        "code": code,
        "message": message
    })
}

fn duplicate_flag_diagnostics(argv: &[String]) -> Vec<Value> {
    matching_duplicate_flag_specs(argv)
        .into_iter()
        .map(duplicate_flag_diagnostic)
        .collect()
}

fn unsafe_proxy_diagnostics(argv: &[String]) -> Vec<Value> {
    if contains_unsafe_proxy_tools_arg(argv) {
        vec![unsafe_proxy_diagnostic()]
    } else {
        Vec::new()
    }
}

#[derive(Clone, Copy)]
struct DuplicateFlagSpec {
    flag: &'static str,
    code: &'static str,
    message: &'static str,
}

fn matching_duplicate_flag_specs(argv: &[String]) -> Vec<DuplicateFlagSpec> {
    duplicate_flag_specs()
        .into_iter()
        .filter(|spec| contains_flag(argv, spec.flag))
        .collect()
}

fn duplicate_flag_specs() -> Vec<DuplicateFlagSpec> {
    vec![
        DuplicateFlagSpec {
            flag: "--append-system-prompt",
            code: "duplicate_claude_append_system_prompt",
            message: "argv already contains --append-system-prompt",
        },
        DuplicateFlagSpec {
            flag: "--allowed-tools",
            code: "duplicate_claude_allowed_tools",
            message: "argv already contains --allowed-tools",
        },
        DuplicateFlagSpec {
            flag: "--disallowed-tools",
            code: "duplicate_claude_disallowed_tools",
            message: "argv already contains --disallowed-tools",
        },
        DuplicateFlagSpec {
            flag: "--disable-slash-commands",
            code: "duplicate_claude_disable_slash_commands",
            message: "argv already contains --disable-slash-commands",
        },
    ]
}

fn duplicate_flag_diagnostic(spec: DuplicateFlagSpec) -> Value {
    diagnostic(spec.code, spec.message)
}

fn contains_unsafe_proxy_tools_arg(argv: &[String]) -> bool {
    argv.iter()
        .enumerate()
        .any(|(idx, arg)| unsafe_proxy_tools_arg(arg, argv.get(idx + 1)))
}

fn unsafe_proxy_diagnostic() -> Value {
    diagnostic(
        "unsafe_proxy_claude_tools_restrict",
        "raw --tools mcp restrictions are unsafe in proxy mode; use --allowed-tools forms",
    )
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
