//! Export settings read from the standard OpenTelemetry environment variables.
//!
//! Every setting is optional, and with none of them present the export is
//! what it was before they existed. A bad value never stops the server: it
//! becomes a warning that is reported once at startup, and the default
//! applies.

use std::collections::BTreeMap;

use opentelemetry::KeyValue;
use opentelemetry_sdk::Resource;

use super::config::read_non_empty_env;

const SERVICE_NAME_ENV: &str = "OTEL_SERVICE_NAME";
const RESOURCE_ATTRIBUTES_ENV: &str = "OTEL_RESOURCE_ATTRIBUTES";
const TRACES_EXPORTER_ENV: &str = "OTEL_TRACES_EXPORTER";
const METRICS_EXPORTER_ENV: &str = "OTEL_METRICS_EXPORTER";
const LOGS_EXPORTER_ENV: &str = "OTEL_LOGS_EXPORTER";

/// Resource attributes the server computes itself. They win over
/// `OTEL_RESOURCE_ATTRIBUTES`.
#[derive(Clone, Debug)]
pub(super) struct ComputedAttributes {
    /// `deployment.environment.name`, when the environment is known.
    pub(super) environment: Option<String>,
    /// `service.version`, when the version has a variable of its own.
    pub(super) version: Option<String>,
    /// `runtime-id`, generated once per process.
    pub(super) runtime_id: String,
}

/// Which signals are exported. All of them unless the operator switches one
/// off.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Signals {
    pub(super) traces: bool,
    pub(super) metrics: bool,
    pub(super) logs: bool,
}

impl Default for Signals {
    fn default() -> Self {
        Self {
            traces: true,
            metrics: true,
            logs: true,
        }
    }
}

impl Signals {
    /// The exported signals for the startup log line, for example
    /// `traces + metrics + logs`.
    pub(super) fn label(self) -> String {
        let exported: Vec<&str> = [
            ("traces", self.traces),
            ("metrics", self.metrics),
            ("logs", self.logs),
        ]
        .into_iter()
        .filter_map(|(signal, on)| on.then_some(signal))
        .collect();
        if exported.is_empty() {
            "no signals".to_string()
        } else {
            exported.join(" + ")
        }
    }
}

/// What the operator asked for through the standard variables.
#[derive(Clone, Debug, Default)]
pub(super) struct ExportSettings {
    service_name: Option<String>,
    resource_attributes: BTreeMap<String, String>,
    signals: Signals,
    warnings: Vec<String>,
}

impl ExportSettings {
    /// Read the settings from the process environment. Called once at
    /// startup.
    pub(super) fn from_env() -> Self {
        Self::from_lookup(read_non_empty_env)
    }

    /// Read the settings through `lookup`, which returns a variable's
    /// trimmed value, or `None` when it is unset or empty.
    pub(super) fn from_lookup(lookup: impl Fn(&str) -> Option<String>) -> Self {
        let mut settings = Self {
            service_name: lookup(SERVICE_NAME_ENV),
            ..Self::default()
        };
        if let Some(raw) = lookup(RESOURCE_ATTRIBUTES_ENV) {
            settings.read_resource_attributes(&raw);
        }
        settings.signals = Signals {
            traces: settings.read_exporter(TRACES_EXPORTER_ENV, &lookup),
            metrics: settings.read_exporter(METRICS_EXPORTER_ENV, &lookup),
            logs: settings.read_exporter(LOGS_EXPORTER_ENV, &lookup),
        };
        settings
    }

    /// Whether the signal behind an `OTEL_*_EXPORTER` variable is exported.
    /// `none` switches it off; unset or `otlp` exports it. OTLP is the only
    /// exporter there is, so any other value is reported and exported as
    /// `otlp`.
    fn read_exporter(&mut self, variable: &str, lookup: &impl Fn(&str) -> Option<String>) -> bool {
        let Some(value) = lookup(variable) else {
            return true;
        };
        if value.eq_ignore_ascii_case("none") {
            return false;
        }
        if !value.eq_ignore_ascii_case("otlp") {
            self.warnings.push(format!(
                "{variable}={value} is not supported (expected otlp or none); exporting as otlp"
            ));
        }
        true
    }

    /// `OTEL_RESOURCE_ATTRIBUTES` is a comma-separated list of `key=value`
    /// pairs. Keys and values are trimmed and otherwise taken as written; a
    /// key given twice keeps its last value.
    fn read_resource_attributes(&mut self, raw: &str) {
        let mut ignored = 0usize;
        for entry in raw.split(',').filter(|entry| !entry.trim().is_empty()) {
            match entry.split_once('=') {
                Some((key, value)) if !key.trim().is_empty() => {
                    self.resource_attributes
                        .insert(key.trim().to_string(), value.trim().to_string());
                }
                _ => ignored += 1,
            }
        }
        // The entries themselves are not printed: a mistyped variable can
        // hold anything.
        match ignored {
            0 => {}
            1 => self.warnings.push(format!(
                "{RESOURCE_ATTRIBUTES_ENV} has 1 entry that is not key=value; it is ignored"
            )),
            _ => self.warnings.push(format!(
                "{RESOURCE_ATTRIBUTES_ENV} has {ignored} entries that are not key=value; \
                 they are ignored"
            )),
        }
    }

    /// The service name to export under: `OTEL_SERVICE_NAME` when it is set,
    /// otherwise the name built into the binary.
    pub(super) fn service_name<'a>(&'a self, built_in: &'a str) -> &'a str {
        self.service_name.as_deref().unwrap_or(built_in)
    }

    /// The resource shared by traces, metrics and logs.
    ///
    /// `OTEL_RESOURCE_ATTRIBUTES` goes in first and what the server computes
    /// itself goes in after it, so a deployment that sets none of the new
    /// variables keeps the attributes it has today. A `service.name` in
    /// `OTEL_RESOURCE_ATTRIBUTES` does not replace the built-in name; only
    /// `OTEL_SERVICE_NAME` does.
    pub(super) fn resource(
        &self,
        built_in_service_name: &str,
        computed: ComputedAttributes,
    ) -> Resource {
        let mut attributes = self.resource_attributes.clone();
        attributes.insert(
            "service.name".to_string(),
            self.service_name(built_in_service_name).to_string(),
        );
        if let Some(environment) = computed.environment {
            attributes.insert("deployment.environment.name".to_string(), environment);
        }
        if let Some(version) = computed.version {
            attributes.insert("service.version".to_string(), version);
        }
        attributes.insert("runtime-id".to_string(), computed.runtime_id);

        Resource::builder_empty()
            .with_attributes(
                attributes
                    .into_iter()
                    .map(|(key, value)| KeyValue::new(key, value)),
            )
            .build()
    }

    /// Which signals to build an exporter for.
    pub(super) fn signals(&self) -> Signals {
        self.signals
    }

    /// Problems found in the settings, to report once at startup.
    pub(super) fn warnings(&self) -> &[String] {
        &self.warnings
    }
}

#[cfg(test)]
#[path = "settings_test.rs"]
mod tests;
