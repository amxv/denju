mod setup;

use std::{ffi::OsString, process::ExitCode};

use clap::{ArgAction, Parser, Subcommand};
use denju_wire::{CliEnvelope, CliError, CliErrorCode};
use serde::Serialize;
use setup::{DoctorOutcome, Guidance, RuntimeError, SetupOutcome};

const HELP: &str = "Denju — agent-native Agent Skills registry and synchronization\n\
\n\
Usage: denju [OPTIONS] [COMMAND]\n\
\n\
Commands:\n\
  setup   Set up this machine without creating an account\n\
  doctor  Check and repair the local Denju installation\n\
\n\
Options:\n\
      --json     Emit one versioned JSON result on stdout\n\
  -V, --version  Print the Denju build version\n\
  -h, --help     Print help\n\
\n\
Run denju with no command for the next useful action.";

#[derive(Debug, Parser)]
#[command(
    name = "denju",
    disable_help_flag = true,
    disable_version_flag = true,
    disable_help_subcommand = true,
    color = clap::ColorChoice::Never
)]
struct Cli {
    #[arg(long, global = true, action = ArgAction::SetTrue)]
    json: bool,
    #[arg(short = 'V', long, global = true, action = ArgAction::SetTrue)]
    version: bool,
    #[arg(short = 'h', long, global = true, action = ArgAction::SetTrue)]
    help: bool,
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Debug, Subcommand)]
enum Command {
    Setup {
        #[arg(long, value_name = "URL")]
        registry: Option<String>,
    },
    Doctor,
    #[command(hide = true)]
    Daemon,
}

#[derive(Debug, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum ResultPayload {
    Guidance {
        state: &'static str,
        #[serde(skip_serializing_if = "Option::is_none")]
        next_command: Option<&'static str>,
    },
    Setup {
        #[serde(flatten)]
        outcome: SetupOutcome,
    },
    Doctor {
        #[serde(flatten)]
        outcome: DoctorOutcome,
    },
    Version {
        version: &'static str,
    },
    Help {
        text: &'static str,
    },
}

struct CommandOutput {
    payload: ResultPayload,
    text: String,
    exit: ExitCode,
}

#[tokio::main]
async fn main() -> ExitCode {
    run(std::env::args_os()).await
}

async fn run(args: impl IntoIterator<Item = OsString>) -> ExitCode {
    let args = args.into_iter().collect::<Vec<_>>();
    let requested_json = args.iter().skip(1).any(|arg| arg == "--json");
    let cli = match Cli::try_parse_from(&args) {
        Ok(cli) => cli,
        Err(error) => return present_parse_error(error, requested_json),
    };

    if cli.help {
        return present(
            cli.json,
            CommandOutput {
                payload: ResultPayload::Help { text: HELP },
                text: HELP.to_owned(),
                exit: ExitCode::SUCCESS,
            },
        );
    }
    if cli.version {
        let version = build_version();
        return present(
            cli.json,
            CommandOutput {
                payload: ResultPayload::Version { version },
                text: format!("denju {version}"),
                exit: ExitCode::SUCCESS,
            },
        );
    }

    let result = match cli.command {
        Some(Command::Setup { registry }) => {
            setup::setup(registry).await.map(|outcome| CommandOutput {
                text: setup_text(&outcome),
                payload: ResultPayload::Setup { outcome },
                exit: ExitCode::SUCCESS,
            })
        }
        Some(Command::Doctor) => setup::doctor().await.map(|outcome| {
            let exit = if outcome.healthy {
                ExitCode::SUCCESS
            } else {
                ExitCode::FAILURE
            };
            CommandOutput {
                text: doctor_text(&outcome),
                payload: ResultPayload::Doctor { outcome },
                exit,
            }
        }),
        Some(Command::Daemon) => return daemon(cli.json).await,
        None => Ok(guidance_output(setup::guidance().await)),
    };

    match result {
        Ok(output) => present(cli.json, output),
        Err(error) => present_runtime_error(error, cli.json),
    }
}

async fn daemon(json: bool) -> ExitCode {
    match setup::daemon().await {
        Ok(exit) => exit,
        Err(error) => present_runtime_error(error, json),
    }
}

