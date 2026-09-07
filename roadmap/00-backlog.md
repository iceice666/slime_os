# Backlog (frozen index of pre-cutover defects)

> **Frozen index, not the backlog.** The backlog is the `backlog`-tagged items
> in `.tasks/items/`, which own identity, state, hierarchy, and dependencies.
> This file indexes the entries that existed before the work-item store became
> the repository's only tracker, and it survives for one reason: 75 devlog
> entries link into it, 8 of them anchored at a specific heading. Use
> `just tasks_list` for current state and `just tasks_next` for actionable work.

**Purpose:** Keep every landed `### B<N>` anchor resolving, and route each one
to the records that own it — the devlog entry holding the investigation and the
work item holding the state. Nothing here is a problem statement, an exit
condition, or a status; the item owns all three.

**New work does not come here.** A defect is
`myque new "<title>" --kind bug --tag backlog`, which allocates its UUID, plus
a devlog entry where the investigation warrants one. No heading, no table row,
and no edit to this file is required, and `just tasks_check` asks for none:
adding a second place to write a defect down is what this cutover removed.

**Priority is unchanged and lives in the store.** Open backlog items are
handled before milestone work; a green verification suite is a precondition for
milestone work, not a milestone itself. `just tasks_check` enforces that against
the store: an `active` milestone alongside an `open` or `active` backlog item
fails the gate. `deferred` and `blocked` are the two explicit escapes — the
first says the work is postponed by decision, the second that it waits on
something outside this repository.

**Entry shape:** each `### B<N> — <title>` heading is a stable link target and
nothing more. It allocates no identity and resolves no reference; the UUID
beside it does both. Leave a landed heading's text alone — devlog anchors point
at it — and note that `B29` and `B30` were each allocated twice, which is why
their four entries name UUIDs rather than share a key. `just tasks_check` fails
when a heading names an item the store does not have, or names one that is not
closed.

## Deferred follow-ups

Some resolved entries deliberately left half their work undone. Those halves
are `followup` items in the store, each deferred and each depending on the item
that surfaced it, so `just tasks_list` answers which exist and what state they
are in — this file does not restate it. The audits that surfaced them are
[`devlog/2026-08-17-structural-audit/`](../devlog/2026-08-17-structural-audit/index.md)
and
[`devlog/2026-08-24-c10-4-adoption-and-leak-evidence/`](../devlog/2026-08-24-c10-4-adoption-and-leak-evidence/index.md).

## Resolved

Every entry below is closed. The heading is the link target, the devlog entry
is the investigation, and the work item is the state and the exit condition
that was observed. None of it is restated here.

### B92 — resident product graph stops after 32768 valid requests

**Evidence:** [`devlog/2026-09-01-b92-resident-graph-lifetime/`](../devlog/2026-09-01-b92-resident-graph-lifetime/index.md) **Item:** `01a0588c-8800-796d-9195-3321aefc0c6c`

### B91 — composition binding slots still duplicate component-local positional ABI

**Evidence:** [`devlog/2026-08-30-b91-slot-pin-reasons/`](../devlog/2026-08-30-b91-slot-pin-reasons/index.md) **Item:** `01a04e3f-d000-74ca-92da-c7e0142bf2e1`

### B90 — the `Block` capability kind and its two rights bits survive B83 with no operation to gate

**Evidence:** [`devlog/2026-08-30-b90-block-kind-retired/`](../devlog/2026-08-30-b90-block-kind-retired/index.md) **Item:** `01a04e3f-d000-7cac-b464-71870be338de`

### B89 — fourteen contract renderers each carried the same 82-line copy of the Rust codec emitters

**Evidence:** [`devlog/2026-08-29-b89-shared-codec-emitters/`](../devlog/2026-08-29-b89-shared-codec-emitters/index.md) **Item:** `01a04919-7400-7796-8f68-7a1a363fe7bc`

### B88 — the network-destination decoder read a Zutai-declared layout through hand-written byte literals

**Evidence:** [`devlog/2026-08-29-b88-network-destination-generated-offsets/`](../devlog/2026-08-29-b88-network-destination-generated-offsets/index.md) **Item:** `01a04919-7400-71d0-99c9-79a20e1974d8`

