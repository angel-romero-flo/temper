//! Input checks shared by directory and complete-application verification.
use anyhow::{Context, Result};
use std::{collections::BTreeMap, fs, path::Path};
use temper_spec::csdl::CsdlDocument;

pub(super) fn validate_ioa_entities(
    csdl: &CsdlDocument,
    sources: &BTreeMap<String, String>,
) -> Result<()> {
    for name in sources.keys() {
        anyhow::ensure!(
            csdl.schemas.iter().any(|schema| schema
                .entity_types
                .iter()
                .any(|entity| entity.name == *name)),
            "IOA entity {name} is missing from CSDL"
        );
        anyhow::ensure!(
            csdl.schemas
                .iter()
                .flat_map(|schema| &schema.entity_containers)
                .flat_map(|container| &container.entity_sets)
                .any(|set| set.entity_type.rsplit('.').next() == Some(name.as_str())),
            "IOA entity {name} has no CSDL entity set"
        );
    }
    Ok(())
}

/// Read all `.ioa.toml` files from the specs directory.
pub(super) fn read_ioa_sources(
    specs_dir: &Path,
    application: bool,
) -> Result<BTreeMap<String, String>> {
    let mut sources = BTreeMap::new();

    if !specs_dir.is_dir() {
        return Ok(sources);
    }

    for entry in fs::read_dir(specs_dir)
        .with_context(|| format!("Failed to read specs directory: {}", specs_dir.display()))?
    {
        let entry = entry?;
        let path = entry.path();

        let file_name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default();

        if file_name.ends_with(".ioa.toml") {
            let source = fs::read_to_string(&path)
                .with_context(|| format!("Failed to read IOA file: {}", path.display()))?;

            let entity_name = if application {
                temper_spec::automaton::parse_automaton(&source)?
                    .automaton
                    .name
            } else {
                // Standalone fixture collections may contain several alternative
                // definitions of one entity; retain their distinct file identities.
                crate::util::to_pascal_case(file_name.trim_end_matches(".ioa.toml"))
            };
            anyhow::ensure!(
                sources.insert(entity_name.clone(), source).is_none(),
                "duplicate entity {entity_name}"
            );
        }
    }

    Ok(sources)
}

/// Reject truncated XML and unrelated XML documents before semantic verification.
pub(super) fn validate_xml_document(xml: &str) -> Result<()> {
    use quick_xml::{Reader, events::Event};
    let mut reader = Reader::from_str(xml);
    let mut depth = 0usize;
    let mut roots = 0usize;
    loop {
        match reader.read_event().context("malformed CSDL XML")? {
            Event::Start(element) => {
                if depth == 0 {
                    roots += 1;
                    anyhow::ensure!(
                        element.local_name().as_ref() == b"Edmx",
                        "CSDL root must be Edmx"
                    );
                }
                depth += 1;
            }
            Event::Empty(element) if depth == 0 => {
                roots += 1;
                anyhow::ensure!(
                    element.local_name().as_ref() == b"Edmx",
                    "CSDL root must be Edmx"
                );
            }
            Event::End(_) => {
                depth = depth.checked_sub(1).context("unbalanced CSDL XML")?;
            }
            Event::DocType(_) => anyhow::bail!("CSDL document types are not supported"),
            Event::Eof => break,
            _ => {}
        }
    }
    anyhow::ensure!(
        depth == 0 && roots == 1,
        "CSDL must be one complete XML document"
    );
    Ok(())
}
