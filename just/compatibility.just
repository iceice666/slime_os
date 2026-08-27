[group('compatibility')]
recovery_check: sel4_recovery_plane_check

# Historical architecture gate backed by a real neutral-source boundary scan.
[group('compatibility')]
aarch64_boot_check: sel4_root_boot_check

# Historical trap gate. seL4 owns exception entry and the trap instruction;
# `slime-root/src/fault.rs` decodes its fault messages, which the root boot
# gate observes as `SLIME_ROOT child fault observed ... kind=VirtualMemory`.
[group('compatibility')]
aarch64_trap_check: sel4_root_boot_check

[group('compatibility')]
product_boot_check: sel4_component_graph_check

[group('compatibility')]
generation_cmd_check: sel4_generation_check

[group('compatibility')]
powerbox_check: sel4_powerbox_check

[group('compatibility')]
shared_buffer_factory_check: sel4_loan_check

[group('compatibility')]
shared_buffer_accounting_check: sel4_loan_check

[group('compatibility')]
shared_buffer_mapping_check: sel4_loan_check

[group('compatibility')]
shared_buffer_loan_check: sel4_loan_check

[group('compatibility')]
fabric_authority_check: sel4_stream_check

[group('compatibility')]
fabric_stream_check: sel4_stream_check

[group('compatibility')]
fabric_qos_check: sel4_qos_check

[group('compatibility')]
fabric_call_check: sel4_call_check

[group('compatibility')]
fabric_operation_check: sel4_operation_check

[group('compatibility')]
fabric_visibility_check: sel4_visibility_check

[group('compatibility')]
data_fabric_boot_check: sel4_boot_check

# C8.12's roadmap-named gate.
[group('compatibility')]
data_fabric_matrix_check: sel4_matrix_check

# C8.13's roadmap-named gate.
[group('compatibility')]
data_fabric_traffic_check: sel4_traffic_check

# C8.13's roadmap-named gate for the saturation half.
[group('compatibility')]
data_fabric_saturation_check: sel4_saturation_check

# C8.14's roadmap-named gate.
[group('compatibility')]
data_fabric_fault_check: sel4_fault_check

# C8.15's roadmap-named gate, and the C8 parent close.
[group('compatibility')]
data_fabric_check: sel4_fabric_aggregate_check

[group('compatibility')]
sample_plane_check: sel4_sample_check

[group('compatibility')]
sample_plane_live_check: sel4_sample_check

[group('compatibility')]
boot_layout_check: sel4_boot_layout_check

[group('compatibility')]
spawn_service_check: sel4_spawn_check

# RP2's roadmap identifier, an alias onto the plane gate it names, the same way
# `data_fabric_matrix_check` aliases `sel4_matrix_check`. The roadmap's
# "Planned verification target" spelled this name before the plane existed, so it
# stays resolvable while the plane keeps the `sel4_<stem>_check` convention.
[group('compatibility')]
rpi5_arm_slice_check: sel4_demo_check

# Historical M5/M6 verification identifiers. The custom-kernel recipes are
# retired; these names resolve to the seL4 gates that now own the contracts.
[group('compatibility')]
spawn_prereq_check: sel4_spawn_check

[group('compatibility')]
storage_cap_check: sel4_storage_check sel4_generation_check sel4_transfer_check

# Physical Framework/NVMe qualification did not move to the seL4 QEMU product.
# Keep the documented identifiers, but fail closed rather than turning a
# missing physical transport/evidence gate into a false pass.
[group('compatibility')]
storage_nvme_read_check:
    @echo "storage_nvme_read_check: unavailable after custom-kernel retirement; M5.7 requires a seL4 NVMe path and observed Framework evidence" >&2
    @exit 1

[group('compatibility')]
framework_inventory_check:
    @echo "framework_inventory_check: unavailable after custom-kernel retirement; H1 requires a seL4 Framework image and evidence/framework-inventory.jsonl" >&2
    @exit 1

[group('compatibility')]
dango_check: sel4_dango_check

[group('compatibility')]
directory_check: sel4_filesystem_check

[group('compatibility')]
transfer_check: sel4_transfer_check

[group('compatibility')]
storage_read_check: sel4_storage_check

[group('compatibility')]
storage_write_check: sel4_storage_check

[group('compatibility')]
storage_fault_check: sel4_storage_check

[group('compatibility')]
storage_store_check: sel4_store_check

[group('compatibility')]
rollback_check: sel4_rollback_check
