// SPDX-License-Identifier: GPL-3.0-only
//! Per-replica process owner for native cells with a Unix personality.
//!
//! IRIX 7 has one global object space and an actor directory sharded by
//! rendezvous home.  This table is **not** that directory: it is the replica's
//! owner/cache of the cells that execute here.  Every record carries the global
//! identity from creation — `Job` OID, `ActorId`, home, home epoch and object
//! epoch — plus a local Unix `pid` that only the POSIX personality sees.
//!
//! `fork` clones the current cell's address space and descriptor table and
//! mints a new child identity.  `exec` replaces the image and advances the
//! `Job` object epoch.  `waitpid` is the local fast path over the child's exit
//! event; a remote wait is a capability invocation routed by the child's home
//! and epochs, not a second local mechanism.
//!
//! The v0 table holds eight cells and services only local operations.  The
//! fields needed by the mesh path are present now so that adding the routed
//! spawn/wait path does not change the process record.

use sys_primitives::{ActorId, NodeId, ObjectId};

use crate::fd;
use crate::syserr::{
    SysResult, CELL_E_AGAIN, CELL_E_CHILD, CELL_E_INVAL, CELL_E_NOMEM, CELL_E_NOTSUP, CELL_E_SRCH,
};
use crate::thread;
use crate::traps::{self, TrapFrame};
use crate::user::{self, Space};

/// Local process slots.  A replica owns a bounded number of local cells; the
/// global actor directory, not this table, is the namespace.
pub const MAX_PROCESSES: usize = 8;

const NO_PROCESS: usize = usize::MAX;
const WNOHANG: i32 = 1;
const RUSAGE_SIZE: u32 = 144;

#[derive(Clone, Copy, PartialEq, Eq)]
enum State {
    Unused,
    Runnable,
    Running,
    /// Parked in `waitpid`; it is not selected until a child wakes it.
    Blocked,
    /// Exited, status retained until the parent waits.
    Zombie,
}

/// The global identity of one cell.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Identity {
    /// Mutable `Job` object identity; stable across `exec`.
    pub job: ObjectId,
    /// Actor instance that executes the cell.
    pub actor: ActorId,
    /// Node that owns the `Job` record.
    pub home: NodeId,
    /// Home assignment epoch at creation.
    pub home_epoch: u32,
    /// Job generation; `exec` advances it.
    pub object_epoch: u32,
    /// Local POSIX pid; not unique across the mesh.
    pub pid: u32,
}

impl Identity {
    const EMPTY: Self = Self {
        job: ObjectId::ZERO,
        actor: ActorId([0; 16]),
        home: NodeId::ZERO,
        home_epoch: 0,
        object_epoch: 0,
        pid: 0,
    };

    /// The ABI record returned by `SYS_CELL_ID`.
    pub fn to_abi(self) -> native_abi::cell_id {
        native_abi::cell_id {
            job: self.job.0,
            actor: self.actor.0,
            home: self.home.0,
            home_epoch: self.home_epoch,
            object_epoch: self.object_epoch,
            pid: self.pid as i32,
            reserved: 0,
        }
    }
}

#[derive(Clone, Copy)]
struct WaitRequest {
    /// Local pid, -1 for any child, 0 for the (empty) process group.
    pid: i32,
    status: u32,
    rusage: u32,
}

/// A child exit that woke a waiting parent.  The status is copied into the
/// parent's own address space only after that space is active again, so the
/// write is cache-coherent.
#[derive(Clone, Copy)]
struct PendingWait {
    pid: u32,
    status: u32,
    status_address: u32,
    rusage: u32,
}

#[derive(Clone, Copy)]
struct Process {
    state: State,
    identity: Identity,
    /// Parent actor, for the routed exit event; `ppid` is the local view.
    parent: Option<ActorId>,
    ppid: u32,
    exit_status: u32,
    frame: TrapFrame,
    threads: thread::Snapshot,
    space: Space,
    fds: fd::Snapshot,
    abi: traps::UserAbi,
    asid: u32,
    wait: Option<WaitRequest>,
    pending_wait: Option<PendingWait>,
}

