// declared_role: accessor, orchestration

use std::io;
use std::path::Path;

pub fn write_transcript_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    crate::fs::atomic::atomic_write_bytes(path, bytes)
}
