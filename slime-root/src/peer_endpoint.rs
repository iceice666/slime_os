//! Endpoints two components use to talk to each other directly (B46).
//!
//! The object a logical channel is being replaced by. Today every message
//! between components crosses the root twice — sender calls the root, the root
//! copies into a queue, receiver calls the root, the root copies out — and
//! `slime-root` re-proves atomicity, blocking, and peer death that the kernel
//! already supplies. A peer endpoint is one seL4 Endpoint both ends hold, so
//! a send is a `seL4_Send` and the root is not involved.
//!
//! # What the root keeps
//!
//! Creation and rights. Which components may hold which end is exactly the
//! authority the generation declares, and a component that could create its
//! own endpoint and hand it around would be minting authority. So the root
//! creates the object, mints one capability per side with the rights that
//! side's grant allows, and installs them. After that it is not in the path.
//!
//! # Why the sides are separate capabilities
//!
//! seL4 rights on an Endpoint are per capability: write is send, read is
//! receive, grant carries capabilities along. A declaration that says one side
//! may only send becomes a capability without read, which the kernel enforces
//! on every invocation — rather than a rights word the root checks on each
//! message, which is what the logical channel does today.

use crate::object_allocator::{AllocError, ObjectAllocator, TaskArenaId};

/// Peer endpoints one generation may create.
///
/// Matches the logical `MAX_CHANNELS` it replaces, so a graph that fits today
/// still fits during the cutover while both exist.
pub const MAX_PEER_ENDPOINTS: usize = 48;

/// Where a channel's native endpoint sits in the holder's CSpace, relative to
/// the logical slot its grant declares.
///
/// A component migrating off the logical channel finds its endpoint at
/// `NATIVE_ENDPOINT_BASE + declared_slot`, so the mapping needs no table on
/// either side and no second declaration in the manifest. The base is above
/// `CHILD_SLOT_CONSOLE` because grant slots start at zero and the console is
/// the highest fixed slot the root installs.
///
/// A CNode too small to hold the offset slot is refused rather than wrapped:
/// `absolute_cptr_from_bits_with_depth` would otherwise resolve a slot inside
/// the declared region and the endpoint would land on top of real authority.
pub const NATIVE_ENDPOINT_BASE: sel4::CPtrBits = 33;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PeerEndpointError {
    TableFull,
    Unknown,
    /// A side was asked for that this endpoint did not create.
    UnknownSide,
    Alloc(AllocError),
    Mint(sel4::Error),
    /// The holder's CNode cannot hold the offset slot.
    SlotOutOfRange {
        slot: sel4::CPtrBits,
        limit: sel4::CPtrBits,
    },
}

impl From<AllocError> for PeerEndpointError {
    fn from(error: AllocError) -> Self {
        Self::Alloc(error)
    }
}

/// Which end of a peer endpoint a capability names.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Side {
    /// May send, and may not receive.
    Producer,
    /// May receive, and may not send.
    Consumer,
    /// May do both. A loopback grant, where one component holds both ends.
    Both,
}

impl Side {
    /// The seL4 rights this side's capability carries.
    ///
    /// `grant` accompanies send authority because a message may carry one
    /// capability, and `grant_reply` accompanies receive so a server can reply
    /// to a `Call` — those are the two the kernel checks, and withholding
    /// either would make the endpoint unable to do what its side declares.
    fn rights(self) -> sel4::CapRights {
        match self {
            Self::Producer => sel4::CapRightsBuilder::none()
                .write(true)
                .grant(true)
                .build(),
            Self::Consumer => sel4::CapRightsBuilder::none()
                .read(true)
                .grant_reply(true)
                .build(),
            Self::Both => sel4::CapRights::all(),
        }
    }

    pub const fn name(self) -> &'static str {
        match self {
            Self::Producer => "producer",
            Self::Consumer => "consumer",
            Self::Both => "both",
        }
    }
}

struct Entry {
    /// The endpoint itself, root-held so it outlives either peer's capability
    /// and can be reclaimed with the arena that owns it.
    object: sel4::cap::Endpoint,
    /// The logical channel this endpoint replaces, while both exist.
    ///
    /// The cutover cannot happen in one commit -- 312 call sites across 41
    /// component files -- so each declared channel gets its kernel object now
    /// and components move onto it one at a time. This is the pairing that
    /// lets a migrated component find the endpoint for the channel its
    /// declaration names.
    channel: u32,
}

/// Peer endpoints the root has created.
pub struct PeerEndpointTable {
    entries: [Option<Entry>; MAX_PEER_ENDPOINTS],
    len: usize,
}

impl PeerEndpointTable {
    pub const fn new() -> Self {
        Self {
            entries: [const { None }; MAX_PEER_ENDPOINTS],
            len: 0,
        }
    }

