use std::str::FromStr;

use denju_core::{OperationId, ResourceId, RevisionId};
use denju_wire::{
    ApiError, ApiErrorCode, ProposalAcceptRequest, ProposalCloseKind, ProposalCloseRequest,
    ProposalCreateRequest, PublicSkillManifest, RequestHash, SkillProposal, SkillProposalDetail,
    SkillProposalList, SkillProposalState, SnapshotDownload, proposal_accept_request_hash,
    proposal_close_request_hash, proposal_create_request_hash,
};
use serde::{Serialize, de::DeserializeOwned};
use serde_json::Value;
use sqlx::{FromRow, Row};
use uuid::Uuid;

use crate::{
    Registry,
    ingest::{decode_32, manifest_blobs},
    ingest_storage::enforce_namespace_quota,
    internal_api_error,
    lifecycle::{generation_u64, next_generation},
    outbox::enqueue_resource_wake,
    team_access::{authorize_resource_publish, ensure_private_workspace_for_user},
};

#[derive(Debug, FromRow)]
struct ProposalRow {
    proposal_id: Uuid,
    proposal_generation: i64,
    state: String,
    message: Option<String>,
    proposer_user_id: Uuid,
    proposer_namespace_id: Uuid,
    proposer: String,
    source_resource_id: Uuid,
    source_owner: String,
    source_name: String,
    source_generation: i64,
    source_revision_id: Vec<u8>,
    target_resource_id: Uuid,
    target_owner_namespace_id: Uuid,
    target_owner_kind: String,
    target_owner: String,
    target_name: String,
    target_generation: i64,
    target_visibility: String,
    target_revision_id: Option<Vec<u8>>,
    target_release_revision_id: Option<Vec<u8>>,
    target_shared_with_proposer: bool,
    target_team_with_proposer: bool,
    sync_base_revision_id: Vec<u8>,
    closed_revision_id: Option<Vec<u8>>,
    closed_source_generation: Option<i64>,
}