### B86 — the virtio-net driver trusted the device's used-ring descriptor id, so a device could settle another client's request

**Evidence:** [`devlog/2026-08-29-b86-virtio-net-device-boundary/`](../devlog/2026-08-29-b86-virtio-net-device-boundary/index.md) **Item:** `01a04919-7400-7b50-8817-dfe33ba3920c`

### B87 — a receive completion's reported length was unbounded and silently truncated to `u16`

**Evidence:** [`devlog/2026-08-29-b86-virtio-net-device-boundary/`](../devlog/2026-08-29-b86-virtio-net-device-boundary/index.md) **Item:** `01a04919-7400-7869-bad9-87915e92f21e`

### B85 — `just machete` fails on three stale `slime-proto` dependencies

**Evidence:** [`devlog/2026-08-29-b85-stale-proto-dependencies/`](../devlog/2026-08-29-b85-stale-proto-dependencies/index.md) **Item:** `01a04919-7400-7162-84c9-033adf51e8a4`

### B83 — The root still owns the product virtio-blk path after IO2's parity proof

**Evidence:** [`devlog/2026-08-29-b83-root-block-path-deleted/`](../devlog/2026-08-29-b83-root-block-path-deleted/index.md) **Item:** `01a04919-7400-7839-b017-2e9566a3dac7`

### B84 — a userspace driver instance can serve exactly one device, so a two-disk plane cannot leave the root

**Evidence:** [`devlog/2026-08-28-b84-two-device-driver-instances/`](../devlog/2026-08-28-b84-two-device-driver-instances/index.md) **Item:** `01a043f3-1800-7e49-948c-39a089fe5c20`

### B82 — `launch_instance_graph` overflowed the root stack into mapped `.bss`

**Evidence:** [`devlog/2026-08-28-io1-hardware-resource-authority/`](../devlog/2026-08-28-io1-hardware-resource-authority/index.md) **Item:** `01a043f3-1800-7ba4-b705-998272a17781`

### B81 — Freestanding C components emitted an unaligned writable load segment

**Evidence:** [`devlog/2026-08-27-b81-rpi5-c-segment-layout/`](../devlog/2026-08-27-b81-rpi5-c-segment-layout/index.md) **Item:** `01a03ecc-bc00-7b8a-ae5c-f4dd8647ef71`

### B80 — Clean RPi5 builds lacked the `aarch64-unknown-none` Rust target

**Evidence:** [`devlog/2026-08-27-b80-rpi5-rust-target/`](../devlog/2026-08-27-b80-rpi5-rust-target/index.md) **Item:** `01a03ecc-bc00-7f23-8d29-7b0f23a82b3a`

### B79 — Default seL4 builds omitted the resident external Slisp ELF

**Evidence:** [`devlog/2026-08-27-b79-default-sel4-build/`](../devlog/2026-08-27-b79-default-sel4-build/index.md) **Item:** `01a03ecc-bc00-70a4-8c4e-1b8a80c79561`

### B78 — CI could not pass: a prefix gate on a prefixless runner, a job with no runner, and a stale child path

**Evidence:** [`devlog/2026-08-27-ci-hosted-arm64-cutover/`](../devlog/2026-08-27-ci-hosted-arm64-cutover/index.md) **Item:** `01a03ecc-bc00-7b29-a1a7-aedeef16aa5c`

### B77 — `budget_us`/`period_us` are authenticated but unvalidated and unread

**Evidence:** [`devlog/2026-08-24-b77-undeclarable-cpu-budget/`](../devlog/2026-08-24-b77-undeclarable-cpu-budget/index.md) **Item:** `01a02f59-a800-71ef-977f-bb263c65b2d4`

### B70 — component definitions and slot/route bindings are compile-time-coupled to one crate's private manifest parser, blocking out-of-tree components

**Evidence:** [`devlog/2026-08-22-b70-profile-include-closure/`](../devlog/2026-08-22-b70-profile-include-closure/index.md) **Item:** `01a0250c-f000-7b2a-92d3-fde78c957925`

### B75 — the fabric graph intermittently stops draining under host load, and the root cannot tell that from success

