//! Versioned Denju wire and structured-output contracts.

mod api;
mod cli;
mod identity;
mod ingest;
mod mutation;
mod public;

pub use api::{
    ApiError, ApiErrorCode, CreateInstallationRequest, CreateInstallationResponse,
    RegistryCapabilities, RegistryLimits,
};
pub use cli::{CLI_ENVELOPE_VERSION, CliEnvelope, CliError, CliErrorCode};
pub use identity::{
    AccountDeleteRequest, AccountDeleteResponse, AutomationTokenCreateRequest,
    AutomationTokenCreateResponse, AutomationTokenInfo, AutomationTokenList,
    AutomationTokenRevokeRequest, AutomationTokenRevokeResponse, ClaimIdentityRequest, DeviceInfo,
    DeviceList, DeviceRevokeRequest, DeviceRevokeResponse, IdentityBackupRequest, IdentityInfo,
    IdentitySessionResponse, LoginRequest, RecoveryResetRequest,
};
pub use ingest::{
    PrivateSkill, PrivateSkillCatalog, PrivateSkillImportCommitRequest,
    PrivateSkillImportPrepareResponse, PrivateSkillImportRequest, PrivateSkillImportResponse,
    StagedBlobUpload,
};
pub use mutation::{
    IdentityMutationDomain, RequestHash, RequestHashError, SubscriptionMutationKind,
    create_installation_request_hash, identity_mutation_request_hash,
    private_skill_import_request_hash, subscription_request_hash,
};
pub use public::{
    PublicSkill, PublicSkillDetail, PublicSkillManifest, PublicSkillManifestEntry,
    PublicSkillSearchResponse, SnapshotDownload, SubscribedSkill, SubscriptionCatalog,
    SubscriptionMutationRequest, SubscriptionMutationResponse,
};
