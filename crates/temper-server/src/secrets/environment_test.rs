use std::ffi::OsString;
use std::io;
use std::sync::{Arc, Mutex};

use temper_runtime::ActorSystem;
use temper_runtime::tenant::TenantId;
use temper_wasm::WasmAuthzContext;

use super::*;
use crate::authz::CedarWasmAuthzGate;
use crate::registry::SpecRegistry;
use crate::state::ServerState;

const TENANT: &str = "tenant-a";
const MODULE: &str = "token-reader";
/// Lets the module read `build_token` and nothing else.
const POLICIES: &str = r#"
permit(principal == Agent::"token-reader", action == Action::"access_secret", resource == Secret::"build_token");
"#;
/// A value no name, reason or message contains, for the checks that nothing
/// reports a value.
const DISTINCT_VALUE: &str = "value-kept-out-of-reports";

fn test_vault() -> SecretsVault {
    SecretsVault::new(&[0x42; 32])
}

fn vars(pairs: &[(&str, &str)]) -> Vec<(OsString, OsString)> {
    pairs
        .iter()
        .map(|(name, value)| (OsString::from(name), OsString::from(value)))
        .collect()
}

/// The lookup a module's `get_secret` call goes through: the tenant's Cedar
/// policies decide, then the vault answers.
fn module_reads(vault: SecretsVault, module: &str, secret: &str) -> Result<String, String> {
    let state =
        ServerState::from_registry(ActorSystem::new("env-secret-tests"), SpecRegistry::new())
            .with_secrets_vault(vault);
    state
        .authz
        .reload_tenant_policies(TENANT, POLICIES)
        .expect("policies load");
    let gate = Arc::new(CedarWasmAuthzGate::new(state.authz.clone()));
    let authz_ctx = WasmAuthzContext {
        tenant: TENANT.to_string(),
        module_name: module.to_string(),
        agent_id: None,
        session_id: None,
        entity_type: "Build".to_string(),
        trigger_action: "Start".to_string(),
    };
    let resolver = state
        .authorized_wasm_secret_resolver(&TenantId::new(TENANT), gate, authz_ctx)
        .expect("resolver exists when a vault is configured");
    resolver(secret)
}

#[derive(Clone, Default)]
struct LogBuffer(Arc<Mutex<Vec<u8>>>);

impl io::Write for LogBuffer {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0
            .lock()
            .expect("log buffer lock")
            .extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// Seed `variables` and return the report with every log line and span the
/// seeding produced, at the most verbose level.
fn seed_with_log(
    vault: &SecretsVault,
    variables: Vec<(OsString, OsString)>,
) -> (EnvironmentSecretsReport, String) {
    let buffer = LogBuffer::default();
    let writer = buffer.clone();
    let subscriber = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::TRACE)
        .with_span_events(tracing_subscriber::fmt::format::FmtSpan::FULL)
        .with_ansi(false)
        .with_writer(move || writer.clone())
        .finish();
    let report = tracing::subscriber::with_default(subscriber, || {
        seed_platform_secrets_from_environment(vault, variables)
    });
    let log =
        String::from_utf8(buffer.0.lock().expect("log buffer lock").clone()).expect("log is UTF-8");
    (report, log)
}

#[test]
fn permitted_module_reads_a_secret_seeded_from_a_prefixed_variable() {
    let vault = test_vault();

    seed_platform_secrets_from_environment(&vault, vars(&[("TEMPER_SECRET_BUILD_TOKEN", "abc")]));

    assert_eq!(
        module_reads(vault, MODULE, "build_token"),
        Ok("abc".to_string())
    );
}

#[test]
fn module_without_permission_is_still_refused_a_seeded_secret() {
    let vault = test_vault();
    seed_platform_secrets_from_environment(&vault, vars(&[("TEMPER_SECRET_BUILD_TOKEN", "abc")]));

    let refused = module_reads(vault, "another-module", "build_token")
        .expect_err("a module the policies do not name must be refused");

    assert!(
        refused.contains("authorization denied for secret 'build_token'"),
        "{refused}"
    );
    assert!(!refused.contains("abc"), "{refused}");
}

#[test]
fn permitted_module_is_refused_a_seeded_secret_its_policy_does_not_name() {
    let vault = test_vault();
    seed_platform_secrets_from_environment(
        &vault,
        vars(&[
            ("TEMPER_SECRET_BUILD_TOKEN", "abc"),
            ("TEMPER_SECRET_DEPLOY_TOKEN", "def"),
        ]),
    );

    let refused =
        module_reads(vault, MODULE, "deploy_token").expect_err("the policy names build_token only");

    assert!(
        refused.contains("authorization denied for secret 'deploy_token'"),
        "{refused}"
    );
}

