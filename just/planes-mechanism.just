[group('planes / mechanism')]
io_link_check: sel4_pin_check
    python3 scripts/check/check-sel4-io-link-plane.py

[group('planes / mechanism')]
io_driver_authority_check: sel4_pin_check
    python3 scripts/check/check-sel4-io-driver-authority-plane.py

[group('planes / mechanism')]
io_network_check: sel4_pin_check
    python3 scripts/check/check-sel4-io-network-plane.py

[group('planes / mechanism')]
io_queue_check: sel4_pin_check
    python3 scripts/check/check-sel4-io-queue-plane.py

[group('planes / mechanism')]
io_block_check: sel4_pin_check
    python3 scripts/check/check-sel4-io-block-plane.py

[group('planes / mechanism')]
sel4_channel_check: sel4_pin_check
    python3 scripts/check/check-sel4-channel-plane.py

[group('planes / mechanism')]
sel4_loan_check: sel4_pin_check
    python3 scripts/check/check-sel4-loan-plane.py

[group('planes / mechanism')]
sel4_spawn_check: sel4_pin_check
    python3 scripts/check/check-sel4-spawn-plane.py

[group('planes / mechanism')]
sel4_sample_check: sel4_pin_check
    python3 scripts/check/check-sel4-sample-plane.py

[group('planes / mechanism')]
sel4_supervision_check: sel4_pin_check
    python3 scripts/check/check-sel4-supervision-plane.py

[group('planes / mechanism')]
sel4_reclamation_check: sel4_pin_check
    python3 scripts/check/check-sel4-reclamation-plane.py

[group('planes / mechanism')]
sel4_crossing_check: sel4_pin_check
    python3 scripts/check/check-sel4-crossing-plane.py
