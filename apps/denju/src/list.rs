use std::collections::{BTreeMap, BTreeSet};

use denju_client::RegistryClient;
use denju_local::{
    LocalDatabase, LocalPaths, ManagedSkillRecord, OwnedSkillRecord, PackMaterializedSkillRecord,
    PackSkillSourceRecord, PackSubscriptionRecord, SubscriptionRecord, resolve_harness_roots,
};
use denju_wire::{
    CatalogResourceKind, CatalogSearchQuery, CatalogVisibility, CliErrorCode, PackRequirement,
    SearchSort, SubscriptionContent,
};
use futures_util::{StreamExt, stream};
use serde::Serialize;

use crate::{
    context::{authenticated_registry_client, local_error},
    setup::RuntimeError,
};

const METADATA_CONCURRENCY: usize = 16;

#[derive(Debug, Clone, Serialize)]
pub(crate) struct ListOutcome {
    pub(crate) agents_root: String,
    pub(crate) claude_root: String,
    pub(crate) registry_metadata_complete: bool,
    pub(crate) skills: Vec<ListSkill>,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct ListSkill {
    pub(crate) resource_id: String,
    pub(crate) locator: String,
    pub(crate) visibility: String,
    pub(crate) version: Option<u64>,
    pub(crate) source: String,
    pub(crate) relationships: Vec<String>,
    pub(crate) desired_revision_id: String,
    pub(crate) materialized_revision_id: Option<String>,
    pub(crate) packs: Vec<ListPack>,
    pub(crate) paths: ListPaths,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct ListPack {
    pub(crate) locator: String,
    pub(crate) version: i64,
    pub(crate) source: String,
    pub(crate) source_label: String,
    pub(crate) enforced: bool,
    pub(crate) member_version: Option<u64>,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct ListPaths {
    pub(crate) canonical: String,
    pub(crate) agents: Option<String>,
    pub(crate) claude: Option<String>,
}

#[derive(Debug, Clone)]
struct CatalogMetadata {
    visibility: String,
    version: Option<u64>,
}

#[derive(Debug, Clone)]
struct RemotePackMember {
    resolved_release_version: Option<u64>,
    private_workspace: bool,
    pack_public: bool,
}

#[derive(Debug, Default)]
struct SkillBuilder {
    resource_id: String,
    locator: String,
    owner: String,
    skill_name: String,
    owned: Option<OwnedSkillRecord>,
    subscription: Option<SubscriptionRecord>,
    pack_materialized: Option<PackMaterializedSkillRecord>,
    managed: Option<ManagedSkillRecord>,
    pack_sources: Vec<PackSkillSourceRecord>,
}

pub(crate) async fn list() -> Result<ListOutcome, RuntimeError> {
    let paths = LocalPaths::discover().map_err(local_error)?;
    if !paths.state_db.is_file() {
        return Err(
            RuntimeError::new(CliErrorCode::SetupRequired, "Denju is not set up")
                .recovery("denju setup"),
        );
    }
    let db = LocalDatabase::open(&paths.state_db)
        .await
        .map_err(local_error)?;
    let recorded = db.harness_config().await.map_err(local_error)?;
    let roots = resolve_harness_roots(&paths, recorded.as_ref()).map_err(local_error)?;

    let owned = db.owned_skills().await.map_err(local_error)?;
    let subscriptions = db.subscriptions().await.map_err(local_error)?;
    let managed = db.managed_skills().await.map_err(local_error)?;
    let pack_materialized = db.pack_materialized_skills().await.map_err(local_error)?;
    let pack_sources = db.pack_skill_sources().await.map_err(local_error)?;
    let packs = db.pack_subscriptions().await.map_err(local_error)?;
    let owned_suppressed = db
        .source_suppressions("owned")
        .await
        .map_err(local_error)?
        .into_iter()
        .collect::<BTreeSet<_>>();
    let subscriptions_suppressed = db
        .source_suppressions("subscription")
        .await
        .map_err(local_error)?
        .into_iter()
        .collect::<BTreeSet<_>>();

    let mut builders = BTreeMap::<String, SkillBuilder>::new();
    for record in owned {
        let builder = builder_for(
            &mut builders,
            &record.resource_id,
            &record.locator,
            &record.owner,
            &record.skill_name,
        );
        builder.owned = Some(record);
    }
    for record in subscriptions {
        let builder = builder_for(
            &mut builders,
            &record.resource_id,
            &record.locator,
            &record.owner,
            &record.skill_name,
        );
        builder.subscription = Some(record);
    }
    for record in pack_materialized {
        let builder = builder_for(
            &mut builders,
            &record.resource_id,
            &record.locator,
            &record.owner,
            &record.skill_name,
        );
        builder.pack_materialized = Some(record);
    }
    for record in pack_sources {
        let builder = builder_for(
            &mut builders,
            &record.resource_id,
            &record.locator,
            &record.owner,
            &record.skill_name,
        );
        builder.pack_sources.push(record);
    }
    for record in managed {
        let builder = builder_for(
            &mut builders,
            &record.resource_id,
            &record.locator,
            &record.owner,
            &record.skill_name,
        );
        builder.managed = Some(record);
    }

    let pack_by_source = packs
        .into_iter()
        .map(|pack| (pack.source_id.clone(), pack))
        .collect::<BTreeMap<_, _>>();
    let targets = builders
        .values()
        .map(|builder| (builder.resource_id.clone(), builder.locator.clone()))
        .collect::<Vec<_>>();

    let (catalog_metadata, remote_pack_members, registry_metadata_complete) =
        remote_metadata(&paths, &db, &targets, !pack_by_source.is_empty()).await;

    let mut skills = builders
        .into_values()
        .map(|builder| {
            finish_skill(
                &paths,
                &roots,
                builder,
                &owned_suppressed,
                &subscriptions_suppressed,
                &pack_by_source,
                &catalog_metadata,
                &remote_pack_members,
            )
        })
        .collect::<Vec<_>>();
    skills.sort_by(|left, right| left.locator.cmp(&right.locator));

    Ok(ListOutcome {
        agents_root: roots.codex_root.display().to_string(),
        claude_root: roots.claude_root.display().to_string(),
        registry_metadata_complete,
        skills,
    })
}

fn builder_for<'a>(
    builders: &'a mut BTreeMap<String, SkillBuilder>,
    resource_id: &str,
    locator: &str,
    owner: &str,
    skill_name: &str,
) -> &'a mut SkillBuilder {
    builders
        .entry(resource_id.to_owned())
        .or_insert_with(|| SkillBuilder {
            resource_id: resource_id.to_owned(),
            locator: locator.to_owned(),
            owner: owner.to_owned(),
            skill_name: skill_name.to_owned(),
            ..SkillBuilder::default()
        })
}

async fn remote_metadata(
    paths: &LocalPaths,
    db: &LocalDatabase,
    targets: &[(String, String)],
    has_packs: bool,
) -> (
    BTreeMap<String, CatalogMetadata>,
    BTreeMap<(String, String, String), RemotePackMember>,
    bool,
) {
    if targets.is_empty() {
        return (BTreeMap::new(), BTreeMap::new(), true);
    }
    let Ok(client) = authenticated_registry_client(paths, db).await else {
        return (BTreeMap::new(), BTreeMap::new(), false);
    };

    let catalog = fetch_catalog_metadata(&client, targets);
    let packs = async {
        if has_packs {
            client.pack_subscriptions().await.ok()
        } else {
            None
        }
    };
    let (catalog, packs) = tokio::join!(catalog, packs);
    let pack_complete = !has_packs || packs.is_some();
    let remote_pack_members = packs.map_or_else(BTreeMap::new, index_remote_pack_members);
    (catalog.0, remote_pack_members, catalog.1 && pack_complete)
}

async fn fetch_catalog_metadata(
    client: &RegistryClient,
    targets: &[(String, String)],
) -> (BTreeMap<String, CatalogMetadata>, bool) {
    let results = stream::iter(targets.iter().cloned().map(|(resource_id, locator)| {
        let client = client.clone();
        async move {
            let query = CatalogSearchQuery {
                q: locator.clone(),
                limit: Some(8),
                cursor: None,
                sort: SearchSort::Relevance,
                following: false,
                topic: None,
            };
            let result = client.search_catalog(&query).await;
            (resource_id, result)
        }
    }))
    .buffer_unordered(METADATA_CONCURRENCY)
    .collect::<Vec<_>>()
    .await;

    let mut complete = true;
    let mut metadata = BTreeMap::new();
    for (resource_id, result) in results {
        let Ok(result) = result else {
            complete = false;
            continue;
        };
        let Some(item) = result.items.into_iter().find(|item| {
            item.resource_id == resource_id && item.kind == CatalogResourceKind::Skill
        }) else {
            complete = false;
            continue;
        };
        metadata.insert(
            resource_id,
            CatalogMetadata {
                visibility: match item.visibility {
                    CatalogVisibility::Public => "public",
                    CatalogVisibility::Private => "private",
                    CatalogVisibility::Team => "team",
                }
                .to_owned(),
                version: item.version,
            },
        );
    }
    (metadata, complete)
}

fn index_remote_pack_members(
    catalog: denju_wire::PackSubscriptionCatalog,
) -> BTreeMap<(String, String, String), RemotePackMember> {
    let mut members = BTreeMap::new();
    for requirement in catalog.packs {
        index_remote_pack_requirement(&mut members, requirement);
    }
    members
}

fn index_remote_pack_requirement(
    members: &mut BTreeMap<(String, String, String), RemotePackMember>,
    requirement: PackRequirement,
) {
    let source_id = requirement.source.source_id;
    let pack_public = requirement.pack.pack.visibility == "public";
    for member in requirement.pack.members {
        let private_workspace = member.desired.as_ref().is_some_and(|desired| {
            matches!(&desired.content, SubscriptionContent::PrivateWorkspace)
        });
        members.insert(
            (source_id.clone(), member.resource_id, member.revision_id),
            RemotePackMember {
                resolved_release_version: member.resolved_release_version,
                private_workspace,
                pack_public,
            },
        );
    }
}

#[allow(clippy::too_many_arguments)]
fn finish_skill(
    paths: &LocalPaths,
    roots: &denju_local::ResolvedHarnessRoots,
    mut builder: SkillBuilder,
    owned_suppressed: &BTreeSet<String>,
    subscriptions_suppressed: &BTreeSet<String>,
    pack_by_source: &BTreeMap<String, PackSubscriptionRecord>,
    catalog_metadata: &BTreeMap<String, CatalogMetadata>,
    remote_pack_members: &BTreeMap<(String, String, String), RemotePackMember>,
) -> ListSkill {
    builder
        .pack_sources
        .sort_by(|left, right| left.source_id.cmp(&right.source_id));

    let owned_active = builder.owned.is_some() && !owned_suppressed.contains(&builder.resource_id);
    let subscription_active =
        builder.subscription.is_some() && !subscriptions_suppressed.contains(&builder.resource_id);
    let pack_active = builder.pack_materialized.is_some();
    let source = if owned_active {
        "owned"
    } else if subscription_active {
        "subscription"
    } else if pack_active || !builder.pack_sources.is_empty() {
        "pack"
    } else {
        "unknown"
    }
    .to_owned();

    let mut relationships = Vec::new();
    if builder.owned.is_some() {
        relationships.push("owned".to_owned());
    }
    if builder.subscription.is_some() {
        relationships.push("subscription".to_owned());
    }
    if !builder.pack_sources.is_empty() || builder.pack_materialized.is_some() {
        relationships.push("pack".to_owned());
    }

    let desired_revision_id = match source.as_str() {
        "owned" => builder
            .owned
            .as_ref()
            .map(|record| record.desired_revision_id.clone()),
        "subscription" => builder
            .subscription
            .as_ref()
            .map(|record| record.desired_revision_id.clone()),
        "pack" => builder
            .pack_materialized
            .as_ref()
            .map(|record| record.desired_revision_id.clone())
            .or_else(|| {
                builder
                    .pack_sources
                    .first()
                    .map(|record| record.desired_revision_id.clone())
            }),
        _ => None,
    }
    .or_else(|| {
        builder
            .owned
            .as_ref()
            .map(|record| record.desired_revision_id.clone())
    })
    .or_else(|| {
        builder
            .subscription
            .as_ref()
            .map(|record| record.desired_revision_id.clone())
    })
    .or_else(|| {
        builder
            .pack_sources
            .first()
            .map(|record| record.desired_revision_id.clone())
    })
    .unwrap_or_default();

    let materialized_revision_id = builder
        .managed
        .as_ref()
        .and_then(|record| record.materialized_revision_id.clone())
        .or_else(|| {
            builder
                .pack_materialized
                .as_ref()
                .map(|record| record.materialized_revision_id.clone())
        });
    let harness_name = builder
        .managed
        .as_ref()
        .and_then(|record| record.harness_name.clone())
        .or_else(|| {
            builder
                .pack_materialized
                .as_ref()
                .and_then(|record| record.harness_name.clone())
        });

    let mut packs = Vec::new();
    let mut pack_version_for_active_revision = None;
    let mut fallback_private = false;
    let mut fallback_public = false;
    for pack_source in &builder.pack_sources {
        let Some(pack) = pack_by_source.get(&pack_source.source_id) else {
            continue;
        };
        let remote = remote_pack_members.get(&(
            pack_source.source_id.clone(),
            builder.resource_id.clone(),
            pack_source.desired_revision_id.clone(),
        ));
        let member_version = remote.and_then(|member| member.resolved_release_version);
        if source == "pack" && pack_source.desired_revision_id == desired_revision_id {
            pack_version_for_active_revision = pack_version_for_active_revision.or(member_version);
        }
        fallback_private |= remote.is_some_and(|member| member.private_workspace);
        fallback_public |= remote.is_some_and(|member| member.pack_public);
        packs.push(ListPack {
            locator: pack.locator.clone(),
            version: pack.pack_version,
            source: pack.source_kind.clone(),
            source_label: pack.source_label.clone(),
            enforced: pack.enforced,
            member_version,
        });
    }
    packs.sort_by(|left, right| {
        left.locator
            .cmp(&right.locator)
            .then(left.source_label.cmp(&right.source_label))
    });

    let catalog = catalog_metadata.get(&builder.resource_id);
    let visibility = catalog
        .map(|metadata| metadata.visibility.clone())
        .or_else(|| {
            builder
                .subscription
                .as_ref()
                .filter(|record| record.live_private)
                .map(|_| "private".to_owned())
        })
        .or_else(|| fallback_private.then(|| "private".to_owned()))
        .or_else(|| fallback_public.then(|| "public".to_owned()))
        .unwrap_or_else(|| "unknown".to_owned());

    let version = match source.as_str() {
        "subscription" => builder.subscription.as_ref().and_then(|record| {
            if record.live_private {
                None
            } else {
                u64::try_from(record.release_version).ok()
            }
        }),
        "pack" => pack_version_for_active_revision,
        _ => catalog.and_then(|metadata| metadata.version),
    };

    let canonical = paths.skills.join(&builder.owner).join(&builder.skill_name);
    let agents = harness_name
        .as_deref()
        .map(|name| roots.codex_root.join(name).display().to_string());
    let claude = harness_name
        .as_deref()
        .map(|name| roots.claude_root.join(name).display().to_string());

    ListSkill {
        resource_id: builder.resource_id,
        locator: builder.locator,
        visibility,
        version,
        source,
        relationships,
        desired_revision_id,
        materialized_revision_id,
        packs,
        paths: ListPaths {
            canonical: canonical.display().to_string(),
            agents,
            claude,
        },
    }
}

pub(crate) fn list_text(outcome: &ListOutcome) -> String {
    if outcome.skills.is_empty() {
        return "Denju is not tracking any skills.".to_owned();
    }

    let rows = outcome
        .skills
        .iter()
        .map(|skill| {
            let version = skill
                .version
                .map(|version| format!("v{version}"))
                .unwrap_or_else(|| "—".to_owned());
            let packs = if skill.packs.is_empty() {
                "—".to_owned()
            } else {
                let mut locators = skill
                    .packs
                    .iter()
                    .map(|pack| pack.locator.as_str())
                    .collect::<Vec<_>>();
                locators.dedup();
                locators.join(", ")
            };
            vec![
                skill.locator.clone(),
                skill.visibility.clone(),
                version,
                skill.source.clone(),
                packs,
            ]
        })
        .collect::<Vec<_>>();
    let headers = ["SKILL", "VISIBILITY", "VERSION", "SOURCE", "PACKS"];
    let mut widths = headers.map(str::len);
    for row in &rows {
        for (index, value) in row.iter().enumerate() {
            widths[index] = widths[index].max(value.chars().count());
        }
    }

    let mut lines = vec![format_row(&headers, &widths)];
    lines.extend(rows.iter().map(|row| {
        let values = [
            row[0].as_str(),
            row[1].as_str(),
            row[2].as_str(),
            row[3].as_str(),
            row[4].as_str(),
        ];
        format_row(&values, &widths)
    }));
    if !outcome.registry_metadata_complete {
        lines.push(
            "Registry metadata is incomplete; unknown visibility/version fields are local-only."
                .to_owned(),
        );
    }
    lines.join("\n")
}

fn format_row(values: &[&str; 5], widths: &[usize; 5]) -> String {
    values
        .iter()
        .enumerate()
        .map(|(index, value)| {
            if index + 1 == values.len() {
                (*value).to_owned()
            } else {
                format!("{value:<width$}", width = widths[index])
            }
        })
        .collect::<Vec<_>>()
        .join("  ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn list_text_keeps_inventory_compact_and_pack_aware() {
        let outcome = ListOutcome {
            agents_root: "/home/alice/.agents/skills".to_owned(),
            claude_root: "/home/alice/.claude/skills".to_owned(),
            registry_metadata_complete: true,
            skills: vec![ListSkill {
                resource_id: "resource".to_owned(),
                locator: "@alice/review".to_owned(),
                visibility: "public".to_owned(),
                version: Some(7),
                source: "owned".to_owned(),
                relationships: vec!["owned".to_owned(), "pack".to_owned()],
                desired_revision_id: "revision".to_owned(),
                materialized_revision_id: Some("revision".to_owned()),
                packs: vec![ListPack {
                    locator: "@alice/packs/core".to_owned(),
                    version: 3,
                    source: "direct".to_owned(),
                    source_label: "direct subscription".to_owned(),
                    enforced: false,
                    member_version: Some(7),
                }],
                paths: ListPaths {
                    canonical: "/home/alice/.denju/skills/alice/review".to_owned(),
                    agents: Some("/home/alice/.agents/skills/review".to_owned()),
                    claude: Some("/home/alice/.claude/skills/review".to_owned()),
                },
            }],
        };
        let text = list_text(&outcome);
        assert!(text.contains("SKILL"));
        assert!(text.contains("@alice/review"));
        assert!(text.contains("public"));
        assert!(text.contains("v7"));
        assert!(text.contains("@alice/packs/core"));

        let json = serde_json::to_value(&outcome).unwrap();
        assert_eq!(
            json["skills"][0]["paths"]["canonical"],
            "/home/alice/.denju/skills/alice/review"
        );
        assert_eq!(
            json["skills"][0]["paths"]["agents"],
            "/home/alice/.agents/skills/review"
        );
        assert_eq!(
            json["skills"][0]["paths"]["claude"],
            "/home/alice/.claude/skills/review"
        );
    }
}
