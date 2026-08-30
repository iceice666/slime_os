# Components

A component is Slime OS's unit of isolation, versioning, and fault
containment: one `no_std` Rust executable in its own seL4 address space, with
its own CSpace, running with exactly the authority its generation declares.

## Not a process

The word "process" carries Unix assumptions that are all false here. A
component has:

- **no ambient environment** — no inherited environment variables, no working
  directory, no open file descriptors, no `PATH`. Spawn supplies nothing
  implicit; if a component needs a working directory or a stdin stream, those
  arrive as explicitly granted capabilities with declared roles.
- **no filesystem identity** — a component is not a file at a path. Its
  executable is generation-module bytes, hash-verified at boot admission and
  referenced by an `Executable` capability. There is no way to run code the
  generation does not carry.
- **no global identity to forge** — the root authenticates every request from
  the badge on the component's endpoint, minted at task construction. There
  is no PID namespace to confuse and no way to speak as another task.
- **no fork, no exec, no signals** — a new component comes only from `SPAWN`
  over a granted executable capability; termination is observed only through
  a supervision handle, as a typed status distinguishing clean exit from
  fault.

## Lifecycle

1. **Declared.** The generation manifest names the executable, its instances,
   and every grant each instance holds. This is the only source of a
   component's authority.
2. **Constructed.** The root builds the whole task — CSpace, VSpace with the
   ELF mapped, TCB, IPC buffer, badged endpoints, fault handler — from a
   bounded per-task arena, before running it.
3. **Running.** The component enters at `slime_rt::entry!`, reads its declared
   bindings, and does its work over IPC. Everything it can do is visible in
   its grant list.
4. **Terminal.** Exit or fault. Either way the root reclaims the entire arena
   — TCBs, CNode, VSpace, frames, every root-side slot — so a dead component
   leaks nothing. A crashing component does not take down its peers, the
   services it used, or the system; its supervisor observes a typed fault and
   applies policy (restart, degrade, report) in userspace.

## Where components live

- Code: one crate per component under
  `components/{system,services,applications,testkit}/<name>/`, entered through
  `src/main.rs`. Shared helpers are in `components/lib`; the syscall surface is
  `components/runtime` (`slime_rt`); build-time support is
  `components/build-support`.
- Authority: the matching generation fixture under
  `contracts/generation-manifest/v1/fixtures/` declares the instance's grants and slot
  layout. A component binary alone never determines its own authority —
  inspect the fixture and the generated boot-layout before reasoning about
  slots.
- Policy vs. mechanism: components own policy (what to do); the root owns
  mechanism (task construction, allocation, supervision) and no policy. If
  you find yourself wanting the root to make a decision, the decision
  belongs in a component.

Small enough to read in one sitting: `components/testkit/echo-agent/src/main.rs`
receives its launch context, validates its explicitly-granted working
directory and stdin, and prints a structured reply — the whole component
model in 64 lines.

## Related

- [Capabilities](capabilities.md) — what a grant actually conveys.
- [Channels](channels.md) — how components talk.
- [Generations](generations.md) — where the declarations come from.
- [Add a component](../getting-started/05-add-a-component.md) — the procedural
  path from an in-tree crate to a composed, authorized, QEMU-observed instance.
- ABI detail: [`../syscall-abi.md`](../syscall-abi.md), including the child
  CSpace layout.
