use std::{
    fs,
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::Duration,
};

use serde_json::Value;
use uuid::Uuid;

use crate::release_artifacts::{self, CLIENT_ASSETS};

const SMOKE_VERSION: &str = "0.3.0-smoke";
const SMOKE_NEXT_VERSION: &str = "0.3.0-smoke-next";
const SMOKE_FAILED_VERSION: &str = "0.3.0-smoke-failed";

pub(crate) fn run(root: &Path) -> Result<(), String> {
    let temporary = SmokeDirectory::new()?;
    let home = temporary.path().join("home");
    let dist = temporary.path().join("dist");
    fs::create_dir_all(&home).map_err(io_error("create release-smoke home"))?;
    fs::create_dir_all(&dist).map_err(io_error("create release-smoke dist"))?;
    fs::write(home.join(".denju-test-home-v1"), b"release smoke\n")
        .map_err(io_error("write release-smoke home marker"))?;

    let native = build_native_client(root, SMOKE_VERSION)?;
    let native_bytes = fs::read(&native).map_err(io_error("read release-smoke native client"))?;
    stage_release(&dist, SMOKE_VERSION, &native_bytes)?;
    let server = StaticReleaseServer::start(dist.clone())?;
    let base = format!("http://{}", server.address());

    #[cfg(unix)]
    let standalone = smoke_posix_installer(root, &home, &base, &native_bytes)?;
    #[cfg(windows)]
    let standalone = smoke_powershell_installer(root, &home, &base, &native_bytes)?;
    smoke_npm_installer(root, temporary.path(), &home, &base, &native_bytes)?;
    smoke_standalone_upgrade(
        temporary.path(),
        &home,
        &dist,
        &base,
        &standalone,
        &native_bytes,
    )?;

    println!("release smoke: installers, manifest, upgrade, and rollback passed");
    Ok(())
}

fn build_native_client(root: &Path, version: &str) -> Result<PathBuf, String> {
    eprintln!("+ DENJU_BUILD_VERSION={version} cargo build --release -p denju --target-dir target");
    let status = Command::new("cargo")
        .args([
            "build",
            "--release",
            "-p",
            "denju",
            "--target-dir",
            "target",
        ])
        .env("DENJU_BUILD_VERSION", version)
        .current_dir(root)
        .status()
        .map_err(|error| format!("failed to build release-smoke client: {error}"))?;
    if !status.success() {
        return Err(format!("release-smoke client build exited with {status}"));
    }
    let extension = if cfg!(windows) { ".exe" } else { "" };
    let path = root.join(format!("target/release/denju{extension}"));
    if path.is_file() {
        Ok(path)
    } else {
        Err(format!(
            "release-smoke client binary was not produced at {}",
            path.display()
        ))
    }
}

fn stage_release(dist: &Path, version: &str, bytes: &[u8]) -> Result<(), String> {
    for name in CLIENT_ASSETS {
        fs::write(dist.join(name), bytes).map_err(io_error("stage release-smoke client asset"))?;
    }
    release_artifacts::write_manifest(dist, version)
}

#[cfg(unix)]
fn smoke_posix_installer(
    root: &Path,
    home: &Path,
    base: &str,
    expected: &[u8],
) -> Result<PathBuf, String> {
    let install_dir = home.join("standalone-bin");
    let status = safe_command(Command::new("sh"), home)
        .arg(root.join("install/denju.sh"))
        .env("DENJU_RELEASE_BASE_URL", base)
        .env("DENJU_INSTALL_DIR", &install_dir)
        .status()
        .map_err(|error| format!("failed to run POSIX installer smoke: {error}"))?;
    if !status.success() {
        return Err(format!("POSIX installer smoke exited with {status}"));
    }
    let installed = install_dir.join("denju");
    assert_installed_bytes(&installed, expected, "POSIX installer")?;
    assert_binary_version(&installed, home, "POSIX installer")?;
    let source = fs::read_to_string(home.join(".denju/install-source.json"))
        .map_err(io_error("read POSIX installer source metadata"))?;
    if !source.contains("\"source\":\"standalone\"") {
        return Err("POSIX installer did not record standalone source metadata".to_owned());
    }
    Ok(installed)
}

