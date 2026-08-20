use std::{fmt, str::FromStr};

use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::{AuthorPrincipalId, OperationId, PortablePath};

const TREE_DOMAIN: &[u8] = b"denju:tree:v1\0";
const REVISION_DOMAIN: &[u8] = b"denju:revision:v1\0";

macro_rules! sha256_id {
    ($name:ident) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub struct $name([u8; 32]);

        impl $name {
            pub const fn from_bytes(bytes: [u8; 32]) -> Self {
                Self(bytes)
            }

            pub const fn as_bytes(&self) -> &[u8; 32] {
                &self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str(&hex::encode(self.0))
            }
        }

        impl FromStr for $name {
            type Err = ObjectIdError;

            fn from_str(value: &str) -> Result<Self, Self::Err> {
                let bytes = hex::decode(value).map_err(ObjectIdError::InvalidHex)?;
                let bytes: [u8; 32] = bytes
                    .try_into()
                    .map_err(|bytes: Vec<u8>| ObjectIdError::InvalidLength(bytes.len()))?;
                Ok(Self(bytes))
            }
        }
    };
}

sha256_id!(BlobId);
sha256_id!(TreeId);
sha256_id!(RevisionId);

impl BlobId {
    pub fn hash(bytes: &[u8]) -> Self {
        Self(Sha256::digest(bytes).into())
    }
}

