use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::BTreeMap;

pub const CONTRACT: &str = "oulipoly.provider/v1";
pub const SETTINGS_SCHEMA_ID: &str = "claude.settings/v1";
const SETTINGS_SCHEMA_URI: &str = "https://oulipoly.dev/schemas/claude.settings/v1";

const KNOWN_LATER_SUBCOMMANDS: &[&str] = &[
    "settings.list",
    "settings.get",
    "settings.create",
    "settings.update",
    "settings.delete",
    "settings.validate",
    "settings.migrate",
    "policy.evaluate",
    "launch",
    "terminal.classify",
    "quota.source",
    "quota.probe",
    "quota.refresh_auth",
    "session.locate_transcript",
    "session.read_turns",
    "session.capture",
    "session.export",
    "session.replace",
    "rotation.assess",
    "rotation.materialize",
    "discovery.models",
    "discovery.accounts",
    "setup.detect",
    "setup.install_plan",
    "setup.sync_plan",
    "setup_brain.turn",
    "migration.plan",
    "migration.apply",
];

#[derive(Debug, PartialEq, Eq)]
pub struct InvocationOutput {
    pub stdout: String,
    pub exit_code: i32,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RequestEnvelope {
    contract: String,
    request_id: String,
    #[allow(dead_code)]
    provider_instance_id: Option<String>,
    host: HostContext,
    params: Value,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct HostContext {
    app: String,
    #[allow(dead_code)]
    app_version: Option<String>,
    #[allow(dead_code)]
    platform: Option<String>,
    #[allow(dead_code)]
    working_directory: Option<String>,
    #[allow(dead_code)]
    config_root: Option<String>,
    #[allow(dead_code)]
    data_root: Option<String>,
    #[allow(dead_code)]
    env: Option<BTreeMap<String, String>>,
    #[allow(dead_code)]
    deadline_unix_ms: Option<u64>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SchemaParams {
    schema_id: String,
}

#[derive(Debug)]
struct ProviderFailure {
    request_id: String,
    code: &'static str,
    category: &'static str,
    message: String,
    retryable: bool,
    details: Value,
    exit_code: i32,
}

impl ProviderFailure {
    fn invalid_request(request_id: String, code: &'static str, message: impl Into<String>) -> Self {
        Self {
            request_id,
            code,
            category: "invalid_request",
            message: message.into(),
            retryable: false,
            details: json!({}),
            exit_code: 2,
        }
    }

    fn unsupported(
        request_id: String,
        code: &'static str,
        message: impl Into<String>,
        exit_code: i32,
    ) -> Self {
        Self {
            request_id,
            code,
            category: "unsupported",
            message: message.into(),
            retryable: false,
            details: json!({}),
            exit_code,
        }
    }
}

pub fn handle_invocation(args: &[String], stdin: &str) -> InvocationOutput {
    match handle_invocation_result(args, stdin) {
        Ok(value) => InvocationOutput {
            stdout: serde_json::to_string(&value).expect("response serialization is infallible"),
            exit_code: 0,
        },
        Err(failure) => InvocationOutput {
            stdout: error_response(&failure),
            exit_code: failure.exit_code,
        },
    }
}

fn handle_invocation_result(args: &[String], stdin: &str) -> Result<Value, ProviderFailure> {
    let request = decode_request(stdin)?;
    let subcommand = match args {
        [_, subcommand] => subcommand.as_str(),
        [_] => {
            return Err(ProviderFailure::unsupported(
                request.request_id,
                "missing_subcommand",
                "provider invocation requires exactly one subcommand argument",
                3,
            ));
        }
        _ => {
            return Err(ProviderFailure::invalid_request(
                request.request_id,
                "invalid_argv",
                "provider invocation accepts exactly one subcommand argument",
            ));
        }
    };

    match subcommand {
        "describe" => Ok(success_response(&request.request_id, describe_result())),
        "schema" => schema_response(request),
        known if KNOWN_LATER_SUBCOMMANDS.contains(&known) => Err(ProviderFailure::unsupported(
            request.request_id,
            "capability_not_implemented",
            format!("{known} is advertised for the Claude provider but is not implemented in this foundation slice"),
            3,
        )),
        unknown => Err(ProviderFailure::unsupported(
            request.request_id,
            "unsupported_subcommand",
            format!("unsupported provider subcommand: {unknown}"),
            3,
        )),
    }
}

fn decode_request(stdin: &str) -> Result<RequestEnvelope, ProviderFailure> {
    let raw: Value = serde_json::from_str(stdin).map_err(|err| {
        ProviderFailure::invalid_request(
            "unknown".to_string(),
            "invalid_json",
            format!("stdin must be one UTF-8 JSON object: {err}"),
        )
    })?;
    let request_id = raw
        .get("request_id")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .unwrap_or("unknown")
        .to_string();
    let request: RequestEnvelope = serde_json::from_value(raw).map_err(|err| {
        ProviderFailure::invalid_request(
            request_id.clone(),
            "invalid_envelope",
            format!("request envelope does not match the provider contract: {err}"),
        )
    })?;

    if request.contract != CONTRACT {
        return Err(ProviderFailure::unsupported(
            request.request_id,
            "unsupported_version",
            format!("unsupported contract version: {}", request.contract),
            3,
        ));
    }
    if request.request_id.trim().is_empty() {
        return Err(ProviderFailure::invalid_request(
            "unknown".to_string(),
            "invalid_request_id",
            "request_id must be a non-empty string",
        ));
    }
    if request.host.app.trim().is_empty() {
        return Err(ProviderFailure::invalid_request(
            request.request_id,
            "invalid_host",
            "host.app must be a non-empty string",
        ));
    }

    Ok(request)
}

fn schema_response(request: RequestEnvelope) -> Result<Value, ProviderFailure> {
    let params: SchemaParams = serde_json::from_value(request.params).map_err(|err| {
        ProviderFailure::invalid_request(
            request.request_id.clone(),
            "invalid_schema_params",
            format!("schema params must contain schema_id only: {err}"),
        )
    })?;
    if params.schema_id != SETTINGS_SCHEMA_ID {
        return Err(ProviderFailure::unsupported(
            request.request_id,
            "unknown_schema",
            format!("unknown provider schema id: {}", params.schema_id),
            1,
        ));
    }

    Ok(success_response(
        &request.request_id,
        json!({
            "schema_id": SETTINGS_SCHEMA_ID,
            "schema": settings_schema(),
            "ui": settings_schema_ui(),
        }),
    ))
}

fn success_response(request_id: &str, result: Value) -> Value {
    json!({
        "contract": CONTRACT,
        "request_id": request_id,
        "ok": true,
        "result": result,
    })
}

fn error_response(failure: &ProviderFailure) -> String {
    let response = json!({
        "contract": CONTRACT,
        "request_id": failure.request_id,
        "ok": false,
        "error": {
            "code": failure.code,
            "category": failure.category,
            "message": failure.message,
            "retryable": failure.retryable,
            "details": failure.details,
        },
    });
    serde_json::to_string(&response).expect("error serialization is infallible")
}

pub fn describe_result() -> Value {
    json!({
        "provider_id": "claude",
        "display_name": "Claude Code",
        "contract_versions": [CONTRACT],
        "preferred_contract": CONTRACT,
        "capabilities": {
            "launch": true,
            "policy": true,
            "quota": true,
            "session": true,
            "terminal": true,
            "rotation": true,
            "discovery": true,
            "settings": true,
            "setup_brain": true,
            "setup": true,
            "migration": true,
        },
        "settings_schema_id": SETTINGS_SCHEMA_ID,
        "concurrency": {
            "safe_for_parallel_invocation": true,
            "state_locking": "atomic_file_writes_and_provider_cli_owned_state",
            "settings_version_tokens": true,
            "stdout_protocol_only": true,
            "notes": "This provider is one-shot and daemonless; future settings mutations must use version tokens and atomic persistence.",
        },
    })
}

pub fn settings_schema() -> Value {
    json!({
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "$id": SETTINGS_SCHEMA_URI,
        "title": "Claude Provider Settings",
        "type": "object",
        "required": ["command"],
        "additionalProperties": false,
        "properties": {
            "name": {
                "type": "string",
                "minLength": 1,
                "description": "Stable provider account identifier. When omitted during migration, agent-runner derives it from command and args."
            },
            "command": {
                "type": "string",
                "minLength": 1,
                "default": "env -u CLAUDECODE claude",
                "description": "Base Claude executable command, including any env prefix."
            },
            "args": {
                "type": "array",
                "items": { "type": "string" },
                "default": []
            },
            "interactive_args": {
                "type": "array",
                "items": { "type": "string" }
            },
            "prompt_mode": {
                "type": "string",
                "enum": ["stdin", "arg"],
                "default": "stdin"
            },
            "invocation_mode": {
                "type": "string",
                "enum": ["headless", "proxy"],
                "default": "headless"
            },
            "quota_script": {
                "type": "string",
                "description": "Command that emits Claude quota windows, commonly anthropic-usage against a Claude credentials file."
            },
            "auth_refresh_command": {
                "type": "string",
                "default": "claude auth status"
            },
            "resume": {
                "oneOf": [
                    {
                        "type": "object",
                        "required": ["kind", "flag"],
                        "additionalProperties": false,
                        "properties": {
                            "kind": { "const": "flag" },
                            "flag": { "type": "string", "minLength": 1 }
                        }
                    },
                    {
                        "type": "object",
                        "required": ["kind", "subcommand"],
                        "additionalProperties": false,
                        "properties": {
                            "kind": { "const": "subcommand" },
                            "subcommand": {
                                "type": "array",
                                "items": { "type": "string", "minLength": 1 },
                                "minItems": 1
                            }
                        }
                    }
                ]
            },
            "session_capture": {
                "oneOf": [
                    {
                        "type": "object",
                        "required": ["kind"],
                        "additionalProperties": false,
                        "properties": {
                            "kind": { "const": "none" }
                        }
                    },
                    {
                        "type": "object",
                        "required": ["kind", "flag"],
                        "additionalProperties": false,
                        "properties": {
                            "kind": { "const": "forced_flag_verified" },
                            "flag": { "type": "string", "minLength": 1 },
                            "readback_args": {
                                "type": "array",
                                "items": { "type": "string" }
                            }
                        }
                    },
                    {
                        "type": "object",
                        "required": [
                            "kind",
                            "json_flag",
                            "last_message_flag",
                            "event_type",
                            "event_id_path"
                        ],
                        "additionalProperties": false,
                        "properties": {
                            "kind": { "const": "stdout_json_event" },
                            "json_flag": { "type": "string", "minLength": 1 },
                            "last_message_flag": { "type": "string", "minLength": 1 },
                            "event_type": { "type": "string", "minLength": 1 },
                            "event_id_path": { "type": "string", "minLength": 1 }
                        }
                    }
                ]
            },
            "resume_acceptance": {
                "type": "object",
                "additionalProperties": false,
                "properties": {
                    "accepted_output_patterns": {
                        "type": "array",
                        "items": { "type": "string", "minLength": 1 }
                    },
                    "rejected_output_patterns": {
                        "type": "array",
                        "items": { "type": "string", "minLength": 1 }
                    }
                }
            },
            "session_storage": {
                "oneOf": [
                    {
                        "type": "object",
                        "required": ["kind", "projects_dir"],
                        "additionalProperties": false,
                        "properties": {
                            "kind": { "const": "claude_code" },
                            "projects_dir": { "type": "string", "minLength": 1 }
                        }
                    },
                    {
                        "type": "object",
                        "required": ["kind", "cwd_script"],
                        "additionalProperties": false,
                        "properties": {
                            "kind": { "const": "script" },
                            "cwd_script": { "type": "string", "minLength": 1 },
                            "transcript_script": { "type": "string", "minLength": 1 },
                            "storage_type": { "const": "claude_code" }
                        },
                        "dependentRequired": {
                            "transcript_script": ["storage_type"],
                            "storage_type": ["transcript_script"]
                        }
                    }
                ]
            },
            "system_prompt_override": {
                "type": "string"
            },
            "tool_restrictions": {
                "type": "object",
                "required": ["kind"],
                "additionalProperties": false,
                "properties": {
                    "kind": { "const": "claude" },
                    "claude": {
                        "type": "object",
                        "additionalProperties": false,
                        "properties": {
                            "disallowed_tools": {
                                "type": "array",
                                "items": { "type": "string", "minLength": 1 }
                            },
                            "allowed_tools": {
                                "type": "array",
                                "items": { "type": "string", "minLength": 1 }
                            },
                            "disable_slash_commands": {
                                "type": "boolean",
                                "default": false
                            }
                        }
                    }
                }
            }
        }
    })
}

fn settings_schema_ui() -> Value {
    json!({
        "sections": [
            {
                "id": "launch",
                "title": "Launch",
                "fields": ["name", "command", "args", "interactive_args", "prompt_mode", "invocation_mode"]
            },
            {
                "id": "state",
                "title": "State",
                "fields": ["resume", "session_capture", "resume_acceptance", "session_storage"]
            },
            {
                "id": "policy",
                "title": "Policy",
                "fields": ["system_prompt_override", "tool_restrictions"]
            },
            {
                "id": "quota",
                "title": "Quota",
                "fields": ["quota_script", "auth_refresh_command"]
            }
        ]
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(params: Value) -> String {
        json!({
            "contract": CONTRACT,
            "request_id": "req-test",
            "host": { "app": "test" },
            "params": params,
        })
        .to_string()
    }

    #[test]
    fn schema_requires_known_schema_id() {
        let args = vec!["agent-runner-claude".to_string(), "schema".to_string()];
        let output = handle_invocation(&args, &request(json!({ "schema_id": "missing" })));
        assert_eq!(output.exit_code, 1);
        let body: Value = serde_json::from_str(&output.stdout).unwrap();
        assert_eq!(body["ok"], false);
        assert_eq!(body["error"]["code"], "unknown_schema");
    }

    #[test]
    fn unsupported_future_capability_uses_contract_error() {
        let args = vec!["agent-runner-claude".to_string(), "launch".to_string()];
        let output = handle_invocation(&args, &request(json!({})));
        assert_eq!(output.exit_code, 3);
        let body: Value = serde_json::from_str(&output.stdout).unwrap();
        assert_eq!(body["error"]["category"], "unsupported");
    }
}