impl Process {
    const EMPTY: Self = Self {
        state: State::Unused,
        identity: Identity::EMPTY,
        parent: None,
        ppid: 0,
        exit_status: 0,
        frame: TrapFrame::ZERO,
        threads: thread::Snapshot::EMPTY,
        space: Space::EMPTY,
        fds: fd::Snapshot::EMPTY,
        abi: traps::UserAbi::N32,
        asid: 0,
        wait: None,
        pending_wait: None,
    };
}

/// The result of retiring the running process.
pub enum Exit {
    /// The trap frame now belongs to another process.
    Switched,
    /// No process is left; the caller must return to the kernel console.
    Last,
}

/// The result of `waitpid`.
pub enum Wait {
    /// The caller continues with the result already in its frame.
    Continue,
    /// The frame now belongs to another process.
    Switched,
}

static mut PROCESSES: [Process; MAX_PROCESSES] = [Process::EMPTY; MAX_PROCESSES];
static mut CURRENT: usize = NO_PROCESS;
static mut NEXT_PID: u32 = 2;
static mut NEXT_CELL: u32 = 1;

/// Clear the table.  Every live address space is released first.
pub fn reset() {
    // SAFETY: single-threaded kernel; no process runs while the console owns
    // the machine.
    unsafe {
        let processes = &mut *core::ptr::addr_of_mut!(PROCESSES);
        for slot in processes.iter_mut() {
            slot.space.release();
            fd::release_snapshot(&slot.fds);
            *slot = Process::EMPTY;
        }
        CURRENT = NO_PROCESS;
        NEXT_PID = 2;
        NEXT_CELL = 1;
    }
}

fn mint_identity(pid: u32) -> Option<Identity> {
    let node = crate::mesh::node_id();
    if node.is_zero() {
        return None;
    }
    // SAFETY: single-threaded kernel; the counter is only read here.
    let counter = unsafe { NEXT_CELL };
    unsafe {
        NEXT_CELL = NEXT_CELL.wrapping_add(1).max(1);
    }
    // A mutable Job has a home-assigned identity, not a content OID.  v0 mints
    // a domain-separated 256-bit identity locally; the home/object epochs fence
    // it, and the object service will own the BLAKE3 namespace when the routed
    // spawn path commits the Job at its home.
    let incarnation = crate::mesh::incarnation();
    let actor = {
        let mut hasher = irix7_sha256::Hasher::new();
        hasher.update(b"IRIX7-ACTOR-v1");
        hasher.update(node.as_bytes());
        hasher.update(&incarnation.to_be_bytes());
        hasher.update(&counter.to_be_bytes());
        hasher.update(&pid.to_be_bytes());
        let digest = hasher.finalize().0;
        let mut actor = [0u8; 16];
        actor.copy_from_slice(&digest[..16]);
        ActorId(actor)
    };
    let home = crate::mesh::actor_home_for(&actor)?;
    let job = {
        let mut hasher = irix7_sha256::Hasher::new();
        hasher.update(b"IRIX7-JOB-v1");
        hasher.update(&actor.0);
        hasher.update(&incarnation.to_be_bytes());
        hasher.update(&counter.to_be_bytes());
        ObjectId(hasher.finalize().0)
    };
    Some(Identity {
        job,
        actor,
        home,
        home_epoch: crate::mesh::epoch(),
        object_epoch: 1,
        pid,
    })
}

fn allocate_pid() -> Option<u32> {
    for _ in 0..=MAX_PROCESSES as u32 + 1 {
        // SAFETY: single-threaded kernel.
        let pid = unsafe { NEXT_PID };
        unsafe {
            NEXT_PID = NEXT_PID.wrapping_add(1);
            if NEXT_PID < 2 {
                NEXT_PID = 2;
            }
        }
        if pid < 2 {
            continue;
        }
        // SAFETY: a plain scan of the table.
        let taken = unsafe {
            (*core::ptr::addr_of!(PROCESSES))
                .iter()
                .any(|slot| slot.state != State::Unused && slot.identity.pid == pid)
        };
        if !taken {
            return Some(pid);
        }
    }
    None
}

