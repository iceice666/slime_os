//! Capability-transfer descriptor validation (C8.3).
//!
//! The wire form lives in `contracts/capability-transfer/v1` and is generated
//! into `slime_proto::capability_transfer`. This module owns only the kernel's
//! side of the contract: which descriptors `SYS_CAP_TRANSFER` will act on, and
//! which [`KernelObject`] each declared `object_kind` names.
//!
//! Binding the descriptor to the move is the point. The kernel parses the same
//! bytes the receiving peer will parse and enforces exactly them, so a
//! descriptor can never advertise authority the receiver did not get: the
//! declared object kind must be the moved capability's real kind, and the
//! declared rights mask is the mask the kernel installs. `route_identity` and
//! `direction` are the fabric's own role binding, opaque here — the kernel
//! knows nothing of routes.

pub use slime_proto::capability_transfer::*;

use crate::capability::{KernelObject, RIGHT_ALL, RIGHT_TRANSFER, Rights};

/// Structural validity of a transfer descriptor, independent of the capability
/// it accompanies: magic, version, known flags, a success status, a nonzero
/// rights mask inside the meaningful bit space, and a defined object kind.
///
/// A zero mask is rejected rather than read as "no rights": a move that grants
/// nothing is a drop, and `SYS_CAP_DROP` already spells that. A nonzero
/// `status` marks a denial, which carries no capability and never reaches this
/// syscall.
pub fn valid_transfer(descriptor: &WireCapabilityTransfer) -> bool {
    descriptor.magic == CAPABILITY_TRANSFER_MAGIC
        && descriptor.version == FORMAT_VERSION
        && descriptor.status == 0
        && descriptor.flags & !KNOWN_FLAGS == 0
        && descriptor.rights_mask != 0
        && descriptor.rights_mask & !RIGHT_ALL == 0
        && is_object_kind(descriptor.object_kind)
}

/// Whether `object` is the kind the descriptor's `object_kind` names. An
/// object outside the transferable set has no code, so it can never match.
pub fn kind_matches(object_kind: u32, object: &KernelObject) -> bool {
    is_object_kind(object_kind) && object_kind == kind_code(object)
}

/// The rights the destination receives for `descriptor`: the declared mask,
/// with the transfer meta-right removed unless the descriptor explicitly
/// retains it. Retention is deliberate and separate from the mask, so a
/// descriptor that merely names the transfer bit still hands over a
/// non-transferable capability.
pub fn destination_rights(descriptor: &WireCapabilityTransfer) -> Rights {
    if descriptor.flags & FLAG_RETAIN_TRANSFER != 0 {
        descriptor.rights_mask
    } else {
        descriptor.rights_mask & !RIGHT_TRANSFER
    }
}

/// Whether this version defines `object_kind`. The transferable set is
/// deliberately narrow: the objects a userspace broker legitimately hands to a
/// participant.
fn is_object_kind(object_kind: u32) -> bool {
    matches!(
        object_kind,
        OBJECT_KIND_ENDPOINT
            | OBJECT_KIND_SHARED_BUFFER
            | OBJECT_KIND_SHARED_BUFFER_LOAN
            | OBJECT_KIND_SUPERVISION
    )
}

/// The descriptor code for a kernel object, or `0` for a kind this contract
/// does not carry. `0` is not a defined `object_kind`, so an undeclared object
/// never matches a valid descriptor.
fn kind_code(object: &KernelObject) -> u32 {
    match object {
        KernelObject::Endpoint(_) => OBJECT_KIND_ENDPOINT,
        KernelObject::SharedBuffer(_) => OBJECT_KIND_SHARED_BUFFER,
        KernelObject::SharedBufferLoan(_) => OBJECT_KIND_SHARED_BUFFER_LOAN,
        KernelObject::Supervision(_) => OBJECT_KIND_SUPERVISION,
        _ => 0,
    }
}
