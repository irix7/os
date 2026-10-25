// SPDX-License-Identifier: GPL-3.0-only
//! Host-side wasmi lifecycle for the IRIX 7 Wasm core.
//!
//! This is a workstation prototype for the host ABI, not the kernel host. It
//! configures deterministic fuel metering, installs the `i7host` functions and
//! runs a small module. The kernel implementation keeps the same guest surface
//! and supplies the native shim through [`HostHooks`].

#![cfg_attr(not(test), no_std)]
#![forbid(unsafe_code)]
#![deny(missing_docs)]

extern crate alloc;

use alloc::collections::VecDeque;
use sys_primitives::{Completion, CompletionId, CompletionKind, Status, Tag};
use wasmi::{Caller, Config, Engine, Extern, Linker, Module, Store};

/// The current host ABI version.
pub const HOST_ABI_VERSION: i32 = 1;

/// Callbacks from the host ABI into the native shim.
///
/// The default is deliberately inert: it is suitable for the workstation
/// tests. The kernel module installs functions that reach the service table.
#[derive(Clone, Copy, Debug)]
pub struct HostHooks {
    /// Monotonic clock in milliseconds.
    pub clock_ms: fn() -> u64,
    /// Write guest console bytes to the native console.
    pub console_write: fn(&[u8]),
    /// The local-policy primitives.
    pub policy: PolicyHooks,
}

/// Issue a capability and return its opaque handle, or zero.
pub type CapIssueHook = fn(usize, &[u8; 32], u32, &[u8; 32], &[u8; 16]) -> i64;

/// Native primitives behind the guest's local-policy decision.
///
/// The guest owns the policy: it chooses paths, object contents, actor
/// identity and capability rights, and checks every result. These callbacks
/// only supply bulk state and the operations Wasm cannot perform directly.
/// All pointers in `context` are opaque to this crate; the caller owns the
/// lifetime and validity of the value behind it.
#[derive(Clone, Copy, Debug)]
pub struct PolicyHooks {
    /// Opaque caller context, passed back unchanged.
    pub context: usize,
    /// Write this machine's 16-byte node identity.
    pub node_id: fn(usize, &mut [u8; 16]) -> i32,
    /// Start a fresh staged policy.
    pub begin: fn(usize) -> i32,
    /// Discard the stage and any published policy.
    pub abort: fn(usize),
    /// Publish the staged policy atomically.
    pub commit: fn(usize) -> i32,
    /// Create a directory and write its root OID.
    pub mkdir: fn(usize, &str, &mut [u8; 32]) -> i32,
    /// Store bytes at a path and write the object OID.
    pub put: fn(usize, &str, u8, &[u8], &mut [u8; 32]) -> i32,
    /// Resolve a path and write the object OID.
    pub lookup: fn(usize, &str, &mut [u8; 32]) -> i32,
    /// Fetch an object by OID.  Returns its length, or a negative status.
    pub get: fn(usize, &[u8; 32], &mut i32, &mut [u8; 512]) -> i32,
    /// Write the current root directory OID.
    pub root: fn(usize, &mut [u8; 32]) -> i32,
    /// Resolve an object's home and write the node identity.
    pub home: fn(usize, &[u8; 32], &mut [u8; 16]) -> i32,
    /// Spawn an actor at its rendezvous home.
    pub actor_spawn: fn(usize, &[u8; 16]) -> i32,
    /// Place an actor on a host.
    pub actor_place: fn(usize, &[u8; 16], &[u8; 16]) -> i32,
    /// Put an actor on the run queue.
    pub actor_queue: fn(usize, &[u8; 16]) -> i32,
    /// Issue a capability and return its opaque handle, or zero.
    pub cap_issue: CapIssueHook,
    /// Check a capability handle for an object and rights set.
    pub cap_check: fn(usize, u64, u32, &[u8; 32]) -> i32,
    /// Write `[objects, actors, queued, caps]`.
    pub stats: fn(usize, &mut [u32; 4]) -> i32,
}

fn zero_clock() -> u64 {
    0
}

fn discard_console(_bytes: &[u8]) {}

fn unavailable_node(_context: usize, _out: &mut [u8; 16]) -> i32 {
    -1
}

fn unavailable_begin(_context: usize) -> i32 {
    -1
}