fn free_slot() -> Option<usize> {
    // SAFETY: single-threaded kernel.
    unsafe {
        (*core::ptr::addr_of!(PROCESSES))
            .iter()
            .position(|slot| slot.state == State::Unused)
    }
}

fn index_of_pid(pid: u32) -> Option<usize> {
    // SAFETY: single-threaded kernel.
    unsafe {
        (*core::ptr::addr_of!(PROCESSES)).iter().position(|slot| {
            matches!(
                slot.state,
                State::Runnable | State::Running | State::Blocked
            ) && slot.identity.pid == pid
        })
    }
}

/// Install the first process after its space and frame have been built.
pub fn start_first(
    space: Space,
    abi: traps::UserAbi,
    entry: u32,
    user_sp: u32,
    argc: u32,
    argv: u32,
    envp: u32,
    auxv: u32,
) -> SysResult<()> {
    reset();
    let Some(identity) = mint_identity(1) else {
        return Err(CELL_E_SRCH);
    };
    if identity.home != crate::mesh::node_id() {
        return Err(CELL_E_NOTSUP);
    }
    let mut frame = TrapFrame::ZERO;
    frame.epc = entry as u64;
    frame.regs[4] = argc as u64;
    frame.regs[5] = argv as u64;
    frame.regs[6] = envp as u64;
    frame.regs[7] = auxv as u64;
    frame.regs[28] = 0;
    frame.regs[29] = user_sp as u64;
    frame.status = traps::user_entry_status();
    let asid = 0; // The whole user TLB set is reloaded on every switch.
    let fds = fd::console_table();

    // SAFETY: single-threaded kernel; this is the first process installation.
    unsafe {
        let processes = &mut *core::ptr::addr_of_mut!(PROCESSES);
        processes[0] = Process {
            state: State::Running,
            identity,
            parent: None,
            ppid: 0,
            exit_status: 0,
            frame,
            threads: thread::single(frame),
            space,
            fds,
            abi,
            asid,
            wait: None,
            pending_wait: None,
        };
        CURRENT = 0;
    }
    thread::install(thread::single(frame));
    fd::restore(fds);
    space.activate(asid);
    traps::set_user_abi(abi);
    Ok(())
}

/// Clone the current cell into a new local process.
pub fn fork(tf: &TrapFrame) -> SysResult<u32> {
    let from = current_index().ok_or(CELL_E_SRCH)?;
    let Some(to) = free_slot() else {
        return Err(CELL_E_AGAIN);
    };
    let Some(pid) = allocate_pid() else {
        return Err(CELL_E_AGAIN);
    };
    let Some(identity) = mint_identity(pid) else {
        return Err(CELL_E_SRCH);
    };
    if identity.home != crate::mesh::node_id() {
        // A child whose rendezvous home is elsewhere must commit its Job at
        // that home first; v0 has no routed spawn, so it fails closed.
        return Err(CELL_E_NOTSUP);
    }

    // SAFETY: single-threaded kernel; the current slot is live.
    let (parent_actor, parent_pid, parent_space, child_fds) = unsafe {
        let processes = &*core::ptr::addr_of!(PROCESSES);
        (
            processes[from].identity.actor,
            processes[from].identity.pid,
            processes[from].space,
            fd::fork_snapshot(),
        )
    };
    let mut child_space = Space::EMPTY;
    if !child_space.copy_from(&parent_space) {
        child_space.release();
        fd::release_snapshot(&child_fds);
        return Err(CELL_E_NOMEM);
    }
    let mut frame = *tf;
    frame.advance_epc();
    frame.regs[2] = native_abi::status::CELL_OK as u64;
    frame.regs[3] = 0;
    let asid = 0;
    // SAFETY: single-threaded kernel; the child slot is free and no process can
    // run until this syscall returns.
    let abi = unsafe { (*core::ptr::addr_of!(PROCESSES))[from].abi };
    unsafe {
        let processes = &mut *core::ptr::addr_of_mut!(PROCESSES);
        processes[to] = Process {
            state: State::Runnable,
            identity,
            parent: Some(parent_actor),
            ppid: parent_pid,
            exit_status: 0,
            frame,
            threads: thread::single(frame),
            space: child_space,
            fds: child_fds,
            abi,
            asid,
            wait: None,
            pending_wait: None,
        };
    }
    crate::print_str("fork: pid=");
    print_dec(parent_pid);
    crate::print_str(" child=");
    print_dec(pid);
    crate::print_str(" actor=");
    print_hex16(&identity.actor.0);
    crate::print_str(" epc=0x");
    crate::print_hex32(frame.epc as u32);
    crate::print_str(" ra=0x");
    crate::print_hex32(frame.regs[31] as u32);
    crate::print_str(" sp=0x");
    crate::print_hex32(frame.regs[29] as u32);
    crate::println("");
    Ok(pid)
}

