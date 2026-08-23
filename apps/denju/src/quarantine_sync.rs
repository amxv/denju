use denju_local::{
    ManagedDesiredKind, ManagedSkillRecord, OwnedSkillRecord, SubscriptionRecord,
    journaled_remove_managed_skill, preserve_quarantined_managed_skill,
};
use denju_wire::{CliErrorCode, QuarantinedResource, SyncKnownResource, SyncReconcileRequest};

use crate::{
    public::{InstalledContext, local_error},
    setup::RuntimeError,
};

pub(crate) async fn enforce_before_upload(
    context: &InstalledContext,
) -> Result<Vec<RuntimeError>, RuntimeError> {
    let mut blockers = Vec::new();
    let existing_subscriptions = context.db.subscriptions().await.map_err(local_error)?;
    let known = existing_subscriptions
        .iter()
        .map(|record| {
            Ok(SyncKnownResource {
                resource_id: record.resource_id.clone(),
                generation: u64::try_from(record.resource_generation).map_err(|_| {
                    RuntimeError::new(
                        CliErrorCode::LocalState,
                        "stored subscription generation is invalid",
                    )
                    .recovery("denju doctor")
                })?,
                revision_id: record.materialized_revision_id.clone().unwrap_or_default(),
            })
        })
        .collect::<Result<Vec<_>, RuntimeError>>()?;
    let reconcile = context
        .client
        .reconcile_subscriptions(&SyncReconcileRequest { known })
        .await
        .map_err(crate::public::client_error)?;
    blockers.extend(
        quarantine_subscriptions(context, &existing_subscriptions, &reconcile.quarantined).await?,
    );

    let has_session = context
        .db
        .identity()
        .await
        .map_err(local_error)?
        .is_some_and(|identity| identity.session_backend.is_some());
    if has_session {
        let catalog = context
            .client
            .private_skills()
            .await
            .map_err(crate::public::client_error)?;
        let existing_owned = context.db.owned_skills().await.map_err(local_error)?;
        blockers.extend(quarantine_owned(context, &existing_owned, &catalog.quarantined).await?);
    }

    Ok(blockers)
}

async fn quarantine_subscriptions(
    context: &InstalledContext,
    existing: &[SubscriptionRecord],
    quarantines: &[QuarantinedResource],
) -> Result<Vec<RuntimeError>, RuntimeError> {
    let mut blockers = Vec::new();
    for quarantine in quarantines {
        let Some(record) = existing
            .iter()
            .find(|record| record.resource_id == quarantine.resource_id)
        else {
            continue;
        };
        if !quarantine_targets_subscription(record, quarantine) {
            continue;
        }
        let managed = managed_subscription(record);
        if quarantine_materialized_subscription_revision(record, quarantine).is_some() {
            preserve_quarantined_managed_skill(&context.paths, &managed).map_err(local_error)?;
        }
        if canonical_targets_revision(
            context,
            &record.resource_id,
            &record.owner,
            &record.skill_name,
            record.materialized_revision_id.as_deref(),
        ) {
            journaled_remove_managed_skill(
                &context.paths,
                &context.db,
                &context.roots,
                &managed,
                ManagedDesiredKind::Subscription,
            )
            .await
            .map_err(local_error)?;
        } else {
            context
                .db
                .remove_subscription_record(record.resource_id.clone())
                .await
                .map_err(local_error)?;
        }
        blockers.push(quarantine_blocker(quarantine));
    }
    Ok(blockers)
}

async fn quarantine_owned(
    context: &InstalledContext,
    existing: &[OwnedSkillRecord],
    quarantines: &[QuarantinedResource],
) -> Result<Vec<RuntimeError>, RuntimeError> {
    let mut blockers = Vec::new();
    for quarantine in quarantines {
        if quarantine.release_version.is_some() {
            // Exact-release quarantine applies to immutable release consumers. An owner's live
            // private workspace remains independently addressable unless the whole resource is
            // quarantined.
            continue;
        }
        let Some(record) = existing
            .iter()
            .find(|record| record.resource_id == quarantine.resource_id)
        else {
            continue;
        };
        let managed = ManagedSkillRecord {
            resource_id: record.resource_id.clone(),
            locator: record.locator.clone(),
            owner: record.owner.clone(),
            skill_name: record.skill_name.clone(),
            harness_name: record.harness_name.clone(),
            materialized_revision_id: record.materialized_revision_id.clone(),
        };
        if record.materialized_revision_id.is_some() {
            preserve_quarantined_managed_skill(&context.paths, &managed).map_err(local_error)?;
        }
        if canonical_targets_revision(
            context,
            &record.resource_id,
            &record.owner,
            &record.skill_name,
            record.materialized_revision_id.as_deref(),
        ) {
            journaled_remove_managed_skill(
                &context.paths,
                &context.db,
                &context.roots,
                &managed,
                ManagedDesiredKind::Owned,
            )
            .await
            .map_err(local_error)?;
        } else {
            context
                .db
                .remove_owned_skill(record.resource_id.clone())
                .await
                .map_err(local_error)?;
        }
        blockers.push(quarantine_blocker(quarantine));
    }
    Ok(blockers)
}

