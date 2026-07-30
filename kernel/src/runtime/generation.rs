pub use boot_contracts::generation::{
    Component, DecodeError, Generation, Grant, Object, StateBinding, generation_identity,
};

use boot_contracts::fabric_graph::{self, FabricGraph};
use boot_contracts::generation::{KIND_BOOTSTRAP, KIND_COMPONENT, KIND_RESOURCE};
use boot_contracts::shared_buffer_budget::{
    self, HolderQuota as BudgetQuota, SharedBufferBudget, holder_identity,
};

use crate::capability::MAX_CAPS;
use crate::ipc::{CHANNEL_QUEUE, MAX_MSG};
use crate::memory::shared_buffer::{
    HolderQuota, MAX_BUFFER_PAGES, MAX_LOANS, MAX_MAPPINGS, MAX_SHARED_BUFFERS, MAX_TOTAL_PAGES,
};
use crate::syscall::MAX_WAIT_SOURCES;

// The fabric-graph contract restates a few of this kernel's bounds so the host
// builder can reject an over-declared graph instead of emitting one the kernel
// refuses at boot. Those copies must be this kernel's: if they diverged, a
// graph would be admitted against the wrong bound on one side.
const _: () = assert!(fabric_graph::CONTROL_MESSAGE_BYTES as usize == MAX_MSG);
const _: () = assert!(fabric_graph::KERNEL_TOTAL_PAGES as usize == MAX_TOTAL_PAGES);
const _: () = assert!(fabric_graph::KERNEL_SHARED_BUFFERS as usize == MAX_SHARED_BUFFERS);
const _: () = assert!(fabric_graph::CHANNEL_QUEUE_DEPTH as usize == CHANNEL_QUEUE);
const _: () = assert!(fabric_graph::KERNEL_MAPPINGS as usize == MAX_MAPPINGS);
const _: () = assert!(fabric_graph::KERNEL_LOANS as usize == MAX_LOANS);

pub fn decode(bytes: &[u8]) -> Result<Generation<'_>, DecodeError> {
    let generation = Generation::decode(bytes)?;
    for index in 0..generation.object_count() {
        let object = generation.object(index)?;
        if matches!(object.kind, KIND_BOOTSTRAP | KIND_COMPONENT) {
            crate::component::decode(object.bytes).map_err(|_| DecodeError::BadBounds)?;
        }
    }
    // A shared-buffer budget resource, when present, is validated deterministically
    // before any component launches: a missing, malformed, or globally-impossible
    // budget fails the whole generation closed rather than silently capping at
    // runtime (C7.3).
    if let Some(budget) = budget_object(&generation) {
        let budget = budget.map_err(|_| DecodeError::BadBounds)?;
        budget
            .validate_against(
                MAX_BUFFER_PAGES as u32,
                MAX_TOTAL_PAGES as u32,
                MAX_SHARED_BUFFERS as u32,
                MAX_MAPPINGS as u32,
                MAX_LOANS as u32,
            )
            .map_err(|_| DecodeError::BadBounds)?;
    }
    // A fabric graph resource, when present, is validated the same way: a
    // malformed graph, an impossible declared limit, or an aggregate demand
    // the kernel could never satisfy fails the whole generation closed before
    // the fabric or any client component launches (C8.2).
    if let Some(graph) = fabric_graph_object(&generation) {
        let graph = graph.map_err(|_| DecodeError::BadBounds)?;
        graph
            .validate_against(
                MAX_WAIT_SOURCES as u32,
                MAX_CAPS as u32,
                MAX_TOTAL_PAGES as u32,
                MAX_SHARED_BUFFERS as u32,
                MAX_MAPPINGS as u32,
                MAX_LOANS as u32,
                MAX_MSG as u32,
                CHANNEL_QUEUE as u32,
            )
            .map_err(|_| DecodeError::BadBounds)?;
    }
    Ok(generation)
}

/// Locate the shared-buffer budget resource object, if the generation declares
/// one. A budget is a `KIND_RESOURCE` object whose payload carries the budget
/// magic. At most one is expected; the first matching object wins.
fn budget_object<'a>(
    generation: &Generation<'a>,
) -> Option<Result<SharedBufferBudget<'a>, shared_buffer_budget::DecodeError>> {
    for index in 0..generation.object_count() {
        let object = generation.object(index).ok()?;
        if object.kind == KIND_RESOURCE
            && object.bytes.len() >= 8
            && object.bytes[..8] == shared_buffer_budget::MAGIC
        {
            return Some(SharedBufferBudget::decode(object.bytes));
        }
    }
    None
}

/// Locate the fabric-graph resource object, if the generation declares one.
/// Same shape as [`budget_object`]: a `KIND_RESOURCE` object carrying the
/// fabric-graph magic. At most one is expected; the first match wins.
fn fabric_graph_object<'a>(
    generation: &Generation<'a>,
) -> Option<Result<FabricGraph<'a>, fabric_graph::DecodeError>> {
    for index in 0..generation.object_count() {
        let object = generation.object(index).ok()?;
        if object.kind == KIND_RESOURCE
            && object.bytes.len() >= 8
            && object.bytes[..8] == fabric_graph::MAGIC
        {
            return Some(FabricGraph::decode(object.bytes));
        }
    }
    None
}

/// The fabric graph this generation declares, if any. Returns `None` when the
/// generation declares no data fabric — no component then holds any route
/// authority, because authority is never ambient.
///
/// The generation is assumed already validated by [`decode`], so a present
/// graph re-decodes cleanly; a decode failure here degrades to `None`.
pub fn fabric_graph<'a>(generation: &Generation<'a>) -> Option<FabricGraph<'a>> {
    match fabric_graph_object(generation) {
        Some(Ok(graph)) => Some(graph),
        _ => None,
    }
}

/// The shared-buffer quota a named component should receive under this
/// generation. Returns [`HolderQuota::DENY`] when the generation declares no
/// budget or the component is absent from it — authority is never ambient.
///
/// The generation is assumed already validated by [`decode`], so a present
/// budget re-decodes cleanly; a decode failure here degrades to `DENY`.
pub fn shared_buffer_quota(generation: &Generation<'_>, component: &str) -> HolderQuota {
    let Some(Ok(budget)) = budget_object(generation) else {
        return HolderQuota::DENY;
    };
    let identity = holder_identity(component);
    match budget.quota_for(&identity) {
        Some(BudgetQuota {
            byte_pages,
            buffer_count,
            mapping_count,
            loan_count,
            ..
        }) => HolderQuota {
            byte_pages,
            buffer_count,
            mapping_count,
            loan_count,
        },
        None => HolderQuota::DENY,
    }
}
