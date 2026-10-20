//! Scheduler — EDF and fair run queues.
//!
//! Two classes (D14):
//! - EDF (earliest deadline first) for media threads.
//! - Fair scheduling for everything else.
//!
//! One run queue per CPU. The Indy is uniprocessor.
//! No admission control in v1.

#![no_std]

use core::cell::Cell;

use crate::print_hex8;

/// Scheduling class.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SchedClass {
    /// Earliest deadline first — for media threads.
    Edf,
    /// Fair scheduling — for everything else.
    Fair,
}

/// Task state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TaskState {
    Running,
    Runnable,
    Blocked,
}

/// A schedulable task.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct Task {
    /// Task state.
    state: TaskState,
    /// Scheduling class.
    class: SchedClass,
    /// Absolute deadline (EDF) or virtual runtime (fair).
    key: u64,
    /// Saved register context (placeholder).
    _ctx: [u32; 32],
    /// Pointer to the owning cell.
    _cell: usize,
}

/// EDF run queue: red-black tree keyed by absolute deadline.
struct EdfQueue {
    head: Option<usize>,
    nodes: [EdfNode; 64],
    count: usize,
}

#[derive(Clone, Copy, Debug)]
struct EdfNode {
    task_idx: usize,
    key: u64,
    left: Option<usize>,
    right: Option<usize>,
    parent: Option<usize>,
}

/// Fair run queue: weighted fair queue keyed by virtual runtime.
struct FairQueue {
    head: Option<usize>,
    nodes: [FairNode; 64],
    count: usize,
}

#[derive(Clone, Copy, Debug)]
struct FairNode {
    task_idx: usize,
    key: u64,
    weight: u32,
    left: Option<usize>,
    right: Option<usize>,
    parent: Option<usize>,
}

/// The scheduler.
pub struct Scheduler {
    edf: EdfQueue,
    fair: FairQueue,
    current: Option<usize>,
    tasks: [Option<Task>; 64],
    task_count: usize,
}

static mut SCHEDULER: Option<Scheduler> = None;

/// Initialise the scheduler.
pub fn init() {
    unsafe {
        SCHEDULER = Some(Scheduler {
            edf: EdfQueue {
                head: None,
                nodes: [EdfNode {
                    task_idx: 0,
                    key: 0,
                    left: None,
                    right: None,
                    parent: None,
                }; 64],
                count: 0,
            },
            fair: FairQueue {
                head: None,
                nodes: [FairNode {
                    task_idx: 0,
                    key: 0,
                    weight: 1,
                    left: None,
                    right: None,
                    parent: None,
                }; 64],
                count: 0,
            },
            current: None,
            tasks: [None; 64],
            task_count: 0,
        });
    }

    crate::println("scheduler: initialised (EDF + fair)");
}

/// Create a new task.
pub fn new_task(class: SchedClass, deadline: u64) -> Option<usize> {
    unsafe {
        let sched = SCHEDULER.as_mut().expect("scheduler not initialised");
        if sched.task_count >= 64 {
            return None;
        }
        let idx = sched.task_count;
        sched.task_count += 1;
        sched.tasks[idx] = Some(Task {
            state: TaskState::Runnable,
            class,
            key: deadline,
            _ctx: [0; 32],
            _cell: 0,
        });
        match class {
            SchedClass::Edf => {
                sched.edf.nodes[sched.edf.count] = EdfNode {
                    task_idx: idx,
                    key: deadline,
                    left: None,
                    right: None,
                    parent: None,
                };
                sched.edf.count += 1;
            }
            SchedClass::Fair => {
                sched.fair.nodes[sched.fair.count] = FairNode {
                    task_idx: idx,
                    key: 0,
                    weight: 1,
                    left: None,
                    right: None,
                    parent: None,
                };
                sched.fair.count += 1;
            }
        }
        Some(idx)
    }
}

/// Select the next task to run (highest priority / earliest deadline).
fn pick_next(sched: &mut Scheduler) -> Option<usize> {
    // EDF first if any EDF tasks are runnable.
    if sched.edf.count > 0 {
        // Find the task with the earliest deadline.
        let mut best_idx = 0;
        let mut best_key = u64::MAX;
        for i in 0..sched.edf.count {
            let node = sched.edf.nodes[i];
            if node.key < best_key {
                best_key = node.key;
                best_idx = node.task_idx;
            }
        }
        return Some(best_idx);
    }
    // Otherwise fair queue.
    if sched.fair.count > 0 {
        let mut best_idx = 0;
        let mut best_key = u64::MAX;
        for i in 0..sched.fair.count {
            let node = sched.fair.nodes[i];
            if node.key < best_key {
                best_key = node.key;
                best_idx = node.task_idx;
            }
        }
        return Some(best_idx);
    }
    None
}

/// Yield the CPU — pick the next task.
pub fn yield_cpu() {
    unsafe {
        let sched = SCHEDULER.as_mut().expect("scheduler not initialised");
        let next = pick_next(sched);
        if next.is_some() && next != sched.current {
            sched.current = next;
            if let Some(task) = sched.tasks[next.unwrap()].as_mut() {
                task.state = TaskState::Running;
            }
        }
    }
}

/// Block the current task.
pub fn block_current() {
    unsafe {
        let sched = SCHEDULER.as_mut().expect("scheduler not initialised");
        if let Some(idx) = sched.current {
            if let Some(task) = sched.tasks[idx].as_mut() {
                task.state = TaskState::Blocked;
            }
            sched.current = None;
        }
    }
}

/// Unblock a task.
pub fn unblock_task(idx: usize) {
    unsafe {
        let sched = SCHEDULER.as_mut().expect("scheduler not initialised");
        if let Some(task) = sched.tasks[idx].as_mut() {
            task.state = TaskState::Runnable;
        }
    }
}

/// Get the current task index.
pub fn current_task() -> Option<usize> {
    unsafe { SCHEDULER.as_ref().unwrap().current }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn task_creation_and_scheduling() {
        init();
        let t1 = new_task(SchedClass::Fair, 0).expect("task 1");
        let t2 = new_task(SchedClass::Edf, 100).expect("task 2");
        let t3 = new_task(SchedClass::Edf, 50).expect("task 3");

        // EDF task with earliest deadline should be picked first.
        let sched = unsafe { SCHEDULER.as_mut().unwrap() };
        let next = pick_next(sched);
        assert_eq!(next, Some(t3));
    }
}
