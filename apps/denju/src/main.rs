mod help;
mod identity;
mod lifecycle;
mod output;
mod owned;
mod public;
mod release;
mod setup;
mod workspace;
mod workspace_merge;

use std::{ffi::OsString, path::PathBuf, process::ExitCode};

use clap::{ArgAction, Parser, Subcommand};
use denju_wire::{
    AutomationTokenList, AutomationTokenRevokeResponse, CliEnvelope, CliError, CliErrorCode,
    DeleteSkillResponse, DeprecateSkillResponse, DeviceList, DeviceRevokeResponse,
    HistoryPruneResponse, IdentityInfo, PublicSkillDetail, PublicSkillSearchResponse,
    PublishSkillResponse, RenameSkillResponse, SkillHistoryResponse, UnpublishSkillResponse,
};
use help::HELP;
use identity::{
    AutomationTokenOutcome, BackupOutcome, ClaimOutcome, DeleteOutcome, LoginOutcome,
    RecoveryOutcome,
};
use lifecycle::UsageOutcome;
use output::{
    append_deprecation_notice, diff_text, history_text, search_text, short_revision, show_text,
    sync_text, usage_text,
};
use owned::ImportOutcome;
use public::{SubscribeOutcome, SyncOutcome, UnsubscribeOutcome};
use release::{DiffOutcome, ExportOutcome, RestoreOutcome};
use serde::Serialize;
use setup::{DoctorOutcome, Guidance, RuntimeError, SetupOutcome};

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
    Claim {
        username: String,
    },
    Login {
        username: String,
    },
    Identity {
        #[command(subcommand)]
        command: Option<IdentityCommand>,
    },
    Devices {
        #[command(subcommand)]
        command: Option<DevicesCommand>,
    },
    Tokens {
        #[command(subcommand)]
        command: Option<TokenCommand>,
    },
    Search {
        query: String,
    },
    Show {
        locator: String,
    },
    Import {
        path: PathBuf,
    },
    Publish {
        locator: String,
        #[arg(long)]
        message: Option<String>,
        #[arg(long = "tag")]
        tags: Vec<String>,
    },
    Rename {
        locator: String,
        new_name: String,
    },
    Unpublish {
        locator: String,
    },
    Delete {
        locator: String,
        #[arg(long, action = ArgAction::SetTrue)]
        yes: bool,
    },
    Deprecate {
        locator: String,
        #[arg(long)]
        replacement: Option<String>,
        #[arg(long, action = ArgAction::SetTrue)]
        undo: bool,
    },
    Usage,
    History {
        locator: Option<String>,
        #[command(subcommand)]
        command: Option<HistoryCommand>,
    },
    Diff {
        locator: String,
        revision_a: Option<String>,
        revision_b: Option<String>,
    },
    Restore {
        locator: String,
        revision: String,
    },
    Export {
        locator: String,
        destination: PathBuf,
    },
    Subscribe {
        locator: String,
        #[arg(long = "version", value_name = "N")]
        release_version: Option<u64>,
        #[arg(long, action = ArgAction::SetTrue)]
        retain_on_delete: bool,
    },
    Unsubscribe {
        locator: String,
    },
    Status,
    Sync,
    Doctor,
    #[command(hide = true)]
    Daemon,
}

#[derive(Debug, Subcommand)]
enum IdentityCommand {
    Backup,
    Recover {
        username: String,
    },
    Delete {
        #[arg(long, action = ArgAction::SetTrue)]
        yes: bool,
    },
}

#[derive(Debug, Subcommand)]
enum HistoryCommand {
    Prune {
        locator: String,
        #[arg(long, action = ArgAction::SetTrue)]
        yes: bool,
    },
}

#[derive(Debug, Subcommand)]
enum DevicesCommand {
    Revoke { session_id: String },
}

#[derive(Debug, Subcommand)]
enum TokenCommand {
    Create {
        #[arg(long = "scope", required = true)]
        scopes: Vec<String>,
        #[arg(long, default_value_t = 86_400)]
        expires_in_seconds: u64,
    },
    Revoke {
        token_id: String,
    },
}

