use std::{
    fs::{self, File},
    io,
    path::Path,
    process::{Command, Stdio},
};

use thiserror::Error;

use crate::{LocalPaths, ServiceRecord};

#[cfg(target_os = "macos")]
const MAC_LABEL: &str = "xyz.ashray.denju";
#[cfg(target_os = "windows")]
const WINDOWS_TASK: &str = "Denju";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServiceInstallMode {
    Start,
    InstallOnly,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServiceKind {
    LaunchAgent,
    SystemdUser,
    WindowsTask,
    Session,
}

impl ServiceKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::LaunchAgent => "launch_agent",
            Self::SystemdUser => "systemd_user",
            Self::WindowsTask => "windows_task",
            Self::Session => "session",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServiceStatus {
    pub kind: ServiceKind,
    pub persistent: bool,
    pub running: bool,
    pub detail: Option<String>,
}

impl ServiceStatus {
    pub fn to_record(&self) -> ServiceRecord {
        ServiceRecord {
            kind: self.kind.as_str().to_owned(),
            persistent: self.persistent,
            running: self.running,
            detail: self.detail.clone(),
        }
    }
}

pub struct ServiceManager;

impl ServiceManager {
    pub fn install_and_start(
        paths: &LocalPaths,
        executable: &Path,
        mode: ServiceInstallMode,
    ) -> Result<ServiceStatus, ServiceError> {
        #[cfg(target_os = "macos")]
        return install_launch_agent(paths, executable, mode);
        #[cfg(target_os = "linux")]
        return install_systemd_user(paths, executable, mode);
        #[cfg(target_os = "windows")]
        return install_windows_task(paths, executable, mode);
        #[allow(unreachable_code)]
        start_session_daemon(paths, executable, mode)
    }

    pub fn status(paths: &LocalPaths) -> Result<ServiceStatus, ServiceError> {
        #[cfg(target_os = "macos")]
        {
            let domain = format!("gui/{}/{}", current_uid()?, MAC_LABEL);
            let running = Command::new("launchctl")
                .args(["print", &domain])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()?
                .success();
            return Ok(ServiceStatus {
                kind: ServiceKind::LaunchAgent,
                persistent: launch_agent_path(paths).is_file(),
                running,
                detail: None,
            });
        }
        #[cfg(target_os = "linux")]
        {
            if !systemd_user_available() {
                return Ok(ServiceStatus {
                    kind: ServiceKind::Session,
                    persistent: false,
                    running: paths.run.join("daemon.pid").is_file(),
                    detail: Some(
                        "persistent user service manager is unavailable; using current-session daemon"
                            .to_owned(),
                    ),
                });
            }
            let running = Command::new("systemctl")
                .args(["--user", "is-active", "--quiet", "denju.service"])
                .status()
                .is_ok_and(|status| status.success());
            return Ok(ServiceStatus {
                kind: ServiceKind::SystemdUser,
                persistent: systemd_unit_path(paths).is_file(),
                running,
                detail: None,
            });
        }
        #[cfg(target_os = "windows")]
        {
            let persistent = Command::new("schtasks")
                .args(["/Query", "/TN", WINDOWS_TASK])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()?
                .success();
            return Ok(ServiceStatus {
                kind: ServiceKind::WindowsTask,
                persistent,
                running: paths.run.join("daemon.pid").is_file(),
                detail: None,
            });
        }
        #[allow(unreachable_code)]
        Ok(ServiceStatus {
            kind: ServiceKind::Session,
            persistent: false,
            running: paths.run.join("daemon.pid").is_file(),
            detail: Some("persistent per-user service manager is unavailable".to_owned()),
        })
    }
}

#[cfg(target_os = "macos")]
fn install_launch_agent(
    paths: &LocalPaths,
    executable: &Path,
    mode: ServiceInstallMode,
) -> Result<ServiceStatus, ServiceError> {
    let plist_path = launch_agent_path(paths);
    if let Some(parent) = plist_path.parent() {
        fs::create_dir_all(parent)?;
    }
    let plist = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
         <!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n\
         <plist version=\"1.0\"><dict>\n\
         <key>Label</key><string>{MAC_LABEL}</string>\n\
         <key>ProgramArguments</key><array><string>{}</string><string>daemon</string></array>\n\
         <key>EnvironmentVariables</key><dict><key>HOME</key><string>{}</string></dict>\n\
         <key>RunAtLoad</key><true/><key>KeepAlive</key><true/>\n\
         <key>StandardOutPath</key><string>{}</string>\n\
         <key>StandardErrorPath</key><string>{}</string>\n\
         </dict></plist>\n",
        xml_escape(&executable.to_string_lossy()),
        xml_escape(&paths.home.to_string_lossy()),
        xml_escape(&paths.logs.join("daemon.log").to_string_lossy()),
        xml_escape(&paths.logs.join("daemon.err.log").to_string_lossy()),
    );
    fs::write(&plist_path, plist)?;
    if mode == ServiceInstallMode::InstallOnly {
        return Ok(ServiceStatus {
            kind: ServiceKind::LaunchAgent,
            persistent: true,
            running: false,
            detail: Some("installed without starting (test mode)".to_owned()),
        });
    }

    let domain = format!("gui/{}", current_uid()?);
    let service = format!("{domain}/{MAC_LABEL}");
    let _ = Command::new("launchctl")
        .args(["bootout", &service])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    run_checked(
        Command::new("launchctl")
            .arg("bootstrap")
            .arg(&domain)
            .arg(&plist_path),
        "launchctl bootstrap",
    )?;
    run_checked(
        Command::new("launchctl").args(["kickstart", "-k", &service]),
        "launchctl kickstart",
    )?;
    Ok(ServiceStatus {
        kind: ServiceKind::LaunchAgent,
        persistent: true,
        running: true,
        detail: None,
    })
}

