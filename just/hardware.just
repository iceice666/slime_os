[group('hardware')]
rpi5_ros2_demo_contract_check:
    python3 scripts/check/check-rpi5-ros2-demo-contract.py

[group('hardware')]
rpi5_ros2_demo_contract_v2_check:
    python3 scripts/check/check-rpi5-ros2-demo-contract-v2.py

[group('hardware')]
rpi5_artifact_check: architecture_contract_check rpi5_ros2_demo_contract_check rpi5_ros2_demo_contract_v2_check
    python3 scripts/check/check-rpi5-artifacts.py

# Uses the board-specific bcm2712 prefix, target directory, generation, image,
# and pinned hashes; it is not interchangeable with the qemu-arm-virt build.
[group('hardware')]
sel4_rpi5_image_check: sel4_pin_check
    python3 scripts/build/build-sel4.py --platform bcm2712-rpi5 --skip-pin-check

[group('hardware')]
sel4_duo_image_check: sel4_pin_check
    python3 scripts/build/build-sel4.py --platform cv1800b-duo --skip-pin-check

# Flatten via the media builder: objcopy drops the sectionless loader payload.
# This recipe writes no block device.
[group('hardware')]
rpi5_media_check: sel4_rpi5_image_check
    python3 scripts/build/build-rpi5-media.py

# Physical gate: require ordered boot evidence from the pinned build on UART10.
# Missing board, adapter, media, or serial evidence is a failure, never a skip.
# Copy build/rpi5-media/* to FAT32, reset, then pass the serial device.
[group('hardware')]
rpi5_boot_check serial="": sel4_pin_check rpi5_artifact_check
    python3 scripts/check/check-rpi5-boot.py {{ if serial == "" { "" } else { "--serial " + serial } }}

# Bring-up aid only: monitor debug UART without qualifying the board.
# Exits after 10 seconds of quiet or the requested timeout.
[group('hardware')]
rpi5_serial_monitor serial timeout="120":
    python3 scripts/check/check-rpi5-boot.py --monitor --serial {{ serial }} --timeout {{ timeout }}

# P3.D: build the Milk-V Duo bring-up payload and wrap it in the FIT the board's
# firmware accepts, writing `build/duo-payload/identity.json`. Deploys nothing
# and touches no board, so it is safe to run anywhere `nix` is available.
#
# The payload is a flat binary, not an ELF: this board's vendor U-Boot has no
# `bootelf`, and FIT `/incbin/` does not parse program headers.
[group('hardware')]
duo_payload_check:
    python3 scripts/build/build-duo-payload.py

# P3.D: boot the pinned payload on the named Milk-V Duo and require ordered
# S-mode evidence on UART0 at the baud `sel4/pins.toml [cv1800b_duo]` pins.
# Physical: it proves the FIT is this build's, deploys it to the board's own FAT
# boot partition over USB-NCM, drives U-Boot over serial, and reads a real
# serial device. A QEMU pass cannot complete this milestone (roadmap invariant
# 8), so a missing board, link, adapter, or marker is a failure and never a skip.
#
# Unlike the Pi 5 path there is no removable-media step: the operator's laptop
# has no SD reader, so the board's card never leaves it. Stock `/boot/boot.sd`
# is never modified, so letting autoboot run returns the board to vendor Linux.
#
# Requires a key the board's dropbear already accepts, bootstrapped once over
# the serial console:
#   just duo_boot_check /dev/ttyUSB0
[group('hardware')]
duo_boot_check serial="": duo_payload_check
    python3 scripts/check/check-duo-boot.py {{ if serial == "" { "" } else { "--serial " + serial } }}

# P3.E: build and digest-deploy the Duo sample-plane and bounded early-fault
# FITs, drive four autonomous physical boots, and require identical normalized
# semantics across three successful runs. Each image cold-resets through the
# observed RTC block and the gate waits for vendor Linux before continuing.
[group('hardware')]
duo_sel4_check serial="": riscv64_qemu_check
    python3 scripts/check/check-duo-sel4.py {{ if serial == "" { "" } else { "--serial " + serial } }}

# P3.F: build the target-qualified resident graph with Duo UART0 input, deploy
# its distinct product FIT, drive one bounded Slisp session, and cold-reset only
# through the gate-only terminator after every assertion has passed.
[group('hardware')]
duo_slisp_check serial="": slisp_core_check sel4_component_graph_check
    python3 scripts/check/check-duo-slisp.py {{ if serial == "" { "" } else { "--serial " + serial } }}

