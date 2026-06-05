// declared_role: accessor, filter, formatter, mapper, orchestration, parser, validator

use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use crate::envelope::decode::RequestEnvelope;
use crate::envelope::error::ProviderFailure;

struct DetectState {
    installed: bool,
    binary_path: Option<PathBuf>,
    auth: String,
    profiles: Vec<Value>,
    warnings: Vec<Value>,
}

pub fn handle(request: &RequestEnvelope) -> Result<Value, ProviderFailure> {
    validate_params(request)?;
    Ok(detect_response(detect_state(&request.host)))
}

fn validate_params(request: &RequestEnvelope) -> Result<(), ProviderFailure> {
    request.params.as_object().ok_or_else(invalid_params)?;
    Ok(())
}

fn detect_state(host: &Value) -> DetectState {
    let env = env_map(host);
    let home = env.get("HOME").map(PathBuf::from);
    let binary_path = find_binary(env.get("PATH"));
    let installed = binary_path.is_some();
    let mut warnings = Vec::new();
    if !installed {
        warnings.push(binary_missing_warning());
    }
    let profiles = home.as_deref().map(read_profiles).unwrap_or_else(|| {
        warnings.push(profiles_home_unavailable_warning());
        Vec::new()
    });
    let auth = home
        .as_deref()
        .map(read_auth_state)
        .unwrap_or_else(|| "unknown".to_string());

    DetectState {
        installed,
        binary_path,
        auth,
        profiles,
        warnings,
    }
}

fn binary_missing_warning() -> Value {
    json!("claude binary was not found on PATH")
}

fn profiles_home_unavailable_warning() -> Value {
    json!("HOME is not available; Claude profiles were not inspected")
}

fn detect_response(state: DetectState) -> Value {
    json!({
        "installed": state.installed,
        "binary": state.binary_path.map(|path| json!({
            "name": "claude",
            "path": path.display().to_string()
        })).unwrap_or_else(|| json!({ "name": "claude", "status": "missing" })),
        "auth": state.auth,
        "profiles": state.profiles,
        "warnings": state.warnings
    })
}

fn env_map(host: &Value) -> std::collections::BTreeMap<String, String> {
    host.get("env")
        .and_then(Value::as_object)
        .map(|env| {
            env.iter()
                .filter_map(|(key, value)| {
                    value.as_str().map(|value| (key.clone(), value.to_string()))
                })
                .collect()
        })
        .unwrap_or_default()
}

fn find_binary(path: Option<&String>) -> Option<PathBuf> {
    find_existing_binary(binary_candidates(non_empty_path_segments(path_segments(
        path?,
    ))))
}

fn path_segments(path: &str) -> impl Iterator<Item = &str> {
    path.split(':')
}

fn non_empty_path_segments<'a>(
    segments: impl Iterator<Item = &'a str>,
) -> impl Iterator<Item = &'a str> {
    segments.filter(|segment| !segment.is_empty())
}

fn binary_candidates<'a>(segments: impl Iterator<Item = &'a str>) -> impl Iterator<Item = PathBuf> {
    segments.map(binary_candidate_path)
}

fn binary_candidate_path(segment: &str) -> PathBuf {
    Path::new(segment).join("claude")
}

fn find_existing_binary(mut candidates: impl Iterator<Item = PathBuf>) -> Option<PathBuf> {
    candidates.find(|candidate| binary_candidate_exists(candidate))
}

fn binary_candidate_exists(candidate: &Path) -> bool {
    candidate.is_file()
}

fn read_auth_state(home: &Path) -> String {
    let path = selected_credential_path(home);
    let Some(bytes) = read_optional_bytes(&path) else {
        return "unauthenticated".to_string();
    };
    let Ok(value) = parse_json_bytes(&bytes) else {
        return "unknown".to_string();
    };
    auth_state_summary(&value)
}

fn selected_credential_path(home: &Path) -> PathBuf {
    credential_paths(home)
        .into_iter()
        .find(|path| path.is_file())
        .unwrap_or_else(|| home.join(".claude").join("credentials.json"))
}

fn auth_state_summary(value: &Value) -> String {
    format_auth_state_summary(auth_state_parts(value))
}

struct AuthStateParts {
    state: String,
    display: Option<String>,
}

fn auth_state_parts(value: &Value) -> AuthStateParts {
    let state = value
        .get("auth_state")
        .and_then(Value::as_str)
        .unwrap_or("authenticated")
        .to_string();
    let display = public_string(value, &["display_name", "email"]);
    AuthStateParts { state, display }
}

fn format_auth_state_summary(parts: AuthStateParts) -> String {
    parts.display.map_or_else(
        || parts.state.clone(),
        |display| format!("{}: {display}", parts.state),
    )
}

fn credential_paths(home: &Path) -> Vec<PathBuf> {
    vec![
        home.join(".claude").join("credentials.json"),
        home.join(".claude").join(".credentials.json"),
    ]
}

fn read_profiles(home: &Path) -> Vec<Value> {
    let profile_root = profiles_root(home);
    profile_dirs(&profile_root)
        .into_iter()
        .map(|path| profile_metadata(&path))
        .collect()
}

fn profiles_root(home: &Path) -> PathBuf {
    home.join(".claude").join("profiles")
}

fn profile_dirs(profile_root: &Path) -> Vec<PathBuf> {
    read_profile_entries(profile_root)
        .into_iter()
        .filter(|path| path.is_dir())
        .collect()
}

fn read_profile_entries(profile_root: &Path) -> Vec<PathBuf> {
    fs::read_dir(profile_root)
        .map(|entries| entries.flatten().map(|entry| entry.path()).collect())
        .unwrap_or_default()
}

fn profile_metadata(path: &Path) -> Value {
    let name = profile_name(path);
    let display_name = profile_settings(path)
        .and_then(|value| public_string(&value, &["display_name", "name"]))
        .unwrap_or_else(|| name.clone());
    profile_response(name, display_name)
}

fn profile_settings(path: &Path) -> Option<Value> {
    read_optional_bytes(&profile_settings_path(path))
        .and_then(|bytes| parse_json_bytes(&bytes).ok())
}

fn profile_settings_path(path: &Path) -> PathBuf {
    path.join("settings.json")
}

fn profile_name(path: &Path) -> String {
    path.file_name()
        .map(|value| value.to_string_lossy().to_string())
        .unwrap_or_default()
}

fn profile_response(name: String, display_name: String) -> Value {
    json!({
        "name": name,
        "display_name": display_name,
        "source": "~/.claude/profiles"
    })
}

fn read_optional_bytes(path: &Path) -> Option<Vec<u8>> {
    fs::read(path).ok()
}

fn parse_json_bytes(bytes: &[u8]) -> Result<Value, serde_json::Error> {
    serde_json::from_slice::<Value>(bytes)
}

fn public_string(value: &Value, keys: &[&str]) -> Option<String> {
    keys.iter()
        .find_map(|key| value.get(*key).and_then(Value::as_str))
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn invalid_params() -> ProviderFailure {
    ProviderFailure::invalid_request(
        "invalid_setup_detect_params",
        "setup.detect params do not match the setup contract",
    )
}