/// Replace the current cell's image and enter the new program.
pub fn exec(tf: &mut TrapFrame, path: u32, argv: u32, envp: u32) -> SysResult<()> {
    // Build the complete new space before touching the old one; a failure
    // leaves the caller's image intact.
    let executable = match user::prepare_exec(path, argv, envp) {
        Ok(executable) => executable,
        Err(status) => {
            // `prepare_exec` may have activated a new space before it failed.
            // Put the old one back so the caller's image remains usable.
            if let Some(space) = current_space() {
                space.activate(0);
            }
            return Err(status);
        }
    };
    let from = current_index().ok_or(CELL_E_SRCH)?;
    // SAFETY: single-threaded kernel; the current slot is live.
    unsafe {
        let processes = &mut *core::ptr::addr_of_mut!(PROCESSES);
        let process = &mut processes[from];
        let mut old_space = process.space;
        process.space = executable.space;
        old_space.release();
        fd::close_cloexec();
        process.fds = fd::snapshot();
        process.identity.object_epoch = process.identity.object_epoch.saturating_add(1);
        process.abi = executable.abi;
        let mut frame = TrapFrame::ZERO;
        frame.epc = executable.entry as u64;
        frame.regs[4] = executable.argc as u64;
        frame.regs[5] = executable.argv as u64;
        frame.regs[6] = executable.envp as u64;
        frame.regs[7] = executable.auxv as u64;
        frame.regs[28] = 0;
        frame.regs[29] = executable.sp as u64;
        frame.status = traps::user_entry_status();
        frame.entryhi = process.asid as u64;
        process.frame = frame;
        process.threads = thread::single(frame);
        process.space.activate(process.asid);
        thread::install(process.threads);
        traps::set_user_abi(process.abi);
        *tf = frame;
    }
    crate::print_str("exec: entry=0x");
    crate::print_hex32(executable.entry);
    crate::println("");
    Ok(())
}

fn encode_status(status: u32) -> u32 {
    (status & 0xFF) << 8
}

fn child_matches(process: &Process, parent_pid: u32, pid: i32) -> bool {
    process.ppid == parent_pid
        && process.state != State::Unused
        && (pid == -1 || pid == 0 || pid as u32 == process.identity.pid)
}

/// Reap one zombie child and copy its status into the caller's space.
fn reap_child(from: usize, index: usize) -> u32 {
    // SAFETY: single-threaded kernel; both indices are live slots.
    unsafe {
        let processes = &mut *core::ptr::addr_of_mut!(PROCESSES);
        let status = encode_status(processes[index].exit_status);
        let pid = processes[index].identity.pid;
        let request = processes[from].wait;
        let status_address = request.map(|request| request.status).unwrap_or(0);
        if status_address != 0 {
            unsafe {
                core::ptr::write_volatile(status_address as *mut u32, status);
            }
        }
        processes[index].space.release();
        fd::release_snapshot(&processes[index].fds);
        processes[index] = Process::EMPTY;
        pid
    }
}