impl Registry {
    pub async fn create_proposal(
        &self,
        bearer: &str,
        request: &ProposalCreateRequest,
    ) -> Result<SkillProposal, ApiError> {
        let authority = self.user_authority(bearer, "skills:write").await?;
        crate::proposal_metadata::validate_message(request.message.as_deref())?;
        let operation_id = OperationId::from_str(&request.operation_id)
            .map_err(|error| ApiError::new(ApiErrorCode::InvalidRequest, error.to_string()))?;
        let source_resource_id = ResourceId::from_str(&request.source_resource_id)
            .map_err(|error| ApiError::new(ApiErrorCode::InvalidRequest, error.to_string()))?;
        let supplied_hash = parse_hash(&request.request_hash)?;
        let expected_hash = proposal_create_request_hash(
            &request.operation_id,
            &request.source_resource_id,
            request.expected_source_generation,
            request.message.as_deref(),
        )
        .map_err(hash_error)?;
        ensure_hash(supplied_hash, expected_hash)?;

        let mut tx = self.begin_actor_tx(authority.user_id).await?;
        if let Some(outcome) = replay_operation::<SkillProposal>(
            &mut tx,
            authority.user_id,
            operation_id,
            supplied_hash,
            "create",
        )
        .await?
        {
            tx.commit().await.map_err(internal_api_error)?;
            return Ok(outcome);
        }

        let source = sqlx::query(
            "SELECT r.owner_namespace_id,r.generation,w.revision_id,r.slug,f.upstream_resource_id,f.promotion_pending \
             FROM resources r JOIN skill_private_workspaces w ON w.resource_id=r.id AND w.workspace_user_id=$2 \
             JOIN skill_forks f ON f.resource_id=r.id \
             WHERE r.id=$1 AND r.kind='skill' AND r.deleted_at IS NULL FOR UPDATE OF r,w,f",
        )
        .bind(source_resource_id.as_uuid())
        .bind(authority.user_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(internal_api_error)?
        .ok_or_else(|| ApiError::new(ApiErrorCode::NotFound, "owned fork not found"))?;
        let source_owner: Uuid = source.get(0);
        let source_generation: i64 = source.get(1);
        let source_name: String = source.get(3);
        let target_resource_id: Uuid = source.get(4);
        let promotion_pending: bool = source.get(5);
        if source_owner != authority.namespace_id || promotion_pending {
            return Err(ApiError::new(
                ApiErrorCode::NotFound,
                "owned fork not found",
            ));
        }
        let expected_source_generation = i64::try_from(request.expected_source_generation)
            .map_err(|_| {
                ApiError::new(
                    ApiErrorCode::InvalidRequest,
                    "generation exceeds database range",
                )
            })?;
        if source_generation != expected_source_generation {
            return Err(generation_conflict(source_generation));
        }
        let target_name = sqlx::query_scalar::<_, String>(
            "SELECT slug FROM resources WHERE id=$1 AND kind='skill' AND deleted_at IS NULL",
        )
        .bind(target_resource_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(internal_api_error)?
        .ok_or_else(|| ApiError::new(ApiErrorCode::NotFound, "upstream skill not found"))?;
        if source_name != target_name {
            return Err(ApiError::new(
                ApiErrorCode::InvalidRequest,
                "a proposal must preserve the upstream skill name; rename the fork to match upstream first",
            ));
        }
        let duplicate = sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS(SELECT 1 FROM skill_proposals WHERE source_resource_id=$1 AND target_resource_id=$2 AND state='open')",
        )
        .bind(source_resource_id.as_uuid())
        .bind(target_resource_id)
        .fetch_one(&mut *tx)
        .await
        .map_err(internal_api_error)?;
        if duplicate {
            return Err(ApiError::new(
                ApiErrorCode::InvalidRequest,
                "this fork already has an open proposal",
            ));
        }
        let proposal_id = Uuid::now_v7();
        sqlx::query(
            "INSERT INTO skill_proposals \
             (id,proposer_user_id,source_resource_id,target_resource_id,generation,state,message) \
             VALUES ($1,$2,$3,$4,1,'open',$5)",
        )
        .bind(proposal_id)
        .bind(authority.user_id)
        .bind(source_resource_id.as_uuid())
        .bind(target_resource_id)
        .bind(request.message.as_deref())
        .execute(&mut *tx)
        .await
        .map_err(internal_api_error)?;
        let row = load_proposal_row(&mut tx, proposal_id).await?;
        let outcome = proposal_summary(&row)?;
        record_operation(
            &mut tx,
            authority.user_id,
            operation_id,
            supplied_hash,
            "create",
            proposal_id,
            &outcome,
        )
        .await?;
        tx.commit().await.map_err(internal_api_error)?;
        Ok(outcome)
    }

    pub async fn proposals(&self, bearer: &str) -> Result<SkillProposalList, ApiError> {
        let authority = self.user_authority(bearer, "skills:read").await?;
        let mut tx = self.begin_actor_tx(authority.user_id).await?;
        let ids = sqlx::query_scalar::<_, Uuid>(
            "SELECT p.id FROM skill_proposals p \
             JOIN resources target ON target.id=p.target_resource_id \
             LEFT JOIN teams team ON team.namespace_id=target.owner_namespace_id \
             LEFT JOIN team_memberships tm ON tm.team_namespace_id=target.owner_namespace_id AND tm.user_id=$1 \
             WHERE p.proposer_user_id=$1 OR target.owner_namespace_id=$2 OR \
               (tm.user_id IS NOT NULL AND (tm.role IN ('owner','maintainer') OR (tm.role='member' AND team.members_can_publish))) \
             ORDER BY p.created_at DESC,p.id",
        )
        .bind(authority.user_id)
        .bind(authority.namespace_id)
        .fetch_all(&mut *tx)
        .await
        .map_err(internal_api_error)?;
        let mut proposals = Vec::with_capacity(ids.len());
        for id in ids {
            let row = load_proposal_row(&mut tx, id).await?;
            proposals.push(proposal_summary(&row)?);
        }
        tx.commit().await.map_err(internal_api_error)?;
        Ok(SkillProposalList { proposals })
    }

    pub async fn proposal_detail(
        &self,
        bearer: &str,
        proposal_id: &str,
    ) -> Result<SkillProposalDetail, ApiError> {
        let authority = self.user_authority(bearer, "skills:read").await?;
        let proposal_id = parse_uuid(proposal_id, "proposal ID")?;
        let mut tx = self.begin_actor_tx(authority.user_id).await?;
        let row = load_proposal_row(&mut tx, proposal_id).await?;
        let target_publisher = can_publish_target(
            &mut tx,
            authority.user_id,
            authority.namespace_id,
            row.target_owner_namespace_id,
        )
        .await?;
        if row.proposer_user_id != authority.user_id && !target_publisher {
            return Err(ApiError::new(ApiErrorCode::NotFound, "proposal not found"));
        }
        let mut proposal = proposal_summary(&row)?;
        if target_publisher {
            proposal.target_generation = workspace_generation_or_resource(
                &mut tx,
                row.target_resource_id,
                authority.user_id,
                row.target_generation,
            )
            .await?;
        }
        let revision = RevisionId::from_str(&proposal.proposed_revision_id)
            .map_err(|error| ApiError::new(ApiErrorCode::Internal, error.to_string()))?;
        let snapshot = sqlx::query_as::<_, (Value, String, Vec<u8>, i64)>(
            "SELECT manifest_json,snapshot_key,snapshot_sha256,snapshot_size \
             FROM resource_revision_snapshots WHERE resource_id=$1 AND revision_id=$2",
        )
        .bind(row.source_resource_id)
        .bind(revision.as_bytes().as_slice())
        .fetch_optional(&mut *tx)
        .await
        .map_err(internal_api_error)?
        .ok_or_else(|| ApiError::new(ApiErrorCode::NotFound, "proposal revision is unavailable"))?;
        tx.commit().await.map_err(internal_api_error)?;
        let manifest: PublicSkillManifest = serde_json::from_value(snapshot.0)
            .map_err(|error| ApiError::new(ApiErrorCode::Internal, error.to_string()))?;
        let snapshot_sha = decode_32(&snapshot.2, "proposal snapshot SHA-256")?;
        let size_bytes = u64::try_from(snapshot.3).map_err(|_| {
            ApiError::new(ApiErrorCode::Internal, "stored snapshot size is invalid")
        })?;
        let url = self
            .objects
            .presign_get(&snapshot.1)
            .await
            .map_err(|error| ApiError::new(ApiErrorCode::Unavailable, error.to_string()))?;
        Ok(SkillProposalDetail {
            proposal,
            manifest,
            snapshot: SnapshotDownload {
                sha256: hex::encode(snapshot_sha),
                size_bytes,
                url: url.to_string(),
            },
        })
    }

    pub async fn reject_proposal(
        &self,
        bearer: &str,
        request: &ProposalCloseRequest,
    ) -> Result<SkillProposal, ApiError> {
        self.close_proposal(bearer, request, ProposalCloseKind::Reject)
            .await
    }

    pub async fn withdraw_proposal(
        &self,
        bearer: &str,
        request: &ProposalCloseRequest,
    ) -> Result<SkillProposal, ApiError> {
        self.close_proposal(bearer, request, ProposalCloseKind::Withdraw)
            .await
    }

    async fn close_proposal(
        &self,
        bearer: &str,
        request: &ProposalCloseRequest,
        kind: ProposalCloseKind,
    ) -> Result<SkillProposal, ApiError> {
        let authority = self.user_authority(bearer, "skills:write").await?;
        let operation_id = OperationId::from_str(&request.operation_id)
            .map_err(|error| ApiError::new(ApiErrorCode::InvalidRequest, error.to_string()))?;
        let proposal_id = parse_uuid(&request.proposal_id, "proposal ID")?;
        let supplied_hash = parse_hash(&request.request_hash)?;
        let expected_hash = proposal_close_request_hash(
            kind,
            &request.operation_id,
            &request.proposal_id,
            request.expected_generation,
        )
        .map_err(hash_error)?;
        ensure_hash(supplied_hash, expected_hash)?;
        let action = match kind {
            ProposalCloseKind::Reject => "reject",
            ProposalCloseKind::Withdraw => "withdraw",
        };
        let terminal = match kind {
            ProposalCloseKind::Reject => "rejected",
            ProposalCloseKind::Withdraw => "withdrawn",
        };

        let mut tx = self.begin_actor_tx(authority.user_id).await?;
        if let Some(outcome) = replay_operation::<SkillProposal>(
            &mut tx,
            authority.user_id,
            operation_id,
            supplied_hash,
            action,
        )
        .await?
        {
            tx.commit().await.map_err(internal_api_error)?;
            return Ok(outcome);
        }
        let row = load_proposal_row_for_update(&mut tx, proposal_id).await?;
        if row.state != "open" {
            return Err(ApiError::new(
                ApiErrorCode::OperationConflict,
                format!("proposal is already {}", row.state),
            ));
        }
        let expected_generation = i64::try_from(request.expected_generation).map_err(|_| {
            ApiError::new(
                ApiErrorCode::InvalidRequest,
                "generation exceeds database range",
            )
        })?;
        if row.proposal_generation != expected_generation {
            return Err(generation_conflict(row.proposal_generation));
        }
        match kind {
            ProposalCloseKind::Reject => {
                if !can_publish_target(
                    &mut tx,
                    authority.user_id,
                    authority.namespace_id,
                    row.target_owner_namespace_id,
                )
                .await?
                {
                    return Err(ApiError::new(ApiErrorCode::NotFound, "proposal not found"));
                }
            }
            ProposalCloseKind::Withdraw if row.proposer_user_id != authority.user_id => {
                return Err(ApiError::new(ApiErrorCode::NotFound, "proposal not found"));
            }
            _ => {}
        }
        let next = next_generation(row.proposal_generation)?;
        sqlx::query(
            "UPDATE skill_proposals SET generation=$1,state=$2,closed_revision_id=$3, \
             closed_source_generation=$4,closed_by_user_id=$5,closed_at=now(),updated_at=now() WHERE id=$6",
        )
        .bind(next)
        .bind(terminal)
        .bind(&row.source_revision_id)
        .bind(row.source_generation)
        .bind(authority.user_id)
        .bind(proposal_id)
        .execute(&mut *tx)
        .await
        .map_err(internal_api_error)?;
        let outcome = proposal_summary(&load_proposal_row(&mut tx, proposal_id).await?)?;
        record_operation(
            &mut tx,
            authority.user_id,
            operation_id,
            supplied_hash,
            action,
            proposal_id,
            &outcome,
        )
        .await?;
        tx.commit().await.map_err(internal_api_error)?;
        Ok(outcome)
    }

    pub async fn accept_proposal(
        &self,
        bearer: &str,
        request: &ProposalAcceptRequest,
    ) -> Result<SkillProposal, ApiError> {
        let authority = self.user_authority(bearer, "skills:write").await?;
        let operation_id = OperationId::from_str(&request.operation_id)
            .map_err(|error| ApiError::new(ApiErrorCode::InvalidRequest, error.to_string()))?;
        let proposal_id = parse_uuid(&request.proposal_id, "proposal ID")?;
        let expected_revision = RevisionId::from_str(&request.expected_proposed_revision_id)
            .map_err(|error| ApiError::new(ApiErrorCode::InvalidRequest, error.to_string()))?;
        let supplied_hash = parse_hash(&request.request_hash)?;
        let expected_hash = proposal_accept_request_hash(
            &request.operation_id,
            &request.proposal_id,
            request.expected_generation,
            &request.expected_proposed_revision_id,
            request.expected_source_generation,
            request.expected_target_generation,
        )
        .map_err(hash_error)?;
        ensure_hash(supplied_hash, expected_hash)?;

        let mut tx = self.begin_actor_tx(authority.user_id).await?;
        if let Some(outcome) = replay_operation::<SkillProposal>(
            &mut tx,
            authority.user_id,
            operation_id,
            supplied_hash,
            "accept",
        )
        .await?
        {
            tx.commit().await.map_err(internal_api_error)?;
            return Ok(outcome);
        }
        let row = load_proposal_row_for_update(&mut tx, proposal_id).await?;
        let target_authority =
            authorize_resource_publish(&mut tx, &authority, row.target_resource_id).await?;
        if target_authority.namespace_id != row.target_owner_namespace_id {
            return Err(ApiError::new(ApiErrorCode::NotFound, "proposal not found"));
        }
        if target_authority.is_team {
            let _ = ensure_private_workspace_for_user(
                &mut tx,
                row.target_resource_id,
                authority.user_id,
            )
            .await?;
        }
        ensure_open_and_expected(&row, request.expected_generation)?;
        if row.source_generation != i64_generation(request.expected_source_generation)?
            || row.source_revision_id.as_slice() != expected_revision.as_bytes()
        {
            return Err(ApiError::new(
                ApiErrorCode::GenerationConflict,
                "proposal fork advanced; inspect the current proposal before accepting",
            ));
        }
        let target_workspace = sqlx::query_as::<_, (i64, Vec<u8>)>(
            "SELECT generation,revision_id FROM skill_private_workspaces \
             WHERE resource_id=$1 AND workspace_user_id=$2 FOR UPDATE",
        )
        .bind(row.target_resource_id)
        .bind(authority.user_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(internal_api_error)?
        .ok_or_else(|| ApiError::new(ApiErrorCode::NotFound, "target workspace not found"))?;
        if target_workspace.0 != i64_generation(request.expected_target_generation)? {
            return Err(generation_conflict(target_workspace.0));
        }
        let still_fork = sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS(SELECT 1 FROM skill_forks WHERE resource_id=$1 AND upstream_resource_id=$2 AND promotion_pending=FALSE)",
        )
        .bind(row.source_resource_id)
        .bind(row.target_resource_id)
        .fetch_one(&mut *tx)
        .await
        .map_err(internal_api_error)?;
        if !still_fork {
            return Err(ApiError::new(
                ApiErrorCode::OperationConflict,
                "proposal source is no longer the target's fork",
            ));
        }
        let active_resources = sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS(SELECT 1 FROM resources source JOIN resources target ON target.id=$2 \
             WHERE source.id=$1 AND source.deleted_at IS NULL AND target.deleted_at IS NULL)",
        )
        .bind(row.source_resource_id)
        .bind(row.target_resource_id)
        .fetch_one(&mut *tx)
        .await
        .map_err(internal_api_error)?;
        if !active_resources {
            return Err(ApiError::new(
                ApiErrorCode::OperationConflict,
                "proposal source or target is deleted",
            ));
        }
        if row.source_name != row.target_name {
            return Err(ApiError::new(
                ApiErrorCode::OperationConflict,
                "proposal fork name no longer matches upstream; rename the fork to match upstream before accepting",
            ));
        }
        // Acceptance deliberately applies the exact proposal head. The accepting
        // maintainer may have unpublished private-workspace edits that are not ancestors
        // of the fork; those revisions remain immutable history, but they do not rewrite
        // or merge the accepted proposal head. `expected_target_generation` above is the
        // concurrency guard against accepting against a workspace that changed after the
        // maintainer inspected the proposal.
        attach_proposal_revision(
            self,
            &mut tx,
            row.source_resource_id,
            row.target_resource_id,
            row.target_owner_namespace_id,
            expected_revision.as_bytes(),
        )
        .await?;
        let source_metadata = crate::proposal_metadata::source_workspace_metadata(
            &mut tx,
            row.source_resource_id,
            row.proposer_user_id,
        )
        .await?;
        let snapshot = sqlx::query_as::<_, (Value, String, Vec<u8>, i64)>(
            "SELECT manifest_json,snapshot_key,snapshot_sha256,snapshot_size FROM resource_revision_snapshots \
             WHERE resource_id=$1 AND revision_id=$2",
        )
        .bind(row.target_resource_id)
        .bind(expected_revision.as_bytes().as_slice())
        .fetch_one(&mut *tx)
        .await
        .map_err(internal_api_error)?;
        let target_workspace_next = next_generation(target_workspace.0)?;
        let target_resource_next = if target_authority.is_team {
            row.target_generation
        } else {
            next_generation(row.target_generation)?
        };
        if !target_authority.is_team {
            sqlx::query(
                "UPDATE resources SET generation=$1,description=$2,license=$3,compatibility=$4 WHERE id=$5",
            )
                .bind(target_resource_next)
                .bind(&source_metadata.0)
                .bind(&source_metadata.1)
                .bind(&source_metadata.2)
                .bind(row.target_resource_id)
                .execute(&mut *tx)
                .await
                .map_err(internal_api_error)?;
        }
        sqlx::query(
            "UPDATE skill_private_workspaces SET revision_id=$1,generation=$2,description=$3,license=$4,compatibility=$5,manifest_json=$6, \
             snapshot_key=$7,snapshot_sha256=$8,snapshot_size=$9,updated_at=now() \
             WHERE resource_id=$10 AND workspace_user_id=$11",
        )
        .bind(expected_revision.as_bytes().as_slice())
        .bind(target_workspace_next)
        .bind(&source_metadata.0)
        .bind(&source_metadata.1)
        .bind(&source_metadata.2)
        .bind(snapshot.0)
        .bind(snapshot.1)
        .bind(snapshot.2)
        .bind(snapshot.3)
        .bind(row.target_resource_id)
        .bind(authority.user_id)
        .execute(&mut *tx)
        .await
        .map_err(internal_api_error)?;

        let proposal_next = next_generation(row.proposal_generation)?;
        sqlx::query(
            "UPDATE skill_proposals SET generation=$1,state='accepted',closed_revision_id=$2, \
             closed_source_generation=$3,closed_by_user_id=$4,closed_at=now(),updated_at=now() WHERE id=$5",
        )
        .bind(proposal_next)
        .bind(expected_revision.as_bytes().as_slice())
        .bind(row.source_generation)
        .bind(authority.user_id)
        .bind(proposal_id)
        .execute(&mut *tx)
        .await
        .map_err(internal_api_error)?;
        let outcome = proposal_summary(&load_proposal_row(&mut tx, proposal_id).await?)?;
        record_operation(
            &mut tx,
            authority.user_id,
            operation_id,
            supplied_hash,
            "accept",
            proposal_id,
            &outcome,
        )
        .await?;
        if !target_authority.is_team {
            enqueue_resource_wake(
                &mut tx,
                row.target_resource_id,
                generation_u64(target_resource_next)?,
            )
            .await?;
        }
        tx.commit().await.map_err(internal_api_error)?;
        if !target_authority.is_team {
            let _ = self.drain_outbox(64).await;
        }
        Ok(outcome)
    }
}

