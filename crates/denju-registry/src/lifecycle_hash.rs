use std::str::FromStr;

use denju_wire::{ApiError, ApiErrorCode, RequestHash};

pub(crate) fn validate_lifecycle_hash(
    supplied: &str,
    expected: Result<RequestHash, denju_wire::RequestHashError>,
) -> Result<RequestHash, ApiError> {
    let supplied = RequestHash::from_str(supplied)
        .map_err(|error| ApiError::new(ApiErrorCode::InvalidRequestHash, error.to_string()))?;
    let expected = expected
        .map_err(|error| ApiError::new(ApiErrorCode::InvalidRequestHash, error.to_string()))?;
    if supplied == expected {
        Ok(supplied)
    } else {
        Err(ApiError::new(
            ApiErrorCode::InvalidRequestHash,
            "request_hash does not match the canonical lifecycle payload",
        ))
    }
}
