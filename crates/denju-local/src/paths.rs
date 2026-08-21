use std::{
    fs, io,
    path::{Path, PathBuf},
};

use thiserror::Error;

pub const TEST_HOME_ENV: &str = "DENJU_TEST_HOME";
pub const TEST_HOME_MARKER: &str = ".denju-test-home-v1";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalPaths {
    pub home: PathBuf,
    pub root: PathBuf,
    pub state_db: PathBuf,
    pub objects: PathBuf,
    pub generations: PathBuf,
    pub derived: PathBuf,
    pub skills: PathBuf,
    pub quarantine: PathBuf,
    pub imports: PathBuf,
    pub run: PathBuf,
    pub logs: PathBuf,
    pub credentials: PathBuf,
}

impl LocalPaths {
    pub fn from_home(home: PathBuf) -> Self {
        let root = home.join(".denju");
        Self {
            state_db: root.join("state.db"),
            objects: root.join("objects/blobs/sha256"),
            generations: root.join("generations"),
            derived: root.join("derived"),
            skills: root.join("skills"),
            quarantine: root.join("quarantine"),
            imports: root.join("staging/imports"),
            run: root.join("run"),
            logs: root.join("logs"),
            credentials: root.join("credentials"),
            root,
            home,
        }
    }

    pub fn discover() -> Result<Self, LocalPathError> {
        let home = discover_home()?;
        Ok(Self::from_home(home))
    }
}

pub fn ensure_local_layout(paths: &LocalPaths) -> Result<(), LocalPathError> {
    for directory in [
        &paths.root,
        &paths.objects,
        &paths.generations,
        &paths.derived,
        &paths.skills,
        &paths.quarantine,
        &paths.imports,
        &paths.run,
        &paths.logs,
        &paths.credentials,
    ] {
        fs::create_dir_all(directory)?;
    }
    Ok(())
}

pub fn verify_native_directory_links(paths: &LocalPaths) -> Result<(), LocalPathError> {
    let target = paths.run.join("link-check-target");
    let link = paths.run.join("link-check-link");
    let _ = fs::remove_file(&link);
    let _ = fs::remove_dir_all(&target);
    fs::create_dir_all(&target)?;
    create_native_directory_link(&target, &link).map_err(LocalPathError::NativeLinkUnavailable)?;
    let metadata = fs::symlink_metadata(&link)?;
    let valid_native_link = metadata.file_type().is_symlink() || cfg!(windows) && link.is_dir();
    if !valid_native_link {
        cleanup_link_check(&target, &link);
        return Err(LocalPathError::NativeLinkUnavailable(io::Error::other(
            "created link is not a native symbolic link",
        )));
    }
    cleanup_link_check(&target, &link);
    Ok(())
}

fn cleanup_link_check(target: &PathBuf, link: &PathBuf) {
    let _ = fs::remove_file(link);
    let _ = fs::remove_dir(link);
    let _ = fs::remove_dir_all(target);
}

#[cfg(unix)]
pub fn create_native_directory_link(target: &Path, link: &Path) -> io::Result<()> {
    std::os::unix::fs::symlink(target, link)
}

#[cfg(windows)]
pub fn create_native_directory_link(target: &Path, link: &Path) -> io::Result<()> {
    match std::os::windows::fs::symlink_dir(target, link) {
        Ok(()) => Ok(()),
        Err(symlink_error) => {
            let status = std::process::Command::new("cmd")
                .args(["/C", "mklink", "/J"])
                .arg(link)
                .arg(target)
                .status()?;
            if status.success() {
                Ok(())
            } else {
                Err(io::Error::new(
                    symlink_error.kind(),
                    format!(
                        "directory symlink failed ({symlink_error}); junction creation exited with {status}"
                    ),
                ))
            }
        }
    }
}

fn discover_home() -> Result<PathBuf, LocalPathError> {
    if let Some(test_home) = std::env::var_os(TEST_HOME_ENV) {
        return validate_test_home(PathBuf::from(test_home));
    }
    if test_mode_requested() {
        return Err(LocalPathError::TestHomeRequired);
    }
    #[cfg(windows)]
    {
        if let Some(home) = std::env::var_os("USERPROFILE") {
            return Ok(PathBuf::from(home));
        }
        let drive = std::env::var_os("HOMEDRIVE");
        let path = std::env::var_os("HOMEPATH");
        if let (Some(drive), Some(path)) = (drive, path) {
            let mut home = PathBuf::from(drive);
            home.push(path);
            return Ok(home);
        }
    }
    #[cfg(not(windows))]
    if let Some(home) = std::env::var_os("HOME") {
        return Ok(PathBuf::from(home));
    }
    Err(LocalPathError::HomeUnavailable)
}

fn validate_test_home(home: PathBuf) -> Result<PathBuf, LocalPathError> {
    if !home.is_absolute() {
        return Err(LocalPathError::InvalidTestHome(
            "DENJU_TEST_HOME must be an absolute path".to_owned(),
        ));
    }
    if !home.is_dir() {
        return Err(LocalPathError::InvalidTestHome(format!(
            "DENJU_TEST_HOME is not a directory: {}",
            home.display()
        )));
    }
    let marker = home.join(TEST_HOME_MARKER);
    let metadata = fs::symlink_metadata(&marker).map_err(|error| {
        LocalPathError::InvalidTestHome(format!(
            "DENJU_TEST_HOME must contain a regular {TEST_HOME_MARKER} marker: {error}"
        ))
    })?;
    if !metadata.file_type().is_file() || metadata.file_type().is_symlink() {
        return Err(LocalPathError::InvalidTestHome(format!(
            "DENJU_TEST_HOME marker must be a regular file: {}",
            marker.display()
        )));
    }
    Ok(home)
}

fn test_mode_requested() -> bool {
    [
        "DENJU_TEST_FILE_CREDENTIALS",
        "DENJU_TEST_SERVICE_INSTALL_ONLY",
        "DENJU_DAEMON_ONCE",
    ]
    .iter()
    .any(|name| std::env::var_os(name).is_some())
}

#[derive(Debug, Error)]
pub enum LocalPathError {
    #[error("cannot determine the current user's home directory")]
    HomeUnavailable,
    #[error("Denju test mode requires an explicit isolated DENJU_TEST_HOME")]
    TestHomeRequired,
    #[error("invalid isolated Denju test home: {0}")]
    InvalidTestHome(String),
    #[error("local filesystem error: {0}")]
    Io(#[from] io::Error),
    #[error("native directory links are unavailable: {0}")]
    NativeLinkUnavailable(io::Error),
}

#[cfg(test)]
mod tests {
    use tempfile::tempdir;

    use super::*;

    #[test]
    fn local_layout_is_contained_under_home() {
        let home = tempdir().unwrap();
        let paths = LocalPaths::from_home(home.path().to_owned());
        ensure_local_layout(&paths).unwrap();
        assert!(paths.state_db.starts_with(home.path()));
        assert!(paths.objects.is_dir());
        assert!(paths.skills.is_dir());
    }

    #[test]
    fn isolated_test_home_requires_absolute_marked_directory() {
        let home = tempdir().unwrap();
        assert!(matches!(
            validate_test_home(PathBuf::from("relative")),
            Err(LocalPathError::InvalidTestHome(_))
        ));
        assert!(matches!(
            validate_test_home(home.path().to_owned()),
            Err(LocalPathError::InvalidTestHome(_))
        ));
        fs::write(home.path().join(TEST_HOME_MARKER), b"isolated\n").unwrap();
        assert_eq!(
            validate_test_home(home.path().to_owned()).unwrap(),
            home.path()
        );
    }
}
