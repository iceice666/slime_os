# P5.4.8 — C8.8 filtered introspection and declared interposition on seL4

| Field | Value |
|---|---|
| Date | 2026-08-08 |
| Kind | Change |
| Status | Verified |
| Scope | `contracts/generation/v1/fixtures/sel4-visibility.{zti,md}`, `contracts/boot-layout/v1/fixtures/sel4-visibility.layout`, `slime-root/src/main.rs`, `scripts/build/{boot_layout,build-generation,build-sel4}.py`, `scripts/check/check-sel4-{visibility-plane,boot-layout,gate-controls}.py`, `components/bins/build.rs`, `components/bins/src/bin/init.rs`, `Justfile` |
| Roadmap | P5.4.8, P5.4, C8.8 |
| Gates | `just sel4_visibility_check`, `just sel4_boot_layout_check`, `just sel4_gate_control_check`, `just test_sel4_root`, `just lint_all`, `just fmt_check_all`, `just ruff`, `just typos`, `just devlog_check` |
| Trigger | P5.4.1 recorded C8.8 as uncovered on seL4; P5.4.7 closed the slice before it |
| Baseline | Twelve seL4 plane gates, none asserting a visibility or interposition property |

## Summary

C8.8 now has an observed seL4 equivalent. A thirteenth image, `sel4-visibility`,
boots generation 21: the stream graph plus one declared interposition, with
`fabric-intruder` as the telemetry subscriber's proxy. The broker and all five
participants are the oracle's binaries unmodified. `just sel4_visibility_check`
asserts 25 markers across seven causal chains and re-derives the oracle's two
structural claims — exactly twelve serialized view records and exactly two
distinct interposition traces.

Building it surfaced a real root defect that no earlier plane could reach:
`DebugWrite` read its payload through the 64-byte *message* reader, so every
128-hex-character view and trace record was refused and vanished from the
transcript. Fixed at the source.

## Changes

| Area | Change | Restored invariant |
|---|---|---|
| `slime-root/src/main.rs` | `Operation::DebugWrite` reads with `transfer_window::read_staged_array` (1 KiB) instead of `read_staged` (64 bytes) | A diagnostic line is bounded by the staging window, not by the channel-message bound |
| `contracts/generation/v1/fixtures/sel4-visibility.zti` | Generation 21: seven components, thirteen grants, the two stream routes, and a `sel4` profile carrying the telemetry→`fabric-intruder` chain | A seL4 plane declares its own graph and its own profile |
| `contracts/generation/v1/fixtures/sel4-visibility.md` | Records only what differs from the stream fixture: the profile-borne chain, the single-capability spawns, the `ingressSources` derivation | The fixture's choices are reviewable |
| `scripts/build/boot_layout.py` | `SEL4_VISIBILITY_LAYOUT`, eight rows, registered as the generation-21 replacement | B10: the declared table is the filled table |
| `scripts/build/build-generation.py` | `sel4-visibility` in `SEL4_MANIFESTS`; its flag row sets `SLIME_SEL4_VISIBILITY_CHECK` **and** `SLIME_FABRIC_VISIBILITY_CHECK`; scrub/forward for the new flag | The broker and participants stay byte-identical with the x86 plane |
| `scripts/build/build-sel4.py` | `--visibility-plane`, variant `visibility`, `root-visibility` target dir | Each gate boots the artifact it asserts about |
| `components/bins/build.rs`, `components/bins/src/bin/init.rs` | Flag forwarding; `drive_visibility_plane`; the oracle branch now requires the seL4 flag to be absent | Generation 21 cannot walk generation 16's layout |
| `scripts/check/check-sel4-visibility-plane.py`, `Justfile` | The gate and `just sel4_visibility_check` | C8.8 has a standing seL4 assertion |
| `scripts/check/check-sel4-{boot-layout,gate-controls}.py` | Plane registered in both registries | The new gate is itself guarded |

### The composition

`init` mints five authenticated control pairs, spawns the fabric with the service
halves in `FABRIC_STREAM_CONTROL_GRANTS` order, and gives each participant
**exactly one capability: its own control endpoint**.

That is stronger than the call and operation planes need, and it is the point.
The visibility broker mints every route half itself and hands out narrowed,
non-delegable roles at provisioning time. So "the proxy relays only its declared
route and direction" is a claim about what the broker transferred, not about what
the parent withheld. No supervision handle is delegated — unlike the call and
operation planes, nothing in this graph names a task.

### The interposition is profile-borne

Every participant declares `interposition = []`; the chain arrives from the
`sel4` profile and is applied by `resolve_fabric_graph`. That mirrors the
oracle's own `visibility` profile rather than inlining the chain, which would
claim the chain is a property of the route rather than of the selected profile.
The admission marker reports `interpositions=1` and the gate asserts it.

### The `DebugWrite` defect

`visibility_broker::write_record` prints a 64-byte record as 128 hex characters.
`slime-root` read the staged payload through `read_staged`, bounded by
`MAX_STAGED_BYTES == ipc::MAX_MESSAGE_BYTES == 64`, so the write was refused as
`InvalidLength` and only the 13-byte prefix reached the transcript.

A diagnostic line is not a message: it crosses no channel and is bounded by
nothing the IPC contract states. `read_staged_array` — already used for the wide
spawn-grant array, and already refusing any descriptor naming a capability, which
is the rule this arm enforced by hand — is the correct reader. See
[`debug-write-bound.txt`](debug-write-bound.txt).

