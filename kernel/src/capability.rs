// SPDX-License-Identifier: GPL-3.0-only
//! Capability table — handles with rights, derivation, delegation, revocation.
//!
//! A capability is an unforgeable reference to an object plus a rights bitmap
//! (D11). The capability table is per cell; a handle is an index plus a
//! generation counter, so a stale handle cannot be reused after revocation.
//!
//! Rights: read, write, execute, map, dma, invoke, transfer, dup, revoke, manage.

/// Capability rights bitmap.
#[derive(Clone, Copy, Debug, Default)]
pub struct Rights(pub u32);

impl Rights {
    pub const READ: u32 = 1 << 0;
    pub const WRITE: u32 = 1 << 1;
    pub const EXECUTE: u32 = 1 << 2;
    pub const MAP: u32 = 1 << 3;
    pub const DMA: u32 = 1 << 4;
    pub const INVOKE: u32 = 1 << 5;
    pub const TRANSFER: u32 = 1 << 6;
    pub const DUP: u32 = 1 << 7;
    pub const REVOKE: u32 = 1 << 8;
    pub const MANAGE: u32 = 1 << 9;

    pub const fn all() -> Rights {
        Rights(0x3ff)
    }

    /// Whether all bits in `right` are present.
    pub const fn has_right(&self, right: u32) -> bool {
        self.0 & right == right
    }
}

/// A capability handle — index plus generation counter.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Cap {
    pub index: u32,
    pub generation: u32,
}

impl Cap {
    pub const fn null() -> Cap {
        Cap {
            index: 0,
            generation: 0,
        }
    }

    pub fn is_null(&self) -> bool {
        self.index == 0
    }
}

/// A capability table entry.
#[derive(Clone, Copy, Debug)]
struct CapEntry {
    /// Object type (opaque identifier).
    obj_type: u32,
    /// Object identifier.
    obj_id: u32,
    /// Rights bitmap.
    rights: u32,
    /// Generation counter for revocation.
    generation: u32,
    /// Whether this slot is allocated.
    allocated: bool,
}

/// Maximum capabilities per cell.
const MAX_CAPS: usize = 256;

/// A capability table — per cell.
pub struct CapTable {
    caps: [CapEntry; MAX_CAPS],
    count: usize,
}

/// A capability handle wrapper with type safety.
pub struct Capability {
    cap: Cap,
    rights: Rights,
}

impl Capability {
    pub fn null() -> Capability {
        Capability {
            cap: Cap::null(),
            rights: Rights(0),
        }
    }

    pub fn is_null(&self) -> bool {
        self.cap.is_null()
    }

    pub fn rights(&self) -> Rights {
        self.rights
    }

    pub fn has_right(&self, right: u32) -> bool {
        self.rights.0 & right == right
    }
}

/// Initialise an empty capability table.
pub fn init() -> CapTable {
    CapTable {
        caps: [CapEntry {
            obj_type: 0,
            obj_id: 0,
            rights: 0,
            generation: 0,
            allocated: false,
        }; MAX_CAPS],
        count: 0,
    }
}

impl CapTable {
    /// Allocate a new capability slot.
    fn alloc_slot(&mut self) -> Option<usize> {
        // Find a free slot (generation 0 means free).
        for i in 1..self.caps.len() {
            if !self.caps[i].allocated {
                self.caps[i].allocated = true;
                self.caps[i].generation += 1;
                self.count += 1;
                return Some(i);
            }
        }
        None
    }

    /// Allocate a new capability.
    pub fn alloc(&mut self, obj_type: u32, obj_id: u32, rights: Rights) -> Option<Capability> {
        let slot = self.alloc_slot()?;
        let gen = self.caps[slot].generation;
        self.caps[slot] = CapEntry {
            obj_type,
            obj_id,
            rights: rights.0,
            generation: gen,
            allocated: true,
        };
        Some(Capability {
            cap: Cap {
                index: slot as u32,
                generation: gen,
            },
            rights,
        })
    }

    /// Look up a capability by handle.
    pub fn lookup(&self, cap: &Cap) -> Option<Rights> {
        if cap.index == 0 || cap.index as usize >= self.caps.len() {
            return None;
        }
        let entry = self.caps[cap.index as usize];
        if !entry.allocated || entry.generation != cap.generation {
            return None; // Stale handle — revoked or never allocated.
        }
        Some(Rights(entry.rights))
    }