**Evidence:** [`devlog/2026-08-20-b75-observed-vs-declared-trace-fields/`](../devlog/2026-08-20-b75-observed-vs-declared-trace-fields/index.md) **Item:** `01a01ac0-3800-7a5a-a50b-14f34e5f1e4e`

### B76 — `IpcError::PeerDead` is declared and status-mapped but never constructed, so three brokers carry unreachable death-detection arms

**Evidence:** [`devlog/2026-08-20-b76-peer-death-cleanup/`](../devlog/2026-08-20-b76-peer-death-cleanup/index.md) **Item:** `01a01ac0-3800-786f-9a5f-99eb8455c3d4`

### B74 — the aggregate gate's traffic schedule failed twice in one session on a gate B68 closed as deterministic

**Evidence:** [`devlog/2026-08-20-b74-aggregate-flake/`](../devlog/2026-08-20-b74-aggregate-flake/index.md) **Item:** `01a01ac0-3800-78cd-81dc-7cecbee5b5cf`

### B73 — the matrix plane never checks the view a graph-visibility holder pages, only the observer's

**Evidence:** [`devlog/2026-08-20-b73-matrix-graph-view/`](../devlog/2026-08-20-b73-matrix-graph-view/index.md) **Item:** `01a01ac0-3800-7348-9883-de37f9f57462`

### B72 — the visibility plane's QoS view records are counted but never checked against the route they describe

**Evidence:** [`devlog/2026-08-19-b72-frozen-visibility-view/`](../devlog/2026-08-19-b72-frozen-visibility-view/index.md) **Item:** `01a01599-dc00-7f6b-a7b8-3a3aac8fa5df`

### B71 — the boot-layout resource's binary encoding was never cross-checked against the manifest's real bindings, and unnamed roles are never narrowed

**Evidence:** [`devlog/2026-08-18-b71-boot-layout-binary-drift/`](../devlog/2026-08-18-b71-boot-layout-binary-drift/index.md) **Item:** `01a01073-8000-7273-b1d7-01b585bcaba1`

### B69 — RP1's artifact gate never ran, and hid three admission defects

**Evidence:** [`devlog/2026-08-17-ros2-transport-zenoh-pivot/`](../devlog/2026-08-17-ros2-transport-zenoh-pivot/index.md) **Item:** `01a00b4d-2400-7c99-91f7-75c4ba8368bb`

### B68 — the determinism gate compared one scheduling interleaving

**Evidence:** [`devlog/2026-08-17-b68-aggregate-trace-determinism/`](../devlog/2026-08-17-b68-aggregate-trace-determinism/index.md) **Item:** `01a00b4d-2400-7c50-98e4-b4455b9b29f2`

### B65 — `init.rs` held 21 plane launchers in one file; four moved out and the binary collapse did not

**Evidence:** [`devlog/2026-08-17-b65-plane-modules/`](../devlog/2026-08-17-b65-plane-modules/index.md) **Item:** `01a00b4d-2400-7af9-b5c1-4546f1b399e0`

### B63 — the plane gates reimplemented helpers a shared library should own

**Evidence:** [`devlog/2026-08-17-b63-gate-helper-consolidation/`](../devlog/2026-08-17-b63-gate-helper-consolidation/index.md) **Item:** `01a00b4d-2400-7f8d-ba52-04cbc6b7df71`

### B61 — `just run` booted a test fixture, and the dispatch routing was untestable

**Evidence:** [`devlog/2026-08-17-b61-product-image-and-dispatch/`](../devlog/2026-08-17-b61-product-image-and-dispatch/index.md) **Item:** `01a00b4d-2400-76f2-80fd-3b6c8f7f99d7`

### B62 — the `.zti` fixtures were copy-paste: three 1882-line files differed by one field

**Evidence:** [`devlog/2026-08-17-b62-fixture-deltas/`](../devlog/2026-08-17-b62-fixture-deltas/index.md) **Item:** `01a00b4d-2400-726c-9b23-cc13f9eb292e`

### B64 — the format-coexistence answer existed in code but was written down nowhere, and one retained schema was unguarded

**Evidence:** [`devlog/2026-08-17-b64-format-coexistence/`](../devlog/2026-08-17-b64-format-coexistence/index.md) **Item:** `01a00b4d-2400-706e-9ff9-ee35b95efde4`

