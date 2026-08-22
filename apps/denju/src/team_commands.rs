use std::str::FromStr;

use denju_core::{OperationId, ResourceKind, ResourceLocator};
use denju_wire::{
    CliErrorCode, ResourceTransferRequest, ResourceTransferResponse, TeamCreateRequest,
    TeamDeleteRequest, TeamDeleteResponse, TeamDetail, TeamInviteRequest, TeamInviteResponse,
    TeamInviteRevokeRequest, TeamJoinRequest, TeamLeaveRequest, TeamLeaveResponse, TeamList,
    TeamMemberRemoveRequest, TeamMemberRoleRequest, TeamMutationResponse,
    TeamOwnerTransferAcceptRequest, TeamOwnerTransferRequest, TeamOwnerTransferResponse,
    TeamPackAssignmentMutationKind, TeamPackAssignmentRequest, TeamPackAssignmentResponse,
    TeamRole, TeamSettingsRequest, invite_code_hash, resource_transfer_request_hash,
    team_create_request_hash, team_delete_request_hash, team_invite_request_hash,
    team_invite_revoke_request_hash, team_join_request_hash, team_leave_request_hash,
    team_member_remove_request_hash, team_member_role_request_hash,
    team_owner_transfer_accept_request_hash, team_owner_transfer_code_hash,
    team_owner_transfer_request_hash, team_pack_assignment_request_hash,
    team_settings_request_hash,
};
use uuid::Uuid;

use crate::{
    CommandOutput, ResultPayload,
    commands::TeamCommand,
    identity::{confirm, prompt_password, require_interactive},
    public::installed_context,
    setup::RuntimeError,
};
use std::process::ExitCode;

#[derive(Debug)]
pub(crate) struct InviteOutcome {
    pub(crate) invite: TeamInviteResponse,
    pub(crate) code: String,
}

#[derive(Debug)]
pub(crate) struct OwnerTransferOutcome {
    pub(crate) transfer: TeamOwnerTransferResponse,
    pub(crate) code: String,
}

