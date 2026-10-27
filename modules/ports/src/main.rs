// SPDX-License-Identifier: GPL-3.0-only
//! Module TCP slot probe: open one socket per slot and echo through each.
//!
//! This is the transport prerequisite for the multi-node membership work in
//! #249.  It proves that the kernel can give one module several simultaneous
//! TCP sockets, not just the HTTP and mesh pair.  The module connects slot `n`
//! to the node's NAT gateway (`192.168.0.1`) at `ports.base + n`, sends
//! `p=n\n`, and requires the fixture to echo the same line.
//!
//! Connections are staggered and the echo is driven one slot at a time on
//! purpose: all sockets are live together, but the emulated Seeq receive ring
//! (eight descriptors) is not asked to absorb a twelve-frame burst of host
//! ACKs and echoes.  The transport claim is "eight concurrent sockets", not
//! "eight simultaneous bursts".
//!
//! The probe is deliberately small and lab-only: the gateway address and the
//! line protocol are fixed, the slot count comes from `tcp_slots()`, and the
//! only configurable values are `ports.base` (default 18300) and
//! `ports.count` (default: every slot).

#![no_std]
#![no_main]

use module_abi::{ModuleVtable, Services};

/// The largest probe table; `tcp_slots()` may report fewer.
const MAX_SLOTS: usize = 8;
/// Node 0's NAT gateway, the host side of the rig's fixture.
const GATEWAY: u32 = 0xC0A8_0001;
/// Default first fixture port.
const DEFAULT_BASE: u16 = 18_300;
/// How long the probe waits for all connections and echoes.
const TIMEOUT_MS: u64 = 30_000;
/// Gap between connection attempts, so the handshake burst stays small.
const CONNECT_GAP_MS: u64 = 60;
/// Gap between echo attempts.
const ECHO_GAP_MS: u64 = 40;

struct Probe {
    slots: u32,
    base: u16,
    connected: bool,
    sent: [bool; MAX_SLOTS],
    done: [bool; MAX_SLOTS],
    next_connect: u32,
    active: u32,
    connect_at: u64,
    echo_at: u64,
    deadline: u64,
}

impl Probe {
    const fn new() -> Self {
        Self {
            slots: 0,
            base: DEFAULT_BASE,
            connected: false,
            sent: [false; MAX_SLOTS],
            done: [false; MAX_SLOTS],
            next_connect: 0,
            active: 0,
            connect_at: 0,
            echo_at: 0,
            deadline: 0,
        }
    }
}

static mut PROBE: Probe = Probe::new();

static OK: ModuleVtable = ModuleVtable {
    status: 0,
    tick: Some(tick),
};

static DONE: ModuleVtable = ModuleVtable {
    status: 0,
    tick: None,
};

/// Write a byte string to the serial console.
fn log(services: &Services, text: &[u8]) {
    (services.write)(text.as_ptr(), text.len());
}

/// Write an unsigned decimal value.
fn log_u64(services: &Services, mut value: u64) {
    let mut buffer = [0u8; 20];
    let mut at = buffer.len();
    loop {
        at -= 1;
        buffer[at] = b'0' + (value % 10) as u8;
        value /= 10;
        if value == 0 {
            break;
        }
    }
    log(services, &buffer[at..]);
}

/// Read a decimal `config_get` value.
fn config_u16(services: &Services, key: &[u8]) -> Option<u16> {
    let mut buffer = [0u8; 16];
    let read = unsafe {
        (services.config_get)(key.as_ptr(), key.len(), buffer.as_mut_ptr(), buffer.len())
    };
    if read <= 0 {
        return None;
    }
    let mut value = 0u16;
    for byte in &buffer[..read as usize] {
        if !byte.is_ascii_digit() {
            return None;
        }
        value = value.checked_mul(10)?.checked_add((byte - b'0') as u16)?;
    }
    Some(value)
}

/// Format `p=<slot>\n` without touching the allocator.
fn probe_line(slot: u32) -> ([u8; 8], usize) {
    let mut line = [0u8; 8];
    line[0] = b'p';
    line[1] = b'=';
    let mut at = 2;
    let mut value = slot;
    let mut digits = [0u8; 3];
    let mut count = 0;
    loop {
        digits[count] = b'0' + (value % 10) as u8;
        count += 1;
        value /= 10;
        if value == 0 {
            break;
        }
    }
    while count > 0 {
        count -= 1;
        line[at] = digits[count];
        at += 1;
    }
    line[at] = b'\n';
    (line, at + 1)
}

