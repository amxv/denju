use denju_core::OperationId;
use denju_wire::{
    CliErrorCode, ProposalAcceptRequest, ProposalCloseKind, ProposalCloseRequest,
    ProposalCreateRequest, SkillProposal, SkillProposalDetail, SkillProposalList,
    SkillProposalState, proposal_accept_request_hash, proposal_close_request_hash,
    proposal_create_request_hash,
};
use uuid::Uuid;

use crate::{
    fork_ops::{find_owned, internal_error, require_identity},
    public::{SyncOutcome, client_error, installed_context, local_error},
    setup::RuntimeError,
};

pub(crate) async fn create(
    locator: &str,
    message: Option<&str>,
) -> Result<SkillProposal, RuntimeError> {
    let context = installed_context(true).await?;
    require_identity(&context).await?;
    let fork = find_owned(&context, locator).await?;
    if fork.fork.is_none() {
        return Err(RuntimeError::new(
            CliErrorCode::InvalidArguments,
            format!("{} is not a fork", fork.locator),
        ));
    }
    let operation_id = new_operation()?;
    let operation = operation_id.to_string();
    let request_hash =
        proposal_create_request_hash(&operation, &fork.resource_id, fork.generation, message)
            .map_err(internal_error)?;
    context
        .client
        .create_proposal(&ProposalCreateRequest {
            operation_id: operation,
            source_resource_id: fork.resource_id,
            expected_source_generation: fork.generation,
            message: message.map(str::to_owned),
            request_hash: request_hash.to_string(),
        })
        .await
        .map_err(client_error)
}

pub(crate) async fn list() -> Result<SkillProposalList, RuntimeError> {
    installed_context(true)
        .await?
        .client
        .proposals()
        .await
        .map_err(client_error)
}

pub(crate) async fn show(proposal_id: &str) -> Result<SkillProposalDetail, RuntimeError> {
    installed_context(true)
        .await?
        .client
        .proposal_detail(proposal_id)
        .await
        .map_err(client_error)
}

pub(crate) async fn accept(proposal_id: &str) -> Result<SkillProposal, RuntimeError> {
    let context = installed_context(true).await?;
    let detail = context
        .client
        .proposal_detail(proposal_id)
        .await
        .map_err(client_error)?;
    if !matches!(detail.proposal.state, SkillProposalState::Open) {
        return Err(proposal_not_open(&detail.proposal));
    }
    let operation_id = new_operation()?;
    let operation = operation_id.to_string();
    let request_hash = proposal_accept_request_hash(
        &operation,
        &detail.proposal.proposal_id,
        detail.proposal.generation,
        &detail.proposal.proposed_revision_id,
        detail.proposal.source_generation,
        detail.proposal.target_generation,
    )
    .map_err(internal_error)?;
    context
        .client
        .accept_proposal(&ProposalAcceptRequest {
            operation_id: operation,
            proposal_id: detail.proposal.proposal_id,
            expected_generation: detail.proposal.generation,
            expected_proposed_revision_id: detail.proposal.proposed_revision_id,
            expected_source_generation: detail.proposal.source_generation,
            expected_target_generation: detail.proposal.target_generation,
            request_hash: request_hash.to_string(),
        })
        .await
        .map_err(client_error)
}

pub(crate) async fn reject(proposal_id: &str) -> Result<SkillProposal, RuntimeError> {
    close(proposal_id, ProposalCloseKind::Reject).await
}

pub(crate) async fn withdraw(proposal_id: &str) -> Result<SkillProposal, RuntimeError> {
    close(proposal_id, ProposalCloseKind::Withdraw).await
}

async fn close(proposal_id: &str, kind: ProposalCloseKind) -> Result<SkillProposal, RuntimeError> {
    let context = installed_context(true).await?;
    let detail = context
        .client
        .proposal_detail(proposal_id)
        .await
        .map_err(client_error)?;
    if !matches!(
        detail.proposal.state,
        SkillProposalState::Open | SkillProposalState::NeedsSync
    ) {
        return Err(proposal_not_open(&detail.proposal));
    }
    let operation_id = new_operation()?;
    let operation = operation_id.to_string();
    let request_hash = proposal_close_request_hash(
        kind,
        &operation,
        &detail.proposal.proposal_id,
        detail.proposal.generation,
    )
    .map_err(internal_error)?;
    let request = ProposalCloseRequest {
        operation_id: operation,
        proposal_id: detail.proposal.proposal_id,
        expected_generation: detail.proposal.generation,
        request_hash: request_hash.to_string(),
    };
    match kind {
        ProposalCloseKind::Reject => context.client.reject_proposal(&request).await,
        ProposalCloseKind::Withdraw => context.client.withdraw_proposal(&request).await,
    }
    .map_err(client_error)
}

pub(crate) async fn sync_once() -> Result<SyncOutcome, RuntimeError> {
    let outcome = crate::public::sync_once().await?;
    let context = installed_context(false).await?;
    let identity = context.db.identity().await.map_err(local_error)?;
    if !identity.is_some_and(|identity| identity.session_backend.is_some()) {
        return Ok(outcome);
    }
    let context = installed_context(true).await?;
    let owned_resource_ids = context
        .db
        .owned_skills()
        .await
        .map_err(local_error)?
        .into_iter()
        .map(|skill| skill.resource_id)
        .collect::<std::collections::HashSet<_>>();
    let proposals = context.client.proposals().await.map_err(client_error)?;
    for proposal in proposals.proposals {
        if owned_resource_ids.contains(&proposal.source_resource_id)
            && matches!(proposal.state, SkillProposalState::NeedsSync)
        {
            crate::fork_sync::sync(&proposal.source_locator).await?;
        }
    }
    Ok(outcome)
}

fn new_operation() -> Result<OperationId, RuntimeError> {
    OperationId::from_uuid(Uuid::now_v7()).map_err(internal_error)
}

fn proposal_not_open(proposal: &SkillProposal) -> RuntimeError {
    RuntimeError::new(
        CliErrorCode::InvalidArguments,
        format!("proposal {} is already terminal", proposal.proposal_id),
    )
}
