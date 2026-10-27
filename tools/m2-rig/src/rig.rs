// SPDX-License-Identifier: GPL-3.0-only
//! The two-node rig: options, instance derivation, NVRAM, process control and
//! serial readiness.
//!
//! One `Machine` per process is an iris invariant, so the rig starts two
//! `iris` processes.  `--instance N` derives a disjoint state directory, CI
//! socket, monitor/serial ports, MAC and NAT subnet.  The rig adds only the
//! explicit host forwards it needs (telnet, MCP HTTP and the future mesh
//! listener), then records every derived value in the evidence bundle.

use std::fs;
use std::io::Write;
use std::net::Ipv4Addr;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use crate::nvram;

/// The guest's future mesh TCP listener.
pub const MESH_GUEST_PORT: u16 = 7000;
/// MCP listens on guest port 80.
pub const MCP_GUEST_PORT: u16 = 80;
/// The kernel telnet console listens on guest port 23.
pub const TELNET_GUEST_PORT: u16 = 23;

/// What the operator asked the rig to do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    /// Start both nodes, probe both directions, stop, and write evidence.
    Run,
    /// Start the M3 shard slice: `--nodes` kernels, join through the direct
    /// two-node relay (or the experimental hub), shard evidence and one route.
    Shard,
    /// Start both nodes and leave them running.
    Up,
    /// Stop both nodes.
    Down,
    /// Show the nodes' lifecycle and boot markers.
    Status,
    /// Probe both directions against already-running nodes.
    Probe,
    /// Print the derived instance table without starting anything.
    PrintConfig,
    /// Remove the rig work directory.
    Clean,
}

impl Action {
    /// Parse the command line word.
    pub fn parse(word: &str) -> Option<Self> {
        Some(match word {
            "run" => Self::Run,
            "shard" => Self::Shard,
            "up" => Self::Up,
            "down" => Self::Down,
            "status" => Self::Status,
            "probe" => Self::Probe,
            "print-config" => Self::PrintConfig,
            "clean" => Self::Clean,
            _ => return None,
        })
    }
}

/// A deliberate join-path fault for negative evidence.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fault {
    /// The normal two-node join.
    None,
    /// Omit every `mesh.*` key: the M2-1 listener and join are not configured.
    NoPeer,
    /// Give the member a valid but wrong root public key.
    WrongAuthority,
    /// Join and homes are enabled, but `mesh.route` is absent.
    NoRoute,
    /// Kill the member after the route steps: the root must fail, fence and
    /// continue on its own.
    KillPeer,
    /// Cut only the mesh link: both kernels stay alive and must not create two
    /// authoritative homes.
    Partition,
}

impl Fault {
    /// Parse the command-line word.
    pub fn parse(word: &str) -> Option<Self> {
        Some(match word {
            "none" => Self::None,
            "no-peer" => Self::NoPeer,
            "wrong-authority" => Self::WrongAuthority,
            "no-route" => Self::NoRoute,
            "kill-peer" => Self::KillPeer,
            "partition" => Self::Partition,
            _ => return None,
        })
    }

    /// A stable label for evidence.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::NoPeer => "no-peer",
            Self::WrongAuthority => "wrong-authority",
            Self::NoRoute => "no-route",
            Self::KillPeer => "kill-peer",
            Self::Partition => "partition",
        }
    }

    /// Whether the join is expected to converge in this mode.
    pub const fn joins(self) -> bool {
        !matches!(self, Self::NoPeer | Self::WrongAuthority)
    }

    /// Whether the remote capability route is expected to run.
    pub const fn routes(self) -> bool {
        matches!(self, Self::None | Self::KillPeer | Self::Partition)
    }

    /// Whether the L0 actor migration is expected to run.
    ///
    /// Migration is driven only in the clean default mode; the failure modes
    /// preserve the M2-6 scenario and the no-route/no-peer negatives keep their
    /// original markers.
    pub const fn migrates(self) -> bool {
        matches!(self, Self::None)
    }

    /// Whether the mode drives a one-node failure or a partition.
    pub const fn failures(self) -> bool {
        matches!(self, Self::KillPeer | Self::Partition)
    }
}

/// Parsed command line.
#[derive(Debug)]
pub struct Options {
    /// Action to perform.
    pub action: Action,
    /// Repository root (contains `kernel/linker.ld`).
    pub repo: PathBuf,
    /// Rig work directory.
    pub work: PathBuf,
    /// `iris` binary.
    pub iris: PathBuf,
    /// `iris-ci` binary.
    pub iris_ci: PathBuf,
    /// PROM image.
    pub rom: PathBuf,
    /// Fresh MCP module image.
    pub module: PathBuf,
    /// First monitor port block.
    pub base_port: u16,
    /// Number of nodes for the M3 shard scenario.
    pub nodes: u32,
    /// Use the experimental host hub for more than two real kernels.
    pub hub: bool,
    /// Boot deadline.
    pub timeout: Duration,
    /// Join-path fault mode.
    pub fault: Fault,
    /// Leave the nodes running after `run`.
    pub keep: bool,
    /// Print extra detail.
    pub verbose: bool,
}

impl Options {
    /// The work directory for one node.
    pub fn node_dir(&self, instance: u32) -> PathBuf {
        self.work.join(format!("n{instance}"))
    }

    /// The generated iris config for one node.
    pub fn node_config(&self, instance: u32) -> PathBuf {
        self.work.join(format!("n{instance}.toml"))
    }

    /// The monitor/serial/CI port block for one node.
    pub fn port_base(&self, instance: u32) -> u16 {
        self.base_port
            .saturating_add((instance as u16).saturating_mul(100))
    }

    /// The first host-forward port for one node.
    ///
    /// Monitor blocks are `base + instance * 100` and use up to
    /// `base + 700` for eight nodes.  The forward block starts at
    /// `base + 1000` so the two ranges cannot overlap at any node count the
    /// rig supports (M2's two-node layout grew into the M3 eight-node one).
    pub fn forward_base(&self, instance: u32) -> u16 {
        self.base_port
            .saturating_add(1000)
            .saturating_add((instance as u16).saturating_mul(10))
    }

    /// Telnet forward: host -> guest 23.
    pub fn telnet_port(&self, instance: u32) -> u16 {
        self.forward_base(instance)
    }

    /// MCP HTTP forward: host -> guest 80.
    pub fn http_port(&self, instance: u32) -> u16 {
        self.forward_base(instance).saturating_add(1)
    }

    /// Mesh forward: host -> guest 7000.
    pub fn mesh_port(&self, instance: u32) -> u16 {
        self.forward_base(instance).saturating_add(2)
    }
}

