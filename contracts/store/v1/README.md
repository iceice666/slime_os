# Object store protocol format 1

This directory defines the request/reply protocol between userspace
components and the kernel object store service (M5.4). `schema.zt` is the
normative layout; `gen_rust.zt` renders the kernel Rust bindings
(`kernel/src/store_proto/gen.rs`) and the identical no_std-compatible Rust
bindings the `components/` workspace depends on
(`components/proto/src/store.rs`). Regenerate with `just store_gen` and
validate freshness with `just contracts_check`.

The protocol transports operation envelopes only. Object payloads move
through caller-supplied buffers the syscall gate validates per operation;
the store format itself (superblocks, records, content hashes) lives in
`kernel/src/object_store.rs`, and partition selection is bounded by GPT
validation in `kernel/src/gpt.rs`. Authority is the `ObjectStore`
capability: `RIGHT_STORE_READ` gates `OP_STAT`/`OP_GET`, `RIGHT_STORE_WRITE`
gates `OP_PUT` (see `docs/capability-matrix.md`).

## Layout

All fields are packed little-endian. Requests and replies each fit one IPC
message (64 bytes). A 32-byte content hash travels as four u64 fields
(`hash0`..`hash3`, little-endian chunks) because layouts carry scalars only.

Request (60 bytes used):

| offset | size | field | rule |
| --- | --- | --- | --- |
| 0 | 4 | magic | `STORE_MAGIC` ("SLST") |
| 4 | 4 | version | exactly `FORMAT_VERSION` (1) |
| 8 | 1 | op | `OP_STAT` (1), `OP_GET` (2), or `OP_PUT` (3) |
| 9 | 1 | flags | written as 0; nonzero is rejected |
| 10 | 2 | reserved | written as 0 |
| 12 | 8 | buffer_addr | payload buffer; 0 when the op carries no payload |
| 20 | 4 | obj_type | `OP_PUT` only; 0 otherwise |
| 24 | 4 | payload_len | `OP_PUT`: payload bytes; `OP_GET`: buffer capacity; at most 32768 |
| 28 | 32 | hash0..hash3 | content identity (`OP_STAT`/`OP_GET`); ignored by `OP_PUT` |

Reply (52 bytes used):

| offset | size | field | rule |
| --- | --- | --- | --- |
| 0 | 4 | magic | `STORE_MAGIC` |
| 4 | 4 | version | exactly 1 |
| 8 | 4 | status | 0 on success, negative `STORE_E_*` on error |
| 12 | 4 | obj_type | `OP_STAT`/`OP_GET` success |
| 16 | 4 | payload_len | actual object bytes; on `STORE_E_BUFFER_TOO_SMALL`, the capacity needed |
| 20 | 32 | hash0..hash3 | `OP_PUT`: stored identity; `OP_GET`: verified identity |

## Operations

- `OP_STAT(hash)`: report an object's type and length without device
  contact. `STORE_E_NOT_FOUND` when absent.
- `OP_GET(hash, buffer)`: copy the object payload into the buffer after the
  complete payload hash re-verifies on the kernel side. A corrupted object
  yields `STORE_E_CORRUPT`, never unverified bytes.
- `OP_PUT(type, buffer, len)`: append and seal a new object. Identical
  content already present is an idempotent no-op returning the existing
  identity; the same identity with different payload bytes is
  `STORE_E_CONFLICT`. Commit order is record sectors, flush, superblock into
  the older slot, flush, so interruption preserves the previously committed
  root.

## Error codes

`STORE_E_OK` 0, `STORE_E_BAD_MAGIC` -1, `STORE_E_BAD_OP` -2,
`STORE_E_NOT_FOUND` -3, `STORE_E_BUFFER_TOO_SMALL` -4, `STORE_E_FULL` -5,
`STORE_E_DEVICE` -6, `STORE_E_CORRUPT` -7, `STORE_E_CONFLICT` -8,
`STORE_E_TIMEOUT` -9. Structural rejections (bad magic, unknown version,
unknown op, nonzero flags, oversized payload, payload fields on `OP_STAT`)
are enforced before any capability or device contact.

## Compatibility rules

- Readers reject unknown `version` values.
- Existing fields do not change meaning within format 1.
- New required fields or new ops require a new format version; old readers
  reject them structurally.
- Reserved and flags fields are written as zero; readers reject nonzero
  flags, so any future flag use needs a new format version.

## Validation

`scripts/check-contracts.py` checks both schemas and verifies the generated
bindings are fresh. `kernel/tests/object_store.rs` pins protocol and store-format
acceptance/rejection classes; `just storage_store_check` exercises the
complete QEMU path (GPT recovery, content-addressed retrieval, append/seal
durability, and malformed-metadata rejection).
