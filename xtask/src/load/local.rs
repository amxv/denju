use std::{
    fs,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    thread,
    time::{Duration, Instant},
};

use serde::Serialize;
use serde_json::Value;

use super::support::{p50_ms, p95_ms};

#[derive(Debug, Serialize)]
pub(super) struct CliLatencyReport {
    pub p50_ms: f64,
    pub p95_ms: f64,
}

#[derive(Debug, Serialize)]
pub(super) struct DaemonRuntimeReport {
    watcher_mode: String,
    iterations: u64,
    full_hash_scans_total: u64,
    capture_errors_total: u64,
    remote_sync_errors_total: u64,
    resident_memory_kib: u64,
}

pub(super) struct IsolatedCliHome {
    path: PathBuf,
}

impl IsolatedCliHome {
    pub(super) fn create(root: &Path) -> Result<Self, String> {
        let path = root
            .join("tmp/gg/denju-rust-greenfield")
            .join(format!("phase17-cli-home-{}", std::process::id()));
        if path.exists() {
            fs::remove_dir_all(&path).map_err(|error| error.to_string())?;
        }
        fs::create_dir_all(&path).map_err(|error| error.to_string())?;
        fs::write(path.join(".denju-test-home-v1"), b"isolated\n")
            .map_err(|error| error.to_string())?;
        Ok(Self { path })
    }
}

impl Drop for IsolatedCliHome {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

fn cli_command(binary: &Path, home: &IsolatedCliHome) -> Command {
    let mut command = Command::new(binary);
    command
        .env("DENJU_TEST_HOME", &home.path)
        .env("CODEX_HOME", "/protected-real-codex-root-must-not-be-used")
        .env(
            "CLAUDE_CONFIG_DIR",
            "/protected-real-claude-root-must-not-be-used",
        )
        .env_remove("DENJU_TEST_FILE_CREDENTIALS")
        .env_remove("DENJU_TEST_SERVICE_INSTALL_ONLY")
        .env_remove("DENJU_DAEMON_ONCE")
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    command
}

pub(super) fn run_cli_setup(
    binary: &Path,
    home: &IsolatedCliHome,
    port: u16,
) -> Result<(), String> {
    let status = cli_command(binary, home)
        .args(["setup", "--registry", &format!("http://127.0.0.1:{port}")])
        .status()
        .map_err(|error| format!("failed to run isolated denju setup: {error}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("isolated denju setup exited with {status}"))
    }
}

pub(super) fn benchmark_cli(
    binary: &Path,
    home: &IsolatedCliHome,
    args: &[&str],
    samples: usize,
) -> Result<CliLatencyReport, String> {
    for _ in 0..5 {
        let status = cli_command(binary, home)
            .args(args)
            .status()
            .map_err(|error| error.to_string())?;
        if !status.success() {
            return Err(format!("CLI warmup `{}` failed", args.join(" ")));
        }
    }
    let mut durations = Vec::with_capacity(samples);
    for _ in 0..samples {
        let started = Instant::now();
        let status = cli_command(binary, home)
            .args(args)
            .status()
            .map_err(|error| error.to_string())?;
        if !status.success() {
            return Err(format!("CLI benchmark `{}` failed", args.join(" ")));
        }
        durations.push(started.elapsed());
    }
    let p50_ms = p50_ms(&mut durations);
    let p95_ms = p95_ms(&mut durations);
    Ok(CliLatencyReport { p50_ms, p95_ms })
}

pub(super) fn exercise_daemon_runtime(
    binary: &Path,
    home: &IsolatedCliHome,
) -> Result<DaemonRuntimeReport, String> {
    let mut command = cli_command(binary, home);
    command.args(["daemon"]);
    let mut child = command
        .spawn()
        .map_err(|error| format!("failed to start isolated Denju daemon: {error}"))?;
    let metrics_path = home.path.join(".denju/run/daemon.metrics.json");
    let deadline = Instant::now() + Duration::from_secs(8);
    let initial = loop {
        if let Some(status) = child.try_wait().map_err(|error| error.to_string())? {
            return Err(format!(
                "isolated Denju daemon exited before health metrics were available: {status}"
            ));
        }
        if let Ok(bytes) = fs::read(&metrics_path)
            && let Ok(value) = serde_json::from_slice::<Value>(&bytes)
        {
            break value;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err("isolated Denju daemon did not publish health metrics".to_owned());
        }
        thread::sleep(Duration::from_millis(25));
    };

    let churn = home.path.join(".denju/generations/phase17-watcher-churn");
    fs::create_dir_all(&churn).map_err(|error| error.to_string())?;
    for index in 0..256 {
        let path = churn.join(format!("event-{index:04}.tmp"));
        fs::write(&path, format!("{index}\n")).map_err(|error| error.to_string())?;
        fs::remove_file(path).map_err(|error| error.to_string())?;
    }
    fs::remove_dir_all(&churn).map_err(|error| error.to_string())?;

    let initial_iterations = initial["iterations"].as_u64().unwrap_or(0);
    let update_deadline = Instant::now() + Duration::from_secs(8);
    let final_metrics = loop {
        if let Ok(bytes) = fs::read(&metrics_path)
            && let Ok(value) = serde_json::from_slice::<Value>(&bytes)
            && value["iterations"].as_u64().unwrap_or(0) > initial_iterations
        {
            break value;
        }
        if Instant::now() >= update_deadline {
            let _ = stop_daemon_child(&mut child);
            return Err("daemon watcher churn did not produce another health iteration".to_owned());
        }
        thread::sleep(Duration::from_millis(25));
    };

    let resident_memory_kib = process_resident_memory_kib(child.id())?;
    stop_daemon_child(&mut child)?;
    Ok(DaemonRuntimeReport {
        watcher_mode: final_metrics["watcher_mode"]
            .as_str()
            .unwrap_or("unknown")
            .to_owned(),
        iterations: final_metrics["iterations"].as_u64().unwrap_or(0),
        full_hash_scans_total: final_metrics["full_hash_scans_total"].as_u64().unwrap_or(0),
        capture_errors_total: final_metrics["capture_errors_total"].as_u64().unwrap_or(0),
        remote_sync_errors_total: final_metrics["remote_sync_errors_total"]
            .as_u64()
            .unwrap_or(0),
        resident_memory_kib,
    })
}

fn process_resident_memory_kib(pid: u32) -> Result<u64, String> {
    let output = Command::new("ps")
        .args(["-o", "rss=", "-p", &pid.to_string()])
        .output()
        .map_err(|error| format!("failed to inspect daemon memory: {error}"))?;
    if !output.status.success() {
        return Err(format!("ps exited with {}", output.status));
    }
    String::from_utf8(output.stdout)
        .map_err(|error| error.to_string())?
        .trim()
        .parse::<u64>()
        .map_err(|error| format!("invalid daemon RSS value: {error}"))
}

fn stop_daemon_child(child: &mut Child) -> Result<(), String> {
    #[cfg(unix)]
    {
        let status = Command::new("kill")
            .args(["-INT", &child.id().to_string()])
            .status()
            .map_err(|error| format!("failed to stop isolated daemon: {error}"))?;
        if !status.success() {
            return Err(format!("kill -INT exited with {status}"));
        }
    }
    #[cfg(not(unix))]
    child.kill().map_err(|error| error.to_string())?;

    let deadline = Instant::now() + Duration::from_secs(5);
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
    Err("isolated daemon did not exit after interrupt".to_owned())
}
