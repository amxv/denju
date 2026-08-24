use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

const DOCS_CONFIG: &str = "vercel.json";
const REGISTRY_CONFIG: &str = "deploy/vercel.registry.json";

pub(crate) fn run(root: &Path) -> Result<(), String> {
    let mut args = std::env::args().skip(2);
    let mut output = None;
    while let Some(argument) = args.next() {
        match argument.as_str() {
            "--out" => output = args.next().map(PathBuf::from),
            other => return Err(format!("unknown vercel-context option: {other}")),
        }
    }
    let output = output.ok_or_else(|| "vercel-context requires --out PATH".to_owned())?;
    create(root, &output)?;
    println!("Vercel registry context: {}", output.display());
    Ok(())
}

fn create(root: &Path, output: &Path) -> Result<(), String> {
    if output.exists() {
        return Err(format!(
            "Vercel context output already exists: {}",
            output.display()
        ));
    }
    let files = current_repository_files(root)?;
    fs::create_dir_all(output).map_err(io_error("create Vercel context output"))?;
    for relative in files {
        if relative == Path::new(DOCS_CONFIG) {
            continue;
        }
        let source = root.join(&relative);
        if !source.is_file() {
            continue;
        }
        let destination = output.join(&relative);
        if let Some(parent) = destination.parent() {
            fs::create_dir_all(parent).map_err(io_error("create Vercel context directory"))?;
        }
        fs::copy(&source, &destination).map_err(|error| {
            format!(
                "copy Vercel context file {} -> {}: {error}",
                source.display(),
                destination.display()
            )
        })?;
    }
    let registry_config = root.join(REGISTRY_CONFIG);
    if !registry_config.is_file() {
        return Err(format!(
            "registry Vercel config is missing: {}",
            registry_config.display()
        ));
    }
    fs::copy(&registry_config, output.join(DOCS_CONFIG))
        .map_err(io_error("stage registry Vercel config"))?;
    Ok(())
}

fn current_repository_files(root: &Path) -> Result<Vec<PathBuf>, String> {
    let output = Command::new("git")
        .args([
            "ls-files",
            "-z",
            "--cached",
            "--others",
            "--exclude-standard",
        ])
        .current_dir(root)
        .output()
        .map_err(|error| format!("failed to list repository files for Vercel context: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "git ls-files for Vercel context exited with {}",
            output.status
        ));
    }
    output
        .stdout
        .split(|byte| *byte == 0)
        .filter(|bytes| !bytes.is_empty())
        .map(|bytes| {
            std::str::from_utf8(bytes)
                .map(PathBuf::from)
                .map_err(|error| format!("non-UTF8 repository path in Vercel context: {error}"))
        })
        .collect()
}

fn io_error(context: &'static str) -> impl FnOnce(std::io::Error) -> String {
    move |error| format!("{context}: {error}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_context_replaces_only_the_root_docs_config() {
        let root = tempfile::tempdir().unwrap();
        let output_parent = tempfile::tempdir().unwrap();
        let output = output_parent.path().join("context");
        fs::create_dir_all(root.path().join("deploy")).unwrap();
        fs::write(root.path().join("vercel.json"), b"docs-config\n").unwrap();
        fs::write(root.path().join(REGISTRY_CONFIG), b"registry-config\n").unwrap();
        fs::write(root.path().join("Dockerfile.vercel"), b"FROM scratch\n").unwrap();

        let status = Command::new("git")
            .args(["init", "-q"])
            .current_dir(root.path())
            .status()
            .unwrap();
        assert!(status.success());
        let status = Command::new("git")
            .args(["add", "."])
            .current_dir(root.path())
            .status()
            .unwrap();
        assert!(status.success());

        create(root.path(), &output).unwrap();

        assert_eq!(
            fs::read(output.join("vercel.json")).unwrap(),
            b"registry-config\n"
        );
        assert_eq!(
            fs::read(output.join("Dockerfile.vercel")).unwrap(),
            b"FROM scratch\n"
        );
        assert_eq!(
            fs::read(output.join(REGISTRY_CONFIG)).unwrap(),
            b"registry-config\n"
        );
    }
}
