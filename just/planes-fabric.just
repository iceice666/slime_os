# P5.5.2: build the stream-plane image, boot it, and require that the full C8.4
# stream plane runs on seL4 with every participant unmodified — two publishers,
# two subscribers, two routes, the >MAX_INLINE_BYTES descriptor and loan path,
# KEEP_LAST eviction under a stalled subscriber, and the frozen P5 cutover
# transcript. The transfer contract's subset test (B17) is observed here too.
#
# A seventh image, beside the six above, on the same rule: each gate boots the
# artifact it asserts about. It replaces P5.5.1's typed-fabric image, whose
# every assertion this one subsumes over a larger graph.
[group('planes / fabric')]
sel4_stream_check: sel4_pin_check
    python3 scripts/check/check-sel4-stream-plane.py

# B25 and P5.4.6: build the bounded-call image, boot it, and require the full
# C8.6 native-call plane plus the seL4 composition that makes it possible:
# init mints authenticated control pairs and transfers each participant's
# supervision handle to the broker after spawn. Every spawned component must
# then reach one clean terminal status.
[group('planes / fabric')]
sel4_call_check: sel4_pin_check
    python3 scripts/check/check-sel4-call-plane.py

# P5.4.7 and C8.7: build the native-operation image, boot it, and require the
# full bounded-operation surface with the shared broker and participants —
# correlation, authority denials, restart determinism, cancellation races,
# explicit-time expiry, peer-death settlement, parent-vouched supervision, and
# one clean exit per spawned task.
[group('planes / fabric')]
sel4_operation_check: sel4_pin_check
    python3 scripts/check/check-sel4-operation-plane.py

# P5.4.8 and C8.8: build the visibility image, boot it, and require filtered
# introspection plus declared interposition with the shared broker and
# participants — three callers with different bounded views, an ungranted
# caller that infers nothing, a telemetry path reaching its subscriber only
# through the declared proxy, and proxy death leaving the unrelated route live.
[group('planes / fabric')]
sel4_visibility_check: sel4_pin_check
    python3 scripts/check/check-sel4-visibility-plane.py

# B72: rewrite the frozen view fixture from an observed boot. The resulting diff
# is the evidence that a route-name or declared-QoS change was intended.
[group('planes / fabric')]
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
[group('planes / fabric')]
sel4_matrix_check: sel4_pin_check
    python3 scripts/check/check-sel4-matrix-plane.py

# C8.13: build the traffic-plane image -- the identical C8.10 collision-free
# partition, carrying real stream, call, and operation traffic concurrently
# instead of parking -- boot it, and require every declared resource ceiling
# to emit bounded high-water evidence with nothing dropped or rejected.
[group('planes / fabric')]
sel4_traffic_check: sel4_pin_check
    python3 scripts/check/check-sel4-traffic-plane.py

# C8.13: build the saturation-plane image -- the identical traffic-action
# scenario against a fixture whose declared ceilings are tightened to the
# traffic plane's own observed peaks -- boot it, and require three of them to
# land exactly at their declared bound with nothing dropped, rejected, or
# deadlocked.
[group('planes / fabric')]
sel4_saturation_check: sel4_pin_check
    python3 scripts/check/check-sel4-saturation-plane.py

# C8.14: boot the identical concurrent traffic graph with the declared
# interposition hop injected to die, and require every degradation and terminal
# condition to stay bounded, distinguishable, and fully reclaimed — while every
# unaffected stream, call, and operation route completes anyway.
[group('planes / fabric')]
sel4_fault_check: sel4_pin_check
    python3 scripts/check/check-sel4-fault-plane.py

# C8.15: boot every C8 aggregate plane twice over one declared composition and
# require each to satisfy its own plane gate on both runs and to produce
# byte-identical semantic traces — the determinism claim no single-boot gate can
# make, and the parent close for the C8 track.
[group('planes / fabric')]
sel4_fabric_aggregate_check: sel4_pin_check
    python3 scripts/check/check-sel4-fabric-aggregate.py

# P5.4.5 on seL4: C8.5's declared QoS policy, on the `sel4-qos` plane.
# Separate from `sel4_stream_check` because `sel4-stream.zti` grants no time
# capability, so its simulated-time clause is structurally unreachable — the
# arms this asserts cannot fire there at all.
[group('planes / fabric')]
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
[group('planes / fabric')]
sel4_trace_check: sel4_pin_check
    python3 scripts/check/check-sel4-trace-plane.py
