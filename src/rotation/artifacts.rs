// declared_role: accessor, formatter, mapper, orchestration

use std::fs;
use std::path::Path;

use serde_json::{json, Value};

use crate::envelope::error::{ErrorCategory, ProviderFailure};

pub fn write_file_artifact(path: &Path, bytes: &[u8]) -> Result<Value, ProviderFailure> {
    write_artifact_bytes(path, bytes)?;
    Ok(file_artifact(path, bytes))
}

fn write_artifact_bytes(path: &Path, bytes: &[u8]) -> Result<(), ProviderFailure> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| unavailable("create", path, error))?;
    }
    crate::fs::atomic::atomic_write_bytes(path, bytes)
        .map_err(|error| unavailable("write", path, error))
}

pub fn file_artifact(path: &Path, bytes: &[u8]) -> Value {
    json!({
        "kind": "file",
        "path": path.display().to_string(),
        "sha256": crate::encoding::sha256_hex(bytes)
    })
}

fn unavailable(action: &str, path: &Path, error: std::io::Error) -> ProviderFailure {
    ProviderFailure::new(
        ErrorCategory::Unavailable,
        "rotation_artifact_io_failed",
        format!(
            "failed to {action} provider rotation artifact at {}: {error}",
            path.display()
        ),
        true,
    )
}