/// Parse arguments.  `argv[0]` is the binary name.
pub fn parse_args<I>(args: I) -> Result<Option<Options>, String>
where
    I: IntoIterator<Item = String>,
{
    let mut action = None;
    let mut repo = None;
    let mut work = None;
    let mut iris = None;
    let mut iris_ci = None;
    let mut rom = None;
    let mut module = None;
    let mut base_port = 9100u16;
    let mut nodes = 2u32;
    let mut hub = false;
    let mut timeout = Duration::from_secs(180);
    let mut fault = Fault::None;
    let mut keep = false;
    let mut verbose = false;

    let mut iter = args.into_iter();
    let _program = iter.next();
    while let Some(argument) = iter.next() {
        match argument.as_str() {
            "-h" | "--help" => return Ok(None),
            "--repo" => repo = Some(PathBuf::from(next_value(&mut iter, "--repo")?)),
            "--work" => work = Some(PathBuf::from(next_value(&mut iter, "--work")?)),
            "--iris" => iris = Some(PathBuf::from(next_value(&mut iter, "--iris")?)),
            "--iris-ci" => iris_ci = Some(PathBuf::from(next_value(&mut iter, "--iris-ci")?)),
            "--rom" => rom = Some(PathBuf::from(next_value(&mut iter, "--rom")?)),
            "--module" => module = Some(PathBuf::from(next_value(&mut iter, "--module")?)),
            "--base-port" => {
                base_port = next_value(&mut iter, "--base-port")?
                    .parse()
                    .map_err(|_| "--base-port must be a TCP port".to_string())?;
            }
            "--nodes" => {
                nodes = next_value(&mut iter, "--nodes")?
                    .parse()
                    .map_err(|_| "--nodes must be 2..=8".to_string())?;
                if !(2..=8).contains(&nodes) {
                    return Err("--nodes must be 2..=8".into());
                }
            }
            "--hub" => hub = true,
            "--timeout" => {
                let seconds: u64 = next_value(&mut iter, "--timeout")?
                    .parse()
                    .map_err(|_| "--timeout must be seconds".to_string())?;
                timeout = Duration::from_secs(seconds.max(10));
            }
            "--fault" => {
                let word = next_value(&mut iter, "--fault")?;
                fault = Fault::parse(&word).ok_or_else(|| {
                    "--fault must be none, no-peer, wrong-authority or no-route".to_string()
                })?;
            }
            "--keep" => keep = true,
            "--verbose" => verbose = true,
            other if other.starts_with('-') => {
                return Err(format!("unknown option {other}"));
            }
            other => {
                if action.is_some() {
                    return Err(format!("unexpected argument {other}"));
                }
                action =
                    Some(Action::parse(other).ok_or_else(|| format!("unknown command {other}"))?);
            }
        }
    }

    let action = action.unwrap_or(Action::Run);
    let repo = match repo {
        Some(path) => path,
        None => find_repo_root()?,
    };
    let work = work.unwrap_or_else(|| repo.join("target/m2-rig"));
    let iris = iris
        .or_else(|| std::env::var_os("IRIS").map(PathBuf::from))
        .unwrap_or_else(|| default_sibling(&repo, "iris"));
    let iris_ci = iris_ci
        .or_else(|| std::env::var_os("IRIS_CI").map(PathBuf::from))
        .unwrap_or_else(|| default_sibling(&repo, "iris-ci"));
    let rom = rom.unwrap_or_else(|| work.join("irix7-rom.bin"));
    let module = module.unwrap_or_else(|| work.join("mcp.module"));

    Ok(Some(Options {
        action,
        repo,
        work,
        iris,
        iris_ci,
        rom,
        module,
        base_port,
        nodes,
        hub,
        timeout,
        fault,
        keep,
        verbose,
    }))
}

fn next_value<I>(iter: &mut I, option: &str) -> Result<String, String>
where
    I: Iterator<Item = String>,
{
    iter.next().ok_or_else(|| format!("{option} needs a value"))
}

fn find_repo_root() -> Result<PathBuf, String> {
    if let Some(path) = std::env::var_os("M2_RIG_REPO") {
        return Ok(PathBuf::from(path));
    }
    let cwd = std::env::current_dir().map_err(|error| format!("cwd: {error}"))?;
    for candidate in cwd.ancestors() {
        if candidate.join("kernel/linker.ld").is_file() && candidate.join("Cargo.toml").is_file() {
            return Ok(candidate.to_path_buf());
        }
    }
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    if let Some(candidate) = manifest.parent().and_then(Path::parent) {
        if candidate.join("kernel/linker.ld").is_file() {
            return Ok(candidate.to_path_buf());
        }
    }
    Err("cannot find the repository root; pass --repo".into())
}

fn default_sibling(repo: &Path, name: &str) -> PathBuf {
    let sibling = repo
        .parent()
        .map(|parent| parent.join("iris/target/release").join(name))
        .filter(|path| path.is_file());
    sibling.unwrap_or_else(|| PathBuf::from(name))
}

/// One independent guest.
#[derive(Clone, Debug)]
pub struct Node {
    /// `n0` or `n1`.
    pub name: String,
    /// iris instance id.
    pub instance: u32,
    /// Instance state directory.
    pub state_dir: PathBuf,
    /// Generated iris config.
    pub config: PathBuf,
    /// CI control socket.
    pub ci_socket: PathBuf,
    /// Serial transcript.
    pub serial_log: PathBuf,
    /// Monitor port.
    pub monitor: u16,
    /// Station address.
    pub mac: [u8; 6],
    /// The kernel's 16-byte NodeId (MAC followed by zeros).
    pub node_id: [u8; 16],
    /// NAT CIDR.
    pub nat_subnet: String,
    /// NAT gateway address seen by this guest.
    pub gateway: Ipv4Addr,
    /// Guest address.
    pub guest: Ipv4Addr,
    /// Host telnet forward.
    pub telnet_port: u16,
    /// Host MCP HTTP forward.
    pub http_port: u16,
    /// Host mesh forward.
    pub mesh_port: u16,
    /// Port base.
    pub port_base: u16,
}

impl Node {
    /// The MAC as lower-case colon-separated text.
    pub fn mac_string(&self) -> String {
        nvram::mac_string(&self.mac)
    }

    /// The NodeId as the kernel prints it: 32 upper-case hex digits.
    pub fn node_id_string(&self) -> String {
        let mut out = String::with_capacity(32);
        for byte in self.node_id {
            out.push(
                char::from_digit((byte >> 4) as u32, 16)
                    .unwrap()
                    .to_ascii_uppercase(),
            );
            out.push(
                char::from_digit((byte & 0xf) as u32, 16)
                    .unwrap()
                    .to_ascii_uppercase(),
            );
        }
        out
    }

