use std::time::{Duration, Instant};

use denju_registry::Registry;
use denju_wire::{
    PackCreateRequest, PackMemberTarget, PackMutationKind, PackMutationRequest,
    PublishSkillRequest, SyncKnownResource, SyncReconcileRequest, pack_create_request_hash,
    pack_mutation_request_hash, publish_skill_request_hash,
};
use serde::Serialize;
use sha2::{Digest, Sha256};
use uuid::Uuid;

use super::{
    DATABASE, OWNER, SERVER_TWO_PORT, millis, seed::SeededPublicSkill, support::docker_psql,
};

pub(super) const SESSION_BEARER: &str =
    "9d4404e4f3151ae6a34abdb5853d49e07636fc7da4c943235483f85be945bd56";
const INSTALL_BEARER: &str = "c4b584af14b95f0f66616a87d29bd08face114578639fecdbfaf6fc5d884595b";
const SUBSCRIBER_BEARER: &str = "5838ad0d2437f8ac0062c1c30e0c1a6cf2163e905069b23d13ae2ce980e1f6b9";

#[derive(Debug, Serialize)]
pub(super) struct PackFanoutReport {
    pub dependent_packs: usize,
    pub direct_subscribers: usize,
    pub subscription_rows_before_publish: u64,
    pub subscription_rows_after_publish: u64,
    pub publish_ms: u64,
    pub online_update_ms: u64,
    pub request_adjacent_pack_revisions: u64,
    pub recovery_drain_ms: u64,
    pub total_pack_revisions: u64,
    pub semantic_release_events: u64,
}

