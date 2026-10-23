// SPDX-License-Identifier: GPL-3.0-only
//! HTTP server module.
//!
//! Listens on guest port 80 through the version 3 TCP services, answers one
//! request per connection with a small status page, and re-arms after the
//! client closes.  This is the first long-running module and the prototype for
//! the MCP control plane: the compressed PROM image does not grow.

#![no_std]
#![no_main]

/// Must match `kernel::module::Services` (version 3).
#[repr(C)]
pub struct Services {
    pub version: u32,
    pub write: extern "C" fn(*const u8, usize),
    pub clock_ms: extern "C" fn() -> u64,
    pub alloc: unsafe extern "C" fn(usize, usize) -> *mut u8,
    pub poll: extern "C" fn(),
    pub tcp_listen: unsafe extern "C" fn(u32, u16) -> i32,
    pub tcp_state: extern "C" fn(u32) -> u32,
    pub tcp_recv: unsafe extern "C" fn(u32, *mut u8, usize) -> isize,
    pub tcp_send: unsafe extern "C" fn(u32, *const u8, usize) -> isize,
    pub tcp_close: extern "C" fn(u32),
    pub tcp_abort: extern "C" fn(u32),
}

#[repr(C)]
pub struct ModuleVtable {
    pub status: i32,
    pub tick: Option<unsafe extern "C" fn(*const Services) -> i32>,
}

/// Listening phase.
const ARM: u32 = 0;
const WAIT_REQUEST: u32 = 1;
const WAIT_CLOSE: u32 = 2;
const CLOSING: u32 = 3;

const PORT: u16 = 80;

static VTABLE: ModuleVtable = ModuleVtable {
    status: 0,
    tick: Some(tick),
};
static ERROR_VTABLE: ModuleVtable = ModuleVtable {
    status: 1,
    tick: None,
};

static mut PHASE: u32 = ARM;
static mut REQUEST: [u8; 1024] = [0; 1024];

/// The slot serving the current connection, and the slot armed for the next
/// one.  Alternating the two hides the close handshake from a back-to-back
/// client.
static mut CURRENT: u32 = 0;
static mut NEXT: u32 = 1;
static mut NEXT_ARMED: bool = false;

/// Module entry.
///
/// # Safety
/// `services` must point to a valid service table supplied by the kernel.
#[no_mangle]
pub unsafe extern "C" fn _start(services: *const Services) -> *const ModuleVtable {
    let Some(services) = (unsafe { services.as_ref() }) else {
        return &ERROR_VTABLE;
    };
    if services.version < 3 {
        return &ERROR_VTABLE;
    }
    let message = b"httpd: listening on port 80\n";
    (services.write)(message.as_ptr(), message.len());
    &VTABLE
}

unsafe extern "C" fn tick(services: *const Services) -> i32 {
    let Some(services) = (unsafe { services.as_ref() }) else {
        return 1;
    };

    match unsafe { PHASE } {
        ARM => {
            let slot = unsafe { CURRENT };
            if (services.tcp_listen)(slot, PORT) == 0 {
                unsafe {
                    PHASE = WAIT_REQUEST;
                }
            }
        }
        WAIT_REQUEST => {
            let slot = unsafe { CURRENT };
            let state = (services.tcp_state)(slot);
            if state == 2 {
                let request = unsafe { &mut *core::ptr::addr_of_mut!(REQUEST) };
                let n = (services.tcp_recv)(slot, request.as_mut_ptr(), request.len());
                if n > 0 {
                    send_response(services, slot);
                    // Arm the other slot before the client closes, so the
                    // next connection lands on a listening socket.
                    let next = unsafe { NEXT };
                    if (services.tcp_listen)(next, PORT) == 0 {
                        unsafe {
                            NEXT_ARMED = true;
                        }
                    }
                    unsafe {
                        PHASE = WAIT_CLOSE;
                    }
                } else if n < 0 {
                    (services.tcp_abort)(slot);
                    unsafe {
                        PHASE = ARM;
                    }
                }
            } else if state == 3 {
                (services.tcp_close)(slot);
                unsafe {
                    PHASE = CLOSING;
                }
            } else if state == 0 {
                unsafe {
                    PHASE = ARM;
                }
            }
        }
        WAIT_CLOSE => {
            let slot = unsafe { CURRENT };
            match (services.tcp_state)(slot) {
                3 => {
                    (services.tcp_close)(slot);
                    unsafe {
                        PHASE = CLOSING;
                    }
                }
                0 => unsafe {
                    finish_close();
                },
                _ => {}
            }
        }
        CLOSING => {
            let slot = unsafe { CURRENT };
            if (services.tcp_state)(slot) == 0 {
                (services.tcp_abort)(slot);
                unsafe {
                    finish_close();
                }
            }
        }
        _ => unsafe {
            PHASE = ARM;
        },
    }
    0
}

