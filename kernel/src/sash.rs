// SPDX-License-Identifier: GPL-3.0-only
//! The IRIX 7 standalone shell (sash).
//!
//! Sash runs in the kernel over the local keyboard/display and the telnet
//! console.  It deliberately has no userland or process dependency: the
//! recovery commands use the bring-up fd table, NVRAM, status block and the
//! boot request flags.  The command vocabulary follows SGI's standalone shell
//! (`help`, `ls`, `cat`, `pwd`, `cd`, `printenv`, `version`, `hinv`, `boot` and
//! `reboot`), but the implementation is original Rust.

use core::fmt::{self, Write};

use crate::fd::{self, Kind};

/// Where sash writes its output.
pub trait Sink {
    /// Write bytes to the console.
    fn write(&mut self, bytes: &[u8]);
}

struct Writer<'a> {
    out: &'a mut dyn Sink,
}

impl Write for Writer<'_> {
    fn write_str(&mut self, text: &str) -> fmt::Result {
        self.out.write(text.as_bytes());
        Ok(())
    }
}

/// Run one command.  Returns false when the command is not a sash command, so
/// the telnet console can fall through to its kernel-only commands.
pub fn execute(line: &[u8], out: &mut dyn Sink) -> bool {
    let line = trim(line);
    let (command, argument) = split(line);
    let mut lower = [0u8; 16];
    let command = if command.len() <= lower.len() {
        for (index, byte) in command.iter().enumerate() {
            lower[index] = byte.to_ascii_lowercase();
        }
        &lower[..command.len()]
    } else {
        command
    };
    match command {
        b"" => true,
        b"help" | b"?" => {
            help(out);
            true
        }
        b"version" => {
            out.write(b"IRIX 7 sash 0.1 (Rust, no userland)\r\n");
            true
        }
        b"pwd" => {
            let mut path = [0u8; 64];
            let len = fd::copy_cwd(&mut path);
            out.write(&path[..len]);
            out.write(b"\r\n");
            true
        }
        b"cd" => {
            if argument.is_empty() {
                out.write(b"cd: missing directory\r\n");
            } else if fd::set_cwd(argument) {
                // cd is silent on success, as in sash.
            } else {
                out.write(b"cd: no such directory\r\n");
            }
            true
        }
        b"ls" => {
            list(out, argument);
            true
        }
        b"cat" => {
            cat(out, argument);
            true
        }
        b"printenv" => {
            print_env(out, argument);
            true
        }
        b"echo" => {
            out.write(argument);
            out.write(b"\r\n");
            true
        }
        b"status" => {
            let mut buffer = [0u8; 512];
            let len = crate::status::text(&mut buffer);
            out.write(&buffer[..len]);
            true
        }
        b"net" => {
            let mut buffer = [0u8; 256];
            let len = crate::status::net(&mut buffer);
            out.write(&buffer[..len]);
            true
        }
        b"mem" => {
            let mut buffer = [0u8; 256];
            let len = crate::status::mem(&mut buffer);
            out.write(&buffer[..len]);
            true
        }
        b"time" => {
            let mut buffer = [0u8; 128];
            let len = crate::status::time(&mut buffer);
            out.write(&buffer[..len]);
            true
        }
        b"hinv" | b"inventory" => {
            // diagnostics prints through the global console, which mirrors
            // both the local screen and a connected telnet client.
            crate::diagnostics::run();
            true
        }
        b"boot" => {
            out.write(b"boot: retrying the target-2 user image\r\n");
            crate::telnet::request_boot();
            true
        }
        b"reboot" | b"reset" => {
            out.write(b"reboot: restarting from the PROM\r\n");
            crate::telnet::request_reboot();
            true
        }
        _ => false,
    }
}

