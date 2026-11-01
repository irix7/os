// SPDX-License-Identifier: GPL-3.0-only
//! The x86 machine under QEMU: one boot disk, one guest, a host HTTP check.
//!
//! This is the first slice of the rig's QEMU backend (docs/plan/x86-rig.md).
//! It boots one x86 guest with the MCP and wasm modules served by host
//! fixtures, waits for `BOOT-READY`, and fetches the module's status page
//! through the forwarded port.  The two-node M2 flow is built on this.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use crate::fixture::Fixture;
use crate::rig::{self, Node, Options};
use crate::sha256;

/// Boot disk size: 8 MiB, the same as the gate's image.
const DISK_SECTORS: usize = 16384;
const SECTOR: usize = 512;
const HEADER_MAGIC: &[u8; 8] = b"IRIX7MOD";
const MCP_HEADER_SECTOR: usize = 8;
const MCP_URL_SECTOR: usize = 9;
const WASM_HEADER_SECTOR: usize = 10;
const WASM_URL_SECTOR: usize = 11;
const CONFIG_SECTOR: usize = 100;

/// Run the one-guest smoke.
pub fn smoke(options: &Options) -> Result<i32, String> {
    require_file(&options.rom, "kernel image (--rom)")?;
    require_file(&options.module, "MCP module (--module)")?;
    require_file(&options.wasm, "wasm module (--wasm)")?;

    let node = derive_node(options, 0)?;
    fs::create_dir_all(&node.state_dir)
        .map_err(|error| format!("create {}: {error}", node.state_dir.display()))?;
    let _ = fs::remove_file(&node.serial_log);

    let mcp = Fixture::start(fs::read(&options.module).map_err(|e| e.to_string())?)
        .map_err(|error| format!("start MCP fixture: {error}"))?;
    let wasm = Fixture::start(fs::read(&options.wasm).map_err(|e| e.to_string())?)
        .map_err(|error| format!("start wasm fixture: {error}"))?;

    let disk = node.state_dir.join("boot.img");
    write_boot_disk(
        &disk,
        &["test.key=config-ok".to_string()],
        (&mcp_module(options), mcp.port),
        (&wasm_module(options), wasm.port),
    )?;

    let mut child = launch(options, &node, &disk)?;
    let result = (|| -> Result<(), String> {
        wait_boot(&node.serial_log, crate::timing::secs(90))?;
        let response = crate::homes::fetch(&node, "/", crate::timing::secs(15))?;
        if response.status != 200 {
            return Err(format!("GET / returned {}", response.status));
        }
        println!("x86: MCP status page {} bytes", response.body.len());
        Ok(())
    })();
    let _ = child.kill();
    let _ = child.wait();
    result?;
    println!(
        "x86-smoke: pass; serial log {}; fixtures {} and {}",
        node.serial_log.display(),
        mcp.port,
        wasm.port
    );
    Ok(0)
}