async fn load_proposal_row(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    proposal_id: Uuid,
) -> Result<ProposalRow, ApiError> {
    proposal_query(tx, proposal_id).await
}

async fn load_proposal_row_for_update(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    proposal_id: Uuid,
) -> Result<ProposalRow, ApiError> {
    let locked = sqlx::query_scalar::<_, bool>("SELECT locked FROM denju_lock_proposal_rows($1)")
        .bind(proposal_id)
        .fetch_optional(&mut **tx)
        .await
        .map_err(internal_api_error)?;
    if locked != Some(true) {
        return Err(ApiError::new(ApiErrorCode::NotFound, "proposal not found"));
    }
    proposal_query(tx, proposal_id).await
}

async fn proposal_query(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    proposal_id: Uuid,
) -> Result<ProposalRow, ApiError> {
    const SELECT_PROPOSAL: &str = "SELECT p.id AS proposal_id,p.generation AS proposal_generation,p.state,p.message, \
         p.proposer_user_id,proposer_ns.id AS proposer_namespace_id,proposer_ns.slug AS proposer, \
         source.id AS source_resource_id,source_owner.slug AS source_owner,source.slug AS source_name, \
         source.generation AS source_generation,source_workspace.revision_id AS source_revision_id, \
         target.id AS target_resource_id,target.owner_namespace_id AS target_owner_namespace_id,target_owner.kind AS target_owner_kind, \
         target_owner.slug AS target_owner,target.slug AS target_name,target.generation AS target_generation, \
         target.visibility AS target_visibility,COALESCE(target_workspace.revision_id,release.revision_id) AS target_revision_id, \
         release.revision_id AS target_release_revision_id, \
         EXISTS(SELECT 1 FROM private_skill_shares share WHERE share.resource_id=target.id AND share.recipient_user_id=p.proposer_user_id) AS target_shared_with_proposer, \
         EXISTS(SELECT 1 FROM team_memberships tm WHERE tm.team_namespace_id=target.owner_namespace_id AND tm.user_id=p.proposer_user_id) AS target_team_with_proposer, \
         fork.sync_base_revision_id,p.closed_revision_id,p.closed_source_generation \
         FROM skill_proposals p JOIN users proposer_user ON proposer_user.id=p.proposer_user_id \
         JOIN namespaces proposer_ns ON proposer_ns.id=proposer_user.namespace_id \
         JOIN resources source ON source.id=p.source_resource_id JOIN namespaces source_owner ON source_owner.id=source.owner_namespace_id \
         JOIN skill_private_workspaces source_workspace ON source_workspace.resource_id=source.id AND source_workspace.workspace_user_id=p.proposer_user_id \
         JOIN skill_forks fork ON fork.resource_id=source.id \
         JOIN resources target ON target.id=p.target_resource_id JOIN namespaces target_owner ON target_owner.id=target.owner_namespace_id \
         LEFT JOIN users target_owner_user ON target_owner_user.namespace_id=target.owner_namespace_id \
         LEFT JOIN skill_private_workspaces target_workspace ON target_workspace.resource_id=target.id AND target_workspace.workspace_user_id=target_owner_user.id \
         LEFT JOIN skill_releases release ON release.resource_id=target.id AND release.version=target.latest_release_version \
         WHERE p.id=$1";
    let row = sqlx::query_as::<_, ProposalRow>(SELECT_PROPOSAL)
        .bind(proposal_id)
        .fetch_optional(&mut **tx)
        .await
        .map_err(internal_api_error)?;
    row.ok_or_else(|| ApiError::new(ApiErrorCode::NotFound, "proposal not found"))
}

