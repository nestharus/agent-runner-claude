// declared_role: formatter, mapper

use serde_json::{json, Value};

pub fn planned_write(provider_root: &str, content: &Value) -> Value {
    json!({
        "kind": "write_file",
        "provider_owned": true,
        "confirmed": true,
        "path": format!("{provider_root}/settings.v1.json"),
        "content": action_content(content),
        "description": "write migrated claude.settings/v1 provider configuration"
    })
}

fn action_content(content: &Value) -> Value {
    json!({
        "encoding": "utf8",
        "data": serde_json::to_string(content).expect("migration content must serialize")
    })
}
