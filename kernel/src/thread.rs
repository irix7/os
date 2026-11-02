// SPDX-License-Identifier: GPL-3.0-only
//! Cooperative user threads inside one address space.
//!
//! The v0 thread gate runs until a thread traps with `SYS_YIELD`,
//! `SYS_THREAD_EXIT`, `SYS_FUTEX_WAIT` or a process exit.  There is no timer
//! preemption and no per-thread kernel stack: a switch happens only at a
//! syscall boundary.  Each thread owns a saved [`TrapFrame`]; a switch saves
//! the outgoing frame in its slot, copies the incoming slot into the trap
//! frame, and lets the exception return path enter the new thread exactly as
//! if it had just returned from its own syscall.
//!
//! A futex wait parks the thread on a 32-bit user word and switches away.  The
//! parked wait is bounded by the same 100 ms poll limit the descriptor waits
//! use, so a program with no waker cannot wedge the machine; a wake is the
//! normal path and returns the waiter to user mode with `CELL_OK`.  When the
//! deadline passes the word is compared again: a changed word resumes the
//! waiter with `CELL_E_AGAIN`, an unchanged one with `CELL_E_TIMEDOUT`.
//!
//! The thread pointer (`$28`) is caller-owned.  The libc allocates the 16-byte
//! TCB and passes it to `SYS_THREAD_CREATE`; the kernel only checks that the
//! TCB, entry and stack top are inside mapped user ranges, builds the first
//! frame and reloads `$28` on every entry.
//!
//! Thread ids are the table slot plus one: the main thread is tid 1 and the
//! first worker is tid 2.  A dead slot is reusable, so a tid is only unique
//! while its thread is alive.  There is no join yet; the exit status is
//! recorded in the slot.

use crate::syserr::{SysResult, CELL_E_AGAIN, CELL_E_FAULT, CELL_E_INVAL, CELL_E_TIMEDOUT};
use crate::traps::TrapFrame;

/// Fixed table size.  The v0 gate is a bring-up surface, not a process model;
/// eight threads cover the probe and leave room for a second worker.
pub const MAX_THREADS: usize = 8;

#[derive(Clone, Copy, PartialEq, Eq)]
enum State {
    /// Never used, or cleared by [`reset`].
    Unused,
    /// Ready to run and holding a saved frame.
    Runnable,
    /// The thread that is running right now.
    Running,
    /// Parked on a futex word; the saved frame resumes when the wait ends.
    Blocked,
    /// Exited; the slot holds the exit status until it is reused.
    Dead,
}

#[derive(Clone, Copy)]
struct Thread {
    state: State,
    tid: u32,
    exit_status: u32,
    /// Futex word this thread is parked on while `state` is `Blocked`.
    wait_addr: u32,
    /// The value the waiter compared, re-checked when the deadline passes.
    wait_expected: u32,
    /// CP0 Count value sampled when the wait began.
    wait_start: u32,
    /// Bounded wait in CP0 Count ticks.
    wait_limit: u32,
    frame: TrapFrame,
}

const EMPTY: Thread = Thread {
    state: State::Unused,
    tid: 0,
    exit_status: 0,
    wait_addr: 0,
    wait_expected: 0,
    wait_start: 0,
    wait_limit: 0,
    frame: TrapFrame::ZERO,
};

static mut THREADS: [Thread; MAX_THREADS] = [EMPTY; MAX_THREADS];
static mut CURRENT: usize = 0;

/// The result of retiring the running thread.
pub enum Exit {
    /// The trap frame now belongs to another thread.
    Switched,
    /// No thread is left; the caller must return to the kernel console.
    Last,
}

/// The result of a futex wait.
pub enum Wait {
    /// The caller keeps running; the frame already holds the status.
    Continue,
    /// The frame now belongs to another thread.
    Switched,
}

/// Clear the table and make tid 1 the running thread.
///
/// A user image starts here; [`crate::user::cleanup`] also calls it so an exit
/// cannot leave a stale slot behind.
pub fn reset() {
    // SAFETY: single-threaded kernel; the table is only touched from this
    // module and only while no other thread can run.
    unsafe {
        let threads = &mut *core::ptr::addr_of_mut!(THREADS);
        for (index, slot) in threads.iter_mut().enumerate() {
            *slot = EMPTY;
            slot.tid = index as u32 + 1;
        }
        threads[0].state = State::Running;
        CURRENT = 0;
    }
}

