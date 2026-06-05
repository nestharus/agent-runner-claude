// declared_role: formatter

use serde_json::{json, Value};

pub const SCHEMA_ID: &str = "claude.settings/v1";

pub fn settings_schema() -> Value {
    json!({
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "$id": "https://schemas.oulipoly.local/claude.settings/v1",
        "title": "Claude Code Provider Settings",
        "type": "object",
        "additionalProperties": true,
        "properties": {
            "command": { "type": "string", "default": "claude" },
            "args": { "type": "array", "items": { "type": "string" }, "default": [] },
            "api_key": { "type": "string", "writeOnly": true },
            "auth_token": { "type": "string", "writeOnly": true },
            "tool_restrictions": { "type": "object", "additionalProperties": true },
            "quota_script": { "type": "string" },
            "auth_refresh_command": { "type": "string" },
            "session_storage": { "type": "object", "additionalProperties": true },
            "setup_brain_model": { "type": "string", "default": "claude-sonnet-4-6" }
        }
    })
}