    /// The environment the rig writes into this node's DS1386 image.
    ///
    /// `mesh` is `None` in the `no-peer` fault mode: no `mesh.*` key is
    /// written, so the module keeps the M1/M2-1 two-slot behaviour.
    pub fn environment(&self, fixture_port: u16, mesh: Option<&MeshEnv>) -> Vec<(String, String)> {
        let mut env = vec![
            (
                "mcp.url".into(),
                format!("http://{}:{fixture_port}/mcp.module", self.gateway),
            ),
            ("mcp.sha256".into(), String::new()),
            ("console".into(), "serial0".into()),
        ];
        if let Some(mesh) = mesh {
            if let Some(peer) = &mesh.peer {
                env.push(("mesh.peer".into(), peer.clone()));
            }
            if let Some(authority) = &mesh.authority {
                env.push(("mesh.authority".into(), authority.clone()));
            }
            // The module defaults the seed epoch to 1; the DS1386 environment
            // has 298 bytes before the MAC slot, so the M2-6 opt-in reuses the
            // default instead of spending 13 bytes restating it.
            if let Some(epoch) = mesh.epoch {
                if epoch != 1 {
                    env.push(("mesh.epoch".into(), epoch.to_string()));
                }
            }
            // The DS1386 environment has only 298 bytes before the MAC slot.
            // The M2-3 default is eight objects, so the route opt-in can omit
            // the redundant key instead of pushing the environment over.
            if let Some(objects) = mesh.objects {
                if objects != crate::homes::OBJECT_COUNT {
                    env.push(("mesh.obj".into(), objects.to_string()));
                }
            }
            if let Some(shard) = mesh.shard {
                env.push(("mesh.shard".into(), shard.to_string()));
            }
            if let Some(key) = &mesh.public_key {
                env.push(("mesh.pub".into(), key.clone()));
            }
            if let Some(key) = &mesh.seed {
                env.push(("mesh.key".into(), key.clone()));
            }
            if let Some(route) = &mesh.route {
                env.push(("mesh.route".into(), route.clone()));
            }
            if let Some(fail) = &mesh.fail {
                env.push(("mesh.fail".into(), fail.clone()));
            }
        }
        env
    }
}

/// The mesh-related NVRAM values for one node.
#[derive(Clone, Debug, Default)]
pub struct MeshEnv {
    /// The peer's mesh host port through this node's gateway.
    pub peer: Option<String>,
    /// The authority root's MAC-derived `NodeId`.
    pub authority: Option<String>,
    /// The bootstrap epoch.
    pub epoch: Option<u32>,
    /// Number of objects in the M2 home-evidence fixture set (`mesh.obj`).
    pub objects: Option<u32>,
    /// Number of objects in the M3 shard-evidence set (`mesh.shard`).
    pub shard: Option<u32>,
    /// The authority root's Ed25519 public key as lower-case hex (`mesh.pub`).
    pub public_key: Option<String>,
    /// The authority root's 32-byte Ed25519 seed as lower-case hex
    /// (`mesh.key`).  Only the root carries this; it is a lab fixture, not a
    /// secret store.
    pub seed: Option<String>,
    /// The M2 route opt-in (`mesh.route`).  Absent keeps the M2-3 behaviour.
    pub route: Option<String>,
    /// The M2-6 failure-detection opt-in (`mesh.fail`).  Absent keeps the
    /// M2-2 same-epoch rejoin behaviour.
    pub fail: Option<String>,
}

/// The deterministic lab authority key pair.
#[derive(Clone, Debug)]
pub struct AuthorityKeys {
    /// Seed as lower-case hex.
    pub seed_hex: String,
    /// Public key as lower-case hex.
    pub public_hex: String,
}

impl AuthorityKeys {
    /// Derive a key pair from a domain separator.
    pub fn derive(domain: &[u8]) -> Self {
        let seed = crate::sha256::digest(domain);
        let signing = ed25519_dalek::SigningKey::from_bytes(&seed);
        let public = signing.verifying_key().to_bytes();
        Self {
            seed_hex: crate::sha256::hex(&seed),
            public_hex: crate::sha256::hex(&public),
        }
    }

    /// The fixed M2 lab authority key.  It is public test material by design;
    /// key custody is not claimed in M2.
    pub fn lab() -> Self {
        Self::derive(b"irix7 M2 lab authority key v1")
    }

    /// A valid but wrong key, for the `wrong-authority` fault.
    pub fn wrong() -> Self {
        Self::derive(b"irix7 M2 lab wrong authority key v1")
    }
}

/// Build the `mesh.*` environment for each node under a fault mode.
pub fn mesh_environments(
    nodes: &[Node],
    fault: Fault,
) -> (Vec<Option<MeshEnv>>, AuthorityKeys, AuthorityKeys) {
    let authority = AuthorityKeys::lab();
    let wrong = AuthorityKeys::wrong();
    if fault == Fault::NoPeer {
        return (vec![None, None], authority, wrong);
    }
    let mut envs = Vec::new();
    for (index, node) in nodes.iter().enumerate() {
        let peer = &nodes[1 - index];
        let mut env = MeshEnv {
            peer: Some(format!("{}:{}", node.gateway, peer.mesh_port)),
            authority: Some(nodes[0].mac_string()),
            epoch: Some(1),
            objects: Some(crate::homes::OBJECT_COUNT),
            shard: None,
            public_key: None,
            seed: None,
            route: if fault.migrates() {
                Some("2".into())
            } else if fault.routes() {
                Some("1".into())
            } else {
                None
            },
            fail: if fault.failures() {
                Some("1".into())
            } else {
                None
            },
        };
        if index == 0 {
            env.seed = Some(authority.seed_hex.clone());
        } else {
            env.public_key = Some(if fault == Fault::WrongAuthority {
                wrong.public_hex.clone()
            } else {
                authority.public_hex.clone()
            });
        }
        envs.push(Some(env));
    }
    (envs, authority, wrong)
}

