//! Root CSpace addressing. Initial authority retains its full CPtr in branch
//! zero; retype destinations name a CNode and a leaf-local offset separately.

use core::{
    marker::PhantomData,
    sync::atomic::{AtomicBool, Ordering},
};

/// A full address in the installed root CSpace, not an initial CNode index.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RootSlot<T: sel4::CapType = sel4::cap_type::Unspecified> {
    address: usize,
    marker: PhantomData<T>,
}

impl<T: sel4::CapType> RootSlot<T> {
    pub const fn from_address(address: usize) -> Self {
        Self {
            address,
            marker: PhantomData,
        }
    }
    pub const fn index(&self) -> usize {
        self.address
    }
    pub const fn cptr(&self) -> sel4::CPtr {
        sel4::CPtr::from_bits(self.address as _)
    }
    pub const fn cap(&self) -> sel4::Cap<T> {
        self.cptr().cast()
    }
    pub const fn cast<U: sel4::CapType>(&self) -> RootSlot<U> {
        RootSlot::from_address(self.address)
    }
    pub const fn upcast(&self) -> RootSlot {
        self.cast()
    }
}

impl RootSlot {
    pub const fn downcast<T: sel4::CapType>(&self) -> RootSlot<T> {
        self.cast()
    }
}

const ROOT_BITS: usize = 4;
pub const LEAF_BITS: usize = 10;
pub const LEAF_SLOTS: usize = 1 << LEAF_BITS;
const INITIAL_BITS: usize = sel4::sel4_cfg_usize!(ROOT_CNODE_SIZE_BITS);
const _: () = assert!(sel4::WORD_SIZE == 64 && INITIAL_BITS <= 60);
static INSTALLED: AtomicBool = AtomicBool::new(false);

pub fn guard() -> sel4::CNodeCapData {
    if INSTALLED.load(Ordering::Acquire) {
        sel4::CNodeCapData::new(0, 0)
    } else {
        sel4::CNodeCapData::skip_high_bits(INITIAL_BITS)
    }
}

pub fn retype(
    parent: sel4::cap::Untyped,
    blueprint: &sel4::ObjectBlueprint,
    address: usize,
    count: usize,
) -> Result<(), sel4::Error> {
    let root = sel4::init_thread::slot::CNODE.cap();
    let installed = INSTALLED.load(Ordering::Acquire);
    let (path, depth, offset) =
        destination(address, count, installed).ok_or(sel4::Error::RangeError)?;
    let destination = root.absolute_cptr_from_bits_with_depth(path as _, depth);
    parent.untyped_retype(blueprint, &destination, offset, count)
}

fn destination(address: usize, count: usize, installed: bool) -> Option<(usize, usize, usize)> {
    if count == 0 {
        return None;
    }
    if address < 1 << INITIAL_BITS {
        let end = address.checked_add(count)?;
        return (end <= 1 << INITIAL_BITS).then_some((
            0,
            if installed { ROOT_BITS } else { 0 },
            address,
        ));
    }
    if !installed || address >> (sel4::WORD_SIZE - ROOT_BITS) == 0 {
        return None;
    }
    let offset = address & (LEAF_SLOTS - 1);
    (offset.checked_add(count)? <= LEAF_SLOTS).then_some((
        address >> LEAF_BITS,
        sel4::WORD_SIZE - LEAF_BITS,
        offset,
    ))
}

/// Install a new root around the initial CNode before any other root thread
/// starts. Failure is fatal to boot: callers must not publish tasks or resume
/// allocation with an incomplete cutover.
pub fn install(temporary: sel4::cap::CNode) -> Result<(), sel4::Error> {
    let initial = sel4::init_thread::slot::CNODE.cap();
    temporary
        .absolute_cptr_from_bits_with_depth(0, ROOT_BITS)
        .mint(
            &initial.absolute_cptr(initial),
            sel4::CapRights::all(),
            sel4::CNodeCapData::new(0, sel4::WORD_SIZE - ROOT_BITS - INITIAL_BITS).into_word(),
        )?;
    initial.absolute_cptr(initial).delete()?;
    temporary
        .absolute_cptr(initial)
        .copy(&temporary.absolute_cptr(temporary), sel4::CapRights::all())?;
    sel4::init_thread::slot::TCB.cap().tcb_set_space(
        sel4::CPtr::from_bits(0),
        initial,
        sel4::CNodeCapData::new(0, 0),
        sel4::init_thread::slot::VSPACE.cap(),
    )?;
    INSTALLED.store(true, Ordering::Release);
    initial.absolute_cptr(temporary).delete()?;
    Ok(())
}

/// Install one unguarded radix node at an exact tree prefix. The source cap
/// remains owned until the caller deletes it after this copy succeeds.
pub fn install_node(
    source: sel4::cap::CNode,
    prefix: usize,
    depth: usize,
) -> Result<(), sel4::Error> {
    let root = sel4::init_thread::slot::CNODE.cap();
    root.absolute_cptr_from_bits_with_depth(prefix as _, depth)
        .copy(&root.absolute_cptr(source), sel4::CapRights::all())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retype_paths_separate_initial_and_expanded_leaf_offsets() {
        assert_eq!(destination(17, 2, false), Some((0, 0, 17)));
        assert_eq!(destination(17, 2, true), Some((0, ROOT_BITS, 17)));
        let high = 1usize << 60;
        assert_eq!(destination(high + 17, 2, true), Some((high >> 10, 54, 17)));
        assert_eq!(destination(high, 1, false), None);
        assert_eq!(destination(1 << INITIAL_BITS, 1, true), None);
        assert_eq!(destination(high + LEAF_SLOTS - 1, 2, true), None);
        assert_eq!(destination((1 << INITIAL_BITS) - 1, 2, true), None);
        assert_eq!(destination(0, 0, true), None);
        assert_eq!(destination(usize::MAX, 2, true), None);
    }
}

pub const fn leaf_blueprint() -> sel4::ObjectBlueprint {
    sel4::ObjectBlueprint::CNode {
        size_bits: LEAF_BITS,
    }
}

pub const fn blueprint() -> sel4::ObjectBlueprint {
    sel4::ObjectBlueprint::CNode {
        size_bits: ROOT_BITS,
    }
}