pub(crate) async fn dispatch(
    command: Option<TeamCommand>,
    json: bool,
) -> Result<CommandOutput, RuntimeError> {
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
            lines.extend(
                outcome
                    .assigned_packs
                    .iter()
                    .map(|assignment| format!("assigned  {}", assignment.pack_locator)),
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
        Some(TeamCommand::Assign { team, pack }) => {
            mutate_assignment(&team, &pack, TeamPackAssignmentMutationKind::Assign)
                .await
                .map(|outcome| CommandOutput {
                    text: format!(
                        "Assigned {} to {}",
                        outcome.assignment.pack_locator, outcome.assignment.team
                    ),
                    payload: ResultPayload::TeamPackAssignment { outcome },
                    exit: ExitCode::SUCCESS,
                })
        }
        Some(TeamCommand::Unassign { team, pack }) => {
            mutate_assignment(&team, &pack, TeamPackAssignmentMutationKind::Unassign)
                .await
                .map(|outcome| CommandOutput {
                    text: format!(
                        "Unassigned {} from {}",
                        outcome.assignment.pack_locator, outcome.assignment.team
                    ),
                    payload: ResultPayload::TeamPackAssignment { outcome },
                    exit: ExitCode::SUCCESS,
                })
        }
        Some(TeamCommand::Leave { team }) => leave(&team).await.map(|outcome| CommandOutput {
            text: format!("Left {}", outcome.team),
            payload: ResultPayload::TeamLeave { outcome },
            exit: ExitCode::SUCCESS,
        }),
        Some(TeamCommand::TransferOwner { team, member }) => {
            transfer_owner(&team, &member).await.map(|outcome| {
                let accept_command = format!("denju team accept-owner {}", outcome.code);
                CommandOutput {
                    text: format!(
                        "Ownership transfer for {} is pending.\n{}",
                        outcome.transfer.team, accept_command
                    ),
                    payload: ResultPayload::TeamOwnerTransfer {
                        outcome: outcome.transfer,
                        accept_command: Some(accept_command),
                    },
                    exit: ExitCode::SUCCESS,
                }
            })
        }
        Some(TeamCommand::AcceptOwner { code }) => {
            accept_owner(&code).await.map(|outcome| CommandOutput {
                text: format!("Accepted ownership of {}", outcome.team),
                payload: ResultPayload::TeamOwnerTransfer {
                    outcome,
                    accept_command: None,
                },
                exit: ExitCode::SUCCESS,
            })
        }
        Some(TeamCommand::Delete { team, yes }) => {
            delete_team(&team, yes, json)
                .await
                .map(|outcome| CommandOutput {
                    text: format!("Deleted {}", outcome.team),
                    payload: ResultPayload::TeamDelete { outcome },
                    exit: ExitCode::SUCCESS,
                })
        }
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
    let outcome = context
        .client
        .join_team(&TeamJoinRequest {
            operation_id,
            code: code.to_owned(),
            request_hash: request_hash.to_string(),
        })
        .await
        .map_err(client_error)?;
    crate::public::sync_once().await?;
    Ok(outcome)
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

pub(crate) async fn mutate_assignment(
    team: &str,
    pack: &str,
    kind: TeamPackAssignmentMutationKind,
) -> Result<TeamPackAssignmentResponse, RuntimeError> {
    let context = installed_context(true).await?;
    let team = canonical_namespace(team)?;
    let locator = ResourceLocator::from_str(pack)
        .map_err(|error| RuntimeError::new(CliErrorCode::InvalidArguments, error.to_string()))?;
    if locator.kind() != ResourceKind::Pack {
        return Err(RuntimeError::new(
            CliErrorCode::InvalidArguments,
            "team assignment requires a pack locator",
        ));
    }
    let pack_resource_id = match kind {
        TeamPackAssignmentMutationKind::Assign => {
            context
                .client
                .pack_detail(&locator.to_string())
                .await
                .map_err(client_error)?
                .pack
                .resource_id
        }
        TeamPackAssignmentMutationKind::Unassign => context
            .client
            .team_detail(&team)
            .await
            .map_err(client_error)?
            .assigned_packs
            .into_iter()
            .find(|assignment| assignment.pack_locator == locator.to_string())
            .map(|assignment| assignment.pack_resource_id)
            .ok_or_else(|| {
                RuntimeError::new(
                    CliErrorCode::NotFound,
                    format!("{} is not assigned to {team}", locator),
                )
            })?,
    };
    let operation_id = new_operation()?.to_string();
    let request_hash =
        team_pack_assignment_request_hash(kind, &operation_id, &team, &pack_resource_id)
            .map_err(hash_error)?;
    let outcome = context
        .client
        .mutate_team_pack_assignment(
            kind,
            &TeamPackAssignmentRequest {
                operation_id,
                team,
                pack_resource_id,
                request_hash: request_hash.to_string(),
            },
        )
        .await
        .map_err(client_error)?;
    crate::public::sync_once().await?;
    Ok(outcome)
}

pub(crate) async fn leave(team: &str) -> Result<TeamLeaveResponse, RuntimeError> {
    let context = installed_context(true).await?;
    let team = canonical_namespace(team)?;
    let operation_id = new_operation()?.to_string();
    let request_hash = team_leave_request_hash(&operation_id, &team).map_err(hash_error)?;
    let outcome = context
        .client
        .leave_team(&TeamLeaveRequest {
            operation_id,
            team,
            request_hash: request_hash.to_string(),
        })
        .await
        .map_err(client_error)?;
    crate::public::sync_once().await?;
    Ok(outcome)
}

pub(crate) async fn transfer_owner(
    team: &str,
    member: &str,
) -> Result<OwnerTransferOutcome, RuntimeError> {
    let context = installed_context(true).await?;
    let team = canonical_namespace(team)?;
    let member = canonical_namespace(member)?;
    // Match invite-code entropy. Acceptance is additionally authenticated and restricted to the
    // nominated current member, but the bearer code still should not be the weak link.
    let code = hex::encode(rand::random::<[u8; 16]>());
    let transfer_code_hash = team_owner_transfer_code_hash(&code).to_string();
    let operation_id = new_operation()?.to_string();
    let request_hash =
        team_owner_transfer_request_hash(&operation_id, &team, &member, &transfer_code_hash)
            .map_err(hash_error)?;
    let transfer = context
        .client
        .create_team_owner_transfer(&TeamOwnerTransferRequest {
            operation_id,
            team,
            member,
            transfer_code_hash,
            request_hash: request_hash.to_string(),
        })
        .await
        .map_err(client_error)?;
    Ok(OwnerTransferOutcome { transfer, code })
}

pub(crate) async fn accept_owner(code: &str) -> Result<TeamOwnerTransferResponse, RuntimeError> {
    if code.is_empty() {
        return Err(RuntimeError::new(
            CliErrorCode::InvalidArguments,
            "ownership transfer code cannot be empty",
        ));
    }
    let context = installed_context(true).await?;
    let operation_id = new_operation()?.to_string();
    let request_hash =
        team_owner_transfer_accept_request_hash(&operation_id, code).map_err(hash_error)?;
    let outcome = context
        .client
        .accept_team_owner_transfer(&TeamOwnerTransferAcceptRequest {
            operation_id,
            code: code.to_owned(),
            request_hash: request_hash.to_string(),
        })
        .await
        .map_err(client_error)?;
    crate::public::sync_once().await?;
    Ok(outcome)
}

pub(crate) async fn delete_team(
    team: &str,
    yes: bool,
    json: bool,
) -> Result<TeamDeleteResponse, RuntimeError> {
    require_interactive(
        json,
        "team deletion requires confirmation and hidden password input",
    )?;
    let team = canonical_namespace(team)?;
    if !yes
        && !confirm(&format!(
            "Delete {team} and all remaining team resources? [y/N] "
        ))?
    {
        return Err(RuntimeError::new(
            CliErrorCode::ConfirmationRequired,
            "team deletion was not confirmed",
        ));
    }
    let password = prompt_password("Password: ")?;
    let context = installed_context(true).await?;
    let operation_id = new_operation()?.to_string();
    let request_hash = team_delete_request_hash(&operation_id, &team).map_err(hash_error)?;
    let outcome = context
        .client
        .delete_team(&TeamDeleteRequest {
            operation_id,
            team,
            password,
            request_hash: request_hash.to_string(),
        })
        .await
        .map_err(client_error)?;
    crate::public::sync_once().await?;
    Ok(outcome)
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
