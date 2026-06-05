// declared_role: accessor, filter, formatter, mapper, orchestration, parser, validator
// intrinsic_surface_declarations:
//   - component: src/settings/store.rs
//     role: intrinsic-surface
//     Domain: claude_provider_settings_store
//     Owns:
//       - "provider settings durable record layout"
//       - "settings JSON write/publish flow using src/fs/atomic.rs"
//       - "advisory-lock-guarded mutation and readback"
//       - "src/fs/paths.rs provider_data_dir host-root resolution seam"
//       - "src/settings/lock.rs SettingsLock acquire/release seam"
//       - "src/settings/version.rs opaque version check/new-version seam"

use std::fs;
use std::io;
use std::path::PathBuf;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::envelope::error::{ErrorCategory, ProviderFailure};

#[derive(Debug, Clone)]
pub struct SettingsStore {
    pub path: PathBuf,
    pub lock_path: PathBuf,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SettingsRecord {
    pub id: String,
    pub display_name: String,
    pub version: String,
    pub values: Value,
}

pub fn setup_brain_model_for_host(
    host: &Value,
    settings_id: &str,
) -> Result<Option<String>, ProviderFailure> {
    let store = SettingsStore::for_host(host)?;
    let record = store.get(settings_id)?;
    Ok(setup_brain_model(&record))
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct StoreFile {
    records: Vec<SettingsRecord>,
}

impl SettingsStore {
    pub fn for_host(host: &Value) -> Result<Self, ProviderFailure> {
        let dir = crate::fs::paths::provider_data_dir(host)?;
        Ok(Self {
            path: dir.join("settings.json"),
            lock_path: dir.join("settings.lock"),
        })
    }

    pub fn list(&self) -> Result<Vec<SettingsRecord>, ProviderFailure> {
        let _lock = self.acquire_lock()?;
        Ok(self.read_unlocked()?.records)
    }

    pub fn get(&self, id: &str) -> Result<SettingsRecord, ProviderFailure> {
        let _lock = self.acquire_lock()?;
        let store = self.read_unlocked()?;
        find_record(&store.records, id)
            .cloned()
            .ok_or_else(|| not_found(id))
    }

    pub fn create(
        &self,
        display_name: Option<&str>,
        values: Value,
    ) -> Result<SettingsRecord, ProviderFailure> {
        let _lock = self.acquire_lock()?;
        let mut store = self.read_unlocked()?;
        let record = new_record(display_name, values);
        store.records.push(record.clone());
        self.write_unlocked(&store)?;
        Ok(record)
    }

    pub fn update(
        &self,
        id: &str,
        expected_version: &str,
        values: Value,
    ) -> Result<SettingsRecord, ProviderFailure> {
        let _lock = self.acquire_lock()?;
        let mut store = self.read_unlocked()?;
        let Some(record) = find_record_mut(&mut store.records, id) else {
            return Err(not_found(id));
        };
        ensure_version(expected_version, &record.version)?;
        apply_values_update(record, values);
        let updated = record.clone();
        self.write_unlocked(&store)?;
        Ok(updated)
    }

    pub fn delete(&self, id: &str, expected_version: &str) -> Result<bool, ProviderFailure> {
        let _lock = self.acquire_lock()?;
        let mut store = self.read_unlocked()?;
        let Some(index) = record_index(&store.records, id) else {
            return Err(not_found(id));
        };
        ensure_version(expected_version, &store.records[index].version)?;
        store.records.remove(index);
        self.write_unlocked(&store)?;
        Ok(true)
    }

    fn acquire_lock(&self) -> Result<crate::settings::lock::SettingsLock, ProviderFailure> {
        crate::settings::lock::SettingsLock::acquire(self.lock_path.clone(), Duration::from_secs(3))
            .map_err(|error| {
                let category = if error.kind() == io::ErrorKind::TimedOut {
                    ErrorCategory::Timeout
                } else {
                    ErrorCategory::Unavailable
                };
                ProviderFailure::new(
                    category,
                    "settings_lock_failed",
                    format!("failed to acquire settings lock: {error}"),
                    true,
                )
            })
    }

    fn read_unlocked(&self) -> Result<StoreFile, ProviderFailure> {
        match self.read_store_bytes()? {
            Some(bytes) => parse_store_file(&bytes),
            None => Ok(StoreFile::default()),
        }
    }

    fn read_store_bytes(&self) -> Result<Option<Vec<u8>>, ProviderFailure> {
        match fs::read(&self.path) {
            Ok(bytes) => Ok(Some(bytes)),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(ProviderFailure::new(
                ErrorCategory::Unavailable,
                "settings_store_read_failed",
                format!("failed to read settings store: {error}"),
                true,
            )),
        }
    }

    fn write_unlocked(&self, store: &StoreFile) -> Result<(), ProviderFailure> {
        crate::fs::atomic::atomic_write_json(&self.path, store).map_err(|error| {
            ProviderFailure::new(
                ErrorCategory::Unavailable,
                "settings_store_write_failed",
                format!("failed to write settings store: {error}"),
                true,
            )
        })
    }
}

fn find_record<'a>(records: &'a [SettingsRecord], id: &str) -> Option<&'a SettingsRecord> {
    records.iter().find(|record| record.id == id)
}

