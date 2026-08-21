use denju_wire::{
    PublicSkillDetail, PublicSkillSearchResponse, SkillDeprecation, SkillHistoryResponse,
};

use crate::{lifecycle::UsageOutcome, public::SyncOutcome, release::DiffOutcome};

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
    append_deprecation_notice(
        format!(
            "{}\n{}\nRelease: v{} ({})",
            outcome.skill.locator,
            outcome.skill.description,
            outcome.skill.version,
            outcome.skill.revision_id
        ),
        outcome.skill.deprecation.as_ref(),
    )
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