/// `waitpid` / `wait4` local fast path.
pub fn wait(
    tf: &mut TrapFrame,
    pid: i32,
    status_address: u32,
    options: i32,
    rusage_address: u32,
) -> SysResult<Wait> {
    if options & !WNOHANG != 0 {
        return Err(CELL_E_INVAL);
    }
    let from = current_index().ok_or(CELL_E_SRCH)?;
    let parent_pid = unsafe { (*core::ptr::addr_of!(PROCESSES))[from].identity.pid };
    if status_address != 0 && !user::range_ok(status_address, 4) {
        return Err(crate::syserr::CELL_E_FAULT);
    }
    if rusage_address != 0 && !user::range_ok(rusage_address, RUSAGE_SIZE) {
        return Err(crate::syserr::CELL_E_FAULT);
    }

    // A zombie that is already waiting for this parent is the common case.
    // SAFETY: single-threaded kernel.
    let zombie = unsafe {
        (*core::ptr::addr_of!(PROCESSES))
            .iter()
            .position(|process| {
                process.state == State::Zombie && child_matches(process, parent_pid, pid)
            })
    };
    if let Some(index) = zombie {
        unsafe {
            (*core::ptr::addr_of_mut!(PROCESSES))[from].wait = Some(WaitRequest {
                pid,
                status: status_address,
                rusage: rusage_address,
            });
        }
        let child_pid = reap_child(from, index);
        if rusage_address != 0 {
            zero_rusage(rusage_address);
        }
        // SAFETY: single-threaded kernel; the caller continues.
        unsafe {
            let frame = &mut (*core::ptr::addr_of_mut!(PROCESSES))[from].frame;
            frame.regs[2] = native_abi::status::CELL_OK as u64;
            frame.regs[3] = child_pid as u64;
            (*core::ptr::addr_of_mut!(PROCESSES))[from].wait = None;
        }
        tf.regs[2] = native_abi::status::CELL_OK as u64;
        tf.regs[3] = child_pid as u64;
        return Ok(Wait::Continue);
    }

    // SAFETY: single-threaded kernel.
    let has_child = unsafe {
        (*core::ptr::addr_of!(PROCESSES)).iter().any(|process| {
            process.state != State::Unused
                && process.ppid == parent_pid
                && (pid == -1 || pid == 0 || pid as u32 == process.identity.pid)
        })
    };
    if !has_child {
        return Err(CELL_E_CHILD);
    }
    if options & WNOHANG != 0 {
        tf.regs[2] = native_abi::status::CELL_OK as u64;
        tf.regs[3] = 0;
        return Ok(Wait::Continue);
    }

    // Block this cell and run the child.  The status is written into the
    // parent's own address space when the exit event arrives.  Advance the
    // saved EPC now: the switch saves this same frame and the incoming frame
    // returns through the exception path rather than re-running the gate.
    let Some(to) = next_runnable(from) else {
        return Err(CELL_E_CHILD);
    };
    tf.advance_epc();
    tf.regs[2] = native_abi::status::CELL_OK as u64;
    tf.regs[3] = 0;
    unsafe {
        let processes = &mut *core::ptr::addr_of_mut!(PROCESSES);
        processes[from].wait = Some(WaitRequest {
            pid,
            status: status_address,
            rusage: rusage_address,
        });
        processes[from].frame = *tf;
    }
    thread::save_current(tf);
    // SAFETY: single-threaded kernel.
    unsafe {
        (*core::ptr::addr_of_mut!(PROCESSES))[from].threads = thread::snapshot();
    }
    // switch_to saves the frame and descriptor table; keep the parent out of
    // the run queue until a child exit supplies a result.
    // SAFETY: single-threaded kernel.
    unsafe {
        (*core::ptr::addr_of_mut!(PROCESSES))[from].state = State::Blocked;
    }
    switch_to(tf, from, to);
    Ok(Wait::Switched)
}

fn zero_rusage(address: u32) {
    for offset in 0..RUSAGE_SIZE {
        // SAFETY: the caller checked the whole range.
        unsafe {
            core::ptr::write_volatile((address as *mut u8).add(offset as usize), 0);
        }
    }
}