/// Build the M3 shard environment for every node.
///
/// Every node carries the same root identity and shard count.  The root is the
/// only key holder; every member gets the public key.  `mesh.peer` is the
/// enable key for the mesh listener; under the M3 host hub the guests do not
/// dial it, the hub connects to each guest's mesh forward.
pub fn shard_environments(
    nodes: &[Node],
    shard_objects: u32,
    route_member: usize,
) -> (Vec<MeshEnv>, AuthorityKeys) {
    let authority = AuthorityKeys::lab();
    let root = &nodes[0];
    let envs = nodes
        .iter()
        .enumerate()
        .map(|(index, node)| {
            let mut env = MeshEnv {
                // Presence enables the guest listener.  Under the M3 host hub
                // the guest does not dial this value; the hub connects to the
                // guest's mesh forward.  Keep it short: the DS1386 environment
                // has only 298 bytes before the MAC slot.
                peer: Some(format!("{}:1", node.gateway)),
                authority: Some(root.mac_string()),
                epoch: Some(1),
                objects: Some(crate::homes::OBJECT_COUNT),
                shard: Some(shard_objects),
                public_key: None,
                seed: None,
                // The M2 route policy only runs on the root and the chosen
                // remote owner.  Other members keep the M3 shard surface and
                // stay well inside the NVRAM budget.
                route: if index == 0 || index == route_member {
                    Some("1".into())
                } else {
                    None
                },
                fail: None,
            };
            if index == 0 {
                env.seed = Some(authority.seed_hex.clone());
            } else {
                env.public_key = Some(authority.public_hex.clone());
            }
            env
        })
        .collect();
    (envs, authority)
}

/// Derive the configured nodes, write their configs, and return them.
///
/// This calls the emulator's own `--print-instance` surface rather than
/// reimplementing its derivation.  The only additions are the explicit host
/// forwards needed by the rig.
pub fn derive_nodes(options: &Options) -> Result<Vec<Node>, String> {
    fs::create_dir_all(&options.work)
        .map_err(|error| format!("create {}: {error}", options.work.display()))?;
    let mut nodes = Vec::new();
    for instance in 0..options.nodes {
        let config = options.node_config(instance);
        write_config(options, instance, &config)?;
        let report = run_print_instance(options, instance, &config)?;
        let monitor = parse_port(&report, "monitor")?;
        let ci_socket = PathBuf::from(parse_field(&report, "ci_socket")?);
        let serial_log = PathBuf::from(parse_field(&report, "serial_log")?);
        let mac = nvram::parse_mac(&parse_field(&report, "mac")?)?;
        let nat_subnet = parse_field(&report, "nat_subnet")?;
        let (gateway, guest) = subnet_hosts(&nat_subnet)?;
        let state_dir = PathBuf::from(parse_field(&report, "state_dir")?);
        let mut node_id = [0u8; 16];
        node_id[..6].copy_from_slice(&mac);
        nodes.push(Node {
            name: format!("n{instance}"),
            instance,
            state_dir,
            config,
            ci_socket,
            serial_log,
            monitor,
            mac,
            node_id,
            nat_subnet,
            gateway,
            guest,
            telnet_port: options.telnet_port(instance),
            http_port: options.http_port(instance),
            mesh_port: options.mesh_port(instance),
            port_base: options.port_base(instance),
        });
    }
    Ok(nodes)
}

fn write_config(options: &Options, instance: u32, path: &Path) -> Result<(), String> {
    let telnet = options.telnet_port(instance);
    let http = options.http_port(instance);
    let mesh = options.mesh_port(instance);
    let node_dir = options.node_dir(instance);
    fs::create_dir_all(&node_dir)
        .map_err(|error| format!("create {}: {error}", node_dir.display()))?;
    // The M3 shard rig keeps its original disk configuration; the M2 two-node
    // rig adds the dedicated epoch-store target so persistence is exercised on
    // every clean, no-route and failure run.
    let scsi_scratch = if options.action == Action::Shard {
        String::new()
    } else {
        let scratch = node_dir.join("scratch.raw");
        format!(
            "# Dedicated scratch volume for the persistent confirmed-epoch record.\n\
             # Target 2 is the MCP module's EROFS fixture and target 1 is the\n\
             # boot disk, so the epoch store uses target 3.\n\
             [scsi.3]\n\
             path = \"{}\"\n\
             scratch = true\n\
             size_mb = 4\n\
             \n",
            scratch.display()
        )
    };
    let config = format!(
        "# Generated by tools/m2-rig. Do not edit.\n\
         banks = [64, 0, 0, 0]\n\
         no_audio = true\n\
         \n\
         [machine]\n\
         profile = \"indy_ip24\"\n\
         \n\
         [scsi]\n\
         \n\
         {scsi_scratch}\
         [[port_forward]]\n\
         proto = \"tcp\"\n\
         host_port = {telnet}\n\
         guest_port = {TELNET_GUEST_PORT}\n\
         bind = \"localhost\"\n\
         \n\
         [[port_forward]]\n\
         proto = \"tcp\"\n\
         host_port = {http}\n\
         guest_port = {MCP_GUEST_PORT}\n\
         bind = \"localhost\"\n\
         \n\
         [[port_forward]]\n\
         proto = \"tcp\"\n\
         host_port = {mesh}\n\
         guest_port = {MESH_GUEST_PORT}\n\
         bind = \"localhost\"\n"
    );
    fs::write(path, config).map_err(|error| format!("write {}: {error}", path.display()))
}

/// Remove the per-node scratch volumes before a fresh `up`.
///
/// The epoch store must survive a process restart but must not leak a record
/// from an earlier run into a new one.  `up` is the clean start; the restart
/// paths call `start_node` directly and therefore keep the volume.
pub fn reset_scratch(nodes: &[Node]) -> Result<(), String> {
    for node in nodes {
        let path = node.state_dir.join("scratch.raw");
        match fs::remove_file(&path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(format!("remove {}: {error}", path.display())),
        }
    }
    Ok(())
}

