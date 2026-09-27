# Component runtime tour

What a component is written against. [Add a component](05-add-a-component.md)
taught you the crate shape and the declarations; this page is the API you
put inside `main`, organized by what you are trying to do, with the refusal
each call can meet and the system-spec declaration that prevents it.

The crate is `slime-rt` (`components/runtime`). Every function below is a
thin wrapper over one root operation labeled in
[`syscall-abi.md`](../syscall-abi.md), or over one direct seL4 invocation on a
native object the root installed in your CSpace. Nothing here has an ambient
fallback: if the generation did not grant it, the call is refused.

## Lifecycle

```rust
#![no_std]
#![no_main]

slime_rt::entry!(main);

fn main(startup_arg: u32) {
    slime_rt::debug_write(b"[hello] running\n");
}
```

- `entry!` installs a 64 KiB stack, the Rust runtime, the IPC buffer, and the
  bound transfer window, then calls `main` and `exit(0)` if it returns.
  `entry!(main, worker = w)` additionally declares a second thread's stack and
  entry the root resolves by symbol; a component gets at most two threads.
- `startup_arg` is the authenticated boot action for the bootstrap instance
  (`init`) and zero for everyone else. Any component can ask
  `slime_rt::boot_action()` instead of trusting a build flag; that is how one
  image boots under every plane that declares it.
- `exit(status) -> !` is the clean end. `unhealthy() -> !` tells the root the
  component considers the generation unhealthy before exiting, which a
  selector image turns into `SLIME_BOOT unhealthy`.
- **A panic is `exit(1)` with no message.** The panic handler discards the
  payload. Allocation failure reaches the same handler. If you need a reason
  in the transcript, print it and exit yourself:

  ```rust
  fn fail(reason: &[u8]) -> ! {
      slime_rt::debug_write(b"[hello] fail: ");
      slime_rt::debug_write(reason);
      slime_rt::debug_write(b"\n");
      slime_rt::exit(1)
  }
  ```

  The `[name] fail: ` prefix is what every plane gate lists as a failure
  marker, so this ends the run with the reason attached.

## Output

`debug_write(bytes) -> i64` sends the bytes to the root's console service on
the fixed console slot (32), which prints them as one line — deliberately not
per-byte `seL4_DebugPutChar`, so markers from different components do not
interleave. Include your own `\n`. Non-UTF-8 is refused by the root.

## Finding your capabilities

Grant slots are the component's own numbering, starting at 0. There are two
legitimate ways to know a slot:

1. **A pinned slot.** The system spec's `slotPins` entry with
   `reason = "componentAbi"` freezes the slot for a grant, and the component
   hard-codes it (`const CARRIER_SLOT: u32 = 0;`). Use this for the one or two
   slots a component's protocol genuinely fixes.
2. **Resolve by name at runtime.** `resolve_binding(name) -> Result<u32, i64>`
   asks the root for the slot of a grant bound to *this* instance:

   | Name form | Resolves |
   | --- | --- |
   | `b"my-grant"` | the grant of that name bound to the caller |
   | `b"kind:sharedBufferFactory+bufferCreate"` | by kind and rights superset; ambiguity is refused |
   | `b"executable:console"` / `b"channel:name"` | boot-layout roles — bootstrap instance only |
   | `b"notification:my-signal+signal"` | a notification binding by role |
   | `b"minted:name"` / `b"owned-minted:name"` | a minted binding |

   Prefer this. Unpinned slots are assigned deterministically by grant name
   within the holder, and a spec edit can move them; a resolved name cannot
   go stale.

Fixed slots you never declare: `ROOT_SERVICE_SLOT = 1` (your badged root
endpoint), console at 32, native endpoints from 33, a received transferred
endpoint lands at 127.

**Refusals:** `SLIME_GRAPH binding unresolved task=N` — the name is not bound
to you; `SLIME_GRAPH request rejected … error=…` with a missing right — the
grant exists but without the right the operation checks. Both are fixed in the
system spec, never in code.