/// Report one failure once and stop.
fn fail(services: &Services, reason: &[u8], slot: u32) -> i32 {
    log(services, b"ports: fail reason=");
    log(services, reason);
    log(services, b" slot=");
    log_u64(services, slot as u64);
    log(services, b"\n");
    1
}

/// Module entry.
///
/// # Safety
/// `services` must point to a valid service table supplied by the kernel.
#[no_mangle]
pub unsafe extern "C" fn _start(services: *const Services) -> *const ModuleVtable {
    let Some(services) = (unsafe { services.as_ref() }) else {
        return core::ptr::null();
    };
    if services.version < 15 {
        log(services, b"ports: fail reason=abi version=");
        log_u64(services, services.version as u64);
        log(services, b"\n");
        return &DONE;
    }
    let slots = (services.tcp_slots)().min(MAX_SLOTS as u32);
    if slots < 2 {
        log(services, b"ports: fail reason=slots count=");
        log_u64(services, slots as u64);
        log(services, b"\n");
        return &DONE;
    }
    let base = config_u16(services, b"ports.base").unwrap_or(DEFAULT_BASE);
    let count = config_u16(services, b"ports.count")
        .unwrap_or(slots as u16)
        .clamp(1, slots as u16) as u32;
    let now = (services.clock_ms)();
    let probe = unsafe { &mut *core::ptr::addr_of_mut!(PROBE) };
    probe.slots = count;
    probe.base = base;
    probe.connect_at = now;
    probe.deadline = now.saturating_add(TIMEOUT_MS);
    log(services, b"ports: start slots=");
    log_u64(services, count as u64);
    log(services, b" base=");
    log_u64(services, base as u64);
    log(services, b"\n");
    &OK
}

/// Drive the staggered connects and the one-at-a-time echo.
unsafe extern "C" fn tick(services: *const Services) -> i32 {
    let Some(services) = (unsafe { services.as_ref() }) else {
        return 1;
    };
    let probe = unsafe { &mut *core::ptr::addr_of_mut!(PROBE) };
    if probe.slots == 0 {
        return 1;
    }
    let now = (services.clock_ms)();
    if now >= probe.deadline {
        return fail(services, b"timeout", probe.active);
    }

    if !probe.connected {
        // Open one slot per gap.  The sockets stay live together; only the
        // handshake frames are paced.
        if probe.next_connect < probe.slots && now >= probe.connect_at {
            let slot = probe.next_connect;
            let port = probe.base.wrapping_add(slot as u16);
            if (services.tcp_connect)(slot, GATEWAY, port) != 0 {
                return fail(services, b"connect", slot);
            }
            probe.next_connect += 1;
            probe.connect_at = now.saturating_add(CONNECT_GAP_MS);
            return 0;
        }
        if probe.next_connect < probe.slots {
            return 0;
        }
        // Every connect has been issued; wait for all sockets to establish.
        for slot in 0..probe.slots {
            match (services.tcp_state)(slot) {
                2 => {}
                0 | 3 => return fail(services, b"closed", slot),
                _ => return 0,
            }
        }
        probe.connected = true;
        log(services, b"ports: connected sockets=");
        log_u64(services, probe.slots as u64);
        log(services, b"\n");
        probe.echo_at = now;
        return 0;
    }

    if probe.active >= probe.slots {
        log(services, b"ports: pass sockets=");
        log_u64(services, probe.slots as u64);
        log(services, b"\n");
        return 1;
    }
    if now < probe.echo_at {
        return 0;
    }
    let slot = probe.active;
    let index = slot as usize;
    match (services.tcp_state)(slot) {
        2 => {}
        0 | 3 => return fail(services, b"closed", slot),
        _ => return 0,
    }
    if !probe.sent[index] {
        let (line, len) = probe_line(slot);
        let sent = unsafe { (services.tcp_send)(slot, line.as_ptr(), len) };
        if sent != len as isize {
            return fail(services, b"short-send", slot);
        }
        probe.sent[index] = true;
        return 0;
    }
    let mut buffer = [0u8; 16];
    let read = unsafe { (services.tcp_recv)(slot, buffer.as_mut_ptr(), buffer.len()) };
    if read > 0 {
        let (line, len) = probe_line(slot);
        let got = &buffer[..read as usize];
        if got != &line[..len] {
            return fail(services, b"bad-echo", slot);
        }
        probe.done[index] = true;
        log(services, b"ports: echo slot=");
        log_u64(services, slot as u64);
        log(services, b"\n");
        probe.active += 1;
        probe.echo_at = now.saturating_add(ECHO_GAP_MS);
    }
    0
}

#[cfg(not(test))]
#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    loop {
        core::hint::spin_loop();
    }
}
