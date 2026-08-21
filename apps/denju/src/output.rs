use denju_wire::{
    AutomationTokenList, DeviceList, PublicSkillDetail, PublicSkillSearchResponse,
    SkillDeprecation, SkillHistoryResponse, SkillProposal, SkillProposalDetail, SkillProposalList,
    SkillProposalState,
};

use crate::{
    identity::{
        AutomationTokenOutcome, BackupOutcome, ClaimOutcome, LoginOutcome, RecoveryOutcome,
    },
    lifecycle::UsageOutcome,
    public::SyncOutcome,
    release::DiffOutcome,
    setup::{DoctorOutcome, SetupOutcome},
    workspace::StatusOutcome,
};

pub(crate) fn search_text(outcome: &PublicSkillSearchResponse) -> String {
    if outcome.items.is_empty() {
        return "No public skills found.".to_owned();
    }
    outcome
        .items
        .iter()
        .map(|skill| {
            let line = format!("{}  {}", skill.locator, skill.description);
            append_deprecation_notice(line, skill.deprecation.as_ref()).replace('\n', "  ")
        })
        .collect::<Vec<_>>()
        .join("\n")
}

pub(crate) fn show_text(outcome: &PublicSkillDetail) -> String {
    let content = match outcome.skill.version {
        Some(version) => format!("Release: v{version} ({})", outcome.skill.revision_id),
        None if outcome.skill.live_private => {
            format!("Private workspace: {}", outcome.skill.revision_id)
        }
        None => format!("Revision: {}", outcome.skill.revision_id),
    };
    let mut text = append_deprecation_notice(
        format!(
            "{}\n{}\n{}",
            outcome.skill.locator, outcome.skill.description, content
        ),
        outcome.skill.deprecation.as_ref(),
    );
    if let Some(fork) = outcome.fork.as_ref() {
        text.push_str(&format!(
            "\nForked from: {} ({})",
            fork.upstream_locator, fork.created_from_revision_id
        ));
    }
    text
}

pub(crate) fn proposal_text(outcome: &SkillProposal) -> String {
    let state = proposal_state(outcome.state);
    let mut lines = vec![
        format!(
            "{}  {} -> {}  {state}",
            outcome.proposal_id, outcome.source_locator, outcome.target_locator
        ),
        format!("Revision: {}", outcome.proposed_revision_id),
    ];
    if matches!(outcome.state, SkillProposalState::NeedsSync) {
        lines.push(format!("Next: denju fork sync {}", outcome.source_locator));
    }
    if let Some(message) = outcome.message.as_deref() {
        lines.push(format!("Message: {message}"));
    }
    lines.join("\n")
}

