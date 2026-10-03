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

/// Every exported log record carries an event time. A backend that dates
/// records by their event time treats a record without one as very old.
#[test]
fn every_log_record_has_an_event_time() {
    let run = child::run("default", &[]);
    let records = run.logs();
    assert!(!records.is_empty(), "no log records were exported");
    for record in records {
        assert_ne!(
            record.time_unix_nano, 0,
            "log record {:?} was exported with an event time of zero",
            record.body
        );
        assert_eq!(
            record.time_unix_nano, record.observed_time_unix_nano,
            "log record {:?} has an event time that is not its observed time",
            record.body
        );
    }
}