fn proposal_summary(row: &ProposalRow) -> Result<SkillProposal, ApiError> {
    let terminal = row.state != "open";
    let revision = if terminal {
        row.closed_revision_id.as_ref().ok_or_else(|| {
            ApiError::new(
                ApiErrorCode::Internal,
                "closed proposal is missing its revision",
            )
        })?
    } else {
        &row.source_revision_id
    };
    let source_generation = if terminal {
        row.closed_source_generation.ok_or_else(|| {
            ApiError::new(
                ApiErrorCode::Internal,
                "closed proposal is missing its source generation",
            )
        })?
    } else {
        row.source_generation
    };
    let state = match row.state.as_str() {
        "open" => {
            let visible_target = if row.target_owner_kind == "team" {
                if row.target_shared_with_proposer || row.target_team_with_proposer {
                    // A personal private target can expose its live workspace to a share,
                    // but a team target never exposes another maintainer's draft. A transfer
                    // can temporarily produce a team target with no release; in that case the
                    // transfer itself did not advance content, so the proposal's recorded sync
                    // base remains the comparison point until the first team release appears.
                    row.target_release_revision_id
                        .as_deref()
                        .or(Some(row.sync_base_revision_id.as_slice()))
                } else if row.target_visibility == "public" {
                    row.target_release_revision_id.as_deref()
                } else {
                    None
                }
            } else if row.target_owner_namespace_id == row.proposer_namespace_id
                || row.target_shared_with_proposer
            {
                row.target_revision_id.as_deref()
            } else if row.target_visibility == "public" {
                row.target_release_revision_id.as_deref()
            } else {
                None
            };
            if visible_target == Some(row.sync_base_revision_id.as_slice()) {
                SkillProposalState::Open
            } else {
                SkillProposalState::NeedsSync
            }
        }
        "accepted" => SkillProposalState::Accepted,
        "rejected" => SkillProposalState::Rejected,
        "withdrawn" => SkillProposalState::Withdrawn,
        _ => {
            return Err(ApiError::new(
                ApiErrorCode::Internal,
                "stored proposal state is invalid",
            ));
        }
    };
    Ok(SkillProposal {
        proposal_id: row.proposal_id.to_string(),
        generation: generation_u64(row.proposal_generation)?,
        state,
        proposer: row.proposer.clone(),
        source_resource_id: row.source_resource_id.to_string(),
        source_locator: format!("@{}/{}", row.source_owner, row.source_name),
        source_generation: generation_u64(source_generation)?,
        target_resource_id: row.target_resource_id.to_string(),
        target_locator: format!("@{}/{}", row.target_owner, row.target_name),
        target_generation: generation_u64(row.target_generation)?,
        proposed_revision_id: hex::encode(decode_32(revision, "proposal revision")?),
        message: row.message.clone(),
    })
}

