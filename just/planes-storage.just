[group('planes / storage')]
sel4_device_check: sel4_pin_check
    python3 scripts/check/check-sel4-device-plane.py

[group('planes / storage')]
sel4_storage_check: sel4_pin_check
    python3 scripts/check/check-sel4-storage-plane.py

[group('planes / storage')]
sel4_store_check: sel4_pin_check
    python3 scripts/check/check-sel4-store-plane.py

[group('planes / storage')]
sel4_rollback_check: sel4_pin_check
    python3 scripts/check/check-sel4-rollback-plane.py

[group('planes / storage')]
sel4_recovery_plane_check: sel4_pin_check
    python3 scripts/check/check-sel4-recovery-plane.py

[group('planes / storage')]
sel4_generation_check: sel4_pin_check
    python3 scripts/check/check-sel4-generation-plane.py

# Fresh QEMU boots share one retained raw disk; do not split this checker.
[group('planes / storage')]
sel4_boot_selection_check: sel4_pin_check
    python3 scripts/check/check-sel4-boot-selection.py

[group('planes / storage')]
sel4_directory_check: sel4_pin_check
    python3 scripts/check/check-sel4-directory-plane.py

[group('planes / storage')]
sel4_input_check: sel4_pin_check
    python3 scripts/check/check-sel4-input-plane.py

[group('planes / storage')]
sel4_powerbox_check: sel4_pin_check
    python3 scripts/check/check-sel4-powerbox-plane.py


[group('planes / storage')]
sel4_transfer_check: sel4_pin_check
    python3 scripts/check/check-sel4-transfer-plane.py

[group('planes / storage')]
sel4_filesystem_check: sel4_pin_check
    python3 scripts/check/check-sel4-filesystem-plane.py
