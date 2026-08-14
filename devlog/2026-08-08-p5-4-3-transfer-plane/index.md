# P5.4.3 — M6.7's generation transfer, and two devices in one granule

| Field | Value |
|---|---|
| Date | 2026-08-08 |
| Kind | Change |
| Status | Verified |
| Scope | `slime-root/src/{device,virtio_blk,main,graph}.rs`, `components/bins/src/bin/{sel4-transfer-probe,init}.rs`, `components/bins/{Cargo.toml,build.rs}`, `contracts/generation/v1/fixtures/sel4-transfer.zti`, `scripts/build/{boot_layout,build-generation,build-sel4,build-store-fixture}.py`, `scripts/check/check-sel4-{transfer-plane,boot-layout,gate-controls}.py`, `Justfile`, `roadmap/00-backlog.md` |
| Roadmap | P5.4.3, P5.4, M6.7 |
| Gates | `just sel4_transfer_check`, all 26 seL4 plane gates, `just test_sel4_root`, `just lint_all`, `just fmt_check_all`, `just ruff`, `just typos`, `just devlog_check`, `just contracts_check`, `just generation_check` |
| Trigger | B29 blocked M6.7, the last M6 gap |
| Baseline | `slime-root` brought up one block device; `Resource::Block` named no index |

## Summary

A generation crosses a persistence boundary. Two devices are attached: a
*source* the component may only read, carrying the transfer manifest, and a
*receiver* it may write, holding the BootState. The manifest's digest, its
object closure, and its travel policy are all verified before any write; the
generation stages **pending**, leaving the known-good root intact; and only
health confirmation promotes it.

M6.7 requires that a transfer "leave every ungranted device byte-identical".
The claim here is sharper: the source *is* granted — read-only — and it is
byte-identical after the component reached it repeatedly and tried to write it.

**M6.7 was the last M6 gap.** M6.1 through M6.7 are now gated on seL4, and
`roadmap/00-backlog.md`'s Open section is empty.

## Changes

| Area | Change | Restored invariant |
|---|---|---|
| `device.rs` | `MappedGranule`: a borrowed view carrying a base and no capability | Two transports can share one mapped page |
| `virtio_blk.rs` | The driver holds a borrow, not the region | Mapping ownership stays with the probe |
| `main.rs` | `bring_up_shared_block`, a standing-granule table | Both attached devices come up |
| `main.rs` | Successive block grants name successive devices | A component holding two devices reaches two |
| `main.rs` | Placement intersects **the grant's** rights, not the component's union | A read-only grant is read-only |
| `build-store-fixture.py` | A `transfer` variant carrying a manifest | The gate boots a real record |

### B29: two transports, one granule

QEMU packs eight virtio-mmio transports into one 4 KiB page, so two attached
disks land at `0xa003e00` and `0xa003c00` — the same granule. seL4's retype is
monotonic and `frame_map` takes the frame once, so the second transport had
nothing left to map and was skipped.

The fix is that a second driver does not need another *mapping*, only the same
one at its own offset. `MappedGranule` carries the virtual base and no
capability, so it can neither remap nor unmap; the mapping's lifetime stays with
the `DeviceRegion` the probe holds, which outlives every borrow because a bound
device stays bound for the boot.

Prototyped and reverted in an earlier slice rather than half-landed. Landing it
with its own gate was the right call: it touched three files and exposed two
more defects.

### The two defects it exposed

**Declared placement hardcoded `Block { device: 0 }`.** A component holding two
device grants reached the same device twice. Successive block grants now name
successive devices, in both placement paths.

**Placement intersected the component's *union* of rights.**
`inbound_authority` unions every grant naming a component, which is right for
deciding *whether* it holds a kind and wrong for deciding *how much*. Against
the union, a read-only source came out writable — and accepted the write, which
is exactly the property M6.7 exists to check. Both paths now use the grant's own
rights.

That second one is worth stating plainly: the plane's first run **passed the
milestone's write-refusal arm only by accident of ordering**, and failed it as
soon as the arm was actually reached. A gate that had only checked the transfer
succeeded would have shipped a broken rights model.

## Regression guards

