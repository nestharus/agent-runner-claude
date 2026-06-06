// declared_role: formatter

use serde_json::{json, Value};

pub const SCHEMA_ID: &str = "claude.settings/v1";

pub fn settings_schema() -> Value {
    json!({
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "$id": "https://schemas.oulipoly.local/claude.settings/v1",
        "title": "Claude Code Provider Settings",
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
                "default": "claude"
            },
            "args": { "type": "array", "items": { "type": "string" }, "default": [] },
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
            "api_key": { "type": "string", "writeOnly": true },
            "auth_token": { "type": "string", "writeOnly": true },
            "quota_script": { "type": "string" },
            "auth_refresh_command": { "type": "string" },
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
            },
            "setup_brain_model": { "type": "string", "default": "claude-sonnet-4-6" }
        }
    })
}