    pub const fn len(&self) -> usize {
        self.len
    }

    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Create an endpoint two components will share.
    ///
    /// From the root's global pool, not a task arena: a declared channel
    /// outlives both its peers -- that is what lets a service whose launcher
    /// exited keep serving, and what makes a respawned instance find its
    /// channel still there. An arena-owned endpoint would be revoked with
    /// whichever task happened to own the arena.
    pub fn create(
        &mut self,
        allocator: &mut ObjectAllocator,
        channel: u32,
    ) -> Result<u32, PeerEndpointError> {
        let index = self
            .entries
            .iter()
            .position(Option::is_none)
            .ok_or(PeerEndpointError::TableFull)?;
        let object = allocator
            .allocate_fixed::<sel4::cap_type::Endpoint>()?
            .cap();
        self.entries[index] = Some(Entry { object, channel });
        self.len += 1;
        Ok(index as u32)
    }

    /// Mint one side's capability.
    ///
    /// The rights come from the side, not from a caller-supplied mask: a
    /// producer capability that could also receive would let a sender drain
    /// its own messages before the peer saw them, and no declaration can ask
    /// for that.
    pub fn mint_side(
        &self,
        allocator: &mut ObjectAllocator,
        arena: TaskArenaId,
        endpoint: u32,
        side: Side,
    ) -> Result<sel4::cap::Endpoint, PeerEndpointError> {
        // The *minted capability* is arena-owned even though the object is
        // not: a capability belongs to the holder's CSpace and must go when
        // the holder does, while the endpoint behind it survives.
        let entry = self
            .entries
            .get(endpoint as usize)
            .and_then(Option::as_ref)
            .ok_or(PeerEndpointError::Unknown)?;
        let minted = allocator
            .reserve_slot_in::<sel4::cap_type::Endpoint>(arena)?
            .cap();
        let root_cnode = sel4::init_thread::slot::CNODE.cap();
        root_cnode
            .absolute_cptr(minted)
            .mint(
                &root_cnode.absolute_cptr(entry.object),
                side.rights(),
                // No badge. A peer endpoint has exactly two ends and the
                // receiver knows which side it is; badging would be the root
                // identifying senders, which is what it does today and what
                // this removes.
                0,
            )
            .map_err(PeerEndpointError::Mint)?;
        Ok(minted)
    }

    /// Where a declared slot's native endpoint goes, or why it cannot.
    pub fn native_slot(
        declared: sel4::CPtrBits,
        cnode_size_bits: usize,
    ) -> Result<sel4::CPtrBits, PeerEndpointError> {
        let slot = NATIVE_ENDPOINT_BASE.checked_add(declared).ok_or(
            PeerEndpointError::SlotOutOfRange {
                slot: declared,
                limit: 0,
            },
        )?;
        let limit: sel4::CPtrBits = 1 << cnode_size_bits;
        if slot >= limit {
            return Err(PeerEndpointError::SlotOutOfRange { slot, limit });
        }
        Ok(slot)
    }

    /// The endpoint standing in for a logical channel, if one was created.
    ///
    /// Linear over a table bounded by `MAX_PEER_ENDPOINTS`: this runs once per
    /// migrated operation, and an index would be a second structure to keep
    /// consistent with the first for no measurable gain at 48 entries.
    pub fn for_channel(&self, channel: u32) -> Option<u32> {
        self.entries.iter().enumerate().find_map(|(index, entry)| {
            entry
                .as_ref()
                .filter(|entry| entry.channel == channel)
                .map(|_| index as u32)
        })
    }

    /// The root-held object, for teardown and for signalling.
    pub fn object(&self, endpoint: u32) -> Result<sel4::cap::Endpoint, PeerEndpointError> {
        self.entries
            .get(endpoint as usize)
            .and_then(Option::as_ref)
            .map(|entry| entry.object)
            .ok_or(PeerEndpointError::Unknown)
    }
}

impl From<crate::graph::Side> for Side {
    /// The logical side a declaration names, as the native rights it implies.
    ///
    /// One-to-one, which is what makes the migration mechanical: a
    /// declaration that said "may only send" already meant a capability
    /// without receive, and the kernel is now the thing that enforces it.
    fn from(side: crate::graph::Side) -> Self {
        match side {
            crate::graph::Side::Producer => Self::Producer,
            crate::graph::Side::Consumer => Self::Consumer,
            crate::graph::Side::Loopback => Self::Both,
        }
    }
}

impl PeerEndpointTable {
    /// Install the endpoint standing in for `channel`, if one exists.
    ///
    /// Returns the slot it landed in, or `None` when there is no endpoint for
    /// the channel or the install failed. The caller reports rather than
    /// aborts: the logical end is already installed and working, so a graph
    /// that does not use the native slot must not fail to launch over it.
    #[allow(clippy::too_many_arguments)]
    pub fn install_for(
        &self,
        allocator: &mut ObjectAllocator,
        arena: TaskArenaId,
        channel: u32,
        side: Side,
        cnode: sel4::cap::CNode,
        cnode_size_bits: usize,
        declared_slot: sel4::CPtrBits,
    ) -> Option<sel4::CPtrBits> {
        let endpoint = self.for_channel(channel)?;
        self.install(
            allocator,
            arena,
            endpoint,
            side,
            cnode,
            cnode_size_bits,
            declared_slot,
        )
        .ok()
    }

