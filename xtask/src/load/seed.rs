use denju_core::{OwnedSkillEntry, build_deterministic_skill_snapshot};
use denju_registry::Registry;

#[derive(Debug, Clone)]
pub(super) struct SeededPublicSkill {
    pub resource_id: String,
    pub locator: String,
    pub generation: u64,
    pub revision_id: String,
}

pub(super) async fn seed_public_catalog(
    registry: &Registry,
    owner: &str,
    count: usize,
) -> Result<Vec<SeededPublicSkill>, String> {
    let mut seeded = Vec::with_capacity(count);
    for index in 0..count {
        let name = format!("load-skill-{index:04}");
        let skill_md = format!(
            "---\nname: {name}\ndescription: Denju benchmark catalog skill {index}.\n---\n# Load skill {index}\n"
        );
        let entries = vec![
            OwnedSkillEntry::File {
                path: "SKILL.md".to_owned(),
                bytes: skill_md.into_bytes(),
                executable: false,
            },
            OwnedSkillEntry::File {
                path: "notes.txt".to_owned(),
                bytes: format!("benchmark fixture {index}\n").into_bytes(),
                executable: false,
            },
        ];
        let snapshot = build_deterministic_skill_snapshot(&name, &entries)
            .map_err(|error| error.to_string())?;
        let detail = registry
            .seed_public_skill(owner, &snapshot, &entries)
            .await
            .map_err(|error| error.to_string())?;
        seeded.push(SeededPublicSkill {
            resource_id: detail.skill.resource_id,
            locator: detail.skill.locator,
            generation: detail.skill.generation,
            revision_id: detail.skill.revision_id,
        });
    }
    Ok(seeded)
}
