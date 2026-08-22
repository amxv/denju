use denju_core::{OperationId, ResourceKind, ResourceLocator};
use denju_wire::{
    CliErrorCode, PackCreateRequest, PackCreateResponse, PackDetail, PackLifecycleRequest,
    PackLifecycleResponse, PackMemberTarget, PackMutationKind, PackMutationRequest,
    PackMutationResponse, PackPublishRequest, PackRenameRequest, PackSubscriptionMutationKind,
    PackSubscriptionRequest, PackSubscriptionResponse, pack_create_request_hash,
    pack_delete_request_hash, pack_mutation_request_hash, pack_publish_request_hash,
    pack_rename_request_hash, pack_subscription_request_hash, pack_unpublish_request_hash,
};
use uuid::Uuid;

use crate::{
    public::{client_error, installed_context},
    setup::RuntimeError,
};

pub(crate) fn is_pack_locator(locator: &str) -> bool {
    locator
        .parse::<ResourceLocator>()
        .is_ok_and(|locator| locator.kind() == ResourceKind::Pack)
}

pub(crate) async fn create(locator: &str) -> Result<PackCreateResponse, RuntimeError> {
    let parsed = pack_locator(locator)?;
    let context = installed_context(true).await?;
    let operation_id = new_operation()?;
    let operation_id_text = operation_id.to_string();
    let request_hash = pack_create_request_hash(&operation_id_text, parsed.owner(), parsed.name())
        .map_err(hash_error)?;
    context
        .client
        .create_pack(&PackCreateRequest {
            operation_id: operation_id_text,
            owner: parsed.owner().to_owned(),
            name: parsed.name().to_owned(),
            request_hash: request_hash.to_string(),
        })
        .await
        .map_err(client_error)
}

pub(crate) async fn mutate(
    locator: &str,
    skills: &[String],
    kind: PackMutationKind,
) -> Result<PackMutationResponse, RuntimeError> {
    pack_locator(locator)?;
    let context = installed_context(true).await?;
    let pack = context
        .client
        .pack_detail(locator)
        .await
        .map_err(client_error)?;
    let mut members = Vec::with_capacity(skills.len());
    for skill in skills {
        members.push(resolve_member_target(&context.client, skill).await?);
    }
    let operation_id = new_operation()?.to_string();
    let request_hash = pack_mutation_request_hash(
        kind,
        &operation_id,
        &pack.pack.resource_id,
        pack.pack.generation,
        &members,
    )
    .map_err(hash_error)?;
    let outcome = context
        .client
        .mutate_pack(
            kind,
            &PackMutationRequest {
                operation_id,
                resource_id: pack.pack.resource_id,
                expected_generation: pack.pack.generation,
                members,
                request_hash: request_hash.to_string(),
            },
        )
        .await
        .map_err(client_error)?;
    crate::public::sync_once().await?;
    Ok(outcome)
}

pub(crate) async fn show(locator: &str) -> Result<PackDetail, RuntimeError> {
    pack_locator(locator)?;
    let context = installed_context(true).await?;
    context
        .client
        .pack_detail(locator)
        .await
        .map_err(client_error)
}

pub(crate) async fn publish(
    locator: &str,
    public: bool,
) -> Result<PackMutationResponse, RuntimeError> {
    let context = installed_context(true).await?;
    let pack = context
        .client
        .pack_detail(locator)
        .await
        .map_err(client_error)?;
    let operation_id = new_operation()?.to_string();
    let request_hash = pack_publish_request_hash(
        &operation_id,
        &pack.pack.resource_id,
        pack.pack.generation,
        public,
    )
    .map_err(hash_error)?;
    let outcome = context
        .client
        .publish_pack(&PackPublishRequest {
            operation_id,
            resource_id: pack.pack.resource_id,
            expected_generation: pack.pack.generation,
            public,
            request_hash: request_hash.to_string(),
        })
        .await
        .map_err(client_error)?;
    crate::public::sync_once().await?;
    Ok(outcome)
}

pub(crate) async fn rename(
    locator: &str,
    new_name: &str,
) -> Result<PackLifecycleResponse, RuntimeError> {
    let context = installed_context(true).await?;
    let pack = context
        .client
        .pack_detail(locator)
        .await
        .map_err(client_error)?;
    let current = pack_locator(&pack.pack.locator)?;
    let proposed = format!("@{}/packs/{new_name}", current.owner());
    pack_locator(&proposed)?;
    let operation_id = new_operation()?.to_string();
    let request_hash = pack_rename_request_hash(
        &operation_id,
        &pack.pack.resource_id,
        pack.pack.generation,
        new_name,
    )
    .map_err(hash_error)?;
    let outcome = context
        .client
        .rename_pack(&PackRenameRequest {
            operation_id,
            resource_id: pack.pack.resource_id,
            expected_generation: pack.pack.generation,
            new_name: new_name.to_owned(),
            request_hash: request_hash.to_string(),
        })
        .await
        .map_err(client_error)?;
    crate::public::sync_once().await?;
    Ok(outcome)
}

pub(crate) async fn unpublish(locator: &str) -> Result<PackLifecycleResponse, RuntimeError> {
    lifecycle(locator, false).await
}

pub(crate) async fn delete(
    locator: &str,
    json: bool,
    yes: bool,
) -> Result<PackLifecycleResponse, RuntimeError> {
    crate::lifecycle::confirm_destructive(
        json,
        yes,
        &format!("Delete {locator}? [y/N] "),
        &format!("denju delete {locator} --yes"),
    )?;
    lifecycle(locator, true).await
}

