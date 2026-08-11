"""B10 boot capability layout resource.

Encodes which capability slot of the bootstrap component holds which role,
under which name, with which rights, for one generation number. The generation
builder serializes the resource, component build scripts render matching Rust
constants, and `slime-root` resolves the declared roles when launching init.

The retained fixtures freeze the layouts observed at the P5 product cutover.
They are deliberately a transcription rather than a redesign: layout changes
must remain reviewable slot-by-slot, and the seL4 boot-layout gate exercises
every product plane.

A layout entry names a role, not a concrete mechanism object. Slot 9 is storage
authority; whether it resolves to a block device or an object store depends on
the admitted generation and discovered devices.
"""

import struct

from boot_contracts import (
    BOOT_LAYOUT_ENTRY,
    BOOT_LAYOUT_ENTRY_BYTES,
    BOOT_LAYOUT_HEADER,
    BOOT_LAYOUT_HEADER_BYTES,
    BOOT_LAYOUT_MAGIC,
    BOOT_LAYOUT_ROLE_DIRECTORY_ROOT,
    BOOT_LAYOUT_ROLE_ENDPOINT_CLIENT,
    BOOT_LAYOUT_ROLE_ENDPOINT_FACTORY,
    BOOT_LAYOUT_ROLE_ENDPOINT_SERVICE,
    BOOT_LAYOUT_ROLE_EXECUTABLE,
    BOOT_LAYOUT_ROLE_GENERATION_CONTROL,
    BOOT_LAYOUT_ROLE_INPUT,
    BOOT_LAYOUT_ROLE_OBJECT_STORE,
    BOOT_LAYOUT_ROLE_SHARED_BUFFER_FACTORY,
    BOOT_LAYOUT_ROLE_STORAGE_CAPABILITY,
    BOOT_LAYOUT_ROLE_TRANSFER_RECEIVER,
    BOOT_LAYOUT_ROLE_TRANSFER_SOURCE,
    BOOT_LAYOUT_VERSION,
    MAX_BOOT_LAYOUT_ENTRIES,
    sha256,
)

ROLE = {
    "endpoint-factory": BOOT_LAYOUT_ROLE_ENDPOINT_FACTORY,
    "shared-buffer-factory": BOOT_LAYOUT_ROLE_SHARED_BUFFER_FACTORY,
    "executable": BOOT_LAYOUT_ROLE_EXECUTABLE,
    "endpoint-client": BOOT_LAYOUT_ROLE_ENDPOINT_CLIENT,
    "endpoint-service": BOOT_LAYOUT_ROLE_ENDPOINT_SERVICE,
    "object-store": BOOT_LAYOUT_ROLE_OBJECT_STORE,
    "directory": BOOT_LAYOUT_ROLE_DIRECTORY_ROOT,
    "input": BOOT_LAYOUT_ROLE_INPUT,
    "generation-control": BOOT_LAYOUT_ROLE_GENERATION_CONTROL,
    "storage-capability": BOOT_LAYOUT_ROLE_STORAGE_CAPABILITY,
    "transfer-receiver": BOOT_LAYOUT_ROLE_TRANSFER_RECEIVER,
    "transfer-source": BOOT_LAYOUT_ROLE_TRANSFER_SOURCE,
}

# Roles that identify a specific component or channel. The rest are singular --
# there is one endpoint factory, one input device -- so a name would add nothing
# to distinguish them. Must agree with `Role::is_named` in
# `boot-contracts/src/boot_layout.rs`.
NAMED_ROLES = {"executable", "endpoint-client", "endpoint-service"}

# An endpoint label ending `-service` is the service half; every other endpoint
# is the client half. Four pairs predate the convention -- `console-output` /
# `dango-output`, `dango-spawn` / `service-spawn`, `sample-lender-side` /
# `sample-receiver-side` -- and are recorded as client halves. Nothing depends
# on the client/service distinction being semantically right for those; the
# label is the lookup key, and the role is carried for validation only. What
# matters is that this file and the kernel agree.

