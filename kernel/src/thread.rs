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
//! A sleep parks the thread on a CP0 Count deadline instead of a word.  It has
//! no waker: the deadline itself resumes the thread, either at a scheduling
//! point or after the timer interrupt that ends a kernel wait.  A sleep never
//! spins: with no runnable peer the kernel arms CP0 Compare for the deadline
//! and executes `wait`.
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
    /// Parked on a sleep deadline; the saved frame resumes at expiry.
    Sleeping,
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
    /// CP0 Count value sampled when the wait or sleep began.
    wait_start: u32,
    /// Wait or sleep interval in CP0 Count ticks.
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

/// A complete thread table, saved with the process that owns it.
///
/// Threads share one address space, so the table is live state, not part of the
/// address space.  A process switch snapshots the outgoing table and installs
/// the incoming one; the opaque wrapper keeps `Thread` private.
#[derive(Clone, Copy)]
pub struct Snapshot {
    slots: [Thread; MAX_THREADS],
    current: usize,
}

impl Snapshot {
    /// An empty table, for a process slot that has never run.
    pub const EMPTY: Self = Self {
        slots: [EMPTY; MAX_THREADS],
        current: 0,
    };
}

/// Copy the live thread table.
pub fn snapshot() -> Snapshot {
    // SAFETY: single-threaded kernel; the table is not modified during the
    // copy.
    unsafe {
        Snapshot {
            slots: THREADS,
            current: CURRENT,
        }
    }
}

/// Make `table` the live thread table.
pub fn install(table: Snapshot) {
    // SAFETY: single-threaded kernel; the table belongs to the process the
    // address space and TLB set are being switched to.
    unsafe {
        THREADS = table.slots;
        CURRENT = table.current;
    }
}

/// Save the running thread's frame into its slot before a process switch.
pub fn save_current(tf: &TrapFrame) {
    unsafe {
        let threads = &mut *core::ptr::addr_of_mut!(THREADS);
        threads[CURRENT].frame = *tf;
    }
}

/// Build a table whose only thread is the process's main thread.
pub fn single(frame: TrapFrame) -> Snapshot {
    let mut snapshot = Snapshot::EMPTY;
    for (index, slot) in snapshot.slots.iter_mut().enumerate() {
        slot.tid = index as u32 + 1;
    }
    snapshot.slots[0].state = State::Running;
    snapshot.slots[0].frame = frame;
    snapshot.current = 0;
    snapshot
}

/// The running thread id (the main thread is 1).
pub fn current_tid() -> u32 {
    unsafe { (*core::ptr::addr_of!(THREADS))[CURRENT].tid }
}

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