async fn lifecycle(locator: &str, delete: bool) -> Result<PackLifecycleResponse, RuntimeError> {
    let context = installed_context(true).await?;
    let pack = context
        .client
        .pack_detail(locator)
        .await
        .map_err(client_error)?;
    let operation_id = new_operation()?.to_string();
    let request_hash = if delete {
        pack_delete_request_hash(&operation_id, &pack.pack.resource_id, pack.pack.generation)
    } else {
        pack_unpublish_request_hash(&operation_id, &pack.pack.resource_id, pack.pack.generation)
    }
    .map_err(hash_error)?;
    let request = PackLifecycleRequest {
        operation_id,
        resource_id: pack.pack.resource_id,
        expected_generation: pack.pack.generation,
        request_hash: request_hash.to_string(),
    };
    let outcome = if delete {
        context
            .client
            .delete_pack(&request)
            .await
            .map_err(client_error)
    } else {
        context
            .client
            .unpublish_pack(&request)
            .await
            .map_err(client_error)
    }?;
    crate::public::sync_once().await?;
    Ok(outcome)
}

pub(crate) async fn subscribe(locator: &str) -> Result<PackSubscriptionResponse, RuntimeError> {
    mutate_subscription(locator, PackSubscriptionMutationKind::Subscribe).await
}

pub(crate) async fn unsubscribe(locator: &str) -> Result<PackSubscriptionResponse, RuntimeError> {
    mutate_subscription(locator, PackSubscriptionMutationKind::Unsubscribe).await
}

async fn mutate_subscription(
    locator: &str,
    kind: PackSubscriptionMutationKind,
) -> Result<PackSubscriptionResponse, RuntimeError> {
    let context = installed_context(true).await?;
    let pack = context
        .client
        .pack_detail(locator)
        .await
        .map_err(client_error)?;
    let operation_id = new_operation()?.to_string();
    let request_hash = pack_subscription_request_hash(
        kind,
        &operation_id,
        &pack.pack.resource_id,
        pack.pack.generation,
    )
    .map_err(hash_error)?;
    let outcome = context
        .client
        .mutate_pack_subscription(
            kind,
            &PackSubscriptionRequest {
                operation_id,
                resource_id: pack.pack.resource_id,
                expected_generation: pack.pack.generation,
                request_hash: request_hash.to_string(),
            },
        )
        .await
        .map_err(client_error)?;
    crate::public::sync_once().await?;
    Ok(outcome)
}

async fn resolve_member_target(
    client: &denju_client::RegistryClient,
    target: &str,
) -> Result<PackMemberTarget, RuntimeError> {
    let (locator, release_version) = split_skill_pin(target)?;
    // An owned private workspace is intentionally not a direct-subscription target, but it
    // is valid input for an owner-only personal pack. Prefer the authenticated private
    // catalog when available, then fall back to the normal public/shared resolver.
    let owned = client.private_skills().await.ok().and_then(|catalog| {
        catalog
            .skills
            .into_iter()
            .find(|skill| skill.locator == locator)
    });
    let resource_id = if let Some(skill) = owned {
        skill.resource_id
    } else {
        client
            .subscription_target(&locator)
            .await
            .map_err(client_error)?
            .resource_id
    };
    Ok(PackMemberTarget {
        resource_id,
        release_version,
    })
}

fn split_skill_pin(value: &str) -> Result<(String, Option<u64>), RuntimeError> {
    let (locator, version) = if let Some((locator, suffix)) = value.rsplit_once("@v") {
        let version = suffix.parse::<u64>().map_err(|_| {
            RuntimeError::new(
                CliErrorCode::InvalidArguments,
                format!("invalid pack member version in {value}"),
            )
        })?;
        if version == 0 {
            return Err(RuntimeError::new(
                CliErrorCode::InvalidArguments,
                "pack member release versions start at v1",
            ));
        }
        (locator.to_owned(), Some(version))
    } else {
        (value.to_owned(), None)
    };
    let parsed = locator
        .parse::<ResourceLocator>()
        .map_err(|error| RuntimeError::new(CliErrorCode::InvalidArguments, error.to_string()))?;
    if parsed.kind() != ResourceKind::Skill {
        return Err(RuntimeError::new(
            CliErrorCode::InvalidArguments,
            "packs may contain skills only",
        ));
    }
    Ok((locator, version))
}

fn pack_locator(value: &str) -> Result<ResourceLocator, RuntimeError> {
    let locator = value
        .parse::<ResourceLocator>()
        .map_err(|error| RuntimeError::new(CliErrorCode::InvalidArguments, error.to_string()))?;
    if locator.kind() != ResourceKind::Pack {
        return Err(RuntimeError::new(
            CliErrorCode::InvalidArguments,
            "expected a pack locator like @owner/packs/name",
        ));
    }
    Ok(locator)
}

fn hash_error(error: denju_wire::RequestHashError) -> RuntimeError {
    RuntimeError::new(CliErrorCode::InvalidArguments, error.to_string())
}

fn new_operation() -> Result<OperationId, RuntimeError> {
    OperationId::from_uuid(Uuid::now_v7())
        .map_err(|error| RuntimeError::new(CliErrorCode::Internal, error.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn member_pin_parser_distinguishes_follow_latest_and_exact_release() {
        assert_eq!(
            split_skill_pin("@alice/review").unwrap(),
            ("@alice/review".to_owned(), None)
        );
        assert_eq!(
            split_skill_pin("@alice/review@v3").unwrap(),
            ("@alice/review".to_owned(), Some(3))
        );
        assert!(split_skill_pin("@alice/review@v0").is_err());
        assert!(split_skill_pin("@alice/packs/core").is_err());
    }
}