fn ensure_open_and_expected(row: &ProposalRow, expected_generation: u64) -> Result<(), ApiError> {
    if row.state != "open" {
        return Err(ApiError::new(
            ApiErrorCode::OperationConflict,
            format!("proposal is already {}", row.state),
        ));
    }
    if row.proposal_generation != i64_generation(expected_generation)? {
        return Err(generation_conflict(row.proposal_generation));
    }
    Ok(())
}

async fn can_publish_target(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    user_id: Uuid,
    personal_namespace_id: Uuid,
    target_namespace_id: Uuid,
) -> Result<bool, ApiError> {
    if target_namespace_id == personal_namespace_id {
        return Ok(true);
    }
    sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(SELECT 1 FROM team_memberships tm JOIN teams t ON t.namespace_id=tm.team_namespace_id \
         WHERE tm.team_namespace_id=$1 AND tm.user_id=$2 AND \
           (tm.role IN ('owner','maintainer') OR (tm.role='member' AND t.members_can_publish)))",
    )
    .bind(target_namespace_id)
    .bind(user_id)
    .fetch_one(&mut **tx)
    .await
    .map_err(internal_api_error)
}

async fn workspace_generation_or_resource(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    resource_id: Uuid,
    user_id: Uuid,
    fallback_resource_generation: i64,
) -> Result<u64, ApiError> {
    let generation = sqlx::query_scalar::<_, i64>(
        "SELECT generation FROM skill_private_workspaces WHERE resource_id=$1 AND workspace_user_id=$2",
    )
    .bind(resource_id)
    .bind(user_id)
    .fetch_optional(&mut **tx)
    .await
    .map_err(internal_api_error)?
    .unwrap_or(fallback_resource_generation);
    generation_u64(generation)
}

