use std::{
    io::{Read, Write},
    net::{SocketAddr, TcpStream},
    path::{Path, PathBuf},
    process::{Command, ExitCode},
    thread,
    time::{Duration, Instant},
};

mod load;
mod repository_checks;

fn main() -> ExitCode {
    let Some(command) = std::env::args().nth(1) else {
        eprintln!("usage: cargo xtask <check|rust|docs|contracts|fuzz|load|dev>");
        return ExitCode::FAILURE;
    };

    let result = match command.as_str() {
        "check" => check_all(),
        "rust" => check_rust(),
        "docs" => run("bun", &["run", "docs:check"]),
        "contracts" => match std::env::args().nth(2).as_deref() {
            None => repository_checks::check(&repo_root()),
            Some("--update") => repository_checks::update_fixture_checksums(&repo_root()),
            Some(other) => Err(format!("unknown contracts option: {other}")),
        },
        "fuzz" => fuzz_properties(),
        "load" => load::run(&repo_root()),
        "dev" => dev(),
        other => {
            eprintln!("unknown xtask command: {other}");
            eprintln!("usage: cargo xtask <check|rust|docs|contracts|fuzz|load|dev>");
            return ExitCode::FAILURE;
        }
    };

    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("{message}");
            ExitCode::FAILURE
        }
    }
}

fn check_all() -> Result<(), String> {
    repository_checks::check(&repo_root())?;
    check_rust()?;
    run("bun", &["run", "check"])
}

fn check_rust() -> Result<(), String> {
    run("cargo", &["fmt", "--all", "--check"])?;
    run(
        "cargo",
        &[
            "clippy",
            "--workspace",
            "--all-targets",
            "--",
            "-D",
            "warnings",
        ],
    )?;
    run("cargo", &["test", "--workspace"])
}

fn fuzz_properties() -> Result<(), String> {
    let cases = std::env::var("DENJU_PROPTEST_CASES").unwrap_or_else(|_| "4096".to_owned());
    eprintln!("property/fuzz cases per property: {cases}");
    run_with_env(
        "cargo",
        &[
            "test",
            "--release",
            "-p",
            "denju-core",
            "--test",
            "properties",
            "-p",
            "denju-wire",
            "--test",
            "properties",
            "-p",
            "denju-sync",
            "--test",
            "properties",
        ],
        &[("DENJU_PROPTEST_CASES", &cases)],
    )
}

