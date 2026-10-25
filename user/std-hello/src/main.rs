// SPDX-License-Identifier: GPL-3.0-only
//! First `std` image for the IRIX 7 native target.

use irix7c as _;

fn main() {
    let mut failed = false;
    println!("hello from std");
    for arg in std::env::args() {
        println!("arg={arg}");
    }
    let sleep_start = std::time::Instant::now();
    std::thread::sleep(std::time::Duration::from_millis(10));
    println!("slept_ms={}", sleep_start.elapsed().as_millis());
    let now = std::time::SystemTime::now();
    if let Ok(since) = now.duration_since(std::time::UNIX_EPOCH) {
        println!("time={}", since.as_secs());
    }
    match std::fs::read("/hello.txt") {
        Ok(data) => println!("file={}", String::from_utf8_lossy(&data).trim_end()),
        Err(error) => {
            failed = true;
            println!("fs error: {error}");
        }
    }
    match std::fs::write("/hello.txt", b"rewritten by std\n") {
        Ok(()) => println!("write=ok"),
        Err(error) => {
            failed = true;
            println!("write error: {error}");
        }
    }
    match std::fs::read("/hello.txt") {
        Ok(data) => println!("after_write={}", String::from_utf8_lossy(&data).trim_end()),
        Err(error) => {
            failed = true;
            println!("read after write error: {error}");
        }
    }
    match std::env::set_current_dir("/etc") {
        Ok(()) => println!("chdir=ok"),
        Err(error) => {
            failed = true;
            println!("chdir error: {error}");
        }
    }
    match std::env::current_dir() {
        Ok(path) => println!("cwd={}", path.display()),
        Err(error) => {
            failed = true;
            println!("cwd error: {error}");
        }
    }
    match std::fs::read("motd") {
        Ok(data) => println!("relative={}", String::from_utf8_lossy(&data).trim_end()),
        Err(error) => {
            failed = true;
            println!("relative read error: {error}");
        }
    }
    let _ = std::env::set_current_dir("/");
    match std::fs::metadata("/hello.txt") {
        Ok(metadata) => println!("stat_size={}", metadata.len()),
        Err(error) => {
            failed = true;
            println!("stat error: {error}");
        }
    }
    match std::fs::read_dir("/") {
        Ok(entries) => {
            for entry in entries.flatten() {
                println!("entry={}", entry.file_name().to_string_lossy());
            }
        }
        Err(error) => {
            failed = true;
            println!("dir error: {error}");
        }
    }
    std::process::exit(if failed { 1 } else { 0 });
}