fn run_print_instance(options: &Options, instance: u32, config: &Path) -> Result<String, String> {
    let output = Command::new(&options.iris)
        .arg("--config")
        .arg(config)
        .arg("--instance")
        .arg(instance.to_string())
        .arg("--state-dir")
        .arg(options.node_dir(instance))
        .arg("--port-base")
        .arg(options.port_base(instance).to_string())
        .arg("--print-instance")
        .output()
        .map_err(|error| format!("run {} --print-instance: {error}", options.iris.display()))?;
    if !output.status.success() {
        return Err(format!(
            "iris --print-instance {} failed: {}{}",
            instance,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

fn parse_field(report: &str, name: &str) -> Result<String, String> {
    let prefix = format!("{name}: ");
    report
        .lines()
        .find_map(|line| line.strip_prefix(&prefix))
        .map(|value| value.trim().to_string())
        .ok_or_else(|| format!("instance report has no {name}"))
}

fn parse_port(report: &str, name: &str) -> Result<u16, String> {
    parse_field(report, name)?
        .parse()
        .map_err(|_| format!("instance report has a bad {name}"))
}

/// Split a /24 NAT CIDR into gateway (.1) and guest (.2).
pub fn subnet_hosts(cidr: &str) -> Result<(Ipv4Addr, Ipv4Addr), String> {
    let (address, prefix) = cidr
        .split_once('/')
        .ok_or_else(|| format!("NAT subnet {cidr:?} is not CIDR"))?;
    if prefix != "24" {
        return Err(format!("NAT subnet {cidr:?} is not a /24"));
    }
    let base: Ipv4Addr = address
        .parse()
        .map_err(|_| format!("NAT subnet {cidr:?} has a bad address"))?;
    let octets = base.octets();
    if octets[3] != 0 {
        return Err(format!(
            "NAT subnet {cidr:?} does not start on a /24 boundary"
        ));
    }
    Ok((
        Ipv4Addr::new(octets[0], octets[1], octets[2], 1),
        Ipv4Addr::new(octets[0], octets[1], octets[2], 2),
    ))
}

/// Write the DS1386 image for a node.
pub fn write_nvram(
    node: &Node,
    fixture_port: u16,
    module_sha256: &str,
    mesh: Option<&MeshEnv>,
) -> Result<(), String> {
    let mut env = node.environment(fixture_port, mesh);
    for (name, value) in &mut env {
        if name == "mcp.sha256" {
            *value = module_sha256.to_string();
        }
    }
    let pairs: Vec<(&str, &str)> = env
        .iter()
        .map(|(name, value)| (name.as_str(), value.as_str()))
        .collect();
    let image = nvram::image(&pairs, node.mac)?;
    fs::create_dir_all(&node.state_dir)
        .map_err(|error| format!("create {}: {error}", node.state_dir.display()))?;
    fs::write(node.state_dir.join("nvram.bin"), image)
        .map_err(|error| format!("write NVRAM for {}: {error}", node.name))
}

/// A started iris process.
pub struct RunningNode {
    /// The node description.
    pub node: Node,
    /// The child process.
    pub child: Child,
}

/// Start one iris process.
pub fn start_node(options: &Options, node: &Node) -> Result<RunningNode, String> {
    fs::create_dir_all(&node.state_dir)
        .map_err(|error| format!("create {}: {error}", node.state_dir.display()))?;
    // iris opens the serial log in append mode, so a previous run's
    // `BOOT-READY` would satisfy the readiness wait before the new kernel has
    // booted.  A run owns its serial transcript from the first byte.
    let _ = fs::remove_file(&node.serial_log);
    let stdout_path = node.state_dir.join("stdout");
    let stderr_path = node.state_dir.join("stderr");
    let stdout = fs::File::create(&stdout_path)
        .map_err(|error| format!("create {}: {error}", stdout_path.display()))?;
    let stderr = fs::File::create(&stderr_path)
        .map_err(|error| format!("create {}: {error}", stderr_path.display()))?;
    let mut command = Command::new(&options.iris);
    command
        .arg("--config")
        .arg(&node.config)
        .arg("--instance")
        .arg(node.instance.to_string())
        .arg("--state-dir")
        .arg(&node.state_dir)
        .arg("--port-base")
        .arg(node.port_base.to_string())
        .arg("--prom")
        .arg(&options.rom)
        .arg("--nvram")
        .arg(node.state_dir.join("nvram.bin"))
        .arg("--ci")
        .arg("--ci-socket")
        .arg(&node.ci_socket)
        .arg("--serial-log")
        .arg(&node.serial_log)
        .arg("--monitor-port")
        .arg(node.monitor.to_string())
        .current_dir(&node.state_dir)
        .env("IRIS_NO_IDLE", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::from(stdout))
        .stderr(Stdio::from(stderr));
    let child = command
        .spawn()
        .map_err(|error| format!("start {}: {error}", options.iris.display()))?;
    Ok(RunningNode {
        node: node.clone(),
        child,
    })
}

/// Wait for the CI socket to appear and the monitor to answer.
pub fn wait_ci_ready(options: &Options, node: &Node, timeout: Duration) -> Result<(), String> {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if node.ci_socket.exists() {
            match run_ci(options, node, &["status", "--wait-ready", "30"]) {
                Ok(_) => return Ok(()),
                Err(error) => {
                    if options.verbose {
                        eprintln!("{}: CI not ready yet: {error}", node.name);
                    }
                }
            }
        }
        thread::sleep(Duration::from_millis(100));
    }
    Err(format!(
        "{}: CI socket {} did not become ready",
        node.name,
        node.ci_socket.display()
    ))
}

/// Run an `iris-ci` subcommand against a node.
pub fn run_ci(options: &Options, node: &Node, args: &[&str]) -> Result<String, String> {
    let output = Command::new(&options.iris_ci)
        .arg("--socket")
        .arg(&node.ci_socket)
        .args(args)
        .output()
        .map_err(|error| format!("run {}: {error}", options.iris_ci.display()))?;
    let mut text = String::from_utf8_lossy(&output.stdout).into_owned();
    text.push_str(&String::from_utf8_lossy(&output.stderr));
    if output.status.success() {
        Ok(text)
    } else {
        Err(format!(
            "iris-ci {} on {} failed: {}",
            args.join(" "),
            node.name,
            text.trim()
        ))
    }
}

/// Start the CPU on a node.
pub fn start_cpu(options: &Options, node: &Node) -> Result<(), String> {
    run_ci(options, node, &["start"]).map(|_| ())
}

/// Read a serial transcript, treating an absent file as empty.
pub fn read_serial(node: &Node) -> String {
    fs::read(&node.serial_log)
        .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
        .unwrap_or_default()
}

/// Wait for a marker in the serial transcript.
pub fn wait_serial(
    _options: &Options,
    running: &mut RunningNode,
    marker: &str,
    timeout: Duration,
) -> Result<String, String> {
    let deadline = Instant::now() + timeout;
    loop {
        let text = read_serial(&running.node);
        // The emulated UART can deliver a line in pieces.  A marker is only
        // ready once its line is newline-terminated; otherwise a caller can
        // parse a truncated `BOOT-READY` and fail the readiness check.
        if let Some(line) = complete_line(&text, marker) {
            return Ok(line);
        }
        if let Some(status) = running
            .child
            .try_wait()
            .map_err(|error| format!("wait for {}: {error}", running.node.name))?
        {
            return Err(format!(
                "{} exited ({status}) before {marker:?}\n--- serial ---\n{}",
                running.node.name,
                tail(&text, 40)
            ));
        }
        if Instant::now() >= deadline {
            return Err(format!(
                "{}: timed out waiting for {marker:?}\n--- serial ---\n{}",
                running.node.name,
                tail(&text, 40)
            ));
        }
        thread::sleep(Duration::from_millis(100));
    }
}

/// Boot markers extracted from one serial transcript.
#[derive(Clone, Debug, Default)]
pub struct BootFacts {
    /// The `BOOT-READY` line.
    pub boot_ready: Option<String>,
    /// The `mcp: listening` line.
    pub mcp: Option<String>,
    /// The `mesh: single node` line.
    pub mesh: Option<String>,
    /// The `mesh-transport: listening` line.
    pub mesh_transport_listen: Option<String>,
    /// The `mesh-transport: pass` line.
    pub mesh_transport_pass: Option<String>,
    /// Every `mesh-transport: disconnected` line.
    pub mesh_transport_disconnects: Vec<String>,
    /// The `mesh-join: enabled` line.
    pub mesh_join_enabled: Option<String>,
    /// The `mesh-join: disabled` line.
    pub mesh_join_disabled: Option<String>,
    /// The `mesh-route: enabled` line.
    pub mesh_route_enabled: Option<String>,
    /// The `mesh-route: disabled` line.
    pub mesh_route_disabled: Option<String>,
    /// The `mesh-migrate: enabled` line.
    pub mesh_migrate_enabled: Option<String>,
    /// The `mesh-migrate: disabled` line.
    pub mesh_migrate_disabled: Option<String>,
    /// Every `mesh-migrate:` line, in serial order.
    pub mesh_migrate_lines: Vec<String>,
    /// The `mesh-join: local` line from this boot.
    pub mesh_join_local: Option<String>,
    /// The first `mesh-join: request` line.
    pub mesh_join_request: Option<String>,
    /// Every `mesh-join: confirm` line (the root re-confirms on rejoin).
    pub mesh_join_confirms: Vec<String>,
    /// The first `mesh-join: converged` line.
    pub mesh_join_converged: Option<String>,
    /// Every `mesh-join: ack` line (the root received the member's ack).
    pub mesh_join_acks: Vec<String>,
    /// Every `mesh-join: ack-sent` line (the member sent its ack).
    pub mesh_join_acks_sent: Vec<String>,
    /// The first `mesh-join: duplicate` line.
    pub mesh_join_duplicate: Option<String>,
    /// The first `mesh-join: refused` line.
    pub mesh_join_refused: Option<String>,
    /// The first `mesh-join: timeout` line.
    pub mesh_join_timeout: Option<String>,
    /// The first `mesh-join: reject` line.
    pub mesh_join_reject: Option<String>,
    /// Every `mesh-homes:` / `mesh-home:` line, in serial order.
    pub mesh_homes_lines: Vec<String>,
    /// The `network: mac` line.
    pub network_mac: Option<String>,
    /// The DHCP address line.
    pub network_ip: Option<String>,
}

impl BootFacts {
    /// Extract the facts from a transcript.
    ///
    /// A trailing partial line is ignored: the serial UART can deliver a line
    /// in pieces, and a fact parsed from half a line is worse than no fact.
    pub fn from_serial(text: &str) -> Self {
        let cutoff = text.rfind('\n').map(|index| index + 1).unwrap_or(0);
        let text = &text[..cutoff];
        let find = |prefix: &str| {
            text.lines()
                .find(|line| line.trim_start().starts_with(prefix))
                .map(|line| line.trim().to_string())
        };
        Self {
            boot_ready: text
                .lines()
                .find(|line| line.trim_start().starts_with("BOOT-READY "))
                .map(|line| line.trim().to_string()),
            mcp: find("mcp: listening on port 80"),
            mesh: find("mesh: single node "),
            mesh_transport_listen: text
                .lines()
                .find(|line| line.contains("mesh-transport: listening"))
                .map(|line| line.trim().to_string()),
            mesh_transport_pass: text
                .lines()
                .find(|line| line.contains("mesh-transport: pass "))
                .map(|line| line.trim().to_string()),
            mesh_transport_disconnects: text
                .lines()
                .filter(|line| line.contains("mesh-transport: disconnected"))
                .map(|line| line.trim().to_string())
                .collect(),
            mesh_join_enabled: text
                .lines()
                .find(|line| line.contains("mesh-join: enabled "))
                .map(|line| line.trim().to_string()),
            mesh_join_disabled: text
                .lines()
                .find(|line| line.contains("mesh-join: disabled "))
                .map(|line| line.trim().to_string()),
            mesh_route_enabled: text
                .lines()
                .find(|line| line.contains("mesh-route: enabled "))
                .map(|line| line.trim().to_string()),
            mesh_route_disabled: text
                .lines()
                .find(|line| line.contains("mesh-route: disabled "))
                .map(|line| line.trim().to_string()),
            mesh_migrate_enabled: text
                .lines()
                .find(|line| line.contains("mesh-migrate: enabled "))
                .map(|line| line.trim().to_string()),
            mesh_migrate_disabled: text
                .lines()
                .find(|line| line.contains("mesh-migrate: disabled "))
                .map(|line| line.trim().to_string()),
            mesh_migrate_lines: text
                .lines()
                .filter(|line| line.contains("mesh-migrate: "))
                .map(|line| line.trim().to_string())
                .collect(),
            mesh_join_local: text
                .lines()
                .find(|line| line.contains("mesh-join: local "))
                .map(|line| line.trim().to_string()),
            mesh_join_request: text
                .lines()
                .find(|line| line.contains("mesh-join: request "))
                .map(|line| line.trim().to_string()),
            mesh_join_confirms: text
                .lines()
                .filter(|line| line.contains("mesh-join: confirm "))
                .map(|line| line.trim().to_string())
                .collect(),
            mesh_join_converged: text
                .lines()
                .find(|line| line.contains("mesh-join: converged "))
                .map(|line| line.trim().to_string()),
            mesh_join_acks: text
                .lines()
                .filter(|line| line.contains("mesh-join: ack "))
                .map(|line| line.trim().to_string())
                .collect(),
            mesh_join_acks_sent: text
                .lines()
                .filter(|line| line.contains("mesh-join: ack-sent "))
                .map(|line| line.trim().to_string())
                .collect(),
            mesh_join_duplicate: text
                .lines()
                .find(|line| line.contains("mesh-join: duplicate "))
                .map(|line| line.trim().to_string()),
            mesh_join_refused: text
                .lines()
                .find(|line| line.contains("mesh-join: refused "))
                .map(|line| line.trim().to_string()),
            mesh_join_timeout: text
                .lines()
                .find(|line| line.contains("mesh-join: timeout "))
                .map(|line| line.trim().to_string()),
            mesh_join_reject: text
                .lines()
                .find(|line| line.contains("mesh-join: reject "))
                .map(|line| line.trim().to_string()),
            mesh_homes_lines: text
                .lines()
                .filter(|line| line.contains("mesh-homes: ") || line.contains("mesh-home: "))
                .map(|line| line.trim().to_string())
                .collect(),
            network_mac: find("network: mac "),
            network_ip: text
                .lines()
                .find(|line| line.trim_start().starts_with("network: dhcp"))
                .map(|line| line.trim().to_string()),
        }
    }

    /// Check the readiness and identity markers against a node.
    pub fn verify(&self, node: &Node) -> Result<(), String> {
        let ready = self
            .boot_ready
            .as_deref()
            .ok_or_else(|| format!("{}: no BOOT-READY", node.name))?;
        for required in ["network=up", "mcp=listening", "mesh=single"] {
            if !ready.contains(required) {
                return Err(format!(
                    "{}: BOOT-READY lacks {required}: {ready}",
                    node.name
                ));
            }
        }
        if self.mcp.is_none() {
            return Err(format!("{}: no MCP listener line", node.name));
        }
        let mesh = self
            .mesh
            .as_deref()
            .ok_or_else(|| format!("{}: no mesh identity line", node.name))?;
        let observed = mesh
            .strip_prefix("mesh: single node ")
            .ok_or_else(|| format!("{}: malformed mesh line {mesh:?}", node.name))?;
        if !observed.eq_ignore_ascii_case(&node.node_id_string()) {
            return Err(format!(
                "{}: guest NodeId {observed} does not match derived {}",
                node.name,
                node.node_id_string()
            ));
        }
        let mac_line = self
            .network_mac
            .as_deref()
            .ok_or_else(|| format!("{}: no network MAC line", node.name))?;
        let mac_text = mac_line
            .strip_prefix("network: mac ")
            .ok_or_else(|| format!("{}: malformed MAC line {mac_line:?}", node.name))?;
        if !mac_text.eq_ignore_ascii_case(&node.mac_string()) {
            return Err(format!(
                "{}: guest MAC {mac_text} does not match derived {}",
                node.name,
                node.mac_string()
            ));
        }
        let ip_line = self
            .network_ip
            .as_deref()
            .ok_or_else(|| format!("{}: no DHCP address line", node.name))?;
        if !ip_line.contains(&node.guest.to_string()) {
            return Err(format!(
                "{}: DHCP line {ip_line:?} does not contain {}",
                node.name, node.guest
            ));
        }
        Ok(())
    }
}

/// Wait until both nodes have booted and their identities match.
pub fn wait_boot(options: &Options, nodes: &mut [RunningNode]) -> Result<Vec<BootFacts>, String> {
    let mut facts = Vec::new();
    for running in nodes.iter_mut() {
        wait_serial(options, running, "BOOT-READY", options.timeout)?;
        let text = read_serial(&running.node);
        let fact = BootFacts::from_serial(&text);
        fact.verify(&running.node)?;
        facts.push(fact);
    }
    Ok(facts)
}

/// Stop a node the way a failure does: SIGKILL, no clean shutdown.
///
/// A clean `quit` would let the peer observe an orderly close before the
/// heartbeat window elapses.  The failure slice needs the process to vanish
/// without protocol notice, so the survivor's detector, not the socket close,
/// decides when the member is suspect and failed.
pub fn kill_node(running: &mut RunningNode) -> Result<(), String> {
    running
        .child
        .kill()
        .map_err(|error| format!("kill {}: {error}", running.node.name))?;
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        match running.child.try_wait() {
            Ok(Some(_)) => return Ok(()),
            Ok(None) if Instant::now() < deadline => {
                thread::sleep(Duration::from_millis(100));
            }
            Ok(None) => return Err(format!("{} did not exit after SIGKILL", running.node.name)),
            Err(error) => return Err(format!("wait {}: {error}", running.node.name)),
        }
    }
}

/// Stop a node cleanly through its CI socket, then wait for the process.
pub fn stop_node(options: &Options, running: &mut RunningNode) -> Result<(), String> {
    let quit = run_ci(options, &running.node, &["quit"]);
    if let Err(error) = &quit {
        if options.verbose {
            eprintln!("{}: clean quit failed: {error}", running.node.name);
        }
    }
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        match running.child.try_wait() {
            Ok(Some(_)) => return Ok(()),
            Ok(None) if Instant::now() < deadline => {
                thread::sleep(Duration::from_millis(100));
            }
            Ok(None) => {
                let _ = running.child.kill();
                let _ = running.child.wait();
                return Err(format!("{}: quit timed out; killed", running.node.name));
            }
            Err(error) => return Err(format!("{}: wait: {error}", running.node.name)),
        }
    }
}

/// Stop every running node, collecting the first error.
pub fn stop_nodes(options: &Options, nodes: &mut [RunningNode]) -> Vec<String> {
    let mut errors = Vec::new();
    for running in nodes.iter_mut() {
        if let Err(error) = stop_node(options, running) {
            errors.push(error);
        }
    }
    errors
}

/// Find a newline-terminated line containing `marker`.
fn complete_line(text: &str, marker: &str) -> Option<String> {
    text.split_inclusive('\n')
        .filter(|line| line.ends_with('\n'))
        .find(|line| line.contains(marker))
        .map(|line| line.trim().to_string())
}

/// The last `count` lines of a transcript.
pub fn tail(text: &str, count: usize) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let start = lines.len().saturating_sub(count);
    lines[start..].join("\n")
}

