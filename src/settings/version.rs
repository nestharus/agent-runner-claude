// declared_role: accessor, mapper, predicate

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

static NEXT_VERSION: AtomicU64 = AtomicU64::new(1);

pub fn new_version() -> String {
    let seq = NEXT_VERSION.fetch_add(1, Ordering::Relaxed);
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    crate::encoding::sha256_hex(format!("{}:{now}:{seq}", std::process::id()).as_bytes())
}

pub fn check_version(expected: &str, actual: &str) -> bool {
    expected == actual
}
