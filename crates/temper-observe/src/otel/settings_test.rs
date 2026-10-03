use opentelemetry::Key;

use super::*;

fn settings(variables: &[(&str, &str)]) -> ExportSettings {
    ExportSettings::from_lookup(|name| {
        variables
            .iter()
            .find(|(variable, _)| *variable == name)
            .map(|(_, value)| value.to_string())
    })
}

fn computed() -> ComputedAttributes {
    ComputedAttributes {
        environment: None,
        version: None,
        runtime_id: "runtime-1".to_string(),
    }
}

fn attribute(resource: &Resource, key: &'static str) -> Option<String> {
    resource
        .get(&Key::from_static_str(key))
        .map(|value| value.to_string())
}

#[test]
fn exporter_variables_switch_signals_off_one_by_one() {
    assert_eq!(settings(&[]).signals(), Signals::default());
    assert_eq!(Signals::default().label(), "traces + metrics + logs");

    let settings = settings(&[
        (TRACES_EXPORTER_ENV, "otlp"),
        (METRICS_EXPORTER_ENV, "None"),
        (LOGS_EXPORTER_ENV, "none"),
    ]);
    let expected = Signals {
        traces: true,
        metrics: false,
        logs: false,
    };
    assert_eq!(settings.signals(), expected);
    assert_eq!(expected.label(), "traces");
    assert!(settings.warnings().is_empty());
}

#[test]
fn unsupported_exporter_is_reported_and_exported_as_otlp() {
    let settings = settings(&[(METRICS_EXPORTER_ENV, "prometheus")]);
    assert_eq!(settings.signals(), Signals::default());
    let [warning] = settings.warnings() else {
        panic!("expected one warning, got {:?}", settings.warnings());
    };
    assert!(warning.contains("OTEL_METRICS_EXPORTER=prometheus is not supported"));
}

#[test]
fn nothing_set_gives_the_built_in_resource_and_no_warnings() {
    let settings = settings(&[]);
    let resource = settings.resource("built-in", computed());
    assert_eq!(resource.len(), 2);
    assert_eq!(
        attribute(&resource, "service.name").as_deref(),
        Some("built-in")
    );
    assert_eq!(
        attribute(&resource, "runtime-id").as_deref(),
        Some("runtime-1")
    );
    assert!(settings.warnings().is_empty());
}

#[test]
fn resource_attributes_are_trimmed_and_the_last_duplicate_wins() {
    let settings = settings(&[(RESOURCE_ATTRIBUTES_ENV, " a = 1 ,b=2,a=3,empty=,")]);
    let resource = settings.resource("built-in", computed());
    assert_eq!(attribute(&resource, "a").as_deref(), Some("3"));
    assert_eq!(attribute(&resource, "b").as_deref(), Some("2"));
    assert_eq!(attribute(&resource, "empty").as_deref(), Some(""));
    assert!(settings.warnings().is_empty());
}

#[test]
fn entries_that_are_not_key_value_are_ignored_with_one_warning() {
    let settings = settings(&[(RESOURCE_ATTRIBUTES_ENV, "a=1,token-without-key,=x,b=2")]);
    let resource = settings.resource("built-in", computed());
    assert_eq!(attribute(&resource, "a").as_deref(), Some("1"));
    assert_eq!(attribute(&resource, "b").as_deref(), Some("2"));
    assert_eq!(resource.len(), 4);
    let [warning] = settings.warnings() else {
        panic!("expected one warning, got {:?}", settings.warnings());
    };
    assert!(warning.contains("OTEL_RESOURCE_ATTRIBUTES has 2 entries"));
    assert!(!warning.contains("token-without-key"));
}

#[test]
fn computed_attributes_win_over_resource_attributes() {
    let settings = settings(&[(
        RESOURCE_ATTRIBUTES_ENV,
        "service.name=x,deployment.environment.name=x,service.version=x,runtime-id=x",
    )]);
    let resource = settings.resource(
        "built-in",
        ComputedAttributes {
            environment: Some("prod".to_string()),
            version: Some("1.0".to_string()),
            runtime_id: "runtime-1".to_string(),
        },
    );
    assert_eq!(
        attribute(&resource, "service.name").as_deref(),
        Some("built-in")
    );
    assert_eq!(
        attribute(&resource, "deployment.environment.name").as_deref(),
        Some("prod")
    );
    assert_eq!(
        attribute(&resource, "service.version").as_deref(),
        Some("1.0")
    );
    assert_eq!(
        attribute(&resource, "runtime-id").as_deref(),
        Some("runtime-1")
    );
}

#[test]
fn service_name_variable_replaces_the_built_in_name() {
    let settings = settings(&[
        (SERVICE_NAME_ENV, "orders-eu"),
        (RESOURCE_ATTRIBUTES_ENV, "service.name=x"),
    ]);
    assert_eq!(settings.service_name("built-in"), "orders-eu");
    let resource = settings.resource("built-in", computed());
    assert_eq!(
        attribute(&resource, "service.name").as_deref(),
        Some("orders-eu")
    );
}
