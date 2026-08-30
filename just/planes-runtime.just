[group('planes / runtime')]
private_memory_check: sel4_pin_check
    python3 scripts/check/check-sel4-private-memory-plane.py

[group('planes / runtime')]
clock_authority_check: sel4_pin_check
    python3 scripts/check/check-sel4-clock-authority-plane.py

[group('planes / runtime')]
scheduling_class_check: sel4_pin_check
    python3 scripts/check/check-sel4-scheduling-class-plane.py

[group('planes / runtime')]
wait_set_check: sel4_pin_check
    python3 scripts/check/check-sel4-wait-set-plane.py

[group('planes / runtime')]
lifecycle_restart_check: sel4_pin_check
    python3 scripts/check/check-sel4-lifecycle-restart-plane.py

[group('planes / runtime')]
replay_check: sel4_pin_check
    python3 scripts/check/check-sel4-replay-plane.py

[group('planes / runtime')]
robot_runtime_check: sel4_pin_check
    python3 scripts/check/check-sel4-robot-runtime-plane.py

[group('planes / runtime')]
sel4_c_runtime_check: sel4_pin_check
    python3 scripts/check/check-sel4-c-runtime.py

[group('planes / runtime')]
slisp_core_check: sel4_pin_check
    python3 scripts/check/check-slisp-core.py