| Risk | Guard | Failure signal |
|---|---|---|
| Only one device comes up again | both must reach `block ready` | "N block devices were brought up" |
| A read-only grant is writable | the probe writes the source and the root must refuse | "the source device accepted a write" |
| The refusal is reported but not honoured | the source image is compared byte for byte | "the read-only source device changed" |
| A tampered manifest installs | the flip is in metadata, so only the digest catches it | "failed on the wrong check" |
| An incomplete closure installs | every object re-hashes before any write | "the closure is incomplete" |
| State the source withheld is shipped | every entry must carry the travel flag | "a state entry that does not travel was shipped" |
| Staging clobbers the known-good root | it is compared after staging | "staging changed the known-good root" |
| Promotion happens without confirmation | pending is asserted before promoted | marker out of order |
| The transfer writes outside BootState | GPT, store region, and the tail are compared | "the transfer modified …" |
| Nothing was transferred at all | the receiver must differ | "the receiver was not written" |
| The gate loses evidence | `just sel4_gate_control_check`, pinned at 12 markers | a mutated transcript is accepted |

## Verification

| Command/scenario | Result | Evidence class |
|---|---|---|
| `just sel4_transfer_check` | Pass; 12 markers, both devices up, source byte-identical | Direct |
| `just sel4_device_check`, `just sel4_storage_check` | Pass; the single-device planes still hold | Direct |
| `just sel4_gate_control_check` | Pass; 26 gates reject 1014 mutated transcripts and layouts | Direct |
| `just sel4_boot_layout_check` | Pass; 23 plane layouts match their fixtures | Direct |
| The other twenty-five seL4 plane gates | Pass | Direct |
| `just test_sel4_root`, `just contracts_check`, `just generation_check` | Pass | Direct |
| `just lint_all`, `just fmt_check_all`, `just ruff`, `just typos`, `just devlog_check` | Pass | Direct |
| A transfer built from two *real* generations | Not covered — see below | — |

## Decisions

- **Decision:** A borrowed granule rather than a second mapping.
  **Rationale:** the frame can be mapped once and the second driver does not
  need a second mapping — it needs the same one at a different offset. Carrying
  no capability is what keeps ownership unambiguous.

- **Decision:** Intersect each grant's own rights, not the component's union.
  **Rationale:** the union answers "does this component hold a device", which is
  a different question from "how much authority does *this* grant carry". Two
  grants of the same kind with different rights is exactly M6.7's shape.

- **Decision:** Encode the fixture's manifest in `build-store-fixture.py` rather
  than reuse `build-transfer.py`.
  **Rationale:** that script builds from a pair of real generations. The
  properties under test — the self-excluding digest, the closure's content
  hashes, the travel flags — need a well-formed record, not a real generation
  behind it. The constants come from the same generated layout either way.

- **Decision:** Compare both images from the host.
  **Rationale:** the serial markers prove the component asked and was refused.
  Only the images prove what reached the devices, and M6.7 is a claim about
  exactly that.

## Open risks and follow-ups

- [ ] The manifest is synthetic. `build-transfer.py` constructs one from two
      real generations with a real release record and a real closure; this
      fixture's release block is zeroed and its state root is a literal. The
      *decoder* is exercised fully, the *producer* is not.
- [ ] Nothing verifies the transferred generation is bootable after promotion —
      the plane installs a BootState naming it and stops. The oracle's
      `transfer_check` boots the receiver again; that would need a second boot
      in the gate.
- [ ] `MAX_BLOCK_DEVICES` is 2, and the standing-granule table is the same size.
      A third device in the same page would be skipped with a marker rather than
      brought up.
- [ ] The oracle's M6.7 also covers a transfer that fails *authorization* rather
      than integrity. This plane covers the tampered-digest path only.

## Artifacts and provenance

- Gate output, both devices' bring-up, the rights refusal, and the image
  comparisons: [`transfer-check.txt`](transfer-check.txt).
- B29's resolved entry: [`roadmap/00-backlog.md`](../../roadmap/00-backlog.md).
- The device substrate this extends:
  [`devlog/2026-08-08-p5-4-2a-device-substrate/`](../2026-08-08-p5-4-2a-device-substrate/index.md).
- The BootState model it installs into:
  [`devlog/2026-08-08-p5-4-2c-rollback-plane/`](../2026-08-08-p5-4-2c-rollback-plane/index.md).
- Related roadmap item: P5.4.3 in
  [`roadmap/07-architecture-portability.md`](../../roadmap/07-architecture-portability.md).
