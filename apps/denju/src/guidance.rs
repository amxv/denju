use std::process::ExitCode;

use crate::setup::Guidance;

use super::{CommandOutput, ResultPayload};

pub(super) fn guidance_output(guidance: Guidance) -> CommandOutput {
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
