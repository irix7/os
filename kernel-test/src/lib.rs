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
    use m1_policy_bridge::{bridge_address, BRIDGE_SIZE};

    #[test]
    fn bridge_page_is_in_the_reserved_gap() {
        let start = bridge_address();
        let end = start + BRIDGE_SIZE;
        // The page sits in the unused gap above the boot stack top, not inside
        // the stack or the kernel image.
        assert!(
            start >= 0x883f_0000 && end <= 0x8840_0000,
            "bridge {start:#x}..{end:#x} is outside the reserved gap"
        );
    }
}