BASE_LAYOUT = (
    (0, 'endpoint-factory', None, 0x20000),
    (1, 'executable', 'console', 0x10008),
    (2, 'endpoint-client', 'console-output', 0x6),
    (3, 'executable', 'dango', 0x10008),
    (4, 'endpoint-client', 'dango-output', 0x5),
    (5, 'executable', 'spawn-service', 0x10008),
    (6, 'executable', 'sysinfo', 0x10008),
    (7, 'executable', 'echo-agent', 0x10008),
    (8, 'executable', 'storage-probe', 0x10008),
    (9, 'storage-capability', None, 0x404),
    (10, 'executable', 'generation-manager', 0x10008),
    (11, 'generation-control', None, 0xc004),
    (12, 'endpoint-client', 'dango-spawn', 0x7),
    (13, 'endpoint-client', 'service-spawn', 0x7),
    (14, 'executable', 'filesystem-service', 0x10008),
    (15, 'executable', 'directory-probe', 0x10008),
    (16, 'endpoint-client', 'directory-client', 0x7),
    (17, 'endpoint-service', 'directory-service', 0x7),
    (18, 'object-store', None, 0x3004),
    (19, 'directory', None, 0x780004),
    (20, 'input', None, 0x800004),
    (21, 'executable', 'generation-list', 0x10008),
    (22, 'executable', 'generation-inspect', 0x10008),
    (23, 'executable', 'generation-stage', 0x10008),
    (24, 'executable', 'generation-select', 0x10008),
    (25, 'executable', 'generation-rollback', 0x10008),
    (26, 'endpoint-client', 'generation-list-client', 0x7),
    (27, 'endpoint-client', 'generation-inspect-client', 0x7),
    (28, 'endpoint-client', 'generation-stage-client', 0x7),
    (29, 'endpoint-client', 'generation-select-client', 0x7),
    (30, 'endpoint-client', 'generation-rollback-client', 0x7),
    (31, 'endpoint-service', 'generation-list-service', 0x7),
    (32, 'endpoint-service', 'generation-inspect-service', 0x7),
    (33, 'endpoint-service', 'generation-stage-service', 0x7),
    (34, 'endpoint-service', 'generation-select-service', 0x7),
    (35, 'endpoint-service', 'generation-rollback-service', 0x7),
    (36, 'executable', 'powerbox-chooser', 0x10008),
    (37, 'executable', 'powerbox-probe', 0x10008),
    (38, 'endpoint-client', 'powerbox-client', 0x7),
    (39, 'endpoint-service', 'powerbox-service', 0x7),
    (40, 'shared-buffer-factory', None, 0x1000004),
    (41, 'executable', 'sample-lender', 0x10008),
    (42, 'executable', 'sample-receiver', 0x10008),
    (43, 'endpoint-client', 'sample-lender-side', 0x7),
    (44, 'endpoint-client', 'sample-receiver-side', 0x7),
    (45, 'executable', 'fabric-service', 0x10008),
    (46, 'executable', 'fabric-publisher', 0x10008),
    (47, 'executable', 'fabric-subscriber', 0x10008),
    (48, 'executable', 'fabric-intruder', 0x10008),
    (49, 'executable', 'fabric-publisher-b', 0x10008),
    (50, 'executable', 'fabric-subscriber-b', 0x10008),
    (51, 'endpoint-client', 'fabric-publisher-client', 0x7),
    (52, 'endpoint-client', 'fabric-subscriber-client', 0x7),
    (53, 'endpoint-client', 'fabric-intruder-client', 0x7),
    (54, 'endpoint-client', 'fabric-publisher-b-client', 0x7),
    (55, 'endpoint-client', 'fabric-subscriber-b-client', 0x7),
    (56, 'endpoint-service', 'fabric-publisher-service', 0x7),
    (57, 'endpoint-service', 'fabric-subscriber-service', 0x7),
    (58, 'endpoint-service', 'fabric-intruder-service', 0x7),
    (59, 'endpoint-service', 'fabric-publisher-b-service', 0x7),
    (60, 'endpoint-service', 'fabric-subscriber-b-service', 0x7),
    # B16's supervision plane. Appended last, and deliberately: every profile
    # that does not declare `supervision-child` drops this entry, and dropping
    # the highest slot renumbers nothing, so the nine existing gates keep their
    # slots byte-for-byte. Only `sel4-supervision.zti` declares it, and that
    # profile prunes to 17 slots.
    #
    # `0x1000c`, not the usual `0x10008`: the extra `0x4` is `RIGHT_TRANSFER`,
    # which is what makes the supervision handle this executable's spawns return
    # movable. The layout and the fixture must agree bit for bit (B10), so the
    # grant's `transferable = true` has to be matched here.
    (61, 'executable', 'supervision-child', 0x1000c),
    # B38 deliberate-fault executable. Only the reclamation fixture declares
    # it, so every other layout prunes the row without renumbering survivors.
    (63, 'executable', 'reclamation-fault', 0x10008),
    # B22's channel-crossing plane, appended last on the same rule: only
    # `sel4-crossing.zti` declares `crossing-peer`, every other profile drops
    # this row, and dropping the highest slot renumbers nothing. That profile
    # prunes to 16 slots — the role rows and channel halves belong to no
    # component and are always kept, so this row lands renumbered at 15, which
    # is the slot the gate's `spawn authorized` marker records.
    #
    # Together with B38's row this fills all `MAX_BOOT_LAYOUT_ENTRIES` (64).
    # Further executable rows require raising the format ceiling deliberately.
    (62, 'executable', 'crossing-peer', 0x10008),
)

# generation 2 (storage-write)
OVERRIDE_2 = (
    (8, 'executable', 'storage-writer', 0x10008),
    (9, 'storage-capability', None, 0xc04),
)

# generation 3 (storage-fault)
OVERRIDE_3 = (
    (8, 'executable', 'storage-fault-probe', 0x10008),
    (9, 'storage-capability', None, 0xc04),
)

# generation 4 (storage-store)
OVERRIDE_4 = (
    (8, 'executable', 'storage-store-probe', 0x10008),
    (9, 'object-store', None, 0x3004),
)

# generation 13 (fabric-qos)
OVERRIDE_13 = (
    (61, 'endpoint-client', 'fabric-time-client', 0x7),
    (62, 'endpoint-service', 'fabric-time-service', 0x7),
)

