// SPDX-License-Identifier: GPL-3.0-only
//! The x86 machine under QEMU: one boot disk, one guest, a host HTTP check.
//!
//! This is the first slice of the rig's QEMU backend (docs/project/x86-rig.md).
//! It boots one x86 guest, waits for `BOOT-READY`, and fetches MCP's status page
//! through the forwarded port.  The two-node M2 flow is built on this.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use crate::rig::{self, Node, Options};

/// Boot disk size: 8 MiB, the same as the gate's image.
const DISK_SECTORS: usize = 16384;
const SECTOR: usize = 512;
const CONFIG_SECTOR: usize = 100;

/// Two x86 guests joined through the host relay: the M2 topology without the
/// rest of the scenario.  Each node listens on 7000; the relay dials both
/// forwards.  The check is the live `/mesh/join` counter and `/mesh/homes`.
/// Take a whitespace-delimited `name=value` field from a report line.
fn field(body: &str, key: &str) -> Option<String> {
    let at = body.find(key)? + key.len();
    let rest = &body[at..];
    let end = rest.find(|c: char| c.is_whitespace()).unwrap_or(rest.len());
    Some(rest[..end].to_string())
}

/// Wait until both guests report a two-member view, returning those views.
fn wait_two_members(nodes: &[Node], timeout: Duration) -> Result<Vec<String>, String> {
    let deadline = Instant::now() + timeout;
    loop {
        let mut views = Vec::new();
        let mut joined = true;
        for node in nodes {
            match crate::homes::fetch(node, "/mesh/homes", crate::timing::secs(5)) {
                Ok(homes) if homes.status == 200 && homes.body.contains("members=2") => {
                    views.push(format!("{}: {}", node.name, homes.body.trim()));
                }
                _ => joined = false,
            }
        }
        if joined {
            return Ok(views);
        }
        if Instant::now() >= deadline {
            return Err("x86 guests did not both report a two-member view".into());
        }
        thread::sleep(Duration::from_millis(500));
    }
}

/// Stop one guest, relaunch it on its own disk, and wait for both views to
/// reconverge.  The store record must be on the host disk before the stop, and
/// the restarted guest must log every marker in `expected`.
fn restart_guest(
    options: &Options,
    nodes: &[Node; 2],
    children: &mut [(&Node, Child)],
    index: usize,
    expected: &[&str],
) -> Result<crate::relay::MeshRelay, String> {
    let node = &nodes[index];
    let disk = fs::read(node.state_dir.join("boot.img")).unwrap_or_default();
    let at = (4096 + 8) * 512;
    if disk.get(at..at + 8) != Some(b"I7EPST01") {
        return Err(format!(
            "{}: epoch record is not on the boot disk before the restart",
            node.name
        ));
    }
    let child = children
        .iter_mut()
        .find(|(child_node, _)| child_node.instance == node.instance)
        .map(|(_, child)| child)
        .ok_or_else(|| format!("x86: {} child missing", node.name))?;
    let _ = child.kill();
    let _ = child.wait();
    thread::sleep(crate::timing::secs(2));
    let image = node.state_dir.join("boot.img");
    let restarted = launch(options, node, &image)?;
    if let Some(slot) = children
        .iter_mut()
        .find(|(child_node, _)| child_node.instance == node.instance)
    {
        slot.1 = restarted;
    }
    wait_boot(&node.serial_log, crate::timing::secs(90))?;
    let relay = crate::relay::MeshRelay::start(
        (std::net::Ipv4Addr::LOCALHOST, nodes[0].mesh_port).into(),
        (std::net::Ipv4Addr::LOCALHOST, nodes[1].mesh_port).into(),
    )
    .map_err(|error| format!("restart mesh relay: {error}"))?;
    let views = wait_two_members(nodes, crate::timing::secs(120))?;
    for view in views {
        println!("x86-mesh: after {} restart {view}", node.name);
    }
    let serial = fs::read_to_string(&node.serial_log).unwrap_or_default();
    for marker in expected {
        if !serial.contains(marker) {
            return Err(format!(
                "{} did not log {marker:?} after restart\n--- serial ---\n{serial}",
                node.name
            ));
        }
    }
    Ok(relay)
}

