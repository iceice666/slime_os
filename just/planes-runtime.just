[group('planes / runtime')]
private_memory_check: sel4_pin_check
    python3 scripts/check/check-sel4-private-memory-plane.py --platform qemu-arm-virt
    python3 scripts/check/check-sel4-private-memory-plane.py --platform qemu-riscv-virt
    python3 scripts/check/check-sel4-private-memory-plane.py --arm capacity --platform qemu-arm-virt
    python3 scripts/check/check-sel4-private-memory-plane.py --arm capacity --platform qemu-riscv-virt

[group('planes / runtime')]
private_memory_stress_check: sel4_pin_check
    python3 scripts/check/check-sel4-private-memory-plane.py --arm stress --platform qemu-arm-virt
    python3 scripts/check/check-sel4-private-memory-plane.py --arm stress --platform qemu-riscv-virt

# Prove kernel capability operations execute beyond the initial CNode.
[group('planes / runtime')]
private_memory_cspace_check: sel4_pin_check
    python3 scripts/check/check-sel4-private-memory-plane.py --arm cspace --platform qemu-arm-virt
    python3 scripts/check/check-sel4-private-memory-plane.py --arm cspace --platform qemu-riscv-virt

# Prove metadata growth, charged reuse and quarantine retry reconcile.
[group('planes / runtime')]
private_memory_metadata_check: sel4_pin_check
    python3 scripts/check/check-sel4-private-memory-plane.py --arm metadata --platform qemu-arm-virt
    python3 scripts/check/check-sel4-private-memory-plane.py --arm metadata --platform qemu-riscv-virt

# Prove bootstrap reserve bounds and independent refusal before publication.
[group('planes / runtime')]
private_memory_bootstrap_check: sel4_pin_check
    python3 scripts/check/check-sel4-private-memory-plane.py --arm bootstrap --platform qemu-arm-virt
    python3 scripts/check/check-sel4-private-memory-plane.py --arm bootstrap --platform qemu-riscv-virt

# Prove an idle maximum reserves nothing and a guarantee survives exhaustion.
[group('planes / runtime')]
private_memory_elastic_check: sel4_pin_check
    python3 scripts/check/check-sel4-private-memory-plane.py --arm elastic --platform qemu-arm-virt
    python3 scripts/check/check-sel4-private-memory-plane.py --arm elastic --platform qemu-riscv-virt

# Prove page-granular growth from backing no aligned span fits.
[group('planes / runtime')]
private_memory_fragmentation_check: sel4_pin_check
    python3 scripts/check/check-sel4-private-memory-plane.py --arm fragmentation --platform qemu-arm-virt
    python3 scripts/check/check-sel4-private-memory-plane.py --arm fragmentation --platform qemu-riscv-virt

# Prove every acquisition and mapping stage rolls back without a second charge.
[group('planes / runtime')]
private_memory_rollback_check: sel4_pin_check
    python3 scripts/check/check-sel4-private-memory-plane.py --arm rollback --platform qemu-arm-virt
    python3 scripts/check/check-sel4-private-memory-plane.py --arm rollback --platform qemu-riscv-virt

# Prove one holder's returned capacity becomes another's, zeroed and reconciled.
[group('planes / runtime')]
private_memory_conservation_check: sel4_pin_check
    python3 scripts/check/check-sel4-private-memory-plane.py --arm conservation --platform qemu-arm-virt
    python3 scripts/check/check-sel4-private-memory-plane.py --arm conservation --platform qemu-riscv-virt

# MEM-1G's authority and permission boundary, kept separate from the capacity
# arm so a 1 GiB working set is never the reason an isolation denial is missed.
[group('planes / runtime')]
private_memory_isolation_check: sel4_pin_check
    python3 scripts/check/check-sel4-private-memory-plane.py --arm isolation --platform qemu-arm-virt
    python3 scripts/check/check-sel4-private-memory-plane.py --arm isolation --platform qemu-riscv-virt

# MEM-64M requires both QEMU architectures to carry this workload, so an
# architecture-specific reclamation path cannot regress behind the other's
# evidence. AArch64 builds through the closure; RV64 re-targets the same
# composition through the legacy plane flag, as the ceiling arm's RV64 case does.
[group('planes / runtime')]
private_memory_cycles_check: sel4_pin_check
    python3 scripts/check/check-sel4-private-memory-plane.py --arm cycles --platform qemu-arm-virt
    python3 scripts/check/check-sel4-private-memory-plane.py --arm cycles --platform qemu-riscv-virt

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