#[cfg(windows)]
fn smoke_powershell_installer(
    root: &Path,
    home: &Path,
    base: &str,
    expected: &[u8],
) -> Result<PathBuf, String> {
    let install_dir = home.join("standalone-bin");
    let status = safe_command(Command::new("powershell.exe"), home)
        .args(["-NoProfile", "-ExecutionPolicy", "Bypass", "-File"])
        .arg(root.join("install/denju.ps1"))
        .env("DENJU_RELEASE_BASE_URL", base)
        .env("DENJU_INSTALL_DIR", &install_dir)
        .status()
        .map_err(|error| format!("failed to run PowerShell installer smoke: {error}"))?;
    if !status.success() {
        return Err(format!("PowerShell installer smoke exited with {status}"));
    }
    let installed = install_dir.join("denju.exe");
    assert_installed_bytes(&installed, expected, "PowerShell installer")?;
    assert_binary_version(&installed, home, "PowerShell installer")?;
    Ok(installed)
}

fn smoke_npm_installer(
    root: &Path,
    temporary: &Path,
    home: &Path,
    base: &str,
    expected: &[u8],
) -> Result<(), String> {
    let package = temporary.join("npm-package");
    fs::create_dir_all(package.join("scripts")).map_err(io_error("create npm smoke scripts"))?;
    fs::create_dir_all(package.join("bin")).map_err(io_error("create npm smoke bin"))?;
    fs::copy(
        root.join("packages/npm/scripts/postinstall.js"),
        package.join("scripts/postinstall.js"),
    )
    .map_err(io_error("copy npm postinstall for smoke"))?;
    fs::copy(
        root.join("packages/npm/bin/denju.js"),
        package.join("bin/denju.js"),
    )
    .map_err(io_error("copy npm launcher for smoke"))?;
    let mut metadata: Value = serde_json::from_slice(
        &fs::read(root.join("packages/npm/package.json"))
            .map_err(io_error("read npm package metadata"))?,
    )
    .map_err(|error| format!("parse npm package metadata: {error}"))?;
    metadata["version"] = Value::String(SMOKE_VERSION.to_owned());
    fs::write(
        package.join("package.json"),
        serde_json::to_vec_pretty(&metadata)
            .map_err(|error| format!("serialize npm smoke metadata: {error}"))?,
    )
    .map_err(io_error("write npm smoke metadata"))?;

    let pack = safe_command(Command::new(npm_program()), home)
        .args(["pack", "--silent", "--pack-destination"])
        .arg(temporary)
        .current_dir(&package)
        .output()
        .map_err(|error| format!("failed to pack npm release smoke: {error}"))?;
    if !pack.status.success() {
        return Err(format!(
            "npm pack smoke exited with {}: {}",
            pack.status,
            String::from_utf8_lossy(&pack.stderr).trim()
        ));
    }
    let pack_stdout = String::from_utf8_lossy(&pack.stdout);
    let tarball_name = pack_stdout
        .lines()
        .last()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .ok_or_else(|| "npm pack did not report a tarball name".to_owned())?
        .to_owned();
    let tarball = temporary.join(&tarball_name);
    let npm_registry = NpmRegistryServer::start(&tarball, SMOKE_VERSION)?;
    let npm_registry_url = format!("http://{}", npm_registry.address());
    let prefix = temporary.join("npm-prefix");
    let status = safe_command(Command::new(npm_program()), home)
        .args([
            "install",
            "--global",
            "--no-audit",
            "--no-fund",
            "--allow-scripts=denju-cli",
        ])
        .arg(format!("denju-cli@{SMOKE_VERSION}"))
        .arg(format!("--registry={npm_registry_url}"))
        .env("npm_config_prefix", &prefix)
        .env("DENJU_RELEASE_BASE_URL", base)
        .status()
        .map_err(|error| format!("failed to run npm installer smoke: {error}"))?;
    if !status.success() {
        return Err(format!("npm installer smoke exited with {status}"));
    }
    let installed_package = npm_global_package_path(&prefix);
    let extension = if cfg!(windows) { ".exe" } else { "-bin" };
    let installed = installed_package.join(format!("bin/denju{extension}"));
    assert_installed_bytes(&installed, expected, "npm installer")?;
    assert_binary_version(&installed, home, "npm installer")?;
    let launcher = if cfg!(windows) {
        prefix.join("denju.cmd")
    } else {
        prefix.join("bin/denju")
    };
    assert_binary_version(&launcher, home, "npm launcher")?;
    assert_npm_json_upgrade_isolated(&launcher, home, base, &npm_registry_url, &prefix)?;

    let asset_path = temporary.join("dist").join(current_asset_name()?);
    let valid_asset = fs::read(&asset_path).map_err(io_error("read smoke release asset"))?;
    let mut corrupt_asset = valid_asset.clone();
    let first = corrupt_asset
        .first_mut()
        .ok_or_else(|| "release-smoke native client was unexpectedly empty".to_owned())?;
    *first ^= 0xff;
    fs::write(&asset_path, corrupt_asset).map_err(io_error("write corrupt smoke asset"))?;
    let checksum_failure = safe_command(Command::new("node"), home)
        .arg(installed_package.join("scripts/postinstall.js"))
        .env("DENJU_RELEASE_BASE_URL", base)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    fs::write(&asset_path, valid_asset).map_err(io_error("restore smoke release asset"))?;
    let checksum_failure = checksum_failure
        .map_err(|error| format!("failed to run checksum-negative npm installer smoke: {error}"))?;
    if checksum_failure.success() {
        return Err("npm installer accepted an asset with the wrong SHA-256".to_owned());
    }
    assert_installed_bytes(&installed, expected, "checksum-failed npm reinstall")?;

    let manifest_path = temporary.join("dist/release-manifest.txt");
    let valid_manifest = fs::read(&manifest_path).map_err(io_error("read smoke manifest"))?;
    let mut invalid_manifest = valid_manifest.clone();
    invalid_manifest.extend_from_slice(b"future_field nope\n");
    fs::write(&manifest_path, invalid_manifest)
        .map_err(io_error("write invalid smoke manifest"))?;
    let failed_install = safe_command(Command::new("node"), home)
        .arg(installed_package.join("scripts/postinstall.js"))
        .env("DENJU_RELEASE_BASE_URL", base)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    fs::write(&manifest_path, valid_manifest).map_err(io_error("restore smoke manifest"))?;
    let failed_install = failed_install
        .map_err(|error| format!("failed to run negative npm installer smoke: {error}"))?;
    if failed_install.success() {
        return Err("npm installer accepted an invalid shared release manifest".to_owned());
    }
    assert_installed_bytes(&installed, expected, "failed npm reinstall")
}

