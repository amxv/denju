use std::{
    fs,
    path::{Path, PathBuf},
};

use sha2::{Digest, Sha256};

const FIXTURE_CHECKSUMS: &str = "spec/fixtures/checksums.sha256";

pub(crate) fn check(root: &Path) -> Result<(), String> {
    check_fixture_checksums(root)?;
    check_fixture_coverage(root)?;
    check_sqlx_offline_policy(root)?;
    check_automation_authority(root)?;
    println!("repository contracts: passed");
    Ok(())
}

pub(crate) fn update_fixture_checksums(root: &Path) -> Result<(), String> {
    let manifest = fixture_checksum_manifest(root)?;
    fs::write(root.join(FIXTURE_CHECKSUMS), manifest)
        .map_err(|error| format!("failed to update {FIXTURE_CHECKSUMS}: {error}"))?;
    println!("updated {FIXTURE_CHECKSUMS}");
    Ok(())
}

fn check_fixture_checksums(root: &Path) -> Result<(), String> {
    let expected_path = root.join(FIXTURE_CHECKSUMS);
    let expected = fs::read_to_string(&expected_path)
        .map_err(|error| format!("failed to read {FIXTURE_CHECKSUMS}: {error}"))?;
    let actual = fixture_checksum_manifest(root)?;
    if normalize_newlines(&expected) == actual {
        Ok(())
    } else {
        Err(format!(
            "spec fixture drift detected; regenerate {FIXTURE_CHECKSUMS} with `cargo xtask contracts --update` after intentionally reviewing changed vectors"
        ))
    }
}

fn fixture_checksum_manifest(root: &Path) -> Result<String, String> {
    let fixture_root = root.join("spec/fixtures");
    let mut files = Vec::new();
    collect_files(&fixture_root, &mut files)?;
    files.retain(|path| path != &root.join(FIXTURE_CHECKSUMS));
    files.sort();

    let mut manifest = String::new();
    for path in files {
        let bytes = fs::read(&path)
            .map_err(|error| format!("failed to read fixture {}: {error}", path.display()))?;
        let relative = path
            .strip_prefix(root)
            .map_err(|error| error.to_string())?
            .to_string_lossy()
            .replace('\\', "/");
        manifest.push_str(&format!("{:x}  {relative}\n", Sha256::digest(bytes)));
    }
    Ok(manifest)
}

fn check_fixture_coverage(root: &Path) -> Result<(), String> {
    let fixture_root = root.join("spec/fixtures");
    let mut fixtures = Vec::new();
    collect_files(&fixture_root, &mut fixtures)?;
    let json_fixtures = fixtures
        .into_iter()
        .filter(|path| {
            path.extension()
                .is_some_and(|extension| extension == "json")
        })
        .collect::<Vec<_>>();

    let mut rust_files = Vec::new();
    collect_rust_files(&root.join("crates"), &mut rust_files)?;
    let mut source = String::new();
    for path in rust_files {
        source.push_str(
            &fs::read_to_string(&path).map_err(|error| {
                format!("failed to read Rust source {}: {error}", path.display())
            })?,
        );
        source.push('\n');
    }
    for fixture in json_fixtures {
        let name = fixture
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| format!("non-UTF8 fixture path: {}", fixture.display()))?;
        if !source.contains(name) {
            return Err(format!(
                "spec fixture {name} is not consumed by a Rust conformance test"
            ));
        }
    }
    Ok(())
}

fn check_sqlx_offline_policy(root: &Path) -> Result<(), String> {
    let mut rust_files = Vec::new();
    collect_rust_files(&root.join("apps"), &mut rust_files)?;
    collect_rust_files(&root.join("crates"), &mut rust_files)?;
    let compile_time_macros = [
        "sqlx::query!(",
        "sqlx::query_as!(",
        "sqlx::query_scalar!(",
        "sqlx::query_file!(",
        "sqlx::query_file_as!(",
        "sqlx::query_file_scalar!(",
    ];
    let mut macro_sites = Vec::new();
    for path in rust_files {
        let source = fs::read_to_string(&path)
            .map_err(|error| format!("failed to read {}: {error}", path.display()))?;
        if compile_time_macros
            .iter()
            .any(|needle| source.contains(needle))
        {
            macro_sites.push(path);
        }
    }

    let sqlx = root.join(".sqlx");
    let mut metadata = Vec::new();
    collect_files(&sqlx, &mut metadata)?;
    let has_metadata = metadata.iter().any(|path| {
        path.file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.starts_with("query-") && name.ends_with(".json"))
    });
    if !macro_sites.is_empty() && !has_metadata {
        return Err(format!(
            "SQLx compile-time query macros exist but .sqlx offline metadata is absent; first site: {}",
            macro_sites[0].display()
        ));
    }
    if macro_sites.is_empty() && has_metadata {
        return Err(
            "stale .sqlx query metadata exists although the workspace uses only runtime SQLx query APIs"
                .to_owned(),
        );
    }
    Ok(())
}

fn check_automation_authority(root: &Path) -> Result<(), String> {
    for forbidden in ["Makefile", "makefile", "GNUmakefile"] {
        if root.join(forbidden).exists() {
            return Err(format!(
                "{forbidden} introduces a second automation authority; use xtask with thin Justfile aliases"
            ));
        }
    }
    Ok(())
}

fn collect_rust_files(directory: &Path, output: &mut Vec<PathBuf>) -> Result<(), String> {
    collect_files(directory, output)?;
    output.retain(|path| path.extension().is_some_and(|extension| extension == "rs"));
    Ok(())
}

fn collect_files(directory: &Path, output: &mut Vec<PathBuf>) -> Result<(), String> {
    let entries = fs::read_dir(directory)
        .map_err(|error| format!("failed to read {}: {error}", directory.display()))?;
    for entry in entries {
        let entry = entry.map_err(|error| error.to_string())?;
        let path = entry.path();
        let file_type = entry.file_type().map_err(|error| error.to_string())?;
        if file_type.is_dir() {
            collect_files(&path, output)?;
        } else if file_type.is_file() {
            output.push(path);
        }
    }
    Ok(())
}

fn normalize_newlines(value: &str) -> String {
    value.replace("\r\n", "\n")
}
