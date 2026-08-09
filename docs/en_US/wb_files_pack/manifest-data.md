# WaterBall Pack Manifest Data Format Specification

> **Language**: [简体中文](../../zh_CN/wb_files_pack/manifest-data.md) | English

_Created: 2026/02/27 &emsp; Last updated: 2026/06/17_

## 1. Overview

The pack file is logically divided into three layers:

| Layer | Description | Storage location |
|---|---|---|
| File header | Magic number, version, flag bits, embedded attribute | Start of `.pack` |
| Manifest index | `Attribute`, `PackStruct`, `PackFileMetadata`, `DataPosList` | Separate `.wbm` by default, or embedded at the end of `.pack` |
| Data region | Virtual file binary contents | `.pack` data region |

Conventions:

- All multi-byte integers are **little-endian**.
- Boolean bit order is **MSB-first** (bit 0 means `>> 7`).
- Index data is stored in units of **manifest data blocks** (`ManifestDataBlock`), updated atomically via **A/B dual blocks**.

## 2. FileHeader

Occupies the first `FILE_HEADER_BLOCK_LEN` bytes.

### 2.1 Basic Info

| Offset | Bytes | Field | Description |
|---|---|---|---|
| 0 | 11 | `type_name` | Magic number `"WBFilesPack"` (`0x57 0x42 0x46 0x69 0x6c 0x65 0x73 0x50 0x61 0x63 0x6b`) |
| 11 | 2 | `version` | Currently `[0,2]` |
| 13 | 1 | `bool_data` | Flag bits |
| 14 | 8 | `pack_len` | Total pack file length |
| 22 | — | `_padding_` | Padded to 128B |

### 2.2 Flag Bits

| Bit | Mask | Flag | Description |
|---|---|---|---|
| 0 | `0b1000_0000` | `cow` | Copy-on-write |
| 1 | `0b0100_0000` | `s_manifest_file` | Separate `.wbm` manifest |

### 2.3 Embedded Attribute