# generation 14 (fabric-call)
OVERRIDE_14 = (
    (45, 'executable', 'fabric-service', 0x1000c),
    (46, 'executable', 'fabric-call-client', 0x1000c),
    (47, 'executable', 'fabric-call-client-b', 0x1000c),
    (48, 'executable', 'fabric-call-time', 0x10008),
    (49, 'executable', 'fabric-call-server', 0x1000c),
    (51, 'endpoint-client', 'fabric-call-client-control', 0x7),
    (52, 'endpoint-client', 'fabric-call-client-b-control', 0x7),
    (53, 'endpoint-client', 'fabric-call-time-control', 0x7),
    (54, 'endpoint-client', 'fabric-call-server-control', 0x7),
    (56, 'endpoint-service', 'fabric-call-client-service', 0x7),
    (57, 'endpoint-service', 'fabric-call-client-b-service', 0x7),
    (58, 'endpoint-service', 'fabric-call-time-service', 0x7),
    (59, 'endpoint-service', 'fabric-call-server-service', 0x7),
    (61, 'endpoint-client', 'fabric-call-phase-time', 0x6),
    (62, 'endpoint-client', 'fabric-call-phase-client', 0x5),
)

# generation 15 (fabric-operation)
OVERRIDE_15 = (
    (45, 'executable', 'fabric-service', 0x1000c),
    (46, 'executable', 'fabric-op-client', 0x1000c),
    (47, 'executable', 'fabric-op-client-b', 0x1000c),
    (48, 'executable', 'fabric-op-server', 0x1000c),
    (49, 'executable', 'fabric-op-time', 0x1000c),
    (50, 'executable', 'fabric-op-client-b-restart', 0x1000c),
    (51, 'endpoint-client', 'fabric-op-client-control', 0x7),
    (52, 'endpoint-client', 'fabric-op-client-b-control', 0x7),
    (53, 'endpoint-client', 'fabric-op-time-control', 0x7),
    (54, 'endpoint-client', 'fabric-op-server-control', 0x7),
    (56, 'endpoint-service', 'fabric-op-client-service', 0x7),
    (57, 'endpoint-service', 'fabric-op-client-b-service', 0x7),
    (58, 'endpoint-service', 'fabric-op-time-service', 0x7),
    (59, 'endpoint-service', 'fabric-op-server-service', 0x7),
)

FABRIC_BOOT_LAYOUT = (
    (0, 'endpoint-factory', None, 0x20004),
    (1, 'shared-buffer-factory', None, 0x1000004),
    (2, 'executable', 'fabric-service', 0x10008),
    (3, 'executable', 'fabric-call-worker', 0x10008),
    (4, 'executable', 'fabric-op-worker', 0x10008),
    (5, 'executable', 'fabric-publisher', 0x1000c),
    (6, 'executable', 'fabric-subscriber', 0x1000c),
    (7, 'executable', 'fabric-publisher-b', 0x1000c),
    (8, 'executable', 'fabric-subscriber-b', 0x1000c),
    (9, 'executable', 'fabric-observer', 0x1000c),
    (10, 'executable', 'fabric-probe', 0x1000c),
    (11, 'executable', 'fabric-proxy', 0x1000c),
    (12, 'executable', 'fabric-call-client', 0x1000c),
    (13, 'executable', 'fabric-call-client-b', 0x1000c),
    (14, 'executable', 'fabric-call-server', 0x1000c),
    (15, 'executable', 'fabric-call-time', 0x1000c),
    (16, 'executable', 'fabric-op-client', 0x1000c),
    (17, 'executable', 'fabric-op-client-b', 0x1000c),
    (18, 'executable', 'fabric-op-server', 0x1000c),
    (19, 'executable', 'fabric-op-time', 0x1000c),
    (20, 'executable', 'fabric-op-client-b-restart', 0x1000c),
    (21, 'endpoint-client', 'fabric-publisher-control', 0x7),
    (22, 'endpoint-service', 'fabric-publisher-control-service', 0x7),
    (23, 'endpoint-client', 'fabric-subscriber-control', 0x7),
    (24, 'endpoint-service', 'fabric-subscriber-control-service', 0x7),
    (25, 'endpoint-client', 'fabric-publisher-b-control', 0x7),
    (26, 'endpoint-service', 'fabric-publisher-b-control-service', 0x7),
    (27, 'endpoint-client', 'fabric-subscriber-b-control', 0x7),
    (28, 'endpoint-service', 'fabric-subscriber-b-control-service', 0x7),
    (29, 'endpoint-client', 'fabric-observer-control', 0x7),
    (30, 'endpoint-service', 'fabric-observer-control-service', 0x7),
    (31, 'endpoint-client', 'fabric-probe-control', 0x7),
    (32, 'endpoint-service', 'fabric-probe-control-service', 0x7),
    (33, 'endpoint-client', 'fabric-proxy-control', 0x7),
    (34, 'endpoint-service', 'fabric-proxy-control-service', 0x7),
    (35, 'endpoint-client', 'fabric-call-client-control', 0x7),
    (36, 'endpoint-service', 'fabric-call-client-control-service', 0x7),
    (37, 'endpoint-client', 'fabric-call-client-b-control', 0x7),
    (38, 'endpoint-service', 'fabric-call-client-b-control-service', 0x7),
    (39, 'endpoint-client', 'fabric-call-server-control', 0x7),
    (40, 'endpoint-service', 'fabric-call-server-control-service', 0x7),
    (41, 'endpoint-client', 'fabric-call-time-control', 0x7),
    (42, 'endpoint-service', 'fabric-call-time-control-service', 0x7),
    (43, 'endpoint-client', 'fabric-op-client-control', 0x7),
    (44, 'endpoint-service', 'fabric-op-client-control-service', 0x7),
    (45, 'endpoint-client', 'fabric-op-client-b-control', 0x7),
    (46, 'endpoint-service', 'fabric-op-client-b-control-service', 0x7),
    (47, 'endpoint-client', 'fabric-op-server-control', 0x7),
    (48, 'endpoint-service', 'fabric-op-server-control-service', 0x7),
    (49, 'endpoint-client', 'fabric-op-time-control', 0x7),
    (50, 'endpoint-service', 'fabric-op-time-control-service', 0x7),
    (51, 'endpoint-client', 'fabric-op-client-b-restart-control', 0x7),
    (52, 'endpoint-service', 'fabric-op-client-b-restart-control-service', 0x7),
)

