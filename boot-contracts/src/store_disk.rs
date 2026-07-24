//! On-disk object-store layout constants (M5.4). The append/commit machinery
//! lives in `kernel/src/object_store.rs`; the layout is pinned by
//! `contracts/store/disk/v1/schema.zt`.

include!("generated/store_disk.rs");
