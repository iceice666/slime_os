# Exhaustively check the bounded BootState transition and interruption model.
[group('contracts')]
bootstate_model_check:
    cargo build --release --manifest-path deps/zutai/Cargo.toml -q -p zutai-cli
    ZUTAI_STDLIB_ROOT=deps/zutai/stdlib deps/zutai/target/release/zutai-cli model-check contracts/bootstate/model/bootstate.zt

# Exhaustively check the bounded capability-rights algebra: narrow-only derive
# and spawn delegation, the consuming export/finalize/import/cancel path, and
# six mutations that must each produce a counterexample.
[group('contracts')]
capability_rights_model_check:
    cargo build --release --manifest-path deps/zutai/Cargo.toml -q -p zutai-cli
    ZUTAI_STDLIB_ROOT=deps/zutai/stdlib deps/zutai/target/release/zutai-cli model-check contracts/capability-rights/model/capability-rights.zt

# Ring occupancy, single-assignment terminal states, exactly-once lease
# release, epoch cutover, and the liveness rule that a live request always
# settles and releases. Six mutations must each produce a counterexample.
#
# Exhaustively check the bounded IO0 request/epoch/lease contract.
[group('contracts')]
io_queue_model_check:
    cargo build --release --manifest-path deps/zutai/Cargo.toml -q -p zutai-cli
    ZUTAI_STDLIB_ROOT=deps/zutai/stdlib deps/zutai/target/release/zutai-cli model-check contracts/io-queue/model/io-queue.zt

# Declared budgets, total charge return on death, single reclamation, epoch
# confinement, stale-acknowledgement refusal, and the liveness rule that a
# faulted driver always reaches zero charges. Seven mutations must each
# produce a counterexample.
#
# Exhaustively check the bounded IO1 hardware-resource charge lifetime.
[group('contracts')]
io_resource_model_check:
    cargo build --release --manifest-path deps/zutai/Cargo.toml -q -p zutai-cli
    ZUTAI_STDLIB_ROOT=deps/zutai/stdlib deps/zutai/target/release/zutai-cli model-check contracts/io-resource/model/io-resource.zt

# Both IO substrate models, for the IO track's own verification stack.
[group('contracts')]
io_model_check: io_queue_model_check io_resource_model_check

# The custom-kernel persistence gates were superseded by the userspace seL4
# planes. These aliases keep historical devlog gate identifiers resolvable.
[group('contracts')]
bootstate_trace_check: bootstate_model_check sel4_rollback_check
    python3 scripts/check/check-bootstate-trace.py

[group('contracts')]
release_trust_check: contracts_check sel4_rollback_check
    python3 scripts/check/check-release-trust.py

# Validate the pinned generation manifest schema and fixtures.
[group('contracts')]
contracts_check: bootstate_model_check capability_rights_model_check io_queue_model_check io_resource_model_check
    python3 scripts/check/check-contracts.py
    python3 scripts/generate/generate-spawn-bindings.py --check
    python3 scripts/check/check-boot-layout-resource.py
    # B42: lifecycle authority is a capability, so no wire record or public
    # runtime type may name a bare task id.
    python3 scripts/check/check-lifecycle-identity.py
    # B50: every generation this repository builds is v5. The manifest's own
    # `formatVersion` is the *manifest* schema's version and says nothing
    # about the wire format, so this builds each one and reads the magic.
    python3 scripts/check/check-generation-v5.py
    # B46: the v2 shared-ring bindings match their contract. Generated code
    # that has drifted from its schema is a hand-written wire format wearing a
    # `@generated` header.
    python3 scripts/generate/generate-fabric-ring-bindings.py --check
    # B59: the root-service ABI is one contract consumed by `slime-root` and
    # `components/runtime`, and `docs/syscall-abi.md` must document every label
    # it declares. Before this the two crates and the doc each held their own
    # copy of the table.
    python3 scripts/generate/generate-syscall-abi-bindings.py --check
    # The language-neutral startup/CSpace/descriptor ABI is generated for both
    # Rust and C consumers; neither side may retain a handwritten layout.
    python3 scripts/generate/generate-component-runtime-abi-bindings.py --check

# P0 target-profile and executable-artifact contract matrix.
[group('contracts')]
architecture_contract_check: contracts_check
    python3 scripts/check/check-architecture-contract.py

[group('contracts')]
interface_schema_check: contracts_check
    python3 scripts/check/check-interface-schema.py

# CP0's component-specification model: every component the reference generation
# declares has a schema-valid `contracts/component-spec/v1` record with a stable
# computed identity, and 37 named malformations are refused.
[group('contracts')]
component_spec_check: contracts_check
    python3 scripts/check/check-component-spec.py

# CP1's generation derivation: `valid.zti` and `sel4-channel.zti` are generated
# from `contracts/system-spec/v1` sources plus the component-spec corpus, must
# regenerate byte-identically, and 17 named malformations are refused.
[group('contracts')]
system_spec_check: component_spec_check
    python3 scripts/check/check-system-spec.py
    python3 scripts/generate/generate-generation-from-spec.py --check

[group('contracts')]
fabric_manifest_check: contracts_check sel4_stream_check
    python3 scripts/check/check-fabric-manifest.py

[group('contracts')]
data_fabric_profile_check: contracts_check
    python3 scripts/check/check-data-fabric-profile.py

# C8.11's roadmap-named gate. Both halves: the contract and its declared sink
# bounds are validated on the host, and the emitted trace is observed on the
# plane that actually advances a clock.
[group('contracts')]
data_fabric_trace_check: contracts_check sel4_trace_check
    python3 scripts/generate/generate-fabric-trace-bindings.py --check

[group('contracts')]
sample_descriptor_check: contracts_check sel4_sample_check
