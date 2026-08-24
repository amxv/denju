use std::{
    path::Path,
    process::Command,
    time::{Duration, Instant},
};

use denju_registry::Registry;
use serde::Serialize;
use url::Url;

use super::{SERVER_ONE_PORT, millis, support::registry_settings};

const CHAOS_PROJECT: &str = "denju-phase17-chaos";
const CHAOS_S3_PORT: u16 = 54_900;
const CHAOS_S3_ADMIN_PORT: u16 = 54_903;

#[derive(Debug, Serialize)]
pub(super) struct ObjectStoreConcurrencyReport {
    pub concurrent_probes: usize,
    pub elapsed_ms: u64,
    pub isolated_restart_recovery_ms: u64,
    pub failure_observed_while_stopped: bool,
}

pub(super) async fn exercise_concurrent_provider(
    registry: &Registry,
    root: &Path,
) -> Result<ObjectStoreConcurrencyReport, String> {
    let started = Instant::now();
    let results = tokio::join!(
        registry.verify_object_store_provider(),
        registry.verify_object_store_provider(),
        registry.verify_object_store_provider(),
        registry.verify_object_store_provider(),
        registry.verify_object_store_provider(),
        registry.verify_object_store_provider(),
        registry.verify_object_store_provider(),
        registry.verify_object_store_provider(),
    );
    for result in [
        results.0, results.1, results.2, results.3, results.4, results.5, results.6, results.7,
    ] {
        result.map_err(|error| format!("concurrent object-store probe failed: {error}"))?;
    }
    let restart = exercise_provider_restart(root).await?;
    Ok(ObjectStoreConcurrencyReport {
        concurrent_probes: 8,
        elapsed_ms: millis(started.elapsed()),
        isolated_restart_recovery_ms: restart.0,
        failure_observed_while_stopped: restart.1,
    })
}

async fn exercise_provider_restart(root: &Path) -> Result<(u64, bool), String> {
    let garage = ChaosGarage::start(root)?;
    let mut settings = registry_settings(SERVER_ONE_PORT);
    settings.object_store_endpoint = Url::parse(&format!("http://127.0.0.1:{CHAOS_S3_PORT}"))
        .map_err(|error| error.to_string())?;
    let registry = Registry::connect(settings)
        .await
        .map_err(|error| format!("connect isolated Garage registry: {error}"))?;
    wait_for_provider(&registry, "initial startup").await?;

    garage.stop()?;
    let failure_observed = match tokio::time::timeout(
        Duration::from_secs(4),
        registry.verify_object_store_provider(),
    )
    .await
    {
        Ok(Ok(())) => false,
        Ok(Err(_)) | Err(_) => true,
    };
    if !failure_observed {
        return Err("object-store probe did not fail while isolated Garage was stopped".to_owned());
    }

    let restarted = Instant::now();
    garage.start_service()?;
    crate::wait_for_tcp(
        "isolated Garage load harness",
        format!("127.0.0.1:{CHAOS_S3_PORT}")
            .parse()
            .map_err(|error| format!("invalid isolated Garage address: {error}"))?,
    )?;
    wait_for_provider(&registry, "restart recovery").await?;
    Ok((millis(restarted.elapsed()), failure_observed))
}

async fn wait_for_provider(registry: &Registry, phase: &str) -> Result<(), String> {
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut last_error = None;
    while Instant::now() < deadline {
        match registry.verify_object_store_provider().await {
            Ok(()) => return Ok(()),
            Err(error) => last_error = Some(error.to_string()),
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    Err(format!(
        "isolated Garage {phase} provider check did not become ready: {}",
        last_error.unwrap_or_else(|| "unknown provider error".to_owned())
    ))
}

struct ChaosGarage {
    root: std::path::PathBuf,
}

impl ChaosGarage {
    fn start(root: &Path) -> Result<Self, String> {
        let garage = Self {
            root: root.to_owned(),
        };
        garage.cleanup()?;
        garage.start_service()?;
        crate::wait_for_tcp(
            "isolated Garage load harness",
            format!("127.0.0.1:{CHAOS_S3_PORT}")
                .parse()
                .map_err(|error| format!("invalid isolated Garage address: {error}"))?,
        )?;
        Ok(garage)
    }

    fn start_service(&self) -> Result<(), String> {
        self.compose(&["up", "-d", "garage"])
    }

    fn stop(&self) -> Result<(), String> {
        self.compose(&["kill", "garage"])
    }

    fn cleanup(&self) -> Result<(), String> {
        self.compose(&["down", "-v", "--remove-orphans"])
    }

    fn compose(&self, trailing: &[&str]) -> Result<(), String> {
        let status = Command::new("docker")
            .args([
                "compose",
                "-p",
                CHAOS_PROJECT,
                "-f",
                "deploy/dev.compose.yml",
            ])
            .args(trailing)
            .current_dir(&self.root)
            .env("DENJU_DEV_S3_PORT", CHAOS_S3_PORT.to_string())
            .env("DENJU_DEV_S3_ADMIN_PORT", CHAOS_S3_ADMIN_PORT.to_string())
            .status()
            .map_err(|error| format!("failed to run isolated Garage Compose command: {error}"))?;
        if status.success() {
            Ok(())
        } else {
            Err(format!(
                "isolated Garage Compose command exited with {status}"
            ))
        }
    }
}

impl Drop for ChaosGarage {
    fn drop(&mut self) {
        let _ = self.cleanup();
    }
}
