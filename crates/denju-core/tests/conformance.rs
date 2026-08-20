use std::str::FromStr;

use denju_core::{
    AuthorPrincipalId, BlobId, OperationId, OwnedSkillEntry, PortableEntry, PortableEntryKind,
    PortablePath, Revision, RevisionId, SkillEntry, TreeEntry, TreeEntryKind, TreeId,
    build_deterministic_skill_snapshot, validate_portable_tree, validate_skill_directory,
};
use serde::Deserialize;

#[derive(Debug, Deserialize)]
struct ObjectFixture {
    version: u32,
    blobs: BlobFixture,
    tree: TreeFixture,
    revision: RevisionFixture,
}

#[derive(Debug, Deserialize)]
struct BlobFixture {
    skill_md_utf8: String,
    skill_md_id: String,
    script_utf8: String,
    script_id: String,
}

#[derive(Debug, Deserialize)]
struct TreeFixture {
    entries: Vec<TreeEntryFixture>,
    transcript_hex: String,
    id: String,
}

#[derive(Debug, Deserialize)]
struct TreeEntryFixture {
    name: String,
    kind: String,
    object_id: Option<String>,
    executable: Option<bool>,
    target: Option<String>,
}

#[derive(Debug, Deserialize)]
struct RevisionFixture {
    root: String,
    parents: Vec<String>,
    author_principal_id: String,
    operation_id: String,
    transcript_hex: String,
    id: String,
}

#[derive(Debug, Deserialize)]
struct PortableFixture {
    version: u32,
    valid_paths: Vec<String>,
    invalid_paths: Vec<String>,
    case_collision: Vec<String>,
    valid_symlink: LinkFixture,
    escaping_symlink: LinkFixture,
}

#[derive(Debug, Deserialize)]
struct LinkFixture {
    path: String,
    target: String,
}

#[derive(Debug, Deserialize)]
struct SnapshotFixture {
    version: u32,
    skill_name: String,
    entries: Vec<SnapshotEntryFixture>,
    root_tree_id: String,
    snapshot_sha256: String,
    snapshot_size: usize,
}

#[derive(Debug, Deserialize)]
struct SnapshotEntryFixture {
    kind: String,
    path: String,
    utf8: Option<String>,
    executable: Option<bool>,
    target: Option<String>,
}

#[test]
fn checked_object_v1_vectors_match_the_semantic_transcript() {
    let fixture: ObjectFixture =
        serde_json::from_str(include_str!("../../../spec/fixtures/object-v1.json"))
            .expect("valid checked object fixture");
    assert_eq!(fixture.version, 1);
    assert_eq!(
        BlobId::hash(&hex::decode(&fixture.tree.transcript_hex).expect("tree transcript hex"))
            .to_string(),
        fixture.tree.id
    );
    assert_eq!(
        BlobId::hash(
            &hex::decode(&fixture.revision.transcript_hex).expect("revision transcript hex")
        )
        .to_string(),
        fixture.revision.id
    );

    assert_eq!(
        BlobId::hash(fixture.blobs.skill_md_utf8.as_bytes()).to_string(),
        fixture.blobs.skill_md_id
    );
    assert_eq!(
        BlobId::hash(fixture.blobs.script_utf8.as_bytes()).to_string(),
        fixture.blobs.script_id
    );

    let entries = fixture
        .tree
        .entries
        .iter()
        .map(tree_entry_from_fixture)
        .collect::<Vec<_>>();
    let tree = TreeId::from_entries(&entries).expect("valid checked tree");
    assert_eq!(tree.to_string(), fixture.tree.id);

    let root = TreeId::from_str(&fixture.revision.root).expect("root tree ID");
    let parents = fixture
        .revision
        .parents
        .iter()
        .map(|value| RevisionId::from_str(value).expect("parent revision ID"))
        .collect::<Vec<_>>();
    let author = AuthorPrincipalId::from_str(&fixture.revision.author_principal_id)
        .expect("author principal ID");
    let operation = OperationId::from_str(&fixture.revision.operation_id).expect("operation ID");
    let revision = Revision::new(root, parents, author, operation).expect("valid checked revision");
    assert_eq!(revision.id().to_string(), fixture.revision.id);
}

