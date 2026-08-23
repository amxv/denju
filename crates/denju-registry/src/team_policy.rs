use denju_wire::{
    ApiError, ApiErrorCode, TeamDeleteRequest, TeamDeleteResponse, TeamLeaveRequest,
    TeamLeaveResponse, TeamOwnerTransferAcceptRequest, TeamOwnerTransferRequest,
    TeamOwnerTransferResponse, TeamPackAssignment, TeamPackAssignmentMutationKind,
    TeamPackAssignmentRequest, TeamPackAssignmentResponse, TeamRole, team_delete_request_hash,
    team_leave_request_hash, team_owner_transfer_accept_request_hash,
    team_owner_transfer_code_hash, team_owner_transfer_request_hash,
    team_pack_assignment_request_hash,
};
use sqlx::Row;
use uuid::Uuid;

use crate::{
    Registry,
    identity_support::{hash_operation_secret, operation_secret_matches, verify_password},
    internal_api_error,
    teams::{
        ensure_hash, hash_error, parse_hash, parse_namespace, parse_operation,
        record_team_operation, remove_team_membership_subscriptions,
        remove_team_workspaces_for_user, replay_team_operation, team_membership_for_update,
        user_by_slug,
    },
};

impl Registry {
    pub(crate) async fn team_pack_assignments_for_team(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        team_id: Uuid,
    ) -> Result<Vec<TeamPackAssignment>, ApiError> {
        sqlx::query_as::<_, (Uuid, String, Uuid, String, String)>(
            "SELECT a.team_namespace_id,team.slug,a.pack_resource_id,pack_owner.slug,p.slug \
             FROM team_pack_assignments a \
             JOIN namespaces team ON team.id=a.team_namespace_id \
             JOIN resources p ON p.id=a.pack_resource_id AND p.kind='pack' \
             JOIN namespaces pack_owner ON pack_owner.id=p.owner_namespace_id \
             WHERE a.team_namespace_id=$1 ORDER BY pack_owner.slug,p.slug,p.id",
        )
        .bind(team_id)
        .fetch_all(&mut **tx)
        .await
        .map_err(internal_api_error)
        .map(|rows| {
            rows.into_iter()
                .map(|(team_id, team, pack_id, owner, pack)| TeamPackAssignment {
                    team_namespace_id: team_id.to_string(),
                    team: format!("@{team}"),
                    pack_resource_id: pack_id.to_string(),
                    pack_locator: format!("@{owner}/packs/{pack}"),
                })
                .collect()
        })
    }

