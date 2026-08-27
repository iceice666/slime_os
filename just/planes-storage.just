# P5.4.2a: boot the component-graph image with a virtio-blk device attached and
# require the root to reach it — retype a granule out of BootInfo device untyped
# memory, map it non-cacheably into its own VSpace, and identify the disk by
# register read. `sel4_root_boot_check` asserts the same probe finding nothing
# when no drive is attached; the pair is what makes the mechanism observed.
[group('planes / storage')]
sel4_device_check: sel4_pin_check
    python3 scripts/check/check-sel4-device-plane.py

# P5.4.2c and M5.2/M5.3: boot the storage image with a virtio-blk device and
# require a *component* to move sectors through a capability its generation
# granted — a read returning the fixture's signature, a write and flush verified
# by read-back and confirmed durable in the image afterwards, and three refusal
# arms. The root-launched copy of the same component parks, so every arm is the
# spawned instance's.
[group('planes / storage')]
sel4_storage_check: sel4_pin_check
    python3 scripts/check/check-sel4-storage-plane.py

# P5.4.2c and M5.4: boot the store image and require a *component* to validate a
# GPT, open a content-addressed object store, retrieve an object by hash with
# its payload re-verified, append a durable commit that preserves the previous
# root, deduplicate identical content, scrub every payload, and fall back to the
# older superblock when the newest is damaged. The oracle keeps all of this in
# the kernel; here the root mediates sectors and nothing else.
[group('planes / storage')]
sel4_store_check: sel4_pin_check
    python3 scripts/check/check-sel4-store-plane.py

# P5.4.2c and M5.6: boot the rollback image and require a *component* to walk
# the BootState transition model on two durable slots — stage a pending
# generation, consume both attempts (the oracle's 2 -> 1 -> 0), roll back to
# known-good when they are exhausted, find rollback idempotent, refuse
# promotion with a wrong running identity or a stale release, and promote the
# running generation. Every commit is older-slot-first, so the previously
# selected root survives each transition.
[group('planes / storage')]
sel4_rollback_check: sel4_pin_check
    python3 scripts/check/check-sel4-rollback-plane.py

# P5.4.2c and M5.9: boot the recovery image with TWO disks — the target, and a
# guard disk no capability names — and require a *component* to refuse two
# corrupt BootState slots, decode a signed recovery index, verify its whole
# state closure against the content-addressed store, and reconstruct a bootable
# root into both slots idempotently. The guard image is hashed before and after:
# M5.9 requires reconstruction to modify no device it was not explicitly
# granted, and only the image proves that.
[group('planes / storage')]
sel4_recovery_plane_check: sel4_pin_check
    python3 scripts/check/check-sel4-recovery-plane.py

# P5.4.3 and M6.5: boot the generation image and require an *unprivileged*
# client to drive list, inspect, stage, select, and rollback through a
# management service that holds the plane's only block capability. Every
# refusal must leave BootState untouched — checked against the disk image, not
# just the status — and the client's own direct BlockTransact must be refused,
# because no slot it holds names a device.
[group('planes / storage')]
sel4_generation_check: sel4_pin_check
    python3 scripts/check/check-sel4-generation-plane.py

# B35: build the immutable selector once, then boot fresh QEMU processes
# against one retained raw disk to prove durable attempt consumption,
# exhaustion rollback, health-only promotion, and sector-scoped mutation.
[group('planes / storage')]
sel4_boot_selection_check: sel4_pin_check
    python3 scripts/check/check-sel4-boot-selection.py

# P5.4.3 and M6.3: boot the directory image and require a component holding one
# unscoped directory capability to derive narrower views that can neither escape
# their scope nor widen their rights, to be refused a stale commit and a scoped
# one, and to see its commits through every view of the shared namespace. The
# root owns this because a namespace root is unforgeable shared state with an
# atomic transition; what a directory contains stays in userspace.
[group('planes / storage')]
sel4_directory_check: sel4_pin_check
    python3 scripts/check/check-sel4-directory-plane.py

# P5.4.3: `InputRead` mediation on its own. A granted capability decodes the
# generation's scripted keys in order, an exhausted script ends its reader
# rather than blocking it, and a slot holding no input capability is refused.
[group('planes / storage')]
sel4_input_check: sel4_pin_check
    python3 scripts/check/check-sel4-input-plane.py

# P5.4.3 and M6.6: boot the powerbox image and require a chooser holding
# directory authority the requester lacks to grant exactly one narrowed object
# capability on a selection gesture, with a provenance record — and to deny a
# request exceeding its own authority, refuse derivation past the granted
# scope, and mint nothing at all on cancellation. Both components are the
# oracle's, unmodified.
[group('planes / storage')]
sel4_powerbox_check: sel4_pin_check
    python3 scripts/check/check-sel4-powerbox-plane.py


# P5.4.3 and M6.7: boot the transfer image with TWO devices — a read-only source
# carrying the manifest and a writable receiver — and require a generation to
# cross: digest, object closure, and travel policy all verified before any
# write, staged pending without disturbing the known-good root, and promoted
# only on health confirmation. The source is compared byte for byte afterwards.
[group('planes / storage')]
sel4_transfer_check: sel4_pin_check
    python3 scripts/check/check-sel4-transfer-plane.py

# P5.4.3 and M6.3's service half: boot the filesystem image and require the
# shared `directory-probe`, unmodified, to resolve names, survive an interrupted
# root transition, commit a new one, and derive a narrowed subdirectory through
# a seL4 filesystem service backed by a userspace object store. The client hands
# its own directory view across with every request.
[group('planes / storage')]
sel4_filesystem_check: sel4_pin_check
    python3 scripts/check/check-sel4-filesystem-plane.py
