mod commands;
mod context;
mod discovery;
mod fork_ops;
mod fork_resolve;
mod fork_sync;
mod forks;
mod guidance;
mod help;
mod identity;
mod lifecycle;
mod list;
mod output;
mod owned;
mod owned_sync;
mod pack_commands;
mod pack_sync;
mod proposals;
mod public;
mod quarantine_sync;
mod release;
mod result;
mod setup;
mod sharing;
mod team_commands;
mod upgrade;
mod workspace;
mod workspace_merge;

use std::{ffi::OsString, process::ExitCode};

use clap::Parser;
use commands::{
    Cli, Command, DevicesCommand, ForkCommand, HistoryCommand, IdentityCommand, PackCommand,
    ProposalCommand, TokenCommand,
};
use denju_wire::{CliEnvelope, CliError, CliErrorCode};
use guidance::guidance_output;
use help::HELP;
use output::{
    append_deprecation_notice, backup_text, claim_text, devices_text, diff_text, doctor_text,
    history_text, login_text, proposal_detail_text, proposal_text, proposals_text, recovery_text,
    search_text, setup_text, short_revision, show_text, status_text, sync_text, token_text,
    tokens_text, usage_text,
};
use serde::Serialize;
use setup::RuntimeError;

pub(crate) use result::{CommandOutput, ResultPayload};

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
            command:
                Some(IdentityCommand::Update {
                    bio,
                    clear_bio,
                    followers_visible,
                    following_visible,
                }),
        }) => discovery::update_profile(bio, clear_bio, followers_visible, following_visible)
            .await
            .map(|outcome| CommandOutput {
                text: format!("Updated {} profile.", outcome.username),
                payload: ResultPayload::ProfileUpdate { outcome },
                exit: ExitCode::SUCCESS,
            }),
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
        Some(Command::Search {
            query,
            sort,
            following,
            topic,
            limit,
            cursor,
        }) => discovery::search(
            &query,
            sort.to_wire(),
            following,
            topic.as_deref(),
            limit,
            cursor.as_deref(),
        )
        .await
        .map(|outcome| CommandOutput {
            text: search_text(&outcome),
            payload: ResultPayload::Search { outcome },
            exit: ExitCode::SUCCESS,
        }),
        Some(Command::Top {
            topic,
            limit,
            cursor,
        }) => discovery::top(topic.as_deref(), limit, cursor.as_deref())
            .await
            .map(|outcome| CommandOutput {
                text: search_text(&outcome),
                payload: ResultPayload::Top { outcome },
                exit: ExitCode::SUCCESS,
            }),
        Some(Command::Show {
            locator,
            followers_cursor,
            following_cursor,
        }) => discovery::show(
            &locator,
            followers_cursor.as_deref(),
            following_cursor.as_deref(),
        )
        .await
        .map(|outcome| CommandOutput {
            text: show_text(&outcome),
            payload: ResultPayload::Show { outcome },
            exit: ExitCode::SUCCESS,
        }),
        Some(Command::Follow { username }) => {
            discovery::follow(&username)
                .await
                .map(|outcome| CommandOutput {
                    text: if outcome.synchronized {
                        format!("Following {}.", outcome.username)
                    } else {
                        format!(
                            "Following {} locally; it will sync after identity claim.",
                            outcome.username
                        )
                    },
                    payload: ResultPayload::Follow { outcome },
                    exit: ExitCode::SUCCESS,
                })
        }
        Some(Command::Unfollow { username }) => {
            discovery::unfollow(&username)
                .await
                .map(|outcome| CommandOutput {
                    text: format!("Not following {}.", outcome.username),
                    payload: ResultPayload::Follow { outcome },
                    exit: ExitCode::SUCCESS,
                })
        }
        Some(Command::Star { locator }) => {
            discovery::star(&locator)
                .await
                .map(|outcome| CommandOutput {
                    text: format!(
                        "Starred {} ({} stars).",
                        outcome.locator, outcome.star_count
                    ),
                    payload: ResultPayload::Star { outcome },
                    exit: ExitCode::SUCCESS,
                })
        }
        Some(Command::Unstar { locator }) => {
            discovery::unstar(&locator)
                .await
                .map(|outcome| CommandOutput {
                    text: format!(
                        "Unstarred {} ({} stars).",
                        outcome.locator, outcome.star_count
                    ),
                    payload: ResultPayload::Star { outcome },
                    exit: ExitCode::SUCCESS,
                })
        }
        Some(Command::Topics { locator, topics }) => discovery::update_topics(&locator, topics)
            .await
            .map(|outcome| CommandOutput {
                text: if outcome.topics.is_empty() {
                    format!("Cleared discovery topics for {}.", outcome.locator)
                } else {
                    format!(
                        "Topics for {}: {}",
                        outcome.locator,
                        outcome.topics.join(", ")
                    )
                },
                payload: ResultPayload::Topics { outcome },
                exit: ExitCode::SUCCESS,
            }),
        Some(Command::Report { locator, reason }) => discovery::report(&locator, &reason)
            .await
            .map(|outcome| CommandOutput {
                text: format!("Report {} accepted.", outcome.report_id),
                payload: ResultPayload::Report { outcome },
                exit: ExitCode::SUCCESS,
            }),
        Some(Command::Import { path, to }) => {
            owned::import(&path, to.as_deref())
                .await
                .map(|outcome| CommandOutput {
                    text: format!("Imported {} as {}", outcome.locator, outcome.harness_name),
                    payload: ResultPayload::Import { outcome },
                    exit: ExitCode::SUCCESS,
                })
        }
        Some(Command::Publish {
            locator,
            public,
            message,
            tags,
        }) => {
            if pack_commands::is_pack_locator(&locator) {
                if message.is_some() || !tags.is_empty() {
                    Err(RuntimeError::new(
                        CliErrorCode::InvalidArguments,
                        "pack publish does not accept skill release messages or tags",
                    ))
                } else {
                    pack_commands::publish(&locator, public)
                        .await
                        .map(|outcome| CommandOutput {
                            text: format!(
                                "Published {} v{}",
                                outcome.pack.locator, outcome.pack.version
                            ),
                            payload: ResultPayload::PackMutation { outcome },
                            exit: ExitCode::SUCCESS,
                        })
                }
            } else {
                release::publish(&locator, public, message, tags)
                    .await
                    .map(|outcome| CommandOutput {
                        text: format!(
                            "Published {} v{}",
                            outcome.skill.locator, outcome.release.version
                        ),
                        payload: ResultPayload::Publish { outcome },
                        exit: ExitCode::SUCCESS,
                    })
            }
        }
        Some(Command::Rename { locator, new_name }) => {
            if pack_commands::is_pack_locator(&locator) {
                pack_commands::rename(&locator, &new_name)
                    .await
                    .map(|outcome| CommandOutput {
                        text: format!(
                            "Renamed {} to {}",
                            outcome.old_locator.as_deref().unwrap_or(&locator),
                            outcome.pack.locator
                        ),
                        payload: ResultPayload::PackLifecycle { outcome },
                        exit: ExitCode::SUCCESS,
                    })
            } else {
                lifecycle::rename(&locator, &new_name)
                    .await
                    .map(|outcome| CommandOutput {
                        text: format!("Renamed {} to {}", outcome.old_locator, outcome.locator),
                        payload: ResultPayload::Rename { outcome },
                        exit: ExitCode::SUCCESS,
                    })
            }
        }
        Some(Command::Unpublish { locator }) => {
            if pack_commands::is_pack_locator(&locator) {
                pack_commands::unpublish(&locator)
                    .await
                    .map(|outcome| CommandOutput {
                        text: format!("Unpublished {}", outcome.pack.locator),
                        payload: ResultPayload::PackLifecycle { outcome },
                        exit: ExitCode::SUCCESS,
                    })
            } else {
                lifecycle::unpublish(&locator)
                    .await
                    .map(|outcome| CommandOutput {
                        text: format!("Unpublished {}", outcome.locator),
                        payload: ResultPayload::Unpublish { outcome },
                        exit: ExitCode::SUCCESS,
                    })
            }
        }
        Some(Command::Delete { locator, yes }) => {
            if pack_commands::is_pack_locator(&locator) {
                pack_commands::delete(&locator, cli.json, yes)
                    .await
                    .map(|outcome| CommandOutput {
                        text: format!("Deleted {}", outcome.pack.locator),
                        payload: ResultPayload::PackLifecycle { outcome },
                        exit: ExitCode::SUCCESS,
                    })
            } else {
                lifecycle::delete(&locator, cli.json, yes)
                    .await
                    .map(|outcome| CommandOutput {
                        text: format!("Deleted {}", outcome.locator),
                        payload: ResultPayload::Delete { outcome },
                        exit: ExitCode::SUCCESS,
                    })
            }
        }
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
        Some(Command::List) => list::list().await.map(|outcome| CommandOutput {
            text: list::list_text(&outcome),
            payload: ResultPayload::List { outcome },
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
        }) => {
            if pack_commands::is_pack_locator(&locator) {
                if release_version.is_some() || retain_on_delete {
                    Err(RuntimeError::new(
                        CliErrorCode::InvalidArguments,
                        "pack subscriptions follow the live pack and do not accept --version or --retain-on-delete",
                    ))
                } else {
                    pack_commands::subscribe(&locator)
                        .await
                        .map(|outcome| CommandOutput {
                            text: format!(
                                "Subscribed {} at pack v{}",
                                outcome.locator, outcome.version
                            ),
                            payload: ResultPayload::PackSubscription { outcome },
                            exit: ExitCode::SUCCESS,
                        })
                }
            } else {
                public::subscribe(&locator, release_version, retain_on_delete)
                    .await
                    .map(|outcome| CommandOutput {
                        text: append_deprecation_notice(
                            format!(
                                "Subscribed {}{} as {}",
                                outcome.locator,
                                outcome
                                    .release_version
                                    .map(|value| format!(" at v{value}"))
                                    .unwrap_or_else(|| " to live private saves".to_owned()),
                                outcome.harness_name
                            ),
                            outcome.deprecation.as_ref(),
                        ),
                        payload: ResultPayload::Subscribe { outcome },
                        exit: ExitCode::SUCCESS,
                    })
            }
        }
        Some(Command::Unsubscribe { locator }) => {
            if pack_commands::is_pack_locator(&locator) {
                pack_commands::unsubscribe(&locator)
                    .await
                    .map(|outcome| CommandOutput {
                        text: format!("Unsubscribed {}", outcome.locator),
                        payload: ResultPayload::PackSubscription { outcome },
                        exit: ExitCode::SUCCESS,
                    })
            } else {
                public::unsubscribe(&locator)
                    .await
                    .map(|outcome| CommandOutput {
                        text: format!("Unsubscribed {}", outcome.locator),
                        payload: ResultPayload::Unsubscribe { outcome },
                        exit: ExitCode::SUCCESS,
                    })
            }
        }
        Some(Command::Share { locator, recipient }) => {
            sharing::mutate(&locator, &recipient, denju_wire::ShareMutationKind::Share)
                .await
                .map(|outcome| CommandOutput {
                    text: format!(
                        "Shared {} with {}.\n{}",
                        outcome.locator,
                        outcome.recipient,
                        outcome.subscribe_command.as_deref().unwrap_or_default()
                    ),
                    payload: ResultPayload::Share { outcome },
                    exit: ExitCode::SUCCESS,
                })
        }
        Some(Command::Unshare { locator, recipient }) => {
            sharing::mutate(&locator, &recipient, denju_wire::ShareMutationKind::Unshare)
                .await
                .map(|outcome| CommandOutput {
                    text: format!("Unshared {} from {}", outcome.locator, outcome.recipient),
                    payload: ResultPayload::Unshare { outcome },
                    exit: ExitCode::SUCCESS,
                })
        }
        Some(Command::Fork {
            locator: Some(locator),
            command: None,
        }) => fork_ops::create(&locator)
            .await
            .map(|outcome| CommandOutput {
                text: format!("Forked {} as {}", outcome.upstream_locator, outcome.locator),
                payload: ResultPayload::Fork { outcome },
                exit: ExitCode::SUCCESS,
            }),
        Some(Command::Fork {
            locator: None,
            command: Some(ForkCommand::Sync { locator }),
        }) => fork_sync::sync(&locator)
            .await
            .map(|outcome| CommandOutput {
                text: if outcome.state == "current" {
                    format!("{} is already current with upstream", outcome.locator)
                } else {
                    format!(
                        "Synced {} from {} at {}",
                        outcome.locator,
                        outcome.upstream_locator,
                        short_revision(&outcome.upstream_revision_id)
                    )
                },
                payload: ResultPayload::ForkSync { outcome },
                exit: ExitCode::SUCCESS,
            }),
        Some(Command::Fork {
            locator: None,
            command:
                Some(ForkCommand::Resolve {
                    locator,
                    as_name,
                    merge_into,
                    discard,
                }),
        }) => fork_resolve::resolve(&locator, as_name.as_deref(), merge_into.as_deref(), discard)
            .await
            .map(|outcome| CommandOutput {
                text: match (outcome.state, outcome.locator.as_deref()) {
                    ("discarded", _) => {
                        format!("Discarded local fork of {}", outcome.upstream_locator)
                    }
                    ("renamed", Some(locator)) => {
                        format!("Resolved fork collision as {locator}")
                    }
                    ("merged", Some(locator)) => {
                        format!("Merged local fork into {locator}")
                    }
                    _ => "Resolved fork collision".to_owned(),
                },
                payload: ResultPayload::ForkResolve { outcome },
                exit: ExitCode::SUCCESS,
            }),
        Some(Command::Fork { .. }) => Err(RuntimeError::new(
            CliErrorCode::InvalidArguments,
            "use `denju fork @owner/skill`, `denju fork sync @you/skill`, or `denju fork resolve @upstream/skill ...`",
        )),
        Some(Command::Propose { locator, message }) => {
            proposals::create(&locator, message.as_deref())
                .await
                .map(|outcome| CommandOutput {
                    text: proposal_text(&outcome),
                    payload: ResultPayload::Proposal { outcome },
                    exit: ExitCode::SUCCESS,
                })
        }
        Some(Command::Proposals) => proposals::list().await.map(|outcome| CommandOutput {
            text: proposals_text(&outcome),
            payload: ResultPayload::Proposals { outcome },
            exit: ExitCode::SUCCESS,
        }),
        Some(Command::Proposal {
            command: ProposalCommand::Show { proposal_id },
        }) => proposals::show(&proposal_id)
            .await
            .map(|outcome| CommandOutput {
                text: proposal_detail_text(&outcome),
                payload: ResultPayload::ProposalDetail { outcome },
                exit: ExitCode::SUCCESS,
            }),
        Some(Command::Proposal {
            command: ProposalCommand::Accept { proposal_id },
        }) => proposals::accept(&proposal_id)
            .await
            .map(|outcome| CommandOutput {
                text: proposal_text(&outcome),
                payload: ResultPayload::Proposal { outcome },
                exit: ExitCode::SUCCESS,
            }),
        Some(Command::Proposal {
            command: ProposalCommand::Reject { proposal_id },
        }) => proposals::reject(&proposal_id)
            .await
            .map(|outcome| CommandOutput {
                text: proposal_text(&outcome),
                payload: ResultPayload::Proposal { outcome },
                exit: ExitCode::SUCCESS,
            }),
        Some(Command::Proposal {
            command: ProposalCommand::Withdraw { proposal_id },
        }) => proposals::withdraw(&proposal_id)
            .await
            .map(|outcome| CommandOutput {
                text: proposal_text(&outcome),
                payload: ResultPayload::Proposal { outcome },
                exit: ExitCode::SUCCESS,
            }),
        Some(Command::Pack {
            command: PackCommand::Create { locator },
        }) => pack_commands::create(&locator)
            .await
            .map(|outcome| CommandOutput {
                text: format!("Created {} v{}", outcome.pack.locator, outcome.pack.version),
                payload: ResultPayload::PackCreate { outcome },
                exit: ExitCode::SUCCESS,
            }),
        Some(Command::Pack {
            command: PackCommand::Add { locator, skills },
        }) => pack_commands::mutate(&locator, &skills, denju_wire::PackMutationKind::Add)
            .await
            .map(|outcome| CommandOutput {
                text: format!(
                    "Updated {} to v{}",
                    outcome.pack.locator, outcome.pack.version
                ),
                payload: ResultPayload::PackMutation { outcome },
                exit: ExitCode::SUCCESS,
            }),
        Some(Command::Pack {
            command: PackCommand::Remove { locator, skills },
        }) => pack_commands::mutate(&locator, &skills, denju_wire::PackMutationKind::Remove)
            .await
            .map(|outcome| CommandOutput {
                text: format!(
                    "Updated {} to v{}",
                    outcome.pack.locator, outcome.pack.version
                ),
                payload: ResultPayload::PackMutation { outcome },
                exit: ExitCode::SUCCESS,
            }),
        Some(Command::Team { command }) => team_commands::dispatch(command, cli.json).await,
        Some(Command::Transfer { locator, team }) => {
            team_commands::dispatch_transfer(&locator, &team).await
        }
        Some(Command::Status) => workspace::status().await.map(|outcome| CommandOutput {
            text: status_text(&outcome),
            payload: ResultPayload::Status { outcome },
            exit: ExitCode::SUCCESS,
        }),
        Some(Command::Sync) => proposals::sync_once().await.map(|outcome| CommandOutput {
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
        Some(Command::Upgrade) => {
            upgrade::upgrade(build_version(), !cli.json)
                .await
                .map(|outcome| CommandOutput {
                    text: upgrade::upgrade_text(&outcome),
                    payload: ResultPayload::Upgrade { outcome },
                    exit: ExitCode::SUCCESS,
                })
        }
        Some(Command::Daemon) => return daemon(cli.json).await,
        Some(Command::UpgradeHealth) => return upgrade_health(cli.json).await,
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

async fn upgrade_health(json: bool) -> ExitCode {
    match upgrade::health_check().await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => present_runtime_error(error, json),
    }
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