/// Resume waiters whose bounded deadline has passed.
///
/// The cooperative kernel has no timer interrupt, so deadlines are applied at
/// scheduling points: the next yield, block or exit.  A wait ended here still
/// follows the futex contract, so the word is compared again.
fn expire_waiters() {
    let now = crate::traps::count_read();
    // SAFETY: single-threaded kernel; no thread runs while this scan is in
    // progress.
    unsafe {
        let threads = &mut *core::ptr::addr_of_mut!(THREADS);
        for slot in threads.iter_mut() {
            if slot.state != State::Blocked || now.wrapping_sub(slot.wait_start) < slot.wait_limit {
                continue;
            }
            let changed = !crate::user::range_ok(slot.wait_addr, 4)
                || read_word(slot.wait_addr) != slot.wait_expected;
            slot.frame.regs[2] = if changed {
                CELL_E_AGAIN as u64
            } else {
                CELL_E_TIMEDOUT as u64
            };
            slot.frame.regs[3] = 0;
            slot.state = State::Runnable;

            crate::print_str("futex: expiry tid=");
            print_dec(slot.tid);
            crate::print_str(" addr=0x");
            crate::print_hex32(slot.wait_addr);
            crate::print_str(if changed {
                " result=again"
            } else {
                " result=timedout"
            });
            crate::println("");
        }
    }
}

fn next_runnable(from: usize) -> Option<usize> {
    expire_waiters();
    // SAFETY: single-threaded kernel; a plain read of the table.
    unsafe {
        let threads = &*core::ptr::addr_of!(THREADS);
        for step in 1..=MAX_THREADS {
            let index = (from + step) % MAX_THREADS;
            if threads[index].state == State::Runnable {
                return Some(index);
            }
        }
    }
    None
}

fn has_blocked() -> bool {
    // SAFETY: single-threaded kernel; a plain read of the table.
    unsafe {
        (*core::ptr::addr_of!(THREADS))
            .iter()
            .any(|slot| slot.state == State::Blocked)
    }
}

fn switch_to(tf: &mut TrapFrame, from: usize, to: usize) {
    // SAFETY: `from` and `to` are table indices; the kernel is single-threaded
    // between the save and the return to user mode.
    unsafe {
        let threads = &mut *core::ptr::addr_of_mut!(THREADS);
        if threads[from].state == State::Running {
            threads[from].state = State::Runnable;
        }
        threads[to].state = State::Running;
        CURRENT = to;
        *tf = threads[to].frame;
    }
}

/// Build a frame for a new thread and mark it runnable.
///
/// `status` is the creator's saved `Status` word; the new frame keeps its
/// coprocessor and interrupt-mask bits and starts in user mode with interrupts
/// enabled, exactly as the creator's next `eret` would.
pub fn create(status: u64, entry: u32, arg: u32, stack_top: u32, tcb: u32) -> SysResult<u32> {
    if entry & 3 != 0 || stack_top & 15 != 0 || tcb & 3 != 0 {
        return Err(CELL_E_INVAL);
    }
    let Some(below_stack_top) = stack_top.checked_sub(1) else {
        return Err(CELL_E_FAULT);
    };
    if !crate::user::range_ok(entry, 4)
        || !crate::user::range_ok(below_stack_top, 1)
        || !crate::user::range_ok(tcb, core::mem::size_of::<native_abi::tcb>() as u32)
    {
        return Err(CELL_E_FAULT);
    }

    // SAFETY: single-threaded kernel; slot selection and the write below do
    // not race with a running thread.
    unsafe {
        let threads = &mut *core::ptr::addr_of_mut!(THREADS);
        let Some(index) = threads
            .iter()
            .position(|slot| matches!(slot.state, State::Unused | State::Dead))
        else {
            return Err(CELL_E_AGAIN);
        };

        let mut frame = TrapFrame::ZERO;
        frame.epc = entry as u64;
        frame.regs[4] = arg as u64;
        frame.regs[28] = tcb as u64;
        frame.regs[29] = stack_top as u64;
        frame.status = crate::traps::thread_entry_status(status);

        threads[index].frame = frame;
        threads[index].exit_status = 0;
        threads[index].wait_addr = 0;
        threads[index].wait_expected = 0;
        threads[index].wait_start = 0;
        threads[index].wait_limit = 0;
        threads[index].state = State::Runnable;
        let tid = threads[index].tid;

        crate::print_str("thread: create by=");
        print_dec(threads[CURRENT].tid);
        crate::print_str(" tid=");
        print_dec(tid);
        crate::print_str(" epc=0x");
        crate::print_hex32(entry);
        crate::print_str(" sp=0x");
        crate::print_hex32(stack_top);
        crate::print_str(" tcb=0x");
        crate::print_hex32(tcb);
        crate::println("");
        Ok(tid)
    }
}

