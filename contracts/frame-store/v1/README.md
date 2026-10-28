# Frame store format 1

A frame store holds the recorded messages of one ROS 2 topic. Each message keeps its CDR bytes as
recorded and its original log time. `schema.zt` is the normative layout. It states the constants,
the header and index entry layouts, the refusal classes and the order a reader validates in. A host
converter writes a store from an MCAP bag. A no_std reader validates the whole store before it
exposes any frame.

This directory holds no renderer yet. Work item `01a18064-a82b-70cb-bb32-527657addf86` adds the
Rust and Python bindings, the reader and the converter. `just frame_store_check` judges them.

## Layout

All integers are little-endian. Every section starts at an offset that is a multiple of 8.

| Bytes | Content |
| --- | --- |
| `[0, 64)` | Header |
| `[64, textEnd)` | Topic, type name and ros2msg schema text, in that order, without terminators |
| `[textEnd, indexStart)` | Zero bytes up to the next multiple of 8 |
| `[indexStart, dataStart)` | `frameCount` index entries of 24 bytes each |
| `[dataStart, storeEnd)` | Payloads in index order, each followed by zero bytes up to the next multiple of 8 |

Header:

| Offset | Size | Field | Rule |
| --- | --- | --- | --- |
| 0 | 4 | `magic` | `SLFS` (`1397115987`) |
| 4 | 2 | `version` | `1` |
| 6 | 1 | `hashVersion` | `1`, meaning RIHS01 |
| 7 | 1 | `reserved0` | zero |
| 8 | 4 | `frameCount` | 1 to 4096 |
| 12 | 2 | `topicBytes` | 1 to 256 |
| 14 | 2 | `typeNameBytes` | 1 to 256 |
| 16 | 4 | `schemaBytes` | 1 to 65536 |
| 20 | 4 | `reserved1` | zero |
| 24 | 8 | `framePeriodNs` | nonzero; the period a publisher replays at |
| 32 | 32 | `typeHash` | the RIHS01 digest; its text form is `RIHS01_` and 64 lowercase hex digits |

Index entry:

| Offset | Size | Field | Rule |
| --- | --- | --- | --- |
| 0 | 8 | `payloadOffset` | from the start of the store |
| 8 | 4 | `payloadBytes` | at most 16 MiB; zero is allowed |
| 12 | 4 | `reserved` | zero |
| 16 | 8 | `logTimeNs` | the MCAP log time; never below the previous entry's |

The layout is canonical. Payload 0 starts at `dataStart`. Each later payload starts at the aligned
end of the one before it. The store ends at the aligned end of the last payload and is at most
128 MiB. One content has exactly one encoding.

## Refusal

A reader applies the checks in the order `schema.zt` lists them. It refuses the store with the class
of the first check that fails. The classes are `store-bound`, `truncated`, `magic`, `version`,
`header`, `frame-count`, `payload-bound`, `order`, `range`, `overlap`, `padding` and `trailing`.
The order makes the class of a malformed store deterministic, so the exam can compare a reader's
refusal with its own by name.

## Compatibility

- A reader refuses any `version` other than 1.
- A field does not change meaning within format 1.
- A writer writes every reserved field and every padding byte as zero, and a reader refuses a
  nonzero one. A new field or a new use of a reserved byte needs a new format version.
- The message encoding is always CDR and the schema encoding is always ros2msg. A store for another
  encoding needs a new format version.

## Validation

`just frame_store_check` runs `scripts/check/check-frame-store.py`. Its expectation is
`scripts/lib/frame_store_exam.py`, with `scripts/lib/mcap_format.py` as its own MCAP reader. It
binds the implementation through three interfaces:

| Interface | Contract |
| --- | --- |
| Converter | `SLIME_LIDAR_BAG=<bag> python3 tools/frame-store/convert-mcap.py --sha256 <hex> --channel <topic> --output <store>`. It refuses a bag by exiting nonzero with `convert-mcap: refused <class>` as its last stderr line, writing no store. The classes are `bag-hash`, `compressed-chunk` and `channel`. |
| Reader | `cargo test -p slime-components --no-default-features -- frame_store::exam`. For each `<case> <path>` line of the manifest that `SLIME_FRAME_STORE_CORPUS` names, it writes the reader's verdict to the file that `SLIME_FRAME_STORE_TRANSCRIPT` names. The line grammar is in the checker's docstring. |
| Fixture | `components/lib/src/frame_store/fixture.bin`: at most 4096 bytes, at least 2 frames, at least one payload whose length is not a multiple of 8. |

The LiDAR bag is a host artifact. It is never committed. The exam refuses a file whose SHA-256 is not
`9ddece1e03367bd7c5cf1b6f9b977be35b97f00a36dfb4d55990429f0e1558ad`.
