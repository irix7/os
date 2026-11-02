// SPDX-License-Identifier: GPL-3.0-only
//! The ADB keyboard and its input queue.
//!
//! The transport is chosen once from the device tree and the keyboard is
//! polled: register 0 returns the last key transition as one or two bytes,
//! and each transition is decoded into a scancode, a direction and a US ASCII
//! character.  Decoded events go into a small fixed ring so the input
//! consumer added in #321 can drain them without owning the ADB transaction.
//!
//! A polled keyboard is not an interrupt source.  The wait below is used only
//! once, after the boot probes, to prove the path end to end; a monitor or
//! shell calls [`poll`] from its own loop and drains [`pop`].

use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use crate::adb::{self, Key, Modifiers, Timeout};
use crate::cuda::CudaAdb;
use crate::platform::AdbHost;
use crate::pmu::PmuAdb;
use crate::{print_dec, print_line, serial};

/// Transport identifiers stored in [`TRANSPORT`].
const NONE: u32 = 0;
const CUDA: u32 = 1;
const PMU: u32 = 2;

static TRANSPORT: AtomicU32 = AtomicU32::new(NONE);
static BASE: AtomicU32 = AtomicU32::new(0);
static ADDRESS: AtomicU32 = AtomicU32::new(0);
/// Set once the controller has been synchronised.
static STARTED: AtomicBool = AtomicBool::new(false);
/// Shift bit 0 and caps-lock bit 1, so the decoder state is not a static mut.
static MODIFIERS: AtomicU32 = AtomicU32::new(0);

const MODIFIER_SHIFT: u32 = 1;
const MODIFIER_CAPS: u32 = 1 << 1;

const QUEUE_CAPACITY: usize = 32;

/// A single-producer, single-consumer ring of decoded key events.
///
/// The producer is [`poll`] and the consumer is the input path; the indices
/// are atomic and the slots are only touched between the corresponding
/// release and acquire.  There is no lock because the kernel's input path is
/// polled on one CPU; the atomics make that contract explicit and leave a
/// later interrupt-fed producer with a defined memory order.
struct Queue {
    slots: UnsafeCell<[Key; QUEUE_CAPACITY]>,
    head: AtomicU32,
    tail: AtomicU32,
}

// SAFETY: `slots` is only read or written by a thread that owns the index it
// is accessing, and the atomic head/tail publish those writes.  The queue has
// one producer (`poll`) and one consumer (`pop`), as documented above.
unsafe impl Sync for Queue {}

impl Queue {
    fn push(&self, key: Key) -> bool {
        let tail = self.tail.load(Ordering::Relaxed);
        let head = self.head.load(Ordering::Acquire);
        if tail.wrapping_sub(head) >= QUEUE_CAPACITY as u32 {
            return false;
        }
        // SAFETY: the slot lies between head and the published tail, so no
        // other accessor can be using it.
        unsafe {
            (*self.slots.get())[tail as usize % QUEUE_CAPACITY] = key;
        }
        self.tail.store(tail.wrapping_add(1), Ordering::Release);
        true
    }

    fn pop(&self) -> Option<Key> {
        let head = self.head.load(Ordering::Relaxed);
        let tail = self.tail.load(Ordering::Acquire);
        if head == tail {
            return None;
        }
        // SAFETY: the slot was published by `push` before the tail increment.
        let key = unsafe { (*self.slots.get())[head as usize % QUEUE_CAPACITY] };
        self.head.store(head.wrapping_add(1), Ordering::Release);
        Some(key)
    }
}

static QUEUE: Queue = Queue {
    slots: UnsafeCell::new([Key::EMPTY; QUEUE_CAPACITY]),
    head: AtomicU32::new(0),
    tail: AtomicU32::new(0),
};

fn modifiers() -> Modifiers {
    let state = MODIFIERS.load(Ordering::Relaxed);
    Modifiers::from_bits(state & MODIFIER_SHIFT != 0, state & MODIFIER_CAPS != 0)
}

fn set_modifiers(state: Modifiers) {
    let mut bits = 0;
    if state.shift() {
        bits |= MODIFIER_SHIFT;
    }
    if state.caps() {
        bits |= MODIFIER_CAPS;
    }
    MODIFIERS.store(bits, Ordering::Relaxed);
}

