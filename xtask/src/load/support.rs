use std::{
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    thread,
    time::{Duration, Instant},
};

use denju_registry::RegistrySettings;
use denju_wire::RegistryLimits;
use reqwest::blocking::Client as BlockingClient;
use serde::Serialize;
use url::Url;

use super::{DATABASE, RECOVERY_TOKEN};

#[derive(Debug, Serialize)]
pub(super) struct EnvironmentReport {
    os: String,
    arch: String,
    postgres: String,
    garage: String,
    profile: &'static str,
}

pub(super) fn ensure_infrastructure(root: &Path) -> Result<(), String> {
    let status = Command::new("docker")
        .args([
            "compose",
            "-f",
            "deploy/dev.compose.yml",
            "up",
            "-d",
            "--wait",
            "--wait-timeout",
            "60",
        ])
        .current_dir(root)
        .status()
        .map_err(|error| format!("failed to start dev infrastructure: {error}"))?;
    if !status.success() {
        return Err(format!("docker compose exited with {status}"));
    }
    crate::wait_for_tcp("PostgreSQL", "127.0.0.1:55432".parse().unwrap())?;
    crate::wait_for_tcp("Garage", "127.0.0.1:53900".parse().unwrap())
}

pub(super) fn reset_database(root: &Path) -> Result<(), String> {
    docker_psql(
        root,
        "postgres",
        &format!("DROP DATABASE IF EXISTS {DATABASE} WITH (FORCE);"),
    )?;
    docker_psql(root, "postgres", &format!("CREATE DATABASE {DATABASE};"))?;
    Ok(())
}

pub(super) fn configure_database_roles(root: &Path) -> Result<(), String> {
    docker_psql(
        root,
        DATABASE,
        "ALTER ROLE denju_app PASSWORD 'denju-app-dev-only'; ALTER ROLE denju_worker PASSWORD 'denju-worker-dev-only';",
    )
    .map(|_| ())
}

pub(super) fn registry_settings(port: u16) -> RegistrySettings {
    RegistrySettings {
        database_url: database_url("denju_app", "denju-app-dev-only"),
        database_worker_url: database_url("denju_worker", "denju-worker-dev-only"),
        database_listen_url: Some(database_url("denju_app", "denju-app-dev-only")),
        public_origin: Url::parse(&format!("http://127.0.0.1:{port}/")).expect("load URL"),
        object_store_endpoint: Url::parse("http://127.0.0.1:53900").expect("Garage URL"),
        object_store_bucket: "denju-dev".to_owned(),
        object_store_region: "garage".to_owned(),
        object_store_access_key_id: "GK1234567890ABCDEFGH".to_owned(),
        object_store_secret_access_key:
            "0000000000000000000000000000000000000000000000000000000000000000".to_owned(),
        object_store_force_path_style: true,
        limits: RegistryLimits {
            max_object_bytes: 16 * 1024 * 1024,
            max_release_bytes: 10 * 1024 * 1024,
            namespace_storage_bytes: 512 * 1024 * 1024,
            max_transfer_bytes: 16 * 1024 * 1024,
        },
        gc_grace: Duration::from_secs(86_400),
    }
}

