[private]
help:
    @just --choose

# === seL4 Product Targets ===
#
# The product runs on upstream seL4 16.0.0 (`deps/sel4`) with `slime-root` as
# the dynamic mechanism owner. All behavioral targets below boot that product
# or exercise its host-testable contracts.

# Verify every exact pin the seL4 product depends on: submodule commits and
# origins, the seL4 release, the rust-sel4 and workspace Rust toolchains, the
# root target spec bytes, and the qemu-arm-virt kernel configuration (hypervisor
# ON, MCS OFF, one node) against `sel4/pins.toml`. Fetches nothing: submodules
# must already be initialized and the pinned toolchain already installed.
sel4_pin_check:
    python3 scripts/check/check-sel4-pins.py

# Configure, build, and install seL4 for qemu-arm-virt, build the root child and
# root task against the pinned rust-sel4 target specs, build the loader, add the
# kernel+root payload, and write `build/slime-sel4.identity.json` recording the
# source, config, ELF, and image digests the boot gate re-checks.
sel4_qemu_image_check: sel4_pin_check
    python3 scripts/build/build-sel4.py --skip-pin-check

# Boot the packaged image on the pinned machine (`virt,virtualization=on`,
# cortex-a53, 1 CPU, 2048 MiB) and require the ordered generation, task, IPC,
# fault, and ready markers on serial. Rebuilds first so the booted bytes are the
# ones the identity manifest describes.
sel4_root_boot_check: sel4_pin_check
    python3 scripts/check/check-sel4-root-boot.py

# P5.2: boot the component-graph image, whose root task embeds the
# `aarch64-sel4-qemu-virt` generation, and require the ordered markers proving
# that its five native ELF components launch with their declared grants, that
# the root answers the operation surface they invoke, and that an unsupported
# operation returns a bounded Slime error with the caller still running.
#
# A separate image from `sel4_root_boot_check`'s: the two differ only in which
# generation the root task embeds, and each gate boots the artifact it asserts
# about so neither invalidates the other's evidence by being built last.
# B49: build the 23-instance image -- the largest graph the root's CSpace
# admits once every declared object is counted at its real root-side cost --
# boot it, and require every instance to be constructed and reclaimed. A graph
# that does not fit is refused before activation rather than partway through
# construction with children already running.
sel4_stress_check: sel4_pin_check
    python3 scripts/build/build-sel4.py --stress-plane
    python3 scripts/check/check-sel4-stress-plane.py

sel4_component_graph_check: sel4_pin_check
    python3 scripts/check/check-sel4-component-graph.py

# P5.3.1: build the channel-plane image, boot it, and require the ordered
# markers proving that channels are materialized from the generation's declared
# send/recv grants, that a component parked in `recv` is woken by its peer's
# send with a payload too large for the fast registers, that the queue-full and
# capability-transfer refusals are bounded Slime errors, and that every channel
# and held reply is reclaimed at teardown.
#
# A third image, beside `sel4_root_boot_check`'s and
# `sel4_component_graph_check`'s. All three differ only in which generation the
# root task embeds, and each gate boots the artifact it asserts about.
sel4_channel_check: sel4_pin_check
    python3 scripts/check/check-sel4-channel-plane.py

# P5.3.2: build the loan-plane image, boot it, and require the ordered markers
# proving that a sealed subrange is loaned to a receiver named by capability,
# mapped read-only and returned exactly once by the unmodified `sample-receiver`,
# that each of the four quota classes refuses at ceiling+1 against limits decoded
# from the generation rather than a hardcoded constant, that an unrelated holder
# is undisturbed, and that every loan, mapping, region, and in-flight capability
# is reclaimed at teardown.
#
# A fourth image, beside the three above, on the same rule: each gate boots the
# artifact it asserts about.
sel4_loan_check: sel4_pin_check
    python3 scripts/check/check-sel4-loan-plane.py

# P5.3.3: build the spawn-plane image, boot it, and require the ordered markers
# proving that a component constructs a child from a grant-resolved executable,
# that the child receives its declared capabilities at the slots its own
# numbering names, that an ungranted or widened grant is refused with nothing
# constructed, and that the parent observes the child's termination through a
# supervision handle rather than an ambient task id.
#
# A fifth image, beside the four above, on the same rule: each gate boots the
# artifact it asserts about.
sel4_spawn_check: sel4_pin_check
    python3 scripts/check/check-sel4-spawn-plane.py

# P5.3.4: build the sample-plane image, boot it, and require the frozen P5
# cutover transcript from the unmodified `sample-lender` and `sample-receiver`.
# Also requires that a spawned child is budgeted from the generation, that the
# declared spawn budget refuses a child past its ceiling, and that every loan,
# mapping, region, and window is reclaimed at teardown.
#
# A sixth image, beside the five above, on the same rule: each gate boots the
# artifact it asserts about.
sel4_sample_check: sel4_pin_check
    python3 scripts/check/check-sel4-sample-plane.py

# P5.5.2: build the stream-plane image, boot it, and require that the full C8.4
# stream plane runs on seL4 with every participant unmodified — two publishers,
# two subscribers, two routes, the >MAX_INLINE_BYTES descriptor and loan path,
# KEEP_LAST eviction under a stalled subscriber, and the frozen P5 cutover
# transcript. The transfer contract's subset test (B17) is observed here too.
#
# A seventh image, beside the six above, on the same rule: each gate boots the
# artifact it asserts about. It replaces P5.5.1's typed-fabric image, whose
# every assertion this one subsumes over a larger graph.
sel4_stream_check: sel4_pin_check
    python3 scripts/check/check-sel4-stream-plane.py

# RP2: build the demo-scoped generation — the only one that carries the C7
# bounded data path, the C8 route graph, *and* the product component graph — boot
# it, and require all three in one transcript under one admitted generation.
# Then two further boots on the same profile: a failing pending demo generation
# rolling back to a verified demo known-good root across fresh QEMU processes,
# and a component image qualified for another admitted target being refused
# before any of its bytes are mapped.
#
# The rollback and wrong-target arms are what RP2 still owed the demo: the
# existing selection gate pairs two `sel4` product generations, and every
# wrong-target assertion in this repository was host-side or a unit test, so the
# root's own refusal had never been observed on a boot.
sel4_demo_check: sel4_pin_check
    python3 scripts/check/check-sel4-demo-plane.py

# B16: build the supervision-plane image, boot it, and require that a graph
# creating more tasks over its lifetime than `MAX_RECORDS` can hold at once
# still answers `supervision_status` correctly for every live handle —
# including one held across the crossing and one parked in `Transit` across it.
#
# An eighth image, on the same rule as the seven above: each gate boots the
# artifact it asserts about.
sel4_supervision_check: sel4_pin_check
    python3 scripts/check/check-sel4-supervision-plane.py

# B38: repeatedly spawn and reclaim more task lifetimes than the old monotonic
# root CSlot and ordinary-untyped watermarks could sustain.
sel4_reclamation_check: sel4_pin_check
    python3 scripts/check/check-sel4-reclamation-plane.py