#[test]
fn seeded_secret_reaches_every_tenant() {
    let vault = test_vault();

    let report = seed_platform_secrets_from_environment(
        &vault,
        vars(&[("TEMPER_SECRET_BUILD_TOKEN", "abc")]),
    );

    assert_eq!(report.seeded, vec!["build_token".to_string()]);
    assert!(report.skipped.is_empty());
    assert_eq!(vault.get_platform_secret("build_token"), Some("abc".into()));
    assert_eq!(
        vault.get_secret("tenant-a", "build_token"),
        Some("abc".into())
    );
    assert_eq!(
        vault.get_secret("tenant-b", "build_token"),
        Some("abc".into())
    );
    assert!(
        vault
            .list_keys("tenant-b")
            .contains(&"build_token".to_string())
    );
}

#[test]
fn several_prefixed_variables_are_all_seeded() {
    let vault = test_vault();

    let report = seed_platform_secrets_from_environment(
        &vault,
        vars(&[
            ("TEMPER_SECRET_REGION", "north"),
            ("TEMPER_SECRET_BUILD_TOKEN", "abc"),
            ("TEMPER_SECRET_KEY_2", "def"),
        ]),
    );

    assert_eq!(
        report.seeded,
        vec![
            "build_token".to_string(),
            "key_2".to_string(),
            "region".to_string()
        ]
    );
    assert_eq!(vault.get_platform_secret("build_token"), Some("abc".into()));
    assert_eq!(vault.get_platform_secret("key_2"), Some("def".into()));
    assert_eq!(vault.get_platform_secret("region"), Some("north".into()));
}

#[test]
fn empty_value_seeds_nothing_and_is_not_reported() {
    let vault = test_vault();

    let (report, log) = seed_with_log(
        &vault,
        vars(&[("TEMPER_SECRET_BUILD_TOKEN", ""), ("TEMPER_SECRET_", "")]),
    );

    assert_eq!(report, EnvironmentSecretsReport::default());
    assert_eq!(vault.get_platform_secret("build_token"), None);
    assert!(vault.get_platform_secrets().is_empty());
    assert_eq!(log, "");
}

#[test]
fn variables_without_the_prefix_are_ignored() {
    let vault = test_vault();

    let (report, log) = seed_with_log(
        &vault,
        vars(&[
            ("BUILD_TOKEN", "abc"),
            ("TEMPER_SECRET", "abc"),
            ("TEMPER_SECRETS_BUILD_TOKEN", "abc"),
            ("temper_secret_BUILD_TOKEN", "abc"),
            ("MY_TEMPER_SECRET_BUILD_TOKEN", "abc"),
        ]),
    );

    assert_eq!(report, EnvironmentSecretsReport::default());
    assert!(vault.get_platform_secrets().is_empty());
    assert_eq!(log, "");
}

#[test]
fn no_prefixed_variable_seeds_nothing_and_logs_nothing() {
    let vault = test_vault();

    let (report, log) = seed_with_log(&vault, vars(&[("HOME", "/home/user"), ("PATH", "/bin")]));

    assert_eq!(report, EnvironmentSecretsReport::default());
    assert!(vault.get_platform_secrets().is_empty());
    assert_eq!(log, "");
}

#[test]
fn badly_named_variables_are_skipped_and_each_reported_once_by_name() {
    let vault = test_vault();
    let bad_names = [
        "TEMPER_SECRET_",
        "TEMPER_SECRET_1TOKEN",
        "TEMPER_SECRET_BUILD-TOKEN",
        "TEMPER_SECRET_BUILD.TOKEN",
        "TEMPER_SECRET_Build_Token",
        "TEMPER_SECRET__TOKEN",
        "TEMPER_SECRET_build_token",
    ];
    let mut variables = vars(&[("TEMPER_SECRET_BUILD_TOKEN", "abc")]);
    variables.extend(
        bad_names
            .iter()
            .flat_map(|name| vars(&[(name, DISTINCT_VALUE)])),
    );

    let (report, log) = seed_with_log(&vault, variables);

    // The well-named variable beside them is still seeded.
    assert_eq!(report.seeded, vec!["build_token".to_string()]);
    assert_eq!(
        vault.get_platform_secrets().keys().collect::<Vec<_>>(),
        vec!["build_token"]
    );
    assert_eq!(
        report.skipped,
        bad_names
            .iter()
            .map(|name| (name.to_string(), EnvironmentSecretSkip::InvalidName))
            .collect::<Vec<_>>()
    );
    for name in bad_names {
        let lines: Vec<&str> = log
            .lines()
            .filter(|line| line.contains(&format!("variable={name} ")))
            .collect();
        assert_eq!(lines.len(), 1, "{name} must be reported once: {log}");
        assert!(lines[0].contains("WARN"), "{log}");
    }
    assert_eq!(log.lines().count(), bad_names.len() + 1, "{log}");
}