### B60 — authority-derivation policy lived in the builder, and one slot number had two independent sources

**Evidence:** [`devlog/2026-08-17-b60-control-plane-authority/`](../devlog/2026-08-17-b60-control-plane-authority/index.md) **Item:** `01a00b4d-2400-7f5d-aba4-9a4c213ab256`

### B66 — `ipc.rs` carried two retired-mechanism constants, one of them load-bearing

**Evidence:** [`devlog/2026-08-17-b59-b66-syscall-abi-contract/`](../devlog/2026-08-17-b59-b66-syscall-abi-contract/index.md) **Item:** `01a00b4d-2400-77c8-a3bf-e1cc8040486d`

### B59 — the syscall ABI had no single source: 97 rights declarations, two label tables, three error tables

**Evidence:** [`devlog/2026-08-17-b59-b66-syscall-abi-contract/`](../devlog/2026-08-17-b59-b66-syscall-abi-contract/index.md) **Item:** `01a00b4d-2400-7c5a-86f1-3469a46c2c25`

### B67 — two negative controls picked declared slots, so neither could fail

**Evidence:** [`devlog/2026-08-17-b67-blind-negative-controls/`](../devlog/2026-08-17-b67-blind-negative-controls/index.md) **Item:** `01a00b4d-2400-7312-88cd-595d915ffae6`

### B57 — `RIGHT_ALL` had two definitions, and the wider one admitted an undefined rights bit

**Evidence:** [`devlog/2026-08-17-b57-b58-rights-vocabulary/`](../devlog/2026-08-17-b57-b58-rights-vocabulary/index.md) **Item:** `01a00b4d-2400-7d7c-82e2-df8402734f41`

### B58 — `check-architecture-contract.py` hand-copied three generated header offsets

**Evidence:** [`devlog/2026-08-17-b57-b58-rights-vocabulary/`](../devlog/2026-08-17-b57-b58-rights-vocabulary/index.md) **Item:** `01a00b4d-2400-74bc-a59c-23cd48a15c89`

### B56 — `data_fabric_profile_check` asserted a contradiction and had been red since B55

**Evidence:** [`devlog/2026-08-17-structural-audit/`](../devlog/2026-08-17-structural-audit/index.md) **Item:** `01a00b4d-2400-732f-b4b2-76a7e272d6b0`

### B55 — the full-graph boot plane refused its own first spawn, then five more defects behind it

**Evidence:** [`devlog/2026-08-15-b55-full-graph-boot-restoration/`](../devlog/2026-08-15-b55-full-graph-boot-restoration/index.md) **Item:** `01a00100-6c00-799f-8f9b-f69e2a662d1f`

### B53 — dango echoed a line one byte past the message bound

**Evidence:** [`devlog/2026-08-14-b53-b54-last-two-planes/`](../devlog/2026-08-14-b53-b54-last-two-planes/index.md) **Item:** `019ffbda-1000-7c7e-9156-b32338d26679`

### B54 — the stress plane borrowed a component that never ends

**Evidence:** [`devlog/2026-08-14-b53-b54-last-two-planes/`](../devlog/2026-08-14-b53-b54-last-two-planes/index.md) **Item:** `019ffbda-1000-7043-9f74-05d2ca292205`

### B46 — logical ChannelTable, Transit, ParkedReplies, and WaitSet duplicate seL4 IPC

**Evidence:** [`devlog/2026-08-13-b46-native-ipc-completion/`](../devlog/2026-08-13-b46-native-ipc-completion/index.md) **Item:** `019ff6b3-b400-7d7e-ab99-1bd3df003fe8`

### B50 — the logical capability and universal syscall compatibility model remains deletable residue

**Evidence:** [`devlog/2026-08-14-b50-minted-endpoint-deletion/`](../devlog/2026-08-14-b50-minted-endpoint-deletion/index.md) **Item:** `019ffbda-1000-7716-9d07-a3a799b8eb97`

### B48 — all child execution shares one fixed priority and no scheduling authority

