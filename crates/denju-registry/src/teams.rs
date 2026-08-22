use std::str::FromStr;

use denju_core::{OperationId, ResourceLocator};
use denju_wire::{
    ApiError, ApiErrorCode, RequestHash, TeamCreateRequest, TeamDetail, TeamInviteRequest,
    TeamInviteResponse, TeamInviteRevokeRequest, TeamJoinRequest, TeamList, TeamMember,
    TeamMemberRemoveRequest, TeamMemberRoleRequest, TeamMutationResponse, TeamRole,
    TeamSettingsRequest, TeamSummary, invite_code_hash, team_create_request_hash,
    team_invite_request_hash, team_invite_revoke_request_hash, team_join_request_hash,
    team_member_remove_request_hash, team_member_role_request_hash, team_settings_request_hash,
};
use serde::{Serialize, de::DeserializeOwned};
use sqlx::Row;
use uuid::Uuid;

use crate::{Registry, internal_api_error};

const INVITE_LIFETIME_SECONDS: i64 = 24 * 60 * 60;

impl Registry {
    pub async fn create_team(
        &self,
        bearer: &str,
        request: &TeamCreateRequest,
    ) -> Result<TeamMutationResponse, ApiError> {
        let authority = self.user_authority(bearer, "skills:write").await?;
        let operation_id = parse_operation(&request.operation_id)?;
        let supplied = parse_hash(&request.request_hash)?;
        ensure_hash(
            supplied,
            team_create_request_hash(&request.operation_id, &request.name).map_err(hash_error)?,
        )?;
        let slug = parse_namespace(&request.name)?;
        let mut tx = self.pool.begin().await.map_err(internal_api_error)?;
        if let Some(outcome) = replay_team_operation::<TeamMutationResponse>(
            &mut tx,
            authority.user_id,
            operation_id,
            supplied,
            "create",
        )
        .await?
        {
            tx.commit().await.map_err(internal_api_error)?;
            return Ok(outcome);
        }
        if sqlx::query_scalar::<_, bool>("SELECT EXISTS(SELECT 1 FROM namespaces WHERE slug=$1)")
            .bind(&slug)
            .fetch_one(&mut *tx)
            .await
            .map_err(internal_api_error)?
        {
            return Err(ApiError::new(
                ApiErrorCode::GenerationConflict,
                format!("@{slug} is already in use"),
            ));
        }
        let namespace_id = Uuid::now_v7();
        sqlx::query("INSERT INTO namespaces (id,slug,kind) VALUES ($1,$2,'team')")
            .bind(namespace_id)
            .bind(&slug)
            .execute(&mut *tx)
            .await
            .map_err(internal_api_error)?;
        sqlx::query("INSERT INTO teams (namespace_id) VALUES ($1)")
            .bind(namespace_id)
            .execute(&mut *tx)
            .await
            .map_err(internal_api_error)?;
        sqlx::query(
            "INSERT INTO team_memberships (team_namespace_id,user_id,role) VALUES ($1,$2,'owner')",
        )
        .bind(namespace_id)
        .bind(authority.user_id)
        .execute(&mut *tx)
        .await
        .map_err(internal_api_error)?;
        let outcome = TeamMutationResponse {
            team: TeamSummary {
                namespace_id: namespace_id.to_string(),
                team: format!("@{slug}"),
                role: TeamRole::Owner,
                members_can_publish: false,
            },
            changed: true,
        };
        record_team_operation(
            &mut tx,
            authority.user_id,
            operation_id,
            supplied,
            Some(namespace_id),
            "create",
            &outcome,
        )
        .await?;
        tx.commit().await.map_err(internal_api_error)?;
        Ok(outcome)
    }

