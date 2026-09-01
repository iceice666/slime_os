# Pin validation is offline: submodules and the pinned toolchain must already
# be installed.
[group('product')]
sel4_pin_check:
    python3 scripts/check/check-sel4-pins.py

# Produces build/slime-sel4.identity.json for the boot gate.
[group('product')]
sel4_qemu_image_check: sel4_pin_check
    python3 scripts/build/build-sel4.py --skip-pin-check

# Rebuilds before boot so QEMU runs the bytes named by the identity manifest.
[group('product')]
sel4_root_boot_check: sel4_pin_check
    python3 scripts/check/check-sel4-root-boot.py

# P3 replays the architecture-neutral corpus on the pinned RV64 reference.
[group('product')]
riscv64_qemu_image_check: sel4_pin_check
    python3 scripts/build/build-sel4.py --platform qemu-riscv-virt --skip-pin-check

[group('product')]
riscv64_qemu_check: sel4_pin_check
    python3 scripts/check/check-sel4-root-boot.py --platform qemu-riscv-virt
    python3 scripts/check/check-sel4-wait-set-plane.py --platform qemu-riscv-virt
    python3 scripts/check/check-sel4-sample-plane.py --platform qemu-riscv-virt
    python3 scripts/check/check-sel4-generation-plane.py --platform qemu-riscv-virt
    python3 scripts/check/check-sel4-rollback-plane.py --platform qemu-riscv-virt
    python3 scripts/check/check-architecture-portability.py


# Each gate must boot its own embedded-generation artifact.
[group('product')]
sel4_stress_check: sel4_pin_check
    python3 scripts/build/build-sel4.py --stress-plane
    python3 scripts/check/check-sel4-stress-plane.py

[group('product')]
sel4_component_graph_check: sel4_pin_check
    python3 scripts/check/check-sel4-component-graph.py

# Use fresh QEMU boots for rollback and wrong-target negative controls.
[group('product')]
sel4_demo_check: sel4_pin_check
    python3 scripts/check/check-sel4-demo-plane.py

[group('product')]
sel4_boot_layout_check: sel4_pin_check
    python3 scripts/check/check-sel4-boot-layout.py

[group('product')]
sel4_boot_layout_bless: sel4_pin_check
    python3 scripts/check/check-sel4-boot-layout.py --bless

# Fail closed: each injected CSpace mutation must make its boot fail.
# This performs six builds and boots, so it is intentionally outside `just test`.
[group('product')]
sel4_capability_layout_check: sel4_pin_check
    python3 scripts/check/check-sel4-capability-layout.py

[group('product')]
sel4_boot_check: sel4_pin_check
    python3 scripts/check/check-sel4-boot-plane.py

# Keep the product component-graph image distinct from fixture variants.
[group('product')]
sel4_product_image: sel4_pin_check
    python3 scripts/build/build-sel4.py --component-graph --skip-pin-check

[group('product')]
run: sel4_product_image
    qemu-system-aarch64 -machine virt,virtualization=on -cpu cortex-a53 -smp 1 \
        -m size=2048M -nographic -serial mon:stdio -kernel build/slime-sel4-graph.elf

[group('product')]
run_release: run

[group('product')]
generation_check: contracts_check sel4_component_graph_check
    python3 scripts/check/check-generation-determinism.py