## Talking to another component

Native endpoints are seL4 objects the root creates from an `endpoint` grant in
the system spec and installs into both holders' CSpaces:

```
grants = [{
  name = "hello-peer"; capabilityKind = "endpoint";
  source = "hello"; target = "peer";
  rights = ["send"; "recv"]; transferable = false;
}];
```

Then, with `MAX_MSG` (64 bytes) and `MAX_CAPS_PER_MSG` (1) from `slime_rt`:

```rust
// sender
if slime_rt::send(PEER_SLOT, b"ping", &[]) != slime_rt::ERR_SUCCESS {
    fail(b"send");
}

// receiver
let mut payload = [0u8; slime_rt::MAX_MSG];
let mut caps = [0u64; slime_rt::MAX_CAPS_PER_MSG];
loop {
    match slime_rt::recv(PEER_SLOT, &mut payload, &mut caps) {
        slime_rt::ERR_WOULDBLOCK => slime_rt::yield_now(),
        n if n < 0 => fail(b"recv"),
        n => break n as usize,   // bytes received
    }
}
```

- `send` / `try_send` (non-blocking, best effort) / `recv` (non-blocking,
  poll with `yield_now`) / `recv_blocking` / `call` (one `seL4_Call`,
  request and reply) / `reply`.
- A message carries at most one capability. `caps[0] != 0` after a `recv`
  means one landed; its value is the slot it was placed in.
- The in-tree minimum is `init` ↔ `crossing-peer`
  (`components/lib/src/crossing_plane.rs` and
  `components/testkit/crossing-peer/src/main.rs`, composed by
  `contracts/system-spec/v1/systems/sel4-crossing.zti`): ping/pong over two
  pinned endpoints plus one delegated, narrowed copy.

**Notifications** are the cheap signal: `notification_signal(slot)`,
`notification_wait(slot) -> Result<u64, i64>` (the badge word),
`notification_poll`. Declared in the spec's `notifications` list with a
`signal` binding for one holder and a `wait` binding for the other.

## Waiting on several things

`WaitSet` multiplexes notification badges, endpoint slots, and a timer over
one declared notification:

```rust
let mut set = slime_rt::WaitSet::declared(NOTIFICATION_SLOT)?; // sources from the root
set.register_timer();
loop {
    set.wait()?;                                  // blocks; queues the ready set
    while let Some(ready) = set.next_ready() { /* ready.kind / ready.slot */ }
}
```

The composition must declare the holder in its `waitSet` list (with
`waitSetObject = true`); without one `WaitSet::declared` sees no sources. The state machine lives in
`boot_contracts::wait_set`; `just wait_set_check` is its gate.

## Time

`monotonic_read()`, `monotonic_frequency()`, `timer_arm(delay) ->
Result<u64, i64>`, `timer_cancel()`. All require a `clock-authority`
declaration for the holder; omission denies every clock operation. Bounded:
four live timers per holder. `simulated_time_*` exist for the replay plane
and are refused elsewhere.

## Spawning and supervising

```rust
let exe = slime_rt::resolve_binding(b"executable:child")?;
let child = slime_rt::spawn(exe, &[
    slime_rt::SpawnGrant { slot: some_slot, rights: RIGHT_SEND | RIGHT_RECV },
])?;
// ... later
match slime_rt::supervision_status(child.supervision_slot) {
    Ok(None) => { /* still running */ }
    Ok(Some(slime_rt::Termination::Exit(0))) => { /* clean */ }
    Ok(Some(other)) => { /* Exit(n) | Fault(code) | Timeout | PeerLoss | Unhealthy */ }
    Err(_) => { /* handle already collected or not yours */ }
}
```

- `RIGHT_*` constants come from `boot_contracts::generation` (the generated
  rights vocabulary); `OBJECT_KIND_*` from `slime_proto::capability_transfer`.
