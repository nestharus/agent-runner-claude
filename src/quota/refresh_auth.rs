// declared_role: accessor, formatter, mapper, orchestration, predicate, validator

use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::thread;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::envelope::decode::RequestEnvelope;
use crate::envelope::error::{ErrorCategory, ProviderFailure};

pub fn handle(request: &RequestEnvelope) -> Result<Value, ProviderFailure> {
    let refresh = validate_refresh_auth_request(request)?;
    let lock_path = refresh_lock_path(refresh.data_root, refresh.settings_id);
    let stamp_path = refresh_stamp_path(&lock_path);

    cached_refresh_result(&stamp_path)
        .map(Ok)
        .unwrap_or_else(|| run_refresh_with_lock(refresh.command, &lock_path, &stamp_path))
}

fn cached_refresh_result(stamp_path: &Path) -> Option<Value> {
    fresh_outcome(stamp_path).map(cached_result)
}

fn run_refresh_with_lock(
    command: &str,
    lock_path: &Path,
    stamp_path: &Path,
) -> Result<Value, ProviderFailure> {
    match acquire_refresh_lock(lock_path) {
        Ok(lock) => run_after_acquired_refresh_lock(lock, command, lock_path, stamp_path),
        Err(error) => recover_refresh_lock(error, command, lock_path, stamp_path),
    }
}

fn run_after_acquired_refresh_lock(
    _lock: RefreshLock,
    command: &str,
    lock_path: &Path,
    stamp_path: &Path,
) -> Result<Value, ProviderFailure> {
    run_locked_refresh(command, lock_path, stamp_path)
}

fn recover_refresh_lock(
    error: io::Error,
    command: &str,
    lock_path: &Path,
    stamp_path: &Path,
) -> Result<Value, ProviderFailure> {
    match refresh_lock_recovery(error, lock_path) {
        RefreshLockRecovery::RetryAfterStaleRemoval => {
            retry_refresh_after_stale_lock_removal(command, lock_path, stamp_path)
        }
        RefreshLockRecovery::WaitForOwner => {
            cached_refresh_after_owner_release(lock_path, stamp_path)
        }
        RefreshLockRecovery::Fail(error) => Err(lock_failed(error)),
    }
}

struct RefreshAuthRequest<'a> {
    settings_id: &'a str,
    command: &'a str,
    data_root: &'a str,
}

struct RefreshLock {
    path: PathBuf,
    _file: fs::File,
}

enum RefreshLockRecovery {
    RetryAfterStaleRemoval,
    WaitForOwner,
    Fail(io::Error),
}

impl Drop for RefreshLock {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
struct RefreshOutcome {
    checked_at_unix_ms: u64,
    available: bool,
}

fn validate_refresh_auth_request(
    request: &RequestEnvelope,
) -> Result<RefreshAuthRequest<'_>, ProviderFailure> {
    let params = request.params.as_object().ok_or_else(invalid_params)?;
    let settings_id = params
        .get("settings_id")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(invalid_params)?;
    let command = params
        .get("context")
        .and_then(|value| value.get("auth_refresh_command"))
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(invalid_params)?;
    let data_root = request_data_root(request)?;

    Ok(RefreshAuthRequest {
        settings_id,
        command,
        data_root,
    })
}

fn request_data_root(request: &RequestEnvelope) -> Result<&str, ProviderFailure> {
    request
        .host
        .get("data_root")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            ProviderFailure::invalid_request(
                "missing_host_data_root",
                "host.data_root is required for quota refresh auth path resolution",
            )
        })
}

fn refresh_lock_path(data_root: &str, settings_id: &str) -> PathBuf {
    PathBuf::from(data_root).join(refresh_lock_file_name(settings_id))
}

fn refresh_lock_file_name(settings_id: &str) -> String {
    format!("quota-refresh-{}.lock", sanitize(settings_id))
}

fn refresh_stamp_path(lock_path: &Path) -> PathBuf {
    lock_path.with_extension("stamp")
}

fn acquire_refresh_lock(path: &Path) -> io::Result<RefreshLock> {
    let mut file = open_refresh_lock(path)?;
    write_refresh_lock_stamp(&mut file)?;
    Ok(refresh_lock(path, file))
}

fn open_refresh_lock(path: &Path) -> io::Result<fs::File> {
    OpenOptions::new().write(true).create_new(true).open(path)
}

fn write_refresh_lock_stamp(file: &mut fs::File) -> io::Result<()> {
    file.write_all(refresh_lock_stamp().as_bytes())?;
    file.sync_all()
}

fn current_stamp_unix_ms() -> u64 {
    crate::encoding::now_unix_ms()
}

fn refresh_lock_stamp() -> String {
    current_stamp_unix_ms().to_string()
}

fn refresh_lock(path: &Path, file: fs::File) -> RefreshLock {
    RefreshLock {
        path: path.to_path_buf(),
        _file: file,
    }
}

fn refresh_lock_recovery(error: io::Error, lock_path: &Path) -> RefreshLockRecovery {
    if !refresh_lock_already_exists(&error) {
        return RefreshLockRecovery::Fail(error);
    }
    if remove_stale_lock(lock_path) {
        RefreshLockRecovery::RetryAfterStaleRemoval
    } else {
        RefreshLockRecovery::WaitForOwner
    }
}

fn refresh_lock_already_exists(error: &io::Error) -> bool {
    error.kind() == io::ErrorKind::AlreadyExists
}

