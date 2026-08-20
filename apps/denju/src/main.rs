use std::{ffi::OsString, process::ExitCode};

use clap::{ArgAction, Parser};
use denju_wire::{CliEnvelope, CliError, CliErrorCode};
use serde::Serialize;

const HELP: &str = "Denju — agent-native Agent Skills registry and synchronization\n\
\n\
Usage: denju [OPTIONS]\n\
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
    #[arg(short = 'V', long, action = ArgAction::SetTrue)]
    version: bool,
    #[arg(short = 'h', long, action = ArgAction::SetTrue)]
    help: bool,
}

#[derive(Debug, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum ResultPayload<'a> {
    Guidance {
        state: &'a str,
        next_command: &'a str,
    },
    Version {
        version: &'a str,
    },
    Help {
        text: &'a str,
    },
}

fn main() -> ExitCode {
    run(std::env::args_os())
}

fn run(args: impl IntoIterator<Item = OsString>) -> ExitCode {
    let args = args.into_iter().collect::<Vec<_>>();
    let requested_json = args.iter().skip(1).any(|arg| arg == "--json");
    let cli = match Cli::try_parse_from(&args) {
        Ok(cli) => cli,
        Err(error) => return present_parse_error(error, requested_json),
    };

    let version = build_version();
    let (payload, text) = if cli.help {
        (ResultPayload::Help { text: HELP }, HELP.to_owned())
    } else if cli.version {
        (
            ResultPayload::Version { version },
            format!("denju {version}"),
        )
    } else {
        (
            ResultPayload::Guidance {
                state: "setup_required",
                next_command: "denju setup",
            },
            "Denju is ready to set up.\nNext: denju setup".to_owned(),
        )
    };

    if cli.json {
        print_json(&CliEnvelope::success(payload))
    } else {
        println!("{text}");
        ExitCode::SUCCESS
    }
}

fn present_parse_error(error: clap::Error, json: bool) -> ExitCode {
    let message = concise_clap_message(&error);
    let error =
        CliError::new(CliErrorCode::InvalidArguments, message).with_recovery("denju --help");

    if json {
        print_json(&CliEnvelope::<ResultPayload<'_>>::failure(error));
    } else {
        eprintln!("error: {}", error.message());
        eprintln!("Next: {}", error.recovery().unwrap_or("denju --help"));
    }
    ExitCode::from(2)
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

fn print_json<T: Serialize>(value: &T) -> ExitCode {
    match serde_json::to_string(value) {
        Ok(value) => {
            println!("{value}");
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("error: failed to encode structured output: {error}");
            ExitCode::FAILURE
        }
    }
}

fn build_version() -> &'static str {
    option_env!("DENJU_BUILD_VERSION").unwrap_or(env!("CARGO_PKG_VERSION"))
}
