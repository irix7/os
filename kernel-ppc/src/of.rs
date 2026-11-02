// SPDX-License-Identifier: GPL-3.0-only
//! Open Firmware client interface.
//!
//! The preloaded-kernel path does not pass a device tree pointer.  It passes
//! the client interface handler in r5.  The handler is called with r3
//! pointing at a `prom_args` block; returns are written after the arguments
//! in the same block.  The device tree is reached through `finddevice`,
//! `child` and `getprop`.

#[repr(C)]
struct PromArgs {
    service: u32,
    nargs: u32,
    nret: u32,
    args: [u32; 10],
}

extern "C" {
    fn of_call(handler: u32, args: *mut PromArgs) -> i32;
}

pub struct Client {
    handler: u32,
}

impl Client {
    pub fn new(handler: u32) -> Self {
        Self { handler }
    }

    pub fn finddevice(&self, path: &[u8]) -> Option<u32> {
        let mut args = [0u32; 10];
        args[0] = path.as_ptr() as u32;
        self.call(b"finddevice\0", 1, &mut args)?;
        nonzero(args[1])
    }

    pub fn child(&self, phandle: u32) -> Option<u32> {
        let mut args = [0u32; 10];
        args[0] = phandle;
        self.call(b"child\0", 1, &mut args)?;
        nonzero(args[1])
    }

    pub fn peer(&self, phandle: u32) -> Option<u32> {
        let mut args = [0u32; 10];
        args[0] = phandle;
        self.call(b"peer\0", 1, &mut args)?;
        nonzero(args[1])
    }

    pub fn getprop(&self, phandle: u32, name: &[u8], buffer: &mut [u8]) -> Option<usize> {
        let mut args = [0u32; 10];
        args[0] = phandle;
        args[1] = name.as_ptr() as u32;
        args[2] = buffer.as_mut_ptr() as u32;
        args[3] = buffer.len() as u32;
        self.call(b"getprop\0", 4, &mut args)?;
        let length = args[4];
        if length == u32::MAX {
            return None;
        }
        Some(length as usize)
    }

    pub fn getprop_u32(&self, phandle: u32, name: &[u8]) -> Option<u32> {
        let mut buffer = [0u8; 4];
        let length = self.getprop(phandle, name, &mut buffer)?;
        if length < 4 {
            return None;
        }
        Some(u32::from_be_bytes(buffer))
    }

    fn call(&self, service: &[u8], nargs: usize, args: &mut [u32; 10]) -> Option<()> {
        let mut block = PromArgs {
            service: service.as_ptr() as u32,
            nargs: nargs as u32,
            nret: 1,
            args: *args,
        };
        let status = unsafe { of_call(self.handler, &mut block) };
        if status != 0 {
            return None;
        }
        *args = block.args;
        Some(())
    }
}

fn nonzero(value: u32) -> Option<u32> {
    if value == 0 || value == u32::MAX {
        None
    } else {
        Some(value)
    }
}