# C10.2: boot a generation declaring one executable twice — as a private-memory
# holder and as an omitted one — and require that the declared page quota is the
# ceiling the granted instance actually measures, while the omitted one grows
# nothing. The quota is read from the fixture rather than restated in the gate,
# and the probe discovers its ceiling by growing until refused, so the assertion
# is a measurement against the generation rather than two copies of a constant.
#
# C10.3 adds a second pair of instances on the same plane, allocating through
# `Vec`/`Box`/`String` over that declared region instead of growing raw pages:
# the granted one crosses a growth batch, reuses freed memory without asking the
# root for more, then observes exhaustion as a structural error and stays alive
# to report it; the omitted one finds no region at all. One plane rather than
# two, because both milestones assert properties of the same declared budget and
# a second image would boot the same root twice to check adjacent halves of it.
private_memory_check: sel4_pin_check
    python3 scripts/check/check-sel4-private-memory-plane.py


# C9.1: boot independently authorized monotonic, timer, and simulated clocks;
# observe a one-shot expiry on the holder's declared notification, cancellation,
# per-holder quota isolation, teardown cleanup, and deny-by-default refusal.
clock_authority_check: sel4_pin_check
    python3 scripts/check/check-sel4-clock-authority-plane.py


# C9.3: a declared scheduling class, its band mapping, and promotion authority
# over another component's class. A foreground component preempts a saturating
# bestEffort loop still in flight, a declared promotion applies within its
# ceiling, no component widens itself, and an undeclared instance runs at the
# declared default.
scheduling_class_check: sel4_pin_check
    python3 scripts/check/check-sel4-scheduling-class-plane.py

# C9.2: a bounded userspace wait set over one declared Notification —
# registration, badge demultiplexing, deterministic dispatch order, every
# ceiling refused, and a peer death observed through a declared source.
wait_set_check: sel4_pin_check
    python3 scripts/check/check-sel4-wait-set-plane.py


# C9.4: a userspace supervisor restarts a component under its declared attempt
# bound and growing backoff. The fault, exit, and unhealthy causes are
# distinguishable from both sides, every predecessor handle is refused while the
# declared configuration survives, an undeclared transition is refused without
# moving the state, and exhausting the bound leaves the instance in the declared
# terminal state with its next spawn refused.
lifecycle_restart_check: sel4_pin_check
    python3 scripts/check/check-sel4-lifecycle-restart-plane.py


# C9.5: a recorded run and a deterministic replay of it. The recorder captures
# its own clock reads, timer expiry, and lifecycle transition and derives typed
# outputs from them; the replayer answers every input from the recording rather
# than the live source, recomputes the outputs, and compares them field by field.
# Two boots of one image must produce identical declared traces. A truncated,
# reordered, or over-capacity stream is refused whole rather than partially
# replayed, and a component holding a right the generation classifies as an
# unrecorded nondeterminism source carries no determinism claim.
replay_check: sel4_pin_check
    python3 scripts/check/check-sel4-replay-plane.py

# C9.6: a simulated sensor -> controller -> actuator graph over the native
# fabric, exercising timer, stream, call, lifecycle, restart, and contention
# paths at once. The graph must run to completion under a declared best-effort
# CPU load with the declared scheduling order preserved; an injected controller
# restart must be bounded, reissue the controller's fabric authority, and let the
# graph resume; and deadline miss, timer expiry, liveliness loss, fault, peer
# loss, and cancellation must stay distinct at the userspace boundary. Asserted
# the way C8.15 asserts its parent close: one composition, both schedules,
# compared semantically rather than by marker presence alone.
robot_runtime_check: sel4_pin_check
    python3 scripts/check/check-sel4-robot-runtime-plane.py

# B22: build the channel-crossing image, boot it, and require that a graph
# minting more channels over its lifetime than `MAX_CHANNELS` holds at once
# still sends and receives on every live channel — including a pair held across
# the crossing and an end parked in `Transit` across it.
#
# A ninth image, on the same rule as the eight above: each gate boots the
# artifact it asserts about.
sel4_crossing_check: sel4_pin_check
    python3 scripts/check/check-sel4-crossing-plane.py

# B10 on seL4 (P5.4.10): init's resolved capability layout, per plane, against
# the fixtures frozen at product cutover.
#
# Boots every plane and reads one `[layout]` block each — a layout diff should
# be readable as a layout diff rather than inferred from a component failing,
# which is why this is separate from the planes' own gates.
#
# Regenerate with `just sel4_boot_layout_bless`; the resulting diff is the
# evidence that a layout change was intended.
sel4_boot_layout_check: sel4_pin_check
    python3 scripts/check/check-sel4-boot-layout.py

sel4_boot_layout_bless: sel4_pin_check
    python3 scripts/check/check-sel4-boot-layout.py --bless

# B40: every child's CSpace against the generation's admitted plan.
#
# Boots the unmutated boot plane, then rebuilds the root once per injected
# mutation and requires the audit to refuse each. Here a boot that *succeeds*
# under a mutation is the failure: it means the audit cannot see that
# deviation. Covers a declared capability missing, an extra one in an
# undeclared slot, one at the wrong slot, one aliased into two slots, and one
# carrying broader rights than admitted.
#
# Six root builds and six boots, so it is slower than the plane gates and is
# not part of `just test`.
sel4_capability_layout_check: sel4_pin_check
    python3 scripts/check/check-sel4-capability-layout.py

# B25 and P5.4.6: build the bounded-call image, boot it, and require the full
# C8.6 native-call plane plus the seL4 composition that makes it possible:
# init mints authenticated control pairs and transfers each participant's
# supervision handle to the broker after spawn. Every spawned component must
# then reach one clean terminal status.
sel4_call_check: sel4_pin_check
    python3 scripts/check/check-sel4-call-plane.py

# P5.4.7 and C8.7: build the native-operation image, boot it, and require the
# full bounded-operation surface with the shared broker and participants —
# correlation, authority denials, restart determinism, cancellation races,
# explicit-time expiry, peer-death settlement, parent-vouched supervision, and
# one clean exit per spawned task.
sel4_operation_check: sel4_pin_check
    python3 scripts/check/check-sel4-operation-plane.py

# P5.4.8 and C8.8: build the visibility image, boot it, and require filtered
# introspection plus declared interposition with the shared broker and
# participants — three callers with different bounded views, an ungranted
# caller that infers nothing, a telemetry path reaching its subscriber only
# through the declared proxy, and proxy death leaving the unrelated route live.
sel4_visibility_check: sel4_pin_check
    python3 scripts/check/check-sel4-visibility-plane.py

# B72: rewrite the frozen view fixture from an observed boot. The resulting diff
# is the evidence that a route-name or declared-QoS change was intended.
sel4_visibility_bless: sel4_pin_check
    python3 scripts/check/check-sel4-visibility-plane.py --bless

# C8.12: build the matrix image, boot it, and require the whole matching,
# visibility, and denial matrix at once — only the exact compatible tuple
# matched, alternate names and conflicting types kept distinct, every
# unauthorized operation refused with zero rights and no route identity, a
# filtered view that yields no route authority, and the declared proxy as the
# only telemetry path. Then boots the sibling generation carrying one
# incompatible QoS pair and requires admission to refuse it before any
# component launches.
sel4_matrix_check: sel4_pin_check
    python3 scripts/check/check-sel4-matrix-plane.py

# C8.13: build the traffic-plane image -- the identical C8.10 collision-free
# partition, carrying real stream, call, and operation traffic concurrently
# instead of parking -- boot it, and require every declared resource ceiling
# to emit bounded high-water evidence with nothing dropped or rejected.
sel4_traffic_check: sel4_pin_check
    python3 scripts/check/check-sel4-traffic-plane.py