/// Retire the current cell with `status`.
pub fn exit_current(tf: &mut TrapFrame, status: u32) -> Exit {
    let from = match current_index() {
        Some(index) => index,
        None => return Exit::Last,
    };
    // SAFETY: single-threaded kernel; the current slot is live.
    let (pid, parent_actor, parent_pid) = unsafe {
        let processes = &mut *core::ptr::addr_of_mut!(PROCESSES);
        let process = &mut processes[from];
        process.state = State::Zombie;
        process.exit_status = status;
        process.space.release();
        fd::release_snapshot(&process.fds);
        (process.identity.pid, process.parent, process.ppid)
    };
    crate::print_str("process: exit pid=");
    print_dec(pid);
    crate::print_str(" status=0x");
    crate::print_hex32(status);
    crate::println("");

    // Wake a parent already blocked in waitpid().  The status is written
    // through the parent's own page map, then the child is reaped.
    let parent = parent_actor
        .and_then(|actor| {
            // SAFETY: single-threaded kernel.
            unsafe {
                (*core::ptr::addr_of!(PROCESSES))
                    .iter()
                    .position(|process| {
                        process.state != State::Unused
                            && process.state != State::Zombie
                            && process.identity.actor == actor
                    })
            }
        })
        .or_else(|| index_of_pid(parent_pid));
    let mut woke = false;
    if let Some(parent_index) = parent {
        // SAFETY: single-threaded kernel; the parent slot is live.
        unsafe {
            let processes = &mut *core::ptr::addr_of_mut!(PROCESSES);
            let request = processes[parent_index].wait;
            if let Some(request) = request {
                if child_matches(
                    &processes[from],
                    processes[parent_index].identity.pid,
                    request.pid,
                ) {
                    processes[parent_index].pending_wait = Some(PendingWait {
                        pid,
                        status: encode_status(status),
                        status_address: request.status,
                        rusage: request.rusage,
                    });
                    processes[parent_index].frame.regs[2] = native_abi::status::CELL_OK as u64;
                    processes[parent_index].frame.regs[3] = pid as u64;
                    processes[parent_index].wait = None;
                    processes[parent_index].state = State::Runnable;
                    // The child is consumed by this wait.
                    processes[from] = Process::EMPTY;
                    woke = true;
                }
            }
        }
    }

    if !woke {
        // An orphan has nobody to report to; keep the slot until the parent is
        // known dead, then reclaim it so the bounded table does not leak.
        let parent_alive = parent_pid != 0 && index_of_pid(parent_pid).is_some();
        if !parent_alive {
            // SAFETY: single-threaded kernel.
            unsafe {
                (*core::ptr::addr_of_mut!(PROCESSES))[from] = Process::EMPTY;
            }
        }
    }

    let Some(to) = next_runnable(from) else {
        // A blocked parent with no runnable child would otherwise abandon the
        // wait.  Wake the first blocked cell with ECHILD rather than leave it
        // parked forever; a later child exit cannot run without a scheduler.
        if let Some(index) = first_blocked() {
            // SAFETY: single-threaded kernel; the slot is live.
            unsafe {
                let processes = &mut *core::ptr::addr_of_mut!(PROCESSES);
                processes[index].frame.regs[2] = CELL_E_CHILD as u64;
                processes[index].frame.regs[3] = 0;
                processes[index].wait = None;
                processes[index].pending_wait = None;
                processes[index].state = State::Runnable;
            }
            switch_to(tf, from, index);
            return Exit::Switched;
        }
        // SAFETY: single-threaded kernel.
        unsafe {
            (*core::ptr::addr_of_mut!(PROCESSES))[from] = Process::EMPTY;
        }
        return Exit::Last;
    };
    switch_to(tf, from, to);
    Exit::Switched
}

fn first_blocked() -> Option<usize> {
    // SAFETY: single-threaded kernel.
    unsafe {
        (*core::ptr::addr_of!(PROCESSES))
            .iter()
            .position(|process| process.state == State::Blocked)
    }
}

fn next_runnable(from: usize) -> Option<usize> {
    // SAFETY: single-threaded kernel.
    unsafe {
        let processes = &*core::ptr::addr_of!(PROCESSES);
        for step in 1..=MAX_PROCESSES {
            let index = (from + step) % MAX_PROCESSES;
            if processes[index].state == State::Runnable {
                return Some(index);
            }
        }
    }
    None
}

