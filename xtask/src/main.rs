use std::process::{Command, ExitCode};

fn main() -> ExitCode {
    let Some(command) = std::env::args().nth(1) else {
        eprintln!("usage: cargo xtask <check|rust|docs>");
        return ExitCode::FAILURE;
    };

    let result = match command.as_str() {
        "check" => check_all(),
        "rust" => check_rust(),
        "docs" => run("bun", &["run", "docs:check"]),
        other => {
            eprintln!("unknown xtask command: {other}");
            eprintln!("usage: cargo xtask <check|rust|docs>");
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

fn run(program: &str, args: &[&str]) -> Result<(), String> {
    eprintln!("+ {program} {}", args.join(" "));
    let status = Command::new(program)
        .args(args)
        .status()
        .map_err(|error| format!("failed to run {program}: {error}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("{program} exited with {status}"))
    }
}