#[derive(Debug, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum ResultPayload {
    Guidance {
        state: &'static str,
        #[serde(skip_serializing_if = "Option::is_none")]
        next_command: Option<String>,
    },
    Setup {
        #[serde(flatten)]
        outcome: SetupOutcome,
    },
    Claim {
        #[serde(flatten)]
        outcome: ClaimOutcome,
    },
    Login {
        #[serde(flatten)]
        outcome: LoginOutcome,
    },
    Identity {
        #[serde(flatten)]
        outcome: IdentityInfo,
    },
    IdentityBackup {
        #[serde(flatten)]
        outcome: BackupOutcome,
    },
    IdentityRecovery {
        #[serde(flatten)]
        outcome: RecoveryOutcome,
    },
    IdentityDelete {
        #[serde(flatten)]
        outcome: DeleteOutcome,
    },
    Devices {
        #[serde(flatten)]
        outcome: DeviceList,
    },
    DeviceRevoke {
        #[serde(flatten)]
        outcome: DeviceRevokeResponse,
    },
    TokenCreate {
        #[serde(flatten)]
        outcome: AutomationTokenOutcome,
    },
    Tokens {
        #[serde(flatten)]
        outcome: AutomationTokenList,
    },
    TokenRevoke {
        #[serde(flatten)]
        outcome: AutomationTokenRevokeResponse,
    },
    Doctor {
        #[serde(flatten)]
        outcome: DoctorOutcome,
    },
    Search {
        #[serde(flatten)]
        outcome: PublicSkillSearchResponse,
    },
    Show {
        #[serde(flatten)]
        outcome: PublicSkillDetail,
    },
    Import {
        #[serde(flatten)]
        outcome: ImportOutcome,
    },
    Publish {
        #[serde(flatten)]
        outcome: PublishSkillResponse,
    },
    Rename {
        #[serde(flatten)]
        outcome: RenameSkillResponse,
    },
    Unpublish {
        #[serde(flatten)]
        outcome: UnpublishSkillResponse,
    },
    Delete {
        #[serde(flatten)]
        outcome: DeleteSkillResponse,
    },
    Deprecate {
        #[serde(flatten)]
        outcome: DeprecateSkillResponse,
    },
    Usage {
        #[serde(flatten)]
        outcome: UsageOutcome,
    },
    History {
        #[serde(flatten)]
        outcome: SkillHistoryResponse,
    },
    HistoryPrune {
        #[serde(flatten)]
        outcome: HistoryPruneResponse,
    },
    Diff {
        #[serde(flatten)]
        outcome: DiffOutcome,
    },
    Restore {
        #[serde(flatten)]
        outcome: RestoreOutcome,
    },
    Export {
        #[serde(flatten)]
        outcome: ExportOutcome,
    },
    Subscribe {
        #[serde(flatten)]
        outcome: SubscribeOutcome,
    },
    Unsubscribe {
        #[serde(flatten)]
        outcome: UnsubscribeOutcome,
    },
    Status {
        #[serde(flatten)]
        outcome: workspace::StatusOutcome,
    },
    Sync {
        #[serde(flatten)]
        outcome: SyncOutcome,
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
        Some(Command::Claim { username }) => {
            identity::claim(&username, cli.json)
                .await
                .map(|outcome| CommandOutput {
                    text: claim_text(&outcome),
                    payload: ResultPayload::Claim { outcome },
                    exit: ExitCode::SUCCESS,
                })
        }
        Some(Command::Login { username }) => {
            identity::login(&username, cli.json)
                .await
                .map(|outcome| CommandOutput {
                    text: login_text(&outcome),
                    payload: ResultPayload::Login { outcome },
                    exit: ExitCode::SUCCESS,
                })
        }
        Some(Command::Identity { command: None }) => {
            identity::info().await.map(|outcome| CommandOutput {
                text: format!("{} ({})", outcome.username, outcome.user_id),
                payload: ResultPayload::Identity { outcome },
                exit: ExitCode::SUCCESS,
            })
        }
        Some(Command::Identity {
            command: Some(IdentityCommand::Backup),
        }) => identity::backup(cli.json)
            .await
            .map(|outcome| CommandOutput {
                text: backup_text(&outcome),
                payload: ResultPayload::IdentityBackup { outcome },
                exit: ExitCode::SUCCESS,
            }),
        Some(Command::Identity {
            command: Some(IdentityCommand::Recover { username }),
        }) => identity::recover(&username, cli.json)
            .await
            .map(|outcome| CommandOutput {
                text: recovery_text(&outcome),
                payload: ResultPayload::IdentityRecovery { outcome },
                exit: ExitCode::SUCCESS,
            }),
        Some(Command::Identity {
            command: Some(IdentityCommand::Delete { yes }),
        }) => identity::delete_account(cli.json, yes)
            .await
            .map(|outcome| CommandOutput {
                text: format!("Deleted {}.", outcome.username),
                payload: ResultPayload::IdentityDelete { outcome },
                exit: ExitCode::SUCCESS,
            }),
        Some(Command::Devices { command: None }) => {
            identity::devices().await.map(|outcome| CommandOutput {
                text: devices_text(&outcome),
                payload: ResultPayload::Devices { outcome },
                exit: ExitCode::SUCCESS,
            })
        }
        Some(Command::Devices {
            command: Some(DevicesCommand::Revoke { session_id }),
        }) => identity::revoke_device(&session_id)
            .await
            .map(|outcome| CommandOutput {
                text: format!(
                    "{} {}",
                    if outcome.revoked {
                        "Revoked"
                    } else {
                        "Already inactive"
                    },
                    outcome.session_id
                ),
                payload: ResultPayload::DeviceRevoke { outcome },
                exit: ExitCode::SUCCESS,
            }),
        Some(Command::Tokens { command: None }) => {
            identity::automation_tokens()
                .await
                .map(|outcome| CommandOutput {
                    text: tokens_text(&outcome),
                    payload: ResultPayload::Tokens { outcome },
                    exit: ExitCode::SUCCESS,
                })
        }
        Some(Command::Tokens {
            command:
                Some(TokenCommand::Create {
                    scopes,
                    expires_in_seconds,
                }),
        }) => identity::create_automation_token(scopes, expires_in_seconds)
            .await
            .map(|outcome| CommandOutput {
                text: token_text(&outcome),
                payload: ResultPayload::TokenCreate { outcome },
                exit: ExitCode::SUCCESS,
            }),
        Some(Command::Tokens {
            command: Some(TokenCommand::Revoke { token_id }),
        }) => identity::revoke_automation_token(&token_id)
            .await
            .map(|outcome| CommandOutput {
                text: format!(
                    "{} {}",
                    if outcome.revoked {
                        "Revoked"
                    } else {
                        "Already inactive"
                    },
                    outcome.token_id
                ),
                payload: ResultPayload::TokenRevoke { outcome },
                exit: ExitCode::SUCCESS,
            }),
        Some(Command::Search { query }) => {
            public::search(&query).await.map(|outcome| CommandOutput {
                text: search_text(&outcome),
                payload: ResultPayload::Search { outcome },
                exit: ExitCode::SUCCESS,
            })
        }
        Some(Command::Show { locator }) => {
            public::show(&locator).await.map(|outcome| CommandOutput {
                text: show_text(&outcome),
                payload: ResultPayload::Show { outcome },
                exit: ExitCode::SUCCESS,
            })
        }
        Some(Command::Import { path }) => owned::import(&path).await.map(|outcome| CommandOutput {
            text: format!("Imported {} as {}", outcome.locator, outcome.harness_name),
            payload: ResultPayload::Import { outcome },
            exit: ExitCode::SUCCESS,
        }),
        Some(Command::Publish {
            locator,
            message,
            tags,
        }) => release::publish(&locator, message, tags)
            .await
            .map(|outcome| CommandOutput {
                text: format!(
                    "Published {} v{}",
                    outcome.skill.locator, outcome.release.version
                ),
                payload: ResultPayload::Publish { outcome },
                exit: ExitCode::SUCCESS,
            }),
        Some(Command::Rename { locator, new_name }) => lifecycle::rename(&locator, &new_name)
            .await
            .map(|outcome| CommandOutput {
                text: format!("Renamed {} to {}", outcome.old_locator, outcome.locator),
                payload: ResultPayload::Rename { outcome },
                exit: ExitCode::SUCCESS,
            }),
        Some(Command::Unpublish { locator }) => {
            lifecycle::unpublish(&locator)
                .await
                .map(|outcome| CommandOutput {
                    text: format!("Unpublished {}", outcome.locator),
                    payload: ResultPayload::Unpublish { outcome },
                    exit: ExitCode::SUCCESS,
                })
        }
        Some(Command::Delete { locator, yes }) => lifecycle::delete(&locator, cli.json, yes)
            .await
            .map(|outcome| CommandOutput {
                text: format!("Deleted {}", outcome.locator),
                payload: ResultPayload::Delete { outcome },
                exit: ExitCode::SUCCESS,
            }),
        Some(Command::Deprecate {
            locator,
            replacement,
            undo,
        }) => lifecycle::deprecate(&locator, replacement.as_deref(), undo)
            .await
            .map(|outcome| CommandOutput {
                text: if outcome.deprecated {
                    outcome
                        .replacement
                        .as_ref()
                        .map(|replacement| {
                            format!("Deprecated {}; use {replacement}", outcome.locator)
                        })
                        .unwrap_or_else(|| format!("Deprecated {}", outcome.locator))
                } else {
                    format!("Undeprecated {}", outcome.locator)
                },
                payload: ResultPayload::Deprecate { outcome },
                exit: ExitCode::SUCCESS,
            }),
        Some(Command::Usage) => lifecycle::usage().await.map(|outcome| CommandOutput {
            text: usage_text(&outcome),
            payload: ResultPayload::Usage { outcome },
            exit: ExitCode::SUCCESS,
        }),
        Some(Command::History {
            locator: Some(locator),
            command: None,
        }) => release::history(&locator).await.map(|outcome| {
            let text = history_text(&outcome);
            CommandOutput {
                text,
                payload: ResultPayload::History { outcome },
                exit: ExitCode::SUCCESS,
            }
        }),
        Some(Command::History {
            locator: None,
            command: Some(HistoryCommand::Prune { locator, yes }),
        }) => lifecycle::prune_history(&locator, cli.json, yes)
            .await
            .map(|outcome| CommandOutput {
                text: format!(
                    "Pruned {} private revisions from {} ({} bytes reclaimed)",
                    outcome.pruned_revisions, outcome.locator, outcome.reclaimed_bytes
                ),
                payload: ResultPayload::HistoryPrune { outcome },
                exit: ExitCode::SUCCESS,
            }),
        Some(Command::History { .. }) => Err(RuntimeError::new(
            CliErrorCode::InvalidArguments,
            "use `denju history @owner/skill` or `denju history prune @owner/skill`",
        )),
        Some(Command::Diff {
            locator,
            revision_a,
            revision_b,
        }) => release::diff(&locator, revision_a.as_deref(), revision_b.as_deref())
            .await
            .map(|outcome| {
                let text = diff_text(&outcome);
                CommandOutput {
                    text,
                    payload: ResultPayload::Diff { outcome },
                    exit: ExitCode::SUCCESS,
                }
            }),
        Some(Command::Restore { locator, revision }) => release::restore(&locator, &revision)
            .await
            .map(|outcome| CommandOutput {
                text: format!(
                    "Restored {} as new revision {}",
                    locator,
                    short_revision(&outcome.revision.revision_id)
                ),
                payload: ResultPayload::Restore { outcome },
                exit: ExitCode::SUCCESS,
            }),
        Some(Command::Export {
            locator,
            destination,
        }) => release::export(&locator, &destination)
            .await
            .map(|outcome| CommandOutput {
                text: format!(
                    "Exported {} to {}",
                    outcome.locator,
                    outcome.destination.display()
                ),
                payload: ResultPayload::Export { outcome },
                exit: ExitCode::SUCCESS,
            }),
        Some(Command::Subscribe {
            locator,
            release_version,
            retain_on_delete,
        }) => public::subscribe(&locator, release_version, retain_on_delete)
            .await
            .map(|outcome| CommandOutput {
                text: append_deprecation_notice(
                    format!(
                        "Subscribed {}{} as {}",
                        outcome.skill.locator,
                        release_version
                            .map(|value| format!(" at v{value}"))
                            .unwrap_or_default(),
                        outcome.harness_name
                    ),
                    outcome.skill.deprecation.as_ref(),
                ),
                payload: ResultPayload::Subscribe { outcome },
                exit: ExitCode::SUCCESS,
            }),
        Some(Command::Unsubscribe { locator }) => {
            public::unsubscribe(&locator)
                .await
                .map(|outcome| CommandOutput {
                    text: format!("Unsubscribed {}", outcome.locator),
                    payload: ResultPayload::Unsubscribe { outcome },
                    exit: ExitCode::SUCCESS,
                })
        }
        Some(Command::Status) => workspace::status().await.map(|outcome| CommandOutput {
            text: status_text(&outcome),
            payload: ResultPayload::Status { outcome },
            exit: ExitCode::SUCCESS,
        }),
        Some(Command::Sync) => public::sync_once().await.map(|outcome| CommandOutput {
            text: sync_text(&outcome),
            payload: ResultPayload::Sync { outcome },
            exit: ExitCode::SUCCESS,
        }),
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
                next_command: Some("denju setup".to_owned()),
            },
            text: "Denju is ready to set up.\nNext: denju setup".to_owned(),
            exit: ExitCode::SUCCESS,
        },
        Guidance::RepairRequired => CommandOutput {
            payload: ResultPayload::Guidance {
                state: "repair_required",
                next_command: Some("denju doctor".to_owned()),
            },
            text: "Denju needs repair.\nNext: denju doctor".to_owned(),
            exit: ExitCode::SUCCESS,
        },
        Guidance::ClaimAvailable => CommandOutput {
            payload: ResultPayload::Guidance {
                state: "identity_available",
                next_command: Some("denju claim @username".to_owned()),
            },
            text: "Denju is healthy.\nNext: denju claim @username".to_owned(),
            exit: ExitCode::SUCCESS,
        },
        Guidance::LoginRequired(username) => {
            let next = format!("denju login {username}");
            CommandOutput {
                payload: ResultPayload::Guidance {
                    state: "login_required",
                    next_command: Some(next.clone()),
                },
                text: format!("Denju is healthy, but {username} is logged out.\nNext: {next}"),
                exit: ExitCode::SUCCESS,
            }
        }
        Guidance::Conflict(locator) => CommandOutput {
            payload: ResultPayload::Guidance {
                state: "conflict",
                next_command: Some("denju status".to_owned()),
            },
            text: format!("{locator} needs conflict resolution.\nNext: denju status"),
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

fn status_text(outcome: &workspace::StatusOutcome) -> String {
    if outcome.resources.is_empty() {
        return "Denju is healthy.".to_owned();
    }
    let mut lines = Vec::new();
    for resource in &outcome.resources {
        lines.push(format!("{}: {}", resource.locator, resource.state.as_str()));
        if let Some(message) = &resource.message {
            lines.push(format!("  {message}"));
        }
        if let Some(conflict) = &resource.conflict {
            if !conflict.conflict_paths.is_empty() {
                lines.push(format!(
                    "  Conflicts: {}",
                    conflict.conflict_paths.join(", ")
                ));
            }
            lines.push(format!(
                "  Heads: {} {}",
                conflict.head_revision_ids[0], conflict.head_revision_ids[1]
            ));
        }
        for command in &resource.next_commands {
            lines.push(format!("  Next: {command}"));
        }
    }
    lines.join("\n")
}

fn claim_text(outcome: &ClaimOutcome) -> String {
    format!(
        "Claimed {}.\nRecovery secret: {}\nStore this secret now; Denju cannot show it again.",
        outcome.identity.username, outcome.recovery_secret
    )
}

fn login_text(outcome: &LoginOutcome) -> String {
    format!("Logged in as {}.", outcome.identity.username)
}

fn backup_text(outcome: &BackupOutcome) -> String {
    format!(
        "Recovery secret replaced.\nRecovery secret: {}\nStore this secret now; the previous secret no longer works.",
        outcome.recovery_secret
    )
}

fn recovery_text(outcome: &RecoveryOutcome) -> String {
    format!(
        "Recovered {}.\nRecovery secret: {}\nStore this replacement secret now.",
        outcome.identity.username, outcome.recovery_secret
    )
}

fn devices_text(outcome: &DeviceList) -> String {
    if outcome.devices.is_empty() {
        return "No active devices.".to_owned();
    }
    outcome
        .devices
        .iter()
        .map(|device| {
            format!(
                "{}{}  {}",
                device.session_id,
                if device.current { " *" } else { "" },
                device.device_name
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn tokens_text(outcome: &AutomationTokenList) -> String {
    if outcome.tokens.is_empty() {
        return "No active automation tokens.".to_owned();
    }
    outcome
        .tokens
        .iter()
        .map(|token| format!("{}  {}", token.token_id, token.scopes.join(",")))
        .collect::<Vec<_>>()
        .join("\n")
}

fn token_text(outcome: &AutomationTokenOutcome) -> String {
    format!(
        "Created token {}.\nToken secret: {}\nStore this secret now; Denju cannot show it again.",
        outcome.token.token_id, outcome.secret
    )
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
