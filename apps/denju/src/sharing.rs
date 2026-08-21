use denju_core::OperationId;
use denju_wire::{
    CliErrorCode, ShareMutationKind, ShareSkillRequest, ShareSkillResponse,
    share_skill_request_hash,
};
use uuid::Uuid;

use crate::{
    public::{client_error, installed_context, local_error},
    setup::RuntimeError,
};

pub(crate) async fn mutate(
    locator: &str,
    recipient: &str,
    kind: ShareMutationKind,
) -> Result<ShareSkillResponse, RuntimeError> {
    let context = installed_context(true).await?;
    let owned = context
        .db
        .owned_skills()
        .await
        .map_err(local_error)?
        .into_iter()
        .find(|skill| skill.locator == locator)
        .ok_or_else(|| {
            RuntimeError::new(
                CliErrorCode::NotFound,
                format!("{locator} is not an owned skill on this identity"),
            )
        })?;
    let operation_id = OperationId::from_uuid(Uuid::now_v7())
        .map_err(|error| RuntimeError::new(CliErrorCode::Internal, error.to_string()))?;
    let request_hash = share_skill_request_hash(
        kind,
        &operation_id.to_string(),
        &owned.resource_id,
        recipient,
    )
    .map_err(|error| RuntimeError::new(CliErrorCode::Internal, error.to_string()))?;
    context
        .client
        .mutate_private_share(
            kind,
            &ShareSkillRequest {
                operation_id: operation_id.to_string(),
                resource_id: owned.resource_id,
                recipient: recipient.to_owned(),
                request_hash: request_hash.to_string(),
            },
        )
        .await
        .map_err(client_error)
}
