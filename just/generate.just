# Regenerate Rust block protocol bindings from the Zutai schema.
[group('generate')]
block_gen:
    python3 scripts/generate/generate-block-bindings.py

# Regenerate Rust component image bindings from the Zutai schema.
[group('generate')]
component_gen:
    python3 scripts/generate/generate-component-bindings.py

# Regenerate Rust + component store protocol bindings from the Zutai schema.
[group('generate')]
store_gen:
    python3 scripts/generate/generate-store-bindings.py

# Regenerate userspace spawn-service protocol bindings.
[group('generate')]
spawn_gen:
    python3 scripts/generate/generate-spawn-bindings.py

# Regenerate the language-neutral component startup/runtime ABI as Rust and C
# bindings. The root, Rust runtime, and non-Rust runtimes consume one layout.
[group('generate')]
component_runtime_abi_gen:
    python3 scripts/generate/generate-component-runtime-abi-bindings.py

# Regenerate the root-service syscall ABI bindings and doc tables (B59). One
# declaration feeds `slime-root`, `components/runtime`, and `docs/syscall-abi.md`,
# so a renumbered operation cannot leave any of the three disagreeing.
[group('generate')]
syscall_abi_gen:
    python3 scripts/generate/generate-syscall-abi-bindings.py

# Regenerate the sample-descriptor protocol bindings (C7.6).
[group('generate')]
sample_descriptor_gen:
    python3 scripts/generate/generate-sample-descriptor-bindings.py

# Regenerate native interface-schema compiler constants and Rust bindings (C8.1).
[group('generate')]
interface_schema_gen:
    python3 scripts/generate/generate-interface-schema-bindings.py

# Regenerate the capability-transfer protocol bindings (C8.3).
[group('generate')]
capability_transfer_gen:
    python3 scripts/generate/generate-capability-transfer-bindings.py

# Regenerate the fabric stream framing bindings (C8.4).
[group('generate')]
fabric_stream_gen:
    python3 scripts/generate/generate-fabric-stream-bindings.py

# B46: regenerate the v2 shared-ring bindings from their contract.
[group('generate')]
fabric_ring_gen:
    python3 scripts/generate/generate-fabric-ring-bindings.py

# C8.11: regenerate the bounded semantic-trace bindings from their contract.
[group('generate')]
fabric_trace_gen:
    python3 scripts/generate/generate-fabric-trace-bindings.py

# C8.8: regenerate the visibility record bindings from their contract — the
# Rust records the broker encodes and the Python offsets the plane gate decodes
# them with (B72).
[group('generate')]
fabric_visibility_gen:
    python3 scripts/generate/generate-fabric-visibility-bindings.py

# Regenerate the fabric-graph resource bindings (C8.2); part of the boot set.
[group('generate')]
fabric_graph_gen: boot_gen

# Regenerate kernel + component generation-management protocol bindings.
[group('generate')]
generation_management_gen:
    python3 scripts/generate/generate-generation-management-bindings.py

# Regenerate userspace powerbox protocol bindings.
[group('generate')]
powerbox_gen:
    python3 scripts/generate/generate-powerbox-bindings.py

# Regenerate host constants for the CP6 component-SDK release record.
[group('generate')]
component_sdk_release_gen:
    python3 scripts/generate/generate-component-sdk-release-bindings.py

# Regenerate host constants for generation v2, kernel image, and BootState.
[group('generate')]
boot_gen:
    python3 scripts/generate/generate-boot-bindings.py

[group('generate')]
generation_gen: boot_gen

[group('generate')]
kernel_image_gen: boot_gen

[group('generate')]
bootstate_gen: boot_gen
