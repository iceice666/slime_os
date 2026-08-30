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