fn server_env(port: u16) -> Vec<(String, String)> {
    vec![
        ("DENJU_BIND".to_owned(), format!("127.0.0.1:{port}")),
        (
            "DENJU_PUBLIC_URL".to_owned(),
            format!("http://127.0.0.1:{port}"),
        ),
        (
            "DENJU_DATABASE_URL".to_owned(),
            database_url("denju_app", "denju-app-dev-only"),
        ),
        (
            "DENJU_DATABASE_WORKER_URL".to_owned(),
            database_url("denju_worker", "denju-worker-dev-only"),
        ),
        (
            "DENJU_DATABASE_DIRECT_URL".to_owned(),
            database_url("denju_app", "denju-app-dev-only"),
        ),
        (
            "DENJU_DATABASE_MIGRATION_URL".to_owned(),
            database_url("denju", "denju-dev-only"),
        ),
        ("DENJU_S3_BUCKET".to_owned(), "denju-dev".to_owned()),
        (
            "DENJU_S3_ENDPOINT".to_owned(),
            "http://127.0.0.1:53900".to_owned(),
        ),
        ("DENJU_S3_REGION".to_owned(), "garage".to_owned()),
        (
            "DENJU_S3_ACCESS_KEY_ID".to_owned(),
            "GK1234567890ABCDEFGH".to_owned(),
        ),
        (
            "DENJU_S3_SECRET_ACCESS_KEY".to_owned(),
            "0000000000000000000000000000000000000000000000000000000000000000".to_owned(),
        ),
        ("DENJU_S3_FORCE_PATH_STYLE".to_owned(), "true".to_owned()),
        ("DENJU_RECOVERY_TOKEN".to_owned(), RECOVERY_TOKEN.to_owned()),
        ("RUST_LOG".to_owned(), "info".to_owned()),
    ]
}

fn database_url(role: &str, password: &str) -> String {
    format!("postgresql://{role}:{password}@127.0.0.1:55432/{DATABASE}")
}

pub(super) fn run_server_subcommand(
    binary: &Path,
    port: u16,
    subcommand: &str,
) -> Result<(), String> {
    let mut command = Command::new(binary);
    command.arg(subcommand);
    apply_env(&mut command, &server_env(port));
    let status = command
        .status()
        .map_err(|error| format!("failed to run {} {subcommand}: {error}", binary.display()))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("denju-server {subcommand} exited with {status}"))
    }
}

pub(super) struct ServerProcess {
    child: Option<Child>,
}

impl ServerProcess {
    pub(super) fn start(binary: &Path, port: u16) -> Result<Self, String> {
        let mut command = Command::new(binary);
        command
            .arg("serve")
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        apply_env(&mut command, &server_env(port));
        let child = command
            .spawn()
            .map_err(|error| format!("failed to start denju-server on {port}: {error}"))?;
        Ok(Self { child: Some(child) })
    }

    pub(super) fn terminate(&mut self) -> Result<(), String> {
        let Some(child) = self.child.as_mut() else {
            return Ok(());
        };
        if child
            .try_wait()
            .map_err(|error| error.to_string())?
            .is_some()
        {
            self.child = None;
            return Ok(());
        }
        #[cfg(unix)]
        {
            let status = Command::new("kill")
                .args(["-TERM", &child.id().to_string()])
                .status()
                .map_err(|error| format!("failed to send SIGTERM: {error}"))?;
            if !status.success() {
                return Err(format!("kill -TERM exited with {status}"));
            }
        }
        #[cfg(not(unix))]
        child.kill().map_err(|error| error.to_string())?;
        wait_child(child, Duration::from_secs(5))?;
        self.child = None;
        Ok(())
    }

    pub(super) fn kill(&mut self) -> Result<(), String> {
        let Some(child) = self.child.as_mut() else {
            return Ok(());
        };
        if child
            .try_wait()
            .map_err(|error| error.to_string())?
            .is_none()
        {
            child.kill().map_err(|error| error.to_string())?;
        }
        wait_child(child, Duration::from_secs(5))?;
        self.child = None;
        Ok(())
    }
}

impl Drop for ServerProcess {
    fn drop(&mut self) {
        let _ = self.kill();
    }
}

fn wait_child(child: &mut Child, duration: Duration) -> Result<(), String> {
    let deadline = Instant::now() + duration;
    while Instant::now() < deadline {
        if child
            .try_wait()
            .map_err(|error| error.to_string())?
            .is_some()
        {
            return Ok(());
        }
        thread::sleep(Duration::from_millis(20));
    }
    child.kill().map_err(|error| error.to_string())?;
    child.wait().map_err(|error| error.to_string())?;
    Err("server did not exit within graceful shutdown bound".to_owned())
}

fn apply_env(command: &mut Command, env: &[(String, String)]) {
    for (key, value) in env {
        command.env(key, value);
    }
}

