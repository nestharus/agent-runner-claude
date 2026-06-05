mod support;

use serde_json::{json, Value};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use support::fixtures::{envelope, host_context, temp_roots, TempRoots, CONTRACT};
use support::invoke::{invoke, parse_one_stdout_json};
use support::schema::assert_valid;

const SECRET: &str = "SETUP_SECRET_SENTINEL_W2C_DO_NOT_LEAK";

fn host_with_env(roots: &TempRoots, entries: &[(&str, String)]) -> Value {
    let mut host = host_context(roots);
    let env = host["env"].as_object_mut().unwrap();
    for (key, value) in entries {
        env.insert((*key).to_string(), json!(value));
    }
    host
}

fn call_with_host(host: Value, subcommand: &str, params: Value) -> (Value, Vec<u8>, Vec<u8>) {
    let request = envelope(CONTRACT, host, params);
    let output = invoke(subcommand, &request);
    assert_eq!(output.code, Some(0));
    assert!(output.stderr.is_empty());
    let response = parse_one_stdout_json(&output);
    (response, output.stdout, output.stderr)
}

fn call(roots: &TempRoots, subcommand: &str, params: Value) -> (Value, Vec<u8>, Vec<u8>) {
    call_with_host(host_context(roots), subcommand, params)
}

fn call_error(roots: &TempRoots, subcommand: &str, params: Value, schema: &str) -> Value {
    let request = envelope(CONTRACT, host_context(roots), params);
    let output = invoke(subcommand, &request);
    assert_eq!(output.code, Some(2));
    assert!(output.stderr.is_empty());
    let response = parse_one_stdout_json(&output);
    assert_valid(schema, &response);
    assert!(!response["ok"].as_bool().unwrap());
    assert_eq!(response["error"]["category"], "invalid_request");
    response
}

fn write_executable(path: &Path, body: &str) {
    fs::write(path, body).expect("write executable");
    let mut perms = fs::metadata(path).unwrap().permissions();
    perms.set_mode(0o755);
    fs::set_permissions(path, perms).unwrap();
}

fn make_claude_stdout_stderr(
    bin_dir: &Path,
    argv_path: &Path,
    stdout_json: &str,
    stderr_text: &str,
) {
    fs::create_dir_all(bin_dir).expect("create bin dir");
    let script = format!(
        "#!/bin/sh\nprintf '%s\\n' \"$@\" > '{}'\nprintf '%s\\n' '{}'\nprintf '%s\\n' '{}' >&2\n",
        argv_path.display(),
        stdout_json,
        stderr_text
    );
    write_executable(&bin_dir.join("claude"), &script);
}