/// Save the running thread and resume the next runnable one.
///
/// Returns `true` when it switched.  A switch advances the outgoing `EPC` past
/// the gate before saving it, because the trap epilogue returns through the
/// incoming frame and the caller must not advance it again.
pub fn yield_current(tf: &mut TrapFrame) -> bool {
    let from = unsafe { CURRENT };
    let Some(to) = next_runnable(from) else {
        return false;
    };
    tf.advance_epc();
    // SAFETY: the running thread's frame is saved before the switch and no
    // other thread can observe the table in between.
    unsafe {
        (*core::ptr::addr_of_mut!(THREADS))[from].frame = *tf;
    }
    switch_to(tf, from, to);
    true
}

/// Park the running thread on a futex word until a wake or the deadline.
///
/// The word is compared before the thread parks, so a mismatch is
/// `CELL_E_AGAIN` without a switch.  A zero timeout probes the word and
/// reports `CELL_E_TIMEDOUT` when it matches.  With no other runnable thread
/// no wake can arrive, so the wait is bounded by the poll rule and returns
/// from the kernel instead of parking.
pub fn futex_wait(tf: &mut TrapFrame, address: u32, expected: u32, timeout_ms: i32) -> Wait {
    if address & 3 != 0 {
        set_result(tf, CELL_E_INVAL);
        return Wait::Continue;
    }
    if !crate::user::range_ok(address, 4) {
        set_result(tf, CELL_E_FAULT);
        return Wait::Continue;
    }
    if read_word(address) != expected {
        set_result(tf, CELL_E_AGAIN);
        return Wait::Continue;
    }

    let limit = timeout_ticks(timeout_ms);
    if limit == 0 {
        set_result(tf, CELL_E_TIMEDOUT);
        return Wait::Continue;
    }

    let from = unsafe { CURRENT };
    let Some(to) = next_runnable(from) else {
        return bounded_wait(tf, address, expected, limit);
    };

    crate::print_str("futex: wait tid=");
    print_dec(unsafe { (*core::ptr::addr_of!(THREADS))[from].tid });
    crate::print_str(" addr=0x");
    crate::print_hex32(address);
    crate::print_str(" value=0x");
    crate::print_hex32(expected);
    crate::print_str(" limit=");
    print_dec(limit / crate::fd::TICKS_PER_MS);
    crate::print_str("ms");
    crate::println("");

    // SAFETY: the running thread's frame is saved before the switch; the
    // waker writes the result into the saved frame while this thread cannot
    // run.
    unsafe {
        let threads = &mut *core::ptr::addr_of_mut!(THREADS);
        threads[from].wait_addr = address;
        threads[from].wait_expected = expected;
        threads[from].wait_start = crate::traps::count_read();
        threads[from].wait_limit = limit;
        threads[from].state = State::Blocked;

        tf.regs[2] = native_abi::status::CELL_OK as u64;
        tf.regs[3] = 0;
        tf.advance_epc();
        threads[from].frame = *tf;
    }
    switch_to(tf, from, to);
    Wait::Switched
}

/// Wait alone, with no thread that could wake this one.
///
/// The poll rule applies: the wait is bounded, the word is re-read while it
/// lasts, and a changed word is `CELL_E_AGAIN`.  No thread can run during the
/// loop, so in practice only the clock advances.
fn bounded_wait(tf: &mut TrapFrame, address: u32, expected: u32, limit: u32) -> Wait {
    let tid = unsafe { (*core::ptr::addr_of!(THREADS))[CURRENT].tid };
    crate::print_str("futex: no-peer tid=");
    print_dec(tid);
    crate::print_str(" addr=0x");
    crate::print_hex32(address);
    crate::print_str(" limit=");
    print_dec(limit / crate::fd::TICKS_PER_MS);
    crate::print_str("ms");
    crate::println("");

    let start = crate::traps::count_read();
    loop {
        if read_word(address) != expected {
            set_result(tf, CELL_E_AGAIN);
            return Wait::Continue;
        }
        if crate::traps::count_read().wrapping_sub(start) >= limit {
            set_result(tf, CELL_E_TIMEDOUT);
            return Wait::Continue;
        }
        core::hint::spin_loop();
    }
}

