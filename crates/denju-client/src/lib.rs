//! Registry HTTP, SSE, authentication, and object-transfer client boundary.

use std::str::FromStr;

use denju_core::BlobId;
use denju_wire::{
    AccountDeleteRequest, AccountDeleteResponse, ApiError, AutomationTokenCreateRequest,
    AutomationTokenCreateResponse, AutomationTokenList, AutomationTokenRevokeRequest,
    AutomationTokenRevokeResponse, ClaimIdentityRequest, CreateInstallationRequest,
    CreateInstallationResponse, DeleteSkillResponse, DeprecateSkillRequest, DeprecateSkillResponse,
    DeviceList, DeviceRevokeRequest, DeviceRevokeResponse, HistoryPruneResponse,
    IdentityBackupRequest, IdentityInfo, IdentitySessionResponse, LoginRequest,
    PrivateRevisionCommitRequest, PrivateRevisionCommitResponse, PrivateRevisionPrepareResponse,
    PrivateRevisionRequest, PrivateSkillCatalog, PrivateSkillImportCommitRequest,
    PrivateSkillImportPrepareResponse, PrivateSkillImportRequest, PrivateSkillImportResponse,
    PublicSkillDetail, PublicSkillSearchResponse, PublishSkillRequest, PublishSkillResponse,
    RecoveryResetRequest, RegistryCapabilities, RenameSkillRequest, RenameSkillResponse,
    ResourceLifecycleRequest, RestoreSkillRequest, RestoreSkillResponse, ShareMutationKind,
    ShareSkillRequest, ShareSkillResponse, SkillHistoryResponse, SkillRevisionDetail,
    SnapshotDownload, StagedBlobUpload, SubscriptionCatalog, SubscriptionMutationRequest,
    SubscriptionMutationResponse, SubscriptionTarget, SyncHint, SyncReconcileRequest,
    SyncReconcileResponse, UnpublishSkillResponse, UsageResponse,
};
use futures_util::StreamExt;
use reqwest::{Client, RequestBuilder, StatusCode};
use thiserror::Error;
use url::Url;

#[derive(Clone)]
pub struct RegistryClient {
    http: Client,
    origin: Url,
    bearer: Option<String>,
}

impl RegistryClient {
    pub fn new(origin: Url) -> Result<Self, ClientError> {
        if !matches!(origin.scheme(), "http" | "https") || origin.cannot_be_a_base() {
            return Err(ClientError::InvalidRegistryOrigin(origin.to_string()));
        }
        Ok(Self {
            http: Client::builder().build()?,
            origin,
            bearer: None,
        })
    }

