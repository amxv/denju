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
    let existing_plist = fs::read_to_string(&plist_path).ok();
    let definition_unchanged = existing_plist.as_deref() == Some(plist.as_str());
    fs::write(&plist_path, &plist)?;
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
    let loaded_program = launch_agent_program(&service)?;

    // Package upgrades replace the native executable in-place. If the loaded LaunchAgent already
    // has the exact definition we just wrote, bootstrapping it again is unnecessary and can race
    // launchd's asynchronous teardown with `bootout` (commonly surfacing as bootstrap exit 5).
    // A kickstart is enough to make launchd exec the newly installed bytes at the same path.
    let can_restart_in_place = can_restart_launch_agent_in_place(
        definition_unchanged,
        loaded_program.as_deref(),
        executable,
    );
    if !can_restart_in_place {
        if loaded_program.is_some() {
            bootout_launch_agent(&service)?;
            wait_for_launch_agent_exit(&service)?;
        }
        bootstrap_launch_agent(&domain, &plist_path)?;
    }
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

#[cfg(target_os = "macos")]
fn can_restart_launch_agent_in_place(
    definition_unchanged: bool,
    loaded_program: Option<&str>,
    executable: &Path,
) -> bool {
    definition_unchanged && loaded_program.is_some_and(|program| Path::new(program) == executable)
}

#[cfg(target_os = "macos")]
fn launch_agent_program(service: &str) -> Result<Option<String>, ServiceError> {
    let output = Command::new("launchctl")
        .args(["print", service])
        .output()?;
    if !output.status.success() {
        return Ok(None);
    }
    let stdout = String::from_utf8(output.stdout)?;
    Ok(parse_launch_agent_program(&stdout).map(str::to_owned))
}

#[cfg(target_os = "macos")]
fn parse_launch_agent_program(output: &str) -> Option<&str> {
    output
        .lines()
        .map(str::trim)
        .find_map(|line| line.strip_prefix("program = "))
}

#[cfg(target_os = "macos")]
fn bootout_launch_agent(service: &str) -> Result<(), ServiceError> {
    let output = Command::new("launchctl")
        .args(["bootout", service])
        .output()?;
    if output.status.success() || launch_agent_program(service)?.is_none() {
        Ok(())
    } else {
        Err(command_output_error("launchctl bootout", &output))
    }
}

#[cfg(target_os = "macos")]
fn wait_for_launch_agent_exit(service: &str) -> Result<(), ServiceError> {
    for _ in 0..40 {
        if launch_agent_program(service)?.is_none() {
            return Ok(());
        }
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
    Err(ServiceError::CommandFailed {
        command: "launchctl bootout".to_owned(),
        status: "LaunchAgent remained loaded after 1 second".to_owned(),
    })
}

#[cfg(target_os = "macos")]
fn bootstrap_launch_agent(domain: &str, plist_path: &Path) -> Result<(), ServiceError> {
    let mut last_error = None;
    for attempt in 0..5 {
        let output = Command::new("launchctl")
            .arg("bootstrap")
            .arg(domain)
            .arg(plist_path)
            .output()?;
        if output.status.success() {
            return Ok(());
        }
        let retryable = output.status.code() == Some(5) && attempt < 4;
        last_error = Some(command_output_error("launchctl bootstrap", &output));
        if !retryable {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(50 * (attempt + 1) as u64));
    }
    Err(last_error.unwrap_or_else(|| ServiceError::CommandFailed {
        command: "launchctl bootstrap".to_owned(),
        status: "failed without an exit status".to_owned(),
    }))
}

#[cfg(target_os = "macos")]
fn command_output_error(command: &str, output: &std::process::Output) -> ServiceError {
    let stderr = String::from_utf8_lossy(&output.stderr);
    let stderr = stderr.trim();
    let status = if stderr.is_empty() {
        output.status.to_string()
    } else {
        format!("{}: {stderr}", output.status)
    };
    ServiceError::CommandFailed {
        command: command.to_owned(),
        status,
    }
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
        Command::new("systemctl").args(["--user", "enable", "denju.service"]),
        "systemctl --user enable denju.service",
    )?;
    run_checked(
        Command::new("systemctl").args(["--user", "restart", "denju.service"]),
        "systemctl --user restart denju.service",
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
    let _ = Command::new("schtasks")
        .args(["/End", "/TN", WINDOWS_TASK])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
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
    #[cfg(target_os = "linux")]
    stop_matching_session_daemon(paths, executable)?;
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

#[cfg(target_os = "linux")]
fn stop_matching_session_daemon(paths: &LocalPaths, executable: &Path) -> Result<(), ServiceError> {
    let pid_path = paths.run.join("daemon.pid");
    let Ok(pid_text) = fs::read_to_string(&pid_path) else {
        return Ok(());
    };
    let Ok(pid) = pid_text.trim().parse::<u32>() else {
        return Ok(());
    };
    let process_executable = fs::read_link(format!("/proc/{pid}/exe"));
    let Ok(process_executable) = process_executable else {
        return Ok(());
    };
    let expected = executable.canonicalize()?;
    if !process_executable_matches(&process_executable, &expected) {
        return Ok(());
    }
    run_checked(
        Command::new("kill").args(["-TERM", &pid.to_string()]),
        "kill -TERM denju session daemon",
    )?;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while std::time::Instant::now() < deadline {
        if !pid_path.is_file() {
            return Ok(());
        }
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
    Err(ServiceError::CommandFailed {
        command: "stop Denju session daemon".to_owned(),
        status: "daemon did not exit within 5 seconds".to_owned(),
    })
}

#[cfg(any(target_os = "linux", test))]
fn process_executable_matches(process_executable: &Path, expected: &Path) -> bool {
    let actual = process_executable.to_string_lossy();
    let actual = actual.strip_suffix(" (deleted)").unwrap_or(&actual);
    Path::new(actual) == expected
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

    #[test]
    fn replaced_running_executable_keeps_its_linux_procfs_identity() {
        let expected = Path::new("/opt/denju/bin/denju");
        assert!(process_executable_matches(expected, expected));
        assert!(process_executable_matches(
            Path::new("/opt/denju/bin/denju (deleted)"),
            expected
        ));
        assert!(!process_executable_matches(
            Path::new("/usr/bin/something-else (deleted)"),
            expected
        ));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn launchctl_print_program_is_parsed_from_loaded_service() {
        let output = "gui/501/xyz.ashray.denju = {\n\
                      \tstate = running\n\
                      \tprogram = /Users/example/.local/bin/denju\n\
                      \targuments = {\n\
                      \t\t/Users/example/.local/bin/denju\n\
                      \t\tdaemon\n\
                      \t}\n\
                      }\n";
        assert_eq!(
            parse_launch_agent_program(output),
            Some("/Users/example/.local/bin/denju")
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn unchanged_launch_agent_restarts_in_place_only_for_the_same_executable() {
        let executable = Path::new("/Users/example/.local/bin/denju");
        assert!(can_restart_launch_agent_in_place(
            true,
            Some("/Users/example/.local/bin/denju"),
            executable
        ));
        assert!(!can_restart_launch_agent_in_place(
            false,
            Some("/Users/example/.local/bin/denju"),
            executable
        ));
        assert!(!can_restart_launch_agent_in_place(
            true,
            Some("/Users/example/old/denju"),
            executable
        ));
        assert!(!can_restart_launch_agent_in_place(true, None, executable));
    }
}