fn retry_refresh_after_stale_lock_removal(
    command: &str,
    lock_path: &Path,
    stamp_path: &Path,
) -> Result<Value, ProviderFailure> {
    let lock = acquire_refresh_lock(lock_path).map_err(lock_failed)?;
    run_after_acquired_refresh_lock(lock, command, lock_path, stamp_path)
}

fn cached_refresh_after_owner_release(
    lock_path: &Path,
    stamp_path: &Path,
) -> Result<Value, ProviderFailure> {
    wait_for_lock_release(lock_path);
    cached_refresh_result(stamp_path).ok_or_else(lock_release_timeout)
}

fn run_locked_refresh(
    command: &str,
    _lock_path: &Path,
    stamp_path: &Path,
) -> Result<Value, ProviderFailure> {
    match super::scripts::run_refresh_command(command) {
        Ok(output) => {
            let available = refresh_available(output.status_code, output.timed_out);
            let _ = write_outcome(stamp_path, available);
            Ok(result(true, available))
        }
        Err(error) => {
            let _ = write_outcome(stamp_path, false);
            Err(refresh_command_failed(error))
        }
    }
}

fn refresh_available(status_code: Option<i32>, timed_out: bool) -> bool {
    status_code == Some(0) && !timed_out
}

fn refresh_command_failed(error: io::Error) -> ProviderFailure {
    ProviderFailure::new(
        ErrorCategory::Unavailable,
        "quota_refresh_auth_unavailable",
        format!("failed to run auth refresh command: {error}"),
        true,
    )
}

fn lock_failed(error: io::Error) -> ProviderFailure {
    ProviderFailure::new(
        ErrorCategory::Unavailable,
        "quota_refresh_auth_lock_failed",
        format!("failed to acquire auth refresh lock: {error}"),
        true,
    )
}

fn result(refreshed: bool, available: bool) -> Value {
    json!({
        "refreshed": refreshed,
        "available": available,
        "checked_at_unix_ms": crate::encoding::now_unix_ms(),
    })
}

fn cached_result(outcome: RefreshOutcome) -> Value {
    result(false, outcome.available)
}

fn lock_release_timeout() -> ProviderFailure {
    lock_failed(io::Error::new(
        io::ErrorKind::TimedOut,
        "auth refresh lock did not release with a readable outcome",
    ))
}

fn fresh_outcome(path: &Path) -> Option<RefreshOutcome> {
    let text = stamp_text(path)?;
    let outcome = stamp_outcome(&text)?;
    stamp_checked_at_is_fresh(outcome.checked_at_unix_ms).then_some(outcome)
}

fn stamp_text(path: &Path) -> Option<String> {
    fs::read_to_string(path).ok()
}

fn stamp_outcome(text: &str) -> Option<RefreshOutcome> {
    json_stamp_outcome(text).or_else(|| legacy_stamp_outcome(text))
}

fn json_stamp_outcome(text: &str) -> Option<RefreshOutcome> {
    serde_json::from_str(text).ok()
}

fn legacy_stamp_outcome(text: &str) -> Option<RefreshOutcome> {
    legacy_stamp_checked_at(text).map(available_refresh_outcome)
}

fn legacy_stamp_checked_at(text: &str) -> Option<u64> {
    text.trim().parse::<u64>().ok()
}

fn available_refresh_outcome(checked_at_unix_ms: u64) -> RefreshOutcome {
    RefreshOutcome {
        checked_at_unix_ms,
        available: true,
    }
}

fn stamp_checked_at_is_fresh(checked_at: u64) -> bool {
    crate::encoding::now_unix_ms().saturating_sub(checked_at) < 60_000
}

fn write_outcome(path: &Path, available: bool) -> io::Result<()> {
    let outcome = refresh_outcome(available);
    let bytes = outcome_bytes(&outcome)?;
    crate::fs::atomic::atomic_write_bytes(path, &bytes)
}

fn refresh_outcome(available: bool) -> RefreshOutcome {
    RefreshOutcome {
        checked_at_unix_ms: crate::encoding::now_unix_ms(),
        available,
    }
}

fn outcome_bytes(outcome: &RefreshOutcome) -> io::Result<Vec<u8>> {
    serde_json::to_vec(outcome).map_err(io::Error::other)
}

fn remove_stale_lock(path: &Path) -> bool {
    if !stale_lock_removable(path) {
        return false;
    }
    remove_lock_file(path)
}

fn stale_lock_removable(path: &Path) -> bool {
    let Some(checked_at) = stale_lock_checked_at(path) else {
        return false;
    };
    stamp_checked_at_is_stale(checked_at)
}

fn stale_lock_checked_at(path: &Path) -> Option<u64> {
    stamp_text(path).and_then(|text| legacy_stamp_checked_at(&text))
}

fn stamp_checked_at_is_stale(checked_at: u64) -> bool {
    !stamp_checked_at_is_fresh(checked_at)
}

fn remove_lock_file(path: &Path) -> bool {
    fs::remove_file(path).is_ok()
}

fn wait_for_lock_release(path: &Path) {
    for _ in 0..500 {
        if !path.exists() {
            return;
        }
        thread::sleep(Duration::from_millis(25));
    }
}

fn sanitize(value: &str) -> String {
    value.chars().map(sanitized_filename_char).collect()
}

fn sanitized_filename_char(ch: char) -> char {
    if filename_char_allowed(ch) {
        ch
    } else {
        '-'
    }
}

fn filename_char_allowed(ch: char) -> bool {
    ch.is_ascii_alphanumeric()
}

fn invalid_params() -> ProviderFailure {
    ProviderFailure::invalid_request(
        "invalid_quota_refresh_auth_params",
        "quota.refresh_auth params do not match the quota contract",
    )
}