**Evidence:** [`devlog/2026-08-10-b48-declared-priority/`](../devlog/2026-08-10-b48-declared-priority/index.md) **Item:** `019ff18d-5800-70c6-a987-6a926141c775`

### B49 — resource ceilings are reactive tables rather than an admitted object budget

**Evidence:** [`devlog/2026-08-10-b49-object-budget/`](../devlog/2026-08-10-b49-object-budget/index.md) **Item:** `019fe740-a000-7a4f-a270-7a921045f9dc`

### B47 — package, process, thread, service instance, and lifecycle are one Task model

**Evidence:** [`devlog/2026-08-10-b47-runtime-threads/`](../devlog/2026-08-10-b47-runtime-threads/index.md) **Item:** `019fe740-a000-722b-bf6a-29f2db805351`

### B52 — the loan plane never launches the receiver it loans to

**Evidence:** [`devlog/2026-08-10-b52-loan-plane-peers/`](../devlog/2026-08-10-b52-loan-plane-peers/index.md) **Item:** `019fe740-a000-7993-bea6-51be10e1ab4b`

### B51 — the spawn preflight cannot tell a respawn from a first launch

**Evidence:** [`devlog/2026-08-10-b51-respawn-provenance/`](../devlog/2026-08-10-b51-respawn-provenance/index.md) **Item:** `019fe740-a000-748f-9d82-241cef6cd4c4`

### B45 — directory, filesystem, and store services still depend on universal root IPC

**Evidence:** [`devlog/2026-08-10-b45-directory-service-split/`](../devlog/2026-08-10-b45-directory-service-split/index.md) **Item:** `019fe740-a000-7a64-a099-8eae60c22667`

### B44 — generation and recovery policy still crosses the universal root dispatcher

**Evidence:** [`devlog/2026-08-10-b44-policy-labels-deleted/`](../devlog/2026-08-10-b44-policy-labels-deleted/index.md) **Item:** `019fe740-a000-7043-9dfe-a26a769fd721`

### B43 — block and durable-store clients still transact through root operation labels

**Evidence:** [`devlog/2026-08-10-b43-block-service-endpoint/`](../devlog/2026-08-10-b43-block-service-endpoint/index.md) **Item:** `019fe740-a000-7141-8813-a58e7b330fce`

### B41 — console and debug traffic still enters the universal root dispatcher

**Evidence:** [`devlog/2026-08-10-b41-console-endpoint/`](../devlog/2026-08-10-b41-console-endpoint/index.md) **Item:** `019fe740-a000-71ef-b559-5eda9e614867`

### B42 — spawn and lifecycle control use ambient task IDs and the universal dispatcher

**Evidence:** [`devlog/2026-08-10-b42-lifecycle-identity/`](../devlog/2026-08-10-b42-lifecycle-identity/index.md) **Item:** `019fe740-a000-7a5f-ae63-3815f93d8de3`

### B40 — child CSpaces are fixed four-slot shells rather than admitted authority

**Evidence:** [`devlog/2026-08-10-b40-native-child-cspaces/`](../devlog/2026-08-10-b40-native-child-cspaces/index.md) **Item:** `019fe740-a000-7e18-b5a8-c9df6403f4d8`

### B39 — Generation v5 must describe the exact seL4 object and authority plan

**Evidence:** [`devlog/2026-08-10-b39-generation-v5-checker-cutover/`](../devlog/2026-08-10-b39-generation-v5-checker-cutover/index.md) **Item:** `019fe740-a000-7e3d-bf71-94087a9b0fb7`

### B34 — generation component records conflate executable catalogue entries with initial instances

**Evidence:** [`devlog/2026-08-09-b34-b38-sel4-model-audit/`](../devlog/2026-08-09-b34-b38-sel4-model-audit/index.md) **Item:** `019fe740-a000-71ab-824f-d410e5503d96`

### B35 — BootState does not select the generation the seL4 product boots

**Evidence:** [`devlog/2026-08-09-b34-b38-sel4-model-audit/`](../devlog/2026-08-09-b34-b38-sel4-model-audit/index.md) **Item:** `019fe740-a000-735c-8140-79780460ecb3`

### B36 — the full-graph gate stops at a non-unique component idle marker