fn unavailable_abort(_context: usize) {}

fn unavailable_commit(_context: usize) -> i32 {
    -1
}

fn unavailable_mkdir(_context: usize, _path: &str, _out: &mut [u8; 32]) -> i32 {
    -1
}

fn unavailable_put(
    _context: usize,
    _path: &str,
    _kind: u8,
    _bytes: &[u8],
    _out: &mut [u8; 32],
) -> i32 {
    -1
}

fn unavailable_lookup(_context: usize, _path: &str, _out: &mut [u8; 32]) -> i32 {
    -1
}

fn unavailable_get(_context: usize, _oid: &[u8; 32], _kind: &mut i32, _out: &mut [u8; 512]) -> i32 {
    -1
}

fn unavailable_root(_context: usize, _out: &mut [u8; 32]) -> i32 {
    -1
}

fn unavailable_home(_context: usize, _oid: &[u8; 32], _out: &mut [u8; 16]) -> i32 {
    -1
}

fn unavailable_actor_spawn(_context: usize, _id: &[u8; 16]) -> i32 {
    -1
}

fn unavailable_actor_place(_context: usize, _id: &[u8; 16], _host: &[u8; 16]) -> i32 {
    -1
}

fn unavailable_actor_queue(_context: usize, _id: &[u8; 16]) -> i32 {
    -1
}

fn unavailable_cap_issue(
    _context: usize,
    _object: &[u8; 32],
    _rights: u32,
    _subject: &[u8; 32],
    _home: &[u8; 16],
) -> i64 {
    0
}

fn unavailable_cap_check(_context: usize, _handle: u64, _rights: u32, _object: &[u8; 32]) -> i32 {
    -1
}

fn unavailable_stats(_context: usize, _out: &mut [u32; 4]) -> i32 {
    -1
}

impl PolicyHooks {
    /// An inert policy surface.
    pub const fn empty() -> Self {
        Self {
            context: 0,
            node_id: unavailable_node,
            begin: unavailable_begin,
            abort: unavailable_abort,
            commit: unavailable_commit,
            mkdir: unavailable_mkdir,
            put: unavailable_put,
            lookup: unavailable_lookup,
            get: unavailable_get,
            root: unavailable_root,
            home: unavailable_home,
            actor_spawn: unavailable_actor_spawn,
            actor_place: unavailable_actor_place,
            actor_queue: unavailable_actor_queue,
            cap_issue: unavailable_cap_issue,
            cap_check: unavailable_cap_check,
            stats: unavailable_stats,
        }
    }
}

impl HostHooks {
    /// An inert hook set.
    pub const fn empty() -> Self {
        Self {
            clock_ms: zero_clock,
            console_write: discard_console,
            policy: PolicyHooks::empty(),
        }
    }
}

/// State shared by all host functions in this store.
#[derive(Debug)]
pub struct HostState {
    /// Last submission token.
    pub next_event: u64,
    /// Number of host submissions observed.
    pub submissions: u64,
    /// Number of console writes observed.
    pub console_writes: u64,
    /// Exit status requested by the guest, if any.
    pub exit_status: Option<i32>,
    /// Completions waiting for the guest to poll.
    pub completions: VecDeque<Completion>,
    /// Native shim callbacks.
    pub hooks: HostHooks,
}

impl Default for HostState {
    fn default() -> Self {
        Self::new(HostHooks::empty())
    }
}

impl HostState {
    fn new(hooks: HostHooks) -> Self {
        Self {
            next_event: 1,
            submissions: 0,
            console_writes: 0,
            exit_status: None,
            completions: VecDeque::new(),
            hooks,
        }
    }

    fn next_event(&mut self) -> u64 {
        let event = self.next_event;
        self.next_event = self.next_event.wrapping_add(1).max(1);
        event
    }
}

/// A deterministic wasmi engine with the `i7host` import surface installed.
pub struct Host {
    engine: Engine,
    linker: Linker<HostState>,
    hooks: HostHooks,
}

fn read_memory(caller: &Caller<'_, HostState>, ptr: i32, len: usize, out: &mut [u8]) -> bool {
    if len > out.len() || ptr < 0 {
        return false;
    }
    let Some(Extern::Memory(memory)) = caller.get_export("memory") else {
        return false;
    };
    memory.read(caller, ptr as usize, &mut out[..len]).is_ok()
}

