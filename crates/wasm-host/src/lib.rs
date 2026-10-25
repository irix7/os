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
    /// Check a capability handle against an object.
    pub cap_check: fn(i64, i32, i32) -> i32,
}

fn zero_clock() -> u64 {
    0
}

fn discard_console(_bytes: &[u8]) {}

fn allow_capability(_handle: i64, _rights: i32, _object: i32) -> i32 {
    0
}

impl HostHooks {
    /// An inert hook set.
    pub const fn empty() -> Self {
        Self {
            clock_ms: zero_clock,
            console_write: discard_console,
            cap_check: allow_capability,
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
                (caller.data().hooks.cap_check)(handle, rights, object)
            },
        )?;
        linker.func_wrap(
            "i7host",
            "host_console_write",
            |mut caller: Caller<'_, HostState>, ptr: i32, len: i32| {
                let hooks = caller.data().hooks;
                let Some(Extern::Memory(memory)) = caller.get_export("memory") else {
                    return;
                };
                let len = len.max(0) as usize;
                if len == 0 || len > 256 {
                    return;
                }
                let mut buffer = [0u8; 256];
                if memory
                    .read(&caller, ptr.max(0) as usize, &mut buffer[..len])
                    .is_err()
                {
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
}