fn dev() -> Result<(), String> {
    let root = repo_root();
    eprintln!("+ docker compose -f deploy/dev.compose.yml up -d");
    let status = Command::new("docker")
        .args(["compose", "-f", "deploy/dev.compose.yml", "up", "-d"])
        .current_dir(&root)
        .status()
        .map_err(|error| format!("failed to run docker compose: {error}"))?;
    if !status.success() {
        return Err(format!("docker compose exited with {status}"));
    }

    wait_for_tcp("PostgreSQL", "127.0.0.1:55432".parse().unwrap())?;
    wait_for_tcp("Garage", "127.0.0.1:53900".parse().unwrap())?;

    let env = dev_server_env();
    eprintln!("+ cargo run -p denju-server -- migrate");
    let migration = server_command(&root, &env, "migrate")
        .status()
        .map_err(|error| format!("failed to run denju-server migrate: {error}"))?;
    if !migration.success() {
        return Err(format!("denju-server migrate exited with {migration}"));
    }
    configure_dev_database_roles(&root)?;

    println!("registry: http://127.0.0.1:7788");
    println!("postgres: postgresql://denju@127.0.0.1:55432/denju");
    println!("s3: http://127.0.0.1:53900 (bucket denju-dev, region garage)");
    println!("credentials: deterministic development values from deploy/dev.compose.yml");

    if registry_live() {
        println!("denju-server is already running");
        return Ok(());
    }

    eprintln!("+ cargo run -p denju-server -- serve");
    let status = server_command(&root, &env, "serve")
        .status()
        .map_err(|error| format!("failed to run denju-server serve: {error}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("denju-server exited with {status}"))
    }
}

fn server_command(root: &Path, env: &[(&str, &str)], subcommand: &str) -> Command {
    let mut command = Command::new("cargo");
    command
        .args(["run", "-p", "denju-server", "--", subcommand])
        .current_dir(root);
    for (key, value) in env {
        command.env(key, value);
    }
    command
}

fn dev_server_env() -> Vec<(&'static str, &'static str)> {
    vec![
        ("DENJU_BIND", "127.0.0.1:7788"),
        ("DENJU_PUBLIC_URL", "http://127.0.0.1:7788"),
        (
            "DENJU_DATABASE_URL",
            "postgresql://denju_app:denju-app-dev-only@127.0.0.1:55432/denju",
        ),
        (
            "DENJU_DATABASE_WORKER_URL",
            "postgresql://denju_worker:denju-worker-dev-only@127.0.0.1:55432/denju",
        ),
        (
            "DENJU_DATABASE_DIRECT_URL",
            "postgresql://denju_app:denju-app-dev-only@127.0.0.1:55432/denju",
        ),
        (
            "DENJU_DATABASE_MIGRATION_URL",
            "postgresql://denju:denju-dev-only@127.0.0.1:55432/denju",
        ),
        ("DENJU_S3_BUCKET", "denju-dev"),
        ("DENJU_S3_ENDPOINT", "http://127.0.0.1:53900"),
        ("DENJU_S3_REGION", "garage"),
        ("DENJU_S3_ACCESS_KEY_ID", "GK1234567890ABCDEFGH"),
        (
            "DENJU_S3_SECRET_ACCESS_KEY",
            "0000000000000000000000000000000000000000000000000000000000000000",
        ),
        ("DENJU_S3_FORCE_PATH_STYLE", "true"),
    ]
}

fn configure_dev_database_roles(root: &Path) -> Result<(), String> {
    eprintln!("+ configure restricted PostgreSQL development roles");
    let status = Command::new("docker")
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
            "denju",
            "-v",
            "ON_ERROR_STOP=1",
            "-c",
            "ALTER ROLE denju_app PASSWORD 'denju-app-dev-only'; ALTER ROLE denju_worker PASSWORD 'denju-worker-dev-only';",
        ])
        .current_dir(root)
        .status()
        .map_err(|error| format!("failed to configure development database roles: {error}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!(
            "development database role configuration exited with {status}"
        ))
    }
}

fn wait_for_tcp(name: &str, address: SocketAddr) -> Result<(), String> {
    let deadline = Instant::now() + Duration::from_secs(60);
    while Instant::now() < deadline {
        if TcpStream::connect_timeout(&address, Duration::from_millis(300)).is_ok() {
            return Ok(());
        }
        thread::sleep(Duration::from_millis(250));
    }
    Err(format!("{name} did not become reachable at {address}"))
}

fn registry_live() -> bool {
    let Ok(mut stream) = TcpStream::connect_timeout(
        &"127.0.0.1:7788".parse().unwrap(),
        Duration::from_millis(300),
    ) else {
        return false;
    };
    let _ = stream.set_read_timeout(Some(Duration::from_secs(1)));
    if stream
        .write_all(b"GET /health/live HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
        .is_err()
    {
        return false;
    }
    let mut response = String::new();
    stream.read_to_string(&mut response).is_ok() && response.starts_with("HTTP/1.1 200")
}

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("xtask lives under repository root")
        .to_owned()
}

fn run(program: &str, args: &[&str]) -> Result<(), String> {
    eprintln!("+ {program} {}", args.join(" "));
    let status = Command::new(program)
        .args(args)
        .current_dir(repo_root())
        .status()
        .map_err(|error| format!("failed to run {program}: {error}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("{program} exited with {status}"))
    }
}

fn run_with_env(program: &str, args: &[&str], env: &[(&str, &str)]) -> Result<(), String> {
    eprintln!("+ {program} {}", args.join(" "));
    let mut command = Command::new(program);
    command.args(args).current_dir(repo_root());
    for (key, value) in env {
        command.env(key, value);
    }
    let status = command
        .status()
        .map_err(|error| format!("failed to run {program}: {error}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("{program} exited with {status}"))
    }
}