#[cfg(target_os = "linux")]
fn install_systemd_user(
    paths: &LocalPaths,
    executable: &Path,
    mode: ServiceInstallMode,
) -> Result<ServiceStatus, ServiceError> {
    if !systemd_user_available() {
        return start_session_daemon(paths, executable, mode);
    }

    let unit_path = systemd_unit_path(paths);
    if let Some(parent) = unit_path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(
        &unit_path,
        format!(
            "[Unit]\nDescription=Denju background synchronization\n\n[Service]\nType=simple\nExecStart={} daemon\nEnvironment=HOME={}\nRestart=on-failure\n\n[Install]\nWantedBy=default.target\n",
            systemd_escape(executable),
            systemd_escape(&paths.home),
        ),
    )?;
    if mode == ServiceInstallMode::InstallOnly {
        return Ok(ServiceStatus {
            kind: ServiceKind::SystemdUser,
            persistent: true,
            running: false,
            detail: Some("installed without starting (test mode)".to_owned()),
        });
    }
    run_checked(
        Command::new("systemctl").args(["--user", "daemon-reload"]),
        "systemctl --user daemon-reload",
    )?;
    run_checked(
        Command::new("systemctl").args(["--user", "enable", "--now", "denju.service"]),
        "systemctl --user enable --now denju.service",
    )?;
    Ok(ServiceStatus {
        kind: ServiceKind::SystemdUser,
        persistent: true,
        running: true,
        detail: None,
    })
}

