// declared_role: orchestration, formatter

use std::fs;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

pub fn write_executable(path: &Path, body: &str) {
    fs::write(path, body).expect("write executable fixture");
    #[cfg(unix)]
    {
        let mut permissions = fs::metadata(path).expect("script metadata").permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(path, permissions).expect("chmod executable fixture");
    }
}

pub fn install_probe_script(path: &Path, stdout_json: &str, stderr_text: &str) {
    install_probe_script_with_marker_line(path, stdout_json, stderr_text, "");
}

pub fn install_probe_script_with_marker(
    path: &Path,
    stdout_json: &str,
    stderr_text: &str,
    marker: &Path,
) {
    let marker_line = marker_append_line(marker);
    install_probe_script_with_marker_line(path, stdout_json, stderr_text, &marker_line);
}

fn install_probe_script_with_marker_line(
    path: &Path,
    stdout_json: &str,
    stderr_text: &str,
    marker_line: &str,
) {
    let body = probe_script_body(stdout_json, stderr_text, marker_line);
    write_executable(path, &body);
}

fn probe_script_body(stdout_json: &str, stderr_text: &str, marker_line: &str) -> String {
    format!(
        "#!/bin/sh\n{marker_line}printf '%s' '{}' >&2\ncat <<'JSON'\n{}\nJSON\n",
        shell_single_quote_contents(stderr_text),
        stdout_json
    )
}

fn marker_append_line(path: &Path) -> String {
    format!("printf 'run\\n' >> '{}'\n", path.display())
}

fn shell_single_quote_contents(text: &str) -> String {
    text.replace('\'', "'\\''")
}

pub fn install_refresh_script(path: &Path, marker: &Path) {
    let body = refresh_script_body(marker);
    write_executable(path, &body);
}

fn refresh_script_body(marker: &Path) -> String {
    format!(
        "#!/bin/sh\nprintf 'run\\n' >> '{}'\nsleep 1\ncat <<'JSON'\n{{\"ok\":true}}\nJSON\n",
        marker.display()
    )
}

pub fn install_fake_claude_stdout_stderr(
    bin_dir: &Path,
    argv_path: &Path,
    stdout_json: &str,
    stderr_text: &str,
) {
    fs::create_dir_all(bin_dir).expect("create bin dir");
    let body = fake_claude_stdout_stderr_body(argv_path, stdout_json, stderr_text);
    write_executable(&bin_dir.join("claude"), &body);
}

fn fake_claude_stdout_stderr_body(
    argv_path: &Path,
    stdout_json: &str,
    stderr_text: &str,
) -> String {
    format!(
        "#!/bin/sh\nprintf '%s\\n' \"$@\" > '{}'\nprintf '%s\\n' '{}'\nprintf '%s\\n' '{}' >&2\n",
        argv_path.display(),
        stdout_json,
        stderr_text
    )
}
