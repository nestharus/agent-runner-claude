// declared_role: accessor, filter, formatter, mapper, orchestration, parser, predicate, validator

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

struct DetectionInputs {
    home: Option<PathBuf>,
    binary_path: Option<PathBuf>,
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
    detect_state_from_inputs(detection_inputs(host))
}

fn detection_inputs(host: &Value) -> DetectionInputs {
    let env = env_map(host);
    DetectionInputs {
        home: home_from_env(&env),
        binary_path: find_binary(path_from_env(&env)),
    }
}

fn detect_state_from_inputs(inputs: DetectionInputs) -> DetectState {
    let installed = detection_installed(&inputs);
    let auth = detected_auth(inputs.home.as_deref());
    let profiles = detected_profiles(inputs.home.as_deref());
    let warnings = detection_warnings(installed, detection_home_available(&inputs));

    DetectState {
        installed,
        binary_path: inputs.binary_path,
        auth,
        profiles,
        warnings,
    }
}

fn home_from_env(env: &std::collections::BTreeMap<String, String>) -> Option<PathBuf> {
    env.get("HOME").map(PathBuf::from)
}

fn path_from_env(env: &std::collections::BTreeMap<String, String>) -> Option<&String> {
    env.get("PATH")
}

fn detection_installed(inputs: &DetectionInputs) -> bool {
    inputs.binary_path.is_some()
}

fn detection_home_available(inputs: &DetectionInputs) -> bool {
    inputs.home.is_some()
}

fn detected_auth(home: Option<&Path>) -> String {
    home.map(read_auth_state)
        .unwrap_or_else(|| "unknown".to_string())
}

fn detected_profiles(home: Option<&Path>) -> Vec<Value> {
    home.map(read_profiles).unwrap_or_default()
}

fn detection_warnings(installed: bool, home_available: bool) -> Vec<Value> {
    let mut warnings = Vec::new();
    if !installed {
        warnings.push(binary_missing_warning());
    }
    if !home_available {
        warnings.push(profiles_home_unavailable_warning());
    }
    warnings
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
    env_object(host)
        .map(|env| env_string_entries(env).into_iter().collect())
        .unwrap_or_default()
}

fn env_object(host: &Value) -> Option<&serde_json::Map<String, Value>> {
    host.get("env").and_then(Value::as_object)
}

fn env_string_entries(env: &serde_json::Map<String, Value>) -> Vec<(String, String)> {
    accepted_env_entries(env)
        .into_iter()
        .map(env_entry_owned_value)
        .collect()
}

fn accepted_env_entries(env: &serde_json::Map<String, Value>) -> Vec<(&String, &Value)> {
    env.iter().filter(env_entry_has_string_value).collect()
}

fn env_entry_has_string_value(entry: &(&String, &Value)) -> bool {
    env_string_value(entry.1).is_some()
}

fn env_entry_owned_value(entry: (&String, &Value)) -> (String, String) {
    let (key, value) = entry;
    env_entry_value(
        key,
        env_string_value(value).expect("accepted env value is a string"),
    )
}

fn env_string_value(value: &Value) -> Option<&str> {
    value.as_str()
}

fn env_entry_value(key: &str, value: &str) -> (String, String) {
    (key.to_string(), value.to_string())
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
        .unwrap_or_else(|| default_credential_path(home))
}

fn default_credential_path(home: &Path) -> PathBuf {
    home.join(".claude").join("credentials.json")
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
    owned_optional_string(public_non_empty_string_value(value, keys))
}

fn public_non_empty_string_value<'a>(value: &'a Value, keys: &[&str]) -> Option<&'a str> {
    public_string_value(value, keys).filter(non_empty_string)
}

fn owned_optional_string(value: Option<&str>) -> Option<String> {
    value.map(owned_string)
}

fn public_string_value<'a>(value: &'a Value, keys: &[&str]) -> Option<&'a str> {
    keys.iter()
        .find_map(|key| value.get(*key).and_then(Value::as_str))
}

fn non_empty_string(value: &&str) -> bool {
    !value.is_empty()
}

fn owned_string(value: &str) -> String {
    value.to_string()
}

fn invalid_params() -> ProviderFailure {
    ProviderFailure::invalid_request(
        "invalid_setup_detect_params",
        "setup.detect params do not match the setup contract",
    )
}