#[cfg(target_os = "linux")]
fn systemd_user_available() -> bool {
    Command::new("systemctl")
        .args(["--user", "show-environment"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

#[cfg(target_os = "windows")]
fn install_windows_task(
    paths: &LocalPaths,
    executable: &Path,
    mode: ServiceInstallMode,
) -> Result<ServiceStatus, ServiceError> {
    let command = format!("\"{}\" daemon", executable.display());
    run_checked(
        Command::new("schtasks").args([
            "/Create",
            "/F",
            "/SC",
            "ONLOGON",
            "/TN",
            WINDOWS_TASK,
            "/TR",
            &command,
        ]),
        "schtasks /Create",
    )?;
    if mode == ServiceInstallMode::InstallOnly {
        return Ok(ServiceStatus {
            kind: ServiceKind::WindowsTask,
            persistent: true,
            running: false,
            detail: Some("installed without starting (test mode)".to_owned()),
        });
    }
    run_checked(
        Command::new("schtasks").args(["/Run", "/TN", WINDOWS_TASK]),
        "schtasks /Run",
    )?;
    let _ = paths;
    Ok(ServiceStatus {
        kind: ServiceKind::WindowsTask,
        persistent: true,
        running: true,
        detail: None,
    })
}

fn start_session_daemon(
    paths: &LocalPaths,
    executable: &Path,
    mode: ServiceInstallMode,
) -> Result<ServiceStatus, ServiceError> {
    if mode == ServiceInstallMode::InstallOnly {
        return Ok(ServiceStatus {
            kind: ServiceKind::Session,
            persistent: false,
            running: false,
            detail: Some(
                "persistent user service unavailable; session daemon not started in test mode"
                    .to_owned(),
            ),
        });
    }
    fs::create_dir_all(&paths.logs)?;
    let stdout = File::create(paths.logs.join("daemon.log"))?;
    let stderr = File::create(paths.logs.join("daemon.err.log"))?;
    Command::new(executable)
        .arg("daemon")
        .env("HOME", &paths.home)
        .stdin(Stdio::null())
        .stdout(stdout)
        .stderr(stderr)
        .spawn()?;
    Ok(ServiceStatus {
        kind: ServiceKind::Session,
        persistent: false,
        running: true,
        detail: Some(
            "persistent user service unavailable; running for the current session".to_owned(),
        ),
    })
}

fn run_checked(command: &mut Command, name: &str) -> Result<(), ServiceError> {
    let status = command.status()?;
    if status.success() {
        Ok(())
    } else {
        Err(ServiceError::CommandFailed {
            command: name.to_owned(),
            status: status.to_string(),
        })
    }
}

#[cfg(target_os = "macos")]
fn current_uid() -> Result<String, ServiceError> {
    let output = Command::new("id").arg("-u").output()?;
    if !output.status.success() {
        return Err(ServiceError::CommandFailed {
            command: "id -u".to_owned(),
            status: output.status.to_string(),
        });
    }
    Ok(String::from_utf8(output.stdout)?.trim().to_owned())
}

#[cfg(target_os = "macos")]
fn launch_agent_path(paths: &LocalPaths) -> std::path::PathBuf {
    paths
        .home
        .join("Library/LaunchAgents")
        .join(format!("{MAC_LABEL}.plist"))
}

#[cfg(target_os = "linux")]
fn systemd_unit_path(paths: &LocalPaths) -> std::path::PathBuf {
    paths.home.join(".config/systemd/user/denju.service")
}

#[cfg(target_os = "macos")]
fn xml_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

#[cfg(target_os = "linux")]
fn systemd_escape(path: &Path) -> String {
    path.to_string_lossy().replace(' ', "\\x20")
}

#[derive(Debug, Error)]
pub enum ServiceError {
    #[error("service filesystem/process error: {0}")]
    Io(#[from] io::Error),
    #[error("service command {command} failed with {status}")]
    CommandFailed { command: String, status: String },
    #[error("service command returned invalid UTF-8: {0}")]
    Utf8(#[from] std::string::FromUtf8Error),
}

#[cfg(test)]
mod tests {
    use tempfile::tempdir;

    use super::*;

    #[test]
    fn install_only_writes_platform_service_definition_without_starting() {
        let home = tempdir().unwrap();
        let paths = LocalPaths::from_home(home.path().to_owned());
        fs::create_dir_all(&paths.logs).unwrap();
        let status = ServiceManager::install_and_start(
            &paths,
            Path::new("/tmp/denju"),
            ServiceInstallMode::InstallOnly,
        )
        .unwrap();
        assert!(!status.running);
        #[cfg(target_os = "macos")]
        assert!(launch_agent_path(&paths).is_file());
        #[cfg(target_os = "linux")]
        if status.kind == ServiceKind::SystemdUser {
            assert!(systemd_unit_path(&paths).is_file());
        }
    }
}