**Evidence:** [`devlog/2026-08-09-b34-b38-sel4-model-audit/`](../devlog/2026-08-09-b34-b38-sel4-model-audit/index.md) **Item:** `019fe740-a000-755e-8b9f-df3cc536849e`

### B37 — dependency activation and non-bootstrap slot ABI are implicit contracts

**Evidence:** [`devlog/2026-08-09-b34-b38-sel4-model-audit/`](../devlog/2026-08-09-b34-b38-sel4-model-audit/index.md) **Item:** `019fe740-a000-7e0e-8eb9-53c701c68f6d`

### B38 — task reclamation cannot reuse root CSlots or untyped memory

**Evidence:** [`devlog/2026-08-09-b34-b38-sel4-model-audit/`](../devlog/2026-08-09-b34-b38-sel4-model-audit/index.md) **Item:** `019fe740-a000-7659-8339-ea3d25b232db`

### B33 — seL4 cutover review findings

**Evidence:** [`devlog/2026-08-09-b33-cutover-review-remediation/`](../devlog/2026-08-09-b33-cutover-review-remediation/index.md) **Item:** `019fe21a-4400-7967-8b1f-1a78eed530ac`

### B31 — six oracle properties blocked `kernel/` deletion

**Evidence:** [`devlog/2026-08-09-p5-4-final-kernel-retirement/`](../devlog/2026-08-09-p5-4-final-kernel-retirement/index.md) **Item:** `019fe21a-4400-7cc2-bb15-be2c4cc277a9`

### B32 — three scenario receive spins were invisible to the root

**Evidence:** [`devlog/2026-08-09-b32-parked-scenario-receivers/`](../devlog/2026-08-09-b32-parked-scenario-receivers/index.md) **Item:** `019fe21a-4400-77cf-930a-cb385f3bc818`

### B29 — one block device per granule

**Evidence:** [`devlog/2026-08-08-p5-4-3-transfer-plane/`](../devlog/2026-08-08-p5-4-3-transfer-plane/index.md) **Item:** `019fdcf3-e800-7f2a-b022-fc9a977cacfa`

### B30 — the dango plane launched no commands

**Evidence:** [`devlog/2026-08-08-p5-4-3-dango-plane/`](../devlog/2026-08-08-p5-4-3-dango-plane/index.md) **Item:** `019fdcf3-e800-7b17-ae1d-6b4b096a6671`

### B25 — a spawn-granted endpoint moves on seL4 and copies on x86, so a parent cannot broker a later introduction

**Evidence:** [`devlog/2026-08-08-b25-endpoint-copy-call-plane/`](../devlog/2026-08-08-b25-endpoint-copy-call-plane/index.md) **Item:** `019fdcf3-e800-75f0-8f8b-39164dfabe16`

### B28 — a `retained` second route on one publisher stops a *different* publisher's parked role reply from ever being taken

**Evidence:** [`devlog/2026-08-07-b28-iteration-budget/`](../devlog/2026-08-07-b28-iteration-budget/index.md) **Item:** `019fd7cd-8c00-7fca-8549-1ccc19b7bea7`

### B12 — the component build's `--remap-path-prefix` names a path that does not exist

**Evidence:** [`devlog/2026-08-07-b12-component-remap/`](../devlog/2026-08-07-b12-component-remap/index.md) **Item:** `019fd7cd-8c00-7a24-907a-86c137e6e62a`

### B30 — `release_trust_check` was red, unregistered, and its rotation refusals never reached Rust

**Evidence:** [`devlog/2026-08-07-b30-release-trust-gate/`](../devlog/2026-08-07-b30-release-trust-gate/index.md) **Item:** `019fd7cd-8c00-76ca-a4f2-2f2dc6c169a0`

### B29 — `ParkedReplies::wake` never deleted the reply CSlot it counted as recycled — **resolved 2026-08-07**

**Evidence:** no devlog entry; the work item is the sole record. **Item:** `019fd7cd-8c00-782b-b8f9-66eb3350e63b`

### B27 — the manifest→flag table set and scrubbed in one pass, so two manifests could not share a flag — **resolved 2026-08-07**

