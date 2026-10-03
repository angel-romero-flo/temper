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
    let run = child::run(&[]);
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
    let run = child::run(&[]);
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
    let run = child::run(&[("OTEL_SERVICE_NAME", "orders-eu")]);
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
    let run = child::run(&[]);
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
    let run = child::run(&[("OTEL_RESOURCE_ATTRIBUTES", "a=1,b=2")]);
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
    let run = child::run(&[
        (
            "OTEL_RESOURCE_ATTRIBUTES",
            "runtime-id=from-attributes,deployment.environment.name=from-attributes,\
                 service.version=from-attributes,service.name=from-attributes,team=storage",
        ),
        ("DD_ENV", "from-env-variable"),
        ("DD_VERSION", "from-version-variable"),
    ]);
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
    let run = child::run(&[
        (
            "OTEL_RESOURCE_ATTRIBUTES",
            "deployment.environment.name=staging, service.version = 1.2.3 ,service.name=ignored",
        ),
        ("OTEL_SERVICE_NAME", "orders-eu"),
    ]);
    for (signal, resource) in resources_of_all_signals(&run) {
        let expect = |key: &str, value: &str| {
            assert_eq!(attribute(&resource, key), Some(value), "{key} on {signal}");
        };
        expect("deployment.environment.name", "staging");
        expect("service.version", "1.2.3");
        expect("service.name", "orders-eu");
    }
}

const SIGNAL_PATHS: [&str; 3] = ["/v1/traces", "/v1/metrics", "/v1/logs"];

/// With `variable` set to `none`, nothing reaches `silent_path`, the other
/// two signals are still exported, and the process logs as before.
fn assert_only_one_signal_is_off(variable: &str, silent_path: &str, exported: &str) {
    let run = child::run(&[(variable, "none")]);
    let messages = log_messages(&run);
    for expected in [
        format!("OTEL initialised ({exported})").as_str(),
        "test log inside a span",
        "test log outside a span",
    ] {
        assert!(
            messages.iter().any(|message| message == expected),
            "{variable}=none: no log line {expected:?} in {messages:?}"
        );
    }
    for path in SIGNAL_PATHS {
        let requests = run.requests_to(path);
        if path == silent_path {
            assert_eq!(requests, 0, "{variable}=none still exported to {path}");
        } else {
            assert!(requests > 0, "{variable}=none stopped the export to {path}");
        }
    }
    assert_eq!(warnings(&run), Vec::<String>::new());
}

/// The messages of the warnings the telemetry setup logged.
fn warnings(run: &Run) -> Vec<String> {
    run.log_lines()
        .iter()
        .filter(|line| line["level"] == "WARN" && line["target"] == "temper_observe::otel")
        .map(message)
        .collect()
}

/// The message of every log line the child printed.
fn log_messages(run: &Run) -> Vec<String> {
    run.log_lines().iter().map(message).collect()
}

