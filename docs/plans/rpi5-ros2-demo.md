# Raspberry Pi 5 ROS 2 two-node demo

## Goal

Run two local Slime-hosted ROS 2 nodes on the named Raspberry Pi 5, exchanging a
bounded typed topic through a real admitted middleware wire profile. QEMU is
required regression coverage but cannot satisfy the board claim.

## Fixed boundary

- Raspberry Pi 5 is an exact target, not generic Arm support.
- ROS concepts stay in userspace. The kernel and root gain no node, topic,
  discovery, executor, or middleware policy.
- Native fabric may carry internal delivery, but the acceptance path must include
  a real bounded middleware transport session and CDR payload mapping.
- Names, domains, types, sessions, destinations, files, devices, clocks, and
  parameters grant nothing without explicit capabilities.
- The initial profile excludes arbitrary discovery, multicast scouting,
  unrestricted LAN access, services/actions, unmodified desktop packages,
  Python, Gazebo, GPU acceleration, Wi-Fi, and Framework support.

## Required sequence

1. retain the existing demo contract and target-qualified build/admission path;
2. obtain physical serial boot evidence for the named board and required minimum
   board services;
3. prove the component data path under AArch64 QEMU and then the board;
4. implement the node/runtime envelope using clock, wait-set, lifecycle,
   parameter, private-memory, and exact network-destination authority;
5. implement the bounded transport/topic profile and two node components;
6. observe physical data transfer, then repeatability, malformed input,
   disconnect, restart, and bounded-resource behavior.

The network path depends on
[`network-data-plane.md`](network-data-plane.md); the current exact-destination
service does not provide a TCP byte stream.

## Physical evidence rule

The current Raspberry Pi 5 kernel, loader, and removable-media files build
reproducibly, but the board boot remains unobserved because the available UART
path produced no serial record. Milk-V Duo, Framework, and QEMU evidence cannot
substitute. A different evidence channel requires its own bounded implementation
and identity binding; documentation does not weaken the serial exit condition.

## Later compatibility

Broader external topic interoperability, ROS services/actions, and existing ROS
workload personalities remain later plans. The minimal demo neither implies nor
requires them.

## Owning references

- `contracts/rpi5-ros2-demo/v2/`
- `contracts/target-profile/v1/`
- [`../architecture/typed-data-fabric.md`](../architecture/typed-data-fabric.md)
- [`../architecture/runtime-authority.md`](../architecture/runtime-authority.md)
- [`../architecture/targets-and-portability.md`](../architecture/targets-and-portability.md)
- the RPi5/ROS work items under `.tasks/items/`
