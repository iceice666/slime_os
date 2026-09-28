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
io_tcp_check: sel4_pin_check io_tcp_host_check
    python3 scripts/check/check-sel4-io-network-plane.py --arm tcp

# Scripted reordering, loss, silence and zero windows held to the declared TCP
# bounds; the peer's host controls run first. Explicit, never part of `all`.
[group('planes / mechanism')]
io_tcp_impairment_check: sel4_pin_check link_peer_check
    python3 scripts/check/check-sel4-io-network-plane.py --arm tcp-impairment

[group('planes / mechanism')]
io_tcp_host_check:
    cargo test -p slime-network-tcp-tests --locked

[group('planes / mechanism')]
io_local_check: sel4_pin_check io_tcp_host_check
    python3 scripts/check/check-sel4-io-network-plane.py --arm local

[group('planes / mechanism')]
io_network_lifetime_check: sel4_pin_check io_tcp_host_check
    python3 scripts/check/check-sel4-io-network-plane.py --arm lifetime

[group('planes / mechanism')]
io_network_service_fault_check: sel4_pin_check io_tcp_host_check
    python3 scripts/check/check-sel4-io-network-plane.py --arm service-fault

[group('planes / mechanism')]
io_network_driver_reset_check: sel4_pin_check io_tcp_host_check
    python3 scripts/check/check-sel4-io-network-plane.py --arm driver-reset

[group('planes / mechanism')]
io_network_qualification_check: contracts_check system_spec_check system_image_builder_check io_tcp_host_check test_host test_sel4_root io_network_check io_link_check link_peer_check sel4_gate_control_check fmt_check_all lint_all
    just docs_check
    just ruff
    just typos

# Reproducible host-stack HTTP/DNS cases; no Internet access is needed.
[group('planes / mechanism')]
io_http_check: sel4_pin_check
    python3 scripts/check/check-sel4-io-network-plane.py --arm http

# Explicit opt-in: DNS and HTTP are performed by the guest, not a host proxy.
[group('planes / mechanism')]
io_http_public_check: sel4_pin_check
    python3 scripts/check/check-sel4-io-network-plane.py --arm http-public --allow-public

# Completion gate includes a fresh live observation rather than cached evidence.
[group('planes / mechanism')]
io_http_qualification_check: io_network_qualification_check io_http_check
    just io_http_public_check

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
