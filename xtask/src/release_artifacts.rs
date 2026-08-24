use std::{fs, path::Path};

use sha2::{Digest, Sha256};

const FORMAT: &str = "denju-release-manifest-v1";
pub(crate) const CLIENT_ASSETS: [&str; 6] = [
    "denju_darwin_amd64",
    "denju_darwin_arm64",
    "denju_linux_amd64",
    "denju_linux_arm64",
    "denju_windows_amd64.exe",
    "denju_windows_arm64.exe",
];

pub(crate) fn run(root: &Path) -> Result<(), String> {
    let mut args = std::env::args().skip(2);
    let mut version = None;
    let mut dist = None;
    while let Some(argument) = args.next() {
        match argument.as_str() {
            "--version" => version = args.next(),
            "--dist" => dist = args.next(),
            other => return Err(format!("unknown release-manifest option: {other}")),
        }
    }
    let version =
        version.ok_or_else(|| "release-manifest requires --version VERSION".to_owned())?;
    validate_version(&version)?;
    let dist = root.join(dist.unwrap_or_else(|| "dist".to_owned()));
    write_manifest(&dist, &version)
}

pub(crate) fn write_manifest(dist: &Path, version: &str) -> Result<(), String> {
    if !dist.is_dir() {
        return Err(format!(
            "release dist directory does not exist: {}",
            dist.display()
        ));
    }
    let mut manifest = format!("format {FORMAT}\nversion {version}\n");
    let mut checksums = String::new();
    for name in CLIENT_ASSETS {
        let path = dist.join(name);
        let bytes = fs::read(&path)
            .map_err(|error| format!("failed to read release asset {}: {error}", path.display()))?;
        let digest = format!("{:x}", Sha256::digest(&bytes));
        manifest.push_str(&format!("asset {name} {digest} {}\n", bytes.len()));
        checksums.push_str(&format!("{digest}  {name}\n"));
    }
    manifest.push_str(&format!(
        "server_image ghcr.io/amxv/denju-server:v{version}\n"
    ));
    fs::write(dist.join("release-manifest.txt"), manifest)
        .map_err(|error| format!("failed to write release manifest: {error}"))?;
    fs::write(dist.join("checksums.txt"), checksums)
        .map_err(|error| format!("failed to write checksum manifest: {error}"))?;
    println!(
        "release manifest: {}",
        dist.join("release-manifest.txt").display()
    );
    Ok(())
}

fn validate_version(version: &str) -> Result<(), String> {
    let valid = !version.is_empty()
        && version.len() <= 64
        && version
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'+'));
    if valid {
        Ok(())
    } else {
        Err(format!("invalid release version: {version}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manifest_is_stable_and_contains_every_client_asset() {
        let temporary = tempfile::tempdir().unwrap();
        for (index, name) in CLIENT_ASSETS.iter().enumerate() {
            fs::write(temporary.path().join(name), format!("asset-{index}\n")).unwrap();
        }
        write_manifest(temporary.path(), "1.2.3").unwrap();
        let manifest = fs::read_to_string(temporary.path().join("release-manifest.txt")).unwrap();
        assert!(manifest.starts_with("format denju-release-manifest-v1\nversion 1.2.3\n"));
        for name in CLIENT_ASSETS {
            assert!(
                manifest
                    .lines()
                    .any(|line| line.starts_with(&format!("asset {name} ")))
            );
        }
        assert!(manifest.contains("server_image ghcr.io/amxv/denju-server:v1.2.3"));
        assert_eq!(
            fs::read_to_string(temporary.path().join("checksums.txt"))
                .unwrap()
                .lines()
                .count(),
            CLIENT_ASSETS.len()
        );
    }
}
