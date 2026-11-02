// SPDX-License-Identifier: GPL-3.0-only
//! Cooperative user threads inside one address space.
//!
//! The v0 thread gate runs until a thread traps with `SYS_YIELD`,
//! `SYS_THREAD_EXIT` or a process exit.  There is no timer preemption and no
//! blocking wait, so one kernel stack serves every thread.  Each thread owns a
//! saved [`TrapFrame`]; a switch saves the outgoing frame in its slot, copies
//! the incoming slot into the trap frame, and lets the exception return path
//! enter the new thread exactly as if it had just returned from its own
//! syscall.
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

use crate::syserr::{SysResult, CELL_E_AGAIN, CELL_E_FAULT, CELL_E_INVAL};
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
    /// Exited; the slot holds the exit status until it is reused.
    Dead,
}

#[derive(Clone, Copy)]
struct Thread {
    state: State,
    tid: u32,
    exit_status: u32,
    frame: TrapFrame,
}

const EMPTY: Thread = Thread {
    state: State::Unused,
    tid: 0,
    exit_status: 0,
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

fn next_runnable(from: usize) -> Option<usize> {
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
        threads[index].state = State::Runnable;
        let tid = threads[index].tid;

        crate::print_str("thread: create by=");
        print_tid(threads[CURRENT].tid);
        crate::print_str(" tid=");
        print_tid(tid);
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

/// Retire the running thread with `status` and resume the next runnable one.
pub fn exit_current(tf: &mut TrapFrame, status: u32) -> Exit {
    let from = unsafe { CURRENT };
    let tid = unsafe {
        let threads = &mut *core::ptr::addr_of_mut!(THREADS);
        threads[from].state = State::Dead;
        threads[from].exit_status = status;
        threads[from].tid
    };
    crate::print_str("thread: exit tid=");
    print_tid(tid);
    crate::print_str(" status=0x");
    crate::print_hex32(status);
    crate::println("");

    match next_runnable(from) {
        Some(to) => {
            switch_to(tf, from, to);
            Exit::Switched
        }
        None => Exit::Last,
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

fn print_tid(tid: u32) {
    irix7_fmt::decimal(u64::from(tid), &mut |byte| crate::write_byte(byte));
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
}
