//! Pure Denju domain types and algorithms.

mod ids;
mod locator;
mod object;
mod portable_path;
mod skill;
mod snapshot;

pub use ids::{AuthorPrincipalId, Generation, IdError, NamespaceId, OperationId, ResourceId};
pub use locator::{LocatorError, ResourceKind, ResourceLocator};
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
    parse_skill_document, rewrite_skill_document_name, validate_skill_directory,
    validate_skill_name,
};
pub use snapshot::{
    DeterministicSkillSnapshot, OwnedSkillEntry, SkillManifest, SkillManifestEntry, SnapshotError,
    build_deterministic_skill_snapshot, build_skill_manifest, validate_skill_snapshot,
};
