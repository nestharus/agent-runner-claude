// declared_role: orchestration, mapper, accessor, validator, formatter, predicate, parser

mod support;

use serde_json::{json, Value};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use support::fixtures::{envelope, host_context, temp_roots, TempRoots, CONTRACT};
use support::invoke::{invoke, parse_one_stdout_json};
use support::schema::assert_valid;

const SECRET: &str = "CHAR_SETUP_SECRET_SENTINEL_W2C_DO_NOT_LEAK";

fn host_with_path(roots: &TempRoots, bin_dir: &Path) -> Value {
    let mut host = host_context(roots);
    host["env"]
        .as_object_mut()
        .unwrap()
        .insert("PATH".to_string(), json!(bin_dir.display().to_string()));
    host
}

fn write_executable(path: &Path, body: &str) {
    fs::write(path, body).expect("write executable");
    let mut perms = fs::metadata(path).unwrap().permissions();
    perms.set_mode(0o755);
    fs::set_permissions(path, perms).unwrap();
}

fn fake_claude(bin_dir: &Path, argv_path: &Path, stdout_json: &str, stderr_text: &str) {
    fs::create_dir_all(bin_dir).unwrap();
    write_executable(
        &bin_dir.join("claude"),
        &fake_claude_script(argv_path, stdout_json, stderr_text),
    );
}

fn fake_claude_script(argv_path: &Path, stdout_json: &str, stderr_text: &str) -> String {
    format!(
        "#!/bin/sh\nprintf '%s\\n' \"$@\" > '{}'\nprintf '%s\\n' '{}'\nprintf '%s\\n' '{}' >&2\n",
        argv_path.display(),
        stdout_json,
        stderr_text
    )
}

fn call(host: Value, subcommand: &str, params: Value) -> (Value, Vec<u8>, Vec<u8>) {
    let request = envelope(CONTRACT, host, params);
    let output = invoke(subcommand, &request);
    assert_success_code(&output);
    let response = parse_one_stdout_json(&output);
    (response, output.stdout, output.stderr)
}

fn assert_success_code(output: &support::invoke::Invocation) {
    assert_eq!(output.code, Some(0));
}

fn argv(path: &Path) -> Vec<String> {
    parse_argv_text(&argv_text(path))
}

fn argv_text(path: &Path) -> String {
    fs::read_to_string(path).unwrap()
}

fn parse_argv_text(text: &str) -> Vec<String> {
    text.lines().map(str::to_string).collect()
}

