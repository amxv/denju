use denju_core::RevisionId;
use denju_wire::{ApiError, ApiErrorCode};

use crate::{ingest::decode_32, internal_api_error};

pub(crate) async fn revision_is_ancestor(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    ancestor: &[u8; 32],
    descendant: &[u8; 32],
) -> Result<bool, ApiError> {
    sqlx::query_scalar::<_, bool>(
        "WITH RECURSIVE ancestry(revision_id) AS ( \
             SELECT $1::bytea \
             UNION \
             SELECT rp.parent_revision_id FROM revision_parents rp \
             JOIN ancestry a ON rp.revision_id=a.revision_id \
         ) SELECT EXISTS(SELECT 1 FROM ancestry WHERE revision_id=$2)",
    )
    .bind(descendant.as_slice())
    .bind(ancestor.as_slice())
    .fetch_one(&mut **tx)
    .await
    .map_err(internal_api_error)
}

pub(crate) async fn merge_base(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    left: &[u8; 32],
    right: &[u8; 32],
) -> Result<[u8; 32], ApiError> {
    let base = sqlx::query_scalar::<_, Vec<u8>>(
        "WITH RECURSIVE \
         left_ancestry(revision_id,depth) AS ( \
           SELECT $1::bytea,0 \
           UNION \
           SELECT rp.parent_revision_id,a.depth+1 FROM revision_parents rp \
           JOIN left_ancestry a ON rp.revision_id=a.revision_id WHERE a.depth<4096 \
         ), \
         right_ancestry(revision_id,depth) AS ( \
           SELECT $2::bytea,0 \
           UNION \
           SELECT rp.parent_revision_id,a.depth+1 FROM revision_parents rp \
           JOIN right_ancestry a ON rp.revision_id=a.revision_id WHERE a.depth<4096 \
         ), \
         left_min AS (SELECT revision_id,min(depth) AS depth FROM left_ancestry GROUP BY revision_id), \
         right_min AS (SELECT revision_id,min(depth) AS depth FROM right_ancestry GROUP BY revision_id) \
         SELECT l.revision_id FROM left_min l JOIN right_min r USING(revision_id) \
         ORDER BY GREATEST(l.depth,r.depth),l.depth+r.depth,l.revision_id LIMIT 1",
    )
    .bind(left.as_slice())
    .bind(right.as_slice())
    .fetch_optional(&mut **tx)
    .await
    .map_err(internal_api_error)?
    .ok_or_else(|| {
        ApiError::new(
            ApiErrorCode::GenerationConflict,
            "maintainer workspace and latest team release have no common revision ancestry",
        )
    })?;
    base.try_into().map_err(|_| {
        ApiError::new(
            ApiErrorCode::Internal,
            "stored merge-base revision ID has invalid length",
        )
    })
}

pub(crate) async fn revision_parents(
    pool: &sqlx::PgPool,
    revision_id: &str,
) -> Result<Vec<String>, ApiError> {
    let revision = revision_id
        .parse::<RevisionId>()
        .map_err(|error| ApiError::new(ApiErrorCode::Internal, error.to_string()))?;
    let rows = sqlx::query_scalar::<_, Vec<u8>>(
        "SELECT parent_revision_id FROM revision_parents WHERE revision_id=$1 ORDER BY ordinal",
    )
    .bind(revision.as_bytes().as_slice())
    .fetch_all(pool)
    .await
    .map_err(internal_api_error)?;
    rows.into_iter()
        .map(|bytes| decode_32(&bytes, "stored parent revision ID").map(hex::encode))
        .collect()
}
