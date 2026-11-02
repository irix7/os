#![no_std]

pub fn add(a: i32, b: i32) -> i32 {
    a + b
}

#[cfg(test)]
extern crate std;

#[cfg(not(test))]
#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    loop {}
}

#[cfg(test)]
mod tests {
    use loader_abi::{ARENA_BASE, ARENA_LEN, ARENA_REGION_SIZE};
    use m1_policy_bridge::{bridge_address, Bridge, BRIDGE_SIZE};
    use std::vec;

    /// The live policy values the kernel keeps independently of the module
    /// image. The bridge test models the kernel's adopted snapshot.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    struct LivePolicy {
        root: [u8; 32],
        actor: [u8; 16],
        home: [u8; 16],
        rights: u32,
    }

    const LIVE: LivePolicy = LivePolicy {
        root: [0x11; 32],
        actor: [0x22; 16],
        home: [0x33; 16],
        rights: 35,
    };

    #[test]
    fn bridge_page_is_outside_every_module_region() {
        let start = bridge_address();
        let end = start + BRIDGE_SIZE;
        assert!(
            end <= ARENA_BASE || start >= ARENA_BASE + ARENA_LEN,
            "bridge {start:#x}..{end:#x} overlaps the runtime arena"
        );
        // Slot A and slot B are the two boot slots. Slot B contains the arena
        // range, so checking the arena and the slot-A range covers both.
        assert!(
            end <= 0x8840_0000 || start >= 0x8848_0000 + 0x0018_0000,
            "bridge {start:#x}..{end:#x} overlaps a boot slot"
        );
        // The page must sit in the unused gap above the boot stack top, not
        // inside the stack or the kernel image.
        assert!(
            start >= 0x883f_0000 && end <= 0x8840_0000,
            "bridge {start:#x}..{end:#x} is outside the reserved gap"
        );
    }

    #[test]
    fn loading_every_arena_region_leaves_the_live_bridge_untouched() {
        // Model the low window from the stack top to the end of the module
        // area. Each arena region is filled with a module image while a live
        // bridge record sits in its reserved page.
        const MODEL_BASE: usize = 0x883f_0000;
        const MODEL_END: usize = ARENA_BASE + ARENA_LEN;
        let mut space = vec![0u8; MODEL_END - MODEL_BASE];
        let at = bridge_address() - MODEL_BASE;
        let marker = [0x5au8; BRIDGE_SIZE];
        space[at..at + marker.len()].copy_from_slice(&marker);

        for region in 0..(ARENA_LEN / ARENA_REGION_SIZE) {
            let start = ARENA_BASE + region * ARENA_REGION_SIZE - MODEL_BASE;
            for byte in &mut space[start..start + ARENA_REGION_SIZE] {
                *byte = 0xa5;
            }
            assert_eq!(
                &space[at..at + marker.len()],
                &marker[..],
                "arena region {region} overwrote the live bridge"
            );
        }
    }

    #[test]
    fn failed_slot_b_reload_keeps_the_previous_live_policy() {
        let mut live = LIVE;

        // module::clear(Slot::B) unpublishes the record before the fetch.
        let mut record = Bridge::published(0x1234);
        assert_eq!(record.published_context(), Some(0x1234));
        record.unpublish();
        // The fetch, digest or entry then fails, so nothing republishes and
        // the kernel never calls adopt.
        assert_eq!(record.published_context(), None);
        if record.published_context().is_some() {
            live = LivePolicy {
                root: [0x44; 32],
                actor: [0x55; 16],
                home: [0x33; 16],
                rights: 35,
            };
        }
        assert_eq!(live, LIVE);

        // A successful commit publishes a replacement; adoption moves the
        // live snapshot only after that publication is valid.
        let replacement = LivePolicy {
            root: [0x44; 32],
            actor: [0x55; 16],
            home: [0x33; 16],
            rights: 35,
        };
        let record = Bridge::published(0x5678);
        assert_eq!(record.published_context(), Some(0x5678));
        if record.published_context().is_some() {
            live = replacement;
        }
        assert_eq!(live, replacement);
    }
}