    /// The object type and identifier behind a live capability.
    pub fn object(&self, cap: &Cap) -> Option<(u32, u32)> {
        if cap.index == 0 || cap.index as usize >= self.caps.len() {
            return None;
        }
        let entry = self.caps[cap.index as usize];
        if !entry.allocated || entry.generation != cap.generation {
            return None;
        }
        Some((entry.obj_type, entry.obj_id))
    }

    /// Check that a live capability carries all requested rights.
    pub fn permits(&self, cap: &Cap, rights: Rights) -> bool {
        self.lookup(cap)
            .map(|actual| actual.0 & rights.0 == rights.0)
            .unwrap_or(false)
    }

    /// Revoke a capability — invalidate its generation.
    pub fn revoke(&mut self, cap: &Cap) -> bool {
        if cap.index == 0 || cap.index as usize >= self.caps.len() {
            return false;
        }
        let entry = &mut self.caps[cap.index as usize];
        if !entry.allocated || entry.generation != cap.generation {
            return false;
        }
        entry.allocated = false;
        entry.generation += 1;
        self.count -= 1;
        true
    }

    /// Derive a new capability with attenuated rights from an existing one.
    pub fn derive(&mut self, parent: &Cap, rights: Rights) -> Option<Capability> {
        let parent_rights = self.lookup(parent)?;
        if rights.0 & !parent_rights.0 != 0 {
            return None; // Derivation can only attenuate rights.
        }
        let entry = self.caps[parent.index as usize];
        self.alloc(entry.obj_type, entry.obj_id, rights)
    }

    /// Count of allocated capabilities.
    pub fn count(&self) -> usize {
        self.count
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn alloc_and_lookup() {
        let mut table = init();
        let cap = table
            .alloc(1, 42, Rights::all())
            .expect("alloc should succeed");
        assert!(!cap.is_null());
        let rights = table.lookup(&cap.cap).expect("lookup should succeed");
        assert!(rights.has_right(Rights::READ));
        assert!(rights.has_right(Rights::WRITE));
    }

    #[test]
    fn revoke_invalidates() {
        let mut table = init();
        let cap = table
            .alloc(1, 42, Rights::all())
            .expect("alloc should succeed");
        assert!(table.revoke(&cap.cap));
        assert!(table.lookup(&cap.cap).is_none());
        // Re-allocating should give a new generation.
        let cap2 = table
            .alloc(1, 43, Rights(Rights::READ))
            .expect("alloc should succeed");
        assert_eq!(cap.cap.index, cap2.cap.index);
        assert_ne!(cap.cap.generation, cap2.cap.generation);
        assert!(table.lookup(&cap.cap).is_none());
        assert!(table.lookup(&cap2.cap).is_some());
    }

    #[test]
    fn derive_preserves_object_and_checks_rights() {
        let mut table = init();
        let parent = table
            .alloc(7, 99, Rights(Rights::READ | Rights::WRITE))
            .expect("alloc");
        assert!(table.permits(&parent.cap, Rights(Rights::READ)));
        assert!(!table.permits(&parent.cap, Rights(Rights::DMA)));
        let child = table
            .derive(&parent.cap, Rights(Rights::READ))
            .expect("derive");
        assert_eq!(table.object(&child.cap), Some((7, 99)));
        assert!(table.permits(&child.cap, Rights(Rights::READ)));
        assert!(!table.permits(&child.cap, Rights(Rights::WRITE)));
    }

    #[test]
    fn derivation_attenuates() {
        let mut table = init();
        let parent = table
            .alloc(1, 42, Rights(Rights::READ | Rights::WRITE))
            .expect("alloc");
        let child = table
            .derive(&parent.cap, Rights(Rights::READ))
            .expect("derive should succeed");
        assert!(child.rights.has_right(Rights::READ));
        assert!(!child.rights.has_right(Rights::WRITE));
        // Cannot derive more rights than parent.
        let child2 = table.derive(&parent.cap, Rights(Rights::READ | Rights::DMA));
        assert!(child2.is_none());
    }
}