# C8.13: build the saturation-plane image -- the identical traffic-action
# scenario against a fixture whose declared ceilings are tightened to the
# traffic plane's own observed peaks -- boot it, and require three of them to
# land exactly at their declared bound with nothing dropped, rejected, or
# deadlocked.
sel4_saturation_check: sel4_pin_check
    python3 scripts/check/check-sel4-saturation-plane.py

# C8.14: boot the identical concurrent traffic graph with the declared
# interposition hop injected to die, and require every degradation and terminal
# condition to stay bounded, distinguishable, and fully reclaimed — while every
# unaffected stream, call, and operation route completes anyway.
sel4_fault_check: sel4_pin_check
    python3 scripts/check/check-sel4-fault-plane.py

# C8.15: boot every C8 aggregate plane twice over one declared composition and
# require each to satisfy its own plane gate on both runs and to produce
# byte-identical semantic traces — the determinism claim no single-boot gate can
# make, and the parent close for the C8 track.
sel4_fabric_aggregate_check: sel4_pin_check
    python3 scripts/check/check-sel4-fabric-aggregate.py

# P5.4.9 and C8.10: build the full-graph image, boot it, and require every C8
# role to launch at once in one collision-free layout — the stream, call, and
# operation planes in disjoint slots, the fabric split into three bounded route
# workers, the unauthorized probe refused as its own task, and the whole graph
# coming to rest rather than finishing.
sel4_boot_check: sel4_pin_check
    python3 scripts/check/check-sel4-boot-plane.py

# P5.4.2a: boot the component-graph image with a virtio-blk device attached and
# require the root to reach it — retype a granule out of BootInfo device untyped
# memory, map it non-cacheably into its own VSpace, and identify the disk by
# register read. `sel4_root_boot_check` asserts the same probe finding nothing
# when no drive is attached; the pair is what makes the mechanism observed.
sel4_device_check: sel4_pin_check
    python3 scripts/check/check-sel4-device-plane.py

# P5.4.2c and M5.2/M5.3: boot the storage image with a virtio-blk device and
# require a *component* to move sectors through a capability its generation
# granted — a read returning the fixture's signature, a write and flush verified
# by read-back and confirmed durable in the image afterwards, and three refusal
# arms. The root-launched copy of the same component parks, so every arm is the
# spawned instance's.
sel4_storage_check: sel4_pin_check
    python3 scripts/check/check-sel4-storage-plane.py

# P5.4.2c and M5.4: boot the store image and require a *component* to validate a
# GPT, open a content-addressed object store, retrieve an object by hash with
# its payload re-verified, append a durable commit that preserves the previous
# root, deduplicate identical content, scrub every payload, and fall back to the
# older superblock when the newest is damaged. The oracle keeps all of this in
# the kernel; here the root mediates sectors and nothing else.
sel4_store_check: sel4_pin_check
    python3 scripts/check/check-sel4-store-plane.py

# P5.4.2c and M5.6: boot the rollback image and require a *component* to walk
# the BootState transition model on two durable slots — stage a pending
# generation, consume both attempts (the oracle's 2 -> 1 -> 0), roll back to
# known-good when they are exhausted, find rollback idempotent, refuse
# promotion with a wrong running identity or a stale release, and promote the
# running generation. Every commit is older-slot-first, so the previously
# selected root survives each transition.
sel4_rollback_check: sel4_pin_check
    python3 scripts/check/check-sel4-rollback-plane.py

# P5.4.2c and M5.9: boot the recovery image with TWO disks — the target, and a
# guard disk no capability names — and require a *component* to refuse two
# corrupt BootState slots, decode a signed recovery index, verify its whole
# state closure against the content-addressed store, and reconstruct a bootable
# root into both slots idempotently. The guard image is hashed before and after:
# M5.9 requires reconstruction to modify no device it was not explicitly
# granted, and only the image proves that.
sel4_recovery_plane_check: sel4_pin_check
    python3 scripts/check/check-sel4-recovery-plane.py

# P5.4.3 and M6.5: boot the generation image and require an *unprivileged*
# client to drive list, inspect, stage, select, and rollback through a
# management service that holds the plane's only block capability. Every
# refusal must leave BootState untouched — checked against the disk image, not
# just the status — and the client's own direct BlockTransact must be refused,
# because no slot it holds names a device.
sel4_generation_check: sel4_pin_check
    python3 scripts/check/check-sel4-generation-plane.py

# B35: build the immutable selector once, then boot fresh QEMU processes
# against one retained raw disk to prove durable attempt consumption,
# exhaustion rollback, health-only promotion, and sector-scoped mutation.
sel4_boot_selection_check: sel4_pin_check
    python3 scripts/check/check-sel4-boot-selection.py

# P5.4.3 and M6.3: boot the directory image and require a component holding one
# unscoped directory capability to derive narrower views that can neither escape
# their scope nor widen their rights, to be refused a stale commit and a scoped
# one, and to see its commits through every view of the shared namespace. The
# root owns this because a namespace root is unforgeable shared state with an
# atomic transition; what a directory contains stays in userspace.
sel4_directory_check: sel4_pin_check
    python3 scripts/check/check-sel4-directory-plane.py

# P5.4.3: `InputRead` mediation on its own, separate from M6.4's Dango session.
# A granted capability decodes the generation's scripted keys in order, an
# exhausted script ends its reader rather than blocking it — the arm that would
# have caught `WAIT_KIND_INPUT` resolving to a never-ready wait target — and a
# slot holding no input capability is refused.
sel4_input_check: sel4_pin_check
    python3 scripts/check/check-sel4-input-plane.py

# P5.4.3 and M6.6: boot the powerbox image and require a chooser holding
# directory authority the requester lacks to grant exactly one narrowed object
# capability on a selection gesture, with a provenance record — and to deny a
# request exceeding its own authority, refuse derivation past the granted
# scope, and mint nothing at all on cancellation. Both components are the
# oracle's, unmodified.
sel4_powerbox_check: sel4_pin_check
    python3 scripts/check/check-sel4-powerbox-plane.py

# P5.4.3, M6.4, and C10.4: boot the dango image and require a scripted console
# session to resolve three commands through the generation's profile and launch
# all three through the spawn service — the second carrying a derived working
# directory and a stdin endpoint, the third repeating the first — while an
# undeclared command is denied at resolution and a malformed line is a parse
# error. The repeat is what makes the free-frame census a conservation claim:
# the root's own allocator watermarks must return to where they stood after the
# previous cycle, with the released arena reused rather than replaced. Every
# component is the oracle's.
sel4_dango_check: sel4_pin_check
    python3 scripts/check/check-sel4-dango-plane.py

# P5.4.3 and M6.7: boot the transfer image with TWO devices — a read-only source
# carrying the manifest and a writable receiver — and require a generation to
# cross: digest, object closure, and travel policy all verified before any
# write, staged pending without disturbing the known-good root, and promoted
# only on health confirmation. The source is compared byte for byte afterwards.
sel4_transfer_check: sel4_pin_check
    python3 scripts/check/check-sel4-transfer-plane.py

# P5.4.3 and M6.3's service half: boot the filesystem image and require the
# shared `directory-probe`, unmodified, to resolve names, survive an interrupted
# root transition, commit a new one, and derive a narrowed subdirectory through
# a seL4 filesystem service backed by a userspace object store. The client hands
# its own directory view across with every request.
sel4_filesystem_check: sel4_pin_check
    python3 scripts/check/check-sel4-filesystem-plane.py

