// SPDX-License-Identifier: GPL-3.0-only
//! Host-side wasmi lifecycle for the IRIX 7 Wasm core.
//!
//! This is a workstation prototype for the host ABI, not the kernel host. It
//! configures deterministic fuel metering, installs the `i7host` functions and
//! runs a small module. The kernel implementation will keep the same guest
//! surface but use the native shim for time, memory, capabilities and devices.

#![forbid(unsafe_code)]
#![deny(missing_docs)]

use wasmi::{Caller, Config, Engine, Linker, Module, Store};

/// The current host ABI version.
pub const HOST_ABI_VERSION: i32 = 1;

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
}

impl Default for HostState {
    fn default() -> Self {
        Self {
            next_event: 1,
            submissions: 0,
            console_writes: 0,
            exit_status: None,
        }
    }
}

impl HostState {
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
}

impl Host {
    /// Build an engine with fuel metering and the host functions.
    pub fn new() -> Result<Self, wasmi::Error> {
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
            |caller: Caller<'_, HostState>| -> i64 {
                let _ = caller;
                0
            },
        )?;
        linker.func_wrap(
            "i7host",
            "host_submit",
            |mut caller: Caller<'_, HostState>,
             _op: i32,
             _a0: i64,
             _a1: i64,
             _a2: i64,
             _a3: i64|
             -> i64 {
                let event = caller.data_mut().next_event();
                caller.data_mut().submissions += 1;
                event as i64
            },
        )?;
        linker.func_wrap(
            "i7host",
            "host_poll",
            |mut caller: Caller<'_, HostState>, _out: i32, _max: i32| -> i32 {
                let _ = caller.data_mut();
                0
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
            |caller: Caller<'_, HostState>, _handle: i64, _rights: i32, _object: i32| -> i32 {
                let _ = caller;
                0
            },
        )?;
        linker.func_wrap(
            "i7host",
            "host_console_write",
            |mut caller: Caller<'_, HostState>, _ptr: i32, _len: i32| {
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

        Ok(Self { engine, linker })
    }

    /// Instantiate a module, start it and call a no-argument `i32` function.
    pub fn run_i32(&self, wasm: &[u8], function: &str, fuel: u64) -> Result<i32, wasmi::Error> {
        let module = Module::new(&self.engine, wasm)?;
        let mut store = Store::new(&self.engine, HostState::default());
        store.set_fuel(fuel)?;
        let instance = self.linker.instantiate_and_start(&mut store, &module)?;
        let function = instance.get_typed_func::<(), i32>(&store, function)?;
        function.call(&mut store, ())
    }

    /// Instantiate a module, start it and call a no-argument `i64` function.
    pub fn run_i64(&self, wasm: &[u8], function: &str, fuel: u64) -> Result<i64, wasmi::Error> {
        let module = Module::new(&self.engine, wasm)?;
        let mut store = Store::new(&self.engine, HostState::default());
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
}