pub fn mesh(options: &Options) -> Result<i32, String> {
    require_file(&options.rom, "kernel image (--rom)")?;
    let nodes = [derive_node(options, 0)?, derive_node(options, 1)?];

    let (environments, _authority, _wrong) =
        rig::mesh_environments(&nodes, rig::Fault::None, rig::Transport::Relay);
    let mut children = Vec::new();
    for (node, environment) in nodes.iter().zip(environments.iter()) {
        fs::create_dir_all(&node.state_dir)
            .map_err(|error| format!("create {}: {error}", node.state_dir.display()))?;
        let mut lines = Vec::new();
        if let Some(environment) = environment {
            // MCP is linked into the image, so the `mcp.*` NVRAM keys have no reader here.
            for (key, value) in node.environment(Some(environment)) {
                if !key.starts_with("mcp.") {
                    lines.push(format!("{key}={value}"));
                }
            }
        }
        let disk = node.state_dir.join("boot.img");
        write_boot_disk(&disk, &lines)?;
        children.push((node, launch(options, node, &disk)?));
    }

    let result = (|| -> Result<(), String> {
        for (node, _) in &children {
            wait_boot(&node.serial_log, crate::timing::secs(90))?;
        }
        let mut relay = crate::relay::MeshRelay::start(
            (std::net::Ipv4Addr::LOCALHOST, nodes[0].mesh_port).into(),
            (std::net::Ipv4Addr::LOCALHOST, nodes[1].mesh_port).into(),
        )
        .map_err(|error| format!("start mesh relay: {error}"))?;
        let views = wait_two_members(&nodes, crate::timing::secs(120))?;
        for view in views {
            println!("x86-mesh: {view}");
        }
        // Route/MOVED runs over the same HTTP surface the iris rig uses.
        let homes = crate::homes::fetch(&nodes[0], "/mesh/homes", crate::timing::secs(10))?;
        let digest = field(&homes.body, "digest=").ok_or("x86: homes view has no digest")?;
        let members = vec![nodes[0].node_id_string(), nodes[1].node_id_string()];
        let route = crate::route::run(
            &nodes[0],
            &nodes[1],
            &members,
            2,
            &digest,
            crate::timing::secs(60),
        );
        if !route.ok {
            return Err(format!("x86 route: {}", route.message));
        }
        println!("x86-mesh: route {}", route.message);
        let migration = crate::migrate::run(
            &nodes[0],
            &nodes[1],
            &members,
            2,
            &digest,
            crate::timing::secs(120),
        );
        if !migration.ok {
            return Err(format!("x86 migration: {}", migration.message));
        }
        println!("x86-mesh: migration {}", migration.message);

        drop(relay);
        relay = restart_guest(
            options,
            &nodes,
            &mut children,
            1,
            &["epoch-store: restored epoch=2", "source=confirmation"],
        )?;
        println!("x86-mesh: member restart restored epoch 2 and both views reconverged");

        drop(relay);
        relay = restart_guest(
            options,
            &nodes,
            &mut children,
            0,
            &["epoch-store: restored epoch=2", "source=root-record"],
        )?;
        println!("x86-mesh: root restart restored epoch 2 and both views reconverged");
        drop(relay);

        Ok(())
    })();

    for (_, child) in children.iter_mut() {
        let _ = child.kill();
        let _ = child.wait();
    }
    result?;
    println!("x86-mesh: pass");
    Ok(0)
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
        instance,
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

/// Write the machine's boot disk: the marker and the boot configuration.
fn write_boot_disk(path: &Path, config_lines: &[String]) -> Result<(), String> {
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
            "file={},if=none,id=disk0,format=raw,cache=writethrough",
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