fn guidance_output(guidance: Guidance) -> CommandOutput {
    match guidance {
        Guidance::SetupRequired => CommandOutput {
            payload: ResultPayload::Guidance {
                state: "setup_required",
                next_command: Some("denju setup"),
            },
            text: "Denju is ready to set up.\nNext: denju setup".to_owned(),
            exit: ExitCode::SUCCESS,
        },
        Guidance::RepairRequired => CommandOutput {
            payload: ResultPayload::Guidance {
                state: "repair_required",
                next_command: Some("denju doctor"),
            },
            text: "Denju needs repair.\nNext: denju doctor".to_owned(),
            exit: ExitCode::SUCCESS,
        },
        Guidance::Healthy => CommandOutput {
            payload: ResultPayload::Guidance {
                state: "healthy",
                next_command: None,
            },
            text: "Denju is healthy.".to_owned(),
            exit: ExitCode::SUCCESS,
        },
    }
}

fn setup_text(outcome: &SetupOutcome) -> String {
    let service = if outcome.service_running {
        format!("{} (running)", outcome.service_kind)
    } else if let Some(detail) = &outcome.service_detail {
        format!("{} ({detail})", outcome.service_kind)
    } else {
        format!("{} (not running)", outcome.service_kind)
    };
    let mut lines = vec![
        "Denju setup complete.".to_owned(),
        format!("Registry: {}", outcome.registry),
        format!("Codex: {}", outcome.codex_root),
        format!("Claude: {}", outcome.claude_root),
        format!("Service: {service}"),
    ];
    if let Some(path) = outcome.unmanaged_skills.first() {
        lines.push(format!("Next: denju import {}", quote_command_arg(path)));
    }
    lines.join("\n")
}

fn doctor_text(outcome: &DoctorOutcome) -> String {
    let mut lines = if outcome.healthy {
        vec!["Denju is healthy.".to_owned()]
    } else {
        vec!["Denju still needs attention.".to_owned()]
    };
    lines.extend(
        outcome
            .repaired
            .iter()
            .map(|item| format!("Repaired: {item}")),
    );
    lines.extend(outcome.issues.iter().map(|item| format!("Issue: {item}")));
    lines.push(format!("Registry: {}", outcome.registry));
    lines.join("\n")
}

fn quote_command_arg(value: &str) -> String {
    format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
}

fn present(json: bool, output: CommandOutput) -> ExitCode {
    if json {
        if print_json(&CliEnvelope::success(output.payload)).is_err() {
            return ExitCode::FAILURE;
        }
    } else if !output.text.is_empty() {
        println!("{}", output.text);
    }
    output.exit
}

fn present_parse_error(error: clap::Error, json: bool) -> ExitCode {
    let message = concise_clap_message(&error);
    let error =
        CliError::new(CliErrorCode::InvalidArguments, message).with_recovery("denju --help");
    present_error(error, json, ExitCode::from(2))
}

fn present_runtime_error(error: RuntimeError, json: bool) -> ExitCode {
    let mut wire = CliError::new(error.code, error.message);
    if let Some(recovery) = error.recovery {
        wire = wire.with_recovery(recovery);
    }
    present_error(wire, json, ExitCode::FAILURE)
}

fn present_error(error: CliError, json: bool, exit: ExitCode) -> ExitCode {
    if json {
        if print_json(&CliEnvelope::<ResultPayload>::failure(error)).is_err() {
            return ExitCode::FAILURE;
        }
    } else {
        eprintln!("error: {}", error.message());
        if let Some(recovery) = error.recovery() {
            eprintln!("Next: {recovery}");
        }
    }
    exit
}

fn concise_clap_message(error: &clap::Error) -> String {
    let rendered = error.to_string();
    rendered
        .lines()
        .find(|line| !line.trim().is_empty())
        .unwrap_or("invalid arguments")
        .trim()
        .strip_prefix("error: ")
        .unwrap_or(rendered.trim())
        .to_owned()
}

fn print_json<T: Serialize>(value: &T) -> Result<(), serde_json::Error> {
    println!("{}", serde_json::to_string(value)?);
    Ok(())
}

fn build_version() -> &'static str {
    option_env!("DENJU_BUILD_VERSION").unwrap_or(env!("CARGO_PKG_VERSION"))
}