fn assert_npm_json_upgrade_isolated(
    launcher: &Path,
    home: &Path,
    release_base: &str,
    npm_registry_url: &str,
    prefix: &Path,
) -> Result<(), String> {
    let output = safe_command(Command::new(launcher), home)
        .args(["--json", "upgrade"])
        .env("DENJU_RELEASE_BASE_URL", release_base)
        .env("npm_config_registry", npm_registry_url)
        .env("npm_config_prefix", prefix)
        .env("npm_config_audit", "false")
        .env("npm_config_fund", "false")
        .output()
        .map_err(|error| format!("failed to execute npm JSON upgrade smoke: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "npm JSON upgrade smoke exited with {}: stdout={} stderr={}",
            output.status,
            String::from_utf8_lossy(&output.stdout).trim(),
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    let stdout = String::from_utf8(output.stdout)
        .map_err(|_| "npm JSON upgrade smoke stdout was not UTF-8".to_owned())?;
    if stdout.lines().count() != 1 {
        return Err(format!(
            "npm JSON upgrade emitted {} stdout lines instead of one: {stdout:?}",
            stdout.lines().count()
        ));
    }
    let result: Value = serde_json::from_str(stdout.trim())
        .map_err(|error| format!("npm JSON upgrade emitted invalid JSON: {error}"))?;
    if result["version"] != 1
        || result["ok"] != true
        || result["result"]["kind"] != "upgrade"
        || result["result"]["source"] != "npm"
        || result["result"]["state"] != "up_to_date"
    {
        return Err(format!(
            "npm JSON upgrade returned an unexpected result: {result}"
        ));
    }
    Ok(())
}

fn npm_global_package_path(prefix: &Path) -> PathBuf {
    if cfg!(windows) {
        prefix.join("node_modules/denju-cli")
    } else {
        prefix.join("lib/node_modules/denju-cli")
    }
}

fn npm_program() -> &'static str {
    if cfg!(windows) { "npm.cmd" } else { "npm" }
}

fn smoke_standalone_upgrade(
    temporary: &Path,
    home: &Path,
    dist: &Path,
    base: &str,
    target: &Path,
    initial_bytes: &[u8],
) -> Result<(), String> {
    // Exercise rollback first while the installed target is still the real Denju binary. The
    // replacement fixtures only need to implement the two probes the updater itself performs;
    // compiling two additional optimized Denju binaries would make this six-platform smoke much
    // slower without testing any additional updater behavior.
    let failed_bytes = build_upgrade_fixture(temporary, SMOKE_FAILED_VERSION, false)?;
    stage_release(dist, SMOKE_FAILED_VERSION, &failed_bytes)?;
    let failure = safe_command(Command::new(target), home)
        .arg("upgrade")
        .env("DENJU_RELEASE_BASE_URL", base)
        .env("DENJU_INSTALL_TARGET", target)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map_err(|error| format!("failed to execute rollback smoke: {error}"))?;
    if failure.success() {
        return Err("standalone upgrade unexpectedly accepted an unhealthy replacement".to_owned());
    }
    assert_installed_bytes(target, initial_bytes, "failed-health standalone rollback")?;
    assert_binary_version_value(
        target,
        home,
        "failed-health standalone rollback",
        SMOKE_VERSION,
    )?;

    let next_bytes = build_upgrade_fixture(temporary, SMOKE_NEXT_VERSION, true)?;
    stage_release(dist, SMOKE_NEXT_VERSION, &next_bytes)?;
    let success = safe_command(Command::new(target), home)
        .arg("upgrade")
        .env("DENJU_RELEASE_BASE_URL", base)
        .env("DENJU_INSTALL_TARGET", target)
        .status()
        .map_err(|error| format!("failed to execute standalone upgrade smoke: {error}"))?;
    if !success.success() {
        return Err(format!("standalone upgrade smoke exited with {success}"));
    }
    assert_installed_bytes(target, &next_bytes, "successful standalone upgrade")?;
    assert_binary_version_value(
        target,
        home,
        "successful standalone upgrade",
        SMOKE_NEXT_VERSION,
    )
}

fn build_upgrade_fixture(
    temporary: &Path,
    version: &str,
    healthy: bool,
) -> Result<Vec<u8>, String> {
    let label = if healthy { "healthy" } else { "unhealthy" };
    let source = temporary.join(format!("upgrade-fixture-{label}.rs"));
    let extension = if cfg!(windows) { ".exe" } else { "" };
    let binary = temporary.join(format!("upgrade-fixture-{label}{extension}"));
    let health_exit = if healthy { 0 } else { 1 };
    let source_text = format!(
        r#"const VERSION: &str = {version:?};
const HEALTH_EXIT: u8 = {health_exit};

fn main() -> std::process::ExitCode {{
    match std::env::args().nth(1).as_deref() {{
        Some("--version") => {{
            println!("denju {{VERSION}}");
            std::process::ExitCode::SUCCESS
        }}
        Some("upgrade-health") => std::process::ExitCode::from(HEALTH_EXIT),
        _ => std::process::ExitCode::from(2),
    }}
}}
"#
    );
    fs::write(&source, source_text).map_err(io_error("write upgrade fixture source"))?;
    let status = Command::new("rustc")
        .arg("--edition=2024")
        .args(["-C", "opt-level=0", "-C", "strip=debuginfo"])
        .arg("-o")
        .arg(&binary)
        .arg(&source)
        .status()
        .map_err(|error| format!("failed to compile upgrade fixture: {error}"))?;
    if !status.success() {
        return Err(format!("upgrade fixture compile exited with {status}"));
    }
    fs::read(&binary).map_err(io_error("read compiled upgrade fixture"))
}

fn assert_installed_bytes(path: &Path, expected: &[u8], label: &str) -> Result<(), String> {
    let actual = fs::read(path).map_err(io_error("read installed release-smoke client"))?;
    if actual == expected {
        Ok(())
    } else {
        Err(format!(
            "{label} made bytes visible that differ from the verified asset"
        ))
    }
}

fn assert_binary_version(path: &Path, home: &Path, label: &str) -> Result<(), String> {
    assert_binary_version_value(path, home, label, SMOKE_VERSION)
}

fn assert_binary_version_value(
    path: &Path,
    home: &Path,
    label: &str,
    expected: &str,
) -> Result<(), String> {
    let output = safe_command(Command::new(path), home)
        .arg("--version")
        .output()
        .map_err(|error| format!("failed to execute {label} binary: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "{label} binary --version exited with {}",
            output.status
        ));
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    if stdout.trim() == format!("denju {expected}") {
        Ok(())
    } else {
        Err(format!(
            "{label} binary reported unexpected version {:?}",
            stdout.trim()
        ))
    }
}

fn current_asset_name() -> Result<String, String> {
    let os = match std::env::consts::OS {
        "macos" => "darwin",
        "linux" => "linux",
        "windows" => "windows",
        other => {
            return Err(format!(
                "unsupported release-smoke operating system: {other}"
            ));
        }
    };
    let arch = match std::env::consts::ARCH {
        "x86_64" => "amd64",
        "aarch64" => "arm64",
        other => return Err(format!("unsupported release-smoke architecture: {other}")),
    };
    let extension = if os == "windows" { ".exe" } else { "" };
    Ok(format!("denju_{os}_{arch}{extension}"))
}

fn safe_command(mut command: Command, home: &Path) -> Command {
    command
        .env("DENJU_TEST_HOME", home)
        .env("HOME", home)
        .env("USERPROFILE", home)
        .env("CODEX_HOME", home.join("poison-codex"))
        .env("CLAUDE_CONFIG_DIR", home.join("poison-claude"));
    command
}

struct SmokeDirectory(PathBuf);

impl SmokeDirectory {
    fn new() -> Result<Self, String> {
        let path = std::env::temp_dir().join(format!("denju-release-smoke-{}", Uuid::now_v7()));
        fs::create_dir(&path).map_err(io_error("create release-smoke directory"))?;
        Ok(Self(path))
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for SmokeDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

struct StaticReleaseServer {
    address: std::net::SocketAddr,
    stop: Arc<AtomicBool>,
    thread: Option<thread::JoinHandle<()>>,
}

struct NpmRegistryServer {
    address: std::net::SocketAddr,
    stop: Arc<AtomicBool>,
    thread: Option<thread::JoinHandle<()>>,
}

impl NpmRegistryServer {
    fn start(tarball: &Path, version: &str) -> Result<Self, String> {
        let tarball = fs::read(tarball).map_err(io_error("read npm smoke tarball"))?;
        let listener = TcpListener::bind("127.0.0.1:0")
            .map_err(|error| format!("bind npm smoke registry: {error}"))?;
        let address = listener
            .local_addr()
            .map_err(|error| format!("read npm smoke registry address: {error}"))?;
        listener
            .set_nonblocking(true)
            .map_err(|error| format!("configure npm smoke registry: {error}"))?;
        let tarball_url = format!("http://{address}/denju-cli/-/denju-cli-{version}.tgz");
        let metadata = serde_json::to_vec(&serde_json::json!({
            "name": "denju-cli",
            "dist-tags": { "latest": version },
            "versions": {
                version: {
                    "name": "denju-cli",
                    "version": version,
                    "bin": { "denju": "bin/denju.js" },
                    "scripts": { "postinstall": "node scripts/postinstall.js" },
                    "dist": { "tarball": tarball_url }
                }
            }
        }))
        .map_err(|error| format!("serialize npm smoke registry metadata: {error}"))?;
        let stop = Arc::new(AtomicBool::new(false));
        let thread_stop = Arc::clone(&stop);
        let version = version.to_owned();
        let handle = thread::spawn(move || {
            while !thread_stop.load(Ordering::Relaxed) {
                match listener.accept() {
                    Ok((stream, _)) => serve_npm_registry(stream, &metadata, &tarball, &version),
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(10));
                    }
                    Err(_) => break,
                }
            }
        });
        Ok(Self {
            address,
            stop,
            thread: Some(handle),
        })
    }

    fn address(&self) -> std::net::SocketAddr {
        self.address
    }
}

impl Drop for NpmRegistryServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        let _ = TcpStream::connect(self.address);
        if let Some(handle) = self.thread.take() {
            let _ = handle.join();
        }
    }
}