fn write_memory(caller: &mut Caller<'_, HostState>, ptr: i32, bytes: &[u8]) -> bool {
    if ptr < 0 {
        return false;
    }
    let Some(Extern::Memory(memory)) = caller.get_export("memory") else {
        return false;
    };
    memory.write(caller, ptr as usize, bytes).is_ok()
}

impl Host {
    /// Build an engine with fuel metering and the host functions.
    pub fn new() -> Result<Self, wasmi::Error> {
        Self::with_hooks(HostHooks::empty())
    }

    /// Build an engine with native shim callbacks.
    pub fn with_hooks(hooks: HostHooks) -> Result<Self, wasmi::Error> {
        let mut config = Config::default();
        config.consume_fuel(true);
        let engine = Engine::new(&config);
        let mut linker = Linker::new(&engine);

        linker.func_wrap(
            "i7host",
            "host_version",
            |caller: Caller<'_, HostState>| -> i32 {
                let _ = caller;
                HOST_ABI_VERSION
            },
        )?;
        linker.func_wrap(
            "i7host",
            "host_now",
            |caller: Caller<'_, HostState>| -> i64 { (caller.data().hooks.clock_ms)() as i64 },
        )?;
        linker.func_wrap(
            "i7host",
            "host_submit",
            |mut caller: Caller<'_, HostState>,
             op: i32,
             a0: i64,
             _a1: i64,
             _a2: i64,
             _a3: i64|
             -> i64 {
                let state = caller.data_mut();
                let event = state.next_event();
                state.submissions += 1;
                state.completions.push_back(Completion {
                    event: CompletionId(event),
                    tag: Tag(0),
                    status: Status::Ok,
                    kind: CompletionKind::None,
                    flags: 0,
                    reserved: 0,
                    value0: op as u32,
                    value1: a0 as u64,
                });
                event as i64
            },
        )?;
        linker.func_wrap(
            "i7host",
            "host_poll",
            |mut caller: Caller<'_, HostState>, out: i32, max: i32| -> i32 {
                let Some(Extern::Memory(memory)) = caller.get_export("memory") else {
                    return 0;
                };
                let limit = max.max(0) as usize;
                let mut written = 0usize;
                while written < limit {
                    let Some(completion) = caller.data_mut().completions.pop_front() else {
                        break;
                    };
                    let Some(offset) = (out as usize).checked_add(written * Completion::LEN) else {
                        break;
                    };
                    if memory
                        .write(&mut caller, offset, &completion.encode())
                        .is_err()
                    {
                        break;
                    }
                    written += 1;
                }
                written as i32
            },
        )?;
        linker.func_wrap(
            "i7host",
            "host_cancel",
            |caller: Caller<'_, HostState>, _token: i64| -> i32 {
                let _ = caller;
                0
            },
        )?;
        linker.func_wrap(
            "i7host",
            "host_cap_check",
            |caller: Caller<'_, HostState>, handle: i64, rights: i32, object: i32| -> i32 {
                let hooks = caller.data().hooks.policy;
                let mut oid = [0u8; 32];
                if !read_memory(&caller, object, 32, &mut oid) {
                    return -1;
                }
                (hooks.cap_check)(hooks.context, handle as u64, rights as u32, &oid)
            },
        )?;
        linker.func_wrap(
            "i7host",
            "host_node_id",
            |mut caller: Caller<'_, HostState>, out: i32| -> i32 {
                let hooks = caller.data().hooks.policy;
                let mut node = [0u8; 16];
                let status = (hooks.node_id)(hooks.context, &mut node);
                if status != 0 {
                    return status;
                }
                if write_memory(&mut caller, out, &node) {
                    0
                } else {
                    -1
                }
            },
        )?;
        linker.func_wrap(
            "i7host",
            "host_policy_begin",
            |caller: Caller<'_, HostState>| -> i32 {
                let hooks = caller.data().hooks.policy;
                (hooks.begin)(hooks.context)
            },
        )?;
        linker.func_wrap(
            "i7host",
            "host_policy_abort",
            |caller: Caller<'_, HostState>| {
                let hooks = caller.data().hooks.policy;
                (hooks.abort)(hooks.context);
            },
        )?;
        linker.func_wrap(
            "i7host",
            "host_policy_commit",
            |caller: Caller<'_, HostState>| -> i32 {
                let hooks = caller.data().hooks.policy;
                (hooks.commit)(hooks.context)
            },
        )?;
        linker.func_wrap(
            "i7host",
            "host_policy_stats",
            |mut caller: Caller<'_, HostState>, out: i32| -> i32 {
                let hooks = caller.data().hooks.policy;
                let mut stats = [0u32; 4];
                let status = (hooks.stats)(hooks.context, &mut stats);
                if status != 0 {
                    return status;
                }
                let mut bytes = [0u8; 16];
                for (index, word) in stats.iter().enumerate() {
                    bytes[index * 4..index * 4 + 4].copy_from_slice(&word.to_le_bytes());
                }
                if write_memory(&mut caller, out, &bytes) {
                    0
                } else {
                    -1
                }
            },
        )?;
        linker.func_wrap(
            "i7host",
            "host_ns_mkdir",
            |mut caller: Caller<'_, HostState>, path: i32, len: i32, out: i32| -> i32 {
                let hooks = caller.data().hooks.policy;
                let len = len.max(0) as usize;
                if len == 0 || len > 160 {
                    return -1;
                }
                let mut buffer = [0u8; 160];
                if !read_memory(&caller, path, len, &mut buffer) {
                    return -1;
                }
                let Ok(path) = core::str::from_utf8(&buffer[..len]) else {
                    return -1;
                };
                let mut oid = [0u8; 32];
                let status = (hooks.mkdir)(hooks.context, path, &mut oid);
                if status != 0 {
                    return status;
                }
                if write_memory(&mut caller, out, &oid) {
                    0
                } else {
                    -1
                }
            },
        )?;
        linker.func_wrap(
            "i7host",
            "host_ns_put",
            |mut caller: Caller<'_, HostState>,
             path: i32,
             path_len: i32,
             kind: i32,
             data: i32,
             data_len: i32,
             out: i32|
             -> i32 {
                let hooks = caller.data().hooks.policy;
                let path_len = path_len.max(0) as usize;
                let data_len = data_len.max(0) as usize;
                if path_len == 0 || path_len > 160 || data_len > 512 {
                    return -1;
                }
                if !(1..=9).contains(&kind) {
                    return -1;
                }
                let mut path_buffer = [0u8; 160];
                let mut data_buffer = [0u8; 512];
                if !read_memory(&caller, path, path_len, &mut path_buffer)
                    || !read_memory(&caller, data, data_len, &mut data_buffer)
                {
                    return -1;
                }
                let Ok(path) = core::str::from_utf8(&path_buffer[..path_len]) else {
                    return -1;
                };
                let mut oid = [0u8; 32];
                let status = (hooks.put)(
                    hooks.context,
                    path,
                    kind as u8,
                    &data_buffer[..data_len],
                    &mut oid,
                );
                if status != 0 {
                    return status;
                }
                if write_memory(&mut caller, out, &oid) {
                    0
                } else {
                    -1
                }
            },
        )?;
        linker.func_wrap(
            "i7host",
            "host_ns_lookup",
            |mut caller: Caller<'_, HostState>, path: i32, len: i32, out: i32| -> i32 {
                let hooks = caller.data().hooks.policy;
                let len = len.max(0) as usize;
                if len == 0 || len > 160 {
                    return -1;
                }
                let mut buffer = [0u8; 160];
                if !read_memory(&caller, path, len, &mut buffer) {
                    return -1;
                }
                let Ok(path) = core::str::from_utf8(&buffer[..len]) else {
                    return -1;
                };
                let mut oid = [0u8; 32];
                let status = (hooks.lookup)(hooks.context, path, &mut oid);
                if status != 0 {
                    return status;
                }
                if write_memory(&mut caller, out, &oid) {
                    0
                } else {
                    -1
                }
            },
        )?;
        linker.func_wrap(
            "i7host",
            "host_ns_get",
            |mut caller: Caller<'_, HostState>,
             oid: i32,
             kind_out: i32,
             out: i32,
             capacity: i32|
             -> i32 {
                let hooks = caller.data().hooks.policy;
                let capacity = capacity.max(0) as usize;
                if capacity > 512 {
                    return -1;
                }
                let mut object = [0u8; 32];
                if !read_memory(&caller, oid, 32, &mut object) {
                    return -1;
                }
                let mut kind = 0i32;
                let mut buffer = [0u8; 512];
                let length = (hooks.get)(hooks.context, &object, &mut kind, &mut buffer);
                if length < 0 || length as usize > capacity {
                    return -1;
                }
                if !write_memory(&mut caller, kind_out, &kind.to_le_bytes())
                    || !write_memory(&mut caller, out, &buffer[..length as usize])
                {
                    return -1;
                }
                length
            },
        )?;
        linker.func_wrap(
            "i7host",
            "host_ns_root",
            |mut caller: Caller<'_, HostState>, out: i32| -> i32 {
                let hooks = caller.data().hooks.policy;
                let mut oid = [0u8; 32];
                let status = (hooks.root)(hooks.context, &mut oid);
                if status != 0 {
                    return status;
                }
                if write_memory(&mut caller, out, &oid) {
                    0
                } else {
                    -1
                }
            },
        )?;
        linker.func_wrap(
            "i7host",
            "host_home",
            |mut caller: Caller<'_, HostState>, oid: i32, out: i32| -> i32 {
                let hooks = caller.data().hooks.policy;
                let mut object = [0u8; 32];
                if !read_memory(&caller, oid, 32, &mut object) {
                    return -1;
                }
                let mut node = [0u8; 16];
                let status = (hooks.home)(hooks.context, &object, &mut node);
                if status != 0 {
                    return status;
                }
                if write_memory(&mut caller, out, &node) {
                    0
                } else {
                    -1
                }
            },
        )?;
        linker.func_wrap(
            "i7host",
            "host_actor_spawn",
            |caller: Caller<'_, HostState>, id: i32| -> i32 {
                let hooks = caller.data().hooks.policy;
                let mut actor = [0u8; 16];
                if !read_memory(&caller, id, 16, &mut actor) {
                    return -1;
                }
                (hooks.actor_spawn)(hooks.context, &actor)
            },
        )?;
        linker.func_wrap(
            "i7host",
            "host_actor_place",
            |caller: Caller<'_, HostState>, id: i32, host: i32| -> i32 {
                let hooks = caller.data().hooks.policy;
                let mut actor = [0u8; 16];
                let mut node = [0u8; 16];
                if !read_memory(&caller, id, 16, &mut actor)
                    || !read_memory(&caller, host, 16, &mut node)
                {
                    return -1;
                }
                (hooks.actor_place)(hooks.context, &actor, &node)
            },
        )?;
        linker.func_wrap(
            "i7host",
            "host_actor_queue",
            |caller: Caller<'_, HostState>, id: i32| -> i32 {
                let hooks = caller.data().hooks.policy;
                let mut actor = [0u8; 16];
                if !read_memory(&caller, id, 16, &mut actor) {
                    return -1;
                }
                (hooks.actor_queue)(hooks.context, &actor)
            },
        )?;
        linker.func_wrap(
            "i7host",
            "host_cap_issue",
            |caller: Caller<'_, HostState>,
             object: i32,
             rights: i32,
             subject: i32,
             home: i32|
             -> i64 {
                let hooks = caller.data().hooks.policy;
                let mut oid = [0u8; 32];
                let mut principal = [0u8; 32];
                let mut node = [0u8; 16];
                if !read_memory(&caller, object, 32, &mut oid)
                    || !read_memory(&caller, subject, 32, &mut principal)
                    || !read_memory(&caller, home, 16, &mut node)
                {
                    return 0;
                }
                (hooks.cap_issue)(hooks.context, &oid, rights as u32, &principal, &node)
            },
        )?;
        linker.func_wrap(
            "i7host",
            "host_console_write",
            |mut caller: Caller<'_, HostState>, ptr: i32, len: i32| {
                let hooks = caller.data().hooks;
                let len = len.max(0) as usize;
                if len == 0 || len > 256 {
                    return;
                }
                let mut buffer = [0u8; 256];
                if !read_memory(&caller, ptr, len, &mut buffer) {
                    return;
                }
                (hooks.console_write)(&buffer[..len]);
                caller.data_mut().console_writes += 1;
            },
        )?;
        linker.func_wrap(
            "i7host",
            "host_exit",
            |mut caller: Caller<'_, HostState>, status: i32| {
                caller.data_mut().exit_status = Some(status);
            },
        )?;
        linker.func_wrap("i7host", "host_yield", |caller: Caller<'_, HostState>| {
            let _ = caller;
        })?;

        Ok(Self {
            engine,
            linker,
            hooks,
        })
    }

    /// Instantiate a module, start it and call a no-argument `i32` function.
    pub fn run_i32(&self, wasm: &[u8], function: &str, fuel: u64) -> Result<i32, wasmi::Error> {
        let module = Module::new(&self.engine, wasm)?;
        let mut store = Store::new(&self.engine, HostState::new(self.hooks));
        store.set_fuel(fuel)?;
        let instance = self.linker.instantiate_and_start(&mut store, &module)?;
        let function = instance.get_typed_func::<(), i32>(&store, function)?;
        function.call(&mut store, ())
    }

    /// Instantiate a module, start it and call a no-argument `i64` function.
    pub fn run_i64(&self, wasm: &[u8], function: &str, fuel: u64) -> Result<i64, wasmi::Error> {
        let module = Module::new(&self.engine, wasm)?;
        let mut store = Store::new(&self.engine, HostState::new(self.hooks));
        store.set_fuel(fuel)?;
        let instance = self.linker.instantiate_and_start(&mut store, &module)?;
        let function = instance.get_typed_func::<(), i64>(&store, function)?;
        function.call(&mut store, ())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const VERSION_WAT: &str = r#"
        (module
          (import "i7host" "host_version" (func $version (result i32)))
          (func (export "run") (result i32)
            call $version))
    "#;

    const SUBMIT_WAT: &str = r#"
        (module
          (import "i7host" "host_submit"
            (func $submit (param i32 i64 i64 i64 i64) (result i64)))
          (func (export "run") (result i64)
            i32.const 1
            i64.const 0
            i64.const 0
            i64.const 0
            i64.const 0
            call $submit))
    "#;

    #[test]
    fn host_version_is_available() {
        let host = Host::new().expect("host");
        assert_eq!(
            host.run_i32(VERSION_WAT.as_bytes(), "run", 1_000).unwrap(),
            1
        );
    }

    #[test]
    fn submissions_return_tokens() {
        let host = Host::new().expect("host");
        assert_eq!(
            host.run_i64(SUBMIT_WAT.as_bytes(), "run", 1_000).unwrap(),
            1
        );
    }

    #[test]
    fn fuel_exhaustion_is_an_error() {
        let wasm = br#"
            (module
              (func (export "run") (result i32)
                (loop $l (br $l))
                i32.const 0))
        "#;
        let host = Host::new().expect("host");
        assert!(host.run_i32(wasm, "run", 100).is_err());
    }
    const POLL_WAT: &str = r#"
        (module
          (import "i7host" "host_submit"
            (func $submit (param i32 i64 i64 i64 i64) (result i64)))
          (import "i7host" "host_poll"
            (func $poll (param i32 i32) (result i32)))
          (memory (export "memory") 1)
          (func (export "run") (result i32)
            i32.const 1
            i64.const 0
            i64.const 0
            i64.const 0
            i64.const 0
            call $submit
            drop
            i32.const 0
            i32.const 1
            call $poll))
    "#;

    #[test]
    fn poll_delivers_a_completion_to_guest_memory() {
        let host = Host::new().expect("host");
        assert_eq!(host.run_i32(POLL_WAT.as_bytes(), "run", 10_000).unwrap(), 1);
    }

    #[test]
    fn policy_guest_imports_only_the_host_surface() {
        let engine = Engine::default();
        let module = Module::new(
            &engine,
            include_bytes!("../../../modules/wasm/src/guest.wasm"),
        )
        .expect("guest module");
        let imports: Vec<_> = module.imports().collect();
        assert!(!imports.is_empty());
        assert!(
            imports.iter().all(|import| import.module() == "i7host"),
            "guest has an ambient import: {:?}",
            imports
                .iter()
                .map(|import| format!("{}.{}", import.module(), import.name()))
                .collect::<Vec<_>>()
        );
    }
}