    pub async fn teams(&self, bearer: &str) -> Result<TeamList, ApiError> {
        let authority = self.user_authority(bearer, "skills:read").await?;
        let rows = sqlx::query_as::<_, (Uuid, String, String, bool)>(
            "SELECT n.id,n.slug,tm.role,t.members_can_publish FROM team_memberships tm \
             JOIN teams t ON t.namespace_id=tm.team_namespace_id \
             JOIN namespaces n ON n.id=tm.team_namespace_id WHERE tm.user_id=$1 \
             ORDER BY n.slug,n.id",
        )
        .bind(authority.user_id)
        .fetch_all(&self.pool)
        .await
        .map_err(internal_api_error)?;
        let teams = rows
            .into_iter()
            .map(|(id, slug, role, members_can_publish)| {
                Ok(TeamSummary {
                    namespace_id: id.to_string(),
                    team: format!("@{slug}"),
                    role: parse_role(&role)?,
                    members_can_publish,
                })
            })
            .collect::<Result<Vec<_>, ApiError>>()?;
        Ok(TeamList { teams })
    }

    pub async fn team_detail(&self, bearer: &str, team: &str) -> Result<TeamDetail, ApiError> {
        let authority = self.user_authority(bearer, "skills:read").await?;
        let slug = parse_namespace(team)?;
        let row = sqlx::query_as::<_, (Uuid, String, bool)>(
            "SELECT n.id,viewer.role,t.members_can_publish FROM namespaces n \
             JOIN teams t ON t.namespace_id=n.id \
             JOIN team_memberships viewer ON viewer.team_namespace_id=n.id AND viewer.user_id=$2 \
             WHERE n.slug=$1",
        )
        .bind(&slug)
        .bind(authority.user_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(internal_api_error)?
        .ok_or_else(|| ApiError::new(ApiErrorCode::NotFound, "team not found"))?;
        let members = sqlx::query_as::<_, (Uuid, String, String)>(
            "SELECT u.id,n.slug,tm.role FROM team_memberships tm JOIN users u ON u.id=tm.user_id \
             JOIN namespaces n ON n.id=u.namespace_id WHERE tm.team_namespace_id=$1 \
             ORDER BY CASE tm.role WHEN 'owner' THEN 0 WHEN 'maintainer' THEN 1 ELSE 2 END,n.slug,u.id",
        )
        .bind(row.0)
        .fetch_all(&self.pool)
        .await
        .map_err(internal_api_error)?
        .into_iter()
        .map(|(user_id, username, role)| {
            Ok(TeamMember {
                user_id: user_id.to_string(),
                username: format!("@{username}"),
                role: parse_role(&role)?,
            })
        })
        .collect::<Result<Vec<_>, ApiError>>()?;
        Ok(TeamDetail {
            team: TeamSummary {
                namespace_id: row.0.to_string(),
                team: format!("@{slug}"),
                role: parse_role(&row.1)?,
                members_can_publish: row.2,
            },
            members,
            assigned_packs: self.team_pack_assignments_for_team(row.0).await?,
        })
    }

    pub async fn create_team_invite(
        &self,
        bearer: &str,
        request: &TeamInviteRequest,
    ) -> Result<TeamInviteResponse, ApiError> {
        let authority = self.user_authority(bearer, "skills:write").await?;
        if request.role == TeamRole::Owner {
            return Err(ApiError::new(
                ApiErrorCode::InvalidRequest,
                "owner transfer is a separate operation",
            ));
        }
        let operation_id = parse_operation(&request.operation_id)?;
        let supplied = parse_hash(&request.request_hash)?;
        ensure_hash(
            supplied,
            team_invite_request_hash(
                &request.operation_id,
                &request.team,
                request.role,
                &request.invite_code_hash,
            )
            .map_err(hash_error)?,
        )?;
        let code_hash = decode_hash(&request.invite_code_hash, "invite_code_hash")?;
        let slug = parse_namespace(&request.team)?;
        let mut tx = self.pool.begin().await.map_err(internal_api_error)?;
        if let Some(outcome) = replay_team_operation::<TeamInviteResponse>(
            &mut tx,
            authority.user_id,
            operation_id,
            supplied,
            "invite",
        )
        .await?
        {
            tx.commit().await.map_err(internal_api_error)?;
            return Ok(outcome);
        }
        let (team_id, actor_role, _) =
            team_membership_for_update(&mut tx, authority.user_id, &slug).await?;
        if actor_role != TeamRole::Owner
            && !(actor_role == TeamRole::Maintainer && request.role == TeamRole::Member)
        {
            return Err(ApiError::new(
                ApiErrorCode::Unauthorized,
                "only owners may invite maintainers; owners and maintainers may invite members",
            ));
        }
        let invite_id = Uuid::now_v7();
        let expires_at = sqlx::query_scalar::<_, i64>(
            "INSERT INTO team_invites \
             (id,team_namespace_id,created_by_user_id,role,code_hash,expires_at) \
             VALUES ($1,$2,$3,$4,$5,now()+make_interval(secs => $6)) \
             RETURNING floor(extract(epoch FROM expires_at))::bigint",
        )
        .bind(invite_id)
        .bind(team_id)
        .bind(authority.user_id)
        .bind(request.role.as_str())
        .bind(code_hash.as_slice())
        .bind(INVITE_LIFETIME_SECONDS as f64)
        .fetch_one(&mut *tx)
        .await
        .map_err(internal_api_error)?;
        let outcome = TeamInviteResponse {
            invite_id: invite_id.to_string(),
            team: format!("@{slug}"),
            role: request.role,
            expires_at_unix_seconds: expires_at,
        };
        record_team_operation(
            &mut tx,
            authority.user_id,
            operation_id,
            supplied,
            Some(team_id),
            "invite",
            &outcome,
        )
        .await?;
        tx.commit().await.map_err(internal_api_error)?;
        Ok(outcome)
    }

    pub async fn revoke_team_invite(
        &self,
        bearer: &str,
        request: &TeamInviteRevokeRequest,
    ) -> Result<TeamMutationResponse, ApiError> {
        let authority = self.user_authority(bearer, "skills:write").await?;
        let operation_id = parse_operation(&request.operation_id)?;
        let invite_id = Uuid::parse_str(&request.invite_id)
            .map_err(|error| ApiError::new(ApiErrorCode::InvalidRequest, error.to_string()))?;
        let supplied = parse_hash(&request.request_hash)?;
        ensure_hash(
            supplied,
            team_invite_revoke_request_hash(
                &request.operation_id,
                &request.team,
                &request.invite_id,
            )
            .map_err(hash_error)?,
        )?;
        let slug = parse_namespace(&request.team)?;
        let mut tx = self.pool.begin().await.map_err(internal_api_error)?;
        if let Some(outcome) = replay_team_operation::<TeamMutationResponse>(
            &mut tx,
            authority.user_id,
            operation_id,
            supplied,
            "invite_revoke",
        )
        .await?
        {
            tx.commit().await.map_err(internal_api_error)?;
            return Ok(outcome);
        }
        let (team_id, actor_role, members_can_publish) =
            team_membership_for_update(&mut tx, authority.user_id, &slug).await?;
        let invite = sqlx::query_as::<_, (Uuid, Option<sqlx::types::Uuid>)>(
            "SELECT created_by_user_id,used_by_user_id FROM team_invites \
             WHERE id=$1 AND team_namespace_id=$2 FOR UPDATE",
        )
        .bind(invite_id)
        .bind(team_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(internal_api_error)?
        .ok_or_else(|| ApiError::new(ApiErrorCode::NotFound, "team invite not found"))?;
        if actor_role != TeamRole::Owner && invite.0 != authority.user_id {
            return Err(ApiError::new(
                ApiErrorCode::Unauthorized,
                "only the owner or invite creator may revoke this invite",
            ));
        }
        let changed = if invite.1.is_some() {
            false
        } else {
            sqlx::query(
                "UPDATE team_invites SET revoked_at=COALESCE(revoked_at,now()) \
                 WHERE id=$1 AND used_at IS NULL AND revoked_at IS NULL",
            )
            .bind(invite_id)
            .execute(&mut *tx)
            .await
            .map_err(internal_api_error)?
            .rows_affected()
                == 1
        };
        let outcome =
            team_mutation_summary(team_id, &slug, actor_role, members_can_publish, changed);
        record_team_operation(
            &mut tx,
            authority.user_id,
            operation_id,
            supplied,
            Some(team_id),
            "invite_revoke",
            &outcome,
        )
        .await?;
        tx.commit().await.map_err(internal_api_error)?;
        Ok(outcome)
    }

    pub async fn join_team(
        &self,
        bearer: &str,
        request: &TeamJoinRequest,
    ) -> Result<TeamMutationResponse, ApiError> {
        let authority = self.user_authority(bearer, "skills:write").await?;
        let operation_id = parse_operation(&request.operation_id)?;
        let supplied = parse_hash(&request.request_hash)?;
        ensure_hash(
            supplied,
            team_join_request_hash(&request.operation_id, &request.code).map_err(hash_error)?,
        )?;
        let code_hash = invite_code_hash(&request.code);
        let mut tx = self.pool.begin().await.map_err(internal_api_error)?;
        if let Some(outcome) = replay_team_operation::<TeamMutationResponse>(
            &mut tx,
            authority.user_id,
            operation_id,
            supplied,
            "join",
        )
        .await?
        {
            tx.commit().await.map_err(internal_api_error)?;
            return Ok(outcome);
        }
        let row = sqlx::query(
            "SELECT i.id,i.team_namespace_id,n.slug,i.role,t.members_can_publish,i.expires_at<=now(), \
                    i.used_at IS NOT NULL,i.revoked_at IS NOT NULL \
             FROM team_invites i JOIN namespaces n ON n.id=i.team_namespace_id \
             JOIN teams t ON t.namespace_id=i.team_namespace_id WHERE i.code_hash=$1 FOR UPDATE OF i",
        )
        .bind(code_hash.as_bytes().as_slice())
        .fetch_optional(&mut *tx)
        .await
        .map_err(internal_api_error)?
        .ok_or_else(|| ApiError::new(ApiErrorCode::NotFound, "team invite is invalid"))?;
        let invite_id: Uuid = row.get(0);
        let team_id: Uuid = row.get(1);
        let slug: String = row.get(2);
        let role = parse_role(row.get::<String, _>(3).as_str())?;
        let members_can_publish: bool = row.get(4);
        if row.get::<bool, _>(5) {
            return Err(ApiError::new(
                ApiErrorCode::InvalidRequest,
                "team invite expired",
            ));
        }
        if row.get::<bool, _>(6) {
            return Err(ApiError::new(
                ApiErrorCode::InvalidRequest,
                "team invite was already used",
            ));
        }
        if row.get::<bool, _>(7) {
            return Err(ApiError::new(
                ApiErrorCode::InvalidRequest,
                "team invite was revoked",
            ));
        }
        if sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS(SELECT 1 FROM team_memberships WHERE team_namespace_id=$1 AND user_id=$2)",
        )
        .bind(team_id)
        .bind(authority.user_id)
        .fetch_one(&mut *tx)
        .await
        .map_err(internal_api_error)?
        {
            return Err(ApiError::new(
                ApiErrorCode::InvalidRequest,
                "identity is already a team member",
            ));
        }
        sqlx::query(
            "INSERT INTO team_memberships (team_namespace_id,user_id,role) VALUES ($1,$2,$3)",
        )
        .bind(team_id)
        .bind(authority.user_id)
        .bind(role.as_str())
        .execute(&mut *tx)
        .await
        .map_err(internal_api_error)?;
        sqlx::query(
            "UPDATE team_invites SET used_by_user_id=$1,used_at=now() WHERE id=$2 AND used_at IS NULL AND revoked_at IS NULL",
        )
        .bind(authority.user_id)
        .bind(invite_id)
        .execute(&mut *tx)
        .await
        .map_err(internal_api_error)?;
        let outcome = team_mutation_summary(team_id, &slug, role, members_can_publish, true);
        record_team_operation(
            &mut tx,
            authority.user_id,
            operation_id,
            supplied,
            Some(team_id),
            "join",
            &outcome,
        )
        .await?;
        tx.commit().await.map_err(internal_api_error)?;
        let _ = self.wake_tx.send(crate::RegistryWake::ResyncAll);
        Ok(outcome)
    }

    pub async fn change_team_member_role(
        &self,
        bearer: &str,
        request: &TeamMemberRoleRequest,
    ) -> Result<TeamMutationResponse, ApiError> {
        if request.role == TeamRole::Owner {
            return Err(ApiError::new(
                ApiErrorCode::InvalidRequest,
                "owner transfer is a separate operation",
            ));
        }
        let authority = self.user_authority(bearer, "skills:write").await?;
        let operation_id = parse_operation(&request.operation_id)?;
        let supplied = parse_hash(&request.request_hash)?;
        ensure_hash(
            supplied,
            team_member_role_request_hash(
                &request.operation_id,
                &request.team,
                &request.member,
                request.role,
            )
            .map_err(hash_error)?,
        )?;
        let slug = parse_namespace(&request.team)?;
        let member = parse_namespace(&request.member)?;
        let mut tx = self.pool.begin().await.map_err(internal_api_error)?;
        if let Some(outcome) = replay_team_operation::<TeamMutationResponse>(
            &mut tx,
            authority.user_id,
            operation_id,
            supplied,
            "member_role",
        )
        .await?
        {
            tx.commit().await.map_err(internal_api_error)?;
            return Ok(outcome);
        }
        let (team_id, actor_role, members_can_publish) =
            team_membership_for_update(&mut tx, authority.user_id, &slug).await?;
        if actor_role != TeamRole::Owner {
            return Err(ApiError::new(
                ApiErrorCode::Unauthorized,
                "only the team owner may change roles",
            ));
        }
        let target = user_by_slug(&mut tx, &member).await?;
        let current_role = sqlx::query_scalar::<_, String>(
            "SELECT role FROM team_memberships WHERE team_namespace_id=$1 AND user_id=$2 FOR UPDATE",
        )
        .bind(team_id)
        .bind(target)
        .fetch_optional(&mut *tx)
        .await
        .map_err(internal_api_error)?
        .ok_or_else(|| ApiError::new(ApiErrorCode::NotFound, "team member not found"))?;
        if parse_role(&current_role)? == TeamRole::Owner {
            return Err(ApiError::new(
                ApiErrorCode::InvalidRequest,
                "owner transfer is a separate operation",
            ));
        }
        let changed = current_role != request.role.as_str();
        if changed {
            sqlx::query(
                "UPDATE team_memberships SET role=$1 WHERE team_namespace_id=$2 AND user_id=$3",
            )
            .bind(request.role.as_str())
            .bind(team_id)
            .bind(target)
            .execute(&mut *tx)
            .await
            .map_err(internal_api_error)?;
            if request.role == TeamRole::Member && !members_can_publish {
                remove_team_workspaces_for_user(&mut tx, team_id, target).await?;
            }
        }
        let outcome =
            team_mutation_summary(team_id, &slug, actor_role, members_can_publish, changed);
        record_team_operation(
            &mut tx,
            authority.user_id,
            operation_id,
            supplied,
            Some(team_id),
            "member_role",
            &outcome,
        )
        .await?;
        tx.commit().await.map_err(internal_api_error)?;
        let _ = self.wake_tx.send(crate::RegistryWake::ResyncAll);
        Ok(outcome)
    }

    pub async fn remove_team_member(
        &self,
        bearer: &str,
        request: &TeamMemberRemoveRequest,
    ) -> Result<TeamMutationResponse, ApiError> {
        let authority = self.user_authority(bearer, "skills:write").await?;
        let operation_id = parse_operation(&request.operation_id)?;
        let supplied = parse_hash(&request.request_hash)?;
        ensure_hash(
            supplied,
            team_member_remove_request_hash(&request.operation_id, &request.team, &request.member)
                .map_err(hash_error)?,
        )?;
        let slug = parse_namespace(&request.team)?;
        let member = parse_namespace(&request.member)?;
        let mut tx = self.pool.begin().await.map_err(internal_api_error)?;
        if let Some(outcome) = replay_team_operation::<TeamMutationResponse>(
            &mut tx,
            authority.user_id,
            operation_id,
            supplied,
            "member_remove",
        )
        .await?
        {
            tx.commit().await.map_err(internal_api_error)?;
            return Ok(outcome);
        }
        let (team_id, actor_role, members_can_publish) =
            team_membership_for_update(&mut tx, authority.user_id, &slug).await?;
        if actor_role != TeamRole::Owner {
            return Err(ApiError::new(
                ApiErrorCode::Unauthorized,
                "only the team owner may remove members",
            ));
        }
        let target = user_by_slug(&mut tx, &member).await?;
        let target_role = sqlx::query_scalar::<_, String>(
            "SELECT role FROM team_memberships WHERE team_namespace_id=$1 AND user_id=$2 FOR UPDATE",
        )
        .bind(team_id)
        .bind(target)
        .fetch_optional(&mut *tx)
        .await
        .map_err(internal_api_error)?
        .ok_or_else(|| ApiError::new(ApiErrorCode::NotFound, "team member not found"))?;
        if parse_role(&target_role)? == TeamRole::Owner {
            return Err(ApiError::new(
                ApiErrorCode::InvalidRequest,
                "the team owner cannot be removed",
            ));
        }
        remove_team_workspaces_for_user(&mut tx, team_id, target).await?;
        remove_team_membership_subscriptions(&mut tx, team_id, target).await?;
        sqlx::query("DELETE FROM team_memberships WHERE team_namespace_id=$1 AND user_id=$2")
            .bind(team_id)
            .bind(target)
            .execute(&mut *tx)
            .await
            .map_err(internal_api_error)?;
        let outcome = team_mutation_summary(team_id, &slug, actor_role, members_can_publish, true);
        record_team_operation(
            &mut tx,
            authority.user_id,
            operation_id,
            supplied,
            Some(team_id),
            "member_remove",
            &outcome,
        )
        .await?;
        tx.commit().await.map_err(internal_api_error)?;
        let _ = self.wake_tx.send(crate::RegistryWake::ResyncAll);
        Ok(outcome)
    }

    pub async fn update_team_settings(
        &self,
        bearer: &str,
        request: &TeamSettingsRequest,
    ) -> Result<TeamMutationResponse, ApiError> {
        let authority = self.user_authority(bearer, "skills:write").await?;
        let operation_id = parse_operation(&request.operation_id)?;
        let supplied = parse_hash(&request.request_hash)?;
        ensure_hash(
            supplied,
            team_settings_request_hash(
                &request.operation_id,
                &request.team,
                request.members_can_publish,
            )
            .map_err(hash_error)?,
        )?;
        let slug = parse_namespace(&request.team)?;
        let mut tx = self.pool.begin().await.map_err(internal_api_error)?;
        if let Some(outcome) = replay_team_operation::<TeamMutationResponse>(
            &mut tx,
            authority.user_id,
            operation_id,
            supplied,
            "settings",
        )
        .await?
        {
            tx.commit().await.map_err(internal_api_error)?;
            return Ok(outcome);
        }
        let (team_id, actor_role, current) =
            team_membership_for_update(&mut tx, authority.user_id, &slug).await?;
        if actor_role != TeamRole::Owner {
            return Err(ApiError::new(
                ApiErrorCode::Unauthorized,
                "only the team owner may change team settings",
            ));
        }
        let changed = current != request.members_can_publish;
        if changed {
            sqlx::query("UPDATE teams SET members_can_publish=$1 WHERE namespace_id=$2")
                .bind(request.members_can_publish)
                .bind(team_id)
                .execute(&mut *tx)
                .await
                .map_err(internal_api_error)?;
            if !request.members_can_publish {
                let members = sqlx::query_scalar::<_, Uuid>(
                    "SELECT user_id FROM team_memberships WHERE team_namespace_id=$1 AND role='member'",
                )
                .bind(team_id)
                .fetch_all(&mut *tx)
                .await
                .map_err(internal_api_error)?;
                for user_id in members {
                    remove_team_workspaces_for_user(&mut tx, team_id, user_id).await?;
                }
            }
        }
        let outcome = team_mutation_summary(
            team_id,
            &slug,
            actor_role,
            request.members_can_publish,
            changed,
        );
        record_team_operation(
            &mut tx,
            authority.user_id,
            operation_id,
            supplied,
            Some(team_id),
            "settings",
            &outcome,
        )
        .await?;
        tx.commit().await.map_err(internal_api_error)?;
        let _ = self.wake_tx.send(crate::RegistryWake::ResyncAll);
        Ok(outcome)
    }
}

pub(crate) async fn team_membership_for_update(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    user_id: Uuid,
    slug: &str,
) -> Result<(Uuid, TeamRole, bool), ApiError> {
    let row = sqlx::query_as::<_, (Uuid, String, bool)>(
        "SELECT n.id,tm.role,t.members_can_publish FROM namespaces n JOIN teams t ON t.namespace_id=n.id \
         JOIN team_memberships tm ON tm.team_namespace_id=n.id AND tm.user_id=$2 \
         WHERE n.slug=$1 FOR UPDATE OF t,tm",
    )
    .bind(slug)
    .bind(user_id)
    .fetch_optional(&mut **tx)
    .await
    .map_err(internal_api_error)?
    .ok_or_else(|| ApiError::new(ApiErrorCode::NotFound, "team not found"))?;
    Ok((row.0, parse_role(&row.1)?, row.2))
}

pub(crate) async fn user_by_slug(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    slug: &str,
) -> Result<Uuid, ApiError> {
    sqlx::query_scalar::<_, Uuid>(
        "SELECT u.id FROM users u JOIN namespaces n ON n.id=u.namespace_id \
         WHERE n.slug=$1 AND n.kind='user' AND u.deleted_at IS NULL",
    )
    .bind(slug)
    .fetch_optional(&mut **tx)
    .await
    .map_err(internal_api_error)?
    .ok_or_else(|| ApiError::new(ApiErrorCode::NotFound, "user not found"))
}

pub(crate) async fn remove_team_workspaces_for_user(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    team_id: Uuid,
    user_id: Uuid,
) -> Result<(), ApiError> {
    sqlx::query(
        "DELETE FROM skill_workspace_conflicts WHERE workspace_user_id=$1 AND resource_id IN \
         (SELECT id FROM resources WHERE owner_namespace_id=$2 AND kind='skill')",
    )
    .bind(user_id)
    .bind(team_id)
    .execute(&mut **tx)
    .await
    .map_err(internal_api_error)?;
    sqlx::query(
        "DELETE FROM skill_private_workspaces WHERE workspace_user_id=$1 AND resource_id IN \
         (SELECT id FROM resources WHERE owner_namespace_id=$2 AND kind='skill')",
    )
    .bind(user_id)
    .bind(team_id)
    .execute(&mut **tx)
    .await
    .map_err(internal_api_error)?;
    Ok(())
}

pub(crate) async fn remove_team_membership_subscriptions(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    team_id: Uuid,
    user_id: Uuid,
) -> Result<(), ApiError> {
    // Leaving a team removes private subscriptions whose authority came from membership. Public
    // direct subscriptions survive. A private skill share is an independent grant (including an
    // inherited pre-transfer share), so preserve that relationship as well.
    sqlx::query(
        "DELETE FROM account_subscriptions s USING resources r \
         WHERE s.user_id=$1 AND s.resource_id=r.id AND r.owner_namespace_id=$2 \
           AND r.visibility='private' AND r.deleted_at IS NULL \
           AND (r.kind='pack' OR NOT EXISTS ( \
             SELECT 1 FROM private_skill_shares ps \
             WHERE ps.resource_id=r.id AND ps.recipient_user_id=$1 \
           ))",
    )
    .bind(user_id)
    .bind(team_id)
    .execute(&mut **tx)
    .await
    .map_err(internal_api_error)?;
    Ok(())
}

fn team_mutation_summary(
    team_id: Uuid,
    slug: &str,
    role: TeamRole,
    members_can_publish: bool,
    changed: bool,
) -> TeamMutationResponse {
    TeamMutationResponse {
        team: TeamSummary {
            namespace_id: team_id.to_string(),
            team: format!("@{slug}"),
            role,
            members_can_publish,
        },
        changed,
    }
}

pub(crate) async fn replay_team_operation<T: DeserializeOwned>(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    user_id: Uuid,
    operation_id: OperationId,
    request_hash: RequestHash,
    kind: &str,
) -> Result<Option<T>, ApiError> {
    let row = sqlx::query_as::<_, (Vec<u8>, String, serde_json::Value)>(
        "SELECT request_hash,operation_kind,outcome_json FROM team_operations \
         WHERE user_id=$1 AND operation_id=$2",
    )
    .bind(user_id)
    .bind(operation_id.as_uuid())
    .fetch_optional(&mut **tx)
    .await
    .map_err(internal_api_error)?;
    let Some((stored_hash, stored_kind, outcome)) = row else {
        return Ok(None);
    };
    if stored_hash.as_slice() != request_hash.as_bytes() || stored_kind != kind {
        return Err(ApiError::new(
            ApiErrorCode::OperationConflict,
            "operation_id was already used with different team mutation content",
        ));
    }
    serde_json::from_value(outcome)
        .map(Some)
        .map_err(|error| ApiError::new(ApiErrorCode::Internal, error.to_string()))
}

pub(crate) async fn record_team_operation<T: Serialize>(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    user_id: Uuid,
    operation_id: OperationId,
    request_hash: RequestHash,
    team_namespace_id: Option<Uuid>,
    kind: &str,
    outcome: &T,
) -> Result<(), ApiError> {
    sqlx::query(
        "INSERT INTO team_operations \
         (user_id,operation_id,request_hash,team_namespace_id,operation_kind,outcome_json) \
         VALUES ($1,$2,$3,$4,$5,$6)",
    )
    .bind(user_id)
    .bind(operation_id.as_uuid())
    .bind(request_hash.as_bytes().as_slice())
    .bind(team_namespace_id)
    .bind(kind)
    .bind(
        serde_json::to_value(outcome)
            .map_err(|error| ApiError::new(ApiErrorCode::Internal, error.to_string()))?,
    )
    .execute(&mut **tx)
    .await
    .map_err(internal_api_error)?;
    Ok(())
}

pub(crate) fn parse_namespace(value: &str) -> Result<String, ApiError> {
    let slug = value.strip_prefix('@').unwrap_or(value);
    if slug.contains('/') || slug.is_empty() {
        return Err(ApiError::new(
            ApiErrorCode::InvalidRequest,
            "team namespace must be a single @name",
        ));
    }
    let locator = format!("@{slug}/validation")
        .parse::<ResourceLocator>()
        .map_err(|error| ApiError::new(ApiErrorCode::InvalidRequest, error.to_string()))?;
    Ok(locator.owner().to_owned())
}

pub(crate) fn parse_role(value: &str) -> Result<TeamRole, ApiError> {
    match value {
        "owner" => Ok(TeamRole::Owner),
        "maintainer" => Ok(TeamRole::Maintainer),
        "member" => Ok(TeamRole::Member),
        _ => Err(ApiError::new(
            ApiErrorCode::Internal,
            "stored team role is invalid",
        )),
    }
}

pub(crate) fn parse_operation(value: &str) -> Result<OperationId, ApiError> {
    OperationId::from_str(value)
        .map_err(|error| ApiError::new(ApiErrorCode::InvalidRequest, error.to_string()))
}

pub(crate) fn parse_hash(value: &str) -> Result<RequestHash, ApiError> {
    RequestHash::from_str(value)
        .map_err(|error| ApiError::new(ApiErrorCode::InvalidRequestHash, error.to_string()))
}

fn decode_hash(value: &str, field: &str) -> Result<[u8; 32], ApiError> {
    let bytes = hex::decode(value)
        .map_err(|error| ApiError::new(ApiErrorCode::InvalidRequest, error.to_string()))?;
    bytes.try_into().map_err(|_| {
        ApiError::new(
            ApiErrorCode::InvalidRequest,
            format!("{field} must be a 32-byte SHA-256 value"),
        )
    })
}

pub(crate) fn ensure_hash(actual: RequestHash, expected: RequestHash) -> Result<(), ApiError> {
    if actual == expected {
        Ok(())
    } else {
        Err(ApiError::new(
            ApiErrorCode::InvalidRequestHash,
            "request_hash does not match the canonical team mutation payload",
        ))
    }
}

pub(crate) fn hash_error(error: impl std::fmt::Display) -> ApiError {
    ApiError::new(ApiErrorCode::InvalidRequestHash, error.to_string())
}