impl StaticReleaseServer {
    fn start(root: PathBuf) -> Result<Self, String> {
        let listener = TcpListener::bind("127.0.0.1:0")
            .map_err(|error| format!("bind release-smoke server: {error}"))?;
        let address = listener
            .local_addr()
            .map_err(|error| format!("read release-smoke server address: {error}"))?;
        listener
            .set_nonblocking(true)
            .map_err(|error| format!("configure release-smoke server: {error}"))?;
        let stop = Arc::new(AtomicBool::new(false));
        let thread_stop = Arc::clone(&stop);
        let handle = thread::spawn(move || {
            while !thread_stop.load(Ordering::Relaxed) {
                match listener.accept() {
                    Ok((stream, _)) => serve_file(stream, &root),
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(10));
                    }
                    Err(_) => break,
                }
            }
        });
        Ok(Self {
            address,
            stop,
            thread: Some(handle),
        })
    }

    fn address(&self) -> std::net::SocketAddr {
        self.address
    }
}

impl Drop for StaticReleaseServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        let _ = TcpStream::connect(self.address);
        if let Some(handle) = self.thread.take() {
            let _ = handle.join();
        }
    }
}

fn serve_file(mut stream: TcpStream, root: &Path) {
    let _ = stream.set_nonblocking(false);
    let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
    let _ = stream.set_write_timeout(Some(Duration::from_secs(10)));
    let mut request = [0_u8; 8192];
    let Ok(read) = stream.read(&mut request) else {
        return;
    };
    let first_line = String::from_utf8_lossy(&request[..read])
        .lines()
        .next()
        .unwrap_or_default()
        .to_owned();
    let mut fields = first_line.split_whitespace();
    let method = fields.next().unwrap_or_default();
    let path = fields.next().unwrap_or_default().trim_start_matches('/');
    let allowed = path == "release-manifest.txt" || CLIENT_ASSETS.contains(&path);
    let response = if method == "GET" && allowed {
        fs::read(root.join(path)).ok()
    } else {
        None
    };
    match response {
        Some(bytes) => {
            let _ = write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                bytes.len()
            );
            let _ = stream.write_all(&bytes);
        }
        None => {
            let _ = stream.write_all(
                b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
            );
        }
    }
}

