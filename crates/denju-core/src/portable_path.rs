use std::collections::BTreeMap;

use thiserror::Error;
use unicode_normalization::UnicodeNormalization;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PortablePath(String);

impl PortablePath {
    pub fn parse(value: &str) -> Result<Self, PortablePathError> {
        validate_portable_path(value)?;
        Ok(Self(value.to_owned()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn component_count(&self) -> usize {
        self.0.split('/').count()
    }

    fn components(&self) -> impl Iterator<Item = &str> {
        self.0.split('/')
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PortableEntry {
    path: PortablePath,
    kind: PortableEntryKind,
}

impl PortableEntry {
    pub fn new(path: &str, kind: PortableEntryKind) -> Result<Self, PortablePathError> {
        Ok(Self {
            path: PortablePath::parse(path)?,
            kind,
        })
    }

    pub const fn path(&self) -> &PortablePath {
        &self.path
    }

    pub const fn kind(&self) -> &PortableEntryKind {
        &self.kind
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PortableEntryKind {
    File { executable: bool },
    Directory,
    Symlink { target: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PortableTree {
    entries: Vec<PortableEntry>,
}

impl PortableTree {
    pub fn entries(&self) -> &[PortableEntry] {
        &self.entries
    }
}

pub fn validate_portable_tree(
    entries: impl IntoIterator<Item = PortableEntry>,
) -> Result<PortableTree, PortableTreeError> {
    let mut entries = entries.into_iter().collect::<Vec<_>>();
    entries.sort_by(|left, right| left.path.as_str().cmp(right.path.as_str()));

    let mut folded_paths = BTreeMap::<String, String>::new();
    let mut kinds = BTreeMap::<String, EntryClass>::new();

    for entry in &entries {
        let path = entry.path.as_str();
        let folded = fold_case(path);
        if let Some(existing) = folded_paths.insert(folded, path.to_owned()) {
            return Err(PortableTreeError::CaseCollision {
                first: existing,
                second: path.to_owned(),
            });
        }

        let class = match entry.kind {
            PortableEntryKind::File { .. } => EntryClass::File,
            PortableEntryKind::Directory => EntryClass::Directory,
            PortableEntryKind::Symlink { .. } => EntryClass::Symlink,
        };
        kinds.insert(path.to_owned(), class);
    }

    for entry in &entries {
        ensure_parent_components_are_directories(entry.path(), &kinds)?;
        if let PortableEntryKind::Symlink { target } = entry.kind() {
            validate_symlink_target(entry.path(), target)?;
        }
    }

    Ok(PortableTree { entries })
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum PortablePathError {
    #[error("portable paths cannot be empty")]
    Empty,
    #[error("portable paths must be relative and use '/' separators")]
    NotRelative,
    #[error("portable paths cannot contain empty, '.' or '..' components")]
    InvalidComponent,
    #[error("portable paths must be NFC-normalized UTF-8")]
    NotNfc,
    #[error("portable paths contain a character unavailable in the cross-platform profile")]
    InvalidCharacter,
    #[error("portable path components cannot end in a dot or space")]
    TrailingDotOrSpace,
    #[error("portable path component is reserved on Windows")]
    WindowsReservedName,
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum PortableTreeError {
    #[error("portable paths collide case-insensitively: {first} and {second}")]
    CaseCollision { first: String, second: String },
    #[error("non-directory entry {parent} cannot contain child {child}")]
    NonDirectoryParent { parent: String, child: String },
    #[error("nested entry {child} is missing directory entry {parent}")]
    MissingParentDirectory { parent: String, child: String },
    #[error("symlink {path} must have a non-empty relative target")]
    InvalidSymlinkTarget { path: String },
    #[error("symlink {path} escapes the skill root through target {target}")]
    EscapingSymlink { path: String, target: String },
    #[error("symlink {path} target is not portable: {source}")]
    InvalidSymlinkTargetComponent {
        path: String,
        source: PortablePathError,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EntryClass {
    File,
    Directory,
    Symlink,
}

fn validate_portable_path(value: &str) -> Result<(), PortablePathError> {
    if value.is_empty() {
        return Err(PortablePathError::Empty);
    }
    if value.starts_with('/') || value.starts_with('\\') || value.contains('\\') {
        return Err(PortablePathError::NotRelative);
    }
    if value.nfc().collect::<String>() != value {
        return Err(PortablePathError::NotNfc);
    }

    for component in value.split('/') {
        validate_component(component, false)?;
    }
    Ok(())
}

fn validate_component(
    component: &str,
    allow_dot_navigation: bool,
) -> Result<(), PortablePathError> {
    if component.is_empty() {
        return Err(PortablePathError::InvalidComponent);
    }
    if matches!(component, "." | "..") {
        return if allow_dot_navigation {
            Ok(())
        } else {
            Err(PortablePathError::InvalidComponent)
        };
    }
    if component.ends_with('.') || component.ends_with(' ') {
        return Err(PortablePathError::TrailingDotOrSpace);
    }
    if component.chars().any(is_invalid_windows_character) {
        return Err(PortablePathError::InvalidCharacter);
    }
    if is_windows_reserved_name(component) {
        return Err(PortablePathError::WindowsReservedName);
    }
    Ok(())
}

fn is_invalid_windows_character(character: char) -> bool {
    character == '\0'
        || character.is_control()
        || matches!(
            character,
            '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*'
        )
}

fn is_windows_reserved_name(component: &str) -> bool {
    let stem = component.split('.').next().unwrap_or(component);
    let upper = stem.to_ascii_uppercase();
    matches!(upper.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || upper.strip_prefix("COM").is_some_and(|suffix| {
            matches!(
                suffix,
                "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" | "¹" | "²" | "³"
            )
        })
        || upper.strip_prefix("LPT").is_some_and(|suffix| {
            matches!(
                suffix,
                "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" | "¹" | "²" | "³"
            )
        })
}

fn fold_case(value: &str) -> String {
    value.chars().flat_map(char::to_lowercase).collect()
}

fn ensure_parent_components_are_directories(
    path: &PortablePath,
    kinds: &BTreeMap<String, EntryClass>,
) -> Result<(), PortableTreeError> {
    let components = path.components().collect::<Vec<_>>();
    for end in 1..components.len() {
        let parent = components[..end].join("/");
        match kinds.get(&parent) {
            Some(EntryClass::Directory) => {}
            Some(_) => {
                return Err(PortableTreeError::NonDirectoryParent {
                    parent,
                    child: path.as_str().to_owned(),
                });
            }
            None => {
                return Err(PortableTreeError::MissingParentDirectory {
                    parent,
                    child: path.as_str().to_owned(),
                });
            }
        }
    }
    Ok(())
}

fn validate_symlink_target(path: &PortablePath, target: &str) -> Result<(), PortableTreeError> {
    if target.is_empty()
        || target.starts_with('/')
        || target.starts_with('\\')
        || target.contains('\\')
        || target.contains('\0')
    {
        return Err(PortableTreeError::InvalidSymlinkTarget {
            path: path.as_str().to_owned(),
        });
    }
    if target.nfc().collect::<String>() != target {
        return Err(PortableTreeError::InvalidSymlinkTargetComponent {
            path: path.as_str().to_owned(),
            source: PortablePathError::NotNfc,
        });
    }

    let mut depth = path.component_count().saturating_sub(1);
    let mut saw_component = false;
    for component in target.split('/') {
        saw_component = true;
        if component.is_empty() {
            return Err(PortableTreeError::InvalidSymlinkTarget {
                path: path.as_str().to_owned(),
            });
        }
        match component {
            "." => {}
            ".." => {
                if depth == 0 {
                    return Err(PortableTreeError::EscapingSymlink {
                        path: path.as_str().to_owned(),
                        target: target.to_owned(),
                    });
                }
                depth -= 1;
            }
            other => {
                validate_component(other, true).map_err(|source| {
                    PortableTreeError::InvalidSymlinkTargetComponent {
                        path: path.as_str().to_owned(),
                        source,
                    }
                })?;
                depth += 1;
            }
        }
    }

    if !saw_component {
        return Err(PortableTreeError::InvalidSymlinkTarget {
            path: path.as_str().to_owned(),
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_cross_platform_path_hazards() {
        for invalid in [
            "../escape",
            "/absolute",
            "nested\\windows",
            "CON",
            "aux.txt",
            "COM¹.log",
            "lpt³",
            "name.",
            "name ",
            "a//b",
            "a/./b",
            "a/../b",
            "a:b",
        ] {
            assert!(PortablePath::parse(invalid).is_err(), "accepted {invalid}");
        }

        assert_eq!(
            PortablePath::parse("cafe\u{301}.md"),
            Err(PortablePathError::NotNfc)
        );
    }

    #[test]
    fn detects_case_collisions_and_file_parents() {
        let collision = validate_portable_tree([
            PortableEntry::new("Readme.md", PortableEntryKind::File { executable: false })
                .expect("path"),
            PortableEntry::new("README.md", PortableEntryKind::File { executable: false })
                .expect("path"),
        ]);
        assert!(matches!(
            collision,
            Err(PortableTreeError::CaseCollision { .. })
        ));

        let nested_under_file = validate_portable_tree([
            PortableEntry::new("scripts", PortableEntryKind::File { executable: false })
                .expect("path"),
            PortableEntry::new(
                "scripts/run.sh",
                PortableEntryKind::File { executable: true },
            )
            .expect("path"),
        ]);
        assert!(matches!(
            nested_under_file,
            Err(PortableTreeError::NonDirectoryParent { .. })
        ));

        let missing_parent = validate_portable_tree([PortableEntry::new(
            "scripts/run.sh",
            PortableEntryKind::File { executable: true },
        )
        .expect("path")]);
        assert!(matches!(
            missing_parent,
            Err(PortableTreeError::MissingParentDirectory { .. })
        ));
    }

    #[test]
    fn relative_symlinks_must_remain_inside_root() {
        let valid = validate_portable_tree([
            PortableEntry::new("shared", PortableEntryKind::Directory).expect("dir"),
            PortableEntry::new("scripts", PortableEntryKind::Directory).expect("dir"),
            PortableEntry::new(
                "scripts/tool",
                PortableEntryKind::Symlink {
                    target: "../shared/tool".to_owned(),
                },
            )
            .expect("symlink"),
        ]);
        assert!(valid.is_ok());

        let escaping = validate_portable_tree([PortableEntry::new(
            "tool",
            PortableEntryKind::Symlink {
                target: "../outside".to_owned(),
            },
        )
        .expect("symlink")]);
        assert!(matches!(
            escaping,
            Err(PortableTreeError::EscapingSymlink { .. })
        ));
    }

    #[test]
    fn valid_tree_preserves_executable_and_link_semantics() {
        let tree = validate_portable_tree([
            PortableEntry::new("SKILL.md", PortableEntryKind::File { executable: false })
                .expect("skill"),
            PortableEntry::new("scripts", PortableEntryKind::Directory).expect("scripts"),
            PortableEntry::new(
                "scripts/run.sh",
                PortableEntryKind::File { executable: true },
            )
            .expect("script"),
            PortableEntry::new(
                "latest",
                PortableEntryKind::Symlink {
                    target: "scripts/run.sh".to_owned(),
                },
            )
            .expect("link"),
        ])
        .expect("valid tree");

        assert_eq!(tree.entries().len(), 4);
        let executable_script = tree
            .entries()
            .iter()
            .find(|entry| entry.path().as_str() == "scripts/run.sh")
            .expect("script entry");
        assert!(matches!(
            executable_script.kind(),
            PortableEntryKind::File { executable: true }
        ));
    }
}