/// Wake up to `count` threads parked on `address`.
///
/// The woken thread resumes with `CELL_OK` in `$v0`.  The waker owns the
/// protocol, so the word is not re-compared here: a waiter woken after the
/// word changed still returns success, exactly as a futex wake does.
pub fn futex_wake(address: u32, count: u32) -> SysResult<u32> {
    if address & 3 != 0 {
        return Err(CELL_E_INVAL);
    }
    if !crate::user::range_ok(address, 4) {
        return Err(CELL_E_FAULT);
    }

    let mut woken = 0u32;
    // SAFETY: single-threaded kernel; a woken thread cannot run until the
    // current syscall returns to user mode.
    unsafe {
        let threads = &mut *core::ptr::addr_of_mut!(THREADS);
        for slot in threads.iter_mut() {
            if woken == count {
                break;
            }
            if slot.state != State::Blocked || slot.wait_addr != address {
                continue;
            }
            slot.state = State::Runnable;
            slot.frame.regs[2] = native_abi::status::CELL_OK as u64;
            slot.frame.regs[3] = 0;
            woken += 1;
        }
    }

    crate::print_str("futex: wake addr=0x");
    crate::print_hex32(address);
    crate::print_str(" count=");
    print_dec(count);
    crate::print_str(" woke=");
    print_dec(woken);
    crate::println("");
    Ok(woken)
}

/// Retire the running thread with `status` and resume the next runnable one.
///
/// With every remaining thread parked on a bounded futex wait, the kernel
/// keeps the image alive until the nearest deadline instead of exiting under
/// a waiter that can still be resumed by the clock.
pub fn exit_current(tf: &mut TrapFrame, status: u32) -> Exit {
    let from = unsafe { CURRENT };
    let tid = unsafe {
        let threads = &mut *core::ptr::addr_of_mut!(THREADS);
        threads[from].state = State::Dead;
        threads[from].exit_status = status;
        threads[from].tid
    };
    crate::print_str("thread: exit tid=");
    print_dec(tid);
    crate::print_str(" status=0x");
    crate::print_hex32(status);
    crate::println("");

    loop {
        if let Some(to) = next_runnable(from) {
            switch_to(tf, from, to);
            return Exit::Switched;
        }
        if !has_blocked() {
            return Exit::Last;
        }
        core::hint::spin_loop();
    }
}

/// Stop every thread ahead of a process exit.
///
/// A `SYS_EXIT` from any thread ends the image, matching `exit_group` rather
/// than `pthread_exit`.  The caller then returns to the console and
/// [`crate::user::cleanup`] resets the table.
pub fn exit_all() {
    // SAFETY: single-threaded kernel; no thread runs between here and the
    // return to the console.
    unsafe {
        let threads = &mut *core::ptr::addr_of_mut!(THREADS);
        for slot in threads.iter_mut() {
            if slot.state != State::Unused {
                slot.state = State::Dead;
            }
        }
    }
}

/// Bound a futex timeout the way [`crate::fd::WAIT_LIMIT_MS`] bounds `poll`.
///
/// A negative timeout means "no deadline" to POSIX, but the cooperative kernel
/// has no timer wakeup, so it is bounded rather than infinite.  A positive
/// timeout may shorten the bound and zero probes the word without parking.
fn timeout_ticks(timeout_ms: i32) -> u32 {
    let millis = match timeout_ms {
        0 => return 0,
        value if value < 0 => crate::fd::WAIT_LIMIT_MS,
        value => (value as u32).min(crate::fd::WAIT_LIMIT_MS),
    };
    millis * crate::fd::TICKS_PER_MS
}

/// Read the 32-bit futex word.  The caller has checked alignment and range.
fn read_word(address: u32) -> u32 {
    // SAFETY: the caller validated a mapped, aligned 4-byte user word.
    unsafe { core::ptr::read_volatile(address as *const u32) }
}

fn set_result(tf: &mut TrapFrame, status: u32) {
    tf.regs[2] = status as u64;
    tf.regs[3] = 0;
}

fn print_dec(value: u32) {
    irix7_fmt::decimal(u64::from(value), &mut |byte| crate::write_byte(byte));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn table_size_and_saved_frame() {
        assert_eq!(MAX_THREADS, 8);
        assert_eq!(core::mem::size_of::<TrapFrame>(), 0x140);
        assert!(core::mem::size_of::<Thread>() >= core::mem::size_of::<TrapFrame>());
    }

    #[test]
    fn timeouts_follow_the_poll_rule() {
        assert_eq!(timeout_ticks(0), 0);
        assert_eq!(
            timeout_ticks(-1),
            crate::fd::WAIT_LIMIT_MS * crate::fd::TICKS_PER_MS
        );
        assert_eq!(timeout_ticks(5), 5 * crate::fd::TICKS_PER_MS);
        assert_eq!(
            timeout_ticks(i32::MAX),
            crate::fd::WAIT_LIMIT_MS * crate::fd::TICKS_PER_MS
        );
    }
}