pub(super) fn wait_ready(client: &BlockingClient, port: u16) -> Result<(), String> {
    let deadline = Instant::now() + Duration::from_secs(15);
    while Instant::now() < deadline {
        if client
            .get(format!("http://127.0.0.1:{port}/health/ready"))
            .send()
            .is_ok_and(|response| response.status().is_success())
        {
            return Ok(());
        }
        thread::sleep(Duration::from_millis(25));
    }
    Err(format!("server on {port} did not become ready"))
}

pub(super) fn docker_psql(root: &Path, database: &str, sql: &str) -> Result<String, String> {
    let output = Command::new("docker")
        .args([
            "compose",
            "-f",
            "deploy/dev.compose.yml",
            "exec",
            "-T",
            "postgres",
            "psql",
            "-U",
            "denju",
            "-d",
            database,
            "-v",
            "ON_ERROR_STOP=1",
            "-At",
            "-c",
            sql,
        ])
        .current_dir(root)
        .output()
        .map_err(|error| format!("failed to run psql: {error}"))?;
    if output.status.success() {
        String::from_utf8(output.stdout).map_err(|error| error.to_string())
    } else {
        Err(String::from_utf8_lossy(&output.stderr).trim().to_owned())
    }
}

pub(super) fn environment_report(root: &Path) -> Result<EnvironmentReport, String> {
    Ok(EnvironmentReport {
        os: std::env::consts::OS.to_owned(),
        arch: std::env::consts::ARCH.to_owned(),
        postgres: docker_psql(root, DATABASE, "SHOW server_version;")?
            .trim()
            .to_owned(),
        garage: command_stdout(
            Command::new("docker")
                .args([
                    "compose",
                    "-f",
                    "deploy/dev.compose.yml",
                    "exec",
                    "-T",
                    "garage",
                    "/garage",
                    "--version",
                ])
                .current_dir(root),
        )?
        .trim()
        .to_owned(),
        profile: "release",
    })
}

fn command_stdout(command: &mut Command) -> Result<String, String> {
    let output = command.output().map_err(|error| error.to_string())?;
    if output.status.success() {
        String::from_utf8(output.stdout).map_err(|error| error.to_string())
    } else {
        Err(String::from_utf8_lossy(&output.stderr).trim().to_owned())
    }
}

pub(super) fn release_binary(name: &str) -> Result<PathBuf, String> {
    let current = std::env::current_exe().map_err(|error| error.to_string())?;
    let target_root = current.parent().and_then(Path::parent).ok_or_else(|| {
        format!(
            "cannot resolve Cargo target root from {}",
            current.display()
        )
    })?;
    let binary = target_root
        .join("release")
        .join(format!("{name}{}", std::env::consts::EXE_SUFFIX));
    if binary.is_file() {
        Ok(binary)
    } else {
        Err(format!(
            "release binary does not exist: {}",
            binary.display()
        ))
    }
}

pub(super) fn p95_ms(values: &mut [Duration]) -> f64 {
    values.sort_unstable();
    let index = ((values.len() as f64 * 0.95).ceil() as usize)
        .saturating_sub(1)
        .min(values.len().saturating_sub(1));
    values[index].as_secs_f64() * 1_000.0
}

pub(super) fn p50_ms(values: &mut [Duration]) -> f64 {
    values.sort_unstable();
    let index = ((values.len() as f64 * 0.50).ceil() as usize)
        .saturating_sub(1)
        .min(values.len().saturating_sub(1));
    values[index].as_secs_f64() * 1_000.0
}

pub(super) fn millis(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

pub(super) fn env_usize(name: &str, default: usize) -> Result<usize, String> {
    match std::env::var(name) {
        Ok(value) => value
            .parse()
            .map_err(|error| format!("invalid {name}: {error}")),
        Err(std::env::VarError::NotPresent) => Ok(default),
        Err(error) => Err(format!("invalid {name}: {error}")),
    }
}