/// Run a git query in the repository.
pub fn git(repo: &Path, args: &[&str]) -> String {
    match Command::new("git").arg("-C").arg(repo).args(args).output() {
        Ok(output) if output.status.success() => {
            String::from_utf8_lossy(&output.stdout).trim().to_string()
        }
        _ => "unknown".into(),
    }
}

/// Copy a file, ignoring a missing source.
pub fn copy_if_present(from: &Path, to: &Path) -> std::io::Result<()> {
    match fs::copy(from, to) {
        Ok(_) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

/// Write a text file, creating parent directories.
pub fn write_text(path: &Path, text: &str) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| format!("create {}: {error}", parent.display()))?;
    }
    let mut file =
        fs::File::create(path).map_err(|error| format!("create {}: {error}", path.display()))?;
    file.write_all(text.as_bytes())
        .map_err(|error| format!("write {}: {error}", path.display()))
}

/// Collect the derived values as a map for evidence and printing.
pub fn node_table(nodes: &[Node]) -> String {
    let mut out = String::new();
    out.push_str("name instance state_dir ci_socket monitor mac node_id nat_subnet guest gateway telnet http mesh\n");
    for node in nodes {
        out.push_str(&format!(
            "{} {} {} {} {} {} {} {} {} {} {} {} {}\n",
            node.name,
            node.instance,
            node.state_dir.display(),
            node.ci_socket.display(),
            node.monitor,
            node.mac_string(),
            node.node_id_string(),
            node.nat_subnet,
            node.guest,
            node.gateway,
            node.telnet_port,
            node.http_port,
            node.mesh_port
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn subnet_is_gateway_one_and_guest_two() {
        let (gateway, guest) = subnet_hosts("192.168.7.0/24").unwrap();
        assert_eq!(gateway, Ipv4Addr::new(192, 168, 7, 1));
        assert_eq!(guest, Ipv4Addr::new(192, 168, 7, 2));
    }

    #[test]
    fn bad_subnet_is_rejected() {
        assert!(subnet_hosts("192.168.7.4/24").is_err());
        assert!(subnet_hosts("192.168.7.0/16").is_err());
    }

    #[test]
    fn ports_are_disjoint() {
        let options = Options {
            action: Action::PrintConfig,
            repo: PathBuf::from("."),
            work: PathBuf::from("."),
            iris: PathBuf::from("iris"),
            iris_ci: PathBuf::from("iris-ci"),
            rom: PathBuf::from("rom"),
            module: PathBuf::from("module"),
            base_port: 9100,
            nodes: 2,
            hub: false,
            timeout: Duration::from_secs(1),
            fault: Fault::None,
            keep: false,
            verbose: false,
        };
        assert_eq!(options.port_base(0), 9100);
        assert_eq!(options.port_base(1), 9200);
        assert_eq!(options.telnet_port(0), 10100);
        assert_eq!(options.telnet_port(1), 10110);
        assert_eq!(options.http_port(0), 10101);
        assert_eq!(options.http_port(1), 10111);
        assert_eq!(options.mesh_port(0), 10102);
        assert_eq!(options.mesh_port(1), 10112);
        // The monitor/serial block for the last of eight nodes ends at
        // base + 702 and the forward range starts at base + 1000.
        assert!(options.forward_base(7) > options.port_base(7).saturating_add(2));
    }

    #[test]
    fn node_id_is_the_mac_then_zeroes() {
        let node = Node {
            name: "n0".into(),
            instance: 0,
            state_dir: PathBuf::new(),
            config: PathBuf::new(),
            ci_socket: PathBuf::new(),
            serial_log: PathBuf::new(),
            monitor: 0,
            mac: [0x08, 0x00, 0x69, 0x4d, 0x32, 0x00],
            node_id: [
                0x08, 0x00, 0x69, 0x4d, 0x32, 0x00, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            ],
            nat_subnet: "192.168.0.0/24".into(),
            gateway: "192.168.0.1".parse().unwrap(),
            guest: "192.168.0.2".parse().unwrap(),
            telnet_port: 9700,
            http_port: 9701,
            mesh_port: 9702,
            port_base: 9100,
        };
        assert_eq!(node.node_id_string(), "0800694D320000000000000000000000");
    }

    #[test]
    fn complete_line_ignores_a_partial_tail() {
        let text = "BOOT-READY v=1 kernel=up network=up\nBOOT-READY v=1 kernel=up net";
        assert_eq!(
            complete_line(text, "BOOT-READY").as_deref(),
            Some("BOOT-READY v=1 kernel=up network=up")
        );
        assert_eq!(complete_line("BOOT-READY v=1 ker", "BOOT-READY"), None);
        let facts = BootFacts::from_serial(text);
        assert_eq!(
            facts.boot_ready.as_deref(),
            Some("BOOT-READY v=1 kernel=up network=up")
        );
    }

    #[test]
    fn lab_authority_key_is_valid_and_deterministic() {
        let first = AuthorityKeys::lab();
        let second = AuthorityKeys::lab();
        assert_eq!(first.public_hex, second.public_hex);
        assert_eq!(first.public_hex.len(), 64);
        assert_ne!(first.public_hex, AuthorityKeys::wrong().public_hex);
        let signing = ed25519_dalek::SigningKey::from_bytes(&crate::sha256::digest(
            b"irix7 M2 lab authority key v1",
        ));
        assert_eq!(
            first.public_hex,
            crate::sha256::hex(&signing.verifying_key().to_bytes())
        );
    }

    #[test]
    fn boot_facts_find_join_lines_after_the_prompt() {
        let text = "sash> mesh-join: enabled role=member seed_epoch=1 \
                    authority=08006995F50000000000000000000000 key=00\n\
                    mesh-join: local role=member epoch=1 members=1 digest=0011 \
                    snapshot=08006969B60100000000000000000000 state=pending\n\
                    mesh-join: request from=08006995F50000000000000000000000 \
                    incarnation=1 last_epoch=1 nonce=7\n\
                    mesh-join: converged role=member epoch=2 members=2 \
                    digest=50D2F9696E00259437DF3A5ACCEB0BEF \
                    snapshot=08006995F50000000000000000000000,08006969B60100000000000000000000\n\
                    mesh-join: ack from=08006969B60100000000000000000000 epoch=2 digest=50D2F9696E00259437DF3A5ACCEB0BEF\n";
        let facts = BootFacts::from_serial(text);
        assert!(facts
            .mesh_join_enabled
            .as_deref()
            .is_some_and(|line| line.contains("role=member")));
        assert!(facts
            .mesh_join_local
            .as_deref()
            .is_some_and(|line| line.contains("state=pending")));
        assert!(facts
            .mesh_join_request
            .as_deref()
            .is_some_and(|line| line.contains("last_epoch=1")));
        assert_eq!(facts.mesh_join_confirms.len(), 0);
        assert!(facts
            .mesh_join_converged
            .as_deref()
            .is_some_and(|line| line.contains("epoch=2")));
        assert_eq!(facts.mesh_join_acks.len(), 1);
    }

    #[test]
    fn boot_facts_find_home_lines_after_the_prompt() {
        let text = "sash> mesh-homes: state=current object_set=m2-v1 epoch=2 \
                    members=2 digest=2EDA13C5AC6F726132E0479DFE6045B7 count=1\n\
                    sash> mesh-home: object=00 home=0800694D320000000000000000000000 \
                    epoch=2 digest=2EDA13C5AC6F726132E0479DFE6045B7\n";
        let facts = BootFacts::from_serial(text);
        assert_eq!(facts.mesh_homes_lines.len(), 2);
        assert!(facts.mesh_homes_lines[0].contains("state=current"));
        assert!(facts.mesh_homes_lines[1].contains("object=00"));
    }

    #[test]
    fn boot_facts_find_mesh_transport_lines_after_the_prompt() {
        let text = "sash> mesh-transport: listening on port 7000\n\
                    mesh-transport: pass peer=0800694D320000000000000000000000 \
                    rx_frames=2 tx_frames=2 rejects=0\n";
        let facts = BootFacts::from_serial(text);
        assert_eq!(
            facts.mesh_transport_listen.as_deref(),
            Some("sash> mesh-transport: listening on port 7000")
        );
        assert!(facts
            .mesh_transport_pass
            .as_deref()
            .is_some_and(|line| line.contains("peer=0800694D320000000000000000000000")));
    }
}
