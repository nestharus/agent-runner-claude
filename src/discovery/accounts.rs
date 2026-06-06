// declared_role: accessor, filter, formatter, mapper, orchestration, parser, predicate, validator

use std::fs;
use std::path::PathBuf;

use serde_json::{json, Value};

use crate::envelope::decode::RequestEnvelope;
use crate::envelope::error::ProviderFailure;

pub fn handle(request: &RequestEnvelope) -> Result<Value, ProviderFailure> {
    validate_params(request)?;
    let (accounts, warnings) = discover_accounts(&request.host);
    Ok(accounts_response(accounts, warnings))
}

fn validate_params(request: &RequestEnvelope) -> Result<(), ProviderFailure> {
    request.params.as_object().ok_or_else(invalid_params)?;
    Ok(())
}

fn discover_accounts(host: &Value) -> (Vec<Value>, Vec<Value>) {
    let home = home_dir(host);
    let mut accounts = Vec::new();
    let mut warnings = Vec::new();

    if let Some(home) = home {
        append_credentials_account(&home, &mut accounts, &mut warnings);
        append_profile_accounts(&home, &mut accounts, &mut warnings);
    } else {
        warnings.push(home_unavailable_warning());
    }

    (accounts, warnings)
}

fn append_credentials_account(
    home: &std::path::Path,
    accounts: &mut Vec<Value>,
    warnings: &mut Vec<Value>,
) {
    let path = selected_credential_path(home);
    let Some(bytes) = read_optional_bytes(&path) else {
        return;
    };
    match parse_json_bytes(&bytes) {
        Ok(value) => accounts.push(credentials_account(&value)),
        Err(_) => warnings.push(credentials_parse_warning()),
    }
}

fn append_profile_accounts(
    home: &std::path::Path,
    accounts: &mut Vec<Value>,
    warnings: &mut Vec<Value>,
) {
    let path = profiles_config_path(home);
    let Some(bytes) = read_optional_bytes(&path) else {
        return;
    };
    let Ok(value) = parse_json_bytes(&bytes) else {
        warnings.push(profile_parse_warning());
        return;
    };
    accounts.extend(profile_accounts(&value));
}

fn home_unavailable_warning() -> Value {
    json!("HOME is not available; Claude account metadata was not inspected")
}

fn credentials_parse_warning() -> Value {
    json!("Claude credentials metadata could not be parsed")
}

fn profile_parse_warning() -> Value {
    json!("Claude profile metadata could not be parsed")
}

fn accounts_response(accounts: Vec<Value>, warnings: Vec<Value>) -> Value {
    json!({
        "accounts": accounts,
        "warnings": warnings
    })
}

fn selected_credential_path(home: &std::path::Path) -> PathBuf {
    credential_paths(home)
        .into_iter()
        .find(|path| path.is_file())
        .unwrap_or_else(|| default_credential_path(home))
}

fn default_credential_path(home: &std::path::Path) -> PathBuf {
    home.join(".claude").join("credentials.json")
}

fn credential_paths(home: &std::path::Path) -> Vec<PathBuf> {
    vec![
        home.join(".claude").join("credentials.json"),
        home.join(".claude").join(".credentials.json"),
    ]
}

fn profiles_config_path(home: &std::path::Path) -> PathBuf {
    home.join(".claude.json")
}

fn read_optional_bytes(path: &std::path::Path) -> Option<Vec<u8>> {
    fs::read(path).ok()
}

fn parse_json_bytes(bytes: &[u8]) -> Result<Value, serde_json::Error> {
    serde_json::from_slice::<Value>(bytes)
}

fn credentials_account(value: &Value) -> Value {
    json!({
        "id": public_string(value, &["account_id", "id"]).unwrap_or_else(|| "claude-credentials".to_string()),
        "provider": "claude",
        "source": "~/.claude/credentials.json",
        "auth_state": public_string(value, &["auth_state"]).unwrap_or_else(|| "authenticated".to_string()),
        "display_name": public_string(value, &["display_name", "name"]),
        "email": public_string(value, &["email"])
    })
}

fn profile_accounts(value: &Value) -> Vec<Value> {
    if let Some(profiles) = value.get("profiles").and_then(Value::as_object) {
        return profiles
            .iter()
            .map(|(name, profile)| profile_account(name, profile))
            .collect();
    }
    Vec::new()
}

fn profile_account(name: &str, profile: &Value) -> Value {
    json!({
        "id": format!("profile-{name}"),
        "provider": "claude",
        "source": "~/.claude.json profiles",
        "auth_state": "configured",
        "profile": name,
        "display_name": public_string(profile, &["display_name", "name"])
            .unwrap_or_else(|| name.to_string())
    })
}

fn public_string(value: &Value, keys: &[&str]) -> Option<String> {
    owned_optional_string(public_non_empty_string_value(value, keys))
}

fn public_non_empty_string_value<'a>(value: &'a Value, keys: &[&str]) -> Option<&'a str> {
    public_string_value(value, public_keys(keys)).filter(non_empty_string)
}

fn owned_optional_string(value: Option<&str>) -> Option<String> {
    value.map(owned_string)
}

fn public_keys<'a>(keys: &'a [&str]) -> Vec<&'a str> {
    keys.iter()
        .copied()
        .filter(|key| !is_secret_key(key))
        .collect()
}

fn public_string_value<'a>(value: &'a Value, keys: Vec<&str>) -> Option<&'a str> {
    keys.into_iter()
        .find_map(|key| value.get(key).and_then(Value::as_str))
}

fn is_secret_key(key: &str) -> bool {
    let key = key.to_ascii_lowercase();
    key.contains("api_key")
        || key.contains("token")
        || key.contains("secret")
        || key.contains("credential")
        || key.contains("password")
}

fn home_dir(host: &Value) -> Option<PathBuf> {
    home_path_buf(accepted_home_value(host))
}

fn accepted_home_value(host: &Value) -> Option<&str> {
    home_value(host).filter(non_empty_string)
}

fn home_path_buf(value: Option<&str>) -> Option<PathBuf> {
    value.map(path_buf_from_str)
}

fn home_value(host: &Value) -> Option<&str> {
    host.get("env")
        .and_then(|env| env.get("HOME"))
        .and_then(Value::as_str)
}

fn non_empty_string(value: &&str) -> bool {
    !value.is_empty()
}

fn owned_string(value: &str) -> String {
    value.to_string()
}

fn path_buf_from_str(value: &str) -> PathBuf {
    PathBuf::from(value)
}

fn invalid_params() -> ProviderFailure {
    ProviderFailure::invalid_request(
        "invalid_discovery_accounts_params",
        "discovery.accounts params do not match the discovery contract",
    )
}