#[derive(Debug, Error)]
pub enum ObjectIdError {
    #[error("invalid hexadecimal object ID: {0}")]
    InvalidHex(hex::FromHexError),
    #[error("object IDs are 32 bytes, got {0}")]
    InvalidLength(usize),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TreeEntry {
    name: String,
    kind: TreeEntryKind,
}

impl TreeEntry {
    pub fn new(name: impl Into<String>, kind: TreeEntryKind) -> Result<Self, TreeError> {
        let name = name.into();
        let path = PortablePath::parse(&name).map_err(TreeError::InvalidName)?;
        if path.component_count() != 1 {
            return Err(TreeError::NestedName);
        }
        Ok(Self { name, kind })
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub const fn kind(&self) -> &TreeEntryKind {
        &self.kind
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TreeEntryKind {
    File { blob: BlobId, executable: bool },
    Directory { tree: TreeId },
    Symlink { target: String },
}

impl TreeId {
    pub fn from_entries(entries: &[TreeEntry]) -> Result<Self, TreeError> {
        let mut entries = entries.iter().collect::<Vec<_>>();
        entries.sort_by(|left, right| left.name.as_bytes().cmp(right.name.as_bytes()));

        for pair in entries.windows(2) {
            if pair[0].name == pair[1].name {
                return Err(TreeError::DuplicateName(pair[0].name.clone()));
            }
        }

        let entry_count = u32::try_from(entries.len()).map_err(|_| TreeError::TooManyEntries)?;
        let mut hasher = Sha256::new();
        hasher.update(TREE_DOMAIN);
        hasher.update(entry_count.to_be_bytes());

        for entry in entries {
            let payload = encode_tree_entry(entry)?;
            let payload_len = u32::try_from(payload.len()).map_err(|_| TreeError::EntryTooLarge)?;
            hasher.update(payload_len.to_be_bytes());
            hasher.update(payload);
        }

        Ok(Self(hasher.finalize().into()))
    }
}

#[derive(Debug, Error)]
pub enum TreeError {
    #[error("invalid tree entry name: {0}")]
    InvalidName(crate::PortablePathError),
    #[error("tree entries are direct children and cannot contain '/'")]
    NestedName,
    #[error("duplicate tree entry name: {0}")]
    DuplicateName(String),
    #[error("tree contains too many entries")]
    TooManyEntries,
    #[error("tree entry transcript exceeds the v1 length field")]
    EntryTooLarge,
    #[error("tree entry name exceeds the v1 length field")]
    NameTooLarge,
    #[error("symlink target exceeds the v1 length field")]
    SymlinkTargetTooLarge,
}

fn encode_tree_entry(entry: &TreeEntry) -> Result<Vec<u8>, TreeError> {
    let name = entry.name.as_bytes();
    let name_len = u32::try_from(name.len()).map_err(|_| TreeError::NameTooLarge)?;
    let mut encoded = Vec::with_capacity(name.len() + 40);
    encoded.extend_from_slice(&name_len.to_be_bytes());
    encoded.extend_from_slice(name);

    match &entry.kind {
        TreeEntryKind::File { blob, executable } => {
            encoded.push(1);
            encoded.push(u8::from(*executable));
            encoded.extend_from_slice(blob.as_bytes());
        }
        TreeEntryKind::Directory { tree } => {
            encoded.push(2);
            encoded.extend_from_slice(tree.as_bytes());
        }
        TreeEntryKind::Symlink { target } => {
            encoded.push(3);
            let target = target.as_bytes();
            let target_len =
                u32::try_from(target.len()).map_err(|_| TreeError::SymlinkTargetTooLarge)?;
            encoded.extend_from_slice(&target_len.to_be_bytes());
            encoded.extend_from_slice(target);
        }
    }

    Ok(encoded)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Revision {
    root: TreeId,
    parents: Vec<RevisionId>,
    author: AuthorPrincipalId,
    operation: OperationId,
}

impl Revision {
    pub fn new(
        root: TreeId,
        mut parents: Vec<RevisionId>,
        author: AuthorPrincipalId,
        operation: OperationId,
    ) -> Result<Self, RevisionError> {
        if parents.len() > 2 {
            return Err(RevisionError::TooManyParents);
        }
        parents.sort_unstable();
        if parents.windows(2).any(|pair| pair[0] == pair[1]) {
            return Err(RevisionError::DuplicateParent);
        }
        Ok(Self {
            root,
            parents,
            author,
            operation,
        })
    }

    pub const fn root(&self) -> TreeId {
        self.root
    }

    pub fn parents(&self) -> &[RevisionId] {
        &self.parents
    }

    pub const fn author(&self) -> AuthorPrincipalId {
        self.author
    }

    pub const fn operation(&self) -> OperationId {
        self.operation
    }

    pub fn id(&self) -> RevisionId {
        let mut hasher = Sha256::new();
        hasher.update(REVISION_DOMAIN);
        hasher.update(self.root.as_bytes());
        let parent_count =
            u32::try_from(self.parents.len()).expect("revision has at most two parents");
        hasher.update(parent_count.to_be_bytes());
        for parent in &self.parents {
            hasher.update(parent.as_bytes());
        }
        hasher.update(self.author.as_bytes());
        hasher.update(self.operation.as_bytes());
        RevisionId(hasher.finalize().into())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum RevisionError {
    #[error("revisions support at most two parents")]
    TooManyParents,
    #[error("a revision cannot name the same parent twice")]
    DuplicateParent,
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use super::*;

    #[test]
    fn blob_identity_is_raw_sha256() {
        assert_eq!(
            BlobId::hash(b"hello\n").to_string(),
            "5891b5b522d5df086d0ff0b110fbd9d21bb4fc7163af34d08286a2e846f6be03"
        );
    }

    #[test]
    fn tree_identity_sorts_entries_by_name_bytes() {
        let blob = BlobId::hash(b"body");
        let a = TreeEntry::new(
            "a.txt",
            TreeEntryKind::File {
                blob,
                executable: false,
            },
        )
        .expect("entry");
        let b = TreeEntry::new(
            "b.txt",
            TreeEntryKind::File {
                blob,
                executable: true,
            },
        )
        .expect("entry");

        assert_eq!(
            TreeId::from_entries(&[a.clone(), b.clone()]).expect("tree"),
            TreeId::from_entries(&[b, a]).expect("tree")
        );
    }

    #[test]
    fn revision_parent_order_is_not_semantic() {
        let root = TreeId::from_bytes([7; 32]);
        let parent_a = RevisionId::from_bytes([1; 32]);
        let parent_b = RevisionId::from_bytes([2; 32]);
        let author =
            AuthorPrincipalId::from_str("01890f47-6a1c-7cc2-98c1-5f6c1ed8a3a1").expect("author");
        let operation =
            OperationId::from_str("01890f47-6a1d-7ad0-8f43-9a4d8c29f002").expect("operation");

        let left = Revision::new(root, vec![parent_a, parent_b], author, operation).expect("left");
        let right =
            Revision::new(root, vec![parent_b, parent_a], author, operation).expect("right");
        assert_eq!(left, right);
        assert_eq!(left.id(), right.id());
    }
}