# P5.4.5 on seL4: C8.5's declared QoS policy, on the `sel4-qos` plane.
# Separate from `sel4_stream_check` because `sel4-stream.zti` grants no time
# capability, so its simulated-time clause is structurally unreachable — the
# arms this asserts cannot fire there at all.
sel4_qos_check: sel4_pin_check
    python3 scripts/check/check-sel4-qos-plane.py


# C8.11: boot the QoS plane and assert its bounded semantic trace -- every
# record structurally valid, the declared total tie order across data,
# acknowledgement, peer death, and time, the sink inside its declared depth
# with nothing dropped or rejected, one terminal record, and byte-identical
# artifacts across two boots of one fixed generation.
#
# It reuses `sel4-qos` deliberately: that is the only plane whose generation
# grants a clock, so it is the only one where all four order classes can occur.
# A dedicated fixture would assert the same property about the same worker.
sel4_trace_check: sel4_pin_check
    python3 scripts/check/check-sel4-trace-plane.py

# Prove the seL4 plane gates fail when their evidence is absent. This drives
# each gate's own marker table with a deleted marker, a transposition, and an
# appended failure marker. Needs no build and no QEMU: it asserts that the
# assertions have teeth, not that any image boots.
sel4_gate_control_check:
    python3 scripts/check/check-sel4-gate-controls.py

# Build the product component-graph image. This is the composition `init`
# launches in a real generation; the default `fixture` variant embeds the same
# generation but compiles the root's two-fixture proof path instead of the
# generation-graph launcher, so it is a verification artifact rather than the
# product (B61).
sel4_product_image: sel4_pin_check
    python3 scripts/build/build-sel4.py --component-graph --skip-pin-check

# Run the seL4 product image interactively on the pinned QEMU machine.
run: sel4_product_image
    qemu-system-aarch64 -machine virt,virtualization=on -cpu cortex-a53 -smp 1 \
        -m size=2048M -nographic -serial mon:stdio -kernel build/slime-sel4-graph.elf

run_release: run

# === Development Tools ===

fmt:
    cargo fmt --all
    cargo fmt --manifest-path slime-root/child/Cargo.toml

fmt_check:
    cargo fmt --all --check
    cargo fmt --manifest-path slime-root/child/Cargo.toml --check

# The runtime, the protocol library, the shared component library, the
# build-support crate, and all 52 component crates.
#
# The package list is derived from `cargo metadata` rather than written out,
# because neither shortcut works: there is no `components/Cargo.toml` — the
# component crates are members of the *root* workspace through the
# `components/bins/*` glob — so `cargo fmt --all` from here resolves the root
# manifest and would also format `slime-root`, duplicating `fmt_sel4_root` and
# making this recipe's name a lie. And unlike `cargo build`/`clippy`, `cargo fmt`
# accepts neither a `-p` glob ("package `slime-component-*` is not a member of
# the workspace") nor `--exclude` ("unexpected argument"). Deriving the list
# keeps a new component crate from having to edit this recipe.
[private]
_component_packages:
    @cd components && cargo metadata --format-version 1 --no-deps \
        | python3 -c 'import json,sys; print(" ".join("-p " + p["name"] for p in json.load(sys.stdin)["packages"] if p["name"].startswith("slime-component") or p["name"] in {"slime-rt","slime-proto","slime-components","slime-build-support"}))'

fmt_components:
    cd components && cargo fmt $(just _component_packages)

fmt_check_components:
    cd components && cargo fmt $(just _component_packages) --check


fmt_boot_contracts:
    cd boot-contracts && cargo fmt

fmt_check_boot_contracts:
    cd boot-contracts && cargo fmt -- --check

fmt_sel4_root:
    cargo fmt -p slime-root
    cargo fmt --manifest-path slime-root/child/Cargo.toml

fmt_check_sel4_root:
    cargo fmt -p slime-root -- --check
    cargo fmt --manifest-path slime-root/child/Cargo.toml -- --check

fmt_check_all: fmt_check

# Regenerate Rust block protocol bindings from the Zutai schema.
block_gen:
    python3 scripts/generate/generate-block-bindings.py

# Regenerate Rust component image bindings from the Zutai schema.
component_gen:
    python3 scripts/generate/generate-component-bindings.py

# Regenerate Rust + component store protocol bindings from the Zutai schema.
store_gen:
    python3 scripts/generate/generate-store-bindings.py

# Regenerate userspace spawn-service protocol bindings.
spawn_gen:
    python3 scripts/generate/generate-spawn-bindings.py

# Regenerate the root-service syscall ABI bindings and doc tables (B59). One
# declaration feeds `slime-root`, `components/runtime`, and `docs/syscall-abi.md`,
# so a renumbered operation cannot leave any of the three disagreeing.
syscall_abi_gen:
    python3 scripts/generate/generate-syscall-abi-bindings.py

# Regenerate the sample-descriptor protocol bindings (C7.6).
sample_descriptor_gen:
    python3 scripts/generate/generate-sample-descriptor-bindings.py

# Regenerate native interface-schema compiler constants and Rust bindings (C8.1).
interface_schema_gen:
    python3 scripts/generate/generate-interface-schema-bindings.py

# Regenerate the capability-transfer protocol bindings (C8.3).
capability_transfer_gen:
    python3 scripts/generate/generate-capability-transfer-bindings.py

# Regenerate the fabric stream framing bindings (C8.4).
fabric_stream_gen:
    python3 scripts/generate/generate-fabric-stream-bindings.py

# B46: regenerate the v2 shared-ring bindings from their contract.
fabric_ring_gen:
    python3 scripts/generate/generate-fabric-ring-bindings.py

# C8.11: regenerate the bounded semantic-trace bindings from their contract.
fabric_trace_gen:
    python3 scripts/generate/generate-fabric-trace-bindings.py

# C8.8: regenerate the visibility record bindings from their contract — the
# Rust records the broker encodes and the Python offsets the plane gate decodes
# them with (B72).
fabric_visibility_gen:
    python3 scripts/generate/generate-fabric-visibility-bindings.py

# Regenerate the fabric-graph resource bindings (C8.2); part of the boot set.
fabric_graph_gen: boot_gen

# Regenerate kernel + component generation-management protocol bindings.
generation_management_gen:
    python3 scripts/generate/generate-generation-management-bindings.py

# Regenerate userspace powerbox protocol bindings.
powerbox_gen:
    python3 scripts/generate/generate-powerbox-bindings.py

# Regenerate host constants for the CP6 component-SDK release record.
component_sdk_release_gen:
    python3 scripts/generate/generate-component-sdk-release-bindings.py

# Regenerate host constants for generation v2, kernel image, and BootState.
boot_gen:
    python3 scripts/generate/generate-boot-bindings.py

generation_gen: boot_gen

kernel_image_gen: boot_gen

bootstate_gen: boot_gen

# Exhaustively check the bounded BootState transition and interruption model.
bootstate_model_check:
    cargo build --release --manifest-path deps/zutai/Cargo.toml -q -p zutai-cli
    ZUTAI_STDLIB_ROOT=deps/zutai/stdlib deps/zutai/target/release/zutai-cli model-check contracts/bootstate/model/bootstate.zt

# Exhaustively check the bounded capability-rights algebra: narrow-only derive
# and spawn delegation, the consuming export/finalize/import/cancel path, and
# six mutations that must each produce a counterexample.
capability_rights_model_check:
    cargo build --release --manifest-path deps/zutai/Cargo.toml -q -p zutai-cli
    ZUTAI_STDLIB_ROOT=deps/zutai/stdlib deps/zutai/target/release/zutai-cli model-check contracts/capability-rights/model/capability-rights.zt

