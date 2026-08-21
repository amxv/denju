//! Versioned Denju wire and structured-output contracts.

mod api;
mod cli;
mod identity;
mod ingest;
mod lifecycle;
mod mutation;
mod proposals;
mod public;
mod release;
mod sharing;
mod workspace;

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
    ForkImportIntent, PrivateSkill, PrivateSkillCatalog, PrivateSkillImportCommitRequest,
    PrivateSkillImportPrepareResponse, PrivateSkillImportRequest, PrivateSkillImportResponse,
    SkillForkProvenance, StagedBlobUpload,
};
pub use lifecycle::{
    DeleteSkillResponse, DeprecateSkillRequest, DeprecateSkillResponse, HistoryPruneResponse,
    RenameSkillRequest, RenameSkillResponse, ResourceLifecycleRequest, UnpublishSkillResponse,
    UsageResponse,
};
pub use mutation::{
    IdentityMutationDomain, PrivateRevisionRequestHashInput, PrivateSkillImportRequestHashInput,
    RequestHash, RequestHashError, SubscriptionMutationKind, create_installation_request_hash,
    delete_skill_request_hash, deprecate_skill_request_hash, history_prune_request_hash,
    identity_mutation_request_hash, private_revision_request_hash,
    private_skill_import_request_hash, proposal_accept_request_hash, proposal_close_request_hash,
    proposal_create_request_hash, publish_skill_request_hash, rename_skill_request_hash,
    restore_skill_request_hash, share_skill_request_hash, subscription_request_hash,
    unpublish_skill_request_hash,
};
pub use proposals::{
    ProposalAcceptRequest, ProposalCloseKind, ProposalCloseRequest, ProposalCreateRequest,
    SkillProposal, SkillProposalDetail, SkillProposalList, SkillProposalState,
};
pub use public::{
    PublicSkill, PublicSkillDetail, PublicSkillManifest, PublicSkillManifestEntry,
    PublicSkillSearchResponse, SkillDeprecation, SnapshotDownload, SubscribedSkill,
    SubscriptionCatalog, SubscriptionContent, SubscriptionMutationRequest,
    SubscriptionMutationResponse, SubscriptionTarget,
};
pub use release::{
    DirtyResource, PublishSkillRequest, PublishSkillResponse, RestoreSkillRequest,
    RestoreSkillResponse, SkillHistoryResponse, SkillRelease, SkillRevisionDetail,
    SkillRevisionSummary, SyncHint, SyncKnownResource, SyncReconcileRequest, SyncReconcileResponse,
};
pub use sharing::{ShareMutationKind, ShareSkillRequest, ShareSkillResponse};
pub use workspace::{
    ForkSyncIntent, PrivateRevisionCommitRequest, PrivateRevisionCommitResponse,
    PrivateRevisionOperationState, PrivateRevisionPrepareResponse, PrivateRevisionRequest,
    PrivateRevisionResponse, PrivateWorkspaceConflict,
};
