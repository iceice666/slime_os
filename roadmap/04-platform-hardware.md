# Platform hardware

**Purpose:** Deliver a Framework daily-driver hardware platform through typed, capability-routed services, deterministic QEMU/host checks, explicit DMA and storage safety gates, and reproducible physical evidence.

**Status:** In progress. H1's bounded inventory and host evidence harness are implemented and QEMU-verified; the required physical Framework evidence record is pending. The current Framework removable image has previously reported no usable physical keyboard input, and H4 is the first slice allowed to claim a working physical keyboard.

**Dependencies:** [Foundations](01-foundations.md), especially the retained M5.7 storage-safety boundary; [Core runtime](02-core-runtime.md), especially C7 generation-v3 authority and C9 scheduling authority; [ROS 2 compatibility](03-ros2-compatibility.md) consumes H6 networking; [Authority and trust](06-authority-trust.md) owns A1 revocation, A2 secrets, and A3 general accelerator authority.

The Hardware track promotes hardware in two distinct steps: deterministic mechanism and fault handling under QEMU first, then an observed Framework run with the exact generation-declared device grant. A QEMU pass never substitutes for physical evidence. DMA-capable physical drivers remain trusted and read-only until H4 installs IOMMU containment; internal NVMe writes remain disabled until H7 completes every promotion gate.

Sequencing:

- H1 inventories the actual Framework firmware and device topology; later drivers consume that evidence rather than guessed BDFs, interrupt routes, or protocols.
- H2 gates every userspace hardware driver. H3 proves xHCI, USB, and HID logic under QEMU; H4 adds AMD-IOMMU containment and promotes USB HID on the Framework.
- H5 and H6 consume H4 and may proceed in parallel. H7 consumes H4 plus H5's disposable external-media path.
- H8 may proceed after H2 because it uses the boot GOP framebuffer; H9 consumes the H1 ACPI inventory. H10 consumes every device service that must quiesce and resume.
- H11 and H12 are independent once their buses, input/audio/network services, and resume hooks exist. H13 consumes H4, H8, and H10. H14 is the integrated acceptance slice and consumes all preceding slices.

### H1: Framework evidence harness and hardware inventory

**Status:** Implementation complete; physical Framework verification pending. `just framework_inventory_check` passes, but the exit condition remains open until `evidence/framework-inventory.jsonl` records the target Framework topology, localized keyboard failure, and byte-identical internal-NVMe comparison region.

Deliverables:

- emit one bounded, versioned hardware report over both framebuffer and serial without requiring keyboard input: ACPI table identities and checksums, MCFG functions, validated BARs, interrupt routes, framebuffer geometry, IOMMU-table presence, NVMe identity, and every input-controller initialization stage;
- add an append-only host-side evidence record containing image identity, machine identity, firmware version, report output, and pre/post hashes for every storage device whose immutability is being asserted;
- keep the report free of raw memory, firmware secrets, storage payloads, and unbounded descriptor dumps;
- preserve the removable-image no-internal-write rule and provide a non-interactive timeout or shutdown path so a failed input driver cannot strand the test.

Required checks:

- malformed ACPI, PCI, BAR, interrupt, and descriptor fixtures fail with a typed bounded error rather than a hang;
- two boots on unchanged firmware produce the same normalized topology report, excluding explicitly named volatile fields;
- the Framework report identifies whether the keyboard path is i8042, USB HID, or another firmware-described controller and records the exact initialization failure;
- the internal NVMe comparison region remains byte-identical across the inventory boot.

Planned verification target:

```sh
just framework_inventory_check
```

Exit condition: the repository contains reproducible evidence for the target Framework's actual controller topology and the current keyboard failure is localized to a named initialization stage.

### H2: Userspace driver authority ABI

**Status:** Not started.