    pub fn authenticated(origin: Url, bearer: String) -> Result<Self, ClientError> {
        let mut client = Self::new(origin)?;
        if bearer.is_empty() {
            return Err(ClientError::AuthenticationRequired);
        }
        client.bearer = Some(bearer);
        Ok(client)
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

    pub async fn claim_identity(
        &self,
        request: &ClaimIdentityRequest,
    ) -> Result<IdentitySessionResponse, ClientError> {
        self.authenticated_post_json("v1/identity/claim", request)
            .await
    }

    pub async fn login(
        &self,
        request: &LoginRequest,
    ) -> Result<IdentitySessionResponse, ClientError> {
        self.authenticated_post_json("v1/identity/login", request)
            .await
    }

    pub async fn recovery_reset(
        &self,
        request: &RecoveryResetRequest,
    ) -> Result<IdentitySessionResponse, ClientError> {
        self.authenticated_post_json("v1/identity/recover", request)
            .await
    }

    pub async fn identity_backup(
        &self,
        request: &IdentityBackupRequest,
    ) -> Result<(), ClientError> {
        let builder = self
            .http
            .post(self.endpoint("v1/identity/backup")?)
            .json(request);
        let response = self.with_auth(builder)?.send().await?;
        decode_empty_response(response).await
    }

    pub async fn identity(&self) -> Result<IdentityInfo, ClientError> {
        self.authenticated_get_json("v1/identity").await
    }

    pub async fn devices(&self) -> Result<DeviceList, ClientError> {
        self.authenticated_get_json("v1/devices").await
    }

    pub async fn revoke_device(
        &self,
        request: &DeviceRevokeRequest,
    ) -> Result<DeviceRevokeResponse, ClientError> {
        self.authenticated_post_json("v1/devices/revoke", request)
            .await
    }

    pub async fn create_automation_token(
        &self,
        request: &AutomationTokenCreateRequest,
    ) -> Result<AutomationTokenCreateResponse, ClientError> {
        self.authenticated_post_json("v1/tokens", request).await
    }

    pub async fn automation_tokens(&self) -> Result<AutomationTokenList, ClientError> {
        self.authenticated_get_json("v1/tokens").await
    }

    pub async fn revoke_automation_token(
        &self,
        request: &AutomationTokenRevokeRequest,
    ) -> Result<AutomationTokenRevokeResponse, ClientError> {
        self.authenticated_post_json("v1/tokens/revoke", request)
            .await
    }

    pub async fn delete_account(
        &self,
        request: &AccountDeleteRequest,
    ) -> Result<AccountDeleteResponse, ClientError> {
        self.authenticated_post_json("v1/account/delete", request)
            .await
    }

    pub async fn search_public_skills(
        &self,
        query: &str,
        limit: u32,
        cursor: Option<&str>,
    ) -> Result<PublicSkillSearchResponse, ClientError> {
        let mut request = self
            .http
            .get(self.endpoint("v1/search")?)
            .query(&[("q", query), ("limit", &limit.to_string())]);
        if let Some(cursor) = cursor {
            request = request.query(&[("cursor", cursor)]);
        }
        let request = if self.bearer.is_some() {
            self.with_auth(request)?
        } else {
            request
        };
        decode_response(request.send().await?).await
    }

    pub async fn show_public_skill(&self, locator: &str) -> Result<PublicSkillDetail, ClientError> {
        let request = self
            .http
            .get(self.endpoint("v1/skills/show")?)
            .query(&[("locator", locator)]);
        let request = if self.bearer.is_some() {
            self.with_auth(request)?
        } else {
            request
        };
        let response = request.send().await?;
        decode_response(response).await
    }

    pub async fn show_released_skill(
        &self,
        locator: &str,
    ) -> Result<PublicSkillDetail, ClientError> {
        let response = self
            .http
            .get(self.endpoint("v1/skills/show")?)
            .query(&[("locator", locator)])
            .send()
            .await?;
        decode_response(response).await
    }

    pub async fn publish_skill(
        &self,
        request: &PublishSkillRequest,
    ) -> Result<PublishSkillResponse, ClientError> {
        self.authenticated_post_json("v1/skills/publish", request)
            .await
    }

    pub async fn skill_history(&self, locator: &str) -> Result<SkillHistoryResponse, ClientError> {
        let builder = self
            .http
            .get(self.endpoint("v1/skills/history")?)
            .query(&[("locator", locator)]);
        let response = if self.bearer.is_some() {
            self.with_auth(builder)?
        } else {
            builder
        }
        .send()
        .await?;
        decode_response(response).await
    }

    pub async fn skill_revision(
        &self,
        locator: &str,
        revision: &str,
    ) -> Result<SkillRevisionDetail, ClientError> {
        let builder = self
            .http
            .get(self.endpoint("v1/skills/revision")?)
            .query(&[("locator", locator), ("revision", revision)]);
        let response = if self.bearer.is_some() {
            self.with_auth(builder)?
        } else {
            builder
        }
        .send()
        .await?;
        decode_response(response).await
    }

    pub async fn restore_skill(
        &self,
        request: &RestoreSkillRequest,
    ) -> Result<RestoreSkillResponse, ClientError> {
        self.authenticated_post_json("v1/skills/restore", request)
            .await
    }

    pub async fn rename_skill(
        &self,
        request: &RenameSkillRequest,
    ) -> Result<RenameSkillResponse, ClientError> {
        self.authenticated_post_json("v1/skills/rename", request)
            .await
    }

    pub async fn unpublish_skill(
        &self,
        request: &ResourceLifecycleRequest,
    ) -> Result<UnpublishSkillResponse, ClientError> {
        self.authenticated_post_json("v1/skills/unpublish", request)
            .await
    }

    pub async fn delete_skill(
        &self,
        request: &ResourceLifecycleRequest,
    ) -> Result<DeleteSkillResponse, ClientError> {
        self.authenticated_post_json("v1/skills/delete", request)
            .await
    }

    pub async fn deprecate_skill(
        &self,
        request: &DeprecateSkillRequest,
    ) -> Result<DeprecateSkillResponse, ClientError> {
        self.authenticated_post_json("v1/skills/deprecate", request)
            .await
    }

    pub async fn usage(&self) -> Result<UsageResponse, ClientError> {
        self.authenticated_get_json("v1/usage").await
    }

    pub async fn prune_skill_history(
        &self,
        request: &ResourceLifecycleRequest,
    ) -> Result<HistoryPruneResponse, ClientError> {
        self.authenticated_post_json("v1/skills/history/prune", request)
            .await
    }

    pub async fn subscribe(
        &self,
        request: &SubscriptionMutationRequest,
    ) -> Result<SubscriptionMutationResponse, ClientError> {
        self.subscription_mutation("v1/subscriptions", request)
            .await
    }

    pub async fn unsubscribe(
        &self,
        request: &SubscriptionMutationRequest,
    ) -> Result<SubscriptionMutationResponse, ClientError> {
        self.subscription_mutation("v1/subscriptions/remove", request)
            .await
    }

    pub async fn subscriptions(&self) -> Result<SubscriptionCatalog, ClientError> {
        let response = self
            .with_auth(self.http.get(self.endpoint("v1/subscriptions")?))?
            .send()
            .await?;
        decode_response(response).await
    }

    pub async fn subscription_target(
        &self,
        locator: &str,
    ) -> Result<SubscriptionTarget, ClientError> {
        let response = self
            .with_auth(
                self.http
                    .get(self.endpoint("v1/subscriptions/resolve")?)
                    .query(&[("locator", locator)]),
            )?
            .send()
            .await?;
        decode_response(response).await
    }

    pub async fn mutate_private_share(
        &self,
        kind: ShareMutationKind,
        request: &ShareSkillRequest,
    ) -> Result<ShareSkillResponse, ClientError> {
        let path = match kind {
            ShareMutationKind::Share => "v1/shares",
            ShareMutationKind::Unshare => "v1/shares/remove",
        };
        self.authenticated_post_json(path, request).await
    }

    pub async fn reconcile_subscriptions(
        &self,
        request: &SyncReconcileRequest,
    ) -> Result<SyncReconcileResponse, ClientError> {
        self.authenticated_post_json("v1/sync/reconcile", request)
            .await
    }

    pub async fn wait_for_sync_hint(&self) -> Result<SyncHint, ClientError> {
        let response = self
            .with_auth(self.http.get(self.endpoint("v1/events")?))?
            .send()
            .await?;
        if !response.status().is_success() {
            return Err(decode_error_response(response).await);
        }

        let mut stream = response.bytes_stream();
        let mut buffer = Vec::new();
        while let Some(chunk) = stream.next().await {
            buffer.extend_from_slice(&chunk?);
            while let Some((end, delimiter_len)) = find_sse_record_end(&buffer) {
                let record = buffer[..end].to_vec();
                buffer.drain(..end + delimiter_len);
                let text = std::str::from_utf8(&record)
                    .map_err(|error| ClientError::InvalidEventStream(error.to_string()))?;
                let data = text
                    .lines()
                    .filter_map(|line| line.strip_prefix("data:"))
                    .map(str::trim_start)
                    .collect::<Vec<_>>()
                    .join("\n");
                if data.is_empty() {
                    continue;
                }
                return serde_json::from_str(&data)
                    .map_err(|error| ClientError::InvalidEventStream(error.to_string()));
            }
        }
        Err(ClientError::Unavailable(
            "registry event stream closed before a sync hint arrived".to_owned(),
        ))
    }

    pub async fn prepare_private_skill_import(
        &self,
        request: &PrivateSkillImportRequest,
    ) -> Result<PrivateSkillImportPrepareResponse, ClientError> {
        self.authenticated_post_json("v1/private-skills/imports/prepare", request)
            .await
    }

    pub async fn commit_private_skill_import(
        &self,
        request: &PrivateSkillImportCommitRequest,
    ) -> Result<PrivateSkillImportResponse, ClientError> {
        self.authenticated_post_json("v1/private-skills/imports/commit", request)
            .await
    }

    pub async fn private_skills(&self) -> Result<PrivateSkillCatalog, ClientError> {
        self.authenticated_get_json("v1/private-skills").await
    }

    pub async fn prepare_private_revision(
        &self,
        request: &PrivateRevisionRequest,
    ) -> Result<PrivateRevisionPrepareResponse, ClientError> {
        self.authenticated_post_json("v1/private-skills/revisions/prepare", request)
            .await
    }

    pub async fn commit_private_revision(
        &self,
        request: &PrivateRevisionCommitRequest,
    ) -> Result<PrivateRevisionCommitResponse, ClientError> {
        self.authenticated_post_json("v1/private-skills/revisions/commit", request)
            .await
    }

    pub async fn upload_staged_blob(
        &self,
        descriptor: &StagedBlobUpload,
        bytes: &[u8],
    ) -> Result<(), ClientError> {
        if u64::try_from(bytes.len()).ok() != Some(descriptor.size_bytes) {
            return Err(ClientError::ContentMismatch(
                "staged upload size does not match registry intent".to_owned(),
            ));
        }
        let expected = BlobId::from_str(&descriptor.blob_id)
            .map_err(|error| ClientError::ContentMismatch(error.to_string()))?;
        if BlobId::hash(bytes) != expected {
            return Err(ClientError::ContentMismatch(
                "staged upload SHA-256 does not match registry intent".to_owned(),
            ));
        }
        let url = Url::parse(&descriptor.url)
            .map_err(|error| ClientError::InvalidDownloadUrl(error.to_string()))?;
        if !matches!(url.scheme(), "http" | "https") {
            return Err(ClientError::InvalidDownloadUrl(url.to_string()));
        }
        let response = self
            .http
            .put(url)
            .header(reqwest::header::CONTENT_LENGTH, descriptor.size_bytes)
            .body(bytes.to_vec())
            .send()
            .await?;
        if response.status().is_success() {
            Ok(())
        } else {
            Err(ClientError::UnexpectedStatus(response.status()))
        }
    }

    pub async fn download_snapshot(
        &self,
        descriptor: &SnapshotDownload,
    ) -> Result<Vec<u8>, ClientError> {
        let url = Url::parse(&descriptor.url)
            .map_err(|error| ClientError::InvalidDownloadUrl(error.to_string()))?;
        if !matches!(url.scheme(), "http" | "https") {
            return Err(ClientError::InvalidDownloadUrl(url.to_string()));
        }
        let response = self.http.get(url).send().await?;
        let status = response.status();
        if !status.is_success() {
            return Err(ClientError::UnexpectedStatus(status));
        }
        let bytes = response.bytes().await?.to_vec();
        if u64::try_from(bytes.len()).ok() != Some(descriptor.size_bytes) {
            return Err(ClientError::ContentMismatch(
                "snapshot size does not match registry metadata".to_owned(),
            ));
        }
        let expected = BlobId::from_str(&descriptor.sha256)
            .map_err(|error| ClientError::ContentMismatch(error.to_string()))?;
        if BlobId::hash(&bytes) != expected {
            return Err(ClientError::ContentMismatch(
                "snapshot SHA-256 does not match registry metadata".to_owned(),
            ));
        }
        Ok(bytes)
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

    async fn authenticated_get_json<T>(&self, path: &str) -> Result<T, ClientError>
    where
        T: serde::de::DeserializeOwned,
    {
        let response = self
            .with_auth(self.http.get(self.endpoint(path)?))?
            .send()
            .await?;
        decode_response(response).await
    }

    async fn authenticated_post_json<T, R>(&self, path: &str, request: &T) -> Result<R, ClientError>
    where
        T: serde::Serialize + ?Sized,
        R: serde::de::DeserializeOwned,
    {
        let builder = self.http.post(self.endpoint(path)?).json(request);
        let response = self.with_auth(builder)?.send().await?;
        decode_response(response).await
    }

    async fn subscription_mutation(
        &self,
        path: &str,
        request: &SubscriptionMutationRequest,
    ) -> Result<SubscriptionMutationResponse, ClientError> {
        let builder = self.http.post(self.endpoint(path)?).json(request);
        let response = self.with_auth(builder)?.send().await?;
        decode_response(response).await
    }

    fn with_auth(&self, builder: RequestBuilder) -> Result<RequestBuilder, ClientError> {
        let bearer = self
            .bearer
            .as_deref()
            .ok_or(ClientError::AuthenticationRequired)?;
        Ok(builder.bearer_auth(bearer))
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

async fn decode_empty_response(response: reqwest::Response) -> Result<(), ClientError> {
    let status = response.status();
    if status.is_success() {
        return Ok(());
    }
    let api_error = response.json::<ApiError>().await.ok();
    if let Some(api_error) = api_error {
        Err(ClientError::Registry(api_error))
    } else if status == StatusCode::SERVICE_UNAVAILABLE {
        Err(ClientError::Unavailable(
            "registry is temporarily unavailable".to_owned(),
        ))
    } else {
        Err(ClientError::UnexpectedStatus(status))
    }
}

async fn decode_error_response(response: reqwest::Response) -> ClientError {
    let status = response.status();
    if let Ok(api_error) = response.json::<ApiError>().await {
        ClientError::Registry(api_error)
    } else if status == StatusCode::SERVICE_UNAVAILABLE {
        ClientError::Unavailable("registry is temporarily unavailable".to_owned())
    } else {
        ClientError::UnexpectedStatus(status)
    }
}

fn find_sse_record_end(bytes: &[u8]) -> Option<(usize, usize)> {
    let crlf = bytes
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .map(|index| (index, 4));
    let lf = bytes
        .windows(2)
        .position(|window| window == b"\n\n")
        .map(|index| (index, 2));
    match (crlf, lf) {
        (Some(left), Some(right)) => Some(if left.0 <= right.0 { left } else { right }),
        (Some(value), None) | (None, Some(value)) => Some(value),
        (None, None) => None,
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
    #[error("installation authentication is required for this registry operation")]
    AuthenticationRequired,
    #[error("invalid snapshot download URL: {0}")]
    InvalidDownloadUrl(String),
    #[error("downloaded content failed verification: {0}")]
    ContentMismatch(String),
    #[error("invalid registry event stream: {0}")]
    InvalidEventStream(String),
}