fn assert_no_secret(value: &Value, stdout: &[u8], stderr: &[u8]) {
    let response_text = value.to_string();
    assert!(
        !response_text.contains(SECRET),
        "secret leaked in response: {response_text}"
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

fn read_argv(path: &Path) -> Vec<String> {
    fs::read_to_string(path)
        .expect("read captured argv")
        .lines()
        .map(str::to_string)
        .collect()
}

fn assert_setup_brain_argv(argv: &[String], model: &str, resume: Option<&str>, prompt: &str) {
    let prefix = expected_setup_brain_prefix(model);
    assert!(argv.len() > prefix.len(), "argv too short: {argv:?}");
    assert_eq!(&argv[..prefix.len()], prefix.as_slice());
    let schema_arg = setup_brain_schema_arg(argv, prefix.len());
    let schema = parse_setup_brain_schema(schema_arg);
    assert!(schema.is_object());
    let expected = expected_setup_brain_argv(prefix, schema_arg, resume, prompt);
    assert_eq!(argv, expected.as_slice());
}

fn expected_setup_brain_prefix(model: &str) -> Vec<String> {
    vec![
        "-p".to_string(),
        "--output-format".to_string(),
        "json".to_string(),
        "--model".to_string(),
        model.to_string(),
        "--allowedTools".to_string(),
        "Read,Bash,Glob,Grep".to_string(),
        "--no-session-persistence".to_string(),
        "--json-schema".to_string(),
    ]
}

fn setup_brain_schema_arg(argv: &[String], schema_index: usize) -> &str {
    &argv[schema_index]
}

fn parse_setup_brain_schema(schema_arg: &str) -> Value {
    serde_json::from_str::<Value>(schema_arg).expect("--json-schema must be JSON")
}

fn expected_setup_brain_argv(
    mut expected: Vec<String>,
    schema_arg: &str,
    resume: Option<&str>,
    prompt: &str,
) -> Vec<String> {
    expected.push(schema_arg.to_string());
    if let Some(resume) = resume {
        expected.push("--resume".to_string());
        expected.push(resume.to_string());
    }
    expected.push(prompt.to_string());
    expected
}

#[test]
fn setup_detect_reports_installed_missing_auth_profiles_warnings_and_redacts_secrets() {
    let roots = temp_roots("setup-detect");
    let bin_dir = roots.root.join("bin");
    fs::create_dir_all(&bin_dir).unwrap();
    write_executable(&bin_dir.join("claude"), "#!/bin/sh\nprintf '1.2.3\\n'\n");

    let claude_home = roots.home.join(".claude");
    fs::create_dir_all(claude_home.join("profiles/work")).unwrap();
    fs::write(
        claude_home.join("credentials.json"),
        format!(
            r#"{{"auth_state":"authenticated","email":"setup@example.test","token":"{SECRET}"}}"#
        ),
    )
    .unwrap();
    fs::write(
        claude_home.join("profiles/work/settings.json"),
        format!(r#"{{"display_name":"Work Setup","api_key":"{SECRET}"}}"#),
    )
    .unwrap();

    let host = host_with_env(&roots, &[("PATH", bin_dir.display().to_string())]);
    let (installed, stdout, stderr) = call_with_host(host, "setup.detect", json!({}));
    assert_valid("setup.schema.json#/$defs/SetupDetectResponse", &installed);
    assert!(installed["result"]["installed"].as_bool().unwrap());
    assert!(has_string(&installed["result"]["binary"], "claude"));
    assert!(
        has_string(&installed["result"]["auth"], "authenticated")
            || installed["result"]["auth"].is_string()
    );
    assert!(
        has_string(&installed["result"]["profiles"], "Work Setup")
            || has_string(&installed["result"]["profiles"], "work")
    );
    assert_no_secret(&installed, &stdout, &stderr);

    let missing_host = host_with_env(
        &roots,
        &[("PATH", roots.root.join("empty-bin").display().to_string())],
    );
    let (missing, stdout, stderr) = call_with_host(missing_host, "setup.detect", json!({}));
    assert_valid("setup.schema.json#/$defs/SetupDetectResponse", &missing);
    assert!(!missing["result"]["installed"].as_bool().unwrap());
    assert!(!missing["result"]["warnings"].as_array().unwrap().is_empty());
    assert_no_secret(&missing, &stdout, &stderr);
}

#[test]
fn setup_detect_malformed_request_uses_capability_error_def() {
    let roots = temp_roots("setup-detect-malformed-request");

    call_error(
        &roots,
        "setup.detect",
        json!(null),
        "setup.schema.json#/$defs/SetupDetectErrorResponse",
    );
}

#[test]
fn setup_install_plan_returns_steps_only_and_does_not_mutate_install_target() {
    let roots = temp_roots("setup-install-plan");
    let install_target = roots.root.join("install-target");
    assert!(!install_target.exists());

    let (response, stdout, stderr) = call(
        &roots,
        "setup.install_plan",
        json!({
            "install_target": install_target.display().to_string(),
            "channel": "stable"
        }),
    );
    assert_valid(
        "setup.schema.json#/$defs/SetupInstallPlanResponse",
        &response,
    );
    assert!(!response["result"]["steps"].as_array().unwrap().is_empty());
    assert!(
        !install_target.exists(),
        "install_plan must not create install target"
    );
    assert_no_secret(&response, &stdout, &stderr);
}

#[test]
fn setup_install_plan_malformed_request_uses_capability_error_def() {
    let roots = temp_roots("setup-install-plan-malformed-request");

    call_error(
        &roots,
        "setup.install_plan",
        json!(null),
        "setup.schema.json#/$defs/SetupInstallPlanErrorResponse",
    );
}

#[test]
fn setup_sync_plan_reports_skill_mcp_config_ops_conflicts_and_never_overwrites() {
    let roots = temp_roots("setup-sync-plan");
    let skill_path = roots.home.join(".claude/skills/agent-runner/SKILL.md");
    let claude_json_path = roots.home.join(".claude.json");
    fs::create_dir_all(skill_path.parent().unwrap()).unwrap();
    fs::write(&skill_path, "USER_SKILL_SENTINEL_DO_NOT_OVERWRITE").unwrap();
    let claude_json_sentinel = format!(
        r#"{{"mcpServers":{{"agent-runner":{{"command":"old","token":"{SECRET}"}}}},"config":{{"theme":"user"}}}}"#
    );
    fs::write(&claude_json_path, &claude_json_sentinel).unwrap();

    let (response, stdout, stderr) = call(
        &roots,
        "setup.sync_plan",
        json!({
            "skills": [{ "name": "agent-runner", "body": "provider skill" }],
            "mcp_servers": [{ "name": "agent-runner", "command": "agent-runner-claude mcp" }],
            "config": { "provider": "claude", "settings_schema_id": "claude.settings/v1" },
            "overwrite": false
        }),
    );
    assert_valid("setup.schema.json#/$defs/SetupSyncPlanResponse", &response);
    let result = &response["result"];
    assert!(has_string(&result["operations"], "skill"));
    assert!(has_string(&result["operations"], "mcp"));
    assert!(has_string(&result["operations"], "config"));
    assert!(has_string(&result["diagnostics"], "conflict"));
    assert_eq!(
        fs::read_to_string(&skill_path).unwrap(),
        "USER_SKILL_SENTINEL_DO_NOT_OVERWRITE"
    );
    assert_eq!(
        fs::read_to_string(&claude_json_path).unwrap(),
        claude_json_sentinel
    );
    assert_no_secret(&response, &stdout, &stderr);
}

#[test]
fn setup_sync_plan_malformed_request_uses_capability_error_def() {
    let roots = temp_roots("setup-sync-plan-malformed-request");

    call_error(
        &roots,
        "setup.sync_plan",
        json!(null),
        "setup.schema.json#/$defs/SetupSyncPlanErrorResponse",
    );
}

#[test]
fn setup_brain_turn_uses_default_model_parses_message_session_and_markers() {
    let roots = temp_roots("setup-brain-default");
    let bin_dir = roots.root.join("bin");
    let argv_path = roots.root.join("argv-default.txt");
    make_claude_stdout_stderr(
        &bin_dir,
        &argv_path,
        r#"{"type":"assistant","content":[{"type":"text","text":"setup answer"}]}"#,
        "Session: setup-session-default-001",
    );
    let host = host_with_env(&roots, &[("PATH", bin_dir.display().to_string())]);
    let prompt = "detect and explain setup";
    let (response, stdout, stderr) =
        call_with_host(host, "setup_brain.turn", json!({ "prompt": prompt }));
    assert_valid("setup.schema.json#/$defs/SetupBrainTurnResponse", &response);
    assert_eq!(
        response["result"]["conversation_id"],
        "setup-session-default-001"
    );
    assert_eq!(response["result"]["message"]["type"], "assistant");
    assert!(has_string(&response["result"]["message"], "setup answer"));
    assert!(!response["result"]["markers"].as_array().unwrap().is_empty());
    assert_setup_brain_argv(&read_argv(&argv_path), "claude-sonnet-4-6", None, prompt);
    assert_no_secret(&response, &stdout, &stderr);
}

#[test]
fn setup_brain_turn_uses_configured_model_and_resume_id_from_claude_settings_v1() {
    let roots = temp_roots("setup-brain-configured");
    let bin_dir = roots.root.join("bin");
    let argv_path = roots.root.join("argv-configured.txt");
    make_claude_stdout_stderr(
        &bin_dir,
        &argv_path,
        r#"{"type":"assistant","content":[{"type":"text","text":"resumed setup answer"}]}"#,
        "session_id: setup-session-resumed-002",
    );
    let host = host_with_env(&roots, &[("PATH", bin_dir.display().to_string())]);
    let settings_request = envelope(
        CONTRACT,
        host.clone(),
        json!({
            "display_name": "setup brain model",
            "values": {
                "command": "claude",
                "setup_brain_model": "claude-opus-4-5"
            }
        }),
    );
    let settings_output = invoke("settings.create", &settings_request);
    assert_eq!(settings_output.code, Some(0));
    assert!(settings_output.stderr.is_empty());
    let settings_response = parse_one_stdout_json(&settings_output);
    assert_valid(
        "settings.schema.json#/$defs/SettingsCreateResponse",
        &settings_response,
    );
    let settings_id = settings_response["result"]["record"]["id"]
        .as_str()
        .unwrap();

    let prompt = "continue setup";
    let resume = "setup-session-default-001";
    let (response, stdout, stderr) = call_with_host(
        host,
        "setup_brain.turn",
        json!({ "settings_id": settings_id, "resume": resume, "prompt": prompt }),
    );
    assert_valid("setup.schema.json#/$defs/SetupBrainTurnResponse", &response);
    assert_eq!(
        response["result"]["conversation_id"],
        "setup-session-resumed-002"
    );
    assert!(has_string(
        &response["result"]["message"],
        "resumed setup answer"
    ));
    assert_setup_brain_argv(
        &read_argv(&argv_path),
        "claude-opus-4-5",
        Some(resume),
        prompt,
    );
    assert_no_secret(&response, &stdout, &stderr);
}

#[test]
fn setup_brain_turn_malformed_request_uses_capability_error_def() {
    let roots = temp_roots("setup-brain-malformed-request");

    call_error(
        &roots,
        "setup_brain.turn",
        json!(null),
        "setup.schema.json#/$defs/SetupBrainTurnErrorResponse",
    );
}
