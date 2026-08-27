# CP3's crate-per-component boundary: every component is its own workspace
# package with one binary and no private manifest parser, the allocator is
# scoped to the six crates that declare it and matches the builder's store
# group, every package carries a release-profile stanza, and no shared source
# remains in `components/bins`.
#
# The allocator arm is the one that needs a gate rather than review. Cargo
# unifies features across every package named in one invocation, so building a
# plain component beside a store component links a `#[global_allocator]` into
# the plain one — measured directly, as 6 heap symbols appearing in the
# `slime_rt` rlib a mixed invocation produced against 0 in a grouped one.
[group('component sdk')]
component_crate_split_check: component_spec_check
    python3 scripts/check/check-component-crate-split.py

# CP4's external-artifact path: the generation builder resolves one component
# through an explicit content-hash-bound ELF mapping, reports the source, signs
# and admits the mixed generation, and refuses hash-mismatched or malformed ELF
# bytes before a generation is signed.
[group('component sdk')]
external_component_admission_check: generation_check
    python3 scripts/check/check-external-component-admission.py

# CP5's out-of-tree development proof: export the versioned component SDK, commit
# it as a pinned git repository, consume it from a distinct RP4 component
# checkout, admit both content-bound ELFs into the demo generation, boot the
# exact signed generation, then remove the checkout and prove the in-tree
# fallback still boots. Since CP6 it consumes the repository-owned exporter
# rather than constructing its own bundle.
[group('component sdk')]
component_sdk_out_of_tree_check: external_component_admission_check
    python3 scripts/check/check-component-sdk-out-of-tree.py

# CP6's deterministic export: one checked-in exporter, invoked twice from the
# same source tree, produces byte-identical self-describing SDK trees. The
# identity is required to move for an allowlisted source and for a pin and to
# hold for two product-only files — without both halves it could be a constant
# or a digest of the whole repository — and the record must decode through its
# generated Zutai binding with every stated digest matching the emitted bytes.
[group('component sdk')]
component_sdk_export_check: external_component_admission_check
    python3 scripts/check/check-component-sdk-export.py

# CP7's permanent publication: one generated commit and one immutable signed tag
# per release, republishing an unchanged tree writes nothing, and the published
# commit regenerates byte-identically from the source commit its own record
# names. A hand edit in the mirror is refused rather than merged, which is what
# makes the mirror generated rather than a second source tree.
[group('component sdk')]
component_sdk_release_check: component_sdk_export_check
    python3 scripts/check/check-component-sdk-release.py

# CP8's platform assets: an external checkout builds target-qualified QEMU and
# RPi component ELFs from one immutable SDK release with `SEL4_PREFIX` poisoned,
# so a build that still succeeds can only have taken its prefix from the release
# record. The RPi arm is host-side qualification: the QEMU profile refuses that
# ELF as wrong-target, and no physical-board claim is made.
[group('component sdk')]
component_sdk_prefix_check: component_sdk_release_check
    python3 scripts/check/check-component-sdk-prefix.py

# CP9's version policy and matrix: two real immutable releases are classified,
# every scalar and structural compatibility axis is moved in isolation and must
# force its expected classification — including equal crate versions across a
# changed syscall ABI — and each published row is backed by a build plus the
# QEMU boot that observed it. An untested pairing reports unsupported.
[group('component sdk')]
component_sdk_compatibility_check: component_sdk_prefix_check
    python3 scripts/check/check-component-sdk-compatibility.py

# CP10's consumer lifecycle: a template consumer pins one release by full
# commit, upgrades to the next with its lockfile, prefix asset, and recorded
# identity in one diff, rebuilds and boots the content-bound generation,
# survives five injected failures with the prior pin intact, and reproduces the
# previous ELF and generation byte-for-byte on rollback.
[group('component sdk')]
component_sdk_upgrade_check: component_sdk_compatibility_check
    python3 scripts/check/check-component-sdk-upgrade.py

# CP2's runtime binding resolution: a component asks the root which of its own
# slots holds a named binding instead of compiling the number in. An unprefixed
# name is a manifest grant; `executable:`/`channel:` reach the boot layout's two
# identity domains for the bootstrap instance, which is what keeps a layout entry
# from shadowing a grant.
#
# The planes are the gate. `sel4_channel_check` asserts the denial arm from both
# the root's line and the component's, and `sel4_loan_check` plus
# `sel4_component_graph_check` boot components that resolve real bindings and
# would fail the rendezvous on a wrong answer. `test_sel4_root` covers the
# name-admissibility guards without a boot.
[group('component sdk')]
runtime_binding_resolution_check: test_sel4_root sel4_gate_control_check sel4_channel_check sel4_loan_check sel4_component_graph_check sel4_dango_check
    @echo "runtime binding resolution: the root resolved named bindings, namespaced boot-layout roles, and unambiguous capability roles for their own instances, refused an ungranted name, and every migrated plane kept its observed behavior"
