# The two docs with update discipline missed the whole I/O track

| Field | Value |
|---|---|
| Date | 2026-08-30 |
| Kind | Audit |
| Status | Verified |
| Scope | `docs/capability-matrix.md`, `docs/syscall-abi.md`, `docs/README.md`, `docs/concepts/{channels,components,contracts,generations}.md`, `docs/getting-started/{03-boot-walkthrough,04-first-change}.md`, `scripts/generate/generate-component-runtime-abi-bindings.py` |
| Roadmap | IO1, IO2, IO4, B83, B90 |
| Gates | `just contracts_check`, `just devlog_check`, `just ruff`, `just typos` |
| Trigger | Asked whether the new I/O substrate needed a new document; audited what the existing reference documents already claimed about it |
| Baseline | `docs/capability-matrix.md` and `docs/syscall-abi.md` each declare in their own opening paragraph that they must change in the same commit as the surface they describe |

## Summary

The question was whether the I/O substrate needed a new document. It does not:
`docs/syscall-abi.md` already carries all ten `IO RESOURCE` labels plus the two
authority-read labels, machine-checked label-for-label, and
`docs/capability-matrix.md` already carries the four I/O capability kinds and
their bounds. What it needed was for those two documents to be *true*. Both
declare an update-in-the-same-commit rule in their first paragraph, and IO0–IO7
plus B83 landed without it: seven claims were false, not merely thin, and one
paragraph contradicted a table four screens above it in the same file. The
underlying cause of the largest cluster is mechanical — the ABI doc's label
coverage is machine-checked, but the checker reads only the *root* service
section, so B83's renumbering of the console table went unnoticed for a month
with every gate green. That gap is now closed by a check that fails on both the
renumbering and the misnaming, demonstrated against the exact historical
mistake.

## Observable symptom

- Command: read `docs/capability-matrix.md` against
  `boot-contracts/src/generation.rs` and `slime-root/src/graph.rs`
- Expected: the matrix's gate-status column agrees with `capability_rights_valid`
  and the `rights_type!` declarations
- Observed: lines 82–85 said bits 4–7 were "gated (IO1)" while lines 124–131 of
  the same file said "No `CapabilityKind` allowed mask admits any of them […]
  and no runtime `rights_type!` can carry one. They are named but ungated."
- Exit/fault/serial evidence: no runtime evidence; this is a documentation audit
  against source. `docs/syscall-abi.md`'s console table listed
  `BLOCK TRANSACT` at label 2 while
  `boot-contracts/src/generated/component_runtime_abi.rs:36-41` declares label 2
  as `DIRECTORY_INSPECT`, and `python3
  scripts/generate/generate-component-runtime-abi-bindings.py --check` reported
  nothing wrong because it made no doc comparison at all.

## Investigation log

| Step | Observation | Consequence |
|---|---|---|
| 1 | `grep` for `io_resource`/`io-queue`/`virtio` across `docs/` returned only four capability-matrix lines and two syscall-abi lines | The I/O surface is present in the reference documents; nothing is missing at the level of a whole document |
| 2 | Capability matrix lines 82–85 and 124–131 make opposite claims about bits 4–7 | The false paragraph is the older one; IO1 updated the table rows and left the prose |
| 3 | `capability_rights_valid` (`boot-contracts/src/generation.rs:117-119`) admits all four bits on the four I/O kinds, and `slime-root/src/graph.rs:97-100` declares the four matching `rights_type!` | Three of the paragraph's four assertions are false |
| 4 | `contracts/generation-manifest/v1/compositions/sel4-storage.zti:16-19` spells `mapMmio`, `irqAck`, `dmaPin`, `dmaRelease` on real grants | The fourth assertion — that no manifest grant may carry them — is false too |
| 5 | The paragraph cites `declared_rights_partition_into_manifest_declarable_and_root_only` as pinning its claim; that test's `BASELINES` enumerates nine kinds and none of the four I/O kinds | The citation is the reason the false claim survived: the test proves a narrower proposition than its name suggests, and it stayed green |
| 6 | `docs/syscall-abi.md` console table lists labels 0–4 with `BLOCK TRANSACT` at 2; the generated `console_labels` module declares 0–3 with `DIRECTORY_INSPECT` at 2 | Three of five rows are wrong: one retired operation, two renumbered |
| 7 | `check_doc` in `generate-syscall-abi-bindings.py:108` partitions on `"## Root service operations"` | The console table is outside every machine check, which is why step 6 survived B83 |
| 8 | `AuthorityTable::resolve_block` has no caller outside its own module's tests, and `service_for_root_label` maps no label to `SERVICE_BLOCK` | The `Block` kind and its two rights bits now have no gate at all — recorded as B90 |
| 9 | `git log -p` on `contracts/component-runtime-abi/v1/schema.zt` shows commit `42782e6` renumbering `DIRECTORY_INSPECT` 3→2 and `DIRECTORY_COMMIT` 4→3 | The console numbering is *not* frozen the way root labels are; the doc now says so explicitly rather than leaving a reader to assume the frozen-label rule applies |
| 10 | `grep components/bins docs/` returns seven hits across five files | CP3's crate-per-component split left every `docs/` path stale |

## Changes

