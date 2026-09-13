# Legacy aliases remain resolvable for roadmap/devlog gate references;
# new dependencies use canonical recipes.

[group('compatibility')]
recovery_check: sel4_recovery_plane_check

[group('compatibility')]
aarch64_boot_check: sel4_root_boot_check

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

[group('compatibility')]
data_fabric_matrix_check: sel4_matrix_check

[group('compatibility')]
data_fabric_traffic_check: sel4_traffic_check

[group('compatibility')]
data_fabric_saturation_check: sel4_saturation_check

[group('compatibility')]
data_fabric_fault_check: sel4_fault_check

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

[group('compatibility')]
rpi5_arm_slice_check: sel4_demo_check

[group('compatibility')]
spawn_prereq_check: sel4_spawn_check

[group('compatibility')]
storage_cap_check: sel4_storage_check sel4_generation_check sel4_transfer_check

# Fail closed when physical Framework or NVMe evidence is unavailable.
[group('compatibility')]
storage_nvme_read_check:
    @echo "storage_nvme_read_check: unavailable after custom-kernel retirement; M5.7 requires a seL4 NVMe path and observed Framework evidence" >&2
    @exit 1

[group('compatibility')]
framework_inventory_check:
    @echo "framework_inventory_check: unavailable after custom-kernel retirement; H1 requires a seL4 Framework image and evidence/framework-inventory.jsonl" >&2
    @exit 1

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

[group('compatibility')]
dango_check: slisp_core_check

[group('compatibility')]
sel4_dango_check: slisp_core_check sel4_component_graph_check