# The custom-kernel persistence gates were superseded by the userspace seL4
# planes. These aliases keep historical devlog gate identifiers resolvable.
bootstate_trace_check: bootstate_model_check sel4_rollback_check
    python3 scripts/check/check-bootstate-trace.py

release_trust_check: contracts_check sel4_rollback_check
    python3 scripts/check/check-release-trust.py

recovery_check: sel4_recovery_plane_check

# Validate devlog structure: entry layout, front matter, kind-required sections,
# roadmap/Justfile identifier resolution, index agreement, and link health.
devlog_check:
    python3 scripts/check/check-devlog.py

# RP0 format 1 (historical): exact Raspberry Pi 5 board/firmware/media and
# ROS 2 Jazzy DDSI-RTPS Profile 0 acceptance fixture. Retained as the record of
# what RP0/RP1 originally admitted; format 2 is the live contract.
rpi5_ros2_demo_contract_check:
    python3 scripts/check/check-rpi5-ros2-demo-contract.py

# RP0 format 2 (live): the same board/firmware/media contract with the transport
# family carried as data rather than frozen into the schema, selecting ROS 2
# Kilted over a bounded Zenoh Profile 0. Every wire constant -- the RIHS01 type
# hash, the data key expression, the per-sample CDR bytes, the 33-byte
# attachment -- is derived and compared rather than transcribed, and the hash
# implementation is itself validated against an upstream `ros2/rcl` fixture.
rpi5_ros2_demo_contract_v2_check:
    python3 scripts/check/check-rpi5-ros2-demo-contract-v2.py

# Validate the pinned generation manifest schema and fixtures.
contracts_check: bootstate_model_check capability_rights_model_check
    python3 scripts/check/check-contracts.py
    python3 scripts/generate/generate-spawn-bindings.py --check
    python3 scripts/check/check-boot-layout-resource.py
    # B42: lifecycle authority is a capability, so no wire record or public
    # runtime type may name a bare task id.
    python3 scripts/check/check-lifecycle-identity.py
    # B50: every generation this repository builds is v5. The manifest's own
    # `formatVersion` is the *manifest* schema's version and says nothing
    # about the wire format, so this builds each one and reads the magic.
    python3 scripts/check/check-generation-v5.py
    # B46: the v2 shared-ring bindings match their contract. Generated code
    # that has drifted from its schema is a hand-written wire format wearing a
    # `@generated` header.
    python3 scripts/generate/generate-fabric-ring-bindings.py --check
    # B59: the root-service ABI is one contract consumed by `slime-root` and
    # `components/runtime`, and `docs/syscall-abi.md` must document every label
    # it declares. Before this the two crates and the doc each held their own
    # copy of the table.
    python3 scripts/generate/generate-syscall-abi-bindings.py --check

# P0 target-profile and executable-artifact contract matrix.
architecture_contract_check: contracts_check
    python3 scripts/check/check-architecture-contract.py

# RP1 derives the demo's executable closure from the live format-2 contract, so
# it depends on that gate. Format 1's gate stays in the chain because RP0/RP1's
# recorded exit conditions were observed against it.
rpi5_artifact_check: architecture_contract_check rpi5_ros2_demo_contract_check rpi5_ros2_demo_contract_v2_check
    python3 scripts/check/check-rpi5-artifacts.py

# P4: configure, build, and install seL4 for the `bcm2712` Raspberry Pi 5
# platform, build the root child, root task, and loader against that prefix, and
# write `build/slime-sel4-bcm2712-rpi5.identity.json`. A separate prefix, cargo
# target directory, generation, image, and pinned artifact hash set from the
# qemu-arm-virt build, because it is a different kernel for a different board:
# every executable in it is admitted for `aarch64-rpi5` alone.
sel4_rpi5_image_check: sel4_pin_check
    python3 scripts/build/build-sel4.py --platform bcm2712-rpi5 --skip-pin-check

# P4: flatten the packaged RPi5 ELF into the exact boot files
# `sel4/pins.toml [bcm2712_rpi5].boot_files` pins — `kernel8.img` and
# `config.txt`. `objcopy -O binary` cannot do this: the loader's payload lives
# in program headers carrying no sections, so objcopy silently drops it and
# emits an image that boots nothing. Writes no block device.
rpi5_media_check: sel4_rpi5_image_check
    python3 scripts/build/build-rpi5-media.py

# P4: boot the pinned bytes on the named Raspberry Pi 5 and require ordered
# generation, timer, task, fault, and ready evidence on UART10 at the baud
# `contracts/rpi5-ros2-demo/v2` pins. Physical: it proves the media is this
# build's, then reads a real serial device. A QEMU pass cannot complete this
# milestone (roadmap invariant 8), so a missing board, adapter, or media is a
# failure and never a skip.
#
# Requires the operator to copy `build/rpi5-media/*` onto the FAT32 boot
# partition and reset the board:
#   just rpi5_media_check
#   cp build/rpi5-media/* /Volumes/<BOOT>/ && diskutil unmount /Volumes/<BOOT>
#   just rpi5_boot_check /dev/cu.usbserial-XXXX
rpi5_boot_check serial="": sel4_pin_check rpi5_artifact_check
    python3 scripts/check/check-rpi5-boot.py {{ if serial == "" { "" } else { "--serial " + serial } }}

# Bring-up aid, not a gate: print whatever the Pi 5's debug UART emits and
# assert nothing. Builds no artifacts and qualifies no board, so it answers the
# one question `rpi5_boot_check` cannot when the wire is silent — whether any
# byte reaches this host. Exits on its own after 10s of quiet or the timeout.
#
#   just rpi5_serial_monitor /dev/cu.usbserial-120
rpi5_serial_monitor serial timeout="120":
    python3 scripts/check/check-rpi5-boot.py --monitor --serial {{ serial }} --timeout {{ timeout }}

# Historical architecture gate backed by a real neutral-source boundary scan.
aarch64_boot_check: sel4_root_boot_check

# Historical trap gate. seL4 owns exception entry and the trap instruction;
# `slime-root/src/fault.rs` decodes its fault messages, which the root boot
# gate observes as `SLIME_ROOT child fault observed ... kind=VirtualMemory`.
aarch64_trap_check: sel4_root_boot_check

x86_portability_check:
    python3 scripts/check/check-architecture-portability.py

generation_check: contracts_check sel4_component_graph_check
    python3 scripts/check/check-generation-determinism.py

framework_safety_check:
    python3 scripts/check/check-framework-authority.py

# Physical Framework image production is intentionally unavailable. P4's board
# image now builds (`just sel4_rpi5_image_check`, `just rpi5_media_check`); what
# P4 still lacks is the observed removable-media boot, which `just
# rpi5_boot_check` requires and never emulates.
# Historical devlog identifiers remain explicit aliases to their product gates.
test: sel4_root_boot_check sel4_component_graph_check sel4_gate_control_check

product_boot_check: sel4_component_graph_check

generation_cmd_check: sel4_generation_check

powerbox_check: sel4_powerbox_check

shared_buffer_factory_check: sel4_loan_check

shared_buffer_accounting_check: sel4_loan_check

shared_buffer_mapping_check: sel4_loan_check

shared_buffer_loan_check: sel4_loan_check

interface_schema_check: contracts_check
    python3 scripts/check/check-interface-schema.py