/// Move to the pre-armed listener, or re-arm the current slot if the second
/// listener could not be created.
unsafe fn finish_close() {
    if unsafe { NEXT_ARMED } {
        let old = unsafe { CURRENT };
        unsafe {
            CURRENT = NEXT;
            NEXT = old;
            NEXT_ARMED = false;
            PHASE = WAIT_REQUEST;
        }
    } else {
        unsafe {
            PHASE = ARM;
        }
    }
}

fn send_response(services: &Services, slot: u32) {
    let mut body = [0u8; 96];
    let mut n = 0;
    n += copy(&mut body[n..], b"IRIX 7 module HTTP server\nuptime: ");
    n += write_u64(&mut body[n..], (services.clock_ms)() / 1000);
    n += copy(&mut body[n..], b" s\n");
    let body = &body[..n];

    let mut length = [0u8; 20];
    let len = write_u64(&mut length, body.len() as u64);

    send_all(
        services,
        slot,
        b"HTTP/1.0 200 OK\r\nContent-Type: text/plain\r\nConnection: close\r\nContent-Length: ",
    );
    send_all(services, slot, &length[..len]);
    send_all(services, slot, b"\r\n\r\n");
    send_all(services, slot, body);
}

fn send_all(services: &Services, slot: u32, data: &[u8]) {
    let mut offset = 0;
    while offset < data.len() {
        let n = unsafe { (services.tcp_send)(slot, data[offset..].as_ptr(), data.len() - offset) };
        if n <= 0 {
            return;
        }
        offset += n as usize;
    }
}

fn copy(dst: &mut [u8], src: &[u8]) -> usize {
    let n = dst.len().min(src.len());
    dst[..n].copy_from_slice(&src[..n]);
    n
}

fn write_u64(dst: &mut [u8], mut value: u64) -> usize {
    let mut digits = [0u8; 20];
    let mut n = 0;
    loop {
        digits[n] = b'0' + (value % 10) as u8;
        value /= 10;
        n += 1;
        if value == 0 {
            break;
        }
    }
    let count = n.min(dst.len());
    for i in 0..count {
        dst[i] = digits[n - 1 - i];
    }
    count
}

#[cfg(not(test))]
#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    loop {
        core::hint::spin_loop();
    }
}

// The module links only core and compiler_builtins; provide the byte
// primitives the compiler may emit for array initialisation and copies.
#[no_mangle]
pub unsafe extern "C" fn memset(dst: *mut u8, c: i32, n: usize) -> *mut u8 {
    let mut i = 0;
    while i < n {
        unsafe {
            *dst.add(i) = c as u8;
        }
        i += 1;
    }
    dst
}

#[no_mangle]
pub unsafe extern "C" fn memcpy(dst: *mut u8, src: *const u8, n: usize) -> *mut u8 {
    let mut i = 0;
    while i < n {
        unsafe {
            *dst.add(i) = *src.add(i);
        }
        i += 1;
    }
    dst
}

#[no_mangle]
pub unsafe extern "C" fn memmove(dst: *mut u8, src: *const u8, n: usize) -> *mut u8 {
    if (dst as usize) < (src as usize) {
        return unsafe { memcpy(dst, src, n) };
    }
    let mut i = n;
    while i > 0 {
        i -= 1;
        unsafe {
            *dst.add(i) = *src.add(i);
        }
    }
    dst
}
