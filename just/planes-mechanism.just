# IO3: supervised userspace virtio-net over duplex IO0 queues and IO1 authority.
[group('planes / mechanism')]
io_link_check: sel4_pin_check
    python3 scripts/check/check-sel4-io-link-plane.py

# IO1: userspace hardware authority, exact MMIO bounds, and denial proof.
[group('planes / mechanism')]
io_driver_authority_check: sel4_pin_check
    python3 scripts/check/check-sel4-io-driver-authority-plane.py

# IO4: bounded userspace network service over a backend-independent LinkDevice.
[group('planes / mechanism')]
io_network_check: sel4_pin_check
    python3 scripts/check/check-sel4-io-network-plane.py

# IO0: build and boot the shared I/O queue proof plane.
[group('planes / mechanism')]
io_queue_check: sel4_pin_check
    python3 scripts/check/check-sel4-io-queue-plane.py

# IO2: build and boot the userspace virtio-blk parity and fault plane.
[group('planes / mechanism')]
io_block_check: sel4_pin_check
    python3 scripts/check/check-sel4-io-block-plane.py

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
[group('planes / mechanism')]
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
[group('planes / mechanism')]
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
[group('planes / mechanism')]
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
[group('planes / mechanism')]
sel4_sample_check: sel4_pin_check
    python3 scripts/check/check-sel4-sample-plane.py

# B16: build the supervision-plane image, boot it, and require that a graph
# creating more tasks over its lifetime than `MAX_RECORDS` can hold at once
# still answers `supervision_status` correctly for every live handle —
# including one held across the crossing and one parked in `Transit` across it.
#
# An eighth image, on the same rule as the seven above: each gate boots the
# artifact it asserts about.
[group('planes / mechanism')]
sel4_supervision_check: sel4_pin_check
    python3 scripts/check/check-sel4-supervision-plane.py

# B38: repeatedly spawn and reclaim more task lifetimes than the old monotonic
# root CSlot and ordinary-untyped watermarks could sustain.
[group('planes / mechanism')]
sel4_reclamation_check: sel4_pin_check
    python3 scripts/check/check-sel4-reclamation-plane.py

# B22: build the channel-crossing image, boot it, and require that a graph
# minting more channels over its lifetime than `MAX_CHANNELS` holds at once
# still sends and receives on every live channel — including a pair held across
# the crossing and an end parked in `Transit` across it.
#
# A ninth image, on the same rule as the eight above: each gate boots the
# artifact it asserts about.
[group('planes / mechanism')]
sel4_crossing_check: sel4_pin_check
    python3 scripts/check/check-sel4-crossing-plane.py
