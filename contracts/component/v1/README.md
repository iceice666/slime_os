# Component image format 1

This directory defines format 1, the original executable encoding for Slime OS
components. It is **retained, not current**: format 2
(`../v2/schema.zt`) supersedes it and is what
`just component_gen` renders into `components/proto/src/component.rs`. A format-1
image is not an architecture-neutral image missing some fields — it is an
`x86_64-qemu-virtio` image whose target was implied by the only builder that
could produce it. Retained decoders preserve that identity; it does not make
the image executable on a seL4 profile. `schema.zt` remains the normative
layout for those bytes and is validated by `just contracts_check`.

Format 2 keeps every segment rule below unchanged and adds the
target-qualification header fields (`architecture`, `abi`, `page_profile`,
`required_features`). The seL4 product path uses format 2's ELF-carrying
revision (`SLIMECME`), where the same 56-byte qualification header wraps a
complete native ELF that `slime-root/src/child_vspace.rs` loads at its own link
addresses; the segment table below is not used there.

A component image is the only executable encoding admitted as a generation
object of kind `bootstrap` or `component`. Under format 1 and format 2's
segment revision, ELF is build-time scaffolding only: it keeps the full
toolchain (compiler, linker, DWARF debug info, `addr2line`/`gdb`
symbolization) on the host where it belongs. Format 2's ELF revision carries
the executable whole instead, because `slime-root` has a real ELF loader.

## Layering

Format 1 is deliberately structural. Three concerns that executable formats
traditionally conflate are kept in separate layers:

- **Integrity** lives in the generation object digest (sha256 in the
  `SLIMEGEN` object record). Images carry no hash of their own.
- **Authority** lives in generation grants. Images declare no capabilities,
  permissions, or ambient requirements; a component image is pure code plus
  memory requirements and cannot mint authority by existing.
- **Mapping** is what remains, and it is all this format describes: where
  each segment goes, what rights its pages get, where execution starts, and
  how much stack the task needs.

Linking is fully static and resolved at build time. Under format 1 every
component links at the fixed component base VA (`0x400000`); each component runs
in its own address space, so a single fixed base serves all components and there
are no relocations, no PIC requirement, and no load-time fixups. The loader is:
validate header, copy bytes, map pages, jump.

## Layout

All fields are packed little-endian. An image is a header, a segment table,
and concatenated segment payloads.

Header (32 bytes):

| offset | size | field | rule |
| --- | --- | --- | --- |
| 0 | 8 | magic | `IMAGE_MAGIC` ("SLIMECMP" as little-endian u64) |
| 8 | 4 | format_version | exactly 1 |
| 12 | 4 | header_size | exactly 32 |
| 16 | 4 | kernel_abi | must equal the kernel's ABI version |
| 20 | 4 | entry_offset | relative to the component base VA; must land inside an executable segment |
| 24 | 2 | segment_count | 1..=16 |
| 26 | 2 | reserved | written as 0 |
| 28 | 4 | stack_bytes | positive page multiple, <= 1 MiB |

Segment record (20 bytes), sorted by strictly increasing `vaddr_offset` with
non-overlapping memory ranges:

| offset | size | field | rule |
| --- | --- | --- | --- |
| 0 | 4 | vaddr_offset | page-aligned, relative to the component base VA |
| 4 | 4 | mem_len | > 0, >= file_len; the tail beyond file_len zero-fills (`.bss`) |
| 8 | 4 | file_offset | relative to the start of the image data region |
| 12 | 4 | file_len | file_offset + file_len must stay inside the image |
| 16 | 2 | flags | bit 0 write, bit 1 execute; never both, no other bits |
| 18 | 2 | reserved | written as 0 |

Bounds: at most 16 segments; summed page footprint at most 16 MiB; stack at
most 1 MiB. Every bound is checked with overflow-safe arithmetic before any
byte is mapped.

## Validation

[`boot-contracts/src/component_image.rs`](../../../boot-contracts/src/component_image.rs)
classifies retained images with their original target identity and validates
header and segment-table constraints. Its `admit_elf` entry point accepts only
the ELF-carrying revision, not format 1. Retained format decoding is not a
promise that the current product loader executes old images.

`just test_host` covers the shared decoder; `just contracts_check` continues
to type-check this retained schema and generator.

## Build pipeline

The current [component, system, and image reference](../../../docs/architecture/component-system-image.md)
owns the build/admission flow and its format-2 ELF payload. The
[original format note](https://git.justaslime.dev/iceice666/slime_os-history/src/commit/45ed1745907b2d0a13fdf70c8b34eb635bed5f23/contracts/component/v1/README.md)
preserves the segment-image conversion pipeline, historical validation account,
and build-time defaults. They are not instructions for today's seL4 build.

## Compatibility rules

- Readers reject unknown `format_version` and `kernel_abi` values.
- Existing fields do not change meaning within format 1.
- New required fields, new flag bits consumed by readers, or layout changes
  require a new format version; old kernels reject them structurally.
- Reserved fields are written as zero. The current retained-header decoder
  rejects nonzero reserved header fields; old reader behavior is recorded in
  the archived format note and must not be inferred as current acceptance.
- The syscall ABI version is bumped independently of the format version; an
  image built against a newer ABI is rejected by older kernels even when its
  layout is valid.

## Non-goals

- No relocations, dynamic linking, GOT/PLT, or runtime symbol resolution.
- No capability, permission, or authority declarations in the image.
- No embedded integrity hash or signature (generation digest covers it).
- No debug information in the image; the host-side ELF keeps DWARF. A future
  generation object kind may carry debug data explicitly, unread by the
  kernel.
- No per-image provenance or credential metadata (Tock-style footers) until
  a supply-chain threat model calls for it.