# The seL4 C8.6 call plane (P5.4.6). A fabric-only layout sharing nothing with
# the base, so it replaces rather than overrides.
#
# Only init's *executables* and its two factories are numbered here. Every
# control channel is minted at runtime by `init.rs::drive_call_plane` through
# `endpoint_create`, exactly as `drive_stream_plane` mints the stream plane's —
# which is what makes the fabric's own slots come out in spawn-grant order
# (`FACTORY_SLOT` 0, `BUFFER_FACTORY_SLOT` 1, controls from
# `FABRIC_FIRST_CONTROL_SLOT`) rather than from the root's channel cursor.
#
# Declaring those controls as *generation* grants instead is what left the
# fabric's ends numbered around its factory grants (`[0, 3, 4, 5, 6]`), because
# the root's channel cursor resumes above everything staging installed. The
# grants themselves stay in the manifest: `_control_sources` derives
# `FABRIC_CALL_CLIENTS` — the table the broker authenticates a caller by — from
# exactly those four names, in `FABRIC_CALL_CONTROL_GRANTS` order rather than in
# the builder's `(name, source, target)` sort. They are the naming source; the
# endpoints init mints are the authority.
#
# Rights match what the root actually places, not what generation 17 declares.
# The two factories carry no `RIGHT_TRANSFER` (`0x4`) because `staging` installs
# them at `RIGHT_ENDPOINT_CREATE`/`RIGHT_BUFFER_CREATE` alone — every other seL4
# plane fixture records `0x20000`/`0x1000000` for the same reason, and
# `bootstrap_role_slot` tests containment rather than equality, so a too-
# permissive row would pass here and then disagree with a blessed fixture.
#
# `0x1000c` marks every executable whose returned supervision handle init
# delegates. The fabric handle is still passed to the client and server as the
# shared-payload receiver identity. Init now also vouches for each call
# participant by transferring its handle to the broker over that participant's
# authenticated control channel. The clock handle remains local to init.
SEL4_CALL_LAYOUT = (
    (1, 'executable', 'fabric-call-client', 0x1000c),
    (2, 'executable', 'fabric-call-client-b', 0x1000c),
    (3, 'executable', 'fabric-call-server', 0x1000c),
    (4, 'executable', 'fabric-call-time', 0x10008),
    (5, 'executable', 'fabric-service', 0x1000c),
    (6, 'endpoint-factory', None, 0x20000),
    (7, 'shared-buffer-factory', None, 0x1000000),
)


# The seL4 C8.7 operation plane (P5.4.7), on `SEL4_CALL_LAYOUT`'s rule: init's
# two factories and its executables, with every control channel minted at
# runtime by `init.rs::drive_operation_plane` so the fabric's own slots come out
# in spawn-grant order.
#
# Six executables rather than five: the operation graph declares a *replacement*
# client-B, which is how C8.7's participant-restart arm is expressed. The broker
# parks on the replacement's authenticated control while client B's slot is
# vacant, so the replacement is a declared identity in the graph rather than a
# second instance of client B.
#
# `0x1000c` on the four participants whose supervision handles init delegates to
# the broker (both clients, the replacement, and the server). The fabric and the
# clock stay `0x10008`: unlike the call plane, no participant here needs to name
# the fabric as a loan receiver, so the fabric's own handle is never transferred.
SEL4_OPERATION_LAYOUT = (
    (1, 'executable', 'fabric-op-client', 0x1000c),
    (2, 'executable', 'fabric-op-client-b', 0x1000c),
    (3, 'executable', 'fabric-op-client-b-restart', 0x1000c),
    (4, 'executable', 'fabric-op-server', 0x1000c),
    (5, 'executable', 'fabric-op-time', 0x10008),
    (6, 'executable', 'fabric-service', 0x10008),
    (7, 'endpoint-factory', None, 0x20000),
    (8, 'shared-buffer-factory', None, 0x1000000),
)


# The seL4 C8.8 visibility plane (P5.4.8), on `SEL4_CALL_LAYOUT`'s rule: init's
# two factories and its executables, with every control channel minted at
# runtime by `init.rs::drive_visibility_plane`.
#
# Six executables, and the same set the stream plane declares: the visibility
# graph is the stream graph with a declared interposition chain, so
# `fabric-intruder` is the proxy rather than an unauthorized probe. No
# executable is transferable — the visibility broker mints every route half
# itself and hands out narrowed non-delegable roles, so no supervision handle is
# ever delegated.
SEL4_VISIBILITY_LAYOUT = (
    (1, 'executable', 'fabric-intruder', 0x10008),
    (2, 'executable', 'fabric-publisher', 0x10008),
    (3, 'executable', 'fabric-publisher-b', 0x10008),
    (4, 'executable', 'fabric-service', 0x10008),
    (5, 'executable', 'fabric-subscriber', 0x10008),
    (6, 'executable', 'fabric-subscriber-b', 0x10008),
    (7, 'endpoint-factory', None, 0x20000),
    (8, 'shared-buffer-factory', None, 0x1000000),
)

