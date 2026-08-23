use denju_wire::ApiError;
use uuid::Uuid;

use crate::internal_api_error;

pub(crate) type WorkspaceDiscoveryMetadata = (String, Option<String>, Option<String>);
const MAX_PROPOSAL_MESSAGE_CHARS: usize = 500;

pub(crate) fn validate_message(message: Option<&str>) -> Result<(), ApiError> {
    if message.is_some_and(|message| message.chars().count() > MAX_PROPOSAL_MESSAGE_CHARS) {
        return Err(ApiError::new(
            denju_wire::ApiErrorCode::InvalidRequest,
            format!("proposal message exceeds {MAX_PROPOSAL_MESSAGE_CHARS} characters"),
        ));
    }
    Ok(())
}

pub(crate) async fn source_workspace_metadata(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    resource_id: Uuid,
    user_id: Uuid,
) -> Result<WorkspaceDiscoveryMetadata, ApiError> {
    sqlx::query_as(
        "SELECT description,license,compatibility FROM skill_private_workspaces \
         WHERE resource_id=$1 AND workspace_user_id=$2",
    )
    .bind(resource_id)
    .bind(user_id)
    .fetch_one(&mut **tx)
    .await
    .map_err(internal_api_error)
}
