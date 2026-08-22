use axum::http::{HeaderMap, header::AUTHORIZATION};
use denju_wire::{ApiError, ApiErrorCode};

use super::ApiResponseError;

pub(super) fn bearer_token(headers: &HeaderMap) -> Result<&str, ApiResponseError> {
    headers
        .get(AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            ApiResponseError(ApiError::new(
                ApiErrorCode::Unauthorized,
                "installation credential required",
            ))
        })
}

pub(super) fn optional_bearer_token(headers: &HeaderMap) -> Option<&str> {
    headers
        .get(AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .filter(|value| !value.is_empty())
}

pub(super) fn recovery_bearer_token(headers: &HeaderMap) -> Result<(), ApiResponseError> {
    let expected = std::env::var("DENJU_RECOVERY_TOKEN")
        .ok()
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            ApiResponseError(ApiError::new(
                ApiErrorCode::Unavailable,
                "registry recovery trigger is not configured",
            ))
        })?;
    let supplied = bearer_token(headers)?;
    if supplied == expected {
        Ok(())
    } else {
        Err(ApiResponseError(ApiError::new(
            ApiErrorCode::Unauthorized,
            "recovery credential rejected",
        )))
    }
}