No earlier plane could have found this: every marker the other twelve gates
assert is well under 64 bytes.

## Regression guards

| Risk | Guard | Failure signal |
|---|---|---|
| A caller's view widens or narrows | the twelve-record count in `just sel4_visibility_check` | "the composition emitted N view records, expected 12" |
| The loss trace stops differing from the relay trace | the distinct-trace assertion | "the relay and loss traces are byte-identical" |
| The declared chain is dropped at admission | `interpositions=1` in the admission marker | the first chain's marker goes missing |
| The proxy is bypassed | `[fabric] direct interposition bypass absent`, asserted before any relay | missing marker |
| Proxy death takes down an unrelated route | the isolation chain, asserted from both the broker and the diagnostics subscriber | missing marker |
| A real participant fails and hides among the unconfigured ones | zero component failures inside the composition window | "a component failed inside the composition: …" |
| A long diagnostic line is silently truncated again | `SLIME_GRAPH debug write refused` is a failure marker | the gate turns red rather than losing a record |
| The gate loses evidence | `just sel4_gate_control_check`, pinned at 25 markers | a mutated transcript is accepted, or the count drifts |

## Verification

| Command/scenario | Result | Evidence class |
|---|---|---|
| `just sel4_visibility_check` | Pass; 25 markers across 7 chains; 12 view records, 2 distinct traces; six spawned tasks exited cleanly | Direct |
| `just sel4_gate_control_check` | Pass; 14 gates reject 653 mutated transcripts and layouts | Direct |
| `just sel4_boot_layout_check` | Pass; 11 plane layouts match their fixtures | Direct |
| `just test_sel4_root` | Pass; 113/113 across 13 modules | Direct |
| `just sel4_root_boot_check` | Pass | Direct |
| `just sel4_component_graph_check` | Pass | Direct |
| `just sel4_channel_check` | Pass | Direct |
| `just sel4_loan_check` | Pass | Direct |
| `just sel4_spawn_check` | Pass | Direct |
| `just sel4_sample_check` | Pass | Direct |
| `just sel4_supervision_check` | Pass | Direct |
| `just sel4_crossing_check` | Pass | Direct |
| `just sel4_stream_check` | Pass | Direct |
| `just sel4_qos_check` | Pass | Direct |
| `just sel4_call_check` | Pass | Direct |
| `just sel4_operation_check` | Pass | Direct |
| `just lint_all`, `just fmt_check_all`, `just ruff`, `just typos`, `just devlog_check` | Pass | Direct |
| C8.8 semantics themselves | Inherited from `just fabric_visibility_check` on the frozen oracle | Inherited |

Every seL4 plane gate was re-run, and here that is not routine caution: the
`DebugWrite` change touches the path every marker in every gate travels.

## Decisions

- **Decision:** Widen `DebugWrite` to `read_staged_array` rather than shortening
  `write_record` or splitting it into 64-byte chunks.
  **Rationale:** the record is one atomic line, and B18's whole fix was making a
  diagnostic line uninterruptible. Chunking would reintroduce interleaving; a
  shorter record would change the oracle's byte-for-byte determinism claim.
  **Rejected alternative:** raising `MAX_MESSAGE_BYTES`, which would change the
  channel contract every plane depends on to fix a diagnostic path.

- **Decision:** Assert zero component failures inside the composition window
  rather than the stream gate's one-failure-per-component budget.
  **Rationale:** on this plane the unconfigured root-launched instances are
  slower than the composition and fail *after* init's clean exit, so a budget
  over the whole transcript depends on how far QEMU ran past the last marker.
  The window assertion cannot be satisfied by a real participant failing.
  **Rejected alternative:** the per-component budget, which failed with
  "fabric-service reported 0 failures" purely because the boot stopped earlier.

- **Decision:** Keep `fabric-intruder` as the declared proxy.
  **Rationale:** that is what the oracle's generation 16 boots. `fabric-proxy`
  and `fabric-observer` belong to the later unified profile; booting them would
  be porting C8.10, not C8.8.
  **Rejected alternative:** the unified plane, which is P5.4.9's subject.

## Open risks and follow-ups

- [ ] The gate boots once. The oracle's `check-fabric-visibility.py` boots the
      normal profile **twice** and compares the twelve view records and two
      traces byte-for-byte across runs, plus a third early-proxy-death boot. The
      determinism half of C8.8's fourth required check is therefore inherited
      rather than re-observed here; the record *counts* and trace *distinctness*
      are observed. A repeat-boot comparison is a small addition and would close
      it.
- [ ] `fabric-observer` and `fabric-probe` remain unbooted on seL4. They are
      C8.10 identities and belong with P5.4.9's full-graph plane.

## Artifacts and provenance

- Direct gate evidence: [`visibility-check.txt`](visibility-check.txt), captured
  from `just sel4_visibility_check` on 2026-08-08 with the pinned qemu-arm-virt
  profile.
- The root defect this slice found, with the before and after transcript lines:
  [`debug-write-bound.txt`](debug-write-bound.txt).
- The slice before it, whose composition this one narrows:
  [`devlog/2026-08-08-p5-4-7-operation-plane/`](../2026-08-08-p5-4-7-operation-plane/index.md).
- The inventory that recorded C8.8 as uncovered:
  [`devlog/2026-08-07-p5-4-1-oracle-inventory/`](../2026-08-07-p5-4-1-oracle-inventory/index.md).
- Related roadmap item: P5.4.8 in
  [`roadmap/07-architecture-portability.md`](../../roadmap/07-architecture-portability.md).
