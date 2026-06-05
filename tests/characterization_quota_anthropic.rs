mod support;

use serde_json::{json, Value};
use std::fs;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use support::fixtures::{envelope, host_context, path_string, temp_roots, TempRoots, CONTRACT};
use support::invoke::{invoke, parse_one_stdout_json};
use support::schema::assert_valid;

const WEEKLY_RESET_MS: u64 = 1780619696000;
const BURST_RESET_MS: u64 = 1780552800000;

fn write_script(path: &Path, stdout_json: &str, stderr_text: &str) {
    fs::write(
        path,
        format!(
            "#!/bin/sh\nprintf '%s' '{}' >&2\ncat <<'JSON'\n{}\nJSON\n",
            stderr_text.replace('\'', "'\\''"),
            stdout_json
        ),
    )
    .expect("write quota fixture script");
    #[cfg(unix)]
    {
        let mut permissions = fs::metadata(path).expect("script metadata").permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(path, permissions).expect("chmod quota fixture script");
    }
}

fn quota_probe(roots: &TempRoots, context: Value) -> support::invoke::Invocation {
    invoke(
        "quota.probe",
        &envelope(
            CONTRACT,
            host_context(roots),
            json!({
                "settings_id": "claude-primary",
                "model_name": "claude-sonnet",
                "context": context
            }),
        ),
    )
}

fn success_probe(roots: &TempRoots, script: &Path) -> Value {
    let output = quota_probe(roots, json!({ "quota_script": path_string(script) }));
    assert_eq!(output.code, Some(0));
    assert!(output.stderr.is_empty());
    let response = parse_one_stdout_json(&output);
    assert_valid("quota.schema.json#/$defs/QuotaProbeResponse", &response);
    response
}

fn assert_window(window: &Value, name: &str, remaining_ratio: f64, reset_ms: u64) {
    assert_eq!(window["name"], name);
    let actual_ratio = window["remaining_ratio"].as_f64().expect("remaining ratio");
    assert!(
        (actual_ratio - remaining_ratio).abs() < 0.000001,
        "remaining ratio {actual_ratio}"
    );
    assert_eq!(window["resets_at_unix_ms"], reset_ms);
}

fn assert_error(output: support::invoke::Invocation) -> Value {
    assert_ne!(output.code, Some(0));
    assert!(output.stderr.is_empty());
    let response = parse_one_stdout_json(&output);
    assert_valid(
        "quota.schema.json#/$defs/QuotaProbeErrorResponse",
        &response,
    );
    response
}

fn captured_anthropic_usage_fixture() -> &'static str {
    r#"{
  "account": "acct-test",
  "windows": [
    {
      "label": "weekly",
      "used_percent": 12.5,
      "reset_timestamp": "2026-06-04T12:34:56Z"
    },
    {
      "label": "5h-burst",
      "used_percent": 99.0,
      "reset_timestamp": 1780552800
    }
  ]
}"#
}

fn legacy_quota_fixture() -> &'static str {
    r#"{
  "window": "weekly",
  "used_percent": 62.5,
  "reset_timestamp": "2026-06-04T12:34:56Z"
}"#
}

#[test]
fn anthropic_usage_fixture_pins_window_labels_reset_conversion_and_remaining_ratios() {
    let roots = temp_roots("anthropic-usage-captured");
    let script = roots.root.join("anthropic-usage.sh");
    write_script(&script, captured_anthropic_usage_fixture(), "");

    let response = success_probe(&roots, &script);
    let windows = response["result"]["windows"].as_array().unwrap();
    assert_eq!(windows.len(), 2);
    assert_window(&windows[0], "weekly", 0.875, WEEKLY_RESET_MS);
    assert_window(&windows[1], "5h-burst", 0.01, BURST_RESET_MS);
}

#[test]
fn legacy_quota_fixture_pins_flat_shape_and_reset_timestamp_conversion() {
    let roots = temp_roots("anthropic-usage-legacy");
    let script = roots.root.join("legacy-quota.sh");
    write_script(&script, legacy_quota_fixture(), "");

    let response = success_probe(&roots, &script);
    let windows = response["result"]["windows"].as_array().unwrap();
    assert_eq!(windows.len(), 1);
    assert_window(&windows[0], "weekly", 0.375, WEEKLY_RESET_MS);
}

#[test]
fn anthropic_usage_rejects_used_percent_outside_zero_to_one_hundred() {
    for (label, used_percent) in [("negative", -0.01), ("over", 100.01)] {
        let roots = temp_roots(label);
        let script = roots.root.join("anthropic-usage.sh");
        write_script(
            &script,
            &format!(
                r#"{{ "windows": [{{ "label": "weekly", "used_percent": {used_percent}, "reset_timestamp": "2026-06-04T12:34:56Z" }}] }}"#
            ),
            "",
        );
        let response = assert_error(quota_probe(
            &roots,
            json!({ "quota_script": path_string(&script) }),
        ));
        assert_eq!(response["error"]["code"], "quota_probe_parse_failed");
    }
}

#[test]
fn anthropic_usage_stderr_is_diagnostic_only_and_does_not_block_stdout_parse() {
    let roots = temp_roots("anthropic-usage-stderr");
    let script = roots.root.join("anthropic-usage.sh");
    write_script(
        &script,
        captured_anthropic_usage_fixture(),
        "warning: quota data may be approximate",
    );

    let response = success_probe(&roots, &script);
    assert!(response["result"]["available"].as_bool().unwrap());
    assert_eq!(response["result"]["windows"].as_array().unwrap().len(), 2);
}

#[test]
fn anthropic_usage_empty_after_prior_data_is_refresh_recommended() {
    let roots = temp_roots("anthropic-usage-empty-after-prior");
    let script = roots.root.join("anthropic-usage.sh");
    write_script(&script, r#"{ "windows": [] }"#, "");

    let output = quota_probe(
        &roots,
        json!({
            "quota_script": path_string(&script),
            "prior_windows": [{ "name": "weekly", "remaining_ratio": 0.5, "resets_at_unix_ms": WEEKLY_RESET_MS }]
        }),
    );
    let response = assert_error(output);
    assert_eq!(
        response["error"]["code"],
        "quota_probe_empty_after_prior_data"
    );
    assert!(response["error"]["retryable"].as_bool().unwrap());
}
