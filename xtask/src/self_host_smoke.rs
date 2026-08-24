use std::{
    fs,
    io::Write,
    net::{Ipv4Addr, TcpListener},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};

use rand::RngExt;
use reqwest::blocking::Client;
use serde_json::Value;
use tempfile::TempDir;
use uuid::Uuid;

const DEFAULT_IMAGE: &str = "denju-server:release-smoke";

pub(crate) fn run(root: &Path) -> Result<(), String> {
    let image = parse_image()?;
    let port = reserve_loopback_port()?;
    let project = format!("denju-self-host-smoke-{}", Uuid::now_v7().simple());
    let temporary = tempfile::tempdir().map_err(io_error("create self-host smoke directory"))?;
    let recovery_token = secret_hex(32);
    let env = SmokeEnvironment::write(temporary, port, &image, &recovery_token)?;
    let stack = SmokeStack::new(root, env.path(), project);

    stack.run(&["config", "--quiet"])?;
    stack.run(&["up", "-d", "--wait"])?;
    wait_for_ready(port)?;
    assert_capabilities(port)?;

    stack.run(&["run", "--rm", "--no-deps", "server", "check-object-store"])?;
    assert_recovery(port, &recovery_token)?;

    stack.run(&["restart", "server"])?;
    wait_for_ready(port)?;
    stack.run(&["run", "--rm", "--no-deps", "migrate"])?;
    wait_for_ready(port)?;

    println!("self-host smoke: empty start, provider, recovery, restart, and migration passed");
    Ok(())
}

fn parse_image() -> Result<String, String> {
    let mut args = std::env::args().skip(2);
    let mut image = DEFAULT_IMAGE.to_owned();
    while let Some(argument) = args.next() {
        match argument.as_str() {
            "--image" => {
                image = args
                    .next()
                    .filter(|value| !value.is_empty())
                    .ok_or_else(|| "self-host-smoke --image requires a value".to_owned())?;
            }
            other => return Err(format!("unknown self-host-smoke option: {other}")),
        }
    }
    Ok(image)
}

fn reserve_loopback_port() -> Result<u16, String> {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
        .map_err(io_error("reserve self-host smoke port"))?;
    listener
        .local_addr()
        .map(|address| address.port())
        .map_err(io_error("read self-host smoke port"))
}

struct SmokeEnvironment {
    _temporary: TempDir,
    path: PathBuf,
}

impl SmokeEnvironment {
    fn write(
        temporary: TempDir,
        port: u16,
        image: &str,
        recovery_token: &str,
    ) -> Result<Self, String> {
        let path = temporary.path().join("self-host.env");
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .map_err(io_error("create self-host smoke environment"))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&path, fs::Permissions::from_mode(0o600))
                .map_err(io_error("protect self-host smoke environment"))?;
        }
        let values = [
            ("DENJU_DB_OWNER_PASSWORD", secret_hex(24)),
            ("DENJU_DB_APP_PASSWORD", secret_hex(24)),
            ("DENJU_DB_WORKER_PASSWORD", secret_hex(24)),
            ("DENJU_GARAGE_RPC_SECRET", secret_hex(32)),
            (
                "DENJU_S3_ACCESS_KEY_ID",
                format!("GK{}", &secret_hex(16)[..20]),
            ),
            ("DENJU_S3_SECRET_ACCESS_KEY", secret_hex(32)),
            ("DENJU_S3_BUCKET", "denju-self-host-smoke".to_owned()),
            ("DENJU_RECOVERY_TOKEN", recovery_token.to_owned()),
            ("DENJU_PUBLIC_URL", format!("http://127.0.0.1:{port}")),
            ("DENJU_PORT", port.to_string()),
            ("DENJU_SERVER_IMAGE", image.to_owned()),
        ];
        for (key, value) in values {
            validate_env_value(key, &value)?;
            writeln!(file, "{key}={value}")
                .map_err(io_error("write self-host smoke environment"))?;
        }
        file.flush()
            .map_err(io_error("flush self-host smoke environment"))?;
        Ok(Self {
            _temporary: temporary,
            path,
        })
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

fn validate_env_value(key: &str, value: &str) -> Result<(), String> {
    if value.is_empty()
        || value
            .bytes()
            .any(|byte| matches!(byte, b'\n' | b'\r' | b'='))
    {
        Err(format!("invalid generated self-host smoke value for {key}"))
    } else {
        Ok(())
    }
}

struct SmokeStack<'a> {
    root: &'a Path,
    env_file: &'a Path,
    project: String,
}