async fn attach_proposal_revision(
    registry: &Registry,
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    source_resource_id: Uuid,
    target_resource_id: Uuid,
    target_namespace_id: Uuid,
    revision_id: &[u8; 32],
) -> Result<(), ApiError> {
    let exists = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(SELECT 1 FROM resource_revision_snapshots WHERE resource_id=$1 AND revision_id=$2)",
    )
    .bind(target_resource_id)
    .bind(revision_id.as_slice())
    .fetch_one(&mut **tx)
    .await
    .map_err(internal_api_error)?;
    if exists {
        return Ok(());
    }
    let source_snapshot = sqlx::query_as::<_, (Value, String, Vec<u8>, i64)>(
        "SELECT manifest_json,snapshot_key,snapshot_sha256,snapshot_size FROM resource_revision_snapshots \
         WHERE resource_id=$1 AND revision_id=$2",
    )
    .bind(source_resource_id)
    .bind(revision_id.as_slice())
    .fetch_optional(&mut **tx)
    .await
    .map_err(internal_api_error)?
    .ok_or_else(|| ApiError::new(ApiErrorCode::OperationConflict, "proposal revision is unavailable"))?;
    let manifest_wire: PublicSkillManifest = serde_json::from_value(source_snapshot.0.clone())
        .map_err(|error| ApiError::new(ApiErrorCode::Internal, error.to_string()))?;
    let manifest = manifest_wire
        .to_core()
        .map_err(|error| ApiError::new(ApiErrorCode::Internal, error))?;
    let blobs = manifest_blobs(&manifest)?;
    enforce_namespace_quota(registry, tx, target_namespace_id, &blobs).await?;
    for blob in blobs.keys() {
        sqlx::query(
            "INSERT INTO resource_blob_reachability (resource_id,blob_id,reference_count) VALUES ($1,$2,1) \
             ON CONFLICT(resource_id,blob_id) DO UPDATE SET reference_count=resource_blob_reachability.reference_count+1",
        )
        .bind(target_resource_id)
        .bind(blob.as_bytes().as_slice())
        .execute(&mut **tx)
        .await
        .map_err(internal_api_error)?;
        sqlx::query(
            "INSERT INTO namespace_blob_reachability (namespace_id,blob_id,reference_count) VALUES ($1,$2,1) \
             ON CONFLICT(namespace_id,blob_id) DO UPDATE SET reference_count=namespace_blob_reachability.reference_count+1",
        )
        .bind(target_namespace_id)
        .bind(blob.as_bytes().as_slice())
        .execute(&mut **tx)
        .await
        .map_err(internal_api_error)?;
        sqlx::query("SELECT denju_cancel_blob_gc($1)")
            .bind(blob.as_bytes().as_slice())
            .execute(&mut **tx)
            .await
            .map_err(internal_api_error)?;
    }
    sqlx::query(
        "INSERT INTO resource_revision_snapshots \
         (resource_id,revision_id,manifest_json,snapshot_key,snapshot_sha256,snapshot_size) VALUES ($1,$2,$3,$4,$5,$6)",
    )
    .bind(target_resource_id)
    .bind(revision_id.as_slice())
    .bind(source_snapshot.0)
    .bind(source_snapshot.1)
    .bind(source_snapshot.2)
    .bind(source_snapshot.3)
    .execute(&mut **tx)
    .await
    .map_err(internal_api_error)?;
    Ok(())
}

