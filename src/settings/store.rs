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
//       - "src/envelope/error.rs ProviderFailure/ErrorCategory store-error mapping seam"

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
        let updated = update_record(&mut store, id, expected_version, values)?;
        self.write_unlocked(&store)?;
        Ok(updated)
    }

    pub fn delete(&self, id: &str, expected_version: &str) -> Result<bool, ProviderFailure> {
        let _lock = self.acquire_lock()?;
        let mut store = self.read_unlocked()?;
        delete_record(&mut store, id, expected_version)?;
        self.write_unlocked(&store)?;
        Ok(true)
    }

    fn acquire_lock(&self) -> Result<crate::settings::lock::SettingsLock, ProviderFailure> {
        crate::settings::lock::SettingsLock::acquire(self.lock_path.clone(), Duration::from_secs(3))
            .map_err(settings_lock_failed)
    }

    fn read_unlocked(&self) -> Result<StoreFile, ProviderFailure> {
        match self.read_store_bytes()? {
            Some(bytes) => parse_store_file(&bytes),
            None => Ok(StoreFile::default()),
        }
    }

    fn read_store_bytes(&self) -> Result<Option<Vec<u8>>, ProviderFailure> {
        store_bytes_from_read_result(read_store_file(&self.path))
    }

    fn write_unlocked(&self, store: &StoreFile) -> Result<(), ProviderFailure> {
        crate::fs::atomic::atomic_write_json(&self.path, store).map_err(settings_store_write_failed)
    }
}

fn settings_lock_failed(error: io::Error) -> ProviderFailure {
    ProviderFailure::new(
        settings_lock_error_category(&error),
        "settings_lock_failed",
        format!("failed to acquire settings lock: {error}"),
        true,
    )
}

fn settings_lock_error_category(error: &io::Error) -> ErrorCategory {
    if error.kind() == io::ErrorKind::TimedOut {
        ErrorCategory::Timeout
    } else {
        ErrorCategory::Unavailable
    }
}

fn read_store_file(path: &std::path::Path) -> io::Result<Vec<u8>> {
    fs::read(path)
}

fn store_bytes_from_read_result(
    result: io::Result<Vec<u8>>,
) -> Result<Option<Vec<u8>>, ProviderFailure> {
    match result {
        Ok(bytes) => Ok(Some(bytes)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(settings_store_read_failed(error)),
    }
}

fn settings_store_read_failed(error: io::Error) -> ProviderFailure {
    ProviderFailure::new(
        ErrorCategory::Unavailable,
        "settings_store_read_failed",
        format!("failed to read settings store: {error}"),
        true,
    )
}

fn settings_store_write_failed(error: io::Error) -> ProviderFailure {
    ProviderFailure::new(
        ErrorCategory::Unavailable,
        "settings_store_write_failed",
        format!("failed to write settings store: {error}"),
        true,
    )
}

fn update_record(
    store: &mut StoreFile,
    id: &str,
    expected_version: &str,
    values: Value,
) -> Result<SettingsRecord, ProviderFailure> {
    let record = required_record_mut(&mut store.records, id)?;
    ensure_record_version(record, expected_version)?;
    apply_values_update(record, values);
    Ok(record.clone())
}

fn required_record_mut<'a>(
    records: &'a mut [SettingsRecord],
    id: &str,
) -> Result<&'a mut SettingsRecord, ProviderFailure> {
    find_record_mut(records, id).ok_or_else(|| not_found(id))
}

fn ensure_record_version(
    record: &SettingsRecord,
    expected_version: &str,
) -> Result<(), ProviderFailure> {
    ensure_version(expected_version, &record.version)
}

fn delete_record(
    store: &mut StoreFile,
    id: &str,
    expected_version: &str,
) -> Result<(), ProviderFailure> {
    let index = required_record_index(&store.records, id)?;
    ensure_record_index_version(&store.records, index, expected_version)?;
    remove_record_at(&mut store.records, index);
    Ok(())
}

fn required_record_index(records: &[SettingsRecord], id: &str) -> Result<usize, ProviderFailure> {
    record_index(records, id).ok_or_else(|| not_found(id))
}

