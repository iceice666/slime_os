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
[group('product')]
sel4_pin_check:
    python3 scripts/check/check-sel4-pins.py

# Configure, build, and install seL4 for qemu-arm-virt, build the root child and
# root task against the pinned rust-sel4 target specs, build the loader, add the
# kernel+root payload, and write `build/slime-sel4.identity.json` recording the
# source, config, ELF, and image digests the boot gate re-checks.
[group('product')]
sel4_qemu_image_check: sel4_pin_check
    python3 scripts/build/build-sel4.py --skip-pin-check

# Boot the packaged image on the pinned machine (`virt,virtualization=on`,
# cortex-a53, 1 CPU, 2048 MiB) and require the ordered generation, task, IPC,
# fault, and ready markers on serial. Rebuilds first so the booted bytes are the
# ones the identity manifest describes.
[group('product')]
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
[group('product')]
sel4_stress_check: sel4_pin_check
    python3 scripts/build/build-sel4.py --stress-plane
    python3 scripts/check/check-sel4-stress-plane.py

[group('product')]
sel4_component_graph_check: sel4_pin_check
    python3 scripts/check/check-sel4-component-graph.py

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
[group('product')]
sel4_demo_check: sel4_pin_check
    python3 scripts/check/check-sel4-demo-plane.py

# B10 on seL4 (P5.4.10): init's resolved capability layout, per plane, against
# the fixtures frozen at product cutover.
#
# Boots every plane and reads one `[layout]` block each — a layout diff should
# be readable as a layout diff rather than inferred from a component failing,
# which is why this is separate from the planes' own gates.
#
# Regenerate with `just sel4_boot_layout_bless`; the resulting diff is the
# evidence that a layout change was intended.
[group('product')]
sel4_boot_layout_check: sel4_pin_check
    python3 scripts/check/check-sel4-boot-layout.py

[group('product')]
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
[group('product')]
sel4_capability_layout_check: sel4_pin_check
    python3 scripts/check/check-sel4-capability-layout.py

# P5.4.9 and C8.10: build the full-graph image, boot it, and require every C8
# role to launch at once in one collision-free layout — the stream, call, and
# operation planes in disjoint slots, the fabric split into three bounded route
# workers, the unauthorized probe refused as its own task, and the whole graph
# coming to rest rather than finishing.
[group('product')]
sel4_boot_check: sel4_pin_check
    python3 scripts/check/check-sel4-boot-plane.py

# Build the product component-graph image. This is the composition `init`
# launches in a real generation; the default `fixture` variant embeds the same
# generation but compiles the root's two-fixture proof path instead of the
# generation-graph launcher, so it is a verification artifact rather than the
# product (B61).
[group('product')]
sel4_product_image: sel4_pin_check
    python3 scripts/build/build-sel4.py --component-graph --skip-pin-check

# Run the seL4 product image interactively on the pinned QEMU machine.
[group('product')]
run: sel4_product_image
    qemu-system-aarch64 -machine virt,virtualization=on -cpu cortex-a53 -smp 1 \
        -m size=2048M -nographic -serial mon:stdio -kernel build/slime-sel4-graph.elf

[group('product')]
run_release: run

[group('product')]
generation_check: contracts_check sel4_component_graph_check
    python3 scripts/check/check-generation-determinism.py