pub(super) async fn exercise_pack_fanout(
    registry: &Registry,
    root: &std::path::Path,
    skill: &SeededPublicSkill,
    dependent_packs: usize,
) -> Result<PackFanoutReport, String> {
    super::create_installation(registry, INSTALL_BEARER).await?;
    bootstrap_seed_owner(root, skill)?;
    for index in 0..dependent_packs {
        let operation = Uuid::now_v7().to_string();
        let name = format!("load-pack-{index:04}");
        let request_hash = pack_create_request_hash(&operation, OWNER, &name)
            .map_err(|error| error.to_string())?;
        let created = registry
            .create_pack(
                SESSION_BEARER,
                &PackCreateRequest {
                    operation_id: operation,
                    owner: OWNER.to_owned(),
                    name,
                    request_hash: request_hash.to_string(),
                },
            )
            .await
            .map_err(|error| error.message)?;
        let operation = Uuid::now_v7().to_string();
        let members = vec![PackMemberTarget {
            resource_id: skill.resource_id.clone(),
            release_version: None,
        }];
        let request_hash = pack_mutation_request_hash(
            PackMutationKind::Add,
            &operation,
            &created.pack.resource_id,
            created.pack.generation,
            &members,
        )
        .map_err(|error| error.to_string())?;
        registry
            .mutate_pack(
                SESSION_BEARER,
                PackMutationKind::Add,
                &PackMutationRequest {
                    operation_id: operation,
                    resource_id: created.pack.resource_id,
                    expected_generation: created.pack.generation,
                    members,
                    request_hash: request_hash.to_string(),
                },
            )
            .await
            .map_err(|error| error.message)?;
    }

    super::create_installation(registry, SUBSCRIBER_BEARER).await?;
    let mut subscriber_skill = skill.clone();
    subscriber_skill.generation = 2;
    super::subscribe_one(registry, SUBSCRIBER_BEARER, &subscriber_skill).await?;
    const DIRECT_SUBSCRIBERS: usize = 1_000;
    seed_installation_subscribers(root, &skill.resource_id, DIRECT_SUBSCRIBERS)?;
    let subscription_rows_before_publish = query_u64(
        root,
        &format!(
            "SELECT COUNT(*) FROM installation_subscriptions WHERE resource_id='{}'::uuid;",
            skill.resource_id
        ),
    )?;
    let mut sse = reqwest::Client::new()
        .get(format!("http://127.0.0.1:{SERVER_TWO_PORT}/v1/events"))
        .bearer_auth(SUBSCRIBER_BEARER)
        .send()
        .await
        .map_err(|error| format!("open online subscriber SSE: {error}"))?;
    if !sse.status().is_success() {
        return Err(format!("online subscriber SSE returned {}", sse.status()));
    }

    let operation = Uuid::now_v7().to_string();
    let request_hash =
        publish_skill_request_hash(&operation, &skill.resource_id, 2, false, None, &[])
            .map_err(|error| error.to_string())?;
    let publish_started = Instant::now();
    let published = registry
        .publish_skill(
            SESSION_BEARER,
            &PublishSkillRequest {
                operation_id: operation,
                resource_id: skill.resource_id.clone(),
                expected_generation: 2,
                public: false,
                message: None,
                tags: Vec::new(),
                request_hash: request_hash.to_string(),
            },
        )
        .await
        .map_err(|error| error.message)?;
    if published.release.version != 2 {
        return Err(format!(
            "fanout fixture expected skill v2, got v{}",
            published.release.version
        ));
    }
    let publish_ms = millis(publish_started.elapsed());
    let subscription_rows_after_publish = query_u64(
        root,
        &format!(
            "SELECT COUNT(*) FROM installation_subscriptions WHERE resource_id='{}'::uuid;",
            skill.resource_id
        ),
    )?;
    if subscription_rows_after_publish != subscription_rows_before_publish {
        return Err(format!(
            "publish mutated direct subscriber rows: {subscription_rows_before_publish} -> {subscription_rows_after_publish}"
        ));
    }
    let chunk = tokio::time::timeout(Duration::from_secs(2), sse.chunk())
        .await
        .map_err(|_| "online subscriber SSE update exceeded two seconds".to_owned())?
        .map_err(|error| format!("read online subscriber SSE: {error}"))?
        .ok_or_else(|| "online subscriber SSE closed before a publish hint".to_owned())?;
    let hint = String::from_utf8_lossy(&chunk);
    if !hint.contains(&skill.resource_id) && !hint.contains("resync_all") {
        return Err(format!(
            "online SSE hint did not identify the published skill: {hint}"
        ));
    }
    let reconcile = registry
        .reconcile_subscriptions(
            SUBSCRIBER_BEARER,
            &SyncReconcileRequest {
                known: vec![SyncKnownResource {
                    resource_id: skill.resource_id.clone(),
                    generation: subscriber_skill.generation,
                    revision_id: skill.revision_id.clone(),
                }],
            },
        )
        .await
        .map_err(|error| error.message)?;
    if reconcile.skills.len() != 1
        || reconcile.skills[0].revision_id != published.release.revision_id
    {
        return Err(
            "online subscriber reconcile did not reach the published v2 revision".to_owned(),
        );
    }
    let online_update_ms = millis(publish_started.elapsed());
    if online_update_ms >= 2_000 {
        return Err(format!(
            "online publish propagation missed the two-second target: {online_update_ms}ms"
        ));
    }
    let event_id = query_u64(
        root,
        &format!(
            "SELECT id FROM authority_events WHERE event_kind='skill_release_published' AND resource_id='{}'::uuid ORDER BY id DESC LIMIT 1;",
            skill.resource_id
        ),
    )?;
    let request_adjacent_pack_revisions = query_u64(
        root,
        &format!("SELECT COUNT(*) FROM pack_revisions WHERE source_release_event_id={event_id};"),
    )?;
    if request_adjacent_pack_revisions > 16 {
        return Err(format!(
            "request-adjacent pack fanout exceeded its fixed bound: {request_adjacent_pack_revisions}"
        ));
    }

    let recovery_started = Instant::now();
    loop {
        let result = registry
            .drain_pack_release_events(256)
            .await
            .map_err(|error| error.message)?;
        if result.pending_release_event_id.is_none() {
            break;
        }
    }
    let recovery_drain_ms = millis(recovery_started.elapsed());
    let total_pack_revisions = query_u64(
        root,
        &format!("SELECT COUNT(*) FROM pack_revisions WHERE source_release_event_id={event_id};"),
    )?;
    if total_pack_revisions != dependent_packs as u64 {
        return Err(format!(
            "pack fanout advanced {total_pack_revisions} of {dependent_packs} dependent packs"
        ));
    }
    let semantic_release_events = query_u64(
        root,
        &format!(
            "SELECT COUNT(*) FROM authority_events WHERE event_kind='skill_release_published' AND resource_id='{}'::uuid;",
            skill.resource_id
        ),
    )?;
    if semantic_release_events != 1 {
        return Err(format!(
            "fanout publish recorded {semantic_release_events} semantic release events instead of one"
        ));
    }
    Ok(PackFanoutReport {
        dependent_packs,
        direct_subscribers: DIRECT_SUBSCRIBERS,
        subscription_rows_before_publish,
        subscription_rows_after_publish,
        publish_ms,
        online_update_ms,
        request_adjacent_pack_revisions,
        recovery_drain_ms,
        total_pack_revisions,
        semantic_release_events,
    })
}

