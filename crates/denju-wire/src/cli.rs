use serde::Serialize;

pub const CLI_ENVELOPE_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CliEnvelope<T> {
    version: u32,
    ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    result: Option<T>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<CliError>,
}

impl<T> CliEnvelope<T> {
    pub const fn success(result: T) -> Self {
        Self {
            version: CLI_ENVELOPE_VERSION,
            ok: true,
            result: Some(result),
            error: None,
        }
    }

    pub const fn failure(error: CliError) -> Self {
        Self {
            version: CLI_ENVELOPE_VERSION,
            ok: false,
            result: None,
            error: Some(error),
        }
    }

    pub const fn version(&self) -> u32 {
        self.version
    }

    pub const fn is_ok(&self) -> bool {
        self.ok
    }

    pub const fn result(&self) -> Option<&T> {
        self.result.as_ref()
    }

    pub const fn error(&self) -> Option<&CliError> {
        self.error.as_ref()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CliError {
    code: CliErrorCode,
    message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    recovery: Option<String>,
}

impl CliError {
    pub fn new(code: CliErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            recovery: None,
        }
    }

    pub fn with_recovery(mut self, recovery: impl Into<String>) -> Self {
        self.recovery = Some(recovery.into());
        self
    }

    pub const fn code(&self) -> CliErrorCode {
        self.code
    }

    pub fn message(&self) -> &str {
        &self.message
    }

    pub fn recovery(&self) -> Option<&str> {
        self.recovery.as_deref()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CliErrorCode {
    InvalidArguments,
    SetupRequired,
    RegistryLocked,
    RegistryUnavailable,
    LocalState,
    CredentialUnavailable,
    ServiceUnavailable,
    NotFound,
    ContentVerification,
    Internal,
}

#[cfg(test)]
mod tests {
    use serde::Serialize;
    use serde_json::{Value, json};

    use super::*;

    #[derive(Debug, Serialize)]
    struct ResultPayload<'a> {
        state: &'a str,
    }

    #[test]
    fn success_envelope_has_one_versioned_result() {
        let envelope = CliEnvelope::success(ResultPayload { state: "ready" });
        assert_eq!(envelope.version(), 1);
        assert!(envelope.is_ok());
        assert!(envelope.result().is_some());
        assert!(envelope.error().is_none());
        let value = serde_json::to_value(envelope).expect("serializable envelope");
        assert_eq!(
            value,
            json!({
                "version": 1,
                "ok": true,
                "result": {"state": "ready"}
            })
        );
    }

    #[test]
    fn failure_envelope_has_stable_machine_error_code() {
        let envelope = CliEnvelope::<Value>::failure(
            CliError::new(CliErrorCode::InvalidArguments, "invalid command")
                .with_recovery("denju --help"),
        );
        assert_eq!(envelope.version(), 1);
        assert!(!envelope.is_ok());
        assert!(envelope.result().is_none());
        assert_eq!(
            envelope.error().map(CliError::code),
            Some(CliErrorCode::InvalidArguments)
        );
        let value = serde_json::to_value(envelope).expect("serializable envelope");
        assert_eq!(
            value,
            json!({
                "version": 1,
                "ok": false,
                "error": {
                    "code": "invalid_arguments",
                    "message": "invalid command",
                    "recovery": "denju --help"
                }
            })
        );
    }
}