# The seL4 QoS plane has the stream composition's init table. Its clock is a
# runtime-minted capability routed through participant control channels, not a
# bootstrap slot.
SEL4_QOS_LAYOUT = (
    (1, 'executable', 'fabric-intruder', 0x10008),
    (2, 'executable', 'fabric-publisher', 0x10008),
    (3, 'executable', 'fabric-publisher-b', 0x10008),
    (4, 'executable', 'fabric-service', 0x10008),
    (5, 'executable', 'fabric-subscriber', 0x10008),
    (6, 'executable', 'fabric-subscriber-b', 0x10008),
    (7, 'endpoint-factory', None, 0x20000),
    (8, 'shared-buffer-factory', None, 0x1000000),
)


# The seL4 C8.10 full-graph boot (P5.4.9). Twenty-one rows: the two
# factories, the fabric, sixteen participants, and two fabric-spawned workers.
# Every C8 role coexists in one generation, so the stream, call, and operation
# planes occupy disjoint slots rather than aliasing a profile-selected range.
#
# The control channels are minted at runtime by `init.rs::drive_boot_plane`, as
# on every other seL4 plane and for the same reason: the root numbers a launched
# component's declared channel ends from its own cursor, so a declared control
# reaches the fabric at a slot no `FABRIC_FIRST_CONTROL_SLOT + index` describes.
# The grants stay in the manifest because `_control_sources` derives the
# identity tables from those names. So this table numbers the twenty-one things
# the *generation* places, and the C8.10 property it carries is that all three
# planes' executables coexist in disjoint slots with no profile-dependent
# rewrite -- which is the half a boot layout can state.
#
# Two rows differ from the x86 table. The factories carry no `RIGHT_TRANSFER`
# (`0x4`), because `staging` installs them at `RIGHT_ENDPOINT_CREATE` and
# `RIGHT_BUFFER_CREATE` alone -- the same reason every other seL4 fixture records
# `0x20000`/`0x1000000` where the oracle records `0x20004`/`0x1000004`.
#
# `0x1000c` on all sixteen participants: init delegates the call and operation
# participants' supervision handles to the fabric, and declaring the whole set
# transferable keeps the table one rule rather than a per-plane exception. The
# fabric and its two workers stay `0x10008` -- their handles never move.
SEL4_BOOT_LAYOUT = (
    (0, 'endpoint-factory', None, 0x20000),
    (1, 'shared-buffer-factory', None, 0x1000000),
    (2, 'executable', 'fabric-service', 0x10008),
    (3, 'executable', 'fabric-call-worker', 0x10008),
    (4, 'executable', 'fabric-op-worker', 0x10008),
    (5, 'executable', 'fabric-publisher', 0x1000c),
    (6, 'executable', 'fabric-subscriber', 0x1000c),
    (7, 'executable', 'fabric-publisher-b', 0x1000c),
    (8, 'executable', 'fabric-subscriber-b', 0x1000c),
    (9, 'executable', 'fabric-observer', 0x1000c),
    (10, 'executable', 'fabric-probe', 0x1000c),
    (11, 'executable', 'fabric-proxy', 0x1000c),
    (12, 'executable', 'fabric-call-client', 0x1000c),
    (13, 'executable', 'fabric-call-client-b', 0x1000c),
    (14, 'executable', 'fabric-call-server', 0x1000c),
    (15, 'executable', 'fabric-call-time', 0x1000c),
    (16, 'executable', 'fabric-op-client', 0x1000c),
    (17, 'executable', 'fabric-op-client-b', 0x1000c),
    (18, 'executable', 'fabric-op-server', 0x1000c),
    (19, 'executable', 'fabric-op-time', 0x1000c),
    (20, 'executable', 'fabric-op-client-b-restart', 0x1000c),
    (21, 'endpoint-factory', None, 0x20000),
    (22, 'shared-buffer-factory', None, 0x1000000),
)

# The seL4 M5 storage plane (P5.4.2c). Init's endpoint factory, the probe's
# executable, and the block capability the probe holds.
#
# The block capability is *not* here, and that is the point: it is granted to
# the probe rather than to init, so the root places it in the probe's own table
# at its runtime cursor. A boot layout numbers the bootstrap component's slots;
# a non-bootstrap component's are numbered `1..=n` above its executables, which
# is the rule every other grant to a child follows.
SEL4_STORAGE_LAYOUT = (
    (1, 'executable', 'sel4-storage-probe', 0x10008),
    (2, 'endpoint-factory', None, 0x20000),
)

# Generation 24, the store plane. Same two rows as the storage plane and for the
# same reason: the block capability is granted to the probe, so the root places
# it in the probe's own table rather than init's.
SEL4_STORE_LAYOUT = (
    (1, 'executable', 'sel4-store-probe', 0x10008),
    (2, 'endpoint-factory', None, 0x20000),
)