# Bring-up aid, not a gate: print whatever the Duo's UART0 emits and assert
# nothing. Builds no artifacts and qualifies no board, so it answers the one
# question `duo_boot_check` cannot when the wire is silent — whether any byte
# reaches this host. Exits on its own after 10s of quiet or the timeout.
#
# just duo_serial_monitor /dev/ttyUSB0
[group('hardware')]
duo_serial_monitor serial timeout="120":
    python3 scripts/check/check-duo-boot.py --monitor --serial {{ serial }} --timeout {{ timeout }}

# P3.D: prove `duo_boot_check`'s marker chain has teeth. Mutates the observed
# board transcript committed beside the P3.D devlog entry — deleting each
# required marker, reordering the terminal one, appending each failure marker —
# and requires every mutation to fail the checker. Touches no hardware, so it
# guards the checker rather than the board.
[group('hardware')]
duo_gate_control_check:
    python3 scripts/check/check-duo-gate-control.py

# P6.A: deterministic and hermetic — it needs no board, and it writes no block
# device: staging the card is an operator step, because copying onto removable
# media is the one action that can destroy an unrelated disk. Add
# `--qemu-variant` to also build the QEMU `virt` retarget of the same source,
# which is how the instruction stream is executed before it reaches hardware.
#
# Build the H1V1 probe into the flat arm64 `Image` its vendor U-Boot can start.
[group('hardware')]
nt98690_payload_check:
    python3 scripts/build/build-nt98690-payload.py

# P6.A: boot the pinned probe on the named H1V1 and require ordered evidence on
# UART0 at the baud `sel4/pins.toml [ns02201_h1v1]` pins.
#
# Physical: it proves the artifact is this build's, drives the unmodified vendor
# U-Boot over serial, checks the card's bytes against the built image before
# spending a boot, launches with `booti`, and reads the probe's measured facts
# and verdicts. Nothing writes eMMC; the probe returns the board to its vendor
# firmware through PSCI, so a run needs no physical intervention.
#
# The board may be on another machine: `serial` takes a tty path or a
# `tcp:HOST:PORT` bridge. Omitting it runs and fails, never skips.
#
#   just nt98690_boot_check /dev/ttyUSB0
#   just nt98690_boot_check tcp:127.0.0.1:5000
[group('hardware')]
nt98690_boot_check serial="": nt98690_payload_check
    python3 scripts/check/check-nt98690-boot.py {{ if serial == "" { "" } else { "--serial " + serial } }}

# Bring-up aid, not a gate: print whatever the H1V1's UART0 emits and assert
# nothing. It answers the one question `nt98690_boot_check` cannot when the wire
# is silent — whether any byte reaches this host — and it is how the vendor
# banner wording is observed before the gate's markers are frozen against it.
# Exits on its own after 10s of quiet or the timeout.
#
# just nt98690_serial_monitor /dev/ttyUSB0
[group('hardware')]
nt98690_serial_monitor serial timeout="180":
    python3 scripts/check/check-nt98690-boot.py --monitor --serial {{ serial }} --timeout {{ timeout }}

# P6.B: build the H1V1's seL4 kernel, loader, root task, and generation for
# the `aarch64-sel4-nt98690-h1v1` profile into their own prefix and
# directories, and pin the kernel artifacts' hashes. Hermetic; it needs no
# board and writes no block device.
[group('hardware')]
sel4_nt98690_image_check: sel4_pin_check
    python3 scripts/build/build-sel4.py --platform ns02201-h1v1 --skip-pin-check

# P6.B: boot the sample-plane image on the named H1V1 three times over the
# same `booti` handoff P6.A qualified, and require the same ordered root
# evidence the reference planes require, byte-identical normalized traces
# across the three runs, and the board's own return to its firmware after
# each. The AArch64 reference profile passes first, as the roadmap requires.
#
# The root resets the board through the SoC watchdog after every run, so the
# operator power-cycles once, at the start. `serial` takes a tty path or a
# `tcp:HOST:PORT` bridge; omitting it runs and fails, never skips.
#
#   just nt98690_sel4_check /dev/ttyUSB0
[group('hardware')]
nt98690_sel4_check serial="": sel4_root_boot_check sel4_sample_check
    python3 scripts/check/check-nt98690-sel4.py {{ if serial == "" { "" } else { "--serial " + serial } }}