fn ensure_record_index_version(
    records: &[SettingsRecord],
    index: usize,
    expected_version: &str,
) -> Result<(), ProviderFailure> {
    ensure_version(expected_version, &records[index].version)
}

fn remove_record_at(records: &mut Vec<SettingsRecord>, index: usize) {
    records.remove(index);
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
    serde_json::from_slice(bytes).map_err(settings_store_invalid_json)
}

fn settings_store_invalid_json(error: serde_json::Error) -> ProviderFailure {
    ProviderFailure::new(
        ErrorCategory::Failed,
        "settings_store_invalid_json",
        format!("settings store contains invalid JSON: {error}"),
        false,
    )
}

fn ensure_version(expected: &str, actual: &str) -> Result<(), ProviderFailure> {
    if crate::settings::version::check_version(expected, actual) {
        return Ok(());
    }
    Err(stale_settings_version())
}

fn stale_settings_version() -> ProviderFailure {
    ProviderFailure::new(
        ErrorCategory::Conflict,
        "stale_settings_version",
        "settings version is stale",
        false,
    )
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
    format_public_display_name(public_display_name_candidate(display_name, values))
}

fn public_display_name_candidate<'a>(
    display_name: Option<&'a str>,
    values: &'a Value,
) -> Option<&'a str> {
    explicit_display_name(display_name).or_else(|| values_display_name(values))
}

fn explicit_display_name(display_name: Option<&str>) -> Option<&str> {
    display_name.filter(non_empty_trimmed)
}

fn values_display_name(values: &Value) -> Option<&str> {
    display_name_value(values).filter(non_empty_trimmed)
}

fn format_public_display_name(display_name: Option<&str>) -> String {
    display_name.unwrap_or("Claude").to_string()
}

fn updated_display_name(values: &Value) -> Option<String> {
    display_name_value(values)
        .filter(non_empty_trimmed)
        .map(owned_string)
}

fn setup_brain_model(record: &SettingsRecord) -> Option<String> {
    setup_brain_model_value(record)
        .filter(non_empty_string)
        .map(owned_string)
}

fn display_name_value(values: &Value) -> Option<&str> {
    values.get("display_name").and_then(Value::as_str)
}

fn setup_brain_model_value(record: &SettingsRecord) -> Option<&str> {
    record
        .values
        .get("setup_brain_model")
        .and_then(Value::as_str)
}

fn non_empty_trimmed(value: &&str) -> bool {
    !value.trim().is_empty()
}

fn non_empty_string(value: &&str) -> bool {
    !value.is_empty()
}

fn owned_string(value: &str) -> String {
    value.to_string()
}

fn unique_id(display_name: Option<&str>, values: &Value) -> String {
    let stem = slug_stem(unique_id_name(display_name, values));
    let version = crate::settings::version::new_version();
    format_unique_id(&stem, &version)
}

fn unique_id_name<'a>(display_name: Option<&'a str>, values: &'a Value) -> &'a str {
    display_name
        .or_else(|| display_name_value(values))
        .unwrap_or("claude")
}

fn slug_stem(value: &str) -> String {
    join_slug_parts(slug_parts(&slug_text(value)))
}

fn slug_text(value: &str) -> String {
    value.chars().map(slug_char).collect()
}

fn slug_char(ch: char) -> char {
    if ch.is_ascii_alphanumeric() {
        ch.to_ascii_lowercase()
    } else {
        '-'
    }
}

fn slug_parts(value: &str) -> Vec<&str> {
    value.split('-').filter(non_empty_part).collect()
}

fn non_empty_part(part: &&str) -> bool {
    !part.is_empty()
}

fn join_slug_parts(parts: Vec<&str>) -> String {
    parts.join("-")
}

fn format_unique_id(stem: &str, version: &str) -> String {
    format!("{}-{}", non_empty_stem_or_default(stem), &version[..12])
}

fn non_empty_stem_or_default(stem: &str) -> &str {
    if stem.is_empty() {
        "claude"
    } else {
        stem
    }
}

fn not_found(id: &str) -> ProviderFailure {
    ProviderFailure::new(
        ErrorCategory::InvalidRequest,
        "settings_record_not_found",
        format!("settings record not found: {id}"),
        false,
    )
}