An `Attribute` manifest data block is embedded at offset 128 (format identical to [5. Attribute](#5-attribute)):

```
FILE_HEADER_BLOCK_LEN = DATA_BLOCK_LEN + MANIFEST_ATTRIBUTE_BLOCK_LEN
```

`MANIFEST_ATTRIBUTE_BLOCK_LEN` is computed from the attribute's actual size per the rules in section 6.

## 3. Manifest Index Overall Structure

The logical blocks are chained together via attribute file pointers:

```
┌─────────────────────────────┐
│  Data free-list DataPosList │ ← empty_data_pos_list_pos points here
├─────────────────────────────┤
│  Manifest free-list DataPosList │ ← manifest_empty_data_pos_list_pos points here (separated mode only)
├─────────────────────────────┤
│  Root struct PackStruct     │ ← root_struct_pos points here
│    ├─ PackStructItem        │
│    │   └─ child PackStruct  │ ← struct_file_pos points here
│    ├─ PackStructItem        │
│    │   └─ PackFileMetadata  │ ← metadata_file_pos points here
│    └─ ...                   │
└─────────────────────────────┘
```

## 4. Common Data Structures

### 4.1 DataPosList

A list of `(position, length)` pairs, used in two scenarios:

| Scenario | Data source | Description |
|---|---|---|
| Free-space tracking | `PackIO.empty_data_list`, pointed to by `attribute.empty_data_pos_list_pos` | Records reusable free regions |
| Allocated-space tracking | `PackFileMetadata.file_type.data_pos_list` | Records the data block positions occupied by a file |

**Serialization:**

| Offset | Bytes | Field | Description |
|---|---|---|---|
| 0 | 8 | `count` | Item count |
| 8 | 16 × count | `items` | Each item is 16B |

**Data position item:**

| Offset | Bytes | Field | Description |
|---|---|---|---|
| 0 | 8 | `pos` | Starting position |
| 8 | 8 | `len` | Length |

Alignment convention: data region allocation/reclamation is aligned to `DATA_BLOCK_LEN` (128B); data blocks are aligned to `DATA_DATA_BLOCK_LEN` (4MiB).

## 5. Attribute

Resident in memory; serialized into a manifest data block.

### 5.1 Serialization (61 bytes)

| Offset | Bytes | Field | Description |
|---|---|---|---|
| 0 | 2 | `version` | `MANIFEST_VERSION` (10) |
| 2 | 2 | `version_compatible` | Minimum compatible version |
| 4 | 1 | `bool_data` | Flag bits |
| 5 | 8 | `empty_data_pos_list_pos` | Data free-list position |
| 13 | 8 | `manifest_empty_data_pos_list_pos` | Manifest free-list position (separated mode only) |
| 21 | 8 | `manifest_file_len` | Manifest file length (valid in separated mode) |
| 29 | 8 | `root_struct_pos` | Root structure position |
| 37 | 8 | `file_count` | Total file count (excluding directories) |
| 45 | 8 | `dir_count` | Directory count |
| 53 | 8 | `data_len` | Total actual size of all file data |

### 5.2 Flag Bits

| Bit | Mask | Flag |
|---|---|---|
| 0 | `0b1000_0000` | `cow` |

### 5.3 Version Compatibility

| Condition | Behavior |
|---|---|
| `== MANIFEST_VERSION` | Fully compatible |
| `< MANIFEST_VERSION_COMPATIBLE` | Rejected (too low) |
| `version_compatible > MANIFEST_VERSION` | Rejected (too high) |
| Otherwise | Attempt compatible read |

## 6. ManifestDataBlock

All index data is stored here; minimum alignment is `DATA_BLOCK_LEN` (128B).

### 6.1 A/B Dual-Block Mechanism

The block is split into equal A/B halves, written alternately:

```
┌─────────── A Block ────────────┬─────────── B Block ────────────┐
│ data_len(8) │ ver(4) │ hash(8) │ data_len(8) │ ver(4) │ hash(8) │
│ actual data (...)              │ actual data (...)              │
│ ver(4) tail                    │ ver(4) tail                    │
└────────────────────────────────┴────────────────────────────────┘
```

- Writes target the half with the **smaller version** (`version + 1`), overwriting it.
- A crash leaves at least one complete block.
- Reads select the block with the **larger version passing validation**.
- The version is stored both in the header and the tail; **header == tail** means the block is complete.

### 6.2 A/B Block Header (20 bytes)

| Offset | Bytes | Field | Description |
|---|---|---|---|
| 0 | 8 | `data_len` | Actual data length |
| 8 | 4 | `ver` | Monotonically increasing |
| 12 | 8 | `hash` | First 8 bytes of BLAKE3 |
| 20 | data_len | `data` | Actual data |
| tail | 4 | `ver` | Same version, stored before the last 4 bytes |

### 6.3 Block Size Calculation

```
A/B half minimum footprint = 8 + 4 + 8 + data_len + 4 = 24 + data_len bytes
Total block size = ceil((24 + data_len) × 2 / DATA_BLOCK_LEN) × DATA_BLOCK_LEN
```

## 7. PackStruct

A directory-tree node containing all child struct items.

### 7.1 Serialization

Struct items are concatenated consecutively:

```
┌───────────────┬───────────────┬───────────────┐
│ Struct item 1 │ Struct item 2 │ ...           │
└───────────────┴───────────────┴───────────────┘
```

Each item carries its own length prefix for traversal; the entire struct data is stored in one manifest data block.

## 8. PackStructItem

### 8.1 Common Fields

| Offset | Bytes | Field | Description |
|---|---|---|---|
| 0 | 8 | `data_len` | Total item length (including itself) |
| 8 | 1 | `type` | 0 = file, 1 = directory |
| 9 | 2 | `name_len` | Name length in bytes |
| 11 | name_len | `name` | UTF-8 |
| 11 + name_len | 8 | `metadata_file_pos` | Metadata position |
| 19 + name_len | — | `type_data` | Type-specific data |

### 8.2 Type 0 — File

`type_data` is empty; total length = `19 + name_len`.

### 8.3 Type 1 — Directory

| Offset | Bytes | Field | Description |
|---|---|---|---|
| 0 | 8 | `struct_file_pos` | Child struct position |

Total length = `27 + name_len`.

## 9. PackFileMetadata

One per virtual file/directory, stored in its own manifest data block.

### 9.1 Common Fields

| Offset | Bytes | Field | Description |
|---|---|---|---|
| 0 | 1 | `type` | 0 = file, 1 = directory |
| 1 | 1 | `bool_data` | Flag bits |
| 2 | 8 | `len` | File = data size; directory = total size of child data |
| 10 | 16 | `modified` | Milliseconds since UNIX epoch, `u128` |
| 26 | — | `type_data` | Type-specific data |

### 9.2 Flag Bits

| Bit | Mask | Flag |
|---|---|---|
| 0 | `0b1000_0000` | `cow` |

### 9.3 Type 0 — File

| Offset | Bytes | Field | Description |
|---|---|---|---|
| 0 | 1 | `hash_type` | 1 = BLAKE3 |
| 1 | 1 | `hash_len` | Currently 32B |
| 2 | hash_len | `hash_value` | Hash value |
| 2 + hash_len | — | `data_pos_list` | Position list (format per [4.1 DataPosList](#41-dataposlist)) |

### 9.4 Type 1 — Directory

| Offset | Bytes | Field | Description |
|---|---|---|---|
| 0 | 8 | `file_count` | Cumulative, including subdirectories |
| 8 | 8 | `dir_count` | Cumulative |

### 9.5 Hash Verification

1. Read the block `hash` field (first 8B of BLAKE3).
2. Recompute BLAKE3 over the data and compare the first 8B.
3. Mismatch → refuse to load.

## Appendix: Rust API Reference

Source: `src/wb_files_pack/`.

### 1) Pack Opening (`std::fs::OpenOptions` style)

| Method | Description |
|---|---|
| `open(path)` | Open an existing pack read-only |
| `create_new(path)` | Default separated manifest, no COW; fails if it exists |
| `options()` | Returns `PackOpenOptions` |

`PackOpenOptions`:

| Method | Default | Description |
|---|---|---|
| `read(true)` | false | Read |
| `write(true)` | false | Write |
| `create(true)` | false | Create |
| `create_new(true)` | false | Create new |
| `cow(true)` | false | Copy-on-write |
| `separate_manifest(true)` | true | Separate manifest |
| `open(path) -> Result<ManagerSync>` | — | Open |

### 2) Virtual File Opening (`std::fs::File` style)

| Method | Description |
|---|---|
| `open_virtual_file(path, end_pos)` | Read-only, like `File::open` |
| `create_virtual_file(path)` | Write-only, like `File::create` |
| `virtual_file_options()` | Returns `VirtualFileOpenOptions` |

`VirtualFileOpenOptions`:

| Method | Default | Description |
|---|---|---|
| `read(true)` | false | Read |
| `write(true)` | false | Write |
| `create_new(true)` | false | Create new |
| `end_pos(bool)` | false | End position |
| `open(sync, path) -> Result<PackVirtualFile>` | — | Open |

### 3) Virtual File Read/Write (`PackVirtualFile`, formerly `PackFileWR`)

Implements `Read`/`Write`/`Seek` with access-mode permission checks:

- Read mode → `Write` returns `PermissionDenied`
- Write mode → `Read` returns `PermissionDenied`
- ReadWrite mode → both allowed

Inherited methods:

| Method | Description |
|---|---|
| `set_len(len)` | Set length |
| `set_modified(ts)` | Set modified timestamp |
| `sync()` | Force flush |

## Appendix: Future Plans

### A. Struct Item Extensions

| Extension | Size | Description |
|---|---|---|
| Struct item hash | 8B | Integrity verification |
| Struct id | 8B | Globally unique |
| Sequence number | 4B | Ordering |

### B. Metadata Extensions

| Extension | Size | Description |
|---|---|---|
| Metadata id | 8B | Deduplicated reference tracking |

### C. Struct Header Extensions

| Extension | Size | Description |
|---|---|---|
| Struct id | 8B | Globally unique |

### D. Symbolic Links

Type value `0x2` is reserved; format TBD.

### E. Full COW Implementation

The `cow` flag is already reserved: on modification, copy the original block instead of overwriting in place; supports snapshot/rollback.