# Generation 30, the M6.4 dango plane. Init's factory plus the three
# executables it spawns; the spawn service's own executables are granted to it,
# not to init, so they land in its table rather than here.
SEL4_DANGO_LAYOUT = (
    (1, 'executable', 'console', 0x10008),
    (2, 'executable', 'dango', 0x10008),
    (3, 'executable', 'spawn-service', 0x10008),
    (4, 'endpoint-factory', None, 0x20000),
    (5, 'shared-buffer-factory', None, 0x1000000),
    (6, 'executable', 'sysinfo', 0x10008),
    (7, 'executable', 'echo-agent', 0x10008),
)

# Generation 29, the M6.3 filesystem plane. Three rows: init's factory and both
# executables. Neither the block capability nor the namespace views are here —
# both are granted to the components that hold them.
SEL4_FILESYSTEM_LAYOUT = (
    (1, 'executable', 'directory-probe', 0x10008),
    (2, 'executable', 'sel4-filesystem-service', 0x10008),
    (3, 'endpoint-factory', None, 0x20000),
)

# Generation 33, the M6.7 transfer plane. Two rows; both device capabilities
# are the probe's own declared grants.
SEL4_TRANSFER_LAYOUT = (
    (1, 'executable', 'sel4-transfer-probe', 0x10008),
    (2, 'endpoint-factory', None, 0x20000),
)

# Generation 32, the M6.6 powerbox plane. Init's factory and both executables;
# the chooser's input and directory views are its own declared grants.
SEL4_POWERBOX_LAYOUT = (
    (1, 'executable', 'powerbox-chooser', 0x10008),
    (2, 'executable', 'powerbox-probe', 0x10008),
    (3, 'endpoint-factory', None, 0x20000),
)

# Generation 31, the input plane. Two rows; the input capability is granted to
# the probe, so the root places it in the probe's own table.
SEL4_INPUT_LAYOUT = (
    (1, 'executable', 'sel4-input-probe', 0x10008),
    (2, 'endpoint-factory', None, 0x20000),
)

# Generation 28, the M6.3 directory plane. Two rows, and the directory
# capability is not among them: it is granted to the probe, so the root places
# it in the probe's own table.
SEL4_DIRECTORY_LAYOUT = (
    (1, 'executable', 'sel4-directory-probe', 0x10008),
    (2, 'endpoint-factory', None, 0x20000),
)

# Generation 27, the M6.5 generation-command plane. Three rows: init's factory
# plus both executables. The block capability is the manager's, so it is placed
# in the manager's own table rather than here.
SEL4_GENERATION_LAYOUT = (
    (1, 'executable', 'sel4-generation-client', 0x10008),
    (2, 'executable', 'sel4-generation-manager', 0x10008),
    (3, 'endpoint-factory', None, 0x20000),
)

# Generation 26, the recovery plane.
SEL4_RECOVERY_LAYOUT = (
    (1, 'executable', 'sel4-recovery-probe', 0x10008),
    (2, 'endpoint-factory', None, 0x20000),
)

# Generation 25, the rollback plane. Same shape again.
SEL4_ROLLBACK_LAYOUT = (
    (1, 'executable', 'sel4-rollback-probe', 0x10008),
    (2, 'endpoint-factory', None, 0x20000),
)


# Per-generation overrides, applied over `BASE_LAYOUT` by slot. A generation
# absent here resolves the base layout unchanged.
#
# Generations 14 and 15 reuse slots 45-59 for the call and operation planes
# respectively: the two are mutually exclusive profiles, so neither grows init's
# capability table past `MAX_CAPS`. That reuse is the aliasing B10 makes
# explicit -- it stays, but as declared data rather than a `caps[46] = ...`
# rewrite in kernel source.
OVERRIDES = {
    2: OVERRIDE_2,
    3: OVERRIDE_3,
    4: OVERRIDE_4,
    13: OVERRIDE_13,
    14: OVERRIDE_14,
    15: OVERRIDE_15,
}

# Generation 17 is the C8.10 full-graph boot: a fabric-only layout sharing
# nothing with the base, so it replaces rather than overrides. Generation 18 is
# the seL4 C8.6 call plane, 20 the seL4 C8.7 operation plane, and 21 the seL4
# C8.8 visibility plane, and 22 the seL4 C8.10 full-graph boot, on the same
# rule.
REPLACEMENTS = {
    17: FABRIC_BOOT_LAYOUT,
    18: SEL4_CALL_LAYOUT,
    19: SEL4_QOS_LAYOUT,
    20: SEL4_OPERATION_LAYOUT,
    21: SEL4_VISIBILITY_LAYOUT,
    22: SEL4_BOOT_LAYOUT,
    23: SEL4_STORAGE_LAYOUT,
    24: SEL4_STORE_LAYOUT,
    25: SEL4_ROLLBACK_LAYOUT,
    26: SEL4_RECOVERY_LAYOUT,
    27: SEL4_GENERATION_LAYOUT,
    28: SEL4_DIRECTORY_LAYOUT,
    29: SEL4_FILESYSTEM_LAYOUT,
    30: SEL4_DANGO_LAYOUT,
    31: SEL4_INPUT_LAYOUT,
    32: SEL4_POWERBOX_LAYOUT,
    33: SEL4_TRANSFER_LAYOUT,
}


def component_identity(name: str) -> bytes:
    """Stable identity for a component, matching `boot_layout::component_identity`."""
    encoded = name.encode("utf-8")
    return sha256(b"slime-boot-layout-component-v1:" + struct.pack("<H", len(encoded)) + encoded)