#[test]
fn checked_portable_profile_vectors_are_enforced() {
    let fixture: PortableFixture = serde_json::from_str(include_str!(
        "../../../spec/fixtures/portable-profile-v1.json"
    ))
    .expect("valid checked portable fixture");
    assert_eq!(fixture.version, 1);

    for path in &fixture.valid_paths {
        PortablePath::parse(path).unwrap_or_else(|error| panic!("rejected {path}: {error}"));
    }
    for path in &fixture.invalid_paths {
        assert!(
            PortablePath::parse(path).is_err(),
            "accepted invalid path {path}"
        );
    }

    let collision = fixture
        .case_collision
        .iter()
        .map(|path| {
            PortableEntry::new(path, PortableEntryKind::File { executable: false })
                .expect("fixture path itself is portable")
        })
        .collect::<Vec<_>>();
    assert!(validate_portable_tree(collision).is_err());

    let valid_link = PortableEntry::new(
        &fixture.valid_symlink.path,
        PortableEntryKind::Symlink {
            target: fixture.valid_symlink.target,
        },
    )
    .expect("valid link path");
    assert!(
        validate_portable_tree([
            PortableEntry::new("scripts", PortableEntryKind::Directory).expect("scripts dir"),
            valid_link,
        ])
        .is_ok()
    );

    let escaping_link = PortableEntry::new(
        &fixture.escaping_symlink.path,
        PortableEntryKind::Symlink {
            target: fixture.escaping_symlink.target,
        },
    )
    .expect("escaping link path is itself portable");
    assert!(validate_portable_tree([escaping_link]).is_err());
}

#[test]
fn checked_agent_skill_fixture_preserves_supported_metadata_and_body() {
    let skill_md = include_bytes!("../../../spec/fixtures/skills/valid-full/SKILL.md");
    let script = include_bytes!("../../../spec/fixtures/skills/valid-full/scripts/run.sh");
    let entries = [
        SkillEntry::File {
            path: "SKILL.md",
            bytes: skill_md,
            executable: false,
        },
        SkillEntry::Directory { path: "scripts" },
        SkillEntry::File {
            path: "scripts/run.sh",
            bytes: script,
            executable: true,
        },
    ];

    let skill = validate_skill_directory("valid-full", &entries).expect("checked skill is valid");
    let frontmatter = skill.document().frontmatter();
    assert_eq!(frontmatter.name(), "valid-full");
    assert_eq!(frontmatter.license(), Some("Apache-2.0"));
    assert_eq!(
        frontmatter.compatibility(),
        Some("Requires a POSIX shell for the example script.")
    );
    assert_eq!(
        frontmatter
            .metadata()
            .get("arbitrary-key")
            .map(String::as_str),
        Some("arbitrary-value")
    );
    assert_eq!(frontmatter.allowed_tools(), Some("Bash(sh:*) Read"));
    assert!(
        skill
            .document()
            .body()
            .ends_with(b"remain unchanged by metadata validation.\n")
    );
}

#[test]
fn checked_snapshot_v1_vector_is_byte_deterministic() {
    let fixture: SnapshotFixture =
        serde_json::from_str(include_str!("../../../spec/fixtures/snapshot-v1.json"))
            .expect("valid checked snapshot fixture");
    assert_eq!(fixture.version, 1);
    let entries = fixture
        .entries
        .into_iter()
        .map(|entry| match entry.kind.as_str() {
            "file" => OwnedSkillEntry::File {
                path: entry.path,
                bytes: entry.utf8.expect("file bytes").into_bytes(),
                executable: entry.executable.expect("file executable bit"),
            },
            "directory" => OwnedSkillEntry::Directory { path: entry.path },
            "symlink" => OwnedSkillEntry::Symlink {
                path: entry.path,
                target: entry.target.expect("symlink target"),
            },
            other => panic!("unknown checked snapshot entry kind: {other}"),
        })
        .collect::<Vec<_>>();
    let snapshot = build_deterministic_skill_snapshot(&fixture.skill_name, &entries)
        .expect("checked snapshot builds");
    assert_eq!(
        snapshot.manifest().root_tree().to_string(),
        fixture.root_tree_id
    );
    assert_eq!(snapshot.bytes().len(), fixture.snapshot_size);
    assert_eq!(
        BlobId::hash(snapshot.bytes()).to_string(),
        fixture.snapshot_sha256
    );
}

fn tree_entry_from_fixture(entry: &TreeEntryFixture) -> TreeEntry {
    let kind = match entry.kind.as_str() {
        "file" => TreeEntryKind::File {
            blob: BlobId::from_str(entry.object_id.as_deref().expect("file object ID"))
                .expect("blob ID"),
            executable: entry.executable.expect("file executable bit"),
        },
        "directory" => TreeEntryKind::Directory {
            tree: TreeId::from_str(entry.object_id.as_deref().expect("directory object ID"))
                .expect("tree ID"),
        },
        "symlink" => TreeEntryKind::Symlink {
            target: entry.target.clone().expect("symlink target"),
        },
        other => panic!("unknown checked entry kind: {other}"),
    };
    TreeEntry::new(entry.name.clone(), kind).expect("checked tree entry")
}
