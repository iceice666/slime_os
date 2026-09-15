# Architecture release qualification

## Goal

Qualify architecture releases for their exact target profiles without implying
product-workload or device support. Work-item state lives in `.tasks/items/`.

## RV64 Milk-V Duo architecture release

The release requires P3, P3.D, and P3.E: boot a target-qualified verified
generation through upstream seL4 on the named Milk-V Duo, reach `slime-root`
ready, refuse incompatible artifacts before mapping executable bytes, and replay
the selected architecture-neutral root/component corpus with physical serial
evidence. It claims no product workload and qualifies no storage, USB, network,
display, sensor, or actuator path.

## AArch64 native architecture release

The release requires P0, P1, and P2. It is architecture evidence for
`aarch64-qemu-virt`, not Raspberry Pi 5 or Milk-V Duo hardware evidence.

## Owning references

- [`../architecture/targets-and-portability.md`](../architecture/targets-and-portability.md)
