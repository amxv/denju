use std::collections::BTreeSet;

use denju_wire::{ApiError, ApiErrorCode};

pub(crate) fn validate_release_metadata(
    message: Option<&str>,
    tags: &[String],
) -> Result<(), ApiError> {
    if message.is_some_and(|value| value.len() > 4096) {
        return Err(ApiError::new(
            ApiErrorCode::InvalidRequest,
            "release message exceeds 4096 bytes",
        ));
    }
    let mut unique = BTreeSet::new();
    for tag in tags {
        if tag.is_empty()
            || tag.len() > 64
            || !tag
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
        {
            return Err(ApiError::new(
                ApiErrorCode::InvalidRequest,
                "release tags must be 1-64 ASCII letters, digits, '.', '_' or '-'",
            ));
        }
        if !unique.insert(tag) {
            return Err(ApiError::new(
                ApiErrorCode::InvalidRequest,
                "release tags must be unique",
            ));
        }
    }
    Ok(())
}
