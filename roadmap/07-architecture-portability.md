# Architecture portability track

> **H2 routing — current targets separated from port history.** Read
> [targets and portability](../docs/architecture/targets-and-portability.md) for
> current target/mechanism/evidence boundaries and the
> [RPi5](../docs/plans/rpi5-ros2-demo.md) and
> [Framework](../docs/plans/framework-hardware.md) plans for qualification scope.
> Completed P0–P3 and P5–P6 delivery chronology is routed to immutable history
> below. P4's board-specific corpus and firmware/media/serial acceptance and the
> prospective MCU companion profile remain here. Historical observations are
> not current image qualification. See the [file classification](README.md).

<a id="p4--raspberry-pi-5-board-bring-up"></a>

## P4: Raspberry Pi 5 physical architecture qualification

**Depends on:** P5, which supplies the kernel this board runs. P2's custom-kernel AArch64 slice is superseded and is not a prerequisite.

### Why serial is the only evidence path

Recorded here because "just use HDMI" is the obvious question, and the answer is
structural rather than a missing feature.

seL4 ships exactly three driver families — `deps/sel4/src/drivers/{serial,timer,smmu}`.
There is no display, framebuffer, HDMI, storage, or network driver anywhere in
the kernel, and that is the design: everything except the mechanism needed to
*be* a microkernel lives in userspace. A grep for framebuffer or HDMI output in
`src/`/`include/` finds only x86 `boot_sys.c` and an unrelated `.dts`.

Even the serial driver is not really an exception. It exists only to implement
`printf`/`seL4_DebugPutChar` for debugging, is selected by device-tree
`compatible` string (`arm,pl011` → `pl011.c` for this board), and is compiled
out entirely in the verified configuration — `AARCH64_bcm2712_verified.cmake`
sets `KernelVerificationBuild ON`, which forces `KernelPrinting OFF`. So the
kernel's console is a debug facility that the proof configuration deliberately
removes, not a supported output device.

That leaves three ways a Pi 5 could ever show output, and only one is available
now:

1. **Debug UART** (current): `seL4_DebugPutChar` over UART10, which the board's
   own `overlay-rpi5.dts` designates through `seL4,elfloader-devices`. Needs a
   working USB-UART adapter. This is what P4 is blocked on.
2. **A userspace display driver**: real work, and much larger than it sounds on
   this SoC — the Pi 5's video output sits behind the RP1 southbridge across
   PCIe, so it needs PCIe enumeration, address translation, and HDMI/VC
   bring-up before a single pixel, all as generation-declared device authority.
   Roadmap invariant 2 puts it in userspace, and invariant 4 requires a real
   gate. It is also the wrong shape for boot evidence: a framebuffer cannot
   report a fault that happens before it is mapped.
3. **JTAG/SWD** over the same 3-pin header: needs a debug probe and gives
   register state rather than a transcript, so it diagnoses a wedge but does not
   produce the ordered marker evidence P4's exit condition asks for.

Roadmap invariant: framebuffer output alone is never milestone completion. That
rule already anticipated this — a display would not close P4 even if it existed.
The cheap unblock is a different USB-UART adapter.

### Deliverables

- select one exact Raspberry Pi 5 board revision or accepted revision set, firmware version, boot path, removable storage medium, interrupt topology, timer, serial path, and minimum device set;
- build the `bcm2712` upstream seL4 kernel and loader image from the existing pins and platform configuration, alongside the current `sel4/config/qemu-arm-virt.cmake`, and pin its artifact hashes the same way;
- source the board's memory map, UART, GIC, and timer facts from seL4's BootInfo and its platform configuration rather than from any Slime-side board table;
- record reproducible removable-media images, generation/release identities, firmware and board identities, normalized device tree/topology, serial evidence, storage-integrity boundaries, and every granted device capability;
- qualify DMA, storage writes, networking, sensors, and actuators only through their owning IO, demo, or hardware milestones; a CPU boot does not promote an untested peripheral or backend;
- replay the AArch64 QEMU semantic corpus on the board where physically meaningful, labeling hardware-only differences instead of hiding them;
- provide the board evidence consumed by RP3, RP4, RP7, and RP8.

### Required checks

- the named Raspberry Pi 5 runs the isolated native vertical slice from reproducible media and preserves every declared no-write or exact-device storage boundary;
- firmware changes, wrong board revisions, unsupported page/interrupt profiles, and missing required devices fail with bounded diagnostics rather than silently selecting a nearby profile;
- physical timer, interrupt, reset, serial, and storage behavior is recorded separately from inherited QEMU evidence;
- the board can run at least two isolated components and report a bounded data-path transcript before any ROS layer is claimed.

### Planned verification target

```sh
just rpi5_boot_check
```

### Exit condition

One named Raspberry Pi 5 profile runs the verified isolated Slime vertical slice with reproducible firmware/media evidence and no unqualified device or storage claim; this physical evidence is available to the RPi5 ROS 2 demo track.

## MCU and embedded-companion boundary

Cortex-M, RV32 microcontrollers, and other systems without the admitted MMU and user/supervisor isolation baseline do not run a weakened form of this kernel. They are external devices reached through bounded userspace services.

A later companion profile may admit micro-ROS/XRCE-DDS, `zenoh-pico`, or a smaller Zutai protocol over an exact serial, CAN, USB, or network capability. `zenoh-pico` is a candidate *here* and not as a Slime component: its C toolchain, `z_malloc`/`z_realloc`/`z_free` requirement, and BSD-socket-shaped port API are properties of the microcontroller it runs on rather than obligations on Slime, and the demo's Zenoh transport is what it would peer with. That profile must declare peer identity, types, directions, payload size, frequency, queue depth, timeout, reset behavior, and actuator authority. Disconnect, malformed traffic, reboot, and resource exhaustion become structured C8/C9 events; the companion never receives ambient graph, network, storage, or device authority.
