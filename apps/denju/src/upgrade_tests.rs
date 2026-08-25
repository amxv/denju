use super::*;

#[test]
fn release_manifest_binds_exact_size_and_sha() {
    let bytes = b"denju-binary";
    let sha = format!("{:x}", Sha256::digest(bytes));
    let text = valid_manifest("2.3.4", &sha, bytes.len());
    let manifest = parse_manifest(&text).unwrap();
    assert_eq!(manifest.version, "2.3.4");
    let asset = manifest
        .assets
        .iter()
        .find(|asset| asset.name == "denju_linux_amd64")
        .unwrap();
    verify_asset(asset, bytes).unwrap();
    assert!(verify_asset(asset, b"denju-binarx").is_err());
}

#[test]
fn malformed_release_manifest_fails_closed() {
    assert!(parse_manifest("version 1.0.0\n").is_err());
    assert!(
        parse_manifest("format denju-release-manifest-v1\nversion 1.0.0\nasset x nope 10\n")
            .is_err()
    );
    let sha = "0".repeat(64);
    let manifest = valid_manifest("1.0.0", &sha, 10);
    assert!(parse_manifest(&format!("{manifest}future_field nope\n")).is_err());
    assert!(parse_manifest(&format!("{manifest}version 1.0.0\n")).is_err());
    assert!(
        parse_manifest(&manifest.replace(
            "server_image ghcr.io/amxv/denju-server:v1.0.0",
            "server_image ghcr.io/example/denju-server:v1.0.0"
        ))
        .is_err()
    );
    assert!(parse_manifest(&manifest.replace("version 1.0.0", "version ../1.0.0")).is_err());
}

#[cfg(unix)]
#[test]
fn standalone_upgrade_replaces_only_after_verification_and_runs_new_health() {
    let temporary = tempfile::tempdir().unwrap();
    let paths = LocalPaths::from_home(temporary.path().to_path_buf());
    let target = temporary.path().join("denju");
    write_fake_binary(&target, "1.0.0", true);
    let next = fake_binary("2.0.0", true);

    let outcome = apply_standalone_upgrade(&paths, "1.0.0", "2.0.0", &target, &next).unwrap();

    assert_eq!(outcome.state, "upgraded");
    assert_eq!(outcome.previous_version, "1.0.0");
    assert_eq!(outcome.version, "2.0.0");
    assert!(outcome.health_verified);
    assert!(!outcome.daemon_restarted);
    assert_eq!(binary_version(&target).unwrap(), "2.0.0");
}

#[cfg(unix)]
#[test]
fn failed_new_binary_health_restores_the_exact_previous_executable() {
    let temporary = tempfile::tempdir().unwrap();
    let paths = LocalPaths::from_home(temporary.path().to_path_buf());
    let target = temporary.path().join("denju");
    let previous = fake_binary("1.0.0", true);
    write_executable(&target, &previous);
    let unhealthy = fake_binary("2.0.0", false);

    let error =
        apply_standalone_upgrade(&paths, "1.0.0", "2.0.0", &target, &unhealthy).unwrap_err();

    assert!(error.message.contains("previous executable was restored"));
    assert_eq!(binary_version(&target).unwrap(), "1.0.0");
    assert_eq!(fs::read(&target).unwrap(), previous);
}

#[cfg(unix)]
#[test]
fn npm_upgrade_runs_new_health_and_reports_the_new_version() {
    let temporary = tempfile::tempdir().unwrap();
    let paths = LocalPaths::from_home(temporary.path().to_path_buf());
    let target = temporary.path().join("denju");
    let previous = temporary.path().join("previous");
    let next = temporary.path().join("next");
    let npm = temporary.path().join("npm");
    write_fake_binary(&target, "1.0.0", true);
    write_fake_binary(&previous, "1.0.0", true);
    write_fake_binary(&next, "2.0.0", true);
    write_fake_npm(&npm, &target, &previous, &next);

    let outcome = apply_package_upgrade(
        &paths,
        "1.0.0",
        "denju-cli",
        "1.0.0",
        &target,
        PackageManager::Npm,
        npm.as_os_str(),
    )
    .unwrap();

    assert_eq!(outcome.state, "upgraded");
    assert_eq!(outcome.version, "2.0.0");
    assert_eq!(binary_version(&target).unwrap(), "2.0.0");
}

