pub use boot_contracts::generation::{
    Component, DecodeError, Generation, Grant, Object, StateBinding, generation_identity,
};

use boot_contracts::generation::{KIND_BOOTSTRAP, KIND_COMPONENT, KIND_RESOURCE};
use boot_contracts::shared_buffer_budget::{
    self, HolderQuota as BudgetQuota, SharedBufferBudget, holder_identity,
};

use crate::memory::shared_buffer::{
    HolderQuota, MAX_BUFFER_PAGES, MAX_SHARED_BUFFERS, MAX_TOTAL_PAGES,
};

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