fn managed_subscription(record: &SubscriptionRecord) -> ManagedSkillRecord {
    ManagedSkillRecord {
        resource_id: record.resource_id.clone(),
        locator: record.locator.clone(),
        owner: record.owner.clone(),
        skill_name: record.skill_name.clone(),
        harness_name: record.harness_name.clone(),
        materialized_revision_id: record.materialized_revision_id.clone(),
    }
}

pub(crate) fn quarantine_targets_subscription(
    record: &SubscriptionRecord,
    quarantine: &QuarantinedResource,
) -> bool {
    match quarantine.release_version {
        None => true,
        Some(version) => {
            !record.live_private && u64::try_from(record.release_version).ok() == Some(version)
        }
    }
}

pub(crate) fn quarantine_materialized_subscription_revision<'a>(
    record: &'a SubscriptionRecord,
    quarantine: &QuarantinedResource,
) -> Option<&'a str> {
    let materialized = record.materialized_revision_id.as_deref()?;
    if quarantine.release_version.is_none() {
        return Some(materialized);
    }
    if !quarantine_targets_subscription(record, quarantine) {
        return None;
    }
    match quarantine.revision_id.as_deref() {
        Some(revision) if revision == materialized => Some(materialized),
        Some(_) => None,
        None if record.desired_revision_id == materialized => Some(materialized),
        None => None,
    }
}

fn canonical_targets_revision(
    context: &InstalledContext,
    resource_id: &str,
    owner: &str,
    skill_name: &str,
    revision_id: Option<&str>,
) -> bool {
    let Some(revision_id) = revision_id else {
        return false;
    };
    let canonical = context.paths.skills.join(owner).join(skill_name);
    let expected = context
        .paths
        .generations
        .join(resource_id)
        .join(revision_id);
    match (
        std::fs::canonicalize(canonical),
        std::fs::canonicalize(expected),
    ) {
        (Ok(canonical), Ok(expected)) => canonical == expected,
        _ => false,
    }
}

fn quarantine_blocker(quarantine: &QuarantinedResource) -> RuntimeError {
    RuntimeError::new(
        CliErrorCode::ContentVerification,
        format!(
            "{} is quarantined: {}",
            quarantine.locator, quarantine.reason
        ),
    )
    .recovery("inspect ~/.denju/quarantine or contact the registry operator")
}

#[cfg(test)]
mod tests {
    use super::{quarantine_materialized_subscription_revision, quarantine_targets_subscription};
    use denju_local::SubscriptionRecord;
    use denju_wire::QuarantinedResource;

    fn subscription() -> SubscriptionRecord {
        SubscriptionRecord {
            resource_id: "018f3b3c-1000-7000-8000-000000000001".to_owned(),
            locator: "@alice/demo".to_owned(),
            owner: "alice".to_owned(),
            skill_name: "demo".to_owned(),
            resource_generation: 7,
            release_version: 3,
            desired_revision_id: "desired".to_owned(),
            harness_name: Some("demo".to_owned()),
            materialized_revision_id: Some("desired".to_owned()),
            retain_on_delete: false,
            retained_after_delete: false,
            live_private: false,
            desired_root_tree_id: "tree".to_owned(),
        }
    }

    fn quarantine(release_version: Option<u64>) -> QuarantinedResource {
        QuarantinedResource {
            resource_id: "018f3b3c-1000-7000-8000-000000000001".to_owned(),
            locator: "@alice/demo".to_owned(),
            release_version,
            revision_id: None,
            reason: "security".to_owned(),
        }
    }

    #[test]
    fn exact_quarantine_only_targets_the_desired_release() {
        let record = subscription();
        assert!(quarantine_targets_subscription(
            &record,
            &quarantine(Some(3))
        ));
        assert!(!quarantine_targets_subscription(
            &record,
            &quarantine(Some(2))
        ));
        assert!(quarantine_targets_subscription(&record, &quarantine(None)));
    }

    #[test]
    fn hidden_quarantine_metadata_preserves_only_bytes_that_are_known_to_match() {
        let record = subscription();
        assert_eq!(
            quarantine_materialized_subscription_revision(&record, &quarantine(Some(3))),
            Some("desired")
        );

        let mut lagging = record.clone();
        lagging.materialized_revision_id = Some("older-safe-revision".to_owned());
        assert_eq!(
            quarantine_materialized_subscription_revision(&lagging, &quarantine(Some(3))),
            None
        );
        assert_eq!(
            quarantine_materialized_subscription_revision(&lagging, &quarantine(None)),
            Some("older-safe-revision")
        );
    }
}