#[cfg(unix)]
#[test]
fn npm_upgrade_verification_failure_reinstalls_and_health_checks_previous_version() {
    let temporary = tempfile::tempdir().unwrap();
    let paths = LocalPaths::from_home(temporary.path().to_path_buf());
    let target = temporary.path().join("denju");
    let previous = temporary.path().join("previous");
    let next = temporary.path().join("next");
    let npm = temporary.path().join("npm");
    write_fake_binary(&target, "1.0.0", true);
    write_fake_binary(&previous, "1.0.0", true);
    write_fake_binary(&next, "2.0.0", false);
    write_fake_npm(&npm, &target, &previous, &next);

    let error = apply_package_upgrade(
        &paths,
        "1.0.0",
        "denju-cli",
        "1.0.0",
        &target,
        PackageManager::Npm,
        npm.as_os_str(),
    )
    .unwrap_err();

    assert!(
        error
            .message
            .contains("npm restored the previous package version")
    );
    assert_eq!(fs::read(&target).unwrap(), fs::read(&previous).unwrap());
    assert_eq!(binary_version(&target).unwrap(), "1.0.0");
}

#[cfg(unix)]
#[test]
fn vite_plus_upgrade_uses_vite_plus_global_install_contract() {
    let temporary = tempfile::tempdir().unwrap();
    let paths = LocalPaths::from_home(temporary.path().to_path_buf());
    let target = temporary.path().join("denju");
    let previous = temporary.path().join("previous");
    let next = temporary.path().join("next");
    let vite_plus = temporary.path().join("vp");
    let log = temporary.path().join("vp-args.log");
    write_fake_binary(&target, "1.0.0", true);
    write_fake_binary(&previous, "1.0.0", true);
    write_fake_binary(&next, "2.0.0", true);
    write_fake_vite_plus(&vite_plus, &target, &previous, &next, &log);

    let outcome = apply_package_upgrade(
        &paths,
        "1.0.0",
        "denju-cli",
        "1.0.0",
        &target,
        PackageManager::VitePlus,
        vite_plus.as_os_str(),
    )
    .unwrap();

    assert_eq!(outcome.state, "upgraded");
    assert_eq!(outcome.source, "vite-plus");
    assert_eq!(outcome.version, "2.0.0");
    assert_eq!(binary_version(&target).unwrap(), "2.0.0");
    assert_eq!(
        fs::read_to_string(log).unwrap(),
        "install|-g|denju-cli@latest|--|--allow-scripts=denju-cli\n"
    );
}

#[cfg(unix)]
fn write_fake_binary(path: &Path, version: &str, healthy: bool) {
    write_executable(path, &fake_binary(version, healthy));
}

#[cfg(unix)]
fn write_executable(path: &Path, bytes: &[u8]) {
    use std::os::unix::fs::PermissionsExt;

    fs::write(path, bytes).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
}

#[cfg(unix)]
fn write_fake_npm(npm: &Path, target: &Path, previous: &Path, next: &Path) {
    let script = format!(
        "#!/bin/sh\nset -eu\ncase \"$4\" in\n  denju-cli@latest) cp '{}' '{}' ;;\n  denju-cli@1.0.0) cp '{}' '{}' ;;\n  *) exit 3 ;;\nesac\nchmod 755 '{}'\n",
        next.display(),
        target.display(),
        previous.display(),
        target.display(),
        target.display()
    );
    write_executable(npm, script.as_bytes());
}

#[cfg(unix)]
fn write_fake_vite_plus(vite_plus: &Path, target: &Path, previous: &Path, next: &Path, log: &Path) {
    let script = format!(
        "#!/bin/sh\nset -eu\nprintf '%s|%s|%s|%s|%s\\n' \"$1\" \"$2\" \"$3\" \"$4\" \"$5\" >> '{}'\ncase \"$3\" in\n  denju-cli@latest) cp '{}' '{}' ;;\n  denju-cli@1.0.0) cp '{}' '{}' ;;\n  *) exit 3 ;;\nesac\nchmod 755 '{}'\n",
        log.display(),
        next.display(),
        target.display(),
        previous.display(),
        target.display(),
        target.display()
    );
    write_executable(vite_plus, script.as_bytes());
}

#[cfg(unix)]
fn fake_binary(version: &str, healthy: bool) -> Vec<u8> {
    let health = if healthy { 0 } else { 1 };
    format!(
            "#!/bin/sh\ncase \"$1\" in\n  --version) echo 'denju {version}' ;;\n  upgrade-health) exit {health} ;;\n  *) exit 2 ;;\nesac\n"
        )
        .into_bytes()
}

fn valid_manifest(version: &str, sha: &str, size: usize) -> String {
    let mut text = format!("format {MANIFEST_FORMAT}\nversion {version}\n");
    for name in CLIENT_ASSETS {
        text.push_str(&format!("asset {name} {sha} {size}\n"));
    }
    text.push_str(&format!(
        "server_image {OFFICIAL_SERVER_IMAGE}:v{version}\n"
    ));
    text
}