H2 consumes [C7's generation-v3 and shared-sample foundation](02-core-runtime.md#c7-bounded-resource-and-shared-sample-plane). C7 owns the deterministic v2-to-v3 migration, `u64` rights, retained-v2 decoding, and the general `SharedBufferFactory` create/map/release quotas; H2 owns the hardware-facing MMIO, DMA, IRQ, supervision, and rollback integration every later userspace driver consumes.

Deliverables:

- consume C7's generation v3 `u64` rights and deterministic builder migration without changing v2 meanings; stage-0 continues to decode retained v2 known-good generations during the rollback window;
- expose bounded operations for mapping only the granted PCI function's validated BAR ranges, allocating/mapping/releasing charged DMA buffers, waiting for and acknowledging only the granted interrupt, and using C7's bounded shared buffers;
- make DMA, IRQ, and MMIO explicit named capabilities with hard per-driver quotas in the generation, and apply C7's shared-buffer quotas to driver IPC; remove the six ungated device-right exceptions from the capability matrix as their gates land;
- supervise driver components so timeout, fault, peer loss, and reset are reported without terminating unrelated services; driver restart receives fresh mappings and cannot reuse stale DMA or IRQ authority;
- declare every new driver IPC protocol schema-first under `contracts/`; clients see typed device services, never ambient PCI enumeration or raw global MMIO.

Required checks:

- an ungranted component cannot enumerate PCI functions, map a BAR, allocate DMA memory, receive or acknowledge an interrupt, or map another component's buffer;
- BAR offsets, mapping lengths, DMA page counts, outstanding requests, interrupt queues, and shared-buffer totals are bounded before allocation or mapping;
- a driver cannot program DMA outside buffers in its account at the software boundary, and reclamation remains impossible while a request is outstanding;
- C7 proves that v2 and v3 artifacts are byte-deterministic for fixed normalized input and rejects unknown versions and rights bits; H2 proves that rollback to a retained v2 generation still boots with hardware authority failing closed;
- driver failure revokes its mappings and returns charged resources before a supervised restart.

Planned verification target:

```sh
just driver_abi_check
```

Exit condition: a manifest-declared userspace driver can access exactly one PCI function, bounded DMA buffers, its interrupts, and typed service endpoints without ambient hardware authority, while C7's generation and shared-buffer checks remain satisfied.

### H3: xHCI, USB core, and HID under QEMU

**Status:** Not started.

This slice proves the USB implementation before physical DMA promotion. It does not yet claim safe Framework xHCI operation.

Deliverables:

- implement a userspace xHCI driver over the H2 ABI with bounded command, event, and transfer rings and explicit controller reset/timeout handling;
- implement root-hub and bounded hub enumeration, descriptor validation, address/configuration selection, endpoint lifecycle, disconnect cancellation, and per-device identities derived from topology plus validated descriptors;
- implement USB HID keyboard and pointer boot protocols, key rollover bounds, press/release events, and hotplug;
- route i8042 and USB HID through one versioned seat/input service so Dango and later compositor clients do not depend on the physical transport;
- keep physical xHCI bus mastering disabled until H4 supplies an IOMMU domain.

Required checks:

- malformed, cyclic, oversized, short, or inconsistent USB descriptors are rejected before ring or buffer allocation;
- transfer timeout, controller reset, endpoint stall, surprise removal, and event-ring overflow leave no pinned buffer or permanently wedged input service;
- one client holding seat input authority receives events while an ungranted component cannot read or inject them;
- scripted QEMU keyboard input drives the native Dango REPL through the same seat protocol used by physical HID;
- repeated attach/detach remains within fixed device, endpoint, transfer, and queue bounds.

Planned verification target:

```sh
just usb_hid_check
```

Exit condition: QEMU xHCI keyboard and pointer input survive malformed devices, hotplug, timeout, and reset through a transport-independent input service.

### H4: AMD IOMMU containment and Framework USB HID promotion

**Status:** Not started.

Deliverables:

- parse the target's ACPI IVRS data with strict bounds and identify the AMD IOMMU and device aliases from H1 evidence;
- create one IOMMU domain per DMA-capable driver, map only its live H2 DMA buffers with the requested direction, invalidate translations before reuse, and enable PCI bus mastering only after the domain is active;
- report IOMMU faults with device identity, access type, and bounded address detail; quarantine the offending driver and complete its supervision handle with a distinct failure;
- boot the Framework xHCI driver inside an enforced domain and bring up the actual built-in or attached USB keyboard identified by H1;
- retain an emergency removable image that does not enable bus mastering and can reproduce the inventory report if IOMMU setup fails.

Required checks:

- a synthetic driver DMA outside its mapped region faults and cannot modify the guard region or another component's memory;
- stale translations are unusable after buffer release, driver restart, device reset, and generation transition;
- unsupported or malformed IVRS data fails closed before bus mastering;
- IOMMU fault storms are bounded and cannot livelock the kernel or suppress unrelated device interrupts;
- on the Framework, the keyboard can type `$(sysinfo)` into Dango, receive `result:exit:0`, use Backspace and Shift punctuation, and close with Escape while internal NVMe comparison hashes remain unchanged.

Planned verification targets:

```sh
just iommu_check
just usb_hid_check
```

Exit condition: the Framework has observable keyboard input through the common seat service, and xHCI DMA is confined to generation-declared buffers by the AMD IOMMU.

### H5: USB mass storage and removable-device identity

**Status:** Not started.

Deliverables:

- implement one standards-based USB mass-storage transport proven by the target test device, with bounded command/data/status phases, sense decoding, timeout, reset, and surprise-removal handling;
- expose USB media through the existing block protocol so filesystem, object-store, recovery, and generation services receive no transport-specific authority;
- identify media by USB topology plus validated device identity and render an operator-visible distinction between removable test media and the internal NVMe;
- require an exact BlockDevice capability for all reads and writes; default Framework generations grant removable media read-only and grant writes only to an explicitly selected disposable device;
- make removal during an outstanding write fail the request and preserve the last committed filesystem/object-store root.

Required checks:

- malformed capacity, short transfer, failed sense, timeout, reset, and unplug paths release every DMA mapping and return a structured block error;
- a writable grant for one USB disk cannot address a second USB disk or internal NVMe;
- removing media at every tested commit boundary preserves the previous committed root;
- repeated enumerate/read/write/flush/remove cycles stay within fixed resource bounds;
- a Framework run writes and verifies a disposable external device while pre/post internal-NVMe hashes remain identical.

Planned verification target:

```sh
just usb_storage_check
```

Exit condition: Slime OS has a capability-selected disposable physical storage target suitable for destructive reliability work without granting internal NVMe write authority.

### H6: Network service, USB Ethernet, and destination authority

**Status:** Not started.

H6 enables [ROS R1](03-ros2-compatibility.md#r1-ros-2-topic-wire-profile). ROS networking work may begin when H6 exits; it does not wait for unrelated compositor, platform UI, audio, wireless, or GPU slices H8-H13.

Deliverables:

- declare versioned link, IP, DNS, UDP, TCP, and network-service protocols under `contracts/`, with a deterministic virtio-net QEMU backend and one Framework USB-Ethernet backend selected from H1 descriptors;
- implement bounded Ethernet, ARP/NDP, IPv4/IPv6, ICMP, DHCP/SLAAC, UDP, TCP, and exact-name DNS resolution sufficient for native update and diagnostic clients;
- add a `NetworkDestination` object in generation v3 identifying transport, exact IP address or exact DNS name, and port, with distinct CONNECT, SEND, RECV, and LISTEN rights; wildcard destinations are not admitted in the Hardware track;
- keep DNS authority inside the network service: resolving an exact declared name does not grant the requesting component arbitrary resolver or raw-packet access;
- account socket count, queued bytes, retransmission state, and per-destination traffic against bounded manifest data.

Required checks:

- a component granted one destination connects only to that exact name/address, transport, and port; alternate address, port, DNS name, raw packet, and listen attempts fail closed;
- malformed frames, DHCP options, DNS messages, fragmented packets, TCP options, retransmission exhaustion, and peer loss do not exceed bounds or wedge unrelated connections;
- the manifest and authority-diff tooling enumerate every reachable destination;
- QEMU transfers deterministic data to an allowed endpoint while a simultaneous denied endpoint receives no packet;
- the Framework obtains a lease/address through USB Ethernet and reaches one declared endpoint after link unplug/replug and after a driver restart.
- after R1/R2 deterministic container conformance passes, a dedicated wired Framework ↔ 64-bit Raspberry Pi 4/5 fixture runs the same pinned Jazzy probes under Fast DDS and Cyclone DDS separately, records packet capture and link/peer restart behavior, and preserves Framework storage integrity; this supplements but never replaces the R1/R2 QEMU gates.

Planned verification target:

```sh
just network_check
```

Exit condition: native components have useful wired networking while the generation, not ambient socket access, determines every reachable destination.

### H7: Native NVMe reliability and internal-storage promotion

**Status:** Not started. Internal NVMe writes remain prohibited until this slice's physical evidence is recorded.

Deliverables:

- move the native NVMe transport behind the H2 userspace driver ABI and H4 IOMMU domain while preserving the common block protocol;
- add bounded interrupt-driven read, write, flush, timeout, abort/reset, and reinitialization paths for the exact target controller and namespace;
- exercise destructive native-NVMe tests only with a dedicated replaceable NVMe installed for testing; the personal internal device is absent during those tests;
- validate flush ordering, forced reset, abrupt power loss, torn metadata, malformed GPT/object-store/generation/BootState records, and recovery across the M5 commit boundaries;
- introduce a separately named internal-storage promotion profile requiring the exact controller/namespace identity and an explicit operator-visible arming step; ordinary Framework and recovery images remain unable to write it;
- record pre/post full metadata-region hashes and rollback/recovery results before promoting any internal write grant.

Required checks:

- IOMMU containment, queue bounds, LBA bounds, timeout/reset, flush ordering, interrupted writes, and malformed metadata pass under QEMU fault injection and on the sacrificial physical NVMe;
- reset or power loss at every recorded commit boundary leaves a bootable known-good generation or signed removable recovery path;
- a wrong controller, namespace identity, capacity, firmware-reported block size, or missing operator arm fails before the first write command;
- the internal-storage profile grants BLOCK_WRITE and BOOT_UPDATE only to the declared storage/generation service and cannot be selected by an unprivileged component;
- the first write-enabled run on the target Framework is observed separately and preserves the retained known-good and recovery roots.

Planned verification target:

```sh
just storage_reliability_check
```

Exit condition: internal NVMe writes are enabled only after deterministic and physical reliability evidence, IOMMU confinement, exact device identity, and rollbackable recovery are all observed.

### H8: Software compositor and desktop shell over GOP

**Status:** Not started.

Deliverables:

- move visible output from the global debug stream to a userspace compositor holding the sole framebuffer capability; retain serial as diagnostics rather than the interactive UI path;
- declare versioned surface, damage, presentation, seat-focus, clipboard, and dialog protocols; clients render only into bounded shared buffers and cannot map the physical framebuffer;
- implement software composition, double buffering, damage tracking, cursor rendering, focus, keyboard/pointer routing, and a terminal surface hosting the native Dango session;
- render the M6 powerbox as a compositor-owned modal dialog while preserving its user-gesture capability mint and cancellation semantics;
- make display dimensions, pixel format, surface count, buffer bytes, damage rectangles, and event queues explicit bounds.

Required checks:

- an ungranted client cannot read input, capture another surface, map the framebuffer, steal focus, or bypass a powerbox dialog;
- malformed dimensions, overflowed strides, out-of-bounds damage, stalled clients, and compositor restart cannot write outside granted buffers;
- deterministic scene fixtures produce byte-identical software-composited frame hashes under QEMU;
- Dango remains usable while another client faults or floods damage/events within its quota;
- on the Framework, terminal text, pointer movement, focus changes, and powerbox selection are visible and survive a compositor restart.

Planned verification target:

```sh
just compositor_check
```

Exit condition: the Framework has an isolated, capability-routed graphical session over GOP without granting applications ambient framebuffer or input access.

### H9: Battery, charger, brightness, lid, and thermal service

**Status:** Not started.

Deliverables:

- consume H1 ACPI/EC evidence through a bounded target-specific ACPI resource evaluator; unsupported AML constructs fail closed rather than growing an implicit general interpreter;
- expose versioned battery charge/rate/health, AC/charger state, lid state, thermal readings/trips, and panel brightness through a userspace platform service;
- keep mechanism in the kernel and policy in userspace: a generation-declared power-policy component decides brightness and thermal responses through explicit control endpoints;
- distinguish read-only telemetry from control authority in the capability matrix and give ordinary applications neither EC register access nor charger/thermal control;
- emit bounded state-change events and provenance for user brightness changes and thermal emergency actions.

Required checks:

- malformed AML/resource data, EC timeout, implausible sensor values, event storms, and missing methods produce typed degraded states without hangs;
- a telemetry-only client cannot change brightness, charger behavior, thermal policy, reset, shutdown, or sleep state;
- brightness remains within firmware-advertised bounds and a thermal emergency cannot be masked by an unprivileged component;
- QEMU fixtures cover AC attach/detach, charge/discharge, lid transitions, and thermal thresholds;
- Framework readings agree with firmware-visible state and brightness/lid events are observed without storage modification.

Planned verification target:

```sh
just platform_service_check
```

Exit condition: the Framework reports and controls its basic power state through explicit service capabilities while platform policy remains outside the kernel.

### H10: Suspend/resume lifecycle and device reinitialization

**Status:** Not started.

The target sleep state is selected from H1 firmware evidence; the slice does not claim both S3 and modern standby when the machine exposes only one supported route.

Deliverables:

- define a checked suspend state machine covering request, service quiesce, storage flush, device stop, IOMMU teardown, platform entry, resume, IOMMU restore, device reinitialization, and userspace thaw;
- require explicit acknowledgements from storage, compositor, input, network, audio, and generation services before entering sleep; timeout aborts suspend and leaves the system running;
- restore monotonic time, interrupt routing, xHCI, network, NVMe, display, and later audio/wireless devices without reusing stale DMA mappings or capabilities;
- preserve pending-generation attempt semantics across sleep and never treat resume as a fresh successful boot or health confirmation;
- expose lid-close and user-request policy in userspace, with an emergency kernel path only for thermal safety.

Required checks:

- failure or timeout at every quiesce/resume stage either aborts to a working session or enters signed recovery on next boot; it never silently advances BootState;
- no device can DMA while its IOMMU domain is torn down, and all post-resume DMA buffers are freshly mapped;
- network reconnect, USB re-enumeration, input, compositor, storage, and audio recover without duplicating capabilities or leaking bounded resources;
- QEMU fault injection covers every transition edge and repeated cycles;
- the Framework completes at least 50 lid/user suspend-resume cycles, including cycles with active network and audio, without data corruption or loss of Dango control.

Planned verification target:

```sh
just suspend_check
```

Exit condition: the Framework repeatedly suspends and resumes through a checked, rollback-safe lifecycle with all active device services re-confined and usable.

### H11: I2C touchpad and audio service

**Status:** Not started. Touchpad and audio are independent implementations and may proceed in parallel after their H2/H9 bus and resource descriptions are available.

Touchpad deliverables and checks:

- implement the Framework's firmware-described I2C controller and HID-over-I2C touchpad path with bounded reports, interrupt handling, reset, and resume;
- route pointer contacts through the common seat service; gesture recognition, acceleration, palm rejection, and tap policy live in userspace;
- reject malformed descriptors/reports and prove an ungranted component cannot read raw contacts or inject pointer events;
- physically verify pointer movement, click, two-finger scroll, palm rejection, hot reset, and resume.

Audio deliverables and checks:

- select the HDA or ACP route from observed hardware/codec evidence rather than implementing an unused backend by assumption;
- declare a versioned PCM service with bounded shared rings, playback/capture stream capabilities, volume/mute control, device routing, underrun/overrun reporting, and resume;
- give the mixer sole hardware authority; clients cannot map audio DMA or read microphone samples without an explicit capture grant;
- verify deterministic virtual-backend mixing, client fault isolation, bounded latency, underrun recovery, speaker/headphone output, microphone capture, mute indication, and repeated suspend/resume on the Framework.

Planned verification targets:

```sh
just touchpad_check
just audio_check
```

Exit condition: the Framework has daily-usable touchpad input and capability-isolated playback/capture through services that survive reset and suspend.

### H12: MT7925 Wi-Fi and Bluetooth

**Status:** Not started.

Deliverables:

- package required device firmware as content-addressed, release-authorized generation objects; drivers never search a global firmware path;
- implement the MT7925/RZ717 Wi-Fi path inside an H4 IOMMU domain and attach it as another backend to the H6 network service;
- support scan, association, WPA2/WPA3 personal authentication, reconnect, regulatory constraints, and suspend/resume with bounded stations, frames, retries, and key material lifetime;
- accept Wi-Fi credentials through an explicit interactive service and keep them scoped to the network service; [A2](06-authority-trust.md) generalizes non-readable and revocable secrets rather than being bypassed here;
- implement the combo device's observed Bluetooth transport, bounded HCI, pairing state, and the HID and audio profiles required for keyboard/pointer and headset use through the existing input/audio services.

Required checks:

- malformed firmware, frames, scan results, association responses, HCI events, pairing messages, and retry storms remain bounded and fail closed;
- Wi-Fi traffic remains subject to the same `NetworkDestination` grants as Ethernet; switching links does not widen reachable destinations;
- ungranted components cannot read credentials, link keys, raw wireless frames, Bluetooth HID events, or microphone audio;
- disconnect, radio reset, driver restart, and suspend/resume clear transient keys and reconnect without stale DMA mappings;
- on the Framework, Wi-Fi reaches one declared destination and Bluetooth keyboard/pointer plus headset audio work after repeated reconnect and resume cycles.

Planned verification targets:

```sh
just wifi_check
just bluetooth_check
```

Exit condition: the Framework has capability-preserving wireless networking and the Bluetooth input/audio paths needed for daily use.

### H13: Radeon display control and graphics acceleration

**Status:** Not started.

Deliverables:

- package Radeon firmware as release-authorized generation objects and run the display driver with exact PCI, interrupt, buffer, and IOMMU authority;
- take over the internal panel from GOP, set its validated native mode, page-flip compositor buffers, control the hardware cursor, recover from display/GPU faults, and restore the panel after suspend;
- accelerate compositor-owned copy, fill, scaling, and composition operations while retaining the H8 software renderer as the correctness oracle and fallback;
- keep command submission private to the compositor/display service in the Hardware track; general application or compute queues and accelerator authority remain [A3](06-authority-trust.md) scope;
- integrate panel brightness and thermal limits with H9 without moving policy into the driver.

Required checks:

- command buffers, relocations, dimensions, pitches, addresses, and synchronization waits are validated before submission and remain within IOMMU-mapped compositor buffers;
- a faulting or timed-out queue resets to the software compositor without exposing stale frames or wedging input;
- accelerated output matches the software renderer's deterministic frame hashes for the supported operations;
- applications cannot submit GPU commands, map scanout buffers they do not own, or bypass compositor focus/powerbox rules;
- the Framework internal panel runs at its validated native mode, remains stable during interactive use, and recovers through repeated suspend/resume and forced driver reset.

Planned verification target:

```sh
just radeon_display_check
```

Exit condition: the Framework compositor owns a stable, IOMMU-contained Radeon display path with verified software fallback and no general ambient GPU authority.

### H14: Energy accounting and integrated daily-driver qualification

**Status:** Not started.

Deliverables:

- attribute scheduler-active time and bounded service work (IPC, storage, network, audio, and graphics bytes/events) to components and supervision subtrees;
- combine those counters with H9 battery/platform telemetry into readable per-component energy estimates and declare their schema in the generation;
- keep Hardware-track accounting as telemetry and threshold events, not hidden scheduler authority; [A1 revocation](06-authority-trust.md) and [C9 scheduling-class authority](02-core-runtime.md#c9-robot-runtime-authority) arrive explicitly in their owning tracks;
- provide an operator-visible hardware status and authority view covering current generation, device drivers, IOMMU domains/faults, storage identity, network destinations, power state, and per-component resource/energy use;
- define and record one reproducible Framework qualification run covering interactive console/compositor use, wired and wireless networking, external removable storage, internal rollbackable storage, audio, touchpad, Bluetooth, display acceleration, suspend/resume, and recovery media.

Required checks:

- accounting totals are monotonic within the declared window, bounded, attributable, and cannot be reset or forged by the measured component;
- shared-service charging rules are fixed and tested so repeated forwarding cannot create or erase usage;
- an eight-hour Framework session with concurrent interactive, network, audio, storage, and background load retains input/display responsiveness and shows no unbounded task, capability, DMA, queue, or buffer growth;
- the qualification run includes at least 50 suspend/resume cycles, repeated USB and network hotplug, forced driver restarts, a failed pending generation, and signed recovery without unauthorized storage writes;
- every physical test records image/generation identity, firmware version, hardware identities, authority manifest, observed output, and storage-integrity evidence.

Planned verification target:

```sh
just daily_driver_check
```

Exit condition: the Framework target sustains the complete native daily-driver workload with explicit hardware and network authority, IOMMU-contained DMA, rollbackable storage, observable power/resource use, and repeatable physical evidence.

## Hardware track verification stack

Every permanent change runs the narrowest QEMU or host-side scenario exercising its new behavior. Every physical promotion additionally records a removable-media Framework run; QEMU evidence alone cannot complete a hardware slice. The repository gates remain mandatory:

```sh
just contracts_check
just generation_check
just test
just fmt_check
just lint
just fmt_check_components
just lint_components
just framework_safety_check
```

Planned slice targets:

```sh
just framework_inventory_check
just driver_abi_check
just usb_hid_check
just iommu_check
just usb_storage_check
just network_check
just storage_reliability_check
just compositor_check
just platform_service_check
just suspend_check
just touchpad_check
just audio_check
just wifi_check
just bluetooth_check
just radeon_display_check
just daily_driver_check
```

## Hardware track definition of done

The Hardware track is complete only when all of the following are observed on the target Framework and backed by the deterministic checks above:

- every DMA-capable physical driver runs in an AMD-IOMMU domain that maps only its live generation-declared buffers, with fault isolation and supervised restart;
- the built-in or attached keyboard, touchpad, pointer, display, audio, USB storage, USB Ethernet, Wi-Fi, and Bluetooth paths are usable through typed services rather than ambient hardware access;
- applications receive input, display surfaces, audio streams, files, and network destinations only through explicit capabilities; the manifest can answer which component can reach which device or remote endpoint;
- internal NVMe writes have passed the [M5.7](01-foundations.md) and H7 bounds, reset, flush, interruption, malformed-metadata, device-identity, rollback, and recovery gates on disposable hardware before promotion on the target device;
- the compositor and Radeon driver survive client faults, driver reset, and suspend/resume with a software-rendered fallback;
- battery, charger, brightness, lid, and thermal state are observable, controls are explicitly authorized, and userspace owns policy;
- suspend/resume repeatedly quiesces and restores storage, IOMMU, USB, input, display, network, wireless, and audio without stale DMA authority or BootState corruption;
- per-component resource and energy accounting is visible and bounded, without silently introducing the [C9 scheduling-authority](02-core-runtime.md#c9-robot-runtime-authority) or [A1 revocation](06-authority-trust.md) models;
- the integrated physical qualification run completes with no unauthorized internal-storage modification, no unbounded resource growth, and a signed removable recovery path that remains bootable.