    pub async fn mutate_team_pack_assignment(
        &self,
        bearer: &str,
        kind: TeamPackAssignmentMutationKind,
        request: &TeamPackAssignmentRequest,
    ) -> Result<TeamPackAssignmentResponse, ApiError> {
        let authority = self.user_authority(bearer, "skills:write").await?;
        let operation_id = parse_operation(&request.operation_id)?;
        let supplied = parse_hash(&request.request_hash)?;
        ensure_hash(
            supplied,
            team_pack_assignment_request_hash(
                kind,
                &request.operation_id,
                &request.team,
                &request.pack_resource_id,
            )
            .map_err(hash_error)?,
        )?;
        let pack_id = Uuid::parse_str(&request.pack_resource_id)
            .map_err(|error| ApiError::new(ApiErrorCode::InvalidRequest, error.to_string()))?;
        let slug = parse_namespace(&request.team)?;
        let operation_kind = match kind {
            TeamPackAssignmentMutationKind::Assign => "pack_assign",
            TeamPackAssignmentMutationKind::Unassign => "pack_unassign",
        };
        let mut tx = self.begin_actor_tx(authority.user_id).await?;
        if let Some(outcome) = replay_team_operation::<TeamPackAssignmentResponse>(
            &mut tx,
            authority.user_id,
            operation_id,
            supplied,
            operation_kind,
        )
        .await?
        {
            tx.commit().await.map_err(internal_api_error)?;
            return Ok(outcome);
        }
        let (team_id, role, _) =
            team_membership_for_update(&mut tx, authority.user_id, &slug).await?;
        if role != TeamRole::Owner {
            return Err(ApiError::new(
                ApiErrorCode::Unauthorized,
                "only the team owner may change enforced pack assignments",
            ));
        }
        let pack = sqlx::query_as::<_, (String, String, String, Option<Uuid>)>(
            "SELECT owner_slug,resource_slug,visibility,owner_namespace_id \
             FROM denju_lock_team_assignment_pack($1,$2)",
        )
        .bind(pack_id)
        .bind(team_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(internal_api_error)?
        .ok_or_else(|| ApiError::new(ApiErrorCode::NotFound, "pack not found"))?;
        if kind == TeamPackAssignmentMutationKind::Assign
            && pack.2 != "public"
            && pack.3 != Some(team_id)
        {
            return Err(ApiError::new(
                ApiErrorCode::InvalidRequest,
                "assigned pack must be public or owned by the assigning team so every member can read it",
            ));
        }
        let changed = match kind {
            TeamPackAssignmentMutationKind::Assign => {
                sqlx::query(
                    "INSERT INTO team_pack_assignments \
                     (team_namespace_id,pack_resource_id,assigned_by_user_id) VALUES ($1,$2,$3) \
                     ON CONFLICT(team_namespace_id,pack_resource_id) DO NOTHING",
                )
                .bind(team_id)
                .bind(pack_id)
                .bind(authority.user_id)
                .execute(&mut *tx)
                .await
                .map_err(internal_api_error)?
                .rows_affected()
                    == 1
            }
            TeamPackAssignmentMutationKind::Unassign => sqlx::query(
                "DELETE FROM team_pack_assignments WHERE team_namespace_id=$1 AND pack_resource_id=$2",
            )
            .bind(team_id)
            .bind(pack_id)
            .execute(&mut *tx)
            .await
            .map_err(internal_api_error)?
            .rows_affected()
                == 1,
        };
        let assignment = TeamPackAssignment {
            team_namespace_id: team_id.to_string(),
            team: format!("@{slug}"),
            pack_resource_id: pack_id.to_string(),
            pack_locator: format!("@{}/packs/{}", pack.0, pack.1),
        };
        let outcome = TeamPackAssignmentResponse {
            assignment,
            assigned: kind == TeamPackAssignmentMutationKind::Assign,
            changed,
        };
        record_team_operation(
            &mut tx,
            authority.user_id,
            operation_id,
            supplied,
            Some(team_id),
            operation_kind,
            &outcome,
        )
        .await?;
        tx.commit().await.map_err(internal_api_error)?;
        let _ = self.wake_tx.send(crate::RegistryWake::ResyncAll);
        Ok(outcome)
    }

    pub async fn leave_team(
        &self,
        bearer: &str,
        request: &TeamLeaveRequest,
    ) -> Result<TeamLeaveResponse, ApiError> {
        let authority = self.user_authority(bearer, "skills:write").await?;
        let operation_id = parse_operation(&request.operation_id)?;
        let supplied = parse_hash(&request.request_hash)?;
        ensure_hash(
            supplied,
            team_leave_request_hash(&request.operation_id, &request.team).map_err(hash_error)?,
        )?;
        let slug = parse_namespace(&request.team)?;
        let mut tx = self.begin_actor_tx(authority.user_id).await?;
        if let Some(outcome) = replay_team_operation::<TeamLeaveResponse>(
            &mut tx,
            authority.user_id,
            operation_id,
            supplied,
            "leave",
        )
        .await?
        {
            tx.commit().await.map_err(internal_api_error)?;
            return Ok(outcome);
        }
        let (team_id, role, _) =
            team_membership_for_update(&mut tx, authority.user_id, &slug).await?;
        if role == TeamRole::Owner {
            return Err(ApiError::new(
                ApiErrorCode::InvalidRequest,
                "team owner must transfer ownership or delete the team before leaving",
            ));
        }
        remove_team_workspaces_for_user(&mut tx, team_id, authority.user_id).await?;
        remove_team_membership_subscriptions(&mut tx, team_id, authority.user_id).await?;
        sqlx::query("DELETE FROM team_memberships WHERE team_namespace_id=$1 AND user_id=$2")
            .bind(team_id)
            .bind(authority.user_id)
            .execute(&mut *tx)
            .await
            .map_err(internal_api_error)?;
        let outcome = TeamLeaveResponse {
            team: format!("@{slug}"),
            left: true,
        };
        record_team_operation(
            &mut tx,
            authority.user_id,
            operation_id,
            supplied,
            Some(team_id),
            "leave",
            &outcome,
        )
        .await?;
        tx.commit().await.map_err(internal_api_error)?;
        let _ = self.wake_tx.send(crate::RegistryWake::ResyncAll);
        Ok(outcome)
    }

    pub async fn create_team_owner_transfer(
        &self,
        bearer: &str,
        request: &TeamOwnerTransferRequest,
    ) -> Result<TeamOwnerTransferResponse, ApiError> {
        let authority = self.user_authority(bearer, "skills:write").await?;
        let operation_id = parse_operation(&request.operation_id)?;
        let supplied = parse_hash(&request.request_hash)?;
        ensure_hash(
            supplied,
            team_owner_transfer_request_hash(
                &request.operation_id,
                &request.team,
                &request.member,
                &request.transfer_code_hash,
            )
            .map_err(hash_error)?,
        )?;
        let transfer_code_hash = decode_32(&request.transfer_code_hash, "transfer_code_hash")?;
        let slug = parse_namespace(&request.team)?;
        let member = parse_namespace(&request.member)?;
        let mut tx = self.begin_actor_tx(authority.user_id).await?;
        if let Some(outcome) = replay_team_operation::<TeamOwnerTransferResponse>(
            &mut tx,
            authority.user_id,
            operation_id,
            supplied,
            "owner_transfer",
        )
        .await?
        {
            tx.commit().await.map_err(internal_api_error)?;
            return Ok(outcome);
        }
        let (team_id, role, _) =
            team_membership_for_update(&mut tx, authority.user_id, &slug).await?;
        if role != TeamRole::Owner {
            return Err(ApiError::new(
                ApiErrorCode::Unauthorized,
                "only the current owner may initiate ownership transfer",
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
        .ok_or_else(|| ApiError::new(ApiErrorCode::NotFound, "target must already be a team member"))?;
        if target_role == "owner" {
            return Err(ApiError::new(
                ApiErrorCode::InvalidRequest,
                "target is already the team owner",
            ));
        }
        sqlx::query(
            "UPDATE team_owner_transfers SET state='cancelled' \
             WHERE team_namespace_id=$1 AND state='pending'",
        )
        .bind(team_id)
        .execute(&mut *tx)
        .await
        .map_err(internal_api_error)?;
        let transfer_id = Uuid::now_v7();
        sqlx::query(
            "INSERT INTO team_owner_transfers \
             (id,team_namespace_id,from_user_id,to_user_id,code_hash,state) VALUES ($1,$2,$3,$4,$5,'pending')",
        )
        .bind(transfer_id)
        .bind(team_id)
        .bind(authority.user_id)
        .bind(target)
        .bind(transfer_code_hash.as_slice())
        .execute(&mut *tx)
        .await
        .map_err(internal_api_error)?;
        let outcome = TeamOwnerTransferResponse {
            transfer_id: transfer_id.to_string(),
            team: format!("@{slug}"),
            from_user_id: authority.user_id.to_string(),
            to_user_id: target.to_string(),
            state: "pending".to_owned(),
        };
        record_team_operation(
            &mut tx,
            authority.user_id,
            operation_id,
            supplied,
            Some(team_id),
            "owner_transfer",
            &outcome,
        )
        .await?;
        tx.commit().await.map_err(internal_api_error)?;
        Ok(outcome)
    }

    pub async fn accept_team_owner_transfer(
        &self,
        bearer: &str,
        request: &TeamOwnerTransferAcceptRequest,
    ) -> Result<TeamOwnerTransferResponse, ApiError> {
        let authority = self.user_authority(bearer, "skills:write").await?;
        let operation_id = parse_operation(&request.operation_id)?;
        let supplied = parse_hash(&request.request_hash)?;
        ensure_hash(
            supplied,
            team_owner_transfer_accept_request_hash(&request.operation_id, &request.code)
                .map_err(hash_error)?,
        )?;
        let transfer_code_hash = team_owner_transfer_code_hash(&request.code);
        let mut tx = self.begin_actor_tx(authority.user_id).await?;
        if let Some(outcome) = replay_team_operation::<TeamOwnerTransferResponse>(
            &mut tx,
            authority.user_id,
            operation_id,
            supplied,
            "owner_transfer_accept",
        )
        .await?
        {
            tx.commit().await.map_err(internal_api_error)?;
            return Ok(outcome);
        }
        let row = sqlx::query(
            "SELECT tr.id,tr.team_namespace_id,tr.from_user_id,tr.to_user_id,tr.state,n.slug \
             FROM team_owner_transfers tr JOIN namespaces n ON n.id=tr.team_namespace_id \
             WHERE tr.code_hash=$1 FOR UPDATE OF tr",
        )
        .bind(transfer_code_hash.as_bytes().as_slice())
        .fetch_optional(&mut *tx)
        .await
        .map_err(internal_api_error)?
        .ok_or_else(|| ApiError::new(ApiErrorCode::NotFound, "ownership transfer not found"))?;
        let transfer_id: Uuid = row.get(0);
        let team_id: Uuid = row.get(1);
        let from: Uuid = row.get(2);
        let to: Uuid = row.get(3);
        let state: String = row.get(4);
        let slug: String = row.get(5);
        if to != authority.user_id {
            return Err(ApiError::new(
                ApiErrorCode::Unauthorized,
                "only the nominated member may accept ownership transfer",
            ));
        }
        if state != "pending" {
            return Err(ApiError::new(
                ApiErrorCode::InvalidRequest,
                "ownership transfer is no longer pending",
            ));
        }
        let current_owner = sqlx::query_scalar::<_, Uuid>(
            "SELECT user_id FROM team_memberships WHERE team_namespace_id=$1 AND role='owner' FOR UPDATE",
        )
        .bind(team_id)
        .fetch_one(&mut *tx)
        .await
        .map_err(internal_api_error)?;
        if current_owner != from {
            return Err(ApiError::new(
                ApiErrorCode::GenerationConflict,
                "team ownership changed before this transfer was accepted",
            ));
        }
        let target_exists = sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS(SELECT 1 FROM team_memberships WHERE team_namespace_id=$1 AND user_id=$2)",
        )
        .bind(team_id)
        .bind(to)
        .fetch_one(&mut *tx)
        .await
        .map_err(internal_api_error)?;
        if !target_exists {
            return Err(ApiError::new(
                ApiErrorCode::InvalidRequest,
                "nominated owner is no longer a team member",
            ));
        }
        sqlx::query(
            "UPDATE team_memberships SET role='maintainer' WHERE team_namespace_id=$1 AND user_id=$2",
        )
        .bind(team_id)
        .bind(from)
        .execute(&mut *tx)
        .await
        .map_err(internal_api_error)?;
        sqlx::query(
            "UPDATE team_memberships SET role='owner' WHERE team_namespace_id=$1 AND user_id=$2",
        )
        .bind(team_id)
        .bind(to)
        .execute(&mut *tx)
        .await
        .map_err(internal_api_error)?;
        sqlx::query(
            "UPDATE team_owner_transfers SET state='accepted',accepted_at=now() WHERE id=$1",
        )
        .bind(transfer_id)
        .execute(&mut *tx)
        .await
        .map_err(internal_api_error)?;
        let outcome = TeamOwnerTransferResponse {
            transfer_id: transfer_id.to_string(),
            team: format!("@{slug}"),
            from_user_id: from.to_string(),
            to_user_id: to.to_string(),
            state: "accepted".to_owned(),
        };
        record_team_operation(
            &mut tx,
            authority.user_id,
            operation_id,
            supplied,
            Some(team_id),
            "owner_transfer_accept",
            &outcome,
        )
        .await?;
        tx.commit().await.map_err(internal_api_error)?;
        // Acceptance is the authority transition. The new owner may have been a plain member and
        // now gains publisher/private-workspace state; the former owner also changes role.
        let _ = self.wake_tx.send(crate::RegistryWake::ResyncAll);
        Ok(outcome)
    }

    pub async fn delete_team(
        &self,
        bearer: &str,
        request: &TeamDeleteRequest,
    ) -> Result<TeamDeleteResponse, ApiError> {
        let authority = self.user_authority(bearer, "skills:write").await?;
        let operation_id = parse_operation(&request.operation_id)?;
        let supplied = parse_hash(&request.request_hash)?;
        ensure_hash(
            supplied,
            team_delete_request_hash(&request.operation_id, &request.team).map_err(hash_error)?,
        )?;
        let slug = parse_namespace(&request.team)?;
        let mut tx = self.begin_actor_tx(authority.user_id).await?;
        if let Some(outcome) = replay_team_delete_operation(
            &mut tx,
            authority.user_id,
            operation_id,
            supplied,
            &request.password,
        )
        .await?
        {
            tx.commit().await.map_err(internal_api_error)?;
            return Ok(outcome);
        }
        let (team_id, role, _) =
            team_membership_for_update(&mut tx, authority.user_id, &slug).await?;
        if role != TeamRole::Owner {
            return Err(ApiError::new(
                ApiErrorCode::Unauthorized,
                "only the team owner may delete the team",
            ));
        }
        let password_hash = sqlx::query_scalar::<_, String>(
            "SELECT denju_actor_password_hash(id) FROM users WHERE id=$1 AND deleted_at IS NULL FOR UPDATE",
        )
        .bind(authority.user_id)
        .fetch_one(&mut *tx)
        .await
        .map_err(internal_api_error)?;
        verify_password(&request.password, &password_hash)?;

        // Pack deletion semantics remove durable pack subscriptions/assignments outright. Skill
        // subscriptions keep their ordinary retain-on-delete behavior through the tombstone.
        sqlx::query(
            "DELETE FROM installation_subscriptions WHERE resource_id IN ( \
               SELECT id FROM resources WHERE owner_namespace_id=$1 AND kind='pack' AND deleted_at IS NULL \
             )",
        )
        .bind(team_id)
        .execute(&mut *tx)
        .await
        .map_err(internal_api_error)?;
        sqlx::query(
            "DELETE FROM account_subscriptions WHERE resource_id IN ( \
               SELECT id FROM resources WHERE owner_namespace_id=$1 AND kind='pack' AND deleted_at IS NULL \
             )",
        )
        .bind(team_id)
        .execute(&mut *tx)
        .await
        .map_err(internal_api_error)?;
        sqlx::query(
            "DELETE FROM team_pack_assignments WHERE pack_resource_id IN ( \
               SELECT id FROM resources WHERE owner_namespace_id=$1 AND kind='pack' AND deleted_at IS NULL \
             )",
        )
        .bind(team_id)
        .execute(&mut *tx)
        .await
        .map_err(internal_api_error)?;

        // Tombstone rather than physically deleting resources. This preserves immutable
        // revision ancestry for forks living outside the team while removing team authority.
        let _ = self
            .tombstone_owned_resources_for_account_delete(&mut tx, team_id, &slug)
            .await?;
        sqlx::query("DELETE FROM namespaces WHERE id=$1")
            .bind(team_id)
            .execute(&mut *tx)
            .await
            .map_err(internal_api_error)?;
        let outcome = TeamDeleteResponse {
            team: format!("@{slug}"),
            deleted: true,
        };
        // The namespace deletion cascades the teams row, so preserve delete replay with a NULL
        // team FK in the operation journal.
        record_team_operation(
            &mut tx,
            authority.user_id,
            operation_id,
            supplied,
            None,
            "delete",
            &outcome,
        )
        .await?;
        let secret_verifier = hash_operation_secret(request.password.as_bytes())?;
        sqlx::query(
            "UPDATE team_operations SET secret_verifier=$1 WHERE user_id=$2 AND operation_id=$3",
        )
        .bind(secret_verifier)
        .bind(authority.user_id)
        .bind(operation_id.as_uuid())
        .execute(&mut *tx)
        .await
        .map_err(internal_api_error)?;
        tx.commit().await.map_err(internal_api_error)?;
        let _ = self.wake_tx.send(crate::RegistryWake::ResyncAll);
        let _ = self.drain_outbox(256).await;
        Ok(outcome)
    }
}

fn decode_32(value: &str, field: &str) -> Result<[u8; 32], ApiError> {
    let bytes = hex::decode(value).map_err(|_| {
        ApiError::new(
            ApiErrorCode::InvalidRequest,
            format!("{field} must be hexadecimal"),
        )
    })?;
    bytes.try_into().map_err(|_| {
        ApiError::new(
            ApiErrorCode::InvalidRequest,
            format!("{field} must encode 32 bytes"),
        )
    })
}

async fn replay_team_delete_operation(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    user_id: Uuid,
    operation_id: denju_core::OperationId,
    request_hash: denju_wire::RequestHash,
    password: &str,
) -> Result<Option<TeamDeleteResponse>, ApiError> {
    let row = sqlx::query(
        "SELECT request_hash,operation_kind,outcome_json,secret_verifier FROM team_operations \
         WHERE user_id=$1 AND operation_id=$2",
    )
    .bind(user_id)
    .bind(operation_id.as_uuid())
    .fetch_optional(&mut **tx)
    .await
    .map_err(internal_api_error)?;
    let Some(row) = row else {
        return Ok(None);
    };
    let stored_hash: Vec<u8> = row.get(0);
    let kind: String = row.get(1);
    if stored_hash.as_slice() != request_hash.as_bytes() || kind != "delete" {
        return Err(ApiError::new(
            ApiErrorCode::OperationConflict,
            "operation_id was already used with different team mutation content",
        ));
    }
    let verifier: Option<String> = row.get(3);
    if !operation_secret_matches(verifier.as_deref(), Some(password.as_bytes())) {
        return Err(ApiError::new(
            ApiErrorCode::OperationConflict,
            "operation_id was already used with different password input",
        ));
    }
    serde_json::from_value(row.get(2))
        .map(Some)
        .map_err(|error| ApiError::new(ApiErrorCode::Internal, error.to_string()))
}
