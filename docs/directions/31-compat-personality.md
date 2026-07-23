# 31. Linux/container compatibility route

| | |
| --- | --- |
| Status | parked |
| Route | compat |
| Depends on | M6 spawn service, endpoint minting, and userspace filesystem service; [Foreign X2](../../roadmap/05-foreign-workloads.md) additionally needs [Hardware H4](../../roadmap/04-platform-hardware.md) IOMMU-enforced DMA and AMD-V enablement |
| Enables | running non-native (Linux/POSIX) workloads — "containers" — under the same generation and capability model as native components; daily-driver mixed workloads |
| Now | Retained design: the syscall-to-capability mapping and personality authority contract feed Foreign X1; guest-VM work is owned by X2. |

## Motivation

README states POSIX/Linux compatibility "may exist later as userspace
personalities or isolated virtual machines" and that Linux "may later
run as an isolated guest." [Foreign X1 and X2](../../roadmap/05-foreign-workloads.md)
now turn those routes into roadmap tracks. A daily-driver that runs native Slime tools,
containers, and agents at once needs a concrete route for the
non-native workloads, and that route must not smuggle in ambient
authority: a Linux process is exactly the kind of code that assumes an
implicit filesystem, environment, and network. Making "container = a
component whose grants are declared by the generation" keeps the
authority model intact while giving the system a way to run software
that was never written for it.

This is the largest gap for the stated daily-driver-plus-containers
target. Everything else in the register refines the native model; this
entry adds the ability to run foreign workloads at all.

## What exists today

- README names the two forms (userspace personality, isolated guest VM)
  as compatibility facilities, explicitly not the native ABI or
  authority model.
- The hardware reference table lists AMD-V, and Foreign X2 now scopes
  guest-VM support.
- The non-goals forbid reproducing FHS, `fork`, signals, or ambient
  path authority as *native* primitives — a personality confines that
  emulation to one component rather than the kernel ABI.
- The agent abstraction already proves the shape: `echo-agent` is a
  component reachable only through declared grants; a container is the
  same contract applied to foreign code.
- The two-form choice is settled: personality first as the main path,
  guest VM added later for high-fidelity compatibility.

## Design sketch

**Personality (first).** A Linux personality is a userspace component
that loads a Linux binary and translates its syscalls into Slime IPC
against declared service capabilities: `open`/`read`/`write` become
object-store or filesystem-service transactions bounded by the
directory grants the generation gave the personality; `socket` traffic
is gated by [entry 18](18-network-authority.md) NetworkDestination
grants; `clock_gettime`/`getrandom` are [entry 3](03-nondeterminism-as-capabilities.md)
clock/entropy capabilities. Anything the container was not granted
returns the Linux errno for "not permitted," so the foreign program
sees a normal-looking but capability-confined kernel. The personality
holds no ambient authority the manifest did not declare, so a container
image plus its grant set is generation data — auditable by
[entry 9](09-grant-graph-introspection.md), diffable by
[entry 1](01-authority-diff-gate.md), rollbackable like every other
component.

**Guest VM (later).** For workloads the personality cannot cover
(custom kernels, syscalls too broad to translate), boot a full Linux
kernel under AMD-V with virtio devices backed by Slime services. The
VM's only authority is the virtio endpoints it is handed — a virtio-blk
capability for one object-store-backed disk, a virtio-net capability
bound to declared destinations — so even a completely opaque guest is
confined to its granted devices. Higher fidelity, much larger
dependency: it needs the Foreign X2 hypervisor subsystem, and its DMA
path needs Hardware H4 IOMMU enforcement before it can touch real hardware.

The upgrade relationship: personality and VM present the *same*
generation-level contract (a foreign workload plus a declared grant
set); they differ only in emulation fidelity and cost. A container
declared in the manifest can move from personality to VM backing
without changing its grants, so the choice is an implementation
attribute, not a new authority model.

## Open questions

- Where is the personality's syscall-translation boundary — a fixed
  supported subset with clean errno failure for the rest, or a
  best-effort surface whose coverage is itself audited?
- Filesystem shape: does a container get a directory capability into
  the shared object store, or a private writable state binding
  ([entry 25](25-resource-accounts.md) quota applies either way)?
- Does the personality run one process or a process tree, and how do
  `fork`/`exec` map onto the spawn service without granting the child
  more than the parent holds?
- Guest-VM device model: which virtio devices are in scope first, and
  does the VM's memory count against the spawner's resource account?
- Is the container image itself a content-addressed object
  (M5.4 store), so image identity and integrity reuse the generation
  verification path?

## Exit-condition sketch

A Linux binary declared as a container in the generation runs under the
personality, reads and writes only the files its declared directory
grant covers, reaches only its declared network destinations, and is
denied everything else with a normal Linux errno — its complete
authority visible in the manifest.

## Probe guidance

M6 now supplies spawn, filesystem, and explicit-context mechanisms. Write
the syscall-to-capability mapping table for a minimal but real workload
(a static busybox-class binary), naming for each syscall the Slime service,
required grant, and errno on absence. Its coverage sizes Foreign X1's
"not permitted" surface and the personality-versus-VM boundary. The
guest-VM half stays paper until Foreign X2 and its Hardware H4 dependency land.
