//! Claude Code translation of `oulipoly.tool_mediation/v1`.
//!
//! A launch environment carrying the host's [`ToolMediation`] makes the SDK's
//! mediated `bash` tool (`agent_provider_execution::tool_bridge`, served by
//! this executable's `tool.bridge` subcommand) Claude Code's only command
//! tool, through Claude Code's own CLI switches:
//!
//! * `--strict-mcp-config --mcp-config <json>`: exactly one MCP server,
//!   `oulipoly`, whose one tool Claude Code names `mcp__oulipoly__bash`. Its
//!   environment names the policy and the root's Bash ingress explicitly, so
//!   the bridge does not depend on what Claude Code passes to MCP servers.
//! * `--tools ""` under an allow list; `--tools Read,Write,Edit` under
//!   `trusted-task`, whose meaning here is any command through the mediated
//!   tool plus Claude Code's own file tools, as the embedded Claude receiver
//!   defined it. Built-in `Bash`, delegation, background, web, notebook and
//!   skill tools are never offered.
//! * `--allowedTools` naming exactly those tools, `--disallowedTools` naming the
//!   rest of the built-in execution set, `--permission-mode dontAsk` (anything
//!   not pre-approved is denied, never asked), `--setting-sources ""` (no user,
//!   project or local settings, hooks or MCP) and `--disable-slash-commands`.
//! * `ENABLE_TOOL_SEARCH=false` in Claude Code's environment, so the mediated
//!   tool is offered upfront rather than deferred.
//!
//! The template must not carry its own tool, permission, settings, agent,
//! plugin or MCP options: mediation owns them, and a combination is refused
//! rather than merged. Only resident turns are mediated; a one-shot launch
//! carrying a policy is refused. These are constructed CLI options checked
//! against a fake Claude Code here; whether a given Claude Code release honours
//! each of them must be qualified against it.

use agent_provider_contract::tool_mediation::{EffectiveMediation, ToolMediation};
use agent_provider_execution::tool_bridge;
use serde_json::{json, Value};

/// The MCP server name; Claude Code names its tool `mcp__<server>__<tool>`.
pub(crate) const SERVER: &str = "oulipoly";
pub(crate) const TOOL: &str = "mcp__oulipoly__bash";
/// Claude Code file tools `trusted-task` adds.
pub(crate) const TRUSTED_TASK_TOOLS: &[&str] = &["Read", "Write", "Edit"];
/// Built-in tools never offered under mediation.
const DENIED_TOOLS: &[&str] = &[
    "Bash",
    "BashOutput",
    "KillShell",
    "Agent",
    "Task",
    "Monitor",
    "TaskOutput",
    "TaskStop",
    "WebFetch",
    "WebSearch",
    "Skill",
    "NotebookEdit",
    "PowerShell",
    "Glob",
    "Grep",
];
/// Template options mediation owns.
const OWNED_FLAGS: &[&str] = &[
    "--allowedTools",
    "--allowed-tools",
    "--disallowedTools",
    "--disallowed-tools",
    "--tools",
    "--mcp-config",
    "--strict-mcp-config",
    "--permission-mode",
    "--permission-prompt-tool",
    "--dangerously-skip-permissions",
    "--allow-dangerously-skip-permissions",
    "--settings",
    "--setting-sources",
    "--agent",
    "--agents",
    "--plugin-dir",
    "--add-dir",
];

/// Every tool Claude Code is offered under `policy`.
pub(crate) fn native_tools(policy: &ToolMediation) -> Vec<String> {
    let mut tools = vec![TOOL.to_owned()];
    if policy.trusted_task() {
        tools.extend(TRUSTED_TASK_TOOLS.iter().map(|tool| (*tool).to_owned()));
    }
    tools
}

pub(crate) fn effective(policy: &ToolMediation) -> EffectiveMediation {
    policy.effective(TOOL, native_tools(policy))
}