pub(crate) fn proposals_text(outcome: &SkillProposalList) -> String {
    if outcome.proposals.is_empty() {
        return "No proposals.".to_owned();
    }
    outcome
        .proposals
        .iter()
        .map(|proposal| {
            format!(
                "{}  {} -> {}  {}",
                proposal.proposal_id,
                proposal.source_locator,
                proposal.target_locator,
                proposal_state(proposal.state)
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

pub(crate) fn proposal_detail_text(outcome: &SkillProposalDetail) -> String {
    proposal_text(&outcome.proposal)
}

fn proposal_state(state: SkillProposalState) -> &'static str {
    match state {
        SkillProposalState::Open => "open",
        SkillProposalState::NeedsSync => "needs_sync",
        SkillProposalState::Accepted => "accepted",
        SkillProposalState::Rejected => "rejected",
        SkillProposalState::Withdrawn => "withdrawn",
    }
}

pub(crate) fn append_deprecation_notice(
    mut text: String,
    deprecation: Option<&SkillDeprecation>,
) -> String {
    let Some(deprecation) = deprecation else {
        return text;
    };
    text.push_str("\nDeprecated");
    if let Some(replacement) = deprecation.replacement_locator.as_deref() {
        text.push_str(&format!("; use {replacement}"));
    }
    text
}

pub(crate) fn history_text(outcome: &SkillHistoryResponse) -> String {
    if outcome.revisions.is_empty() {
        return format!("{} has no visible revisions.", outcome.locator);
    }
    outcome
        .revisions
        .iter()
        .map(|revision| {
            let releases = if revision.released_versions.is_empty() {
                "private".to_owned()
            } else {
                revision
                    .released_versions
                    .iter()
                    .map(|version| format!("v{version}"))
                    .collect::<Vec<_>>()
                    .join(",")
            };
            format!("{}  {releases}", short_revision(&revision.revision_id))
        })
        .collect::<Vec<_>>()
        .join("\n")
}

pub(crate) fn usage_text(outcome: &UsageOutcome) -> String {
    format!(
        "Storage: {} / {} bytes ({} available)\nQueued locally: {} bytes\nPrunable: {} revisions / {} bytes",
        outcome.registry.storage_used_bytes,
        outcome.registry.storage_limit_bytes,
        outcome.registry.storage_available_bytes,
        outcome.queued_local_bytes,
        outcome.registry.prunable_private_revisions,
        outcome.registry.prunable_bytes,
    )
}

pub(crate) fn diff_text(outcome: &DiffOutcome) -> String {
    if outcome.changes.is_empty() {
        return format!(
            "No changes between {} and {}.",
            short_revision(&outcome.from_revision),
            short_revision(&outcome.to_revision)
        );
    }
    outcome
        .changes
        .iter()
        .map(|change| format!("{}  {}", change.change, change.path))
        .collect::<Vec<_>>()
        .join("\n")
}

pub(crate) fn short_revision(revision: &str) -> &str {
    revision.get(..12).unwrap_or(revision)
}

pub(crate) fn sync_text(outcome: &SyncOutcome) -> String {
    format!(
        "Synced {} skills ({} materialized, {} removed).",
        outcome.desired, outcome.materialized, outcome.removed
    )
}

pub(crate) fn setup_text(outcome: &SetupOutcome) -> String {
    let service = if outcome.service_running {
        format!("{} (running)", outcome.service_kind)
    } else if let Some(detail) = &outcome.service_detail {
        format!("{} ({detail})", outcome.service_kind)
    } else {
        format!("{} (not running)", outcome.service_kind)
    };
    let mut lines = vec![
        "Denju setup complete.".to_owned(),
        format!("Registry: {}", outcome.registry),
        format!("Codex: {}", outcome.codex_root),
        format!("Claude: {}", outcome.claude_root),
        format!("Service: {service}"),
    ];
    if let Some(path) = outcome.unmanaged_skills.first() {
        lines.push(format!("Next: denju import {}", quote_command_arg(path)));
    }
    lines.join("\n")
}

pub(crate) fn status_text(outcome: &StatusOutcome) -> String {
    if outcome.resources.is_empty() && outcome.forks.is_empty() {
        return "Denju is healthy.".to_owned();
    }
    let mut lines = Vec::new();
    for resource in &outcome.resources {
        lines.push(format!("{}: {}", resource.locator, resource.state.as_str()));
        if let Some(message) = &resource.message {
            lines.push(format!("  {message}"));
        }
        if let Some(conflict) = &resource.conflict {
            if !conflict.conflict_paths.is_empty() {
                lines.push(format!(
                    "  Conflicts: {}",
                    conflict.conflict_paths.join(", ")
                ));
            }
            lines.push(format!(
                "  Heads: {} {}",
                conflict.head_revision_ids[0], conflict.head_revision_ids[1]
            ));
        }
        for command in &resource.next_commands {
            lines.push(format!("  Next: {command}"));
        }
    }
    for fork in &outcome.forks {
        lines.push(format!("{}: {}", fork.locator, fork.state));
        lines.push(format!("  {}", fork.message));
        for command in &fork.next_commands {
            lines.push(format!("  Next: {command}"));
        }
    }
    lines.join("\n")
}

pub(crate) fn claim_text(outcome: &ClaimOutcome) -> String {
    format!(
        "Claimed {}.\nRecovery secret: {}\nStore this secret now; Denju cannot show it again.",
        outcome.identity.username, outcome.recovery_secret
    )
}

pub(crate) fn login_text(outcome: &LoginOutcome) -> String {
    format!("Logged in as {}.", outcome.identity.username)
}

pub(crate) fn backup_text(outcome: &BackupOutcome) -> String {
    format!(
        "Recovery secret replaced.\nRecovery secret: {}\nStore this secret now; the previous secret no longer works.",
        outcome.recovery_secret
    )
}

pub(crate) fn recovery_text(outcome: &RecoveryOutcome) -> String {
    format!(
        "Recovered {}.\nRecovery secret: {}\nStore this replacement secret now.",
        outcome.identity.username, outcome.recovery_secret
    )
}

pub(crate) fn devices_text(outcome: &DeviceList) -> String {
    if outcome.devices.is_empty() {
        return "No active devices.".to_owned();
    }
    outcome
        .devices
        .iter()
        .map(|device| {
            format!(
                "{}{}  {}",
                device.session_id,
                if device.current { " *" } else { "" },
                device.device_name
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

pub(crate) fn tokens_text(outcome: &AutomationTokenList) -> String {
    if outcome.tokens.is_empty() {
        return "No active automation tokens.".to_owned();
    }
    outcome
        .tokens
        .iter()
        .map(|token| format!("{}  {}", token.token_id, token.scopes.join(",")))
        .collect::<Vec<_>>()
        .join("\n")
}

pub(crate) fn token_text(outcome: &AutomationTokenOutcome) -> String {
    format!(
        "Created token {}.\nToken secret: {}\nStore this secret now; Denju cannot show it again.",
        outcome.token.token_id, outcome.secret
    )
}

pub(crate) fn doctor_text(outcome: &DoctorOutcome) -> String {
    let mut lines = if outcome.healthy {
        vec!["Denju is healthy.".to_owned()]
    } else {
        vec!["Denju still needs attention.".to_owned()]
    };
    lines.extend(
        outcome
            .repaired
            .iter()
            .map(|item| format!("Repaired: {item}")),
    );
    lines.extend(outcome.issues.iter().map(|item| format!("Issue: {item}")));
    lines.push(format!("Registry: {}", outcome.registry));
    lines.join("\n")
}

fn quote_command_arg(value: &str) -> String {
    format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
}
