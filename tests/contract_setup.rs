// declared_role: orchestration, mapper, parser, validator, accessor, predicate, formatter
// intrinsic_surface_declarations:
//   - component: tests/contract_setup.rs
//     role: intrinsic-surface
//     Domain: contract_setup_proof_surface
//     Owns:
//       - setup contract scenarios
//       - support harness dependencies for setup script/invoke/schema proof

mod support;

use serde_json::{json, Value};
use std::fs;
use std::path::{Path, PathBuf};
use support::assertions::assert_setup_brain_argv;
use support::fixtures::{envelope, host_context, temp_roots, TempRoots, CONTRACT};
use support::invoke::{invoke, parse_one_stdout_json};
use support::schema::assert_valid;
use support::scripts::{install_fake_claude_stdout_stderr, write_executable};

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
    let output = invoke_setup(subcommand, setup_request(host, params));
    assert_success_invocation(&output);
    let response = stdout_json(&output);
    (response, output_stdout(&output), output_stderr(&output))
}

fn call(roots: &TempRoots, subcommand: &str, params: Value) -> (Value, Vec<u8>, Vec<u8>) {
    call_with_host(host_context(roots), subcommand, params)
}

fn call_error(roots: &TempRoots, subcommand: &str, params: Value, schema: &str) -> Value {
    let output = invoke_setup(subcommand, setup_request(host_context(roots), params));
    assert_invalid_request_invocation(&output);
    let response = stdout_json(&output);
    assert_invalid_request_response(schema, &response);
    response
}

fn setup_request(host: Value, params: Value) -> Value {
    envelope(CONTRACT, host, params)
}

fn invoke_setup(subcommand: &str, request: Value) -> support::invoke::Invocation {
    invoke(subcommand, &request)
}

fn assert_success_invocation(output: &support::invoke::Invocation) {
    assert_eq!(output.code, Some(0));
    assert_empty_stderr(output);
}

fn assert_invalid_request_invocation(output: &support::invoke::Invocation) {
    assert_eq!(output.code, Some(2));
    assert_empty_stderr(output);
}

fn assert_empty_stderr(output: &support::invoke::Invocation) {
    assert!(output.stderr.is_empty());
}

fn stdout_json(output: &support::invoke::Invocation) -> Value {
    parse_one_stdout_json(output)
}

fn output_stdout(output: &support::invoke::Invocation) -> Vec<u8> {
    output.stdout.clone()
}

fn output_stderr(output: &support::invoke::Invocation) -> Vec<u8> {
    output.stderr.clone()
}

