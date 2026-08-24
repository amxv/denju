use std::{
    fs,
    io::Write,
    net::{Ipv4Addr, TcpListener},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};

use denju_wire::{
    CreateInstallationRequest, SubscriptionCatalog, SubscriptionMutationKind,
    SubscriptionMutationRequest, SubscriptionTarget, create_installation_request_hash,
    subscription_request_hash,
};
use rand::RngExt;
use reqwest::blocking::Client;
use serde_json::Value;
use sha2::{Digest, Sha256};
use tempfile::TempDir;
use url::Url;
use uuid::Uuid;

const DEFAULT_IMAGE: &str = "denju-server:release-smoke";

pub(crate) fn run(root: &Path) -> Result<(), String> {
    let image = parse_image()?;
    let port = reserve_loopback_port()?;
    let s3_port = reserve_distinct_loopback_port(port)?;
    let project = format!("denju-self-host-smoke-{}", Uuid::now_v7().simple());
    let temporary = tempfile::tempdir().map_err(io_error("create self-host smoke directory"))?;
    let recovery_token = secret_hex(32);
    let env = SmokeEnvironment::write(temporary, port, s3_port, &image, &recovery_token)?;
    let stack = SmokeStack::new(root, env.path(), project);

    stack.run(&["config", "--quiet"])?;
    stack.run(&["up", "-d", "--wait"])?;
    wait_for_ready(port)?;
    assert_capabilities(port)?;

    stack.run(&["run", "--rm", "--no-deps", "server", "check-object-store"])?;
    assert_client_facing_presigned_download(&stack, env.root(), port, s3_port)?;
    assert_recovery(port, &recovery_token)?;

    stack.run(&["restart", "server"])?;
    wait_for_ready(port)?;
    stack.run(&["run", "--rm", "--no-deps", "migrate"])?;
    wait_for_ready(port)?;

    println!(
        "self-host smoke: empty start, provider, client presign, recovery, restart, and migration passed"
    );
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

fn reserve_distinct_loopback_port(other: u16) -> Result<u16, String> {
    loop {
        let port = reserve_loopback_port()?;
        if port != other {
            return Ok(port);
        }
    }
}

struct SmokeEnvironment {
    _temporary: TempDir,
    path: PathBuf,
}

impl SmokeEnvironment {
    fn write(
        temporary: TempDir,
        port: u16,
        s3_port: u16,
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
            ("DENJU_S3_PORT", s3_port.to_string()),
            (
                "DENJU_S3_PRESIGN_ENDPOINT",
                format!("http://127.0.0.1:{s3_port}"),
            ),
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

    fn root(&self) -> &Path {
        self._temporary.path()
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

    fn seed_public(&self, owner: &str, fixture: &Path) -> Result<(), String> {
        let fixture_name = fixture
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| "self-host public fixture must have a UTF-8 basename".to_owned())?;
        let container_path = format!("/fixtures/{fixture_name}");
        let mount = format!("{}:{container_path}:ro", fixture.display());
        eprintln!("+ docker compose run --rm --no-deps -v <fixture> server seed-public");
        let status = self
            .command()
            .args(["run", "--rm", "--no-deps", "-v"])
            .arg(mount)
            .args([
                "server",
                "seed-public",
                "--owner",
                owner,
                "--path",
                &container_path,
            ])
            .status()
            .map_err(|error| format!("failed to seed self-host public fixture: {error}"))?;
        if status.success() {
            Ok(())
        } else {
            Err(format!("self-host public seed exited with {status}"))
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

fn assert_client_facing_presigned_download(
    stack: &SmokeStack<'_>,
    temporary: &Path,
    port: u16,
    s3_port: u16,
) -> Result<(), String> {
    const OWNER: &str = "self-host-smoke";
    const NAME: &str = "presign-smoke";
    let fixture = temporary.join(NAME);
    fs::create_dir(&fixture).map_err(io_error("create self-host presign fixture"))?;
    fs::write(
        fixture.join("SKILL.md"),
        b"---\nname: presign-smoke\ndescription: Self-host client presign regression fixture.\n---\n# Presign smoke\n",
    )
    .map_err(io_error("write self-host presign fixture"))?;
    stack.seed_public(OWNER, &fixture)?;

    let origin = format!("http://127.0.0.1:{port}");
    let client = smoke_client()?;
    let bearer = secret_hex(32);
    let raw_bearer = hex::decode(&bearer).map_err(|error| error.to_string())?;
    let credential_hash = hex::encode(Sha256::digest(&raw_bearer));
    let operation_id = Uuid::now_v7().to_string();
    let request_hash = create_installation_request_hash(&operation_id, &credential_hash)
        .map_err(|error| error.to_string())?;
    client
        .post(format!("{origin}/v1/installations"))
        .json(&CreateInstallationRequest {
            operation_id,
            credential_hash,
            request_hash: request_hash.to_string(),
        })
        .send()
        .map_err(http_error("create self-host presign installation"))?
        .error_for_status()
        .map_err(http_error("validate self-host presign installation"))?;

    let locator = format!("@{OWNER}/{NAME}");
    let target: SubscriptionTarget = client
        .get(format!("{origin}/v1/subscriptions/resolve"))
        .bearer_auth(&bearer)
        .query(&[("locator", &locator)])
        .send()
        .map_err(http_error("resolve self-host presign subscription"))?
        .error_for_status()
        .map_err(http_error("validate self-host presign subscription target"))?
        .json()
        .map_err(http_error("decode self-host presign subscription target"))?;
    let operation_id = Uuid::now_v7().to_string();
    let request_hash = subscription_request_hash(
        SubscriptionMutationKind::Subscribe,
        &operation_id,
        &target.resource_id,
        target.generation,
        None,
        false,
    )
    .map_err(|error| error.to_string())?;
    client
        .post(format!("{origin}/v1/subscriptions"))
        .bearer_auth(&bearer)
        .json(&SubscriptionMutationRequest {
            operation_id,
            resource_id: target.resource_id,
            expected_generation: target.generation,
            release_version: None,
            retain_on_delete: false,
            request_hash: request_hash.to_string(),
        })
        .send()
        .map_err(http_error("create self-host presign subscription"))?
        .error_for_status()
        .map_err(http_error("validate self-host presign subscription"))?;
    let catalog: SubscriptionCatalog = client
        .get(format!("{origin}/v1/subscriptions"))
        .bearer_auth(&bearer)
        .send()
        .map_err(http_error("read self-host presign catalog"))?
        .error_for_status()
        .map_err(http_error("validate self-host presign catalog"))?
        .json()
        .map_err(http_error("decode self-host presign catalog"))?;
    let skill = catalog
        .skills
        .iter()
        .find(|skill| skill.locator == locator)
        .ok_or_else(|| "self-host presign subscription missing from catalog".to_owned())?;
    let download = Url::parse(&skill.snapshot.url)
        .map_err(|error| format!("invalid self-host presigned download URL: {error}"))?;
    if download.host_str() != Some("127.0.0.1") || download.port() != Some(s3_port) {
        return Err(format!(
            "self-host product presign leaked a non-client endpoint: {}",
            download.origin().ascii_serialization()
        ));
    }
    let bytes = client
        .get(download)
        .send()
        .map_err(http_error("download self-host presigned snapshot"))?
        .error_for_status()
        .map_err(http_error("validate self-host presigned snapshot"))?
        .bytes()
        .map_err(http_error("read self-host presigned snapshot"))?;
    if bytes.len() as u64 != skill.snapshot.size_bytes
        || hex::encode(Sha256::digest(&bytes)) != skill.snapshot.sha256
    {
        return Err("self-host presigned snapshot bytes did not match the catalog".to_owned());
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
        .no_proxy()
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
