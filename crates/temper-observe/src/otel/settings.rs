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

/// What the operator asked for through the standard variables.
#[derive(Clone, Debug, Default)]
pub(super) struct ExportSettings {
    service_name: Option<String>,
    resource_attributes: BTreeMap<String, String>,
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
        settings
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

    /// Problems found in the settings, to report once at startup.
    pub(super) fn warnings(&self) -> &[String] {
        &self.warnings
    }
}

#[cfg(test)]
mod tests {
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
}