def channel_identity(name: str) -> bytes:
    """Stable identity for a channel half, matching `boot_layout::channel_identity`."""
    encoded = name.encode("utf-8")
    return sha256(b"slime-boot-layout-channel-v1:" + struct.pack("<H", len(encoded)) + encoded)


def _distinct_slots(table: tuple, name: str) -> None:
    """Reject a source table that claims one slot twice.

    An override *replacing* a base slot is the mechanism; a table claiming one
    slot twice within itself is a typo whose effect depends on which entry the
    merge below happened to apply last. Checked here because the merge would
    otherwise silently drop one of them, and the encoder would then see a table
    that is already well-formed.
    """
    slots = [entry[0] for entry in table]
    if len(set(slots)) != len(slots):
        duplicated = sorted({slot for slot in slots if slots.count(slot) > 1})
        raise ValueError(f"boot layout {name}: slot(s) {duplicated} declared twice")


# Every component any layout names an executable for. A channel half belongs to
# whichever of these its label is built from, so dropping a component from a
# profile drops the endpoints minted for it too.
def _layout_components() -> set[str]:
    tables = (BASE_LAYOUT, *OVERRIDES.values(), *REPLACEMENTS.values())
    return {
        label
        for table in tables
        for _slot, role, label, _rights in table
        if role == "executable"
    }


# Channel label -> the component it belongs to. Built by stripping the suffixes
# the endpoint convention appends, longest first so `-control-service` is not
# read as a `-control` channel of a component ending `-control`. A label that
# strips to no known component belongs to no one -- `console-output` and
# `dango-spawn` name a channel between two components rather than one
# component's own endpoint, and both ends live or die with the base layout.
def _channel_components() -> dict[str, str]:
    components = _layout_components()
    suffixes = ("-control-service", "-control", "-service", "-client", "-side")
    owners: dict[str, str] = {}
    for table in (BASE_LAYOUT, *OVERRIDES.values(), *REPLACEMENTS.values()):
        for _slot, role, label, _rights in table:
            if label is None or role == "executable" or label in owners:
                continue
            for suffix in suffixes:
                if label.endswith(suffix) and label[: -len(suffix)] in components:
                    owners[label] = label[: -len(suffix)]
                    break
    return owners


CHANNEL_COMPONENTS = _channel_components()


def layout_for(number: int, components: set[str] | None = None) -> tuple:
    """The slot table this generation number resolves, as (slot, role, label, rights).

    B11: `components` is the set the selected boot profile declares. A slot
    naming a component the profile leaves out is dropped, and the survivors are
    renumbered from zero in declared order -- init's table cannot carry a hole,
    since `LayoutPlacer::finish` requires every slot below the high-water mark
    to be filled.

    Renumbering is safe where it happens because `init.rs` addresses every slot
    through the generated constant for its label rather than a literal, so a
    moved slot moves in both readers at once. It also only happens for a profile
    that actually drops something: a profile declaring every component this
    table names filters nothing and compacts nothing, which is why the test,
    visibility, and full-graph profiles keep their slots byte-for-byte and their
    gates keep observing the evidence they already recorded.
    """
    if number in REPLACEMENTS:
        _distinct_slots(REPLACEMENTS[number], f"replacement {number}")
        entries = REPLACEMENTS[number]
    else:
        _distinct_slots(BASE_LAYOUT, "base")
        override = OVERRIDES.get(number, ())
        _distinct_slots(override, f"override {number}")
        merged = {slot: (slot, role, label, rights) for slot, role, label, rights in BASE_LAYOUT}
        for slot, role, label, rights in override:
            merged[slot] = (slot, role, label, rights)
        entries = tuple(merged[slot] for slot in sorted(merged))
    if components is None:
        return entries
    kept = [
        entry
        for entry in entries
        if (owner := _entry_component(entry)) is None or owner in components
    ]
    if len(kept) == len(entries):
        return entries
    return tuple(
        (index, role, label, rights)
        for index, (_slot, role, label, rights) in enumerate(kept)
    )


def _entry_component(entry: tuple) -> str | None:
    """The component a slot belongs to, or `None` when it belongs to no one.

    An executable is its own component. A channel half belongs to the component
    its label names, which is the label with the `-client`/`-service` suffix
    the endpoint convention adds -- so dropping a component drops both halves of
    its control channel with it. A role slot (the endpoint factory, the input
    device) belongs to no component and is always kept.
    """
    _slot, role, label, _rights = entry
    if label is None:
        return None
    if role == "executable":
        return label
    return CHANNEL_COMPONENTS.get(label)