/// Two x86 guests joined through the host relay: the M2 topology without the
/// rest of the scenario.  Each node listens on 7000; the relay dials both
/// forwards.  The check is the live `/mesh/join` counter and `/mesh/homes`.
pub fn mesh(options: &Options) -> Result<i32, String> {
    require_file(&options.rom, "kernel image (--rom)")?;
    require_file(&options.module, "MCP module (--module)")?;
    require_file(&options.wasm, "wasm module (--wasm)")?;
    let nodes = [derive_node(options, 0)?, derive_node(options, 1)?];

    let mcp = Fixture::start(fs::read(&options.module).map_err(|e| e.to_string())?)
        .map_err(|error| format!("start MCP fixture: {error}"))?;
    let wasm = Fixture::start(fs::read(&options.wasm).map_err(|e| e.to_string())?)
        .map_err(|error| format!("start wasm fixture: {error}"))?;

    let mcp_image = mcp_module(options);
    let wasm_image = wasm_module(options);
    let (environments, _authority, _wrong) =
        rig::mesh_environments(&nodes, rig::Fault::None, rig::Transport::Relay);
    let mut children = Vec::new();
    for (node, environment) in nodes.iter().zip(environments.iter()) {
        fs::create_dir_all(&node.state_dir)
            .map_err(|error| format!("create {}: {error}", node.state_dir.display()))?;
        let mut lines = Vec::new();
        if let Some(environment) = environment {
            for (key, value) in node.environment(mcp.port, Some(environment)) {
                lines.push(format!("{key}={value}"));
            }
        }
        let disk = node.state_dir.join("boot.img");
        write_boot_disk(
            &disk,
            &lines,
            (&mcp_image, mcp.port),
            (&wasm_image, wasm.port),
        )?;
        children.push((node, launch(options, node, &disk)?));
    }

    let result = (|| -> Result<(), String> {
        for (node, _) in &children {
            wait_boot(&node.serial_log, crate::timing::secs(90))?;
        }
        let _relay = crate::relay::MeshRelay::start(
            (std::net::Ipv4Addr::LOCALHOST, nodes[0].mesh_port).into(),
            (std::net::Ipv4Addr::LOCALHOST, nodes[1].mesh_port).into(),
        )
        .map_err(|error| format!("start mesh relay: {error}"))?;
        // The root never sends a join request, so the request counter is not a
        // two-node signal.  Wait for both live views to report the two-member
        // epoch instead.
        let deadline = Instant::now() + crate::timing::secs(120);
        loop {
            let mut views = Vec::new();
            let mut joined = true;
            for node in &nodes {
                match crate::homes::fetch(node, "/mesh/homes", crate::timing::secs(5)) {
                    Ok(homes) if homes.status == 200 && homes.body.contains("members=2") => {
                        views.push(format!("{}: {}", node.name, homes.body.trim()));
                    }
                    _ => joined = false,
                }
            }
            if joined {
                for view in views {
                    println!("x86-mesh: {view}");
                }
                return Ok(());
            }
            if Instant::now() >= deadline {
                return Err("x86 guests did not both report a two-member view".into());
            }
            thread::sleep(Duration::from_millis(500));
        }
    })();

    for (_, child) in children.iter_mut() {
        let _ = child.kill();
        let _ = child.wait();
    }
    result?;
    println!("x86-mesh: pass");
    Ok(0)
}

fn mcp_module(options: &Options) -> Vec<u8> {
    fs::read(&options.module).unwrap_or_default()
}

fn wasm_module(options: &Options) -> Vec<u8> {
    fs::read(&options.wasm).unwrap_or_default()
}

fn require_file(path: &Path, what: &str) -> Result<(), String> {
    if path.is_file() {
        Ok(())
    } else {
        Err(format!("{what} is missing: {}", path.display()))
    }
}

/// Node description for one x86 guest.  QEMU's user network gives the guest
/// 10.0.2.15 and reaches the host at 10.0.2.2.
fn derive_node(options: &Options, instance: u32) -> Result<Node, String> {
    let state_dir = options.node_dir(instance);
    let serial_log = options.work.join(format!("x86-{instance}.serial"));
    let mac = [0x52, 0x54, 0x00, 0x00, 0x00, 0x01 + instance as u8];
    let mut node_id = [0u8; 16];
    node_id[..6].copy_from_slice(&mac);
    Ok(Node {
        name: format!("x{instance}"),
        instance: 0,
        state_dir,
        config: options.node_config(instance),
        ci_socket: PathBuf::new(),
        serial_log,
        monitor: 0,
        mac,
        node_id,
        nat_subnet: "10.0.2.0/24".into(),
        gateway: std::net::Ipv4Addr::new(10, 0, 2, 2),
        guest: std::net::Ipv4Addr::new(10, 0, 2, 15),
        telnet_port: options.telnet_port(instance),
        http_port: options.http_port(instance),
        mesh_port: options.mesh_port(instance),
        port_base: options.port_base(instance),
    })
}

