//! End-to-end tests of the OTLP export settings.
//!
//! Each test starts the telemetry setup in a child process, pointed at a
//! local OTLP/HTTP listener, and asserts on what the listener received and on
//! what the child printed at startup.

mod child;
mod listener;
mod render;
mod wire;

const BASELINE_PATH: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/otel_export/default_export.baseline.txt"
);

/// With none of the export settings present, the startup output and the
/// exported telemetry match the recorded baseline.
///
/// To record the baseline again after an intended change, run this test with
/// `UPDATE_OTEL_EXPORT_BASELINE=1` and review the diff of the baseline file.
#[test]
fn default_export_matches_recorded_baseline() {
    let run = child::run("default", &[]);
    for path in ["/v1/traces", "/v1/metrics", "/v1/logs"] {
        assert!(run.requests_to(path) > 0, "no export reached {path}");
    }
    let actual = render::render(&run);

    if std::env::var_os("UPDATE_OTEL_EXPORT_BASELINE").is_some() {
        std::fs::write(BASELINE_PATH, &actual).expect("write the baseline");
        return;
    }
    let expected = std::fs::read_to_string(BASELINE_PATH).expect("read the baseline");
    assert!(
        actual == expected,
        "default export differs from the recorded baseline\n\
         --- recorded ({BASELINE_PATH})\n{expected}\n--- actual\n{actual}"
    );
}
