use axum::http::{HeaderMap, header::AUTHORIZATION};
use denju_wire::{ApiError, ApiErrorCode};
use sha2::{Digest, Sha256};

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
    if constant_time_secret_eq(supplied.as_bytes(), expected.as_bytes()) {
        Ok(())
    } else {
        Err(ApiResponseError(ApiError::new(
            ApiErrorCode::Unauthorized,
            "recovery credential rejected",
        )))
    }
}

fn constant_time_secret_eq(left: &[u8], right: &[u8]) -> bool {
    let left: [u8; 32] = Sha256::digest(left).into();
    let right: [u8; 32] = Sha256::digest(right).into();
    left.iter()
        .zip(right)
        .fold(0_u8, |diff, (left, right)| diff | (left ^ right))
        == 0
}

#[cfg(test)]
mod tests {
    use super::constant_time_secret_eq;

    #[test]
    fn secret_comparison_hashes_before_comparing() {
        assert!(constant_time_secret_eq(
            b"recovery-token",
            b"recovery-token"
        ));
        assert!(!constant_time_secret_eq(
            b"recovery-token",
            b"recovery-tokeN"
        ));
        assert!(!constant_time_secret_eq(b"short", b"a-much-longer-secret"));
    }
}