# CP0's component-specification model: every component the reference generation
# declares has a schema-valid `contracts/component-spec/v1` record with a stable
# computed identity, and 37 named malformations are refused.
component_spec_check: contracts_check
    python3 scripts/check/check-component-spec.py

# CP1's generation derivation: `valid.zti` and `sel4-channel.zti` are generated
# from `contracts/system-spec/v1` sources plus the component-spec corpus, must
# regenerate byte-identically, and 17 named malformations are refused.
system_spec_check: component_spec_check
    python3 scripts/check/check-system-spec.py
    python3 scripts/generate/generate-generation-from-spec.py --check

# CP3's crate-per-component boundary: every component is its own workspace
# package with one binary and no private manifest parser, the allocator is
# scoped to the six crates that declare it and matches the builder's store
# group, every package carries a release-profile stanza, and no shared source
# remains in `components/bins`.
#
# The allocator arm is the one that needs a gate rather than review. Cargo
# unifies features across every package named in one invocation, so building a
# plain component beside a store component links a `#[global_allocator]` into
# the plain one — measured directly, as 6 heap symbols appearing in the
# `slime_rt` rlib a mixed invocation produced against 0 in a grouped one.
component_crate_split_check: component_spec_check
    python3 scripts/check/check-component-crate-split.py


# CP4's external-artifact path: the generation builder resolves one component
# through an explicit content-hash-bound ELF mapping, reports the source, signs
# and admits the mixed generation, and refuses hash-mismatched or malformed ELF
# bytes before a generation is signed.
external_component_admission_check: generation_check
    python3 scripts/check/check-external-component-admission.py

# CP5's out-of-tree development proof: export the versioned component SDK, commit
# it as a pinned git repository, consume it from a distinct RP4 component
# checkout, admit both content-bound ELFs into the demo generation, boot the
# exact signed generation, then remove the checkout and prove the in-tree
# fallback still boots. Since CP6 it consumes the repository-owned exporter
# rather than constructing its own bundle.
component_sdk_out_of_tree_check: external_component_admission_check
    python3 scripts/check/check-component-sdk-out-of-tree.py

# CP6's deterministic export: one checked-in exporter, invoked twice from the
# same source tree, produces byte-identical self-describing SDK trees. The
# identity is required to move for an allowlisted source and for a pin and to
# hold for two product-only files — without both halves it could be a constant
# or a digest of the whole repository — and the record must decode through its
# generated Zutai binding with every stated digest matching the emitted bytes.
component_sdk_export_check: external_component_admission_check
    python3 scripts/check/check-component-sdk-export.py

# CP7's permanent publication: one generated commit and one immutable signed tag
# per release, republishing an unchanged tree writes nothing, and the published
# commit regenerates byte-identically from the source commit its own record
# names. A hand edit in the mirror is refused rather than merged, which is what
# makes the mirror generated rather than a second source tree.
component_sdk_release_check: component_sdk_export_check
    python3 scripts/check/check-component-sdk-release.py

# CP8's platform assets: an external checkout builds target-qualified QEMU and
# RPi component ELFs from one immutable SDK release with `SEL4_PREFIX` poisoned,
# so a build that still succeeds can only have taken its prefix from the release
# record. The RPi arm is host-side qualification: the QEMU profile refuses that
# ELF as wrong-target, and no physical-board claim is made.
component_sdk_prefix_check: component_sdk_release_check
    python3 scripts/check/check-component-sdk-prefix.py

# CP9's version policy and matrix: two real immutable releases are classified,
# every scalar and structural compatibility axis is moved in isolation and must
# force its expected classification — including equal crate versions across a
# changed syscall ABI — and each published row is backed by a build plus the
# QEMU boot that observed it. An untested pairing reports unsupported.
component_sdk_compatibility_check: component_sdk_prefix_check
    python3 scripts/check/check-component-sdk-compatibility.py

# CP10's consumer lifecycle: a template consumer pins one release by full
# commit, upgrades to the next with its lockfile, prefix asset, and recorded
# identity in one diff, rebuilds and boots the content-bound generation,
# survives five injected failures with the prior pin intact, and reproduces the
# previous ELF and generation byte-for-byte on rollback.
component_sdk_upgrade_check: component_sdk_compatibility_check
    python3 scripts/check/check-component-sdk-upgrade.py

# CP2's runtime binding resolution: a component asks the root which of its own
# slots holds a named binding instead of compiling the number in. An unprefixed
# name is a manifest grant; `executable:`/`channel:` reach the boot layout's two
# identity domains for the bootstrap instance, which is what keeps a layout entry
# from shadowing a grant.
#
# The planes are the gate. `sel4_channel_check` asserts the denial arm from both
# the root's line and the component's, and `sel4_loan_check` plus
# `sel4_component_graph_check` boot components that resolve real bindings and
# would fail the rendezvous on a wrong answer. `test_sel4_root` covers the
# name-admissibility guards without a boot.
runtime_binding_resolution_check: test_sel4_root sel4_gate_control_check sel4_channel_check sel4_loan_check sel4_component_graph_check sel4_dango_check
    @echo "runtime binding resolution: the root resolved named bindings, namespaced boot-layout roles, and unambiguous capability roles for their own instances, refused an ungranted name, and every migrated plane kept its observed behavior"

fabric_manifest_check: contracts_check sel4_stream_check
    python3 scripts/check/check-fabric-manifest.py

fabric_authority_check: sel4_stream_check

fabric_stream_check: sel4_stream_check

fabric_qos_check: sel4_qos_check

fabric_call_check: sel4_call_check

fabric_operation_check: sel4_operation_check

fabric_visibility_check: sel4_visibility_check

data_fabric_profile_check: contracts_check
    python3 scripts/check/check-data-fabric-profile.py

data_fabric_boot_check: sel4_boot_check

# C8.12's roadmap-named gate.
data_fabric_matrix_check: sel4_matrix_check

# C8.13's roadmap-named gate.
data_fabric_traffic_check: sel4_traffic_check

# C8.13's roadmap-named gate for the saturation half.
data_fabric_saturation_check: sel4_saturation_check

# C8.14's roadmap-named gate.
data_fabric_fault_check: sel4_fault_check

# C8.15's roadmap-named gate, and the C8 parent close.
data_fabric_check: sel4_fabric_aggregate_check

# C8.11's roadmap-named gate. Both halves: the contract and its declared sink
# bounds are validated on the host, and the emitted trace is observed on the
# plane that actually advances a clock.
data_fabric_trace_check: contracts_check sel4_trace_check
    python3 scripts/generate/generate-fabric-trace-bindings.py --check

sample_descriptor_check: contracts_check sel4_sample_check

sample_plane_check: sel4_sample_check

sample_plane_live_check: sel4_sample_check

boot_layout_check: sel4_boot_layout_check

spawn_service_check: sel4_spawn_check

# RP2's roadmap identifier, an alias onto the plane gate it names, the same way
# `data_fabric_matrix_check` aliases `sel4_matrix_check`. The roadmap's
# "Planned verification target" spelled this name before the plane existed, so it
# stays resolvable while the plane keeps the `sel4_<stem>_check` convention.
rpi5_arm_slice_check: sel4_demo_check

# Historical M5/M6 verification identifiers. The custom-kernel recipes are
# retired; these names resolve to the seL4 gates that now own the contracts.
spawn_prereq_check: sel4_spawn_check

storage_cap_check: sel4_storage_check sel4_generation_check sel4_transfer_check

