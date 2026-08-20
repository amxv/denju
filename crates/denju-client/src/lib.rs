//! Registry HTTP, SSE, authentication, and object-transfer client boundary.

use denju_wire::{
    ApiError, CreateInstallationRequest, CreateInstallationResponse, RegistryCapabilities,
};
use reqwest::{Client, StatusCode};
use thiserror::Error;
use url::Url;

#[derive(Clone)]
pub struct RegistryClient {
    http: Client,
    origin: Url,
}

impl RegistryClient {
    pub fn new(origin: Url) -> Result<Self, ClientError> {
        if !matches!(origin.scheme(), "http" | "https") || origin.cannot_be_a_base() {
            return Err(ClientError::InvalidRegistryOrigin(origin.to_string()));
        }
        Ok(Self {
            http: Client::builder().build()?,
            origin,
        })
    }

    pub fn origin(&self) -> &Url {
        &self.origin
    }

    pub async fn capabilities(&self) -> Result<RegistryCapabilities, ClientError> {
        self.get_json("v1/capabilities").await
    }

    pub async fn create_installation(
        &self,
        request: &CreateInstallationRequest,
    ) -> Result<CreateInstallationResponse, ClientError> {
        let response = self
            .http
            .post(self.endpoint("v1/installations")?)
            .json(request)
            .send()
            .await?;
        decode_response(response).await
    }

    pub async fn ready(&self) -> Result<(), ClientError> {
        let response = self.http.get(self.endpoint("health/ready")?).send().await?;
        if response.status().is_success() {
            Ok(())
        } else {
            Err(ClientError::Unavailable(format!(
                "registry readiness returned {}",
                response.status()
            )))
        }
    }

    async fn get_json<T>(&self, path: &str) -> Result<T, ClientError>
    where
        T: serde::de::DeserializeOwned,
    {
        let response = self.http.get(self.endpoint(path)?).send().await?;
        decode_response(response).await
    }

    fn endpoint(&self, path: &str) -> Result<Url, ClientError> {
        self.origin
            .join(path)
            .map_err(|error| ClientError::InvalidRegistryOrigin(error.to_string()))
    }
}

async fn decode_response<T>(response: reqwest::Response) -> Result<T, ClientError>
where
    T: serde::de::DeserializeOwned,
{
    let status = response.status();
    if status.is_success() {
        return response.json::<T>().await.map_err(ClientError::Http);
    }

    let api_error = response.json::<ApiError>().await.ok();
    if let Some(api_error) = api_error {
        return Err(ClientError::Registry(api_error));
    }
    if status == StatusCode::SERVICE_UNAVAILABLE {
        Err(ClientError::Unavailable(
            "registry is temporarily unavailable".to_owned(),
        ))
    } else {
        Err(ClientError::UnexpectedStatus(status))
    }
}

#[derive(Debug, Error)]
pub enum ClientError {
    #[error("invalid registry origin: {0}")]
    InvalidRegistryOrigin(String),
    #[error("registry request failed: {0}")]
    Http(#[from] reqwest::Error),
    #[error("registry error {code:?}: {message}", code = .0.code, message = .0.message)]
    Registry(ApiError),
    #[error("registry unavailable: {0}")]
    Unavailable(String),
    #[error("registry returned unexpected HTTP status {0}")]
    UnexpectedStatus(StatusCode),
}
