//! Validate a complete deployment artifact with the same verifier used by `temper verify`.
use anyhow::{Context, Result};
use std::path::Path;

pub(crate) fn run(source: &Path) -> Result<()> {
    let specs = source.join("specs");
    anyhow::ensure!(
        specs.join("model.csdl.xml").is_file(),
        "missing specs/model.csdl.xml"
    );
    let mut policies = String::new();
    let mut entities = 0;
    for entry in std::fs::read_dir(&specs)? {
        let path = entry?.path();
        let Some(stem) = path
            .file_name()
            .and_then(|s| s.to_str())
            .and_then(|s| s.strip_suffix(".ioa.toml"))
        else {
            continue;
        };
        let automaton = temper_spec::automaton::parse_automaton(&std::fs::read_to_string(&path)?)?;
        for module in automaton
            .actions
            .iter()
            .flat_map(|a| &a.triggers)
            .filter_map(|t| t.module.as_deref())
            .chain(
                automaton
                    .integrations
                    .iter()
                    .filter_map(|i| i.module.as_deref()),
            )
        {
            anyhow::ensure!(
                !module.is_empty()
                    && module
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-'),
                "invalid module name"
            );
            anyhow::ensure!(
                specs
                    .join("modules")
                    .join(format!("{module}.wasm"))
                    .is_file(),
                "missing compiled WASM module {module}"
            );
        }
        policies.push_str(
            &std::fs::read_to_string(specs.join("policies").join(format!("{stem}.cedar")))
                .with_context(|| format!("missing Cedar policy for {stem}"))?,
        );
        policies.push('\n');
        entities += 1;
    }
    anyhow::ensure!(entities > 0, "source contains no IOA specifications");
    temper_authz::AuthzEngine::new(&policies).context("invalid Cedar policy")?;
    super::run_specs(&specs, true)
}
