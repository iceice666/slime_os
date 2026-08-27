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
[group('planes / runtime')]
private_memory_check: sel4_pin_check
    python3 scripts/check/check-sel4-private-memory-plane.py

# C9.1: boot independently authorized monotonic, timer, and simulated clocks;
# observe a one-shot expiry on the holder's declared notification, cancellation,
# per-holder quota isolation, teardown cleanup, and deny-by-default refusal.
[group('planes / runtime')]
clock_authority_check: sel4_pin_check
    python3 scripts/check/check-sel4-clock-authority-plane.py

# C9.3: a declared scheduling class, its band mapping, and promotion authority
# over another component's class. A foreground component preempts a saturating
# bestEffort loop still in flight, a declared promotion applies within its
# ceiling, no component widens itself, and an undeclared instance runs at the
# declared default.
[group('planes / runtime')]
scheduling_class_check: sel4_pin_check
    python3 scripts/check/check-sel4-scheduling-class-plane.py

# C9.2: a bounded userspace wait set over one declared Notification —
# registration, badge demultiplexing, deterministic dispatch order, every
# ceiling refused, and a peer death observed through a declared source.
[group('planes / runtime')]
wait_set_check: sel4_pin_check
    python3 scripts/check/check-sel4-wait-set-plane.py

# C9.4: a userspace supervisor restarts a component under its declared attempt
# bound and growing backoff. The fault, exit, and unhealthy causes are
# distinguishable from both sides, every predecessor handle is refused while the
# declared configuration survives, an undeclared transition is refused without
# moving the state, and exhausting the bound leaves the instance in the declared
# terminal state with its next spawn refused.
[group('planes / runtime')]
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
[group('planes / runtime')]
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
[group('planes / runtime')]
robot_runtime_check: sel4_pin_check
    python3 scripts/check/check-sel4-robot-runtime-plane.py

# Language-neutral component proof: build a freestanding C ELF against the
# generated C ABI, admit it as an external component, and boot it through QEMU.
[group('planes / runtime')]
sel4_c_runtime_check: sel4_pin_check
    python3 scripts/check/check-sel4-c-runtime.py