# Physical Framework/NVMe qualification did not move to the seL4 QEMU product.
# Keep the documented identifiers, but fail closed rather than turning a
# missing physical transport/evidence gate into a false pass.
storage_nvme_read_check:
    @echo "storage_nvme_read_check: unavailable after custom-kernel retirement; M5.7 requires a seL4 NVMe path and observed Framework evidence" >&2
    @exit 1

framework_inventory_check:
    @echo "framework_inventory_check: unavailable after custom-kernel retirement; H1 requires a seL4 Framework image and evidence/framework-inventory.jsonl" >&2
    @exit 1

dango_check: sel4_dango_check

directory_check: sel4_filesystem_check

transfer_check: sel4_transfer_check

storage_read_check: sel4_storage_check

storage_write_check: sel4_storage_check

storage_fault_check: sel4_storage_check

storage_store_check: sel4_store_check

rollback_check: sel4_rollback_check

lint: lint_all



lint_boot_contracts:
    cd boot-contracts && cargo clippy --all-features -- -D warnings

# Host-target clippy for every cutover component crate that does not require a
# built seL4 prefix. The product-target pass remains `lint_sel4_root`.
lint_components_host:
    #!/usr/bin/env bash
    set -euo pipefail
    host="$(rustc -vV | sed -n 's/^host: //p')"
    cargo clippy -p slime-proto --target "$host" -- -D warnings

# Clippy for the seL4 product crates: the root task, its child, and the
# seL4-enabled component runtime. Unlike the format gates these compile, so
# they need the installed seL4 prefix (libsel4 headers and config), the
# rust-sel4 toolchain pin, the custom target specs, and the child ELF the root
# task embeds at compile time. `sel4_qemu_image_check` produces both; this gate
# refuses to run against a missing one rather than silently linting a
# different configuration.
lint_sel4_root clippy_flags='-D warnings':
    #!/usr/bin/env bash
    set -euo pipefail
    prefix="$PWD/build/sel4-prefix"
    if [ ! -f "$prefix/libsel4/include/kernel/gen_config.json" ]; then
        echo "lint_sel4_root: no installed seL4 prefix at $prefix; run 'just sel4_qemu_image_check' first" >&2
        exit 1
    fi
    # Platform-qualified since `0dd7d0c` gave the builder a second platform:
    # `build-sel4.py` writes each platform's child under
    # `build/sel4-cargo/<platform>/child/`. The unqualified path this named
    # before only resolved on a checkout still holding a pre-`0dd7d0c` build,
    # so the gate passed locally and refused on any clean tree.
    child_elf="$PWD/build/sel4-cargo/qemu-arm-virt/child/aarch64-sel4-minimal/release/slime-root-child.elf"
    if [ ! -f "$child_elf" ]; then
        echo "lint_sel4_root: no root child ELF at $child_elf; run 'just sel4_qemu_image_check' first" >&2
        exit 1
    fi
    toolchain="$(python3 -c "import pathlib,tomllib; print(tomllib.loads(pathlib.Path('sel4/pins.toml').read_text())['rust_sel4']['toolchain'])")"
    targets="$PWD/deps/rust-sel4/support/targets"
    export SEL4_PREFIX="$prefix" RUSTUP_TOOLCHAIN="$toolchain" CHILD_ELF="$child_elf"
    build_std=(-Z json-target-spec -Z build-std=core,alloc,compiler_builtins -Z build-std-features=compiler-builtins-mem)
    cargo clippy -p slime-root --target "$targets/aarch64-sel4-roottask-minimal.json" \
        --target-dir build/sel4-cargo/lint-root "${build_std[@]}" -- {{clippy_flags}}
    cargo clippy --manifest-path slime-root/child/Cargo.toml -p slime-root-child \
        --target "$targets/aarch64-sel4-minimal.json" \
        --target-dir build/sel4-cargo/lint-child "${build_std[@]}" -- {{clippy_flags}}
    cd components
    # Three invocations, not one, for the same reason the *builder* uses three:
    # Cargo unifies features across every package named in one invocation, and
    # `slime-rt`'s two allocators are mutually exclusive — `heap` and
    # `private-heap` each register a `#[global_allocator]`, and one link holds
    # one. A single `-p 'slime-component-*'` pass therefore enables both and dies
    # on "cannot define a new global allocator" rather than reporting lints.
    #
    # The groups are derived from `cargo metadata`, not listed here: a component
    # declaring an allocator moves group by editing its own manifest, and
    # `just component_crate_split_check` pins that the builder's grouping agrees
    # with what the crates declare.
    groups="$(cargo metadata --format-version 1 --no-deps | python3 -c '
    import json, sys
    packages = json.load(sys.stdin)["packages"]
    groups = {"plain": [], "heap": [], "private-heap": []}
    for package in packages:
        if not package["name"].startswith("slime-component-"):
            continue
        features = set()
        for dependency in package["dependencies"]:
            if dependency["name"] == "slime-rt":
                features.update(dependency.get("features") or [])
        key = "heap" if "heap" in features else "private-heap" if "private-heap" in features else "plain"
        groups[key].append(package["name"])
    for key, names in groups.items():
        if names:
            print(key + " " + " ".join("-p " + name for name in sorted(names)))
    ')"
    if ! grep -q '[^[:space:]]' <<< "$groups"; then
        echo "lint_sel4_root: cargo metadata named no slime-component-* package" >&2
        exit 1
    fi
    while read -r group packages; do
        [ -n "$group" ] || continue
        # `slime-rt` is named in *every* group, deliberately. Its two allocator
        # modules are behind the features this grouping exists to separate, so
        # naming it only in the plain group would cfg both of them out of every
        # lint pass — including the new unsafe pointer arithmetic in
        # `private_heap.rs`, which is the code in this tree that most needs
        # `just lint_pedantic`'s `undocumented_unsafe_blocks`. A path dependency
        # cargo merely checks is not linted. `slime-proto`/`slime-components`
        # declare no allocator feature, so they stay with the plain group alone.
        shared="-p slime-rt"
        if [ "$group" = plain ]; then
            shared="$shared -p slime-proto -p slime-components"
        fi
        SLIME_TARGET_PROFILE=aarch64-sel4-qemu-virt \
            cargo clippy $shared $packages \
            --target "$targets/aarch64-sel4-minimal.json" \
            --target-dir "../build/sel4-cargo/lint-components-$group" "${build_std[@]}" -- {{clippy_flags}}
    done <<< "$groups"

# Every surviving workspace crate plus the seL4 product crates.
lint_all: lint_boot_contracts lint_components_host lint_sel4_root

# Historical component lint identifiers now resolve to the product lint.
lint_components: lint_sel4_root

lint_fix_components: lint_sel4_root

# Dependency advisories (RUSTSEC), duplicate/wildcard bans, license
# allowlist, and source pinning. Config in deny.toml.
deny:
    cargo-deny check

# Unused-dependency scan; scoped to surviving workspace crates.
machete:
    cargo-machete boot-contracts components slime-root

# UB check for the host-testable crates, on the actual host triple. Components
# has a bare-metal default target, so both invocations override it explicitly.
miri:
    #!/usr/bin/env bash
    set -euo pipefail
    host="$(rustc -vV | sed -n 's/^host: //p')"
    cd boot-contracts
    cargo miri test --all-features --target "$host"
    cd ../components
    cargo miri test --target "$host" -p slime-proto

