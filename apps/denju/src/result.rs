use std::process::ExitCode;

use denju_wire::{
    AutomationTokenList, AutomationTokenRevokeResponse, DeleteSkillResponse,
    DeprecateSkillResponse, DeviceList, DeviceRevokeResponse, HistoryPruneResponse, IdentityInfo,
    PackCreateResponse, PackDetail, PackLifecycleResponse, PackMutationResponse,
    PackSubscriptionResponse, PublicSkillDetail, PublicSkillSearchResponse, PublishSkillResponse,
    RenameSkillResponse, ResourceTransferResponse, SkillHistoryResponse, SkillProposal,
    SkillProposalDetail, SkillProposalList, TeamDeleteResponse, TeamDetail, TeamLeaveResponse,
    TeamList, TeamMutationResponse, TeamOwnerTransferResponse, TeamPackAssignmentResponse,
    UnpublishSkillResponse,
};
use serde::Serialize;

use crate::{
    fork_ops,
    identity::{
        AutomationTokenOutcome, BackupOutcome, ClaimOutcome, DeleteOutcome, LoginOutcome,
        RecoveryOutcome,
    },
    lifecycle::UsageOutcome,
    owned::ImportOutcome,
    public::{SubscribeOutcome, SyncOutcome, UnsubscribeOutcome},
    release::{DiffOutcome, ExportOutcome, RestoreOutcome},
    setup::{DoctorOutcome, SetupOutcome},
    workspace,
};

#[derive(Debug, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum ResultPayload {
    Guidance {
        state: &'static str,
        #[serde(skip_serializing_if = "Option::is_none")]
        next_command: Option<String>,
    },
    Setup {
        #[serde(flatten)]
        outcome: SetupOutcome,
    },
    Claim {
        #[serde(flatten)]
        outcome: ClaimOutcome,
    },
    Login {
        #[serde(flatten)]
        outcome: LoginOutcome,
    },
    Identity {
        #[serde(flatten)]
        outcome: IdentityInfo,
    },
    IdentityBackup {
        #[serde(flatten)]
        outcome: BackupOutcome,
    },
    IdentityRecovery {
        #[serde(flatten)]
        outcome: RecoveryOutcome,
    },
    IdentityDelete {
        #[serde(flatten)]
        outcome: DeleteOutcome,
    },
    Devices {
        #[serde(flatten)]
        outcome: DeviceList,
    },
    DeviceRevoke {
        #[serde(flatten)]
        outcome: DeviceRevokeResponse,
    },
    TokenCreate {
        #[serde(flatten)]
        outcome: AutomationTokenOutcome,
    },
    Tokens {
        #[serde(flatten)]
        outcome: AutomationTokenList,
    },
    TokenRevoke {
        #[serde(flatten)]
        outcome: AutomationTokenRevokeResponse,
    },
    Doctor {
        #[serde(flatten)]
        outcome: DoctorOutcome,
    },
    Search {
        #[serde(flatten)]
        outcome: PublicSkillSearchResponse,
    },
    Show {
        #[serde(flatten)]
        outcome: PublicSkillDetail,
    },
    PackShow {
        #[serde(flatten)]
        outcome: PackDetail,
    },
    PackCreate {
        #[serde(flatten)]
        outcome: PackCreateResponse,
    },
    PackMutation {
        #[serde(flatten)]
        outcome: PackMutationResponse,
    },
    PackLifecycle {
        #[serde(flatten)]
        outcome: PackLifecycleResponse,
    },
    PackSubscription {
        #[serde(flatten)]
        outcome: PackSubscriptionResponse,
    },
    Teams {
        #[serde(flatten)]
        outcome: TeamList,
    },
    Team {
        #[serde(flatten)]
        outcome: TeamDetail,
    },
    TeamMutation {
        #[serde(flatten)]
        outcome: TeamMutationResponse,
    },
    TeamInvite {
        invite_id: String,
        team: String,
        role: denju_wire::TeamRole,
        expires_at_unix_seconds: i64,
        join_command: String,
    },
    TeamPackAssignment {
        #[serde(flatten)]
        outcome: TeamPackAssignmentResponse,
    },
    TeamLeave {
        #[serde(flatten)]
        outcome: TeamLeaveResponse,
    },
    TeamOwnerTransfer {
        #[serde(flatten)]
        outcome: TeamOwnerTransferResponse,
        #[serde(skip_serializing_if = "Option::is_none")]
        accept_command: Option<String>,
    },
    TeamDelete {
        #[serde(flatten)]
        outcome: TeamDeleteResponse,
    },
    Transfer {
        #[serde(flatten)]
        outcome: ResourceTransferResponse,
    },
    Import {
        #[serde(flatten)]
        outcome: ImportOutcome,
    },
    Publish {
        #[serde(flatten)]
        outcome: PublishSkillResponse,
    },
    Rename {
        #[serde(flatten)]
        outcome: RenameSkillResponse,
    },
    Unpublish {
        #[serde(flatten)]
        outcome: UnpublishSkillResponse,
    },
    Delete {
        #[serde(flatten)]
        outcome: DeleteSkillResponse,
    },
    Deprecate {
        #[serde(flatten)]
        outcome: DeprecateSkillResponse,
    },
    Usage {
        #[serde(flatten)]
        outcome: UsageOutcome,
    },
    History {
        #[serde(flatten)]
        outcome: SkillHistoryResponse,
    },
    HistoryPrune {
        #[serde(flatten)]
        outcome: HistoryPruneResponse,
    },
    Diff {
        #[serde(flatten)]
        outcome: DiffOutcome,
    },
    Restore {
        #[serde(flatten)]
        outcome: RestoreOutcome,
    },
    Export {
        #[serde(flatten)]
        outcome: ExportOutcome,
    },
    Subscribe {
        #[serde(flatten)]
        outcome: SubscribeOutcome,
    },
    Unsubscribe {
        #[serde(flatten)]
        outcome: UnsubscribeOutcome,
    },
    Share {
        #[serde(flatten)]
        outcome: denju_wire::ShareSkillResponse,
    },
    Unshare {
        #[serde(flatten)]
        outcome: denju_wire::ShareSkillResponse,
    },
    Fork {
        #[serde(flatten)]
        outcome: fork_ops::ForkOutcome,
    },
    ForkSync {
        #[serde(flatten)]
        outcome: fork_ops::ForkSyncOutcome,
    },
    ForkResolve {
        #[serde(flatten)]
        outcome: fork_ops::ForkResolveOutcome,
    },
    Proposal {
        #[serde(flatten)]
        outcome: SkillProposal,
    },
    Proposals {
        #[serde(flatten)]
        outcome: SkillProposalList,
    },
    ProposalDetail {
        #[serde(flatten)]
        outcome: SkillProposalDetail,
    },
    Status {
        #[serde(flatten)]
        outcome: workspace::StatusOutcome,
    },
    Sync {
        #[serde(flatten)]
        outcome: SyncOutcome,
    },
    Version {
        version: &'static str,
    },
    Help {
        text: &'static str,
    },
}

pub(crate) struct CommandOutput {
    pub(crate) payload: ResultPayload,
    pub(crate) text: String,
    pub(crate) exit: ExitCode,
}
