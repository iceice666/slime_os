# `sel4-channel.zti` — direct endpoint rendezvous

This generation declares one static direct seL4 Endpoint edge at declared slot
0 and gives init executable authority to spawn the scenario's console. The
root-autostart console exits without running the probe; the spawned body receives
`startup_arg = 1` and owns the observed lifecycle.

The spawned console reports unrelated progress before entering
`recv_blocking(0)`. Init reports that its rendezvous send is entering, calls the
blocking direct `send`, and reports completion only after console accepts and
prints the payload. The marker order proves rendezvous backpressure and unrelated
progress while the sender has not completed.

Init then sends the explicit `SLIME.CONSOLE.CLOSE` protocol record. Console
emits `[console] channel close received` and `[console] channel plane complete`
and exits. Init consumes the supervision result, emits
`[init] channel receiver completed`, and only then reports the whole channel
plane complete. No endpoint peer-death inference, runtime endpoint creation,
logical queue depth, or endpoint wait source participates.