fn seed_installation_subscribers(
    root: &std::path::Path,
    resource_id: &str,
    subscribers: usize,
) -> Result<(), String> {
    let sql = format!(
        r#"WITH seed AS MATERIALIZED (
            SELECT i,uuidv7() AS author_id,uuidv7() AS installation_id,sha256(convert_to(format('phase17-subscriber-%s',i),'UTF8')) AS credential_hash
            FROM generate_series(1,{subscribers}) AS i
         ), authors AS (
            INSERT INTO author_principals(id,kind) SELECT author_id,'installation' FROM seed
         ), installations_inserted AS (
            INSERT INTO installations(id,author_principal_id,credential_hash)
            SELECT installation_id,author_id,credential_hash FROM seed
         )
         INSERT INTO installation_subscriptions(installation_id,resource_id)
         SELECT installation_id,'{resource_id}'::uuid FROM seed;"#
    );
    docker_psql(root, DATABASE, &sql).map(|_| ())
}

fn bootstrap_seed_owner(root: &std::path::Path, skill: &SeededPublicSkill) -> Result<(), String> {
    let install_hash = hex_secret_hash(INSTALL_BEARER)?;
    let session_hash = hex_secret_hash(SESSION_BEARER)?;
    let recovery_hash = hex_sha256("phase17-fanout-recovery");
    let user_id = Uuid::now_v7();
    let author_id = Uuid::now_v7();
    let session_id = Uuid::now_v7();
    let revision_id = hex_sha256(&format!("phase17-fanout-workspace:{}", skill.resource_id));
    let operation_id = Uuid::now_v7();
    let sql = format!(
        r#"WITH target AS (
            SELECT r.id AS resource_id,r.owner_namespace_id,r.description,r.license,r.compatibility,
                   sr.revision_id AS parent_revision_id,sr.root_tree_id,sr.manifest_json,sr.snapshot_key,sr.snapshot_sha256,sr.snapshot_size
            FROM resources r JOIN skill_releases sr ON sr.resource_id=r.id AND sr.version=1
            WHERE r.id='{resource}'::uuid
         ), installation AS (
            SELECT id,author_principal_id FROM installations WHERE credential_hash=decode('{install_hash}','hex')
         ), new_author AS (
            INSERT INTO author_principals(id,kind) VALUES ('{author}'::uuid,'user') RETURNING id
         ), new_user AS (
            INSERT INTO users(id,namespace_id,author_principal_id,password_hash,recovery_secret_hash)
            SELECT '{user}'::uuid,target.owner_namespace_id,'{author}'::uuid,'phase17-load-password-hash',decode('{recovery_hash}','hex') FROM target RETURNING id
         ), link_install AS (
            UPDATE installations SET user_id='{user}'::uuid WHERE id=(SELECT id FROM installation) RETURNING id
         ), principal_links AS (
            INSERT INTO author_principal_users(author_principal_id,user_id)
            SELECT '{author}'::uuid,'{user}'::uuid UNION ALL SELECT author_principal_id,'{user}'::uuid FROM installation
         ), new_session AS (
            INSERT INTO sessions(id,user_id,installation_id,token_hash,device_name)
            SELECT '{session}'::uuid,'{user}'::uuid,id,decode('{session_hash}','hex'),'phase17-load' FROM installation
         ), new_revision AS (
            INSERT INTO revisions(revision_id,root_tree_id,author_principal_id,operation_id)
            SELECT decode('{revision}','hex'),root_tree_id,'{author}'::uuid,'{operation}'::uuid FROM target RETURNING revision_id
         ), new_parent AS (
            INSERT INTO revision_parents(revision_id,parent_revision_id,ordinal)
            SELECT decode('{revision}','hex'),parent_revision_id,0 FROM target
         ), new_reachability AS (
            INSERT INTO revision_blob_reachability(revision_id,blob_id)
            SELECT decode('{revision}','hex'),rbr.blob_id FROM target JOIN revision_blob_reachability rbr ON rbr.revision_id=target.parent_revision_id
         ), bumped_resource_refs AS (
            UPDATE resource_blob_reachability refs SET reference_count=reference_count+1 FROM target
            WHERE refs.resource_id=target.resource_id RETURNING refs.blob_id
         ), bumped_namespace_refs AS (
            UPDATE namespace_blob_reachability refs SET reference_count=reference_count+1 FROM target
            WHERE refs.namespace_id=target.owner_namespace_id RETURNING refs.blob_id
         ), snapshot AS (
            INSERT INTO resource_revision_snapshots(resource_id,revision_id,manifest_json,snapshot_key,snapshot_sha256,snapshot_size)
            SELECT resource_id,decode('{revision}','hex'),manifest_json,snapshot_key,snapshot_sha256,snapshot_size FROM target
         ), workspace AS (
            INSERT INTO skill_private_workspaces(resource_id,revision_id,generation,manifest_json,snapshot_key,snapshot_sha256,snapshot_size,workspace_user_id,description,license,compatibility)
            SELECT resource_id,decode('{revision}','hex'),2,manifest_json,snapshot_key,snapshot_sha256,snapshot_size,'{user}'::uuid,description,license,compatibility FROM target
         )
         UPDATE resources SET generation=2 WHERE id='{resource}'::uuid;"#,
        resource = skill.resource_id,
        author = author_id,
        user = user_id,
        session = session_id,
        operation = operation_id,
        revision = revision_id,
    );
    let installation_count = query_u64(
        root,
        &format!(
            "SELECT COUNT(*) FROM installations WHERE credential_hash=decode('{install_hash}','hex');"
        ),
    )?;
    if installation_count != 1 {
        return Err("fanout owner installation fixture is missing".to_owned());
    }
    docker_psql(root, DATABASE, &sql).map(|_| ())
}

fn query_u64(root: &std::path::Path, sql: &str) -> Result<u64, String> {
    docker_psql(root, DATABASE, sql)?
        .lines()
        .find_map(|line| line.trim().parse::<u64>().ok())
        .ok_or_else(|| "could not parse benchmark database count".to_owned())
}

fn hex_sha256(value: &str) -> String {
    format!("{:x}", Sha256::digest(value.as_bytes()))
}

fn hex_secret_hash(value: &str) -> Result<String, String> {
    let raw = hex::decode(value).map_err(|error| format!("invalid fixture secret: {error}"))?;
    if raw.len() != 32 {
        return Err("fixture secret must decode to 32 bytes".to_owned());
    }
    Ok(format!("{:x}", Sha256::digest(raw)))
}