fn message(line: &serde_json::Map<String, serde_json::Value>) -> String {
    line["fields"]["message"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

#[test]
fn traces_exporter_none_switches_only_traces_off() {
    assert_only_one_signal_is_off("OTEL_TRACES_EXPORTER", "/v1/traces", "metrics + logs");
}

#[test]
fn metrics_exporter_none_switches_only_metrics_off() {
    assert_only_one_signal_is_off("OTEL_METRICS_EXPORTER", "/v1/metrics", "traces + logs");
}

#[test]
fn logs_exporter_none_switches_only_logs_off() {
    assert_only_one_signal_is_off("OTEL_LOGS_EXPORTER", "/v1/logs", "traces + metrics");
}

/// `otlp` is the default spelled out: everything is exported as when the
/// variables are unset, and nothing is reported.
#[test]
fn exporters_set_to_otlp_export_as_by_default() {
    let run = child::run(&[
        ("OTEL_TRACES_EXPORTER", "otlp"),
        ("OTEL_METRICS_EXPORTER", "OTLP"),
        ("OTEL_LOGS_EXPORTER", " otlp "),
    ]);
    let expected = std::fs::read_to_string(BASELINE_PATH).expect("read the baseline");
    assert!(
        render::render(&run) == expected,
        "export differs from the baseline"
    );
}

/// Name and trace ID of every exported span, sorted.
fn exported_spans(run: &Run) -> Vec<(String, String)> {
    let mut spans: Vec<(String, String)> = run
        .spans()
        .into_iter()
        .map(|span| (span.name, span.trace_id))
        .collect();
    spans.sort();
    spans
}

fn span_names(run: &Run) -> Vec<String> {
    exported_spans(run)
        .into_iter()
        .map(|(name, _)| name)
        .collect()
}

/// What the workload exports when every span the name-based filter lets
/// through is sampled, whatever its caller said.
const EVERY_SPAN: [&str; 5] = [
    "test.child",
    "test.request",
    "test.sampled_caller",
    "test.unsampled_caller",
    "wasm:workspace_fs.read",
];
/// What it exports when the caller's decision is followed, as by default.
const SPANS_FOLLOWING_THE_CALLER: [&str; 4] = [
    "test.child",
    "test.request",
    "test.sampled_caller",
    "wasm:workspace_fs.read",
];
/// What it exports when only spans with a sampled caller are kept.
const SPANS_WITH_A_SAMPLED_CALLER: [&str; 2] = ["test.sampled_caller", "wasm:workspace_fs.read"];

/// With `always_on`, a request that arrives marked "not sampled" still
/// produces a span, and the span keeps the caller's trace ID.
#[test]
fn always_on_records_a_request_marked_not_sampled() {
    let run = child::run(&[("OTEL_TRACES_SAMPLER", "always_on")]);
    let spans = exported_spans(&run);
    assert!(
        spans.contains(&(
            "test.unsampled_caller".to_string(),
            child::UNSAMPLED_CALLER_TRACE_ID.to_string()
        )),
        "no span with the caller's trace ID in {spans:?}"
    );
    assert_eq!(span_names(&run), EVERY_SPAN);
}

/// With the sampler unset, a request marked "not sampled" produces no span.
#[test]
fn default_sampler_follows_a_caller_that_did_not_sample() {
    let run = child::run(&[]);
    assert_eq!(span_names(&run), SPANS_FOLLOWING_THE_CALLER);
}

#[test]
fn traceidratio_zero_exports_no_spans() {
    let run = child::run(&[
        ("OTEL_TRACES_SAMPLER", "traceidratio"),
        ("OTEL_TRACES_SAMPLER_ARG", "0"),
    ]);
    assert_eq!(span_names(&run), Vec::<String>::new());
}

#[test]
fn traceidratio_one_exports_every_span() {
    let run = child::run(&[
        ("OTEL_TRACES_SAMPLER", "traceidratio"),
        ("OTEL_TRACES_SAMPLER_ARG", "1"),
    ]);
    assert_eq!(span_names(&run), EVERY_SPAN);
}

/// Every supported sampler exports what it should, and under every one of
/// them the name-based filter drops what it drops by default: the two names
/// it drops outright, and the reduced-rate name on a trace ID outside the
/// rate.
#[test]
fn every_sampler_keeps_the_name_based_filter() {
    let cases: [(&str, &str, &[&str]); 9] = [
        ("always_on", "", &EVERY_SPAN),
        ("always_off", "", &[]),
        ("parentbased_always_on", "", &SPANS_FOLLOWING_THE_CALLER),
        ("parentbased_always_off", "", &SPANS_WITH_A_SAMPLED_CALLER),
        ("traceidratio", "1", &EVERY_SPAN),
        ("traceidratio", "0", &[]),
        ("traceidratio", "", &EVERY_SPAN),
        ("parentbased_traceidratio", "1", &SPANS_FOLLOWING_THE_CALLER),
        (
            "parentbased_traceidratio",
            "0",
            &SPANS_WITH_A_SAMPLED_CALLER,
        ),
    ];
    for (sampler, arg, expected) in cases {
        let run = child::run(&[
            ("OTEL_TRACES_SAMPLER", sampler),
            ("OTEL_TRACES_SAMPLER_ARG", arg),
        ]);
        let case = format!("OTEL_TRACES_SAMPLER={sampler} OTEL_TRACES_SAMPLER_ARG={arg}");
        assert_eq!(span_names(&run), expected, "{case}");
        for (name, trace_id) in exported_spans(&run) {
            assert_ne!(name, "clock_time_get", "{case}");
            assert_ne!(name, "turso.configured_connection", "{case}");
            assert_ne!(trace_id, child::REDUCED_RATE_DROPPED_TRACE_ID, "{case}");
        }
        assert_eq!(warnings(&run), Vec::<String>::new(), "{case}");
    }
}

/// Asking "is logging enabled?" through the `log` bridge, with no log line
/// after it, must not cost the next span or log line on that thread: the
/// export and the log lines are the same as without the question.
#[test]
fn log_enabled_probe_does_not_drop_the_next_span() {
    let run = child::run_after_log_probe(&[]);
    assert_eq!(span_names(&run), SPANS_FOLLOWING_THE_CALLER);
    let expected = std::fs::read_to_string(BASELINE_PATH).expect("read the baseline");
    let actual = render::render(&run);
    assert!(
        actual == expected,
        "export after a log probe differs from the baseline\n\
         --- recorded ({BASELINE_PATH})\n{expected}\n--- actual\n{actual}"
    );
}

/// The child started, reported `expected_warning` exactly once, on its own
/// log output and as an exported log record, and exported all three signals.
fn assert_one_warning(run: &Run, expected_warning: &str) {
    assert_eq!(warnings(run), [expected_warning]);
    let exported: Vec<String> = run
        .logs()
        .into_iter()
        .filter(|record| record.severity_text == "WARN" && record.scope == "temper_observe::otel")
        .map(|record| record.body)
        .collect();
    assert_eq!(exported, [expected_warning]);
    for path in SIGNAL_PATHS {
        assert!(run.requests_to(path) > 0, "no export reached {path}");
    }
}

/// An exporter that is not supported is reported once and the signal is
/// exported as by default.
#[test]
fn unsupported_exporter_is_reported_once_and_exported() {
    for variable in [
        "OTEL_TRACES_EXPORTER",
        "OTEL_METRICS_EXPORTER",
        "OTEL_LOGS_EXPORTER",
    ] {
        let run = child::run(&[(variable, "console")]);
        assert_one_warning(
            &run,
            &format!(
                "OTEL export setting: {variable}=console is not supported \
                 (expected otlp or none); exporting as otlp"
            ),
        );
        assert_eq!(span_names(&run), SPANS_FOLLOWING_THE_CALLER, "{variable}");
    }
}

/// A sampler that is not supported is reported once and the default sampler
/// is used.
#[test]
fn unsupported_sampler_is_reported_once_and_the_default_is_used() {
    let run = child::run(&[("OTEL_TRACES_SAMPLER", "jaeger_remote")]);
    assert_one_warning(
        &run,
        "OTEL export setting: OTEL_TRACES_SAMPLER=jaeger_remote is not supported \
         (expected always_on, always_off, traceidratio, parentbased_always_on, \
         parentbased_always_off or parentbased_traceidratio); using parentbased_always_on",
    );
    assert_eq!(span_names(&run), SPANS_FOLLOWING_THE_CALLER);
}

/// A sampler ratio that is not a number from 0 to 1 is reported once and the
/// ratio's default, 1, is used.
#[test]
fn bad_sampler_ratio_is_reported_once_and_one_is_used() {
    let run = child::run(&[
        ("OTEL_TRACES_SAMPLER", "traceidratio"),
        ("OTEL_TRACES_SAMPLER_ARG", "half"),
    ]);
    assert_one_warning(
        &run,
        "OTEL export setting: OTEL_TRACES_SAMPLER_ARG=half is not a ratio from 0 to 1; using 1",
    );
    assert_eq!(span_names(&run), EVERY_SPAN);
}

/// Resource attribute entries that are not `key=value` are reported once,
/// without being printed, and the resource is the default one.
#[test]
fn malformed_resource_attributes_are_reported_once_and_ignored() {
    let run = child::run(&[("OTEL_RESOURCE_ATTRIBUTES", "no-equals-sign,=no-key")]);
    assert_one_warning(
        &run,
        "OTEL export setting: OTEL_RESOURCE_ATTRIBUTES has 2 entries that are not key=value; \
         they are ignored",
    );
    for (signal, resource) in resources_of_all_signals(&run) {
        let keys: Vec<&str> = resource.keys().map(String::as_str).collect();
        assert_eq!(keys, ["runtime-id", "service.name"], "resource of {signal}");
    }
}

/// An empty value is the same as an unset variable: default behaviour and
/// nothing reported.
#[test]
fn empty_values_are_the_same_as_unset() {
    let run = child::run(&[
        ("OTEL_SERVICE_NAME", ""),
        ("OTEL_RESOURCE_ATTRIBUTES", " "),
        ("OTEL_TRACES_EXPORTER", ""),
        ("OTEL_METRICS_EXPORTER", ""),
        ("OTEL_LOGS_EXPORTER", ""),
        ("OTEL_TRACES_SAMPLER", ""),
        ("OTEL_TRACES_SAMPLER_ARG", ""),
    ]);
    let expected = std::fs::read_to_string(BASELINE_PATH).expect("read the baseline");
    assert!(
        render::render(&run) == expected,
        "export differs from the baseline"
    );
}

/// The credential in the OTLP headers reaches the backend on every signal
/// and is never printed or exported, whatever else is reported at startup.
#[test]
fn credentials_are_sent_and_never_logged() {
    const SECRET: &str = "s3cr3t-credential";
    let run = child::run(&[
        (
            "OTEL_EXPORTER_OTLP_HEADERS",
            "authorization=Bearer s3cr3t-credential",
        ),
        ("OTEL_RESOURCE_ATTRIBUTES", "team=storage,s3cr3t-credential"),
        ("OTEL_SERVICE_NAME", "orders-eu"),
        ("OTEL_TRACES_EXPORTER", "console"),
        ("OTEL_TRACES_SAMPLER", "traceidratio"),
        ("OTEL_TRACES_SAMPLER_ARG", "half"),
    ]);
    for path in SIGNAL_PATHS {
        assert!(run.requests_to(path) > 0, "no export reached {path}");
    }
    for request in &run.received {
        assert_eq!(
            request.header("authorization"),
            "Bearer s3cr3t-credential",
            "authorization header on {}",
            request.path
        );
    }
    assert_eq!(warnings(&run).len(), 3, "{:?}", warnings(&run));
    assert!(!run.stdout.contains(SECRET), "credential on stdout");
    assert!(!run.stderr.contains(SECRET), "credential on stderr");
    assert!(
        !render::render(&run).contains(SECRET),
        "credential in the exported telemetry"
    );
}