async fn replay_operation<T: DeserializeOwned>(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    user_id: Uuid,
    operation_id: OperationId,
    request_hash: RequestHash,
    action: &str,
) -> Result<Option<T>, ApiError> {
    let row = sqlx::query(
        "SELECT request_hash,action,outcome_json FROM skill_proposal_operations WHERE user_id=$1 AND operation_id=$2",
    )
    .bind(user_id)
    .bind(operation_id.as_uuid())
    .fetch_optional(&mut **tx)
    .await
    .map_err(internal_api_error)?;
    let Some(row) = row else { return Ok(None) };
    let stored_hash: Vec<u8> = row.get(0);
    let stored_action: String = row.get(1);
    if stored_hash.as_slice() != request_hash.as_bytes() || stored_action != action {
        return Err(ApiError::new(
            ApiErrorCode::OperationConflict,
            "operation_id was already used with different proposal content",
        ));
    }
    serde_json::from_value(row.get(2))
        .map(Some)
        .map_err(|error| ApiError::new(ApiErrorCode::Internal, error.to_string()))
}

async fn record_operation<T: Serialize>(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    user_id: Uuid,
    operation_id: OperationId,
    request_hash: RequestHash,
    action: &str,
    proposal_id: Uuid,
    outcome: &T,
) -> Result<(), ApiError> {
    sqlx::query(
        "INSERT INTO skill_proposal_operations (user_id,operation_id,request_hash,action,proposal_id,outcome_json) \
         VALUES ($1,$2,$3,$4,$5,$6)",
    )
    .bind(user_id)
    .bind(operation_id.as_uuid())
    .bind(request_hash.as_bytes().as_slice())
    .bind(action)
    .bind(proposal_id)
    .bind(serde_json::to_value(outcome).map_err(|error| ApiError::new(ApiErrorCode::Internal, error.to_string()))?)
    .execute(&mut **tx)
    .await
    .map_err(internal_api_error)?;
    Ok(())
}