| Area | Change | Restored invariant |
|---|---|---|
| `docs/capability-matrix.md` | Replaced the "bits 4–7 are named but ungated" paragraph with the current fact: bits 4–7 are admitted, typed, checked, and manifest-declared, naming the three source sites. Bits 12–15 are separated out as the four that genuinely remain ungated | A row's gate status matches the code that enforces it |
| `docs/capability-matrix.md` | Rewrote the `declared_rights_partition…` paragraph to state what the test actually covers: nine kinds, so its `NOT_MANIFEST_DECLARABLE` list means "rejected for those nine". Added the requirement that a new kind join `BASELINES` in the same change | A cited test proves the proposition it is cited for |
| `docs/capability-matrix.md` | `Block` kind and both `BLOCK_*` rows now state that no root operation resolves them, why the bits still decode (the retained `x86_64-qemu-virtio` identity), and where the surviving per-ring gate lives | Grammar rule 2's violation is visible instead of disguised as `gated (M5.3)` |
| `docs/capability-matrix.md` | `MAP_MMIO` row renamed to `Device / MmioRegion` and each I/O row now names the operations that check it; two Bounds rows added for the IO0 ring geometry and the per-ring block authority | The gated-operation column names operations rather than paraphrasing |
| `docs/capability-matrix.md` | Two Horizon rows corrected: generation management reaches storage over a ring, and IO4 answered `NetworkDestination`'s shape question without a capability kind | The horizon does not list as open a question a shipped milestone answered |
| `docs/syscall-abi.md` | Console table corrected to labels 0–3, with an explicit note that B83 retired label 2's occupant and renumbered the two directory operations, and that the console numbering is separate from the root's | A component author reading the table reaches the operation they mean |
| `docs/syscall-abi.md` | Root-served mechanism list and endpoint table drop "blocks" and add clock and I/O resource; label 64's row moved into numeric order | The endpoint table describes what each thread serves |
| `docs/syscall-abi.md` | Service admission section gains ids `10` clock and `11` IO resource, corrects `service_for_root_label`'s file to `slime-root/src/ipc.rs`, states that required services are *derived* from held authority in either direction, and records that id `8` is derived and admitted but reachable by no label | The list of gates a caller must pass is complete |
| `docs/syscall-abi.md` | Error-model note replaced: `BLOCK TRANSACT`'s convention is gone, and the I/O identity operations are documented as answering nonzero ids (both mapping counters start at 1) | No convention is described for a deleted operation |
| `scripts/generate/generate-component-runtime-abi-bindings.py` | Added `declared_console_labels` + `check_doc`: reads the generated `console_labels` module and compares label *and* name against the doc's console section, refusing missing, extra, and misnamed rows | The console table is machine-checked, so this drift cannot recur silently |
| `docs/README.md`, `docs/concepts/channels.md` | Routing row for driver hardware authority and the I/O gates; channels' root-served list corrected, with an explicit paragraph on why storage is not in it, and rings added to Related | A reader looking for the I/O substrate finds it from the index |
| `docs/concepts/{components,contracts,generations}.md`, `docs/getting-started/{03,04}` | Seven `components/bins/...` paths corrected to their CP3 locations; the generations page no longer says management holds block capabilities | Every path in `docs/` resolves |
| `roadmap/00-backlog.md` | Opened B90 for the ungated `Block` kind, with the two clean-cutover options and the rollback-window constraint on deleting the wire discriminant | A defect found by this audit is tracked rather than only described |

## Verification

| Command/scenario | Result | Evidence class |
|---|---|---|
| `python3 scripts/generate/generate-component-runtime-abi-bindings.py --check` | `docs/syscall-abi.md documents all 4 console operations` / `Component runtime ABI bindings are current` | Direct |
| Renumber the doc's `DIRECTORY INSPECT` row 2→3, re-run the check | Fails: `does not document console operations: 2 (DIRECTORY_INSPECT)`, exit 1 | Direct |
| Restore label 2's name to `BLOCK TRANSACT` — the exact pre-audit text — and re-run | Fails: `misnames console operations: 2: contract DIRECTORY_INSPECT vs doc BLOCK_TRANSACT`, exit 1 | Direct |
| `just contracts_check` | See "Artifacts and provenance" | Direct |
| `just devlog_check` | See "Artifacts and provenance" | Direct |
| `just ruff`, `just typos` | See "Artifacts and provenance" | Direct |
| Runtime behaviour of the root or any component | Not exercised; no Rust source changed, so no plane gate was run | Not applicable |

## Open risks and follow-ups

- [x] B90: resolved 2026-08-30 by deleting the `Block` kind, both rights bits,
      `SERVICE_BLOCK`, and the root-side machinery, and by completing
      `declared_rights_partition_into_manifest_declarable_and_root_only`'s
      `BASELINES` to all twelve declared kinds. The research also disproved this
      entry's assumption that the rollback window forced the decode-only option.
      See [`devlog/2026-08-30-b90-block-kind-retired/`](../2026-08-30-b90-block-kind-retired/index.md).
- [ ] `docs/capability-matrix.md`'s Bounds and gate-status columns are still
      prose checked only by review. The rights *vocabulary* is generated, but
      no gate compares the matrix's gate-status column against the set of rights
      some operation actually checks — which is precisely the class of error this
      audit found by hand. A check that extracts `rights.allows(RIGHT_*)` and
      `rights_type!` sites and compares them to the table would have caught it.
- [ ] `docs/` paths are unchecked. Seven were stale for the nine days since CP3.
      A link/path check over `docs/` is cheap and would have caught all seven.

## Artifacts and provenance

- Focused report: this entry; there is no separate report.
- Raw transcript: none retained; every observation is a `grep`/`read` against
  the tree at `2045442` and is reproducible from the file:line citations in the
  investigation log.
- Serial/debugger/model output: none. No image was built and no plane was
  booted, because no Rust source changed.
- Related roadmap item: `roadmap/11-io-substrate.md` (IO1, IO2, IO4),
  `roadmap/00-backlog.md` (B83 resolved, B90 opened).
