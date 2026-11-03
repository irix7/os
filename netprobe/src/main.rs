// EXPERIMENT (spike): link the kernel's TLS, HTTP and range code into a
// ROM-shaped image to measure what the ROM network stage would cost.
#![no_std]
#![no_main]
#![feature(asm_experimental_arch)]

extern crate alloc;

mod http;
mod range;
mod tls;

/// Stand-ins for kernel services the TLS and HTTP code calls.  The probe links
/// the protocol code, not a working network; these return fixed values.
pub mod net {
    use alloc::boxed::Box;
    use smoltcp::iface::{Config, Interface, SocketSet};
    use smoltcp::phy::{Device, DeviceCapabilities, Medium, RxToken, TxToken};
    use smoltcp::time::Instant;
    use smoltcp::wire::{EthernetAddress, HardwareAddress};

    pub struct SeeqDevice;

    /// A phy that never delivers a frame: enough for the protocol code to run.
    pub struct Dev;
    pub struct Rx;
    pub struct Tx;
    impl RxToken for Rx {
        fn consume<R, F>(self, f: F) -> R
        where
            F: FnOnce(&[u8]) -> R,
        {
            f(&[])
        }
    }
    impl TxToken for Tx {
        fn consume<R, F: FnOnce(&mut [u8]) -> R>(self, len: usize, f: F) -> R {
            let mut buf = alloc::vec![0u8; len];
            f(&mut buf)
        }
    }
    impl Device for Dev {
        type RxToken<'a> = Rx where Self: 'a;
        type TxToken<'a> = Tx where Self: 'a;
        fn receive(&mut self, _t: Instant) -> Option<(Rx, Tx)> { None }
        fn transmit(&mut self, _t: Instant) -> Option<Tx> { Some(Tx) }
        fn capabilities(&self) -> DeviceCapabilities {
            let mut c = DeviceCapabilities::default();
            c.medium = Medium::Ethernet;
            c.max_transmission_unit = 1500;
            c
        }
    }

    pub fn clock_now() -> Instant { Instant::from_millis(0) }
    pub fn poll() {}

    pub fn with_stack<R>(f: impl FnOnce(&mut Interface, &mut SocketSet<'static>) -> R) -> Option<R> {
        let mut dev = Dev;
        let mut iface = Interface::new(Config::new(HardwareAddress::Ethernet(EthernetAddress([2, 0, 0, 0, 0, 1]))), &mut dev, Instant::from_millis(0));
        let mut sockets = SocketSet::new(alloc::vec::Vec::<smoltcp::iface::SocketStorage<'static>>::new().leak());
        Some(f(&mut iface, &mut sockets))
    }

    pub fn sockets_with<R>(f: impl FnOnce(&mut SocketSet<'static>) -> R) -> Option<R> {
        let mut sockets = SocketSet::new(alloc::vec::Vec::<smoltcp::iface::SocketStorage<'static>>::new().leak());
        Some(f(&mut sockets))
    }

    pub unsafe fn raw_stack() -> Option<(*mut Interface, *mut SocketSet<'static>, *mut SeeqDevice)> {
        let dev: &'static mut Dev = Box::leak(Box::new(Dev));
        let iface: &'static mut Interface = Box::leak(Box::new(Interface::new(Config::new(HardwareAddress::Ethernet(EthernetAddress([2, 0, 0, 0, 0, 1]))), dev, Instant::from_millis(0))));
        let sockets: &'static mut SocketSet<'static> = Box::leak(Box::new(SocketSet::new(alloc::vec::Vec::<smoltcp::iface::SocketStorage<'static>>::new().leak())));
        let seeq: &'static mut SeeqDevice = Box::leak(Box::new(SeeqDevice));
        Some((iface as *mut _, sockets as *mut _, seeq as *mut _))
    }

    pub unsafe fn poll_raw(_i: *mut Interface, _s: *mut SocketSet<'static>, _d: *mut SeeqDevice) {}
}

pub mod ntp {
    pub fn now_unix() -> Option<u64> { None }
}

pub mod traps {
    pub fn count_read() -> u32 { 0 }
}

pub fn println(_s: &str) {}
pub mod irix7_fmt {
    pub fn decimal(mut value: u64, put: &mut dyn FnMut(u8)) {
        let mut digits = [0u8; 20];
        let mut count = 0;
        loop {
            digits[count] = b'0' + (value % 10) as u8;
            count += 1;
            value /= 10;
            if value == 0 { break; }
        }
        while count > 0 { count -= 1; put(digits[count]); }
    }
}
pub fn print_str(_s: &str) {}
pub fn print_hex(_v: u64) {}
pub fn print_hex32(_v: u32) {}

#[global_allocator]
static HEAP: Bump = Bump;

struct Bump;

static mut ARENA: [u8; 1 << 20] = [0; 1 << 20];
static mut NEXT: usize = 0;

unsafe impl core::alloc::GlobalAlloc for Bump {
    unsafe fn alloc(&self, layout: core::alloc::Layout) -> *mut u8 {
        let align = layout.align().max(8);
        let start = (NEXT + align - 1) & !(align - 1);
        if start + layout.size() > (1 << 20) {
            return core::ptr::null_mut();
        }
        NEXT = start + layout.size();
        core::ptr::addr_of_mut!(ARENA).cast::<u8>().add(start)
    }
    unsafe fn dealloc(&self, _p: *mut u8, _l: core::alloc::Layout) {}
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    loop {}
}

#[no_mangle]
pub extern "C" fn _start() -> ! {
    // Keep the TLS entry points reachable so the linker keeps their code.
    tls::probe_link();
    loop {}
}
