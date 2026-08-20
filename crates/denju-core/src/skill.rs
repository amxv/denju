use std::collections::BTreeMap;

use serde_yaml::{Mapping, Value};
use thiserror::Error;

use crate::{
    PortableEntry, PortableEntryKind, PortablePathError, PortableTree, PortableTreeError,
    validate_portable_tree,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillFrontmatter {
    name: String,
    description: String,
    license: Option<String>,
    compatibility: Option<String>,
    metadata: BTreeMap<String, String>,
    allowed_tools: Option<String>,
}

impl SkillFrontmatter {
    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn description(&self) -> &str {
        &self.description
    }

    pub fn license(&self) -> Option<&str> {
        self.license.as_deref()
    }

    pub fn compatibility(&self) -> Option<&str> {
        self.compatibility.as_deref()
    }

    pub const fn metadata(&self) -> &BTreeMap<String, String> {
        &self.metadata
    }

    pub fn allowed_tools(&self) -> Option<&str> {
        self.allowed_tools.as_deref()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillDocument {
    frontmatter: SkillFrontmatter,
    body: Vec<u8>,
}

impl SkillDocument {
    pub const fn frontmatter(&self) -> &SkillFrontmatter {
        &self.frontmatter
    }

    pub fn body(&self) -> &[u8] {
        &self.body
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SkillEntry<'a> {
    File {
        path: &'a str,
        bytes: &'a [u8],
        executable: bool,
    },
    Directory {
        path: &'a str,
    },
    Symlink {
        path: &'a str,
        target: &'a str,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidatedSkill {
    document: SkillDocument,
    portable_tree: PortableTree,
}

impl ValidatedSkill {
    pub const fn document(&self) -> &SkillDocument {
        &self.document
    }

    pub const fn portable_tree(&self) -> &PortableTree {
        &self.portable_tree
    }
}

pub fn validate_skill_name(name: &str) -> Result<(), SkillValidationError> {
    let length = name.chars().count();
    if !(1..=64).contains(&length) {
        return Err(SkillValidationError::InvalidName);
    }
    if name.starts_with('-')
        || name.ends_with('-')
        || name.contains("--")
        || !name
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
    {
        return Err(SkillValidationError::InvalidName);
    }
    Ok(())
}

pub fn parse_skill_document(
    parent_directory_name: &str,
    bytes: &[u8],
) -> Result<SkillDocument, SkillValidationError> {
    let source = std::str::from_utf8(bytes).map_err(|_| SkillValidationError::SkillMdNotUtf8)?;
    let (frontmatter_source, body_offset) = split_frontmatter(source)?;
    let value: Value = serde_yaml::from_str(frontmatter_source)
        .map_err(|error| SkillValidationError::InvalidFrontmatter(error.to_string()))?;
    let mapping = value
        .as_mapping()
        .ok_or(SkillValidationError::FrontmatterMustBeMapping)?;

    let name = required_string(mapping, "name")?;
    validate_skill_name(&name)?;
    if name != parent_directory_name {
        return Err(SkillValidationError::NameDoesNotMatchDirectory {
            name,
            directory: parent_directory_name.to_owned(),
        });
    }

    let description = required_string(mapping, "description")?;
    let description_len = description.chars().count();
    if description.is_empty() || description_len > 1024 {
        return Err(SkillValidationError::InvalidDescription);
    }

    let license = optional_string(mapping, "license")?;
    let compatibility = optional_string(mapping, "compatibility")?;
    if compatibility
        .as_ref()
        .is_some_and(|value| value.is_empty() || value.chars().count() > 500)
    {
        return Err(SkillValidationError::InvalidCompatibility);
    }
    let allowed_tools = optional_string(mapping, "allowed-tools")?;
    let metadata = metadata_map(mapping)?;

    Ok(SkillDocument {
        frontmatter: SkillFrontmatter {
            name,
            description,
            license,
            compatibility,
            metadata,
            allowed_tools,
        },
        body: bytes[body_offset..].to_vec(),
    })
}

pub fn validate_skill_directory(
    parent_directory_name: &str,
    entries: &[SkillEntry<'_>],
) -> Result<ValidatedSkill, SkillValidationError> {
    validate_skill_name(parent_directory_name)?;

    let mut portable_entries = Vec::with_capacity(entries.len());
    let mut skill_md = None;

    for entry in entries {
        match entry {
            SkillEntry::File {
                path,
                bytes,
                executable,
            } => {
                portable_entries.push(
                    PortableEntry::new(
                        path,
                        PortableEntryKind::File {
                            executable: *executable,
                        },
                    )
                    .map_err(SkillValidationError::PortablePath)?,
                );
                if *path == "SKILL.md" && skill_md.replace(*bytes).is_some() {
                    return Err(SkillValidationError::DuplicateSkillMd);
                }
            }
            SkillEntry::Directory { path } => portable_entries.push(
                PortableEntry::new(path, PortableEntryKind::Directory)
                    .map_err(SkillValidationError::PortablePath)?,
            ),
            SkillEntry::Symlink { path, target } => portable_entries.push(
                PortableEntry::new(
                    path,
                    PortableEntryKind::Symlink {
                        target: (*target).to_owned(),
                    },
                )
                .map_err(SkillValidationError::PortablePath)?,
            ),
        }
    }

    let portable_tree =
        validate_portable_tree(portable_entries).map_err(SkillValidationError::PortableTree)?;
    let bytes = skill_md.ok_or(SkillValidationError::MissingSkillMd)?;
    let document = parse_skill_document(parent_directory_name, bytes)?;

    Ok(ValidatedSkill {
        document,
        portable_tree,
    })
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum SkillValidationError {
    #[error("skill names must be 1-64 lowercase ASCII letters, numbers, or single hyphens")]
    InvalidName,
    #[error("SKILL.md must be valid UTF-8")]
    SkillMdNotUtf8,
    #[error("SKILL.md must start with YAML frontmatter delimited by '---'")]
    MissingFrontmatter,
    #[error("invalid SKILL.md YAML frontmatter: {0}")]
    InvalidFrontmatter(String),
    #[error("SKILL.md frontmatter must be a YAML mapping")]
    FrontmatterMustBeMapping,
    #[error("required frontmatter field '{0}' is missing or is not a string")]
    MissingRequiredField(&'static str),
    #[error("optional frontmatter field '{0}' must be a string when present")]
    InvalidOptionalField(&'static str),
    #[error("SKILL.md name '{name}' must match parent directory '{directory}'")]
    NameDoesNotMatchDirectory { name: String, directory: String },
    #[error("SKILL.md description must contain 1-1024 characters")]
    InvalidDescription,
    #[error("SKILL.md compatibility must contain 1-500 characters when present")]
    InvalidCompatibility,
    #[error("SKILL.md metadata must map string keys to string values")]
    InvalidMetadata,
    #[error("skill directory must contain one root SKILL.md file")]
    MissingSkillMd,
    #[error("skill directory contains more than one root SKILL.md entry")]
    DuplicateSkillMd,
    #[error("invalid portable path: {0}")]
    PortablePath(PortablePathError),
    #[error("invalid portable tree: {0}")]
    PortableTree(PortableTreeError),
}

fn split_frontmatter(source: &str) -> Result<(&str, usize), SkillValidationError> {
    let mut offset = 0;
    let mut lines = source.split_inclusive('\n');
    let first = lines
        .next()
        .ok_or(SkillValidationError::MissingFrontmatter)?;
    if first.trim_end_matches(['\r', '\n']) != "---" {
        return Err(SkillValidationError::MissingFrontmatter);
    }
    offset += first.len();
    let frontmatter_start = offset;

    for line in lines {
        let line_start = offset;
        offset += line.len();
        if line.trim_end_matches(['\r', '\n']) == "---" {
            return Ok((&source[frontmatter_start..line_start], offset));
        }
    }

    if source[frontmatter_start..].ends_with("\n---")
        || source[frontmatter_start..].ends_with("\r\n---")
    {
        let marker_start = source
            .rfind("---")
            .ok_or(SkillValidationError::MissingFrontmatter)?;
        return Ok((&source[frontmatter_start..marker_start], source.len()));
    }

    Err(SkillValidationError::MissingFrontmatter)
}

fn mapping_key(name: &'static str) -> Value {
    Value::String(name.to_owned())
}

fn required_string(mapping: &Mapping, field: &'static str) -> Result<String, SkillValidationError> {
    mapping
        .get(mapping_key(field))
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or(SkillValidationError::MissingRequiredField(field))
}

fn optional_string(
    mapping: &Mapping,
    field: &'static str,
) -> Result<Option<String>, SkillValidationError> {
    match mapping.get(mapping_key(field)) {
        None => Ok(None),
        Some(Value::String(value)) => Ok(Some(value.clone())),
        Some(_) => Err(SkillValidationError::InvalidOptionalField(field)),
    }
}

fn metadata_map(mapping: &Mapping) -> Result<BTreeMap<String, String>, SkillValidationError> {
    let Some(value) = mapping.get(mapping_key("metadata")) else {
        return Ok(BTreeMap::new());
    };
    let mapping = value
        .as_mapping()
        .ok_or(SkillValidationError::InvalidMetadata)?;
    let mut metadata = BTreeMap::new();
    for (key, value) in mapping {
        let key = key.as_str().ok_or(SkillValidationError::InvalidMetadata)?;
        let value = value
            .as_str()
            .ok_or(SkillValidationError::InvalidMetadata)?;
        metadata.insert(key.to_owned(), value.to_owned());
    }
    Ok(metadata)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_current_agent_skills_frontmatter_and_preserves_body() {
        let bytes = b"---\r\nname: pdf-processing\r\ndescription: Extract PDFs when handling documents.\r\nlicense: Apache-2.0\r\ncompatibility: Requires pdftotext\r\nmetadata:\r\n  author: example-org\r\n  custom-key: custom-value\r\nallowed-tools: Bash(pdftotext:*) Read\r\n---\r\n# Exact body\r\n\r\nKeep me.\r\n";
        let document = parse_skill_document("pdf-processing", bytes).expect("valid skill");

        assert_eq!(document.frontmatter().name(), "pdf-processing");
        assert_eq!(document.frontmatter().license(), Some("Apache-2.0"));
        assert_eq!(
            document.frontmatter().compatibility(),
            Some("Requires pdftotext")
        );
        assert_eq!(
            document.frontmatter().metadata().get("custom-key"),
            Some(&"custom-value".to_owned())
        );
        assert_eq!(
            document.frontmatter().allowed_tools(),
            Some("Bash(pdftotext:*) Read")
        );
        assert_eq!(document.body(), b"# Exact body\r\n\r\nKeep me.\r\n");
    }

    #[test]
    fn rejects_name_and_description_contract_violations() {
        assert_eq!(
            validate_skill_name("PDF"),
            Err(SkillValidationError::InvalidName)
        );
        assert_eq!(
            validate_skill_name("pdf--processing"),
            Err(SkillValidationError::InvalidName)
        );

        let mismatch = parse_skill_document(
            "other-name",
            b"---\nname: code-review\ndescription: Reviews code when asked.\n---\n",
        );
        assert!(matches!(
            mismatch,
            Err(SkillValidationError::NameDoesNotMatchDirectory { .. })
        ));

        let empty_description = parse_skill_document(
            "code-review",
            b"---\nname: code-review\ndescription: \"\"\n---\n",
        );
        assert_eq!(
            empty_description,
            Err(SkillValidationError::InvalidDescription)
        );
    }

    #[test]
    fn enforces_agent_skills_length_boundaries() {
        let max_name = "a".repeat(64);
        assert!(validate_skill_name(&max_name).is_ok());
        assert_eq!(
            validate_skill_name(&"a".repeat(65)),
            Err(SkillValidationError::InvalidName)
        );

        let max_description = "d".repeat(1024);
        let document = format!(
            "---\nname: code-review\ndescription: {max_description}\ncompatibility: {}\n---\n",
            "c".repeat(500)
        );
        assert!(parse_skill_document("code-review", document.as_bytes()).is_ok());

        let too_long_description = format!(
            "---\nname: code-review\ndescription: {}\n---\n",
            "d".repeat(1025)
        );
        assert_eq!(
            parse_skill_document("code-review", too_long_description.as_bytes()),
            Err(SkillValidationError::InvalidDescription)
        );

        let too_long_compatibility = format!(
            "---\nname: code-review\ndescription: Reviews code.\ncompatibility: {}\n---\n",
            "c".repeat(501)
        );
        assert_eq!(
            parse_skill_document("code-review", too_long_compatibility.as_bytes()),
            Err(SkillValidationError::InvalidCompatibility)
        );
    }

    #[test]
    fn metadata_must_be_string_to_string() {
        let invalid = parse_skill_document(
            "code-review",
            b"---\nname: code-review\ndescription: Reviews code.\nmetadata:\n  attempts: 2\n---\n",
        );
        assert_eq!(invalid, Err(SkillValidationError::InvalidMetadata));
    }

    #[test]
    fn validates_complete_skill_tree_in_memory() {
        let skill_md =
            b"---\nname: code-review\ndescription: Reviews code when asked.\n---\n# Review\n";
        let entries = [
            SkillEntry::File {
                path: "SKILL.md",
                bytes: skill_md,
                executable: false,
            },
            SkillEntry::Directory { path: "scripts" },
            SkillEntry::File {
                path: "scripts/check.sh",
                bytes: b"#!/bin/sh\n",
                executable: true,
            },
            SkillEntry::Symlink {
                path: "scripts/current",
                target: "check.sh",
            },
        ];

        let validated = validate_skill_directory("code-review", &entries).expect("valid skill");
        assert_eq!(validated.portable_tree().entries().len(), 4);
        assert_eq!(validated.document().body(), b"# Review\n");
    }
}