#[test]
fn log_gives_the_count_of_seeded_secrets_and_no_names() {
    let vault = test_vault();

    let (_, log) = seed_with_log(
        &vault,
        vars(&[
            ("TEMPER_SECRET_BUILD_TOKEN", "abc"),
            ("TEMPER_SECRET_REGION", "north"),
        ]),
    );

    assert_eq!(log.lines().count(), 1, "{log}");
    assert!(log.contains("INFO"), "{log}");
    assert!(
        log.contains("seeded platform secrets from TEMPER_SECRET_ environment variables"),
        "{log}"
    );
    assert!(log.contains("count=2"), "{log}");
    assert!(!log.to_lowercase().contains("build_token"), "{log}");
    assert!(!log.to_lowercase().contains("region"), "{log}");
}

#[test]
fn no_report_log_line_or_span_contains_a_value() {
    let vault = test_vault();
    // One variable for every outcome: seeded, badly named, already set, and
    // (below) over the budget.
    vault
        .cache_platform_secret("region", "north".to_string())
        .expect("platform secret cached");
    let mut variables = vars(&[
        ("TEMPER_SECRET_BUILD_TOKEN", DISTINCT_VALUE),
        ("TEMPER_SECRET_build_token", DISTINCT_VALUE),
        ("TEMPER_SECRET_REGION", DISTINCT_VALUE),
    ]);
    variables.extend(
        (0..crate::secrets::vault::MAX_SECRETS_PER_TENANT).flat_map(|i| {
            vars(&[(
                format!("TEMPER_SECRET_FILL_{i:03}").as_str(),
                DISTINCT_VALUE,
            )])
        }),
    );

    let (report, log) = seed_with_log(&vault, variables);

    let reasons: Vec<EnvironmentSecretSkip> =
        report.skipped.iter().map(|(_, reason)| *reason).collect();
    for reason in [
        EnvironmentSecretSkip::InvalidName,
        EnvironmentSecretSkip::AlreadySet,
        EnvironmentSecretSkip::BudgetExhausted,
    ] {
        assert!(reasons.contains(&reason), "{reason:?} not exercised");
    }
    assert!(!report.seeded.is_empty());
    assert!(!log.is_empty());
    assert!(!log.contains(DISTINCT_VALUE), "{log}");
    assert!(
        !format!("{report:?}").contains(DISTINCT_VALUE),
        "{report:?}"
    );
}

#[cfg(unix)]
#[test]
fn value_that_is_not_utf8_is_skipped_and_reported_by_name() {
    use std::os::unix::ffi::OsStringExt;

    let vault = test_vault();
    let variables = vec![(
        OsString::from("TEMPER_SECRET_BUILD_TOKEN"),
        OsString::from_vec(vec![0x61, 0xff, 0x62]),
    )];

    let (report, log) = seed_with_log(&vault, variables);

    assert!(report.seeded.is_empty());
    assert_eq!(
        report.skipped,
        vec![(
            "TEMPER_SECRET_BUILD_TOKEN".to_string(),
            EnvironmentSecretSkip::ValueNotUnicode
        )]
    );
    assert_eq!(vault.get_platform_secret("build_token"), None);
    assert_eq!(log.lines().count(), 1, "{log}");
    assert!(log.contains("variable=TEMPER_SECRET_BUILD_TOKEN "), "{log}");
}

#[cfg(unix)]
#[test]
fn name_that_is_not_utf8_is_skipped_and_reported() {
    use std::os::unix::ffi::OsStringExt;

    let vault = test_vault();
    let mut name = b"TEMPER_SECRET_BUILD".to_vec();
    name.push(0xff);
    let variables = vec![(OsString::from_vec(name), OsString::from("abc"))];

    let report = seed_platform_secrets_from_environment(&vault, variables);

    assert!(report.seeded.is_empty());
    assert_eq!(
        report.skipped,
        vec![(
            "TEMPER_SECRET_BUILD\u{fffd}".to_string(),
            EnvironmentSecretSkip::InvalidName
        )]
    );
    assert!(vault.get_platform_secrets().is_empty());
}