/// Resume parked threads whose deadline has passed.
///
/// Deadlines are applied at scheduling points: the next yield, block or exit,
/// and the kernel wait that follows a timer interrupt.  There is no
/// preemption, so a parked thread still cannot run until the running thread
/// reaches one of those points.  A futex wait ended here still follows the
/// futex contract, so the word is compared again; a sleep has no word and
/// resumes with `CELL_OK`.
fn expire_waiters() {
    let now = crate::traps::count_read();
    // SAFETY: single-threaded kernel; no thread runs while this scan is in
    // progress.
    unsafe {
        let threads = &mut *core::ptr::addr_of_mut!(THREADS);
        for slot in threads.iter_mut() {
            if !matches!(slot.state, State::Blocked | State::Sleeping)
                || !deadline_passed(now, slot.wait_start, slot.wait_limit)
            {
                continue;
            }
            if slot.state == State::Sleeping {
                let elapsed = now.wrapping_sub(slot.wait_start);
                slot.frame.regs[2] = native_abi::status::CELL_OK as u64;
                slot.frame.regs[3] = 0;
                slot.state = State::Runnable;
                crate::print_str("sleep: expiry tid=");
                print_dec(slot.tid);
                crate::print_str(" limit=");
                print_dec(slot.wait_limit / crate::fd::TICKS_PER_MS);
                crate::print_str("ms elapsed=");
                print_dec(elapsed / crate::fd::TICKS_PER_MS);
                crate::print_str("ms");
                crate::println("");
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

/// Whether `limit` ticks have passed since `start`.
///
/// CP0 Count is 32 bits and wraps every 130 s, so the difference between two
/// samples is the only meaningful measure.  Every parked interval is shorter
/// than half the range, which keeps the unsigned comparison unambiguous.
fn deadline_passed(now: u32, start: u32, limit: u32) -> bool {
    now.wrapping_sub(start) >= limit
}

/// Apply this cell's thread deadlines and return the frame to resume.
///
/// A process-level wake can belong to a thread other than the one that parked
/// the cell: the cell's deadline is the earliest of its parked threads, and a
/// sibling thread may own it.  This scan expires the cell's parked threads
/// against the now-active address space (a futex waiter re-checks its word),
/// then makes the first runnable thread current and returns its saved frame.
pub(crate) fn resume_after_wake() -> Option<TrapFrame> {
    expire_waiters();
    // SAFETY: single-threaded kernel; the cell's table was just installed and
    // no thread runs while this scan is in progress.
    unsafe {
        let threads = &mut *core::ptr::addr_of_mut!(THREADS);
        let from = CURRENT;
        let to = if matches!(threads[from].state, State::Runnable | State::Running) {
            from
        } else {
            let mut found = None;
            for step in 1..=MAX_THREADS {
                let index = (from + step) % MAX_THREADS;
                if threads[index].state == State::Runnable {
                    found = Some(index);
                    break;
                }
            }
            found?
        };
        threads[to].state = State::Running;
        CURRENT = to;
        Some(threads[to].frame)
    }
}

/// The earliest deadline of any parked thread, as a CP0 Count value.
///
/// Every parked thread is bounded: a futex wait by the poll rule and a sleep
/// by [`user::SLEEP_LIMIT_MS`].  The kernel can therefore always park until
/// the nearest deadline instead of spinning when nothing is runnable.
pub(crate) fn nearest_deadline() -> Option<u32> {
    let mut nearest: Option<u32> = None;
    // SAFETY: single-threaded kernel; a plain read of the table.
    unsafe {
        for slot in (*core::ptr::addr_of!(THREADS)).iter() {
            if !matches!(slot.state, State::Blocked | State::Sleeping) {
                continue;
            }
            let deadline = slot.wait_start.wrapping_add(slot.wait_limit);
            nearest = Some(match nearest {
                Some(current) if (deadline.wrapping_sub(current) as i32) < 0 => deadline,
                Some(current) => current,
                None => deadline,
            });
        }
    }
    nearest
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
    park_futex(tf, from, to, address, expected, limit)
}

/// Park the running thread on `address` and resume `to`.
fn park_futex(
    tf: &mut TrapFrame,
    from: usize,
    to: usize,
    address: u32,
    expected: u32,
    limit: u32,
) -> Wait {
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

    prepare_park(tf, from, State::Blocked, address, expected, limit);
    switch_to(tf, from, to);
    Wait::Switched
}

/// Save `tf` into the current thread's slot and mark it parked.
///
/// The frame is completed before it is saved: `$v0`/`$v1` hold the result the
/// thread sees when it resumes, and `EPC` is advanced past the gate so the
/// exception return does not run the syscall again.
fn prepare_park(
    tf: &mut TrapFrame,
    from: usize,
    state: State,
    address: u32,
    expected: u32,
    limit: u32,
) {
    // SAFETY: the running thread's frame is saved before the switch; a waker
    // or the deadline scan writes the result into the saved frame while this
    // thread cannot run.
    unsafe {
        let threads = &mut *core::ptr::addr_of_mut!(THREADS);
        threads[from].wait_addr = address;
        threads[from].wait_expected = expected;
        threads[from].wait_start = crate::traps::count_read();
        threads[from].wait_limit = limit;
        threads[from].state = state;

        tf.regs[2] = native_abi::status::CELL_OK as u64;
        tf.regs[3] = 0;
        tf.advance_epc();
        threads[from].frame = *tf;
    }
}

/// Park the current thread on a sleep deadline without switching threads.
///
/// The caller hands the cell to [`crate::process::park_current`], which may
/// install another cell.  If no cell is runnable the caller must not use this:
/// the thread would stay parked in a table that nothing selects.
pub(crate) fn prepare_sleep(tf: &mut TrapFrame, limit: u32) {
    prepare_park(tf, unsafe { CURRENT }, State::Sleeping, 0, 0, limit);
}

/// Park the current thread on a futex word without switching threads.
///
/// Used when no sibling thread can run but another cell can.  The deadline
/// scan re-checks the word when the cell wakes, so the futex contract is
/// unchanged by the process-level park.
pub(crate) fn prepare_futex(tf: &mut TrapFrame, address: u32, expected: u32, limit: u32) {
    prepare_park(
        tf,
        unsafe { CURRENT },
        State::Blocked,
        address,
        expected,
        limit,
    );
}

/// Wait alone, with no runnable thread that could wake this one.
///
/// The poll rule applies: the wait is bounded, the word is re-read while it
/// lasts, and a changed word is `CELL_E_AGAIN`.  Nothing can wake this thread
/// early, so the kernel parks the CPU on the timer: until this wait's deadline
/// or a sleeping peer's earlier one, whichever comes first.  If a sleeper
/// expires first it becomes runnable, and this thread parks for the rest of
/// its own budget and lets it run.
fn bounded_wait(tf: &mut TrapFrame, address: u32, expected: u32, limit: u32) -> Wait {
    let from = unsafe { CURRENT };
    let tid = unsafe { (*core::ptr::addr_of!(THREADS))[from].tid };
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
        let elapsed = crate::traps::count_read().wrapping_sub(start);
        if elapsed >= limit {
            set_result(tf, CELL_E_TIMEDOUT);
            return Wait::Continue;
        }
        if let Some(to) = next_runnable(from) {
            return park_futex(tf, from, to, address, expected, limit - elapsed);
        }
        // No sibling thread, but another cell may be runnable.  Park the cell
        // with this thread's deadline and let it run; the cell's wake scan
        // re-checks the word before the waiter resumes.
        if crate::process::park_current(tf, |frame| {
            prepare_futex(frame, address, expected, limit - elapsed)
        }) {
            return Wait::Switched;
        }
        // Wait in short slices so a terminal signal ends the sleep: the
        // console is polled, and nothing else would run the line discipline
        // while this cell holds the CPU.
        let slice = crate::traps::count_read().wrapping_add(SIGNAL_SLICE_TICKS);
        let wake = next_wake(start.wrapping_add(limit));
        crate::traps::wait_until(if (slice.wrapping_sub(wake) as i32) < 0 {
            slice
        } else {
            wake
        });
        crate::tty::pump();
        if crate::process::signal_pending() {
            set_result(tf, native_abi::status::CELL_E_INTR);
            return Wait::Continue;
        }
    }
}

/// The earliest of `own`, a parked thread's deadline and a sleeping cell's
/// deadline.  CPU parking must end at whichever comes first.
fn next_wake(own: u32) -> u32 {
    let mut wake = own;
    if let Some(deadline) = nearest_deadline() {
        if (deadline.wrapping_sub(wake) as i32) < 0 {
            wake = deadline;
        }
    }
    if let Some(deadline) = crate::process::nearest_deadline() {
        if (deadline.wrapping_sub(wake) as i32) < 0 {
            wake = deadline;
        }
    }
    wake
}

/// How long a solo sleep waits before it polls the console for a signal.
const SIGNAL_SLICE_TICKS: u32 = 10 * crate::fd::TICKS_PER_MS;

/// Park the running thread until `limit` CP0 Count ticks have passed.
///
/// The sleep has no waker, so the only way out is the deadline.  A runnable
/// peer is switched to and runs until its next scheduling point, where this
/// thread's deadline is re-checked.  With no runnable peer the kernel cannot
/// hand the CPU to anyone, so it arms the timer for the deadline (or a parked
/// peer's earlier one) and parks the CPU in `wait` instead of spinning.  If
/// that peer becomes runnable first, this thread parks for the rest of its
/// budget and switches to it.
pub fn sleep_current(tf: &mut TrapFrame, limit: u32) -> Wait {
    let from = unsafe { CURRENT };
    if limit == 0 {
        set_result(tf, native_abi::status::CELL_OK);
        return Wait::Continue;
    }

    crate::print_str("sleep: wait tid=");
    print_dec(unsafe { (*core::ptr::addr_of!(THREADS))[from].tid });
    crate::print_str(" limit=");
    print_dec(limit / crate::fd::TICKS_PER_MS);
    crate::print_str("ms");
    crate::println("");

    let start = crate::traps::count_read();
    loop {
        let elapsed = crate::traps::count_read().wrapping_sub(start);
        if elapsed >= limit {
            set_result(tf, native_abi::status::CELL_OK);
            return Wait::Continue;
        }
        if let Some(to) = next_runnable(from) {
            return park_sleep(tf, from, to, limit - elapsed);
        }
        // No thread in this cell can run, but another cell can.  Mark the
        // cell sleeping and hand it the CPU; it resumes through the deadline
        // scan with `CELL_OK`, and this thread's own slot expires with it.
        if crate::process::park_current(tf, |frame| prepare_sleep(frame, limit - elapsed)) {
            return Wait::Switched;
        }
        // Wait in short slices so a terminal signal ends the sleep: the
        // console is polled, and nothing else would run the line discipline
        // while this cell holds the CPU.
        let slice = crate::traps::count_read().wrapping_add(SIGNAL_SLICE_TICKS);
        let wake = next_wake(start.wrapping_add(limit));
        crate::traps::wait_until(if (slice.wrapping_sub(wake) as i32) < 0 {
            slice
        } else {
            wake
        });
        crate::tty::pump();
        if crate::process::signal_pending() {
            set_result(tf, native_abi::status::CELL_E_INTR);
            return Wait::Continue;
        }
    }
}

/// Park the sleeping thread for `limit` more ticks and resume `to`.
fn park_sleep(tf: &mut TrapFrame, from: usize, to: usize, limit: u32) -> Wait {
    crate::print_str("sleep: park tid=");
    print_dec(unsafe { (*core::ptr::addr_of!(THREADS))[from].tid });
    crate::print_str(" limit=");
    print_dec(limit / crate::fd::TICKS_PER_MS);
    crate::print_str("ms");
    crate::println("");

    prepare_park(tf, from, State::Sleeping, 0, 0, limit);
    switch_to(tf, from, to);
    Wait::Switched
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
/// With every remaining thread parked on a bounded futex wait or a sleep, the
/// kernel keeps the image alive until the nearest deadline instead of exiting
/// under a thread that the clock can still resume.  That wait is a timer wait,
/// not a spin.
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
        let Some(deadline) = nearest_deadline() else {
            return Exit::Last;
        };
        crate::traps::wait_until(deadline);
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
    fn deadlines_compare_across_the_count_wrap() {
        assert!(!deadline_passed(99, 100, 2));
        assert!(deadline_passed(102, 100, 2));
        assert!(!deadline_passed(u32::MAX, u32::MAX - 1, 4));
        assert!(deadline_passed(1, u32::MAX - 1, 4));
        // The longest parked sleep stays below half the Count range.
        let cap = crate::user::SLEEP_LIMIT_MS * crate::fd::TICKS_PER_MS;
        assert!(cap < u32::MAX / 2);
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
