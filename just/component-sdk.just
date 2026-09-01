# Keep allocator groups in separate Cargo invocations: feature unification
# would enable mutually exclusive global allocators.
[group('component sdk')]
component_crate_split_check: component_spec_check
    python3 scripts/check/check-component-crate-split.py

[group('component sdk')]
external_component_admission_check: generation_check
    python3 scripts/check/check-external-component-admission.py

[group('component sdk')]
component_sdk_out_of_tree_check: external_component_admission_check
    python3 scripts/check/check-component-sdk-out-of-tree.py

[group('component sdk')]
component_sdk_export_check: external_component_admission_check
    python3 scripts/check/check-component-sdk-export.py

# Read-only and credential-free: it gates a publication before the release job.
# Report the SDK version a publication from this commit must claim
[group('component sdk')]
component_sdk_preflight *ARGS:
    python3 scripts/check/check-component-sdk-preflight.py {{ARGS}}

[group('component sdk')]
component_sdk_release_check: component_sdk_export_check
    python3 scripts/check/check-component-sdk-release.py

# Keep SDK artifacts platform-qualified; QEMU must reject the RPi ELF.
[group('component sdk')]
component_sdk_prefix_check: component_sdk_release_check
    python3 scripts/check/check-component-sdk-prefix.py

[group('component sdk')]
component_sdk_compatibility_check: component_sdk_prefix_check
    python3 scripts/check/check-component-sdk-compatibility.py

[group('component sdk')]
component_sdk_upgrade_check: component_sdk_compatibility_check
    python3 scripts/check/check-component-sdk-upgrade.py

[group('component sdk')]
runtime_binding_resolution_check: test_sel4_root sel4_gate_control_check sel4_channel_check sel4_loan_check sel4_component_graph_check
    @echo "runtime binding resolution: the root resolved named bindings, namespaced boot-layout roles, and unambiguous capability roles for their own instances, refused an ungranted name, and every migrated plane kept its observed behavior"
