# RP0 format 1 (historical): exact Raspberry Pi 5 board/firmware/media and
# ROS 2 Jazzy DDSI-RTPS Profile 0 acceptance fixture. Retained as the record of
# what RP0/RP1 originally admitted; format 2 is the live contract.
[group('hardware')]
rpi5_ros2_demo_contract_check:
    python3 scripts/check/check-rpi5-ros2-demo-contract.py

# RP0 format 2 (live): the same board/firmware/media contract with the transport
# family carried as data rather than frozen into the schema, selecting ROS 2
# Kilted over a bounded Zenoh Profile 0. Every wire constant -- the RIHS01 type
# hash, the data key expression, the per-sample CDR bytes, the 33-byte
# attachment -- is derived and compared rather than transcribed, and the hash
# implementation is itself validated against an upstream `ros2/rcl` fixture.
[group('hardware')]
rpi5_ros2_demo_contract_v2_check:
    python3 scripts/check/check-rpi5-ros2-demo-contract-v2.py

# RP1 derives the demo's executable closure from the live format-2 contract, so
# it depends on that gate. Format 1's gate stays in the chain because RP0/RP1's
# recorded exit conditions were observed against it.
[group('hardware')]
rpi5_artifact_check: architecture_contract_check rpi5_ros2_demo_contract_check rpi5_ros2_demo_contract_v2_check
    python3 scripts/check/check-rpi5-artifacts.py

# P4: configure, build, and install seL4 for the `bcm2712` Raspberry Pi 5
# platform, build the root child, root task, and loader against that prefix, and
# write `build/slime-sel4-bcm2712-rpi5.identity.json`. A separate prefix, cargo
# target directory, generation, image, and pinned artifact hash set from the
# qemu-arm-virt build, because it is a different kernel for a different board:
# every executable in it is admitted for `aarch64-rpi5` alone.
[group('hardware')]
sel4_rpi5_image_check: sel4_pin_check
    python3 scripts/build/build-sel4.py --platform bcm2712-rpi5 --skip-pin-check

# P4: flatten the packaged RPi5 ELF into the exact boot files
# `sel4/pins.toml [bcm2712_rpi5].boot_files` pins — `kernel8.img` and
# `config.txt`. `objcopy -O binary` cannot do this: the loader's payload lives
# in program headers carrying no sections, so objcopy silently drops it and
# emits an image that boots nothing. Writes no block device.
[group('hardware')]
rpi5_media_check: sel4_rpi5_image_check
    python3 scripts/build/build-rpi5-media.py

# P4: boot the pinned bytes on the named Raspberry Pi 5 and require ordered
# generation, timer, task, fault, and ready evidence on UART10 at the baud
# `contracts/rpi5-ros2-demo/v2` pins. Physical: it proves the media is this
# build's, then reads a real serial device. A QEMU pass cannot complete this
# milestone (roadmap invariant 8), so a missing board, adapter, or media is a
# failure and never a skip.
#
# Requires the operator to copy `build/rpi5-media/*` onto the FAT32 boot
# partition and reset the board:
#   just rpi5_media_check
#   cp build/rpi5-media/* /Volumes/<BOOT>/ && diskutil unmount /Volumes/<BOOT>
# just rpi5_boot_check /dev/cu.usbserial-XXXX
[group('hardware')]
rpi5_boot_check serial="": sel4_pin_check rpi5_artifact_check
    python3 scripts/check/check-rpi5-boot.py {{ if serial == "" { "" } else { "--serial " + serial } }}

# Bring-up aid, not a gate: print whatever the Pi 5's debug UART emits and
# assert nothing. Builds no artifacts and qualifies no board, so it answers the
# one question `rpi5_boot_check` cannot when the wire is silent — whether any
# byte reaches this host. Exits on its own after 10s of quiet or the timeout.
#
# just rpi5_serial_monitor /dev/cu.usbserial-120
[group('hardware')]
rpi5_serial_monitor serial timeout="120":
    python3 scripts/check/check-rpi5-boot.py --monitor --serial {{ serial }} --timeout {{ timeout }}