#[test]
fn name_the_server_already_set_keeps_its_value() {
    let vault = test_vault();
    // What start-up does for ANTHROPIC_API_KEY before the prefixed variables.
    vault
        .cache_platform_secret("anthropic_api_key", "from-fixed-variable".to_string())
        .expect("platform secret cached");

    let (report, log) = seed_with_log(
        &vault,
        vars(&[
            ("TEMPER_SECRET_ANTHROPIC_API_KEY", "from-prefixed-variable"),
            ("TEMPER_SECRET_BUILD_TOKEN", "abc"),
        ]),
    );

    assert_eq!(
        vault.get_secret(TENANT, "anthropic_api_key"),
        Some("from-fixed-variable".into())
    );
    assert_eq!(report.seeded, vec!["build_token".to_string()]);
    assert_eq!(
        report.skipped,
        vec![(
            "TEMPER_SECRET_ANTHROPIC_API_KEY".to_string(),
            EnvironmentSecretSkip::AlreadySet
        )]
    );
    assert!(
        log.contains("variable=TEMPER_SECRET_ANTHROPIC_API_KEY "),
        "{log}"
    );
    assert!(!log.contains("from-prefixed-variable"), "{log}");
    assert!(!log.contains("from-fixed-variable"), "{log}");
}

#[test]
fn prefixed_form_of_a_fixed_name_is_seeded_when_the_server_has_not_set_it() {
    let vault = test_vault();

    let report = seed_platform_secrets_from_environment(
        &vault,
        vars(&[("TEMPER_SECRET_ANTHROPIC_API_KEY", "from-prefixed-variable")]),
    );

    assert_eq!(report.seeded, vec!["anthropic_api_key".to_string()]);
    assert_eq!(
        vault.get_secret(TENANT, "anthropic_api_key"),
        Some("from-prefixed-variable".into())
    );
}

#[test]
fn stored_tenant_secret_wins_over_a_seeded_one_for_that_tenant_only() {
    let vault = test_vault();
    // tenant-a stored its own build_token before the server started;
    // tenant-b stores one after.
    vault
        .cache_secret("tenant-a", "build_token", "stored-by-a".to_string())
        .expect("tenant secret cached");

    seed_platform_secrets_from_environment(&vault, vars(&[("TEMPER_SECRET_BUILD_TOKEN", "abc")]));
    vault
        .cache_secret("tenant-b", "build_token", "stored-by-b".to_string())
        .expect("tenant secret cached");

    assert_eq!(
        vault.get_secret("tenant-a", "build_token"),
        Some("stored-by-a".into())
    );
    assert_eq!(
        vault.get_secret("tenant-b", "build_token"),
        Some("stored-by-b".into())
    );
    assert_eq!(
        vault.get_tenant_secrets("tenant-a").get("build_token"),
        Some(&"stored-by-a".to_string())
    );
    // A tenant with no stored secret of that name reads the seeded one.
    assert_eq!(
        vault.get_secret("tenant-c", "build_token"),
        Some("abc".into())
    );

    // Removing the stored secret uncovers the seeded one again.
    assert!(vault.remove_secret("tenant-a", "build_token"));
    assert_eq!(
        vault.get_secret("tenant-a", "build_token"),
        Some("abc".into())
    );
}

#[test]
fn variables_over_the_platform_budget_are_skipped_in_name_order() {
    let vault = test_vault();
    let budget = crate::secrets::vault::MAX_SECRETS_PER_TENANT;
    // Listed in reverse, to show the outcome follows the names and not the
    // order the environment lists them in.
    let variables: Vec<(OsString, OsString)> = (0..budget + 2)
        .rev()
        .flat_map(|i| vars(&[(format!("TEMPER_SECRET_KEY_{i:03}").as_str(), "abc")]))
        .collect();

    let report = seed_platform_secrets_from_environment(&vault, variables);

    assert_eq!(report.seeded.len(), budget);
    assert_eq!(report.seeded.first().map(String::as_str), Some("key_000"));
    assert_eq!(
        report.skipped,
        vec![
            (
                format!("TEMPER_SECRET_KEY_{budget:03}"),
                EnvironmentSecretSkip::BudgetExhausted
            ),
            (
                format!("TEMPER_SECRET_KEY_{:03}", budget + 1),
                EnvironmentSecretSkip::BudgetExhausted
            ),
        ]
    );
    assert_eq!(vault.get_platform_secrets().len(), budget);
}