**Evidence:** [`devlog/2026-08-07-p5-4-5-qos-clock/`](../devlog/2026-08-07-p5-4-5-qos-clock/index.md) **Item:** `019fd7cd-8c00-767c-bfdd-df37c023edec`

### B26 — the `[layout]` dump reported the grant's rights, so a too-permissive layout row was unobservable — **resolved 2026-08-07**

**Evidence:** [`devlog/2026-08-07-b26-layout-declared-rights/`](../devlog/2026-08-07-b26-layout-declared-rights/index.md) **Item:** `019fd7cd-8c00-77be-9bbe-b0a3d09a370a`

### B24 — `SharedBufferTable::quotas` never reclaimed, so `MAX_CHARGE_HOLDERS` was a lifetime bound — **resolved 2026-08-07**

**Evidence:** [`devlog/2026-08-07-b24-shared-buffer-quotas/`](../devlog/2026-08-07-b24-shared-buffer-quotas/index.md) **Item:** `019fd7cd-8c00-7f4d-bff4-21e1cfdc315d`

### B23 — `slime-root`'s unit tests were run by no gate — **resolved 2026-08-07**

**Evidence:** [`devlog/2026-08-07-b23-slime-root-host-tests/`](../devlog/2026-08-07-b23-slime-root-host-tests/index.md) **Item:** `019fd7cd-8c00-77e1-bc23-fdb64b7d5df3`

### B22 — `ChannelTable` never reclaimed, so `MAX_CHANNELS` was a lifetime bound — **resolved 2026-08-07**

**Evidence:** [`devlog/2026-08-07-p5-4-1-oracle-inventory/`](../devlog/2026-08-07-p5-4-1-oracle-inventory/index.md) **Item:** `019fd7cd-8c00-70b2-91d1-8670457ab82e`

### B21 — the toolchain was pinned by name, so each host resolved a different binary — **resolved 2026-08-06**

**Evidence:** [`devlog/2026-08-06-b21-cross-toolchain-binary-selection/`](../devlog/2026-08-06-b21-cross-toolchain-binary-selection/index.md) **Item:** `019fd2a7-3000-780d-8f18-9aa66c0afff9`

### B16 — a supervision termination record was never reclaimed, so a long-lived graph exhausted the table — **resolved 2026-08-07**

**Evidence:** [`devlog/2026-08-07-b16-supervision-records/`](../devlog/2026-08-07-b16-supervision-records/index.md) **Item:** `019fd7cd-8c00-7817-aa68-aa49dacd7a0a`

### B20 — the prefix pin held for one platform at a time — **resolved 2026-08-06**

**Evidence:** [`devlog/2026-08-06-b20-cross-platform-kernel-identity/`](../devlog/2026-08-06-b20-cross-platform-kernel-identity/index.md) **Item:** `019fd2a7-3000-7239-adb2-62b25802fdf1`

### B19 — the seL4 prefix pins bound the dev-shell derivation hash, not the toolchain — **resolved 2026-08-06**

**Evidence:** [`devlog/2026-08-06-b19-sel4-prefix-pin-shell-coupling/`](../devlog/2026-08-06-b19-sel4-prefix-pin-shell-coupling/index.md) **Item:** `019fd2a7-3000-7ff0-adfa-bd30cd147992`

### B18 — the seL4 stream gate was scheduling-dependent — **resolved 2026-08-06**

**Evidence:** [`devlog/2026-08-05-p5-5-2-stream-plane/`](../devlog/2026-08-05-p5-5-2-stream-plane/index.md) **Item:** `019fd2a7-3000-7949-9c37-f642fdfc7d18`

### B17 — the capability transfer's subset test had no coverage — **resolved 2026-08-05**

**Evidence:** [`devlog/2026-08-05-p5-5-2-stream-plane/`](../devlog/2026-08-05-p5-5-2-stream-plane/index.md) **Item:** `019fcd80-d400-7b8a-bd67-244e04f6ac14`

### B15 — a spawn carries at most four grants on seL4, against the oracle's sixty-four — **resolved 2026-08-05**

**Evidence:** [`devlog/2026-08-05-p5-5-1-typed-fabric/`](../devlog/2026-08-05-p5-5-1-typed-fabric/index.md) **Item:** `019fcd80-d400-7b3e-ad54-c642ffd3eb4e`

