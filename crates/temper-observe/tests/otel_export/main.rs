//! End-to-end tests of the OTLP export settings.
//!
//! Each test starts the telemetry setup in a child process, pointed at a
//! local OTLP/HTTP listener, and asserts on what the listener received and on
//! what the child printed at startup.

mod child;
mod listener;
mod render;
mod wire;

use child::{BUILT_IN_SERVICE_NAME, Run};
use wire::Attributes;

/// The resource of each of the three signals, which must all have exported.
fn resources_of_all_signals(run: &Run) -> Vec<(&'static str, Attributes)> {
    let resources = run.resources();
    let signals: Vec<&str> = resources.iter().map(|(signal, _)| *signal).collect();
    assert_eq!(signals, ["traces", "metrics", "logs"]);
    resources
}

fn attribute<'a>(resource: &'a Attributes, key: &str) -> Option<&'a str> {
    resource.get(key).map(String::as_str)
}

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

/// `OTEL_SERVICE_NAME` replaces the built-in service name on every signal.
#[test]
fn service_name_follows_otel_service_name() {
    let run = child::run("default", &[("OTEL_SERVICE_NAME", "orders-eu")]);
    for (signal, resource) in resources_of_all_signals(&run) {
        assert_eq!(
            attribute(&resource, "service.name"),
            Some("orders-eu"),
            "service.name on {signal}"
        );
    }
}

/// Without `OTEL_SERVICE_NAME` the service name is the built-in one.
#[test]
fn service_name_is_built_in_when_unset() {
    let run = child::run("default", &[]);
    for (signal, resource) in resources_of_all_signals(&run) {
        assert_eq!(
            attribute(&resource, "service.name"),
            Some(BUILT_IN_SERVICE_NAME),
            "service.name on {signal}"
        );
    }
}

/// `OTEL_RESOURCE_ATTRIBUTES` is merged into the resource of every signal,
/// next to what the server computes itself.
#[test]
fn resource_attributes_are_merged_on_every_signal() {
    let run = child::run("default", &[("OTEL_RESOURCE_ATTRIBUTES", "a=1,b=2")]);
    for (signal, resource) in resources_of_all_signals(&run) {
        assert_eq!(attribute(&resource, "a"), Some("1"), "a on {signal}");
        assert_eq!(attribute(&resource, "b"), Some("2"), "b on {signal}");
        assert_eq!(
            attribute(&resource, "service.name"),
            Some(BUILT_IN_SERVICE_NAME),
            "service.name on {signal}"
        );
        let runtime_id = attribute(&resource, "runtime-id").unwrap_or_default();
        assert!(!runtime_id.is_empty(), "runtime-id missing on {signal}");
    }
}

/// What the server computes itself wins over `OTEL_RESOURCE_ATTRIBUTES`:
/// the runtime id always, the environment and the version when their own
/// variables are set, and the service name unless `OTEL_SERVICE_NAME` is set.
#[test]
fn computed_resource_attributes_keep_precedence() {
    let run = child::run(
        "default",
        &[
            (
                "OTEL_RESOURCE_ATTRIBUTES",
                "runtime-id=from-attributes,deployment.environment.name=from-attributes,\
                 service.version=from-attributes,service.name=from-attributes,team=storage",
            ),
            ("DD_ENV", "from-env-variable"),
            ("DD_VERSION", "from-version-variable"),
        ],
    );
    for (signal, resource) in resources_of_all_signals(&run) {
        let expect = |key: &str, value: &str| {
            assert_eq!(attribute(&resource, key), Some(value), "{key} on {signal}");
        };
        expect("deployment.environment.name", "from-env-variable");
        expect("service.version", "from-version-variable");
        expect("service.name", BUILT_IN_SERVICE_NAME);
        expect("team", "storage");
        assert_ne!(
            attribute(&resource, "runtime-id"),
            Some("from-attributes"),
            "runtime-id on {signal}"
        );
    }
}

/// Without their own variables, the environment and the version come from
/// `OTEL_RESOURCE_ATTRIBUTES`, and `OTEL_SERVICE_NAME` wins over a
/// `service.name` given there.
#[test]
fn resource_attributes_supply_what_has_no_variable_of_its_own() {
    let run = child::run(
        "default",
        &[
            (
                "OTEL_RESOURCE_ATTRIBUTES",
                "deployment.environment.name=staging, service.version = 1.2.3 ,service.name=ignored",
            ),
            ("OTEL_SERVICE_NAME", "orders-eu"),
        ],
    );
    for (signal, resource) in resources_of_all_signals(&run) {
        let expect = |key: &str, value: &str| {
            assert_eq!(attribute(&resource, key), Some(value), "{key} on {signal}");
        };
        expect("deployment.environment.name", "staging");
        expect("service.version", "1.2.3");
        expect("service.name", "orders-eu");
    }
}
