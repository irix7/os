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
            if (services.tcp_listen)(0, PORT) == 0 {
                unsafe {
                    PHASE = WAIT_REQUEST;
                }
            }
        }
        WAIT_REQUEST => {
            let state = (services.tcp_state)(0);
            if state == 2 {
                let request = unsafe { &mut *core::ptr::addr_of_mut!(REQUEST) };
                let n = (services.tcp_recv)(0, request.as_mut_ptr(), request.len());
                if n > 0 {
                    send_response(services);
                    unsafe {
                        PHASE = WAIT_CLOSE;
                    }
                } else if n < 0 {
                    (services.tcp_abort)(0);
                    unsafe {
                        PHASE = ARM;
                    }
                }
            } else if state == 3 {
                (services.tcp_close)(0);
                unsafe {
                    PHASE = CLOSING;
                }
            } else if state == 0 {
                unsafe {
                    PHASE = ARM;
                }
            }
        }
        WAIT_CLOSE => match (services.tcp_state)(0) {
            3 => {
                (services.tcp_close)(0);
                unsafe {
                    PHASE = CLOSING;
                }
            }
            0 => unsafe {
                PHASE = ARM;
            },
            _ => {}
        },
        CLOSING => {
            if (services.tcp_state)(0) == 0 {
                (services.tcp_abort)(0);
                unsafe {
                    PHASE = ARM;
                }
            }
        }
        _ => unsafe {
            PHASE = ARM;
        },
    }
    0
}

fn send_response(services: &Services) {
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
        b"HTTP/1.0 200 OK\r\nContent-Type: text/plain\r\nConnection: close\r\nContent-Length: ",
    );
    send_all(services, &length[..len]);
    send_all(services, b"\r\n\r\n");
    send_all(services, body);
}

fn send_all(services: &Services, data: &[u8]) {
    let mut offset = 0;
    while offset < data.len() {
        let n = unsafe { (services.tcp_send)(0, data[offset..].as_ptr(), data.len() - offset) };
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
