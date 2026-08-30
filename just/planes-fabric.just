[group('planes / fabric')]
sel4_stream_check: sel4_pin_check
    python3 scripts/check/check-sel4-stream-plane.py

[group('planes / fabric')]
sel4_call_check: sel4_pin_check
    python3 scripts/check/check-sel4-call-plane.py

[group('planes / fabric')]
sel4_operation_check: sel4_pin_check
    python3 scripts/check/check-sel4-operation-plane.py

[group('planes / fabric')]
sel4_visibility_check: sel4_pin_check
    python3 scripts/check/check-sel4-visibility-plane.py

# Blessing rewrites the frozen view fixture from the observed boot.
[group('planes / fabric')]
sel4_visibility_bless: sel4_pin_check
    python3 scripts/check/check-sel4-visibility-plane.py --bless

[group('planes / fabric')]
sel4_matrix_check: sel4_pin_check
    python3 scripts/check/check-sel4-matrix-plane.py

[group('planes / fabric')]
sel4_traffic_check: sel4_pin_check
    python3 scripts/check/check-sel4-traffic-plane.py

[group('planes / fabric')]
sel4_saturation_check: sel4_pin_check
    python3 scripts/check/check-sel4-saturation-plane.py

[group('planes / fabric')]
sel4_fault_check: sel4_pin_check
    python3 scripts/check/check-sel4-fault-plane.py

[group('planes / fabric')]
sel4_fabric_aggregate_check: sel4_pin_check
    python3 scripts/check/check-sel4-fabric-aggregate.py

# Keep QoS separate: the stream generation has no time capability.
[group('planes / fabric')]
sel4_qos_check: sel4_pin_check
    python3 scripts/check/check-sel4-qos-plane.py

[group('planes / fabric')]
sel4_trace_check: sel4_pin_check
    python3 scripts/check/check-sel4-trace-plane.py