    /// Install one side's capability into a child's CSpace.
    ///
    /// Beside the logical end, not instead of it: both exist through the
    /// cutover so a component can be migrated on its own without its peers
    /// changing. A component that has not migrated never looks at this slot.
    pub fn install(
        &self,
        allocator: &mut ObjectAllocator,
        arena: TaskArenaId,
        endpoint: u32,
        side: Side,
        cnode: sel4::cap::CNode,
        cnode_size_bits: usize,
        declared_slot: sel4::CPtrBits,
    ) -> Result<sel4::CPtrBits, PeerEndpointError> {
        let slot = Self::native_slot(declared_slot, cnode_size_bits)?;
        let minted = self.mint_side(allocator, arena, endpoint, side)?;
        let root_cnode = sel4::init_thread::slot::CNODE.cap();
        cnode
            .absolute_cptr_from_bits_with_depth(slot, cnode_size_bits)
            .copy(&root_cnode.absolute_cptr(minted), side.rights())
            .map_err(PeerEndpointError::Mint)?;
        Ok(slot)
    }
}

impl Default for PeerEndpointTable {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::{NATIVE_ENDPOINT_BASE, PeerEndpointError, PeerEndpointTable, Side};

    /// `CapRights::new` takes them in this order, which is easy to get
    /// backwards; naming the arguments here keeps each test readable.
    fn rights(grant_reply: bool, grant: bool, read: bool, write: bool) -> sel4::CapRights {
        sel4::CapRights::new(grant_reply, grant, read, write)
    }

    #[test]
    fn a_producer_sends_and_may_carry_a_capability_but_cannot_receive() {
        // The property the kernel enforces on every invocation, which the
        // logical channel checks per message in the root instead. A producer
        // that could read would drain its own messages before the peer saw
        // them; one without grant could not carry the single capability an
        // IPC message may hold.
        assert_eq!(
            Side::Producer.rights(),
            rights(false, true, false, true),
            "producer: grant + write only"
        );
    }

    #[test]
    fn a_consumer_receives_and_may_reply_but_cannot_send() {
        // Without `grant_reply` a server cannot answer a `Call`, which would
        // silently disable synchronous RPC rather than refuse it.
        assert_eq!(
            Side::Consumer.rights(),
            rights(true, false, true, false),
            "consumer: grant_reply + read only"
        );
    }

    #[test]
    fn a_native_slot_sits_above_every_fixed_root_slot() {
        // Grant slots start at zero and the console is the highest slot the
        // root installs, so a base below it would put an endpoint on top of
        // declared authority in every migrated fixture.
        assert!(
            NATIVE_ENDPOINT_BASE > crate::task::CHILD_SLOT_CONSOLE,
            "the native region must not overlap the fixed slots"
        );
        assert_eq!(
            PeerEndpointTable::native_slot(0, 6).expect("slot 0 in a 64-slot CNode"),
            NATIVE_ENDPOINT_BASE
        );
        assert_eq!(
            PeerEndpointTable::native_slot(2, 6).expect("slot 2"),
            NATIVE_ENDPOINT_BASE + 2
        );
    }

    #[test]
    fn a_cnode_too_small_for_the_offset_refuses_rather_than_wrapping() {
        // The failure this prevents is silent: `absolute_cptr_from_bits_with_depth`
        // resolves a slot inside the declared region, so the endpoint would
        // land on top of real authority rather than fail to install.
        assert!(matches!(
            PeerEndpointTable::native_slot(0, 5),
            Err(PeerEndpointError::SlotOutOfRange { .. })
        ));
        // The boundary: a 64-slot CNode holds base + 30 and not base + 31.
        assert!(PeerEndpointTable::native_slot(30, 6).is_ok());
        assert!(matches!(
            PeerEndpointTable::native_slot(31, 6),
            Err(PeerEndpointError::SlotOutOfRange { .. })
        ));
    }

    #[test]
    fn a_loopback_side_holds_both_directions() {
        // One component holding both ends is a declared case -- a
        // `source == target` grant -- and must not be crippled by the split.
        assert_eq!(Side::Both.rights(), sel4::CapRights::all());
    }

    #[test]
    fn no_side_is_all_rights_by_accident() {
        // The split is the point. If either directed side widened to `all`,
        // the kernel would stop enforcing the direction and nothing else in
        // the root would notice.
        assert_ne!(Side::Producer.rights(), sel4::CapRights::all());
        assert_ne!(Side::Consumer.rights(), sel4::CapRights::all());
        assert_ne!(Side::Producer.rights(), Side::Consumer.rights());
    }
}
