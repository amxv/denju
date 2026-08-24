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
    let denju_token = std::env::var("DENJU_RECOVERY_TOKEN").ok();
    let cron_secret = std::env::var("CRON_SECRET").ok();
    let expected = configured_recovery_secret(denju_token.as_deref(), cron_secret.as_deref())
        .map_err(|message| ApiResponseError(ApiError::new(ApiErrorCode::Unavailable, message)))?
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

fn configured_recovery_secret<'a>(
    denju_token: Option<&'a str>,
    cron_secret: Option<&'a str>,
) -> Result<Option<&'a str>, &'static str> {
    let denju_token = denju_token.filter(|value| !value.is_empty());
    let cron_secret = cron_secret.filter(|value| !value.is_empty());
    match (denju_token, cron_secret) {
        (Some(denju), Some(cron)) => {
            if constant_time_secret_eq(denju.as_bytes(), cron.as_bytes()) {
                Ok(Some(cron))
            } else {
                Err("DENJU_RECOVERY_TOKEN and CRON_SECRET must match when both are configured")
            }
        }
        (Some(denju), None) => Ok(Some(denju)),
        (None, Some(cron)) => Ok(Some(cron)),
        (None, None) => Ok(None),
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
    use super::{configured_recovery_secret, constant_time_secret_eq};

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

    #[test]
    fn recovery_secret_accepts_portable_or_vercel_configuration_but_rejects_drift() {
        assert_eq!(
            configured_recovery_secret(Some("portable"), None).unwrap(),
            Some("portable")
        );
        assert_eq!(
            configured_recovery_secret(None, Some("vercel")).unwrap(),
            Some("vercel")
        );
        assert_eq!(
            configured_recovery_secret(Some("same"), Some("same")).unwrap(),
            Some("same")
        );
        assert!(configured_recovery_secret(Some("left"), Some("right")).is_err());
        assert_eq!(
            configured_recovery_secret(Some(""), Some("")).unwrap(),
            None
        );
    }
}