### B14 — `slime-root` ignores the generation's declared spawn budget

**Evidence:** [`devlog/2026-08-05-p5-3-4-sample-plane/`](../devlog/2026-08-05-p5-3-4-sample-plane/index.md) **Item:** `019fcd80-d400-7a0f-bda9-4434605b76ad`

### B13 — `slime-root` admits a shared-buffer allocation without resolving a factory capability

**Evidence:** [`devlog/2026-08-05-p5-3-3-spawn-plane/`](../devlog/2026-08-05-p5-3-3-spawn-plane/index.md) **Item:** `019fcd80-d400-7c84-b07d-5e6fafe7d13d`

### B11 — test scaffolding is declared in the product boot generation

**Evidence:** [`devlog/2026-08-01-b11-product-boot-profiles/`](../devlog/2026-08-01-b11-product-boot-profiles/index.md) **Item:** `019fb8e7-6400-718f-84ee-7ede540da059`

### B10 — init's capability layout is a positional convention, so boot paths are selected at kernel compile time

**Evidence:** [`devlog/2026-07-31-boot-layout-baseline/`](../devlog/2026-07-31-boot-layout-baseline/index.md) **Item:** `019fb8e7-6400-7e95-b349-16e4c7073161`

### B9 — terminated tasks are never reaped, so their frames never return

**Evidence:** [`devlog/2026-07-28-b9-task-frame-reclamation/`](../devlog/2026-07-28-b9-task-frame-reclamation/index.md) **Item:** `019fa44d-f400-7dd6-bdd8-c247dd974dd4`

### B8 — budget validation bounded each holder but never the aggregate

**Evidence:** [`devlog/2026-07-26-b7-b8-budget-hygiene/`](../devlog/2026-07-26-b7-b8-budget-hygiene/index.md) **Item:** `019f9a01-3c00-7fea-8d50-eb4f205cd96d`

### B7 — the `RIGHT_MAP` rename never reached the manifest vocabulary

**Evidence:** [`devlog/2026-07-26-b7-b8-budget-hygiene/`](../devlog/2026-07-26-b7-b8-budget-hygiene/index.md) **Item:** `019f9a01-3c00-75ed-9d34-a0b4a282c01a`

### B6 — the retained-v2 "still boots" claim was proven only as decode

**Evidence:** [`devlog/2026-07-26-b6-retained-v2-rollback-scope/`](../devlog/2026-07-26-b6-retained-v2-rollback-scope/index.md) **Item:** `019f9a01-3c00-7a85-b473-6dbefa027e6e`

### B5 — no C7 gate exercised the syscall layer or real components

**Evidence:** [`devlog/2026-07-26-b5-live-sample-plane/`](../devlog/2026-07-26-b5-live-sample-plane/index.md) **Item:** `019f9a01-3c00-70d4-b48d-53f80f2c70e7`

### B4 — the C7 shared-buffer plane was dormant on the live boot path

**Evidence:** [`devlog/2026-07-26-b4-live-shared-buffer-budget/`](../devlog/2026-07-26-b4-live-shared-buffer-budget/index.md) **Item:** `019f9a01-3c00-768b-a173-3116d5c9c5ea`

### B3 — C7.5 wedged every full-graph boot (kernel-stack overflow)

**Evidence:** [`devlog/2026-07-26-b3-shared-buffer-table-stack-overflow/`](../devlog/2026-07-26-b3-shared-buffer-table-stack-overflow/index.md) **Item:** `019f9a01-3c00-71a4-a030-dac5023f9522`

### B2 — scheduler has no `Blocked` task state (busy-poll pathology)

**Evidence:** [`devlog/2026-07-24-boot-check-hangs/`](../devlog/2026-07-24-boot-check-hangs/index.md) **Item:** `019f8fb4-8400-78c4-a3c0-3b831fb020cc`

### B1 — `generation_cmd_check` negative scenarios corrupted the wrong generation

**Evidence:** [`devlog/2026-07-24-generation-cmd-check-wrong-target/`](../devlog/2026-07-24-generation-cmd-check-wrong-target/index.md) **Item:** `019f8fb4-8400-7ead-be2d-c5e980505613`