fn list(out: &mut dyn Sink, path: &[u8]) {
    let path = if path.is_empty() {
        b".".as_slice()
    } else {
        path
    };
    let fd = fd::open_path(path, 0);
    if fd < 0 {
        out.write(b"ls: no such file or directory\r\n");
        return;
    }
    let mut writer = Writer { out };
    if matches!(fd::kind(fd), Kind::Dir(_)) {
        let mut name = [0u8; 256];
        let mut index = 0usize;
        while let Some((kind, _)) = fd::readdir(fd, index, &mut name) {
            let len = name.iter().position(|byte| *byte == 0).unwrap_or(0);
            if &name[..len] != b"." && &name[..len] != b".." {
                let _ = writer.write_str(core::str::from_utf8(&name[..len]).unwrap_or("?"));
                if kind == 4 {
                    let _ = writer.write_str("/");
                }
                let _ = writer.write_str("\r\n");
            }
            index += 1;
        }
    } else if let Some((mode, size)) = fd::metadata(fd) {
        let _ = writer.write_str("file mode=0o");
        let _ = write!(writer, "{:o} size={}\r\n", mode, size);
    }
    let _ = fd::close(fd);
}

fn cat(out: &mut dyn Sink, path: &[u8]) {
    if path.is_empty() {
        out.write(b"cat: missing file\r\n");
        return;
    }
    let fd = fd::open_path(path, 0);
    if fd < 0 {
        out.write(b"cat: no such file\r\n");
        return;
    }
    let mut buffer = [0u8; 512];
    loop {
        let count = fd::read(fd, &mut buffer);
        if count < 0 {
            out.write(b"cat: read error\r\n");
            break;
        }
        if count == 0 {
            break;
        }
        out.write(&buffer[..count as usize]);
    }
    let _ = fd::close(fd);
}

fn print_env(out: &mut dyn Sink, name: &[u8]) {
    if name.is_empty() {
        out.write(b"printenv: name required\r\n");
        return;
    }
    let Ok(name) = core::str::from_utf8(name) else {
        out.write(b"printenv: name is not UTF-8\r\n");
        return;
    };
    let mut value = [0u8; 256];
    match crate::nvram::get(name, &mut value) {
        Some(len) => {
            out.write(&value[..len]);
            out.write(b"\r\n");
        }
        None => out.write(b"printenv: variable not set\r\n"),
    }
}

fn help(out: &mut dyn Sink) {
    out.write(
        b"sash commands:\r\n\
          \x20 help                 this text\r\n\
          \x20 version              sash and kernel identity\r\n\
          \x20 pwd                  print the current directory\r\n\
          \x20 cd DIRECTORY         change directory\r\n\
          \x20 ls [DIRECTORY]       list the bring-up namespace\r\n\
          \x20 cat FILE             write a file to the console\r\n\
          \x20 printenv NAME        print a PROM environment value\r\n\
          \x20 echo TEXT            echo the argument\r\n\
          \x20 status               kernel and network state\r\n\
          \x20 net                  interface addresses\r\n\
          \x20 mem                  RAM accounting\r\n\
          \x20 time                 uptime and RTC time\r\n\
          \x20 hinv                 CPU, memory and network inventory\r\n\
          \x20 boot                 retry the target-2 user image\r\n\
          \x20 reboot               restart from the PROM\r\n\
          \r\n\
          kernel-only (telnet): diagnostics, scsi, http, https, ntp, ping,\r\n\
          module, user, userelf, panic, trap, tlb and gfx.\r\n",
    );
}

/// Remove leading and trailing spaces and tabs.
pub fn trim(mut line: &[u8]) -> &[u8] {
    while let Some((&byte, rest)) = line.split_first() {
        if byte == b' ' || byte == b'\t' {
            line = rest;
        } else {
            break;
        }
    }
    while let Some((&byte, rest)) = line.split_last() {
        if byte == b' ' || byte == b'\t' {
            line = rest;
        } else {
            break;
        }
    }
    line
}

fn split(line: &[u8]) -> (&[u8], &[u8]) {
    let Some(index) = line.iter().position(|byte| *byte == b' ' || *byte == b'\t') else {
        return (line, b"");
    };
    (trim(&line[..index]), trim(&line[index..]))
}