def build_boot_layout(number: int, fail, components: set[str] | None = None) -> bytes:
    """Encode the B10 boot capability layout resource object for one generation.

    Entries are sorted by slot and unique: the decoder rejects any other order,
    because a slot claimed twice would make the layout depend on which claim was
    applied last -- the positional ambiguity this resource exists to remove.

    `components` is the set the selected boot profile declares (B11); the layout
    is narrowed to it so the resource and the generation cannot disagree about
    which components exist.
    """
    entries = layout_for(number, components)
    if len(entries) > MAX_BOOT_LAYOUT_ENTRIES:
        fail(f"boot layout for generation {number} exceeds the capability table")
    encoded = b""
    seen = set()
    for slot, role, label, rights in entries:
        if role not in ROLE:
            fail(f"boot layout: unknown role {role!r} at slot {slot}")
        if slot in seen:
            fail(f"boot layout: slot {slot} declared twice")
        if not 0 <= slot < MAX_BOOT_LAYOUT_ENTRIES:
            fail(f"boot layout: slot {slot} outside the capability table")
        seen.add(slot)
        named = role in NAMED_ROLES
        if named != (label is not None):
            fail(f"boot layout: slot {slot} role {role!r} disagrees with its label")
        if label is None:
            identity = bytes(32)
        elif role == "executable":
            identity = component_identity(label)
        else:
            identity = channel_identity(label)
        if not 0 <= rights <= 0xFFFFFFFFFFFFFFFF:
            fail(f"boot layout: slot {slot} rights out of range")
        encoded += BOOT_LAYOUT_ENTRY.pack(identity, slot, ROLE[role], rights)
    total_len = BOOT_LAYOUT_HEADER_BYTES + len(entries) * BOOT_LAYOUT_ENTRY_BYTES
    header = BOOT_LAYOUT_HEADER.pack(
        BOOT_LAYOUT_MAGIC,
        BOOT_LAYOUT_VERSION,
        BOOT_LAYOUT_HEADER_BYTES,
        0,
        number,
        len(entries),
        total_len,
    )
    return header + encoded


# Every label any profile declares, so the generated Rust table defines a
# constant for each in every build. A component body gated by a check flag still
# *compiles* when that flag is unset, so a table emitting only the labels the
# current layout happens to declare would fail to build for fifteen profiles.
# A label this layout is silent about gets `SLOT_ABSENT`.
# How many times one role may repeat in a layout. Generation 4 declares two
# object stores; the table emits this many indexed constants for every role so
# a name is present in every generation's table, whatever that generation
# declares.
MAX_ROLE_REPEATS = 2


def all_labels() -> list[tuple[str, str]]:
    labels: dict[str, str] = {}
    for number in sorted({1, *OVERRIDES, *REPLACEMENTS}):
        for _, role, label, _ in layout_for(number):
            if label is not None:
                labels[label] = role
    return sorted(labels.items())


def rust_identifier(label: str) -> str:
    return label.replace("-", "_").upper() + "_SLOT"


def render_rust(
    number: int,
    components: set[str] | None = None,
    binding_slots: dict[str, int] | None = None,
    role_bindings: dict[str, int] | None = None,
) -> str:
    """The slot table for one generation, including explicit init bindings.

    `init.rs` addresses slots by these names rather than by literal numbers, so
    the kernel that places a capability and the component that uses it read one
    source. A label this generation does not declare is `SLOT_ABSENT`: using it
    then fails at the syscall with a slot number in hand, rather than failing
    the build for every other profile.

    `components` narrows the table to the selected boot profile (B11). The set
    of constant *names* is unaffected -- `all_labels()` unions every profile, so
    a body gated by a check flag still compiles -- and a component the profile
    drops simply has `SLOT_ABSENT` where it had a slot.
    """
    declared = {
        label: slot
        for slot, _, label, _ in layout_for(number, components)
        if label is not None
    }
    if binding_slots is not None:
        declared.update(binding_slots)
    lines = [
        "// @generated from contracts/boot-layout/v1 by scripts/build/boot_layout.py;",
        "// do not edit. Regenerate through `just generation_check`.",
        "",
        "/// A slot this generation's boot layout does not declare. Using one is a",
        "/// component asking for authority this profile never granted it.",
        "#[allow(dead_code)]",
        "pub const SLOT_ABSENT: u32 = u32::MAX;",
        "",
        "/// The generation this table was emitted for.",
        "#[allow(dead_code)]",
        f"pub const BOOT_LAYOUT_GENERATION: u64 = {number};",
        "",
    ]
    for label, _ in all_labels():
        slot = declared.get(label)
        value = "SLOT_ABSENT" if slot is None else str(slot)
        lines.append("#[allow(dead_code)]")
        lines.append(f"pub const {rust_identifier(label)}: u32 = {value};")
    # The singular objects carry no name -- there is one endpoint factory, one
    # input device -- so they are addressed by role. A role appearing more than
    # once (generation 4 declares two object stores) is emitted in declared
    # order with an index suffix, matching the order the kernel places them in.
    lines.append("")
    by_role: dict[str, list[int]] = {}
    for slot, role, label, _ in layout_for(number, components):
        if label is None:
            by_role.setdefault(role, []).append(slot)
    if role_bindings is not None:
        for role, slot in role_bindings.items():
            by_role[role] = [slot]
    # A role can repeat: generation 4 declares an object store in both the
    # storage and filesystem slots. Every role therefore emits `_0`.._N` for a
    # fixed N in every generation, plus an unsuffixed alias for the first, so a
    # constant's *name* never varies between profiles -- only its value.
    roles = sorted(
        {role for _, role, label, _ in BASE_LAYOUT if label is None}
        | {role for table in REPLACEMENTS.values() for _, role, label, _ in table if label is None}
        | set(by_role)
    )
    for role in roles:
        slots = by_role.get(role, [])
        base = role.replace("-", "_").upper() + "_SLOT"
        lines.append("#[allow(dead_code)]")
        lines.append(f"pub const {base}: u32 = {slots[0] if slots else 'SLOT_ABSENT'};")
        for index in range(MAX_ROLE_REPEATS):
            value = str(slots[index]) if index < len(slots) else "SLOT_ABSENT"
            lines.append("#[allow(dead_code)]")
            lines.append(f"pub const {base}_{index}: u32 = {value};")
    return "\n".join(lines) + "\n"
