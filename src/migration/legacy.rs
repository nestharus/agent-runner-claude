// declared_role: formatter, mapper

use serde_json::{json, Value};

pub fn planned_write(provider_root: &str) -> Value {
    json!({
        "kind": "write_file",
        "provider_owned": true,
        "path": format!("{provider_root}/settings.v1.json"),
        "description": "write migrated claude.settings/v1 provider configuration"
    })
}