fn assert_invalid_request_response(schema: &str, response: &Value) {
    assert_valid(schema, response);
    assert!(!response["ok"].as_bool().unwrap());
    assert_eq!(response["error"]["category"], "invalid_request");
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

fn read_argv_text(path: &Path) -> String {
    fs::read_to_string(path).expect("read captured argv")
}

fn parse_argv_lines(text: &str) -> Vec<String> {
    text.lines().map(str::to_string).collect()
}

fn setup_empty_params() -> Value {
    json!({})
}

fn setup_bin_dir(roots: &TempRoots) -> PathBuf {
    roots.root.join("bin")
}

fn setup_detect_missing_bin_dir(roots: &TempRoots) -> PathBuf {
    roots.root.join("empty-bin")
}

fn setup_detect_claude_home(roots: &TempRoots) -> PathBuf {
    roots.home.join(".claude")
}

fn setup_detect_profile_dir(claude_home: &Path) -> PathBuf {
    claude_home.join("profiles/work")
}

fn setup_detect_credentials_path(claude_home: &Path) -> PathBuf {
    claude_home.join("credentials.json")
}

fn setup_detect_profile_settings_path(claude_home: &Path) -> PathBuf {
    claude_home.join("profiles/work/settings.json")
}

fn setup_detect_credentials_fixture() -> String {
    format!(r#"{{"auth_state":"authenticated","email":"setup@example.test","token":"{SECRET}"}}"#)
}

fn setup_detect_profile_fixture() -> String {
    format!(r#"{{"display_name":"Work Setup","api_key":"{SECRET}"}}"#)
}

fn install_setup_detect_fixture(roots: &TempRoots, bin_dir: &Path) {
    fs::create_dir_all(bin_dir).unwrap();
    write_executable(&bin_dir.join("claude"), "#!/bin/sh\nprintf '1.2.3\\n'\n");
    let claude_home = setup_detect_claude_home(roots);
    fs::create_dir_all(setup_detect_profile_dir(&claude_home)).unwrap();
    fs::write(
        setup_detect_credentials_path(&claude_home),
        setup_detect_credentials_fixture(),
    )
    .unwrap();
    fs::write(
        setup_detect_profile_settings_path(&claude_home),
        setup_detect_profile_fixture(),
    )
    .unwrap();
}

fn setup_host_with_path(roots: &TempRoots, path: &Path) -> Value {
    host_with_env(roots, &[("PATH", path.display().to_string())])
}

fn assert_setup_detect_response_schema(response: &Value) {
    assert_valid("setup.schema.json#/$defs/SetupDetectResponse", response);
}

fn assert_setup_detect_installed_response(response: &Value, stdout: &[u8], stderr: &[u8]) {
    assert_setup_detect_response_schema(response);
    assert!(response["result"]["installed"].as_bool().unwrap());
    assert!(has_string(&response["result"]["binary"], "claude"));
    assert!(
        has_string(&response["result"]["auth"], "authenticated")
            || response["result"]["auth"].is_string()
    );
    assert!(
        has_string(&response["result"]["profiles"], "Work Setup")
            || has_string(&response["result"]["profiles"], "work")
    );
    assert_no_secret(response, stdout, stderr);
}

fn assert_setup_detect_missing_response(response: &Value, stdout: &[u8], stderr: &[u8]) {
    assert_setup_detect_response_schema(response);
    assert!(!response["result"]["installed"].as_bool().unwrap());
    assert!(!response["result"]["warnings"]
        .as_array()
        .unwrap()
        .is_empty());
    assert_no_secret(response, stdout, stderr);
}

fn setup_install_target(roots: &TempRoots) -> PathBuf {
    roots.root.join("install-target")
}

fn setup_install_plan_params(install_target: &Path) -> Value {
    json!({
        "install_target": install_target.display().to_string(),
        "channel": "stable"
    })
}

fn assert_install_target_absent(install_target: &Path) {
    assert!(!install_target.exists());
}

fn assert_setup_install_plan_response(response: &Value) {
    assert_valid(
        "setup.schema.json#/$defs/SetupInstallPlanResponse",
        response,
    );
    assert!(!response["result"]["steps"].as_array().unwrap().is_empty());
}

fn assert_install_plan_did_not_create_target(install_target: &Path) {
    assert!(
        !install_target.exists(),
        "install_plan must not create install target"
    );
}

fn setup_sync_skill_path(roots: &TempRoots) -> PathBuf {
    roots.home.join(".claude/skills/agent-runner/SKILL.md")
}

fn setup_sync_claude_json_path(roots: &TempRoots) -> PathBuf {
    roots.home.join(".claude.json")
}

fn setup_sync_claude_json_sentinel() -> String {
    format!(
        r#"{{"mcpServers":{{"agent-runner":{{"command":"old","token":"{SECRET}"}}}},"config":{{"theme":"user"}}}}"#
    )
}

fn install_setup_sync_fixture(
    skill_path: &Path,
    claude_json_path: &Path,
    claude_json_sentinel: &str,
) {
    fs::create_dir_all(skill_path.parent().unwrap()).unwrap();
    fs::write(skill_path, "USER_SKILL_SENTINEL_DO_NOT_OVERWRITE").unwrap();
    fs::write(claude_json_path, claude_json_sentinel).unwrap();
}

fn setup_sync_plan_params() -> Value {
    json!({
        "skills": [{ "name": "agent-runner", "body": "provider skill" }],
        "mcp_servers": [{ "name": "agent-runner", "command": "agent-runner-claude mcp" }],
        "config": { "provider": "claude", "settings_schema_id": "claude.settings/v1" },
        "overwrite": false
    })
}

fn assert_setup_sync_plan_response(response: &Value) {
    assert_valid("setup.schema.json#/$defs/SetupSyncPlanResponse", response);
    let result = &response["result"];
    assert!(has_string(&result["operations"], "skill"));
    assert!(has_string(&result["operations"], "mcp"));
    assert!(has_string(&result["operations"], "config"));
    assert!(has_string(&result["diagnostics"], "conflict"));
}

fn assert_setup_sync_fixture_unchanged(
    skill_path: &Path,
    claude_json_path: &Path,
    claude_json_sentinel: &str,
) {
    assert_eq!(
        fs::read_to_string(skill_path).unwrap(),
        "USER_SKILL_SENTINEL_DO_NOT_OVERWRITE"
    );
    assert_eq!(
        fs::read_to_string(claude_json_path).unwrap(),
        claude_json_sentinel
    );
}

fn setup_brain_default_argv_path(roots: &TempRoots) -> PathBuf {
    roots.root.join("argv-default.txt")
}

fn setup_brain_configured_argv_path(roots: &TempRoots) -> PathBuf {
    roots.root.join("argv-configured.txt")
}

fn install_setup_brain_default_fixture(bin_dir: &Path, argv_path: &Path) {
    install_fake_claude_stdout_stderr(
        bin_dir,
        argv_path,
        r#"{"type":"assistant","content":[{"type":"text","text":"setup answer"}]}"#,
        "Session: setup-session-default-001",
    );
}

fn install_setup_brain_configured_fixture(bin_dir: &Path, argv_path: &Path) {
    install_fake_claude_stdout_stderr(
        bin_dir,
        argv_path,
        r#"{"type":"assistant","content":[{"type":"text","text":"resumed setup answer"}]}"#,
        "session_id: setup-session-resumed-002",
    );
}

fn setup_brain_prompt_params(prompt: &str) -> Value {
    json!({ "prompt": prompt })
}

fn setup_brain_settings_create_params() -> Value {
    json!({
        "display_name": "setup brain model",
        "values": {
            "command": "claude",
            "setup_brain_model": "claude-opus-4-5"
        }
    })
}

fn setup_brain_resume_params(settings_id: &str, resume: &str, prompt: &str) -> Value {
    json!({ "settings_id": settings_id, "resume": resume, "prompt": prompt })
}

fn create_setup_brain_settings(host: Value) -> Value {
    let (response, _, _) = call_with_host(
        host,
        "settings.create",
        setup_brain_settings_create_params(),
    );
    response
}

fn settings_record_id(response: &Value) -> &str {
    response["result"]["record"]["id"].as_str().unwrap()
}

fn assert_setup_brain_settings_response(response: &Value) {
    assert_valid(
        "settings.schema.json#/$defs/SettingsCreateResponse",
        response,
    );
}

fn assert_setup_brain_default_response(response: &Value) {
    assert_valid("setup.schema.json#/$defs/SetupBrainTurnResponse", response);
    assert_eq!(
        response["result"]["conversation_id"],
        "setup-session-default-001"
    );
    assert_eq!(response["result"]["message"]["type"], "assistant");
    assert!(has_string(&response["result"]["message"], "setup answer"));
    assert!(!response["result"]["markers"].as_array().unwrap().is_empty());
}

fn assert_setup_brain_configured_response(response: &Value) {
    assert_valid("setup.schema.json#/$defs/SetupBrainTurnResponse", response);
    assert_eq!(
        response["result"]["conversation_id"],
        "setup-session-resumed-002"
    );
    assert!(has_string(
        &response["result"]["message"],
        "resumed setup answer"
    ));
}

fn assert_setup_brain_default_argv(argv: &[String], prompt: &str) {
    assert_setup_brain_argv(argv, "claude-sonnet-4-6", None, prompt);
}

fn assert_setup_brain_configured_argv(argv: &[String], resume: &str, prompt: &str) {
    assert_setup_brain_argv(argv, "claude-opus-4-5", Some(resume), prompt);
}

#[test]
fn setup_detect_reports_installed_missing_auth_profiles_warnings_and_redacts_secrets() {
    let roots = temp_roots("setup-detect");
    let bin_dir = setup_bin_dir(&roots);
    install_setup_detect_fixture(&roots, &bin_dir);

    let host = setup_host_with_path(&roots, &bin_dir);
    let (installed, stdout, stderr) = call_with_host(host, "setup.detect", setup_empty_params());
    assert_setup_detect_installed_response(&installed, &stdout, &stderr);

    let missing_bin_dir = setup_detect_missing_bin_dir(&roots);
    let missing_host = setup_host_with_path(&roots, &missing_bin_dir);
    let (missing, stdout, stderr) =
        call_with_host(missing_host, "setup.detect", setup_empty_params());
    assert_setup_detect_missing_response(&missing, &stdout, &stderr);
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
    let install_target = setup_install_target(&roots);
    assert_install_target_absent(&install_target);

    let (response, stdout, stderr) = call(
        &roots,
        "setup.install_plan",
        setup_install_plan_params(&install_target),
    );
    assert_setup_install_plan_response(&response);
    assert_install_plan_did_not_create_target(&install_target);
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
    let skill_path = setup_sync_skill_path(&roots);
    let claude_json_path = setup_sync_claude_json_path(&roots);
    let claude_json_sentinel = setup_sync_claude_json_sentinel();
    install_setup_sync_fixture(&skill_path, &claude_json_path, &claude_json_sentinel);

    let (response, stdout, stderr) = call(&roots, "setup.sync_plan", setup_sync_plan_params());
    assert_setup_sync_plan_response(&response);
    assert_setup_sync_fixture_unchanged(&skill_path, &claude_json_path, &claude_json_sentinel);
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
    let bin_dir = setup_bin_dir(&roots);
    let argv_path = setup_brain_default_argv_path(&roots);
    install_setup_brain_default_fixture(&bin_dir, &argv_path);
    let host = setup_host_with_path(&roots, &bin_dir);
    let prompt = "detect and explain setup";
    let (response, stdout, stderr) =
        call_with_host(host, "setup_brain.turn", setup_brain_prompt_params(prompt));
    assert_setup_brain_default_response(&response);
    let argv_text = read_argv_text(&argv_path);
    let argv = parse_argv_lines(&argv_text);
    assert_setup_brain_default_argv(&argv, prompt);
    assert_no_secret(&response, &stdout, &stderr);
}

#[test]
fn setup_brain_turn_uses_configured_model_and_resume_id_from_claude_settings_v1() {
    let roots = temp_roots("setup-brain-configured");
    let bin_dir = setup_bin_dir(&roots);
    let argv_path = setup_brain_configured_argv_path(&roots);
    install_setup_brain_configured_fixture(&bin_dir, &argv_path);
    let host = setup_host_with_path(&roots, &bin_dir);
    let settings_response = create_setup_brain_settings(host.clone());
    assert_setup_brain_settings_response(&settings_response);
    let settings_id = settings_record_id(&settings_response);

    let prompt = "continue setup";
    let resume = "setup-session-default-001";
    let (response, stdout, stderr) = call_with_host(
        host,
        "setup_brain.turn",
        setup_brain_resume_params(settings_id, resume, prompt),
    );
    assert_setup_brain_configured_response(&response);
    let argv_text = read_argv_text(&argv_path);
    let argv = parse_argv_lines(&argv_text);
    assert_setup_brain_configured_argv(&argv, resume, prompt);
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