- The executable slot must carry `exec` + `spawn`; the holder's declared
  `spawnBudget` bounds live children (`spawn_budget()` reads the remainder).
- Each `SpawnGrant` is a *non-consuming narrowed copy* of one of your own
  slots; the child receives them at slots 0, 1, 2… **in ascending declared
  order**, so the grant list's order is load-bearing.
- `spawn` returns only an opaque supervision slot; task identity stays
  root-local. Collecting a terminal status consumes the handle.
  `cap_drop(slot)` releases a live child's handle without waiting.
- A parent sees `Termination::Fault(code)`, never an address.
- `supervision_derive(slot)` makes a transferable copy for handing to a
  client; `spawn-service` does this for Slisp.

`components/system/init/src/main.rs` is the product spawner and the reference
for this pattern.

## Handing a capability to someone else

```rust
slime_rt::capability_delegate(
    endpoint_slot,            // where it travels
    capability_slot,          // what you are giving
    CapabilityDisposition::Retain, // or Move
    OBJECT_KIND_ENDPOINT,     // what you claim it is
    RIGHT_SEND,               // ceiling: narrow only
    &descriptor,              // 64 bytes the receiver decodes
)?;
```

The receiver's `recv` reports the landing slot in `caps[0]`, or it calls
`capability_import()` for the protocol form. The root authenticates kind and
rights from your request, not from the descriptor bytes; the grant must be
`transferable = true`; a receiver declared `deterministic` cannot be widened.
Only endpoints, shared buffers, loans, supervision handles, and directories
travel.

## Shared memory

`shared_buffer_create(pages)` needs a `sharedBufferFactory` grant with
`bufferCreate` *and* a shared-buffer budget for the holder
(`bufferBytePages`, `bufferCount`, `mappingCount`, `loanCount` in its
placement). Then `map`, `unmap`, `seal`, `loan` / `loan_map` / `return` /
`revoke`, `occupancy`. A mapping lands outside your private-memory window;
slices you pass to others carry no physical address. `just sel4_loan_check`
is the gate.

## Heap

Two mutually exclusive features on `slime-rt`:

- `heap` — a fixed 256 KiB bump allocator (no free). Needed only when a
  dependency uses `alloc` (the GPT and object-store decoders do).
- `private-heap` — a real free-list allocator growing through
  `private_memory_grow` in 4-page steps; requires a `privateMemoryBudget`
  quota in the generation or the build refuses.

An authority-free leaf component enables neither. The two are built in
separate cargo invocations because feature unification would register two
global allocators; `just component_crate_split_check` enforces the grouping.

## Introspection

`generation_composition` (which composition am I in), `graph_read` /
`graph_query` (my own fabric rows), `capability_slot_occupancy`,
`scheduling_class_read`, `lifecycle_*`, `recording_participation`. All are
self-scoped: the root answers only about the badge it authenticated, so a
component can read its own declaration but never another's.

## Driver-only surface

`io_device_bind`, `io_mmio_*`, `io_dma_*`, `io_queue_map`, `io_irq_ack`,
`io_request_*` exist for components with an `io-resource` budget and a device
grant. Read [I/O substrate](../architecture/io-substrate.md) before touching
them; ordinary services never hold this authority.

## Shared code

`components/lib` (`slime-components`) holds helpers used by more than one
crate: `block_io` (synchronous block client over IO0), `network_io` and
`http`, `link_frames`, `virtio_mmio`, `uart16550`, `tick_clock`, the fabric
brokers, and the plane scenarios. If a second component needs a module from
your crate, move it there rather than including it across crates.

## Next

- [Recipes](11-recipes.md) — the common changes, each as a checklist of files.
- [`syscall-abi.md`](../syscall-abi.md) — every label, operand, and error.
- [`capability-matrix.md`](../capability-matrix.md) — every right and bound.