/// Write the machine's boot disk: marker, configuration, module headers and
/// the fixture URLs the guest fetches at boot.
fn write_boot_disk(
    path: &Path,
    config_lines: &[String],
    mcp: (&Vec<u8>, u16),
    wasm: (&Vec<u8>, u16),
) -> Result<(), String> {
    let mut disk = vec![0u8; DISK_SECTORS * SECTOR];
    disk[SECTOR..SECTOR + 17].copy_from_slice(b"IRIX7-X86-DISK-OK");
    let config: String = config_lines
        .iter()
        .map(|line| format!("{line}\n"))
        .collect();
    let config = config.as_bytes();
    let capacity = 4 * SECTOR;
    if config.len() > capacity {
        return Err(format!(
            "x86 boot config is {} bytes, over the {capacity}-byte block",
            config.len()
        ));
    }
    disk[CONFIG_SECTOR * SECTOR..][..config.len()].copy_from_slice(config);
    let urls = [
        (MCP_URL_SECTOR, format!("10.0.2.2:{}/mcp.module", mcp.1)),
        (WASM_URL_SECTOR, format!("10.0.2.2:{}/mcp.module", wasm.1)),
    ];
    for (sector, url) in urls {
        disk[sector * SECTOR..][..url.len()].copy_from_slice(url.as_bytes());
    }
    for (sector, image) in [(MCP_HEADER_SECTOR, mcp.0), (WASM_HEADER_SECTOR, wasm.0)] {
        let header = sector * SECTOR;
        disk[header..header + 8].copy_from_slice(HEADER_MAGIC);
        disk[header + 8..header + 16].copy_from_slice(&(image.len() as u64).to_le_bytes());
        disk[header + 16..header + 48].copy_from_slice(&sha256::digest(image));
    }
    let mut file =
        fs::File::create(path).map_err(|error| format!("create {}: {error}", path.display()))?;
    file.write_all(&disk)
        .map_err(|error| format!("write {}: {error}", path.display()))
}

fn launch(options: &Options, node: &Node, disk: &Path) -> Result<Child, String> {
    // QEMU opens the serial log itself; create it first so a stale transcript
    // cannot satisfy the readiness wait.
    fs::File::create(&node.serial_log)
        .map_err(|error| format!("create {}: {error}", node.serial_log.display()))?;
    let stderr = fs::File::create(node.state_dir.join("qemu.stderr"))
        .map_err(|error| format!("create qemu stderr: {error}"))?;
    let mac = format!(
        "{:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
        node.mac[0], node.mac[1], node.mac[2], node.mac[3], node.mac[4], node.mac[5]
    );
    let netdev = format!(
        "user,id=net0,hostfwd=tcp:127.0.0.1:{}-:80,hostfwd=tcp:127.0.0.1:{}-:7000",
        node.http_port, node.mesh_port
    );
    Command::new(&options.qemu)
        .arg("-M")
        .arg("q35")
        .arg("-m")
        .arg("256M")
        .arg("-kernel")
        .arg(&options.rom)
        .arg("-display")
        .arg("none")
        .arg("-serial")
        .arg(format!("file:{}", node.serial_log.display()))
        .arg("-monitor")
        .arg("none")
        .arg("-no-reboot")
        .arg("-device")
        .arg("isa-debug-exit,iobase=0x501,iosize=0x04")
        .arg("-netdev")
        .arg(netdev)
        .arg("-device")
        .arg(format!(
            "virtio-net-pci,netdev=net0,disable-modern=on,mac={mac}"
        ))
        .arg("-drive")
        .arg(format!(
            "file={},if=none,id=disk0,format=raw",
            disk.display()
        ))
        .arg("-device")
        .arg("virtio-blk-pci,drive=disk0,disable-modern=on")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::from(stderr))
        .spawn()
        .map_err(|error| format!("start {}: {error}", options.qemu.display()))
}

fn wait_boot(log: &Path, timeout: Duration) -> Result<(), String> {
    let deadline = Instant::now() + timeout;
    loop {
        let text = fs::read_to_string(log).unwrap_or_default();
        if text.lines().any(|line| line.starts_with("BOOT-READY ")) {
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err(format!(
                "x86 guest did not reach BOOT-READY\n--- serial ---\n{}",
                text.lines().rev().take(20).collect::<Vec<_>>().join("\n")
            ));
        }
        thread::sleep(Duration::from_millis(200));
    }
}