fn switch_to(tf: &mut TrapFrame, from: usize, to: usize) {
    // SAFETY: single-threaded kernel; `from` and `to` are table indices and no
    // other process can observe the table between the save and the return.
    unsafe {
        let processes = &mut *core::ptr::addr_of_mut!(PROCESSES);
        if from != NO_PROCESS && matches!(processes[from].state, State::Running | State::Blocked) {
            processes[from].frame = *tf;
            processes[from].threads = thread::snapshot();
            processes[from].fds = fd::snapshot();
            if processes[from].state == State::Running {
                processes[from].state = State::Runnable;
            }
        }
        let target = &mut processes[to];
        target.state = State::Running;
        CURRENT = to;
        thread::install(target.threads);
        fd::restore(target.fds);
        target.space.activate(target.asid);
        if let Some(pending) = target.pending_wait.take() {
            if pending.status_address != 0 {
                // The parent's space is active again; a normal user-visible
                // store reaches the same cache lines the parent will read.
                core::ptr::write_volatile(pending.status_address as *mut u32, pending.status);
            }
            if pending.rusage != 0 {
                for offset in 0..RUSAGE_SIZE {
                    core::ptr::write_volatile(
                        (pending.rusage as *mut u8).add(offset as usize),
                        0u8,
                    );
                }
            }
            target.frame.regs[2] = native_abi::status::CELL_OK as u64;
            target.frame.regs[3] = pending.pid as u64;
        }
        *tf = target.frame;
        tf.entryhi = 0;
        traps::set_user_abi(target.abi);
    }
}

fn current_index() -> Option<usize> {
    // SAFETY: single-threaded kernel.
    let index = unsafe { CURRENT };
    if index == NO_PROCESS || index >= MAX_PROCESSES {
        None
    } else {
        Some(index)
    }
}

/// The current process's local pid, or 0 when no user process is running.
pub fn current_pid() -> u32 {
    current_index()
        .map(|index| unsafe { (*core::ptr::addr_of!(PROCESSES))[index].identity.pid })
        .unwrap_or(0)
}

/// The current process's local parent pid, or 0 when it has none or the
/// parent has exited.
pub fn current_ppid() -> u32 {
    let ppid = current_index()
        .map(|index| unsafe { (*core::ptr::addr_of!(PROCESSES))[index].ppid })
        .unwrap_or(0);
    if ppid != 0 && index_of_pid(ppid).is_some() {
        ppid
    } else {
        0
    }
}

/// The current process's global cell identity.
pub fn current_identity() -> Option<Identity> {
    current_index().map(|index| unsafe { (*core::ptr::addr_of!(PROCESSES))[index].identity })
}

/// The current process's ASID.
pub fn current_asid() -> u32 {
    current_index()
        .map(|index| unsafe { (*core::ptr::addr_of!(PROCESSES))[index].asid })
        .unwrap_or(0)
}

/// The current address space.
pub fn current_space() -> Option<&'static Space> {
    current_index().map(|index| {
        // SAFETY: the table entry is live and single-threaded code holds the
        // reference only for the duration of the syscall.
        unsafe { &(*core::ptr::addr_of!(PROCESSES))[index].space }
    })
}

/// The current address space, mutably.
pub fn current_space_mut() -> Option<&'static mut Space> {
    current_index().map(|index| {
        // SAFETY: as `current_space`; the kernel is single-threaded and this is
        // the only mutable reference in flight.
        unsafe { &mut (*core::ptr::addr_of_mut!(PROCESSES))[index].space }
    })
}

/// Run `body` with the current address space borrowed mutably.
pub fn with_current_space_mut<T>(body: impl FnOnce(&mut Space) -> T) -> Option<T> {
    current_space_mut().map(body)
}

/// Process table state for the diagnostic line printed after boot.
pub fn live_count() -> usize {
    // SAFETY: single-threaded kernel.
    unsafe {
        (*core::ptr::addr_of!(PROCESSES))
            .iter()
            .filter(|process| process.state != State::Unused)
            .count()
    }
}

fn print_dec(value: u32) {
    irix7_fmt::decimal(u64::from(value), &mut |byte| crate::write_byte(byte));
}

fn print_hex16(bytes: &[u8; 16]) {
    for byte in bytes {
        crate::print_hex8(*byte);
    }
}

const _: () = assert!(MAX_PROCESSES <= 8);