fn parse_uuid(value: &str, label: &str) -> Result<Uuid, ApiError> {
    Uuid::parse_str(value).map_err(|error| {
        ApiError::new(
            ApiErrorCode::InvalidRequest,
            format!("invalid {label}: {error}"),
        )
    })
}

fn parse_hash(value: &str) -> Result<RequestHash, ApiError> {
    RequestHash::from_str(value).map_err(hash_error)
}

fn hash_error(error: denju_wire::RequestHashError) -> ApiError {
    ApiError::new(ApiErrorCode::InvalidRequestHash, error.to_string())
}

fn ensure_hash(supplied: RequestHash, expected: RequestHash) -> Result<(), ApiError> {
    if supplied == expected {
        Ok(())
    } else {
        Err(ApiError::new(
            ApiErrorCode::InvalidRequestHash,
            "request_hash does not match the canonical proposal payload",
        ))
    }
}

fn i64_generation(value: u64) -> Result<i64, ApiError> {
    i64::try_from(value).map_err(|_| {
        ApiError::new(
            ApiErrorCode::InvalidRequest,
            "generation exceeds database range",
        )
    })
}

fn generation_conflict(current: i64) -> ApiError {
    ApiError::new(
        ApiErrorCode::GenerationConflict,
        format!("resource advanced to generation {current}"),
    )
}