fn serve_npm_registry(mut stream: TcpStream, metadata: &[u8], tarball: &[u8], version: &str) {
    let _ = stream.set_nonblocking(false);
    let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
    let _ = stream.set_write_timeout(Some(Duration::from_secs(10)));
    let mut request = [0_u8; 8192];
    let Ok(read) = stream.read(&mut request) else {
        return;
    };
    let first_line = String::from_utf8_lossy(&request[..read])
        .lines()
        .next()
        .unwrap_or_default()
        .to_owned();
    let mut fields = first_line.split_whitespace();
    let method = fields.next().unwrap_or_default();
    let path = fields.next().unwrap_or_default();
    let tarball_path = format!("/denju-cli/-/denju-cli-{version}.tgz");
    let response = match (method, path) {
        ("GET", "/denju-cli") => Some(("application/json", metadata)),
        ("GET", candidate) if candidate == tarball_path => {
            Some(("application/octet-stream", tarball))
        }
        _ => None,
    };
    match response {
        Some((content_type, bytes)) => {
            let _ = write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                bytes.len()
            );
            let _ = stream.write_all(bytes);
        }
        None => {
            let _ = stream.write_all(
                b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
            );
        }
    }
}

fn io_error(context: &'static str) -> impl FnOnce(std::io::Error) -> String {
    move |error| format!("{context}: {error}")
}
