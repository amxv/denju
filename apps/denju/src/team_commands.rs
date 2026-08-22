use std::str::FromStr;

use denju_core::{OperationId, ResourceKind, ResourceLocator};
use denju_wire::{
    CliErrorCode, ResourceTransferRequest, ResourceTransferResponse, TeamCreateRequest, TeamDetail,
    TeamInviteRequest, TeamInviteResponse, TeamInviteRevokeRequest, TeamJoinRequest, TeamList,
    TeamMemberRemoveRequest, TeamMemberRoleRequest, TeamMutationResponse, TeamRole,
    TeamSettingsRequest, invite_code_hash, resource_transfer_request_hash,
    team_create_request_hash, team_invite_request_hash, team_invite_revoke_request_hash,
    team_join_request_hash, team_member_remove_request_hash, team_member_role_request_hash,
    team_settings_request_hash,
};
use uuid::Uuid;

use crate::{
    CommandOutput, ResultPayload, commands::TeamCommand, public::installed_context,
    setup::RuntimeError,
};
use std::process::ExitCode;

#[derive(Debug)]
pub(crate) struct InviteOutcome {
    pub(crate) invite: TeamInviteResponse,
    pub(crate) code: String,
}

pub(crate) async fn dispatch(command: Option<TeamCommand>) -> Result<CommandOutput, RuntimeError> {
    match command {
        None => list().await.map(|outcome| {
            let text = if outcome.teams.is_empty() {
                "No teams.".to_owned()
            } else {
                outcome
                    .teams
                    .iter()
                    .map(|team| {
                        format!(
                            "{}  {}{}",
                            team.team,
                            team.role.as_str(),
                            if team.members_can_publish {
                                "  members-can-publish"
                            } else {
                                ""
                            }
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("\n")
            };
            CommandOutput {
                text,
                payload: ResultPayload::Teams { outcome },
                exit: ExitCode::SUCCESS,
            }
        }),
        Some(TeamCommand::Create { team }) => create(&team).await.map(|outcome| CommandOutput {
            text: format!("Created {}", outcome.team.team),
            payload: ResultPayload::TeamMutation { outcome },
            exit: ExitCode::SUCCESS,
        }),
        Some(TeamCommand::Invite { team, role }) => {
            invite(&team, role.to_wire()).await.map(|outcome| {
                let join_command = format!("denju team join {}", outcome.code);
                CommandOutput {
                    text: format!(
                        "Invite {} ({})\n{}",
                        outcome.invite.invite_id,
                        outcome.invite.role.as_str(),
                        join_command
                    ),
                    payload: ResultPayload::TeamInvite {
                        invite_id: outcome.invite.invite_id,
                        team: outcome.invite.team,
                        role: outcome.invite.role,
                        expires_at_unix_seconds: outcome.invite.expires_at_unix_seconds,
                        join_command,
                    },
                    exit: ExitCode::SUCCESS,
                }
            })
        }
        Some(TeamCommand::InviteRevoke { team, invite_id }) => revoke_invite(&team, &invite_id)
            .await
            .map(|outcome| CommandOutput {
                text: format!("Revoked invite {invite_id} for {}", outcome.team.team),
                payload: ResultPayload::TeamMutation { outcome },
                exit: ExitCode::SUCCESS,
            }),
        Some(TeamCommand::Join { code }) => join(&code).await.map(|outcome| CommandOutput {
            text: format!("Joined {}", outcome.team.team),
            payload: ResultPayload::TeamMutation { outcome },
            exit: ExitCode::SUCCESS,
        }),
        Some(TeamCommand::Show { team }) => show(&team).await.map(|outcome| {
            let mut lines = vec![format!(
                "{}  {}{}",
                outcome.team.team,
                outcome.team.role.as_str(),
                if outcome.team.members_can_publish {
                    "  members-can-publish"
                } else {
                    ""
                }
            )];
            lines.extend(
                outcome
                    .members
                    .iter()
                    .map(|member| format!("{}  {}", member.username, member.role.as_str())),
            );
            CommandOutput {
                text: lines.join("\n"),
                payload: ResultPayload::Team { outcome },
                exit: ExitCode::SUCCESS,
            }
        }),
        Some(TeamCommand::Role {
            team,
            member,
            role: requested_role,
        }) => role(&team, &member, requested_role.to_wire())
            .await
            .map(|outcome| CommandOutput {
                text: format!("Updated {} membership", outcome.team.team),
                payload: ResultPayload::TeamMutation { outcome },
                exit: ExitCode::SUCCESS,
            }),
        Some(TeamCommand::Remove { team, member }) => {
            remove(&team, &member).await.map(|outcome| CommandOutput {
                text: format!("Removed {member} from {}", outcome.team.team),
                payload: ResultPayload::TeamMutation { outcome },
                exit: ExitCode::SUCCESS,
            })
        }
        Some(TeamCommand::Settings {
            team,
            members_can_publish,
        }) => settings(&team, members_can_publish)
            .await
            .map(|outcome| CommandOutput {
                text: format!(
                    "{} members-can-publish={}",
                    outcome.team.team, outcome.team.members_can_publish
                ),
                payload: ResultPayload::TeamMutation { outcome },
                exit: ExitCode::SUCCESS,
            }),
    }
}

pub(crate) async fn dispatch_transfer(
    locator: &str,
    team: &str,
) -> Result<CommandOutput, RuntimeError> {
    transfer(locator, team).await.map(|outcome| CommandOutput {
        text: format!(
            "Transferred {} to {}",
            outcome.old_locator, outcome.new_locator
        ),
        payload: ResultPayload::Transfer { outcome },
        exit: ExitCode::SUCCESS,
    })
}

pub(crate) async fn list() -> Result<TeamList, RuntimeError> {
    installed_context(true)
        .await?
        .client
        .teams()
        .await
        .map_err(client_error)
}

pub(crate) async fn show(team: &str) -> Result<TeamDetail, RuntimeError> {
    installed_context(true)
        .await?
        .client
        .team_detail(team)
        .await
        .map_err(client_error)
}

pub(crate) async fn create(team: &str) -> Result<TeamMutationResponse, RuntimeError> {
    let context = installed_context(true).await?;
    let name = normalize_namespace(team)?;
    let operation_id = new_operation()?.to_string();
    let request_hash = team_create_request_hash(&operation_id, &name).map_err(hash_error)?;
    context
        .client
        .create_team(&TeamCreateRequest {
            operation_id,
            name,
            request_hash: request_hash.to_string(),
        })
        .await
        .map_err(client_error)
}

pub(crate) async fn invite(team: &str, role: TeamRole) -> Result<InviteOutcome, RuntimeError> {
    let context = installed_context(true).await?;
    let team = canonical_namespace(team)?;
    let code = hex::encode(rand::random::<[u8; 16]>());
    let code_hash = invite_code_hash(&code).to_string();
    let operation_id = new_operation()?.to_string();
    let request_hash =
        team_invite_request_hash(&operation_id, &team, role, &code_hash).map_err(hash_error)?;
    let invite = context
        .client
        .create_team_invite(&TeamInviteRequest {
            operation_id,
            team,
            role,
            invite_code_hash: code_hash,
            request_hash: request_hash.to_string(),
        })
        .await
        .map_err(client_error)?;
    Ok(InviteOutcome { invite, code })
}

pub(crate) async fn revoke_invite(
    team: &str,
    invite_id: &str,
) -> Result<TeamMutationResponse, RuntimeError> {
    Uuid::parse_str(invite_id)
        .map_err(|error| RuntimeError::new(CliErrorCode::InvalidArguments, error.to_string()))?;
    let context = installed_context(true).await?;
    let team = canonical_namespace(team)?;
    let operation_id = new_operation()?.to_string();
    let request_hash =
        team_invite_revoke_request_hash(&operation_id, &team, invite_id).map_err(hash_error)?;
    context
        .client
        .revoke_team_invite(&TeamInviteRevokeRequest {
            operation_id,
            team,
            invite_id: invite_id.to_owned(),
            request_hash: request_hash.to_string(),
        })
        .await
        .map_err(client_error)
}

pub(crate) async fn join(code: &str) -> Result<TeamMutationResponse, RuntimeError> {
    if code.is_empty() {
        return Err(RuntimeError::new(
            CliErrorCode::InvalidArguments,
            "team invite code cannot be empty",
        ));
    }
    let context = installed_context(true).await?;
    let operation_id = new_operation()?.to_string();
    let request_hash = team_join_request_hash(&operation_id, code).map_err(hash_error)?;
    context
        .client
        .join_team(&TeamJoinRequest {
            operation_id,
            code: code.to_owned(),
            request_hash: request_hash.to_string(),
        })
        .await
        .map_err(client_error)
}

pub(crate) async fn role(
    team: &str,
    member: &str,
    role: TeamRole,
) -> Result<TeamMutationResponse, RuntimeError> {
    let context = installed_context(true).await?;
    let team = canonical_namespace(team)?;
    let member = canonical_namespace(member)?;
    let operation_id = new_operation()?.to_string();
    let request_hash =
        team_member_role_request_hash(&operation_id, &team, &member, role).map_err(hash_error)?;
    context
        .client
        .change_team_member_role(&TeamMemberRoleRequest {
            operation_id,
            team,
            member,
            role,
            request_hash: request_hash.to_string(),
        })
        .await
        .map_err(client_error)
}

pub(crate) async fn remove(team: &str, member: &str) -> Result<TeamMutationResponse, RuntimeError> {
    let context = installed_context(true).await?;
    let team = canonical_namespace(team)?;
    let member = canonical_namespace(member)?;
    let operation_id = new_operation()?.to_string();
    let request_hash =
        team_member_remove_request_hash(&operation_id, &team, &member).map_err(hash_error)?;
    context
        .client
        .remove_team_member(&TeamMemberRemoveRequest {
            operation_id,
            team,
            member,
            request_hash: request_hash.to_string(),
        })
        .await
        .map_err(client_error)
}

pub(crate) async fn settings(
    team: &str,
    members_can_publish: bool,
) -> Result<TeamMutationResponse, RuntimeError> {
    let context = installed_context(true).await?;
    let team = canonical_namespace(team)?;
    let operation_id = new_operation()?.to_string();
    let request_hash = team_settings_request_hash(&operation_id, &team, members_can_publish)
        .map_err(hash_error)?;
    context
        .client
        .update_team_settings(&TeamSettingsRequest {
            operation_id,
            team,
            members_can_publish,
            request_hash: request_hash.to_string(),
        })
        .await
        .map_err(client_error)
}

pub(crate) async fn transfer(
    locator: &str,
    team: &str,
) -> Result<ResourceTransferResponse, RuntimeError> {
    // A namespace transfer changes future write authorization. Settle any already-queued
    // personal workspace revisions first so the move cannot strand a valid local save under
    // the former namespace.
    crate::public::sync_once().await?;
    let context = installed_context(true).await?;
    let locator = ResourceLocator::from_str(locator)
        .map_err(|error| RuntimeError::new(CliErrorCode::InvalidArguments, error.to_string()))?;
    let team = canonical_namespace(team)?;
    let (resource_id, generation) = match locator.kind() {
        ResourceKind::Pack => {
            let pack = context
                .client
                .pack_detail(&locator.to_string())
                .await
                .map_err(client_error)?;
            (pack.pack.resource_id, pack.pack.generation)
        }
        ResourceKind::Skill => {
            let owned = context
                .client
                .private_skills()
                .await
                .map_err(client_error)?
                .skills
                .into_iter()
                .find(|skill| skill.locator == locator.to_string())
                .ok_or_else(|| {
                    RuntimeError::new(
                        CliErrorCode::NotFound,
                        format!("{} is not a personally owned skill", locator),
                    )
                })?;
            (owned.resource_id, owned.generation)
        }
    };
    let operation_id = new_operation()?.to_string();
    let request_hash =
        resource_transfer_request_hash(&operation_id, &resource_id, generation, &team)
            .map_err(hash_error)?;
    let outcome = context
        .client
        .transfer_resource(&ResourceTransferRequest {
            operation_id,
            resource_id,
            expected_generation: generation,
            destination_team: team,
            request_hash: request_hash.to_string(),
        })
        .await
        .map_err(client_error)?;
    crate::public::sync_once().await?;
    Ok(outcome)
}

fn normalize_namespace(value: &str) -> Result<String, RuntimeError> {
    Ok(canonical_namespace(value)?
        .trim_start_matches('@')
        .to_owned())
}

fn canonical_namespace(value: &str) -> Result<String, RuntimeError> {
    let name = value.trim_start_matches('@');
    let locator = ResourceLocator::from_str(&format!("@{name}/validation"))
        .map_err(|error| RuntimeError::new(CliErrorCode::InvalidArguments, error.to_string()))?;
    Ok(format!("@{}", locator.owner()))
}

fn new_operation() -> Result<OperationId, RuntimeError> {
    OperationId::from_uuid(Uuid::now_v7())
        .map_err(|error| RuntimeError::new(CliErrorCode::Internal, error.to_string()))
}

fn client_error(error: denju_client::ClientError) -> RuntimeError {
    crate::public::client_error(error)
}

fn hash_error(error: impl std::fmt::Display) -> RuntimeError {
    RuntimeError::new(CliErrorCode::Internal, error.to_string())
}
