use std::collections::{BTreeMap, BTreeSet};

use similar::{DiffTag, TextDiff};

use crate::OwnedSkillEntry;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MergeConflictKind {
    AddAdd,
    DeleteModify,
    TypeOrMetadata,
    BinaryContent,
    TextOverlap,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MergeConflict {
    pub path: String,
    pub kind: MergeConflictKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SkillMergeResult {
    Clean { entries: Vec<OwnedSkillEntry> },
    Conflicted { conflicts: Vec<MergeConflict> },
}

/// Deterministically three-way merges two complete skill trees that share `base`.
///
/// The algorithm is intentionally symmetric in `head_a`/`head_b`: callers observing the same
/// three immutable trees produce identical clean bytes regardless of which device lost the CAS
/// race. Structural changes are merged atomically per portable path; simultaneous UTF-8 file
/// edits receive a line-oriented three-way merge when their changed base regions do not overlap.
pub fn merge_skill_entries(
    base: &[OwnedSkillEntry],
    head_a: &[OwnedSkillEntry],
    head_b: &[OwnedSkillEntry],
) -> SkillMergeResult {
    let base = by_path(base);
    let head_a = by_path(head_a);
    let head_b = by_path(head_b);
    let paths = base
        .keys()
        .chain(head_a.keys())
        .chain(head_b.keys())
        .cloned()
        .collect::<BTreeSet<_>>();
    let mut entries = Vec::new();
    let mut conflicts = Vec::new();

    for path in paths {
        let base_entry = base.get(&path).copied();
        let a = head_a.get(&path).copied();
        let b = head_b.get(&path).copied();
        if a == b {
            if let Some(entry) = a {
                entries.push(entry.clone());
            }
            continue;
        }
        if a == base_entry {
            if let Some(entry) = b {
                entries.push(entry.clone());
            }
            continue;
        }
        if b == base_entry {
            if let Some(entry) = a {
                entries.push(entry.clone());
            }
            continue;
        }

        match merge_changed_path(base_entry, a, b) {
            Ok(Some(entry)) => entries.push(entry),
            Ok(None) => {}
            Err(kind) => conflicts.push(MergeConflict {
                path: path.clone(),
                kind,
            }),
        }
    }

    if conflicts.is_empty() {
        entries.sort_by(|left, right| left.path().as_bytes().cmp(right.path().as_bytes()));
        SkillMergeResult::Clean { entries }
    } else {
        SkillMergeResult::Conflicted { conflicts }
    }
}

fn by_path(entries: &[OwnedSkillEntry]) -> BTreeMap<String, &OwnedSkillEntry> {
    entries
        .iter()
        .map(|entry| (entry.path().to_owned(), entry))
        .collect()
}

fn merge_changed_path(
    base: Option<&OwnedSkillEntry>,
    a: Option<&OwnedSkillEntry>,
    b: Option<&OwnedSkillEntry>,
) -> Result<Option<OwnedSkillEntry>, MergeConflictKind> {
    let (Some(base), Some(a), Some(b)) = (base, a, b) else {
        return Err(if base.is_none() {
            MergeConflictKind::AddAdd
        } else {
            MergeConflictKind::DeleteModify
        });
    };
    let (
        OwnedSkillEntry::File {
            path,
            bytes: base_bytes,
            executable: base_executable,
        },
        OwnedSkillEntry::File {
            bytes: a_bytes,
            executable: a_executable,
            ..
        },
        OwnedSkillEntry::File {
            bytes: b_bytes,
            executable: b_executable,
            ..
        },
    ) = (base, a, b)
    else {
        return Err(MergeConflictKind::TypeOrMetadata);
    };

    let executable = merge_scalar(*base_executable, *a_executable, *b_executable)
        .ok_or(MergeConflictKind::TypeOrMetadata)?;
    let bytes = merge_file_bytes(base_bytes, a_bytes, b_bytes)?;
    Ok(Some(OwnedSkillEntry::File {
        path: path.clone(),
        bytes,
        executable,
    }))
}

fn merge_scalar<T: Copy + Eq>(base: T, a: T, b: T) -> Option<T> {
    if a == b {
        Some(a)
    } else if a == base {
        Some(b)
    } else if b == base {
        Some(a)
    } else {
        None
    }
}

fn merge_file_bytes(base: &[u8], a: &[u8], b: &[u8]) -> Result<Vec<u8>, MergeConflictKind> {
    if a == b {
        return Ok(a.to_vec());
    }
    if a == base {
        return Ok(b.to_vec());
    }
    if b == base {
        return Ok(a.to_vec());
    }
    let base = std::str::from_utf8(base).map_err(|_| MergeConflictKind::BinaryContent)?;
    let a = std::str::from_utf8(a).map_err(|_| MergeConflictKind::BinaryContent)?;
    let b = std::str::from_utf8(b).map_err(|_| MergeConflictKind::BinaryContent)?;
    merge_text(base, a, b).ok_or(MergeConflictKind::TextOverlap)
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct TextEdit {
    start: usize,
    end: usize,
    replacement: Vec<String>,
}

fn merge_text(base: &str, a: &str, b: &str) -> Option<Vec<u8>> {
    let base_lines = lines(base);
    let a_edits = text_edits(&base_lines, &lines(a));
    let b_edits = text_edits(&base_lines, &lines(b));
    let mut edits = a_edits;

    for candidate in b_edits {
        if edits.contains(&candidate) {
            continue;
        }
        if edits
            .iter()
            .any(|existing| edits_overlap(existing, &candidate))
        {
            return None;
        }
        edits.push(candidate);
    }
    edits.sort_by(|left, right| {
        (left.start, left.end, &left.replacement).cmp(&(right.start, right.end, &right.replacement))
    });

    let mut output = String::new();
    let mut cursor = 0;
    for edit in edits {
        if edit.start < cursor {
            return None;
        }
        for line in &base_lines[cursor..edit.start] {
            output.push_str(line);
        }
        for line in &edit.replacement {
            output.push_str(line);
        }
        cursor = edit.end;
    }
    for line in &base_lines[cursor..] {
        output.push_str(line);
    }
    Some(output.into_bytes())
}

fn lines(value: &str) -> Vec<&str> {
    value.split_inclusive('\n').collect()
}

fn text_edits(base: &[&str], variant: &[&str]) -> Vec<TextEdit> {
    let diff = TextDiff::from_slices(base, variant);
    let mut edits = Vec::<TextEdit>::new();
    for op in diff.ops() {
        if op.tag() == DiffTag::Equal {
            continue;
        }
        let old = op.old_range();
        let new = op.new_range();
        let edit = TextEdit {
            start: old.start,
            end: old.end,
            replacement: variant[new].iter().map(|line| (*line).to_owned()).collect(),
        };
        if let Some(last) = edits.last_mut()
            && last.end == edit.start
        {
            last.end = edit.end;
            last.replacement.extend(edit.replacement);
        } else {
            edits.push(edit);
        }
    }
    edits
}

fn edits_overlap(a: &TextEdit, b: &TextEdit) -> bool {
    let a_insert = a.start == a.end;
    let b_insert = b.start == b.end;
    match (a_insert, b_insert) {
        (true, true) => a.start == b.start,
        (true, false) => a.start >= b.start && a.start < b.end,
        (false, true) => b.start >= a.start && b.start < a.end,
        (false, false) => a.start < b.end && b.start < a.end,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(path: &str, bytes: &str) -> OwnedSkillEntry {
        OwnedSkillEntry::File {
            path: path.to_owned(),
            bytes: bytes.as_bytes().to_vec(),
            executable: false,
        }
    }

    fn bytes(entries: &[OwnedSkillEntry], path: &str) -> String {
        let OwnedSkillEntry::File { bytes, .. } =
            entries.iter().find(|entry| entry.path() == path).unwrap()
        else {
            panic!("expected file")
        };
        String::from_utf8(bytes.clone()).unwrap()
    }

    #[test]
    fn different_file_changes_merge_symmetrically() {
        let base = vec![file("a.txt", "a0\n"), file("b.txt", "b0\n")];
        let a = vec![file("a.txt", "a1\n"), file("b.txt", "b0\n")];
        let b = vec![file("a.txt", "a0\n"), file("b.txt", "b1\n")];
        let first = merge_skill_entries(&base, &a, &b);
        let second = merge_skill_entries(&base, &b, &a);
        assert_eq!(first, second);
        let SkillMergeResult::Clean { entries } = first else {
            panic!("expected clean merge")
        };
        assert_eq!(bytes(&entries, "a.txt"), "a1\n");
        assert_eq!(bytes(&entries, "b.txt"), "b1\n");
    }

    #[test]
    fn non_overlapping_same_file_regions_merge() {
        let base = vec![file("notes.txt", "one\ntwo\nthree\nfour\n")];
        let a = vec![file("notes.txt", "ONE\ntwo\nthree\nfour\n")];
        let b = vec![file("notes.txt", "one\ntwo\nTHREE\nfour\n")];
        let SkillMergeResult::Clean { entries } = merge_skill_entries(&base, &a, &b) else {
            panic!("expected clean merge")
        };
        assert_eq!(bytes(&entries, "notes.txt"), "ONE\ntwo\nTHREE\nfour\n");
    }

    #[test]
    fn overlapping_same_file_regions_conflict() {
        let base = vec![file("notes.txt", "one\ntwo\nthree\n")];
        let a = vec![file("notes.txt", "one\nTWO-A\nthree\n")];
        let b = vec![file("notes.txt", "one\nTWO-B\nthree\n")];
        let SkillMergeResult::Conflicted { conflicts } = merge_skill_entries(&base, &a, &b) else {
            panic!("expected conflict")
        };
        assert_eq!(
            conflicts,
            vec![MergeConflict {
                path: "notes.txt".to_owned(),
                kind: MergeConflictKind::TextOverlap,
            }]
        );
    }

    #[test]
    fn delete_modify_conflicts_without_choosing_a_side() {
        let base = vec![file("notes.txt", "base\n")];
        let a = Vec::new();
        let b = vec![file("notes.txt", "edited\n")];
        assert!(matches!(
            merge_skill_entries(&base, &a, &b),
            SkillMergeResult::Conflicted { .. }
        ));
    }
}