fn assert_no_secret(value: &Value, stdout: &[u8], stderr: &[u8]) {
    assert!(!value.to_string().contains(SECRET));
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
fn fixture_claude_pins_setup_brain_argv_stdout_stderr_and_session_prefixes() {
    let roots = temp_roots("char-setup-brain");
    let bin_dir = roots.root.join("bin");
    let argv_default = roots.root.join("argv-default.txt");
    fake_claude(
        &bin_dir,
        &argv_default,
        r#"{"role":"assistant","content":"default setup"}"#,
        "Session: char-session-default",
    );
    let host = host_with_path(&roots, &bin_dir);
    let prompt = "characterize setup brain default";
    let (response, stdout, stderr) = call(
        host.clone(),
        "setup_brain.turn",
        json!({ "prompt": prompt }),
    );
    assert_default_setup_brain_response(&response, &stdout, &stderr, &argv_default, prompt);

    let argv_resume = roots.root.join("argv-resume.txt");
    fake_claude(
        &bin_dir,
        &argv_resume,
        r#"{"role":"assistant","content":"resumed setup"}"#,
        "session_id: char-session-resumed",
    );
    let resume_prompt = "characterize setup brain resume";
    let (response, stdout, stderr) = call(
        host,
        "setup_brain.turn",
        json!({ "prompt": resume_prompt, "resume": "char-session-default" }),
    );
    assert_resume_setup_brain_response(&response, &stdout, &stderr, &argv_resume, resume_prompt);
}

fn assert_default_setup_brain_response(
    response: &Value,
    stdout: &[u8],
    stderr: &[u8],
    argv_default: &Path,
    prompt: &str,
) {
    assert_valid("setup.schema.json#/$defs/SetupBrainTurnResponse", response);
    assert_eq!(
        response["result"]["conversation_id"],
        "char-session-default"
    );
    assert_eq!(response["result"]["message"]["content"], "default setup");
    let captured = argv(argv_default);
    assert_eq!(captured[0], "-p");
    assert_eq!(
        &captured[1..8],
        [
            "--output-format",
            "json",
            "--model",
            "claude-sonnet-4-6",
            "--allowedTools",
            "Read,Bash,Glob,Grep",
            "--no-session-persistence"
        ]
    );
    assert_eq!(captured[8], "--json-schema");
    serde_json::from_str::<Value>(&captured[9]).expect("schema arg must be JSON");
    assert_eq!(captured.last().unwrap(), prompt);
    assert_no_secret(response, stdout, stderr);
}

fn assert_resume_setup_brain_response(
    response: &Value,
    stdout: &[u8],
    stderr: &[u8],
    argv_resume: &Path,
    resume_prompt: &str,
) {
    assert_valid("setup.schema.json#/$defs/SetupBrainTurnResponse", response);
    assert_eq!(
        response["result"]["conversation_id"],
        "char-session-resumed"
    );
    assert_eq!(response["result"]["message"]["content"], "resumed setup");
    let captured = argv(argv_resume);
    assert_eq!(
        &captured[captured.len() - 3..],
        ["--resume", "char-session-default", resume_prompt]
    );
    assert_no_secret(response, stdout, stderr);
}

#[test]
fn setup_detect_characterizes_claude_home_layout_auth_metadata_redaction_and_profiles() {
    let roots = temp_roots("char-setup-detect");
    let bin_dir = roots.root.join("bin");
    fs::create_dir_all(&bin_dir).unwrap();
    write_executable(
        &bin_dir.join("claude"),
        "#!/bin/sh\nprintf 'Claude Code 9.9.9\\n'\n",
    );
    let claude_home = roots.home.join(".claude");
    fs::create_dir_all(claude_home.join("profiles/personal")).unwrap();
    fs::write(claude_home.join("credentials.json"), credentials_fixture()).unwrap();
    fs::write(
        claude_home.join("profiles/personal/settings.json"),
        profile_fixture(),
    )
    .unwrap();

    let host = host_with_path(&roots, &bin_dir);
    let (response, stdout, stderr) = call(host, "setup.detect", json!({}));
    assert_setup_detect_response(&response, &stdout, &stderr);
}

fn credentials_fixture() -> String {
    format!(
        r#"{{"auth_state":"authenticated","display_name":"Personal Claude","email":"personal@example.test","token":"{SECRET}"}}"#
    )
}

fn profile_fixture() -> String {
    format!(r#"{{"display_name":"Personal Profile","api_key":"{SECRET}"}}"#)
}

fn assert_setup_detect_response(response: &Value, stdout: &[u8], stderr: &[u8]) {
    assert_valid("setup.schema.json#/$defs/SetupDetectResponse", response);
    assert!(response["result"]["installed"].as_bool().unwrap());
    assert!(
        has_string(&response["result"]["binary"], "Claude Code 9.9.9")
            || has_string(&response["result"]["binary"], "claude")
    );
    assert!(
        has_string(&response["result"], "Personal Claude")
            || has_string(&response["result"], "personal@example.test")
    );
    assert!(
        has_string(&response["result"]["profiles"], "Personal Profile")
            || has_string(&response["result"]["profiles"], "personal")
    );
    assert_no_secret(response, stdout, stderr);
}

#[test]
fn setup_sync_characterizes_skills_mcp_config_plans_and_no_overwrite_conflicts() {
    let roots = temp_roots("char-setup-sync");
    let skill = roots.home.join(".claude/skills/existing/SKILL.md");
    let claude_json_path = roots.home.join(".claude.json");
    fs::create_dir_all(skill.parent().unwrap()).unwrap();
    fs::write(&skill, "USER_EXISTING_SKILL").unwrap();
    let claude_json_sentinel = claude_json_sentinel();
    fs::write(&claude_json_path, &claude_json_sentinel).unwrap();

    let (response, stdout, stderr) = call(
        host_context(&roots),
        "setup.sync_plan",
        json!({
            "skills": [{ "name": "existing", "body": "provider replacement" }],
            "mcp_servers": [{ "name": "existing", "command": "agent-runner-claude mcp" }],
            "config": { "permissions": { "allow": ["Read", "Bash"] } },
            "overwrite": false
        }),
    );
    assert_setup_sync_response(
        &response,
        &stdout,
        &stderr,
        &skill,
        &claude_json_path,
        &claude_json_sentinel,
    );
}

fn claude_json_sentinel() -> String {
    format!(
        r#"{{"mcpServers":{{"existing":{{"command":"user-command","secret":"{SECRET}"}}}},"permissions":{{"allow":["Read"]}}}}"#
    )
}

fn assert_setup_sync_response(
    response: &Value,
    stdout: &[u8],
    stderr: &[u8],
    skill: &Path,
    claude_json_path: &Path,
    claude_json_sentinel: &str,
) {
    assert_valid("setup.schema.json#/$defs/SetupSyncPlanResponse", response);
    assert!(has_string(&response["result"]["operations"], "skill"));
    assert!(has_string(&response["result"]["operations"], "mcp"));
    assert!(has_string(&response["result"]["operations"], "config"));
    assert!(has_string(&response["result"]["diagnostics"], "conflict"));
    assert_eq!(fs::read_to_string(skill).unwrap(), "USER_EXISTING_SKILL");
    assert_eq!(
        fs::read_to_string(claude_json_path).unwrap(),
        claude_json_sentinel
    );
    assert_no_secret(response, stdout, stderr);
}