/// The transport selected from the device tree, or `None`.
pub fn init(host: AdbHost) {
    match host {
        AdbHost::Cuda { base, keyboard } => {
            TRANSPORT.store(CUDA, Ordering::Relaxed);
            BASE.store(base, Ordering::Relaxed);
            ADDRESS.store(u32::from(keyboard), Ordering::Relaxed);
        }
        AdbHost::Pmu { base, keyboard } => {
            TRANSPORT.store(PMU, Ordering::Relaxed);
            BASE.store(base, Ordering::Relaxed);
            ADDRESS.store(u32::from(keyboard), Ordering::Relaxed);
        }
    }
}

/// Synchronise with the ADB controller.
///
/// This runs once, after the Mac I/O window is mapped.  A failed sync is
/// reported and leaves [`poll`] inert; the boot continues to its timeout
/// marker so a machine with no working ADB still reaches `ppc: halted`.
pub fn start() -> bool {
    let kind = TRANSPORT.load(Ordering::Relaxed);
    let base = BASE.load(Ordering::Relaxed);
    let address = ADDRESS.load(Ordering::Relaxed) as u8;
    serial::write(b"ppc: keyboard transport=");
    serial::write(match kind {
        CUDA => b"cuda",
        PMU => b"pmu",
        _ => b"none",
    });
    serial::write(b" address=");
    print_dec(u32::from(address));
    serial::write_byte(b'\n');

    let result = match kind {
        CUDA => CudaAdb::new(base).reset(),
        PMU => PmuAdb::new(base).reset(),
        _ => return false,
    };
    match result {
        Ok(()) => {
            STARTED.store(true, Ordering::Relaxed);
            print_line(b"ppc: keyboard reset=ok");
            true
        }
        Err(_) => {
            print_line(b"ppc: keyboard reset=failed");
            false
        }
    }
}

/// Run one ADB talk to the keyboard and queue every transition in the reply.
///
/// Returns true when at least one transition was queued; false for no key, a
/// transport failure, a full queue or an unstarted keyboard.
pub fn poll() -> bool {
    if !STARTED.load(Ordering::Relaxed) {
        return false;
    }
    let kind = TRANSPORT.load(Ordering::Relaxed);
    let base = BASE.load(Ordering::Relaxed);
    let address = ADDRESS.load(Ordering::Relaxed) as u8;
    let mut reply = [0u8; 4];
    let result = match kind {
        CUDA => CudaAdb::new(base).talk(address, adb::REGISTER_KEY, &mut reply),
        PMU => PmuAdb::new(base).talk(address, adb::REGISTER_KEY, &mut reply),
        _ => return false,
    };
    let length = match result {
        Ok(length) => length,
        Err(_) => return false,
    };
    if length == 0 {
        return false;
    }

    // A real CUDA and the PMU echo the command byte in front of the two data
    // bytes; QEMU's direct CUDA reply contains only the data.  Only a
    // three-byte reply can carry the echo, so a two-byte reply whose release
    // scancode happens to equal the command byte is not mistaken for one.
    let command = adb::talk(address, adb::REGISTER_KEY);
    let start = if length >= 3 && reply[0] == command {
        1
    } else {
        0
    };
    let mut state = modifiers();
    let mut queued = false;
    for byte in &reply[start..length] {
        if *byte == 0xff {
            continue;
        }
        let key = state.update(*byte);
        queued |= QUEUE.push(key);
    }
    set_modifiers(state);
    queued
}

/// Remove the oldest decoded key event.
pub fn pop() -> Option<Key> {
    QUEUE.pop()
}

/// Poll until a printable key-down arrives or `timeout_ms` expires.
///
/// Releases and modifier transitions are decoded and discarded while waiting
/// for the first printable press; the events after it stay in the queue.
pub fn wait_for_key(timeout_ms: u32) -> Option<Key> {
    if !STARTED.load(Ordering::Relaxed) {
        return None;
    }
    let timeout = Timeout::after_ms(timeout_ms);
    loop {
        poll();
        while let Some(key) = pop() {
            if key.down && key.ascii != 0 {
                return Some(key);
            }
        }
        if timeout.expired() {
            return None;
        }
        adb::spin_ms(5);
    }
}
