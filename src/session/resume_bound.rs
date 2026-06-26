// declared_role: orchestration, accessor, formatter, mapper, parser, predicate
// intrinsic_surface_declarations:
//   - component: src/session/resume_bound.rs
//     role: intrinsic-surface
//     Domain: claude_code_resume_transcript_bounding
//     Owns:
//       - "pre-launch native resume transcript compaction-bound truncation"
//       - "durable adjacent full-transcript backup before bounding"
//       - "latest isCompactSummary byte-offset detection without re-rendering transcript JSONL"

use std::fs;
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::envelope::decode::RequestEnvelope;
use crate::envelope::error::ProviderFailure;

use super::storage::{locate_by_session_id, scan_bounds, LocateOutcome};
use super::types::{conflict, failed, host_home};

pub fn bound_latest_compaction_for_resume(
    request: &RequestEnvelope,
    params: &Value,
    session_id: &str,
) -> Result<(), ProviderFailure> {
    let home = host_home(request)?;
    match locate_by_session_id(&home, session_id, scan_bounds(params))
        .map_err(resume_locate_failed)?
    {
        LocateOutcome::Missing => Ok(()),
        LocateOutcome::Found(path) => bound_transcript(&path, session_id),
        LocateOutcome::Ambiguous(paths) => Err(ambiguous_session_transcript(&paths)),
    }
}

fn bound_transcript(path: &Path, session_id: &str) -> Result<(), ProviderFailure> {
    let original = read_transcript_bytes(path)?;
    let Some(offset) = latest_compaction_summary_offset(&original, session_id) else {
        return Ok(());
    };
    if already_bounded(offset) {
        return Ok(());
    }

    write_full_backup(path, &original)?;
    write_bounded_transcript(path, &original[offset..])
}

fn read_transcript_bytes(path: &Path) -> Result<Vec<u8>, ProviderFailure> {
    fs::read(path).map_err(resume_read_failed)
}

fn latest_compaction_summary_offset(bytes: &[u8], session_id: &str) -> Option<usize> {
    let mut latest = None;
    let mut offset = 0usize;
    for line in bytes.split_inclusive(|byte| *byte == b'\n') {
        if is_session_compaction_summary(line, session_id) {
            latest = Some(offset);
        }
        offset += line.len();
    }
    latest
}

fn is_session_compaction_summary(line: &[u8], session_id: &str) -> bool {
    if line_is_blank(line) {
        return false;
    }
    let Ok(value) = serde_json::from_slice::<Value>(line) else {
        return false;
    };
    value.get("isCompactSummary").and_then(Value::as_bool) == Some(true)
        && record_session_matches_or_unspecified(&value, session_id)
}

fn line_is_blank(line: &[u8]) -> bool {
    line.iter().all(|byte| byte.is_ascii_whitespace())
}

fn record_session_matches_or_unspecified(value: &Value, session_id: &str) -> bool {
    match record_session_id(value) {
        Some(record_session_id) => record_session_id == session_id,
        None => true,
    }
}

fn record_session_id(value: &Value) -> Option<&str> {
    value
        .get("sessionId")
        .and_then(Value::as_str)
        .or_else(|| value.get("session_id").and_then(Value::as_str))
}

fn already_bounded(offset: usize) -> bool {
    offset == 0
}

fn write_full_backup(path: &Path, bytes: &[u8]) -> Result<(), ProviderFailure> {
    let backup_path = full_backup_path(path, bytes);
    super::atomic::write_transcript_atomic(&backup_path, bytes).map_err(resume_backup_failed)
}

fn full_backup_path(path: &Path, bytes: &[u8]) -> PathBuf {
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy())
        .unwrap_or_else(|| "transcript.jsonl".into());
    let digest = crate::encoding::sha256_hex(bytes);
    let digest_prefix: String = digest.chars().take(16).collect();
    path.with_file_name(format!(
        "{name}.oulipoly-full-backup-{}-{}-{digest_prefix}.bak",
        crate::encoding::now_unix_ms(),
        std::process::id()
    ))
}

fn write_bounded_transcript(path: &Path, bytes: &[u8]) -> Result<(), ProviderFailure> {
    super::atomic::write_transcript_atomic(path, bytes).map_err(resume_write_failed)
}

fn resume_locate_failed(error: std::io::Error) -> ProviderFailure {
    failed(
        "resume_bound_locate_failed",
        format!("failed to locate Claude resume transcript: {error}"),
    )
}

fn resume_read_failed(error: std::io::Error) -> ProviderFailure {
    failed(
        "resume_bound_read_failed",
        format!("failed to read Claude resume transcript: {error}"),
    )
}

fn resume_backup_failed(error: std::io::Error) -> ProviderFailure {
    failed(
        "resume_bound_backup_failed",
        format!("failed to durably back up Claude resume transcript: {error}"),
    )
}

fn resume_write_failed(error: std::io::Error) -> ProviderFailure {
    failed(
        "resume_bound_write_failed",
        format!("failed to atomically bound Claude resume transcript: {error}"),
    )
}