impl<'a> SmokeStack<'a> {
    fn new(root: &'a Path, env_file: &'a Path, project: String) -> Self {
        Self {
            root,
            env_file,
            project,
        }
    }

    fn command(&self) -> Command {
        let mut command = Command::new("docker");
        command
            .args(["compose", "--env-file"])
            .arg(self.env_file)
            .args(["-p", &self.project, "-f", "deploy/compose.yml"])
            .current_dir(self.root);
        command
    }

    fn run(&self, args: &[&str]) -> Result<(), String> {
        eprintln!("+ docker compose {}", args.join(" "));
        let status = self
            .command()
            .args(args)
            .status()
            .map_err(|error| format!("failed to run self-host docker compose: {error}"))?;
        if status.success() {
            Ok(())
        } else {
            Err(format!("self-host docker compose exited with {status}"))
        }
    }

    fn cleanup(&self) {
        let _ = self
            .command()
            .args(["down", "-v", "--remove-orphans"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
}

impl Drop for SmokeStack<'_> {
    fn drop(&mut self) {
        self.cleanup();
    }
}

fn wait_for_ready(port: u16) -> Result<(), String> {
    let client = smoke_client()?;
    let url = format!("http://127.0.0.1:{port}/health/ready");
    let deadline = Instant::now() + Duration::from_secs(60);
    while Instant::now() < deadline {
        if client
            .get(&url)
            .send()
            .is_ok_and(|response| response.status().is_success())
        {
            return Ok(());
        }
        thread::sleep(Duration::from_millis(250));
    }
    Err(format!("self-host registry did not become ready at {url}"))
}

fn assert_capabilities(port: u16) -> Result<(), String> {
    let response: Value = smoke_client()?
        .get(format!("http://127.0.0.1:{port}/v1/capabilities"))
        .send()
        .map_err(http_error("request self-host capabilities"))?
        .error_for_status()
        .map_err(http_error("validate self-host capabilities status"))?
        .json()
        .map_err(http_error("decode self-host capabilities"))?;
    if response.get("api_version").and_then(Value::as_str) != Some("v1")
        || response
            .get("object_store_required")
            .and_then(Value::as_bool)
            != Some(true)
    {
        return Err("self-host capabilities did not preserve the registry contract".to_owned());
    }
    Ok(())
}

fn assert_recovery(port: u16, token: &str) -> Result<(), String> {
    let client = smoke_client()?;
    let url = format!("http://127.0.0.1:{port}/v1/internal/recover");
    let unauthenticated = client
        .get(&url)
        .send()
        .map_err(http_error("request unauthenticated self-host recovery"))?;
    if unauthenticated.status().is_success() {
        return Err("self-host recovery endpoint accepted a missing bearer".to_owned());
    }
    for _ in 0..2 {
        let response: Value = client
            .get(&url)
            .bearer_auth(token)
            .send()
            .map_err(http_error("request authenticated self-host recovery"))?
            .error_for_status()
            .map_err(http_error("validate self-host recovery status"))?
            .json()
            .map_err(http_error("decode self-host recovery response"))?;
        if response
            .get("outbox_dispatched")
            .and_then(Value::as_u64)
            .is_none()
            || response
                .get("pack_revisions_processed")
                .and_then(Value::as_u64)
                .is_none()
        {
            return Err("self-host recovery response is missing bounded drain counters".to_owned());
        }
    }
    Ok(())
}

fn smoke_client() -> Result<Client, String> {
    Client::builder()
        .connect_timeout(Duration::from_secs(2))
        .timeout(Duration::from_secs(10))
        .build()
        .map_err(|error| format!("build self-host smoke HTTP client: {error}"))
}

fn secret_hex(bytes: usize) -> String {
    let mut buffer = vec![0_u8; bytes];
    rand::rng().fill(&mut buffer[..]);
    hex::encode(buffer)
}

fn io_error(context: &'static str) -> impl FnOnce(std::io::Error) -> String {
    move |error| format!("{context}: {error}")
}

fn http_error(context: &'static str) -> impl FnOnce(reqwest::Error) -> String {
    move |error| format!("{context}: {error}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_environment_values_are_compose_safe() {
        for _ in 0..32 {
            let value = secret_hex(32);
            validate_env_value("TEST", &value).unwrap();
            assert_eq!(value.len(), 64);
        }
        assert!(validate_env_value("TEST", "line\nbreak").is_err());
        assert!(validate_env_value("TEST", "left=right").is_err());
    }
}
