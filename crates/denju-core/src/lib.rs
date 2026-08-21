//! Pure Denju domain types and algorithms.

mod ids;
mod locator;
mod merge;
mod object;
mod portable_path;
mod skill;
mod snapshot;

pub use ids::{AuthorPrincipalId, Generation, IdError, NamespaceId, OperationId, ResourceId};
pub use locator::{LocatorError, ResourceKind, ResourceLocator};
pub use merge::{
    MergeConflict, MergeConflictKind, SkillMergeResult, merge_skill_entries,
    merge_skill_entries_with_resolutions,
};
pub use object::{
    BlobId, ObjectIdError, Revision, RevisionError, RevisionId, TreeEntry, TreeEntryKind,
    TreeError, TreeId,
};
pub use portable_path::{
    PortableEntry, PortableEntryKind, PortablePath, PortablePathError, PortableTree,
    PortableTreeError, validate_portable_tree,
};
pub use skill::{
    SkillDocument, SkillEntry, SkillFrontmatter, SkillValidationError, ValidatedSkill,
    parse_skill_document, rewrite_skill_document_name, skill_document_declared_name,
    validate_skill_directory, validate_skill_name,
};
pub use snapshot::{
    DeterministicSkillSnapshot, OwnedSkillEntry, SkillManifest, SkillManifestEntry,
    SkillManifestTree, SnapshotError, build_deterministic_skill_snapshot, build_skill_manifest,
    build_skill_manifest_from_hashed_entries, validate_declared_skill_manifest,
    validate_skill_snapshot,
};