fn ambiguous_session_transcript(paths: &[PathBuf]) -> ProviderFailure {
    conflict(
        "resume_bound_ambiguous_session_transcript",
        format!(
            "session_id matched multiple Claude transcripts before resume bounding: {}",
            paths
                .iter()
                .map(|path| path.display().to_string())
                .collect::<Vec<_>>()
                .join(", ")
        ),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn temp_home(name: &str) -> PathBuf {
        let home = std::env::temp_dir().join(format!(
            "agent-runner-claude-resume-bound-{name}-{}-{}",
            std::process::id(),
            crate::encoding::now_unix_ms()
        ));
        let _ = fs::remove_dir_all(&home);
        fs::create_dir_all(&home).expect("temp home should be created");
        home
    }

    fn request(home: &Path, params: Value) -> RequestEnvelope {
        RequestEnvelope {
            contract: crate::envelope::CONTRACT.to_string(),
            request_id: "req_fixture".to_string(),
            provider_instance_id: None,
            host: json!({ "env": { "HOME": home.display().to_string() } }),
            params,
        }
    }

    fn transcript_path(home: &Path, session_id: &str) -> PathBuf {
        let path = home
            .join(".claude")
            .join("projects")
            .join("-tmp-fixture")
            .join(format!("{session_id}.jsonl"));
        fs::create_dir_all(path.parent().expect("transcript has parent"))
            .expect("transcript parent should be created");
        path
    }

    fn line(value: Value) -> String {
        format!("{value}\n")
    }

    fn backups(path: &Path) -> Vec<PathBuf> {
        let mut paths: Vec<PathBuf> = fs::read_dir(path.parent().expect("transcript has parent"))
            .expect("parent should be readable")
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| {
                path.file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.contains("oulipoly-full-backup"))
            })
            .collect();
        paths.sort();
        paths
    }

    #[test]
    fn bounds_to_latest_compaction_summary_and_writes_non_jsonl_backup() {
        let session_id = "ses_fixture";
        let home = temp_home("latest-summary");
        let path = transcript_path(&home, session_id);
        let old_turn = line(
            json!({ "sessionId": session_id, "type": "user", "message": { "content": "old" } }),
        );
        let older_summary = line(
            json!({ "sessionId": session_id, "type": "system", "isCompactSummary": true, "message": { "content": "older summary" } }),
        );
        let middle_turn = line(
            json!({ "sessionId": session_id, "type": "assistant", "message": { "content": "middle" } }),
        );
        let latest_summary = line(
            json!({ "sessionId": session_id, "type": "system", "isCompactSummary": true, "message": { "content": "latest summary" } }),
        );
        let new_turn = line(
            json!({ "sessionId": session_id, "type": "assistant", "message": { "content": "new" } }),
        );
        let original = format!("{old_turn}{older_summary}{middle_turn}{latest_summary}{new_turn}");
        fs::write(&path, &original).expect("transcript should be written");
        let params = json!({});
        let request = request(&home, params.clone());

        bound_latest_compaction_for_resume(&request, &params, session_id)
            .expect("resume bounding should succeed");

        assert_eq!(
            fs::read_to_string(&path).expect("bounded transcript should be readable"),
            format!("{latest_summary}{new_turn}")
        );
        let backups = backups(&path);
        assert_eq!(backups.len(), 1);
        assert_ne!(
            backups[0].extension().and_then(|ext| ext.to_str()),
            Some("jsonl")
        );
        assert_eq!(
            fs::read_to_string(&backups[0]).expect("backup should be readable"),
            original
        );
    }

    #[test]
    fn leaves_transcript_without_compaction_summary_unchanged() {
        let session_id = "ses_fixture_no_summary";
        let home = temp_home("no-summary");
        let path = transcript_path(&home, session_id);
        let original = line(
            json!({ "sessionId": session_id, "type": "user", "message": { "content": "small" } }),
        );
        fs::write(&path, &original).expect("transcript should be written");
        let params = json!({});
        let request = request(&home, params.clone());

        bound_latest_compaction_for_resume(&request, &params, session_id)
            .expect("resume bounding should succeed");

        assert_eq!(fs::read_to_string(&path).unwrap(), original);
        assert!(backups(&path).is_empty());
    }

    #[test]
    fn already_bounded_transcript_is_not_backed_up_again() {
        let session_id = "ses_fixture_already_bounded";
        let home = temp_home("already-bounded");
        let path = transcript_path(&home, session_id);
        let original = format!(
            "{}{}",
            line(
                json!({ "sessionId": session_id, "type": "system", "isCompactSummary": true, "message": { "content": "summary" } })
            ),
            line(
                json!({ "sessionId": session_id, "type": "assistant", "message": { "content": "new" } })
            )
        );
        fs::write(&path, &original).expect("transcript should be written");
        let params = json!({});
        let request = request(&home, params.clone());

        bound_latest_compaction_for_resume(&request, &params, session_id)
            .expect("resume bounding should succeed");

        assert_eq!(fs::read_to_string(&path).unwrap(), original);
        assert!(backups(&path).is_empty());
    }

    #[test]
    fn ignores_compaction_summary_for_a_different_session() {
        let session_id = "ses_fixture_target";
        let home = temp_home("different-session");
        let path = transcript_path(&home, session_id);
        let original = format!(
            "{}{}",
            line(
                json!({ "sessionId": session_id, "type": "user", "message": { "content": "target" } })
            ),
            line(
                json!({ "sessionId": "other", "type": "system", "isCompactSummary": true, "message": { "content": "other summary" } })
            )
        );
        fs::write(&path, &original).expect("transcript should be written");
        let params = json!({});
        let request = request(&home, params.clone());

        bound_latest_compaction_for_resume(&request, &params, session_id)
            .expect("resume bounding should succeed");

        assert_eq!(fs::read_to_string(&path).unwrap(), original);
        assert!(backups(&path).is_empty());
    }
}
