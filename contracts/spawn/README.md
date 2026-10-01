# Spawn protocol, version 2

`v2/schema.zt` defines the spawn-service request, reply and argument frame.
Generate the Rust (`components/proto/src/spawn.rs`) and C
(`components/runtime/include/slime/spawn.h`) bindings with
`python3 scripts/generate/generate-spawn-bindings.py`. `v1/` remains as the
retained source of format version 1; spawn-service serves only version 2 and
refuses version 1.

## Messages

Every request, reply and frame is one 64-byte IPC message.

- The **request** carries the command, flags, roles, budget and environment as
  before, plus four argument lengths and their total. At most 4 arguments and
  256 bytes are allowed; unused lengths are zero. A wait request names its
  supervision handle in `supervision_handle`.
- A **frame** carries up to 48 argument bytes: magic `SPPF` (`FRAME_MAGIC`), version 2, a
  sequence number from 0, the byte offset, the length and a final flag. Unused
  payload bytes are zero. A request whose arguments are all empty still ends
  with one empty final frame.
- The **reply** is unchanged in shape. An accepted request or frame that does
  not yet complete the arguments is answered with "would block" (`-3`).

## Validation

`components/proto/src/lib.rs` (`valid_spawn_request`) and
`components/proto/src/spawn_arguments.rs` (`SpawnArguments`) are the single
validator both ends use. A frame that is missing, repeated, out of order,
non-contiguous, past the declared total, finished early or never finished is
refused, and nothing is spawned until the final frame completes the declared
total. spawn-service releases every capability the client handed over on any
refusal, cancels an unfinished request when a fresh header arrives, and drops a
partial request it has not heard from within a bounded number of polls.

spawn-service forwards the validated request and the same canonical frames on
the command's declared launch-context endpoint, so the child reads its
arguments with the same validator (`components/lib/src/launch_context.rs`).
