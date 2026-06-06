// declared_role: accessor, formatter, mapper, orchestration
// adapter_declarations:
//   - component: tests/support/fixtures.rs
//     role: adapter
//     Translates:
//       - contract/v1/common.schema.json#/$defs/RequestEnvelope
//       - contract/v1/common.schema.json#/$defs/HostContext
//       - provider contract id fixture constant
//       - isolated test temp-root fixture surface
//       - test path display string surface

use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

pub const CONTRACT: &str = "oulipoly.provider/v1";

static NEXT_ID: AtomicU64 = AtomicU64::new(1);

#[derive(Debug)]
pub struct TempRoots {
    pub root: PathBuf,
    pub home: PathBuf,
    pub config_root: PathBuf,
    pub data_root: PathBuf,
}

impl Drop for TempRoots {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

pub fn envelope(contract: &str, host: Value, request_value: Value) -> Value {
    json!({
        "contract": contract,
        "request_id": unique_id("req"),
        "provider_instance_id": "claude-primary",
        "host": host,
        "params": request_value
    })
}

pub fn request(request_value: Value) -> Value {
    let roots = temp_roots("request");
    envelope(CONTRACT, host_context(&roots), request_value)
}

pub fn host_context(roots: &TempRoots) -> Value {
    json!({
        "app": "oulipoly-agent-runner",
        "app_version": "0.0.0-test",
        "platform": std::env::consts::OS,
        "working_directory": roots.root.display().to_string(),
        "config_root": roots.config_root.display().to_string(),
        "data_root": roots.data_root.display().to_string(),
        "env": {
            "HOME": roots.home.display().to_string(),
            "TERM": "xterm-256color"
        }
    })
}

pub fn temp_roots(label: &str) -> TempRoots {
    let root = unique_temp_dir(label);
    let home = root.join("home");
    let config_root = root.join("config");
    let data_root = root.join("data");
    for dir in [&home, &config_root, &data_root] {
        std::fs::create_dir_all(dir).expect("create temp root child");
    }
    TempRoots {
        root,
        home,
        config_root,
        data_root,
    }
}

pub fn unique_temp_dir(label: &str) -> PathBuf {
    let safe_label = safe_temp_label(label);
    let path = unique_temp_path(&safe_label);
    prepare_unique_temp_dir(&path);
    path
}

fn safe_temp_label(label: &str) -> String {
    label
        .chars()
        .map(|ch| if ch.is_ascii_alphanumeric() { ch } else { '-' })
        .collect()
}

fn unique_temp_path(safe_label: &str) -> PathBuf {
    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!(
        "agent-runner-claude-{safe_label}-{}-{id}",
        std::process::id()
    ))
}

fn prepare_unique_temp_dir(path: &Path) {
    let _ = std::fs::remove_dir_all(path);
    std::fs::create_dir_all(path).expect("create unique temp dir");
}

pub fn path_string(path: &Path) -> String {
    path.display().to_string()
}

fn unique_id(prefix: &str) -> String {
    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    format!("{prefix}-{id}")
}
