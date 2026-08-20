# Denju semantic object hash transcript v1

Denju object identities are SHA-256 hashes over semantic byte transcripts. They do not hash JSON, YAML, CBOR, Bincode, tar headers, database rows, filesystem timestamps, owners, groups, or platform-specific permission bits.

All integer fields below are unsigned 32-bit big-endian values. `bytes(value)` means the exact bytes named by the field. UUID fields are their 16 RFC 9562 UUID bytes, not textual UUID strings. SHA-256 object IDs are their 32 raw digest bytes, not hexadecimal text.

## Blob

```text
blob-v1 = SHA256(raw file bytes)
```

There is deliberately no domain prefix for blobs. A blob ID is exactly the ordinary SHA-256 digest of the file bytes.

## Tree

A tree represents exactly one directory level. Sort direct entries by their canonical UTF-8 name bytes in ascending byte order before hashing. Duplicate names are invalid.

```text
tree-v1 = SHA256(
  "denju:tree:v1\0" ||
  u32(entry_count) ||
  repeated(
    u32(entry_payload_length) ||
    entry_payload
  )
)
```

Every entry payload begins with:

```text
u32(name_byte_length) || bytes(name)
```

and then exactly one typed payload:

```text
file      = 0x01 || executable_byte || blob_id
directory = 0x02 || tree_id
symlink   = 0x03 || u32(target_byte_length) || bytes(relative_target)
```

`executable_byte` is `0x00` for non-executable and `0x01` for executable. Valid tree inputs have already passed Denju's portable path and internal-relative-symlink validation.

## Revision

A revision has zero, one, or two parents. Parent IDs are sorted by their 32 raw bytes before hashing, so a two-parent merge has the same identity regardless of observation order. Duplicate parents are invalid.

```text
revision-v1 = SHA256(
  "denju:revision:v1\0" ||
  root_tree_id ||
  u32(parent_count) ||
  repeated(parent_revision_id) ||
  author_principal_uuid_bytes ||
  operation_uuid_bytes
)
```

`author_principal_uuid_bytes` is the immutable `AuthorPrincipalId`; `operation_uuid_bytes` is the immutable `OperationId`. Both are UUIDv7 values in v1.

## Display form

`BlobId`, `TreeId`, and `RevisionId` are displayed as 64 lowercase hexadecimal characters. Hexadecimal text is only a display/transport form and is never fed back into the hash transcript.

## Conformance

`spec/fixtures/object-v1.json` freezes concrete transcripts and expected IDs. `denju-core` tests consume that checked fixture directly so transcript changes are explicit compatibility changes rather than incidental serialization changes.
