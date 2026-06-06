// declared_role: orchestration

use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::PathBuf;
use std::thread;
use std::time::{Duration, Instant};

#[derive(Debug)]
pub struct SettingsLock {
    pub path: PathBuf,
    _file: File,
}

impl SettingsLock {
    pub fn acquire(path: PathBuf, timeout: Duration) -> io::Result<Self> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }

        let started = Instant::now();
        loop {
            match OpenOptions::new().write(true).create_new(true).open(&path) {
                Ok(mut file) => {
                    let _ = writeln!(file, "{}", std::process::id());
                    return Ok(Self { path, _file: file });
                }
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                    if started.elapsed() >= timeout {
                        return Err(settings_lock_timeout());
                    }
                    thread::sleep(Duration::from_millis(5));
                }
                Err(error) => return Err(error),
            }
        }
    }
}

fn settings_lock_timeout() -> io::Error {
    io::Error::new(
        io::ErrorKind::TimedOut,
        "timed out waiting for settings lock",
    )
}

impl Drop for SettingsLock {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}
