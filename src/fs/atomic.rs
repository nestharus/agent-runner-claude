// declared_role: accessor, formatter, mapper, orchestration, predicate, validator
// intrinsic_surface_declarations:
//   - component: src/fs/atomic.rs
//     role: intrinsic-surface
//     Domain: atomic_file_write
//     Owns:
//       - "temp+fsync+rename complete-file publication"
//       - "atomic_write_bytes"
//       - "atomic_write_json"
//       - "src/encoding.rs now_unix_ms temp-name disambiguation seam"
//       - "src/encoding.rs sha256_hex verification seam in atomic-write tests"

use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use serde::Serialize;

pub fn atomic_write_bytes(path: &Path, bytes: &[u8]) -> io::Result<()> {
    atomic_write_bytes_with_rename(path, bytes, |temp_path, target_path| {
        fs::rename(temp_path, target_path)
    })
}

fn atomic_write_bytes_with_rename(
    path: &Path,
    bytes: &[u8],
    rename_temp: impl FnOnce(&Path, &Path) -> io::Result<()>,
) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let temp_path = temp_path_for(path);
    {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp_path)?;
        file.write_all(bytes)?;
        file.sync_all()?;
    }
    rename_temp(&temp_path, path).inspect_err(|_| {
        let _ = fs::remove_file(&temp_path);
    })?;
    sync_parent(path)?;
    Ok(())
}

pub fn atomic_write_json<T: Serialize>(path: &Path, value: &T) -> io::Result<()> {
    let bytes = serialize_json(value)?;
    atomic_write_bytes(path, &bytes)
}

fn serialize_json<T: Serialize>(value: &T) -> io::Result<Vec<u8>> {
    serde_json::to_vec(value).map_err(io::Error::other)
}

fn temp_path_for(path: &Path) -> PathBuf {
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("atomic");
    path.with_file_name(format!(
        ".{name}.tmp-{}-{}",
        std::process::id(),
        crate::encoding::now_unix_ms()
    ))
}

fn sync_parent(path: &Path) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        let directory = File::open(parent)?;
        directory.sync_all()?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::ser::Serializer;
    use std::fs;

    struct FailingSerialize;

    impl Serialize for FailingSerialize {
        fn serialize<S>(&self, _serializer: S) -> Result<S::Ok, S::Error>
        where
            S: Serializer,
        {
            Err(serde::ser::Error::custom("serialization fixture failure"))
        }
    }

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "agent-runner-claude-{name}-{}-{}",
            std::process::id(),
            crate::encoding::now_unix_ms()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("temp test directory should be created");
        dir
    }

    #[test]
    fn atomic_write_bytes_publishes_complete_file() {
        let dir = temp_dir("atomic-bytes");
        let path = dir.join("nested").join("record.json");

        atomic_write_bytes(&path, br#"{"complete":true}"#).expect("atomic write should succeed");

        assert_eq!(
            fs::read(&path).expect("published file should be readable"),
            br#"{"complete":true}"#
        );
        assert!(
            fs::read_dir(path.parent().expect("path should have parent"))
                .expect("parent dir should be readable")
                .all(|entry| !entry
                    .expect("dir entry should be readable")
                    .file_name()
                    .to_string_lossy()
                    .contains("tmp"))
        );
    }

    #[test]
    fn atomic_write_json_serialization_error_preserves_existing_file() {
        let dir = temp_dir("atomic-json");
        let path = dir.join("record.json");
        fs::write(&path, b"old complete file").expect("preimage should be writable");

        let error = atomic_write_json(&path, &FailingSerialize).expect_err("fixture should fail");

        assert_eq!(error.kind(), io::ErrorKind::Other);
        assert_eq!(
            fs::read(&path).expect("preimage should remain readable"),
            b"old complete file"
        );
    }

    #[test]
    fn atomic_write_bytes_failed_rename_never_publishes_partial_file_and_cleans_temp() {
        let dir = temp_dir("atomic-rename-failure");
        let path = dir.join("record.jsonl");
        let preimage = b"original complete transcript\n";
        let postimage = b"replacement complete transcript\n";
        fs::write(&path, preimage).expect("preimage should be writable");

        let success_hash = crate::encoding::sha256_hex(postimage);
        atomic_write_bytes(&path, postimage).expect("successful write should publish postimage");
        assert_eq!(
            crate::encoding::sha256_hex(&fs::read(&path).unwrap()),
            success_hash
        );

        fs::write(&path, preimage).expect("preimage should be restored");
        let error = atomic_write_bytes_with_rename(&path, postimage, |temp_path, _target| {
            assert_eq!(
                fs::read(temp_path).expect("temp postimage should be complete before rename"),
                postimage
            );
            Err(io::Error::other("forced rename failure"))
        })
        .expect_err("forced rename failure should be returned");

        assert_eq!(error.kind(), io::ErrorKind::Other);
        let actual = fs::read(&path).expect("target should remain readable");
        assert!(
            actual == preimage || actual == postimage,
            "failed atomic write exposed partial target: {}",
            String::from_utf8_lossy(&actual)
        );
        assert_eq!(actual, preimage);
        assert!(
            fs::read_dir(path.parent().expect("path should have parent"))
                .expect("parent dir should be readable")
                .all(|entry| !entry
                    .expect("dir entry should be readable")
                    .file_name()
                    .to_string_lossy()
                    .contains("tmp"))
        );
    }
}