fn find_record_mut<'a>(
    records: &'a mut [SettingsRecord],
    id: &str,
) -> Option<&'a mut SettingsRecord> {
    records.iter_mut().find(|record| record.id == id)
}

fn record_index(records: &[SettingsRecord], id: &str) -> Option<usize> {
    records.iter().position(|record| record.id == id)
}

fn parse_store_file(bytes: &[u8]) -> Result<StoreFile, ProviderFailure> {
    serde_json::from_slice(bytes).map_err(|error| {
        ProviderFailure::new(
            ErrorCategory::Failed,
            "settings_store_invalid_json",
            format!("settings store contains invalid JSON: {error}"),
            false,
        )
    })
}

fn ensure_version(expected: &str, actual: &str) -> Result<(), ProviderFailure> {
    if crate::settings::version::check_version(expected, actual) {
        return Ok(());
    }
    Err(ProviderFailure::new(
        ErrorCategory::Conflict,
        "stale_settings_version",
        "settings version is stale",
        false,
    ))
}

fn new_record(display_name: Option<&str>, values: Value) -> SettingsRecord {
    SettingsRecord {
        id: unique_id(display_name, &values),
        display_name: public_display_name(display_name, &values),
        version: crate::settings::version::new_version(),
        values,
    }
}

fn apply_values_update(record: &mut SettingsRecord, values: Value) {
    record.values = merge_values(&record.values, &values);
    if let Some(name) = updated_display_name(&record.values) {
        record.display_name = name;
    }
    record.version = crate::settings::version::new_version();
}

fn merge_values(existing: &Value, patch: &Value) -> Value {
    match (existing, patch) {
        (Value::Object(existing), Value::Object(patch)) => {
            let mut merged = existing.clone();
            for (key, value) in patch {
                merged.insert(key.clone(), value.clone());
            }
            Value::Object(merged)
        }
        _ => patch.clone(),
    }
}

fn public_display_name(display_name: Option<&str>, values: &Value) -> String {
    display_name
        .filter(|value| !value.trim().is_empty())
        .or_else(|| values.get("display_name").and_then(Value::as_str))
        .filter(|value| !value.trim().is_empty())
        .unwrap_or("Claude")
        .to_string()
}

fn updated_display_name(values: &Value) -> Option<String> {
    values
        .get("display_name")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .map(str::to_string)
}

fn setup_brain_model(record: &SettingsRecord) -> Option<String> {
    record
        .values
        .get("setup_brain_model")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn unique_id(display_name: Option<&str>, values: &Value) -> String {
    let stem = display_name
        .or_else(|| values.get("display_name").and_then(Value::as_str))
        .unwrap_or("claude")
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() {
                ch.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect::<String>()
        .split('-')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("-");
    let version = crate::settings::version::new_version();
    format!(
        "{}-{}",
        if stem.is_empty() { "claude" } else { &stem },
        &version[..12]
    )
}

fn not_found(id: &str) -> ProviderFailure {
    ProviderFailure::new(
        ErrorCategory::InvalidRequest,
        "settings_record_not_found",
        format!("settings record not found: {id}"),
        false,
    )
}