/// A template option that mediation owns, if any. `value_flags` are the
/// options whose next token is data, which is never read as an option.
pub(crate) fn conflict(argv: &[String], value_flags: &[&str]) -> Option<String> {
    let mut args = argv.iter().skip(1);
    while let Some(arg) = args.next() {
        let flag = arg.split_once('=').map_or(arg.as_str(), |(flag, _)| flag);
        if OWNED_FLAGS.contains(&flag) {
            return Some(format!("{flag} conflicts with the host's tool mediation, which sets Claude Code's tools, permissions, settings and MCP servers itself"));
        }
        if value_flags.contains(&flag) && !arg.contains('=') {
            args.next();
        }
    }
    None
}

/// Claude Code options for one mediated turn: `bridge` is this provider's
/// executable, `ingress` the root ingress value this turn's process holds.
pub(crate) fn native_args(policy: &ToolMediation, bridge: &str, ingress: &str) -> Vec<String> {
    let mut env = serde_json::Map::new();
    env.insert(
        agent_provider_contract::tool_mediation::ENV.into(),
        json!(policy.encode()),
    );
    env.insert(policy.ingress_env.clone(), json!(ingress));
    let config = json!({"mcpServers": {SERVER: {"type": "stdio", "command": bridge,
        "args": [tool_bridge::SUBCOMMAND], "env": env}}});
    let builtin: Vec<&str> = if policy.trusted_task() {
        TRUSTED_TASK_TOOLS.to_vec()
    } else {
        Vec::new()
    };
    vec![
        "--strict-mcp-config".into(),
        "--mcp-config".into(),
        config.to_string(),
        "--tools".into(),
        builtin.join(","),
        "--allowedTools".into(),
        native_tools(policy).join(","),
        "--disallowedTools".into(),
        DENIED_TOOLS.join(","),
        "--permission-mode".into(),
        "dontAsk".into(),
        "--setting-sources".into(),
        String::new(),
        "--disable-slash-commands".into(),
    ]
}

/// Claude Code environment entries mediation sets.
pub(crate) const NATIVE_ENV: &[(&str, &str)] = &[("ENABLE_TOOL_SEARCH", "false")];

/// Native inventory is a report, not enforcement attestation. Missing fields
/// remain explicitly unreported; present contradictory or invalid fields fail
/// the turn. Keep the actual report with the expected configuration in the
/// durable native-turn journal for qualification.
pub(crate) fn inventory(policy: &ToolMediation, init: &Value) -> (Value, bool) {
    let expected = native_tools(policy);
    let tools = match init.get("tools") {
        None => "not-reported",
        Some(Value::Array(tools))
            if tools.len() == expected.len()
                && tools.iter().all(|tool| {
                    tool.as_str()
                        .is_some_and(|tool| expected.iter().any(|e| e == tool))
                })
                && expected
                    .iter()
                    .all(|tool| tools.iter().filter(|t| t.as_str() == Some(tool)).count() == 1) =>
        {
            "consistent"
        }
        Some(Value::Array(tools)) if tools.iter().all(Value::is_string) => "contradictory",
        Some(_) => "invalid",
    };
    let servers = match init.get("mcp_servers") {
        None => "not-reported",
        Some(Value::Array(servers))
            if servers.len() == 1
                && servers[0]["name"] == json!(SERVER)
                && servers[0]["status"] == json!("connected") =>
        {
            "consistent"
        }
        Some(Value::Array(servers))
            if servers
                .iter()
                .all(|s| s["name"].is_string() && s["status"].is_string()) =>
        {
            "contradictory"
        }
        Some(_) => "invalid",
    };
    let contradiction = [tools, servers]
        .iter()
        .any(|s| matches!(*s, "contradictory" | "invalid"));
    (
        json!({"source":"claude.stream_json.init", "tools_observation":tools,
        "mcp_observation":servers, "expected_tools":expected,
        "expected_mcp_servers":[{"name":SERVER,"status":"connected"}],
        "reported_tools":init.get("tools"), "reported_mcp_servers":init.get("mcp_servers"),
        "enforcement_attested":false}),
        contradiction,
    )
}