# Host-side unit tests for the crates that need neither QEMU nor a built seL4
# prefix. Use the actual host triple: hardcoding Linux makes the gate fail on
# Darwin, while omitting `--target` makes components pick its bare-metal default.
#
# `slime-build-support` is a host crate, so its tests need no `--target`
# override. They are here because CP3 is what made them runnable at all: while
# the manifest parser lived in `components/bins/build.rs`, `cargo test` never
# built it as a test target, so its two `#[test]`s were compiled and run by
# nothing — and had rotted into asserting real slot numbers against a fixture
# that parsed as a single block, so they could only have failed. A parser with
# no test is how a wrong slot number reaches a component image.
test_host:
    #!/usr/bin/env bash
    set -euo pipefail
    host="$(rustc -vV | sed -n 's/^host: //p')"
    cargo test --manifest-path boot-contracts/Cargo.toml --all-features
    (cd components && cargo test --target "$host" -p slime-proto)
    cargo test -p slime-build-support

# B23: `slime-root`'s mechanism modules, run on the host.
#
# They were compiled by nothing and run by nothing until the crate grew a lib
# target: `main.rs` is unconditionally `no_std`/`no_main`, and the package built
# only for a seL4 JSON target with no `libtest`. The library is the same code
# the seL4 image links, so a pass here is evidence about the shipped root.
#
# Needs the installed seL4 prefix because `sel4` reads `libsel4`'s generated
# config at build time even on a host target; the gate refuses rather than
# silently skipping, on `lint_sel4_root`'s rule. It runs no seL4 syscall — the
# tests exercise the state machines, and behavior needing a live kernel stays
# the `sel4_*` gates' job.
#
# The count is asserted so a module that stops being covered is visible, which
# is B23's exit condition. Raise it deliberately when tests are added.
test_sel4_root:
    #!/usr/bin/env bash
    set -euo pipefail
    prefix="$PWD/build/sel4-prefix"
    if [ ! -f "$prefix/libsel4/include/kernel/gen_config.json" ]; then
        echo "test_sel4_root: no installed seL4 prefix at $prefix; run 'just sel4_qemu_image_check' first" >&2
        exit 1
    fi
    # B70: 129 -> 130. `generation` gained
    # `an_interposition_hop_resolves_to_the_declared_component_name`, covering
    # the root's resolution of a declared interposition hop's identity back to
    # a generation instance name. It replaces an `assert_declared_chain` that
    # lived in each fabric broker and compared a build-time table against a
    # constant compiled beside it.
    #
    # B70: 130 -> 131. `ipc` gained
    # `owned_minted_names_are_their_own_namespace`, covering the
    # `owned-minted:` axis that answers `mintedBindings` from the owner's side
    # rather than the holder's. It pins the dispatch property the two arms
    # depend on -- neither prefix is a prefix of the other -- so an owner's
    # question cannot be answered against a holder's instance index.
    #
    # C10.1: 131 -> 146. The new `private_memory` module contributes twelve
    # tests over the growth admission ordering, the deny-by-default region, the
    # quota clamp, the root-wide ceiling, reclamation idempotence, and the
    # arena frame plan; `child_vspace` goes from eight to nine for the reserved
    # window's guard and alignment; and `object_allocator` gains two for the
    # arena slot table's release invariant, which had no inverse before the
    # growth unwind needed one. Two of `child_vspace`'s existing headroom tests
    # were rewritten rather than added to, because the mapped span a child
    # receives now includes that window.
    #
    # C10.2: 146 -> 149. `generation` gains three, covering the private-memory
    # budget's admission against this root's own ceilings: a satisfiable budget
    # at exactly the per-task reservation is admitted, a quota above that
    # reservation is refused rather than clamped at growth, and B8's aggregate
    # arm refuses holders that each fit but cannot all peak at once.
    #
    # C10.4: 149 -> 152. `private_memory` gains three, covering the one shared
    # resource the two memory planes still have — the child's address space.
    # A mapping anywhere in the reservation overlaps even where no page is
    # backed yet (so the answer cannot depend on how far an allocator has
    # grown), a mapping touching either boundary from outside does not (so the
    # guard granule below the window stays usable), and a denied region
    # overlaps nothing (so a component with no declared quota is not a
    # component that may not map).
    #
    # C9.1: 152 -> 158. The new `clock` module contributes five tests over
    # independently grantable authorities, per-task timer quota isolation,
    # termination cleanup, reuse of live-authority slots across lifetime task
    # ids, and stale expiry suppression; `ipc` adds the sixth for exact clock
    # request shapes.
    #
    # C9.2: 158 -> 160. The new `wait_set` module contributes both: a task is
    # declared once and its row survives a source-free declaration (so
    # `clear_task` answers about a live task rather than about whether it was
    # ever declared), and the per-waiter supervision ceiling is the contract's
    # own source ceiling rather than a second number this module picked.
    # Direction 24: 183 -> 184. `graph` gains the capability-rights partition
    # test for root-only supervision rights and declared-but-ungated rights.
    expected=184
    # Pinned rather than ambient, on `lint_sel4_root`'s rule: this build
    # consumes the installed seL4 prefix, so it must use the toolchain that
    # prefix was produced against. `rust-toolchain.toml`'s default is a
    # different nightly, and a gate whose result depends on which shell you are
    # in is the property `sel4_pin_check` exists to prevent.
    toolchain="$(python3 -c "import pathlib,tomllib; print(tomllib.loads(pathlib.Path('sel4/pins.toml').read_text())['rust_sel4']['toolchain'])")"
    host="$(rustc -vV | sed -n 's/^host: //p')"
    mkdir -p build
    capture="build/slime-root-host-tests.txt"
    # Not captured by a command substitution: a build failure must surface as
    # itself. `set -e` ends the recipe here rather than letting an empty capture
    # become a count mismatch reporting "ran  tests" for a compile error. The
    # transcript lands under `build/` rather than a fixed `/tmp` name, which two
    # checkouts or two concurrent runs would share.
    SEL4_PREFIX="$prefix" RUSTUP_TOOLCHAIN="$toolchain" \
        cargo test -p slime-root --target "$host" --lib -- --format=terse \
        | tee "$capture"
    output="$(cat "$capture")"
    actual="$(printf '%s\n' "$output" | sed -n 's/^running \([0-9]*\) test.*/\1/p' | head -1)"
    if [ -z "$actual" ]; then
        echo "test_sel4_root: cargo printed no 'running N tests' line; the harness did not start" >&2
        exit 1
    fi
    if [ "$actual" != "$expected" ]; then
        echo "test_sel4_root: ran $actual tests, expected $expected; a module lost or gained coverage (B23)" >&2
        exit 1
    fi
    # Belt and braces: the count says the harness found them, this says they
    # passed. `cargo test`'s own exit status covers it, but a filtered or
    # ignored run would still print a count.
    if ! printf '%s\n' "$output" | grep -q "^test result: ok\. $expected passed; 0 failed"; then
        echo "test_sel4_root: the run did not report $expected passed and 0 failed" >&2
        exit 1
    fi
    echo "slime-root host tests: $actual/$expected across 19 modules"

# Python lint for the host-side build/check/generate scripts. Config in ruff.toml.
ruff:
    ruff check scripts/

ruff_fix:
    ruff check scripts/ --fix

# Spell-check sources and docs. Config in _typos.toml.
typos:
    typos

# Advisory correctness lint set documented by the workspace policy.
lint_pedantic:
    just lint_sel4_root '-D warnings -W clippy::undocumented_unsafe_blocks -W clippy::cast_possible_truncation'
