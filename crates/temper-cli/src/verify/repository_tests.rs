//! Verify source collections without claiming they are packaged applications.
//!
//! Several examples supply their policies or modules through their Rust host or
//! build scripts. The CLI requires the assembled files; this test keeps checking
//! their CSDL and IOA behavior before packaging, using the production cascade.
use super::*;
use std::collections::{BTreeMap, BTreeSet};

fn specification_directories(root: &Path, found: &mut BTreeSet<std::path::PathBuf>) {
    let mut has_ioa = false;
    for entry in fs::read_dir(root).unwrap() {
        let entry = entry.unwrap();
        let path = entry.path();
        let kind = entry.file_type().unwrap();
        if kind.is_dir() {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if name != "target" && !name.starts_with('.') {
                specification_directories(&path, found);
            }
        } else if path.to_string_lossy().ends_with(".ioa.toml") {
            has_ioa = true;
        }
    }
    if has_ioa && root.join("model.csdl.xml").is_file() {
        found.insert(root.to_owned());
    }
}

#[test]
fn repository_source_collections_pass_behavior_verification() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut directories = BTreeSet::new();
    specification_directories(&root, &mut directories);
    assert!(!directories.is_empty());
    let mut failures = Vec::new();
    for directory in directories {
        let result = (|| -> Result<()> {
            let xml = fs::read_to_string(directory.join("model.csdl.xml"))?;
            input::validate_xml_document(&xml)?;
            let model = build_spec_model(parse_csdl(&xml)?, read_tla_sources(&directory)?);
            anyhow::ensure!(model.validation.is_valid(), "{:?}", model.validation.errors);
            if directory.ends_with("test-fixtures/specs") {
                // This is a catalog of independent test inputs, including two
                // alternative Process definitions, not one application. Check
                // each input without combining those mutually exclusive specs.
                for entry in fs::read_dir(&directory)? {
                    let path = entry?.path();
                    if path.to_string_lossy().ends_with(".ioa.toml") {
                        let source = fs::read_to_string(&path)?;
                        let name = temper_spec::automaton::parse_automaton(&source)?
                            .automaton
                            .name;
                        verify_ioa_sources(&BTreeMap::from([(name, source)]))
                            .with_context(|| format!("{}", path.display()))?;
                    }
                }
                Ok(())
            } else {
                verify_ioa_sources(&read_ioa_sources(&directory)?)
            }
        })();
        if let Err(error) = result {
            failures.push(format!("{}: {error:#}", directory.display()));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
