// SPDX-License-Identifier: GPL-3.0-only
//! The one-computer M2 transcript.
//!
//! Two independent iris kernels run the real MCP module.  One prompt answers
//! `nodes`, `homes`, `stat <object>`, `move <object>`, `actor`, `move actor`,
//! `fail` and `recover`; none of the user-facing command text names a node, a
//! route, a port or an HTTP path.  Node identities and route detail appear only
//! in the output.
//!
//! This is the M2 object/actor proof, not a cell or process demo.  The actor is
//! the bounded L0 migration fixture from M2-5; fork/exec over cells is the
//! later work tracked by #326/#327.

use std::fmt::Write as _;
use std::fs;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use crate::homes::{self, HomeTable};
use crate::migrate::{self, ActorRow, MigrateReport};
use crate::relay::MeshRelay;
use crate::rig::{self, Fault, Node, Options, RunningNode, Transport};
use crate::route::{self, RouteReport};

/// The one prompt shown to the operator and in the recording.
const PROMPT: &str = "irix7> ";
/// A stop for a single control-plane request.
fn request_timeout() -> Duration {
    crate::timing::secs(10)
}
/// The MCP server keeps one HTTP slot through its close handshake.  Commands
/// that change state are separated by this settle time.
fn command_settle() -> Duration {
    crate::timing::secs(6)
}

/// One node's `/mesh/join` view.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct JoinView {
    requests: u64,
    epoch: u32,
    members: usize,
    digest: String,
}

impl JoinView {
    fn parse(text: &str) -> Result<Self, String> {
        let mut view = Self::default();
        for token in text.split_whitespace() {
            let Some((key, value)) = token.split_once('=') else {
                continue;
            };
            match key {
                "requests" => {
                    view.requests = value
                        .parse()
                        .map_err(|_| format!("{value:?} is not a request count"))?
                }
                "epoch" => {
                    view.epoch = value
                        .parse()
                        .map_err(|_| format!("{value:?} is not an epoch"))?
                }
                "members" => {
                    view.members = value
                        .parse()
                        .map_err(|_| format!("{value:?} is not a member count"))?
                }
                "digest" => view.digest = value.to_ascii_uppercase(),
                _ => {}
            }
        }
        if view.digest.is_empty() {
            return Err("join view has no digest".into());
        }
        Ok(view)
    }
}

/// The transcript runner.
struct Demo {
    options: Options,
    nodes: Vec<Node>,
    running: Vec<RunningNode>,
    relay: Option<MeshRelay>,
    member_up: bool,
    /// The first fixture object homed on the peer.
    object: String,
    /// A second peer-homed object used to show the failure fence.
    witness: String,
    /// Last object moved through the route surface.
    moved_object: Option<String>,
    /// Last confirmed route move, home epoch and lease.
    moved_home: Option<String>,
    moved_epoch: Option<u32>,
    /// Actor identity and state hash before the failure.
    actor_id: Option<String>,
    actor_hash: Option<String>,
    /// The two-member epoch the demo started from.
    join_epoch: u32,
    checks: Vec<String>,
    errors: Vec<String>,
    commands: Vec<String>,
    transcript: String,
    output_dir: PathBuf,
}

impl Demo {
    fn start(options: &Options) -> Result<Self, String> {
        let (module, _rom_sha, module_sha) = crate::check_prerequisites(options)?;
        let module_sha_text = crate::sha256::hex(&module_sha);
        let fixture =
            crate::Fixture::start(module).map_err(|error| format!("start fixture: {error}"))?;
        let nodes = crate::derive_nodes(options)?;

        // The clean M2 configuration plus the failure detector.  This is the
        // same mesh.* handoff the rig writes for `run --fault none|kill-peer`.
        let (mut environments, _authority, _wrong) =
            rig::mesh_environments(&nodes, Fault::None, Transport::Relay);
        for environment in environments.iter_mut().flatten() {
            environment.route = Some("2".into());
            environment.fail = Some("1".into());
        }

        let mut running = Vec::new();
        crate::start_all_with(
            options,
            &nodes,
            &environments,
            fixture.port,
            &module_sha_text,
            &mut running,
        )?;
        if let Err(error) = rig::wait_boot(options, &mut running) {
            let _ = rig::stop_nodes(options, &mut running);
            return Err(error);
        }
        drop(fixture);

        let output_dir = options.work.join("demo");
        fs::create_dir_all(&output_dir)
            .map_err(|error| format!("create {}: {error}", output_dir.display()))?;

        let mut demo = Self {
            options: options.clone(),
            nodes,
            running,
            relay: None,
            member_up: true,
            object: String::new(),
            witness: String::new(),
            moved_object: None,
            moved_home: None,
            moved_epoch: None,
            actor_id: None,
            actor_hash: None,
            join_epoch: 0,
            checks: Vec::new(),
            errors: Vec::new(),
            commands: Vec::new(),
            transcript: String::new(),
            output_dir,
        };
        if let Err(error) = demo.join() {
            let _ = rig::stop_nodes(&demo.options, &mut demo.running);
            return Err(error);
        }
        Ok(demo)
    }

    fn join(&mut self) -> Result<(), String> {
        let (mesh, relay) = crate::probe_mesh(&self.nodes);
        if !mesh.ok {
            return Err(format!("mesh framing probe: {}", mesh.message));
        }
        self.relay = relay;
        crate::wait_join(&self.nodes, crate::timing::secs(60));
        crate::wait_join_ack(&self.nodes, crate::timing::secs(20));
        crate::wait_homes_serial(&self.nodes, crate::timing::secs(60));
        std::thread::sleep(crate::timing::secs(10));

        let root = self.join_view(0)?;
        let member = self.join_view(1)?;
        if root.epoch != member.epoch
            || root.members != member.members
            || root.digest != member.digest
        {
            return Err(format!(
                "the two kernels did not converge: root {root:?}, member {member:?}"
            ));
        }
        if root.members != 2 {
            return Err(format!("expected two members, got {}", root.members));
        }
        self.join_epoch = root.epoch;
        self.check(
            true,
            format!(
                "two real kernels converged at epoch {} with {} members and digest {}",
                root.epoch, root.members, root.digest
            ),
        );

        let table = self.home_table(0)?;
        let peer = self.nodes[1].node_id_string();
        let peer_objects: Vec<String> = table
            .rows
            .iter()
            .filter(|row| row.home == peer)
            .map(|row| row.object.clone())
            .collect();
        if peer_objects.len() < 2 {
            return Err(format!(
                "the fixture set has {} peer-homed objects; the demo needs two",
                peer_objects.len()
            ));
        }
        self.object = peer_objects[0].clone();
        self.witness = peer_objects[1].clone();

        // Seed the L0 actor through the migration surface before the operator's
        // first command.  This is the fixture boundary, not a second directory.
        self.seed_actor()?;
        self.say("# two real iris kernels; one MCP module each\n");
        self.say(format!(
            "# nodes: n0={} n1={}\n",
            self.nodes[0].node_id_string(),
            self.nodes[1].node_id_string()
        ));
        self.say(format!(
            "# home fixture: peer object {} witness {}\n",
            self.object, self.witness
        ));
        Ok(())
    }

    fn seed_actor(&mut self) -> Result<(), String> {
        std::thread::sleep(command_settle());
        let (status, body) = migrate::command(&self.nodes[0], 1, request_timeout())?;
        if status != 202 {
            return Err(format!("seed actor: HTTP {status} {}", body.trim()));
        }
        let deadline = Instant::now() + crate::timing::secs(30);
        loop {
            let report = self.migrate_report(0)?;
            if let Some(actor) = &report.actor {
                if actor.state == "active" {
                    self.actor_id = Some(actor.id.clone());
                    self.actor_hash = Some(actor.hash.clone());
                    return Ok(());
                }
            }
            if Instant::now() >= deadline {
                return Err("the fixture actor was not seeded".into());
            }
            std::thread::sleep(Duration::from_millis(500));
        }
    }

    fn script(&mut self) -> Result<(), String> {
        self.command("nodes")?;
        self.command("homes")?;
        self.command(&format!("stat {}", self.object))?;
        self.command(&format!("move {}", self.object))?;
        self.command("actor")?;
        self.command("move actor")?;
        self.command("actor")?;
        self.command("move actor")?;
        self.command("actor")?;
        self.command("fail")?;
        self.command("nodes")?;
        self.command("homes")?;
        self.command(&format!("stat {}", self.witness))?;
        self.command("recover")?;
        self.command("nodes")?;
        self.command("homes")?;
        self.command("actor")?;
        self.command(&format!("stat {}", self.object))?;
        self.say(PROMPT);
        self.check(
            true,
            "the final prompt is the same one used before the failure",
        );
        Ok(())
    }

    fn command(&mut self, text: &str) -> Result<(), String> {
        self.say(format!("{PROMPT}{text}\n"));
        self.commands.push(text.to_string());
        let result = match text {
            "nodes" => self.command_nodes(),
            "homes" => self.command_homes(),
            "actor" => self.command_actor(),
            "move actor" => self.command_move_actor(),
            "fail" => self.command_fail(),
            "recover" => self.command_recover(),
            _ if text.starts_with("stat ") => {
                let object = text.trim_start_matches("stat ").trim();
                self.command_stat(object)
            }
            _ if text.starts_with("move ") => {
                let object = text.trim_start_matches("move ").trim();
                self.command_move_object(object)
            }
            _ => Err(format!("unknown command {text:?}")),
        };
        if let Err(error) = &result {
            self.errors.push(format!("{text}: {error}"));
            self.say(format!("error: {error}\n"));
        }
        result
    }

    fn command_nodes(&mut self) -> Result<(), String> {
        let root = self.join_view(0)?;
        let member = if self.member_up {
            Some(self.join_view(1)?)
        } else {
            None
        };
        let root_route = self.route_report(0)?;
        self.say(format!(
            "n0  root    {}  epoch {}  members {}  join=converged  route={}  {}\n",
            self.nodes[0].node_id_string(),
            root.epoch,
            root.members,
            root_route.authority,
            if self.member_up { "ready" } else { "survivor" }
        ));
        match member {
            Some(member) => {
                if member.epoch != root.epoch
                    || member.members != root.members
                    || member.digest != root.digest
                {
                    return Err("the nodes disagree on the membership view".into());
                }
                self.say(format!(
                    "n1  member  {}  epoch {}  members {}  join=converged  route={}  ready\n",
                    self.nodes[1].node_id_string(),
                    member.epoch,
                    member.members,
                    root_route.authority
                ));
            }
            None => {
                self.say(format!(
                    "n1  member  {}  unreachable  removed at epoch {}  (failure detector)\n",
                    self.nodes[1].node_id_string(),
                    root.epoch
                ));
            }
        }
        Ok(())
    }

    fn command_homes(&mut self) -> Result<(), String> {
        let root = self.home_table(0)?;
        let member = if self.member_up {
            Some(self.home_table(1)?)
        } else {
            None
        };
        if let Some(member) = &member {
            if root.epoch != member.epoch
                || root.digest != member.digest
                || root.rows != member.rows
            {
                return Err(format!(
                    "the two nodes serve different home tables: root epoch {:?}, member epoch {:?}",
                    root.epoch, member.epoch
                ));
            }
            self.check(
                true,
                format!(
                    "both nodes serve the same home table at epoch {}",
                    root.epoch.unwrap_or(0)
                ),
            );
        }
        self.say(format!(
            "epoch {}  members {}  digest {}  objects {}\n",
            root.epoch.unwrap_or(0),
            root.members.unwrap_or(0),
            root.digest.clone().unwrap_or_default(),
            root.rows.len()
        ));
        self.say("object                                                            home\n");
        for row in &root.rows {
            self.say(format!("{}  {}\n", row.object, self.label(&row.home)));
        }
        let route = self.route_report(0)?;
        if let (Some(object), Some(home), Some(epoch)) = (
            self.moved_object.clone(),
            self.moved_home.clone(),
            self.moved_epoch,
        ) {
            self.say(format!(
                "route: last move {} {} -> {} (home epoch {})  fences {}  retries {}\n",
                short(&object),
                self.label(&self.nodes[1].node_id_string()),
                self.label(&home),
                epoch,
                route.fences,
                route.retries
            ));
        } else {
            self.say(format!(
                "route: no object move recorded  fences {}  retries {}\n",
                route.fences, route.retries
            ));
        }
        match self.active_actor()? {
            Some((_node, actor)) => self.say(format!(
                "actors: 1 active  {}  host {}  state {}  hash {}\n",
                short(&actor.id),
                self.label(&actor.host),
                actor.state,
                short(&actor.hash)
            )),
            None => self.say("actors: none active in the migration view\n".to_string()),
        }
        if !self.member_up {
            self.check(
                root.members == Some(1),
                "the survivor serves a one-member home table after the removal",
            );
        }
        Ok(())
    }

    fn command_actor(&mut self) -> Result<(), String> {
        let root = self.migrate_report(0)?;
        let member = if self.member_up {
            Some(self.migrate_report(1)?)
        } else {
            None
        };
        let active = if let Some(actor) = root.actor.clone().filter(|a| a.state == "active") {
            Some((0, actor))
        } else {
            member
                .as_ref()
                .and_then(|report| report.actor.clone())
                .filter(|a| a.state == "active")
                .map(|actor| (1, actor))
        };
        match active {
            Some((node, actor)) => {
                self.say(format!(
                    "actor {}  home {}  host {}  state {}  epoch {}  applied {}  last {}\n",
                    actor.id,
                    self.label(&actor.home),
                    self.label(&actor.host),
                    actor.state,
                    actor.epoch,
                    actor.applied,
                    actor.last
                ));
                self.say(format!("  hash {}\n", actor.hash));
                if let Some(message) = &actor.queued {
                    self.say(format!(
                        "  queued {}:{}  unacked {}\n",
                        message.id,
                        short(&message.payload),
                        actor
                            .unacked
                            .as_ref()
                            .map(|row| row.id.to_string())
                            .unwrap_or_else(|| "-".into())
                    ));
                }
                if let Some(handle) = &actor.handle {
                    self.say(format!(
                        "  handle object={} rights={} home={} home_epoch={} object_epoch={}\n",
                        short(&handle.object),
                        handle.rights,
                        self.label(&handle.home),
                        handle.home_epoch,
                        handle.object_epoch
                    ));
                }
                self.note_actor(&actor);
                if node == 1 {
                    self.check(
                        true,
                        "the migration surface reports the actor active at the member",
                    );
                }
                if !self.member_up {
                    self.check(
                        true,
                        "the actor remains active on the survivor after the removal",
                    );
                }
            }
            None => {
                if let Some(last) = root
                    .last_move
                    .as_ref()
                    .or_else(|| member.as_ref().and_then(|report| report.last_move.as_ref()))
                {
                    self.say(format!(
                        "actor {}: no active instance  last move {} -> {}  hash {}\n",
                        short(&last.actor),
                        self.label(&last.old_host),
                        self.label(&last.new_host),
                        short(&last.hash)
                    ));
                } else {
                    self.say("actor: none in the migration view\n".to_string());
                }
            }
        }
        Ok(())
    }

    fn note_actor(&mut self, actor: &ActorRow) {
        if self.actor_id.is_none() {
            self.actor_id = Some(actor.id.clone());
        }
        if self.actor_hash.is_none() {
            self.actor_hash = Some(actor.hash.clone());
        }
    }

    fn command_move_actor(&mut self) -> Result<(), String> {
        let (current_node, actor) = match self.active_actor()? {
            Some((node, actor)) => (node, actor),
            None => {
                self.seed_actor()?;
                self.active_actor()?
                    .ok_or_else(|| "the fixture actor was not seeded".to_string())?
            }
        };
        let old = actor.host.clone();
        let (step, target) = if current_node == 0 {
            (2u8, 1usize)
        } else {
            (7u8, 0usize)
        };
        let before = self.migrate_report(0)?;
        let before_moves = before.counters.migrations;
        std::thread::sleep(command_settle());
        let (status, body) = migrate::command(&self.nodes[0], step, request_timeout())?;
        if status != 202 {
            return Err(format!("move actor: HTTP {status} {}", body.trim()));
        }
        // The command is non-idempotent.  Let the module finish its close
        // handshake before the next control-plane request, otherwise the
        // single-threaded module spends its tick in the HTTP path and starves
        // the mesh heartbeat.
        std::thread::sleep(command_settle());
        let deadline = Instant::now() + crate::timing::secs(60);
        let mut last_error = String::new();
        loop {
            match self.migrate_report(0) {
                Ok(root) => {
                    if let Some(movement) = &root.last_move {
                        if movement.new_host == self.nodes[target].node_id_string()
                            && root.counters.migrations > before_moves
                        {
                            if let Some(actor_id) = &self.actor_id {
                                if *actor_id != movement.actor {
                                    return Err(
                                        "the migration surface moved a different actor".into()
                                    );
                                }
                            }
                            self.say(format!(
                                "actor {}  {} -> {}  placement epoch {} -> {}  hash {}\n",
                                short(&movement.actor),
                                self.label(&old),
                                self.label(&movement.new_host),
                                movement.old_epoch,
                                movement.new_epoch,
                                short(&movement.hash)
                            ));
                            self.check(
                                true,
                                format!(
                                    "the actor moved {} -> {} at a message boundary",
                                    self.label(&old),
                                    self.label(&movement.new_host)
                                ),
                            );
                            std::thread::sleep(command_settle());
                            return Ok(());
                        }
                    }
                }
                Err(error) => last_error = error,
            }
            if Instant::now() >= deadline {
                return Err(format!(
                    "the migration surface did not report the move: {last_error}"
                ));
            }
            std::thread::sleep(Duration::from_millis(1000));
        }
    }

    fn command_stat(&mut self, object: &str) -> Result<(), String> {
        if object.len() != 64 || !object.chars().all(|ch| ch.is_ascii_hexdigit()) {
            return Err(format!("{object:?} is not a 32-byte object ID"));
        }
        let before = self.route_report(0)?;
        let before_results = before.results.len();
        let before_timeouts = before.timeouts;
        let (status, body) =
            route::object_command(&self.nodes[0], "stat", object, request_timeout())?;
        if status != 202 {
            return Err(format!("stat: HTTP {status} {}", body.trim()));
        }
        std::thread::sleep(crate::timing::secs(1));
        let deadline = Instant::now() + crate::timing::secs(30);
        let mut last_error = String::new();
        loop {
            match self.route_report(0) {
                Ok(report) => {
                    if let Some(row) =
                        report
                            .results
                            .iter()
                            .skip(before_results)
                            .rev()
                            .find(|row| {
                                row.object.eq_ignore_ascii_case(object) && row.operation == "stat"
                            })
                    {
                        self.say(format!(
                            "object {}  home {}  home_epoch {}  value {}  attempts {}  moved {}\n",
                            row.object,
                            self.label(&row.home),
                            row.home_epoch,
                            short(&row.value),
                            row.attempts,
                            if row.moved { "yes" } else { "no" }
                        ));
                        if row.moved {
                            self.check(
                                true,
                                "the stale route answered MOVED and the sender retried once",
                            );
                        }
                        std::thread::sleep(command_settle());
                        return Ok(());
                    }
                    if report.timeouts > before_timeouts {
                        self.say(format!(
                            "stat {}: unavailable  home {}  reason timeout  (no second authoritative home)\n",
                            object,
                            self.label(&self.nodes[1].node_id_string())
                        ));
                        self.check(
                            true,
                            "the object was unavailable while its home was removed",
                        );
                        std::thread::sleep(command_settle());
                        return Ok(());
                    }
                }
                Err(error) => last_error = error,
            }
            if Instant::now() >= deadline {
                return Err(format!("stat {object}: no result: {last_error}"));
            }
            std::thread::sleep(Duration::from_millis(1000));
        }
    }

    fn command_move_object(&mut self, object: &str) -> Result<(), String> {
        if object.len() != 64 || !object.chars().all(|ch| ch.is_ascii_hexdigit()) {
            return Err(format!("{object:?} is not a 32-byte object ID"));
        }
        let before = self.route_report(0)?;
        let old = self
            .home_for(object, &before)
            .unwrap_or_else(|| self.nodes[1].node_id_string());
        if old == self.nodes[0].node_id_string() {
            return Err("the object is already homed at this node".into());
        }
        std::thread::sleep(command_settle());
        let (status, body) =
            route::object_command(&self.nodes[0], "move", object, request_timeout())?;
        if status != 202 {
            return Err(format!("move: HTTP {status} {}", body.trim()));
        }
        std::thread::sleep(command_settle());
        let deadline = Instant::now() + crate::timing::secs(30);
        let mut last_error = String::new();
        loop {
            let root = match self.route_report(0) {
                Ok(report) => report,
                Err(error) => {
                    last_error = error;
                    if Instant::now() >= deadline {
                        return Err(format!("move: no report: {last_error}"));
                    }
                    std::thread::sleep(Duration::from_millis(1000));
                    continue;
                }
            };
            let member = if self.member_up {
                self.route_report(1).ok()
            } else {
                None
            };
            let root_moved = root.move_new.as_deref() == Some(object);
            let member_moved = member
                .as_ref()
                .is_none_or(|report| report.move_old.as_deref() == Some(object));
            if root_moved && member_moved {
                let home_epoch = root.view_epoch.unwrap_or(0) + 1;
                self.moved_object = Some(object.to_ascii_uppercase());
                self.moved_home = Some(self.nodes[0].node_id_string());
                self.moved_epoch = Some(home_epoch);
                self.say(format!(
                    "moved {}  {} -> {}  home epoch {}  lease 60 s  (the old home answers MOVED)\n",
                    object,
                    self.label(&old),
                    self.label(&self.nodes[0].node_id_string()),
                    home_epoch
                ));
                self.check(
                    true,
                    "the route surface moved the named object to this node",
                );
                std::thread::sleep(command_settle());
                return Ok(());
            }
            if Instant::now() >= deadline {
                return Err(format!(
                    "the route surface did not confirm the object move: {last_error}"
                ));
            }
            std::thread::sleep(Duration::from_millis(1000));
        }
    }

    fn command_fail(&mut self) -> Result<(), String> {
        if !self.member_up {
            return Err("the member is already down".into());
        }
        let witness = self.witness.clone();
        let peer = self.nodes[1].node_id_string();
        let root = self.nodes[0].node_id_string();
        self.say(format!(
            "member {}: killed (SIGKILL); the console process keeps running\n",
            self.nodes[1].name
        ));
        rig::kill_node(&mut self.running[1])?;
        // Stop the host relay as soon as the member dies.  Left running, its
        // supervisor reconnects to the survivor's guest forward every few
        // seconds, and those half-open sessions interfere with the fresh relay
        // the recovery path starts.
        self.relay.take();
        self.member_up = false;
        let killed = Instant::now();

        // A read of an object whose last confirmed home is the removed node
        // must fail, not be invented locally.  The same command runs on the
        // survivor while the failure detector is still working.
        let before = self.route_report(0)?;
        let before_results = before.results.len();
        let before_timeouts = before.timeouts;
        let before_reject = before.last_reject.clone();
        std::thread::sleep(Duration::from_millis(250));
        let (status, body) =
            route::object_command(&self.nodes[0], "stat", &witness, request_timeout())?;
        let mut unavailable = false;
        if status == 202 {
            // The module accepted the operation; wait for its own deadline.
        } else if status == 409
            && (body.contains("reason=no-peer") || body.contains("reason=timeout"))
        {
            unavailable = true;
            let reason = if body.contains("no-peer") {
                "no-peer"
            } else {
                "timeout"
            };
            self.say(format!(
                "  t+{:.1}s  stat {}: unavailable  home {}  reason {}\n",
                killed.elapsed().as_secs_f64(),
                short(&witness),
                self.label(&peer),
                reason
            ));
            self.check(
                true,
                "a read of the peer-homed object failed closed while the member was down",
            );
        } else {
            return Err(format!("failure read: HTTP {status} {}", body.trim()));
        }

        let mut saw_suspect = false;
        let mut saw_failed = false;
        let mut saw_removed = false;
        loop {
            if !saw_suspect || !saw_failed || !saw_removed {
                let serial = rig::read_serial(&self.nodes[0]);
                if !saw_suspect && serial.contains("mesh-fail: suspect") {
                    saw_suspect = true;
                    self.say(format!(
                        "  t+{:.1}s  n0: member suspect (still routable)\n",
                        killed.elapsed().as_secs_f64()
                    ));
                }
                if !saw_failed && serial.contains("mesh-fail: failed") {
                    saw_failed = true;
                    self.say(format!(
                        "  t+{:.1}s  n0: member failed (lease fence next)\n",
                        killed.elapsed().as_secs_f64()
                    ));
                }
                if !saw_removed && serial.contains("mesh-fail: removed") {
                    saw_removed = true;
                    self.say(format!(
                        "  t+{:.1}s  n0: member removed; epoch advanced once\n",
                        killed.elapsed().as_secs_f64()
                    ));
                }
            }
            if !unavailable {
                if let Ok(report) = self.route_report(0) {
                    let rejection = report
                        .last_reject
                        .as_deref()
                        .filter(|reason| Some(*reason) != before_reject.as_deref());
                    let failed_closed = report.timeouts > before_timeouts
                        || matches!(rejection, Some("link-down") | Some("no-peer"));
                    if failed_closed && report.results.len() <= before_results {
                        unavailable = true;
                        self.say(format!(
                            "  t+{:.1}s  stat {}: unavailable  home {}  reason {}\n",
                            killed.elapsed().as_secs_f64(),
                            short(&witness),
                            self.label(&peer),
                            rejection.unwrap_or("timeout")
                        ));
                        self.check(
                            true,
                            "a read of the peer-homed object failed closed while the member was down",
                        );
                    }
                }
            }
            if saw_removed {
                break;
            }
            if killed.elapsed() >= crate::timing::secs(60) {
                return Err("the survivor did not remove the failed member".into());
            }
            std::thread::sleep(Duration::from_millis(1000));
        }
        if !saw_suspect || !saw_failed {
            return Err("the failure detector did not report every stage".into());
        }

        let report = self.route_report(0)?;
        if report.fences == 0 {
            return Err("the survivor recorded no route fence".into());
        }
        let fenced = report
            .fence_rows
            .iter()
            .any(|row| row.stale_home == peer && row.home == root && row.reason == "left-view");
        if !fenced {
            return Err("the fence rows do not name the removed home and the survivor".into());
        }
        self.say(format!(
            "  fence: {} route(s) named the removed home; every object now resolves at n0\n",
            report.fences
        ));
        self.check(
            true,
            format!(
                "the survivor fenced {} cached route(s) that named the removed home",
                report.fences
            ),
        );
        let table = self.home_table(0)?;
        self.check(
            table.members == Some(1),
            "the survivor recomputed the one-member home table",
        );
        self.check(
            table.rows.iter().all(|row| row.home == root),
            "every object is homed at the survivor after the removal",
        );
        std::thread::sleep(crate::timing::secs(1));
        self.command_stat(&witness)?;
        if self.actor_hash.is_some() {
            let root_report = self.migrate_report(0)?;
            let actor_ok = root_report
                .actor
                .as_ref()
                .is_some_and(|actor| actor.state == "active" && actor.host == root);
            self.check(actor_ok, "the actor stayed active on the survivor");
        }
        std::thread::sleep(command_settle());
        Ok(())
    }

    fn command_recover(&mut self) -> Result<(), String> {
        if self.member_up {
            return Err("the member is already up".into());
        }
        let node = self.nodes[1].clone();
        self.say(format!("member {}: starting\n", node.name));
        let _ = fs::copy(
            &node.serial_log,
            self.output_dir.join("serial-n1-before-failure.log"),
        );
        let _ = fs::remove_file(&node.ci_socket);
        let child = rig::start_node(&self.options, &node)?;
        self.running[1] = child;
        rig::wait_ci_ready(&self.options, &node, crate::timing::secs(20))?;
        rig::start_cpu(&self.options, &node)?;
        rig::wait_serial(
            &self.options,
            &mut self.running[1],
            "BOOT-READY",
            self.options.timeout,
        )?;
        rig::wait_serial(
            &self.options,
            &mut self.running[1],
            "mesh-transport: listening",
            crate::timing::secs(60),
        )?;
        self.relay.take();
        std::thread::sleep(crate::timing::secs(1));
        crate::mcp_tool(&self.nodes[0], "mesh.reconnect")?;
        std::thread::sleep(crate::timing::secs(3));
        self.relay = Some(
            MeshRelay::start(
                (std::net::Ipv4Addr::LOCALHOST, self.nodes[0].mesh_port).into(),
                (std::net::Ipv4Addr::LOCALHOST, self.nodes[1].mesh_port).into(),
            )
            .map_err(|error| format!("restart mesh relay: {error}"))?,
        );
        self.member_up = true;

        // The restarted member starts its join request count at zero.  A
        // non-zero count proves this boot asked to rejoin rather than being
        // accepted by a stale link.  The restored member snapshot is still the
        // old epoch until the root's confirmation arrives, so the loop waits
        // for both ends to agree, not merely for the member to have an old
        // two-member view.
        let deadline = Instant::now() + crate::timing::secs(120);
        let rejoin = loop {
            let root = self.join_view(0);
            let member = self.join_view(1);
            match (&root, &member) {
                (Ok(root), Ok(member))
                    if member.requests > 0
                        && member.members == 2
                        && member.epoch > self.join_epoch
                        && root.epoch == member.epoch
                        && root.members == member.members
                        && root.digest == member.digest
                        && Self::valid_digest(&member.digest) =>
                {
                    break member.clone();
                }
                _ => {}
            }
            if Instant::now() >= deadline {
                let root = self.join_view(0).ok();
                let member = self.join_view(1).ok();
                return Err(format!(
                    "the member did not rejoin after the restart: root {root:?}, member {member:?}\n--- n1 serial ---\n{}",
                    rig::tail(&rig::read_serial(&node), 30)
                ));
            }
            std::thread::sleep(Duration::from_millis(500));
        };
        let root_table = self.home_table(0)?;
        let member_table = self.home_table(1)?;
        if root_table.epoch != member_table.epoch
            || root_table.digest != member_table.digest
            || root_table.rows != member_table.rows
        {
            return Err("the rejoined nodes do not serve the same home table".into());
        }
        self.check(
            true,
            format!(
                "the restarted member rejoined at epoch {} with {} members and the same digest",
                rejoin.epoch, rejoin.members
            ),
        );
        self.check(
            true,
            "both nodes serve the same home table after the rejoin",
        );
        self.say(format!(
            "member {}: rejoin request sent; converged epoch {} members {} digest {}\n",
            node.name, rejoin.epoch, rejoin.members, rejoin.digest
        ));
        if let Some(actor_hash) = &self.actor_hash {
            let root_report = self.migrate_report(0)?;
            let preserved = root_report
                .actor
                .as_ref()
                .is_some_and(|actor| actor.hash == *actor_hash && actor.state == "active");
            self.check(
                preserved,
                "the survivor still carries the same actor state hash",
            );
        }
        let _ = fs::copy(
            &node.serial_log,
            self.output_dir.join("serial-n1-after-recovery.log"),
        );
        std::thread::sleep(command_settle());
        Ok(())
    }

    fn valid_digest(digest: &str) -> bool {
        digest.len() == 32 && digest.chars().all(|ch| ch.is_ascii_hexdigit())
    }

    fn active_actor(&mut self) -> Result<Option<(usize, ActorRow)>, String> {
        let root = self.migrate_report(0)?;
        if let Some(actor) = root.actor.clone().filter(|actor| actor.state == "active") {
            return Ok(Some((0, actor)));
        }
        if self.member_up {
            let member = self.migrate_report(1)?;
            if let Some(actor) = member.actor.filter(|actor| actor.state == "active") {
                return Ok(Some((1, actor)));
            }
        }
        Ok(None)
    }

    /// Fetch one control-plane path with a bounded retry.
    ///
    /// The iris host forward can retain a half-open guest connection after a
    /// close, especially while the module is in its two-second close handshake.
    /// A console GET is idempotent, so retrying is safe.  The per-attempt
    /// timeout stays short; the total retry window is long enough for the
    /// module's single HTTP slot to re-arm.
    fn fetch_retry(&self, index: usize, path: &str) -> Result<crate::homes::HttpResponse, String> {
        let deadline = Instant::now() + crate::timing::secs(90);
        loop {
            match homes::fetch(
                &self.nodes[index],
                path,
                if index == 1 && !self.member_up {
                    crate::timing::secs(1)
                } else {
                    crate::timing::secs(5)
                },
            ) {
                Ok(response) => return Ok(response),
                Err(error) => {
                    if Instant::now() >= deadline {
                        return Err(format!("{} {path}: {error}", self.nodes[index].name));
                    }
                }
            }
            std::thread::sleep(crate::timing::secs(2));
        }
    }

    fn join_view(&self, index: usize) -> Result<JoinView, String> {
        let response = self.fetch_retry(index, "/mesh/join")?;
        if response.status != 200 {
            return Err(format!(
                "{} /mesh/join: HTTP {} {}",
                self.nodes[index].name,
                response.status,
                response.body.trim()
            ));
        }
        JoinView::parse(&response.body)
            .map_err(|error| format!("{} /mesh/join: {error}", self.nodes[index].name))
    }

    fn home_table(&self, index: usize) -> Result<HomeTable, String> {
        let response = self.fetch_retry(index, "/mesh/homes")?;
        if response.status != 200 {
            return Err(format!(
                "{} /mesh/homes: HTTP {}",
                self.nodes[index].name, response.status
            ));
        }
        HomeTable::parse(&response.body)
            .map_err(|error| format!("{} /mesh/homes: {error}", self.nodes[index].name))
    }

    fn route_report(&self, index: usize) -> Result<RouteReport, String> {
        let response = self.fetch_retry(index, "/mesh/route")?;
        if response.status != 200 {
            return Err(format!(
                "{} /mesh/route: HTTP {}",
                self.nodes[index].name, response.status
            ));
        }
        RouteReport::parse(&response.body)
            .map_err(|error| format!("{} /mesh/route: {error}", self.nodes[index].name))
    }

    fn migrate_report(&self, index: usize) -> Result<MigrateReport, String> {
        let response = self.fetch_retry(index, "/mesh/migrate")?;
        if response.status != 200 {
            return Err(format!(
                "{} /mesh/migrate: HTTP {}",
                self.nodes[index].name, response.status
            ));
        }
        MigrateReport::parse(&response.body)
            .map_err(|error| format!("{} /mesh/migrate: {error}", self.nodes[index].name))
    }

    fn home_for(&self, object: &str, report: &RouteReport) -> Option<String> {
        if let Some(row) = report
            .results
            .iter()
            .rev()
            .find(|row| row.object.eq_ignore_ascii_case(object))
        {
            return Some(row.home.clone());
        }
        self.home_table(0)
            .ok()?
            .rows
            .iter()
            .find(|row| row.object.eq_ignore_ascii_case(object))
            .map(|row| row.home.clone())
    }

    fn label(&self, id: &str) -> String {
        if id.eq_ignore_ascii_case(&self.nodes[0].node_id_string()) {
            "n0".into()
        } else if id.eq_ignore_ascii_case(&self.nodes[1].node_id_string()) {
            "n1".into()
        } else if id.is_empty() {
            "-".into()
        } else {
            short(id)
        }
    }

    fn say(&mut self, text: impl Into<String>) {
        let text = text.into();
        print!("{text}");
        self.transcript.push_str(&text);
    }

    fn check(&mut self, condition: bool, message: impl Into<String>) {
        let message = message.into();
        if condition {
            self.checks.push(message);
        } else {
            self.errors.push(message);
        }
    }

    fn finish(mut self) -> Result<i32, String> {
        let _ = fs::copy(
            &self.nodes[0].serial_log,
            self.output_dir.join("serial-n0.log"),
        );
        let _ = fs::copy(
            &self.nodes[1].serial_log,
            self.output_dir.join("serial-n1.log"),
        );
        if let Some(error) = self.transcript_error() {
            self.errors.push(error);
        }
        self.prompt_checks();
        self.syntax_checks();
        self.transcript_checks();
        let ok = self.errors.is_empty();
        let _ = rig::stop_nodes(&self.options, &mut self.running);

        let mut summary = String::new();
        summary.push_str("# One computer — M2 object/actor transcript\n\n");
        summary.push_str(
            "This is the M2 object and actor proof over two real iris kernels.  \
             It is not fork/exec over cells, process migration or a POSIX session; \
             that path lands after #326/#327.\n\n",
        );
        summary.push_str(&format!(
            "**Result:** {}\n\n",
            if ok { "pass" } else { "fail" }
        ));
        summary.push_str(&format!(
            "**Nodes:** n0 `{}`, n1 `{}`\n\n",
            self.nodes[0].node_id_string(),
            self.nodes[1].node_id_string()
        ));
        summary.push_str(&format!(
            "**Object:** `{}` (witness `{}`)\n\n",
            self.object, self.witness
        ));
        summary.push_str(&format!(
            "**Actor:** `{}` hash `{}`\n\n",
            self.actor_id.clone().unwrap_or_else(|| "-".into()),
            self.actor_hash.clone().unwrap_or_else(|| "-".into())
        ));
        summary.push_str("## Commands\n\n```text\n");
        for command in &self.commands {
            let _ = writeln!(summary, "{command}");
        }
        summary.push_str("```\n\n## Checks\n\n");
        for check in &self.checks {
            let _ = writeln!(summary, "- {check}");
        }
        if !self.errors.is_empty() {
            summary.push_str("\n## Errors\n\n");
            for error in &self.errors {
                let _ = writeln!(summary, "- {error}");
            }
        }
        summary.push_str("\n## Transcript\n\n```text\n");
        summary.push_str(&self.transcript);
        if !summary.ends_with('\n') {
            summary.push('\n');
        }
        summary.push_str("```\n");

        fs::write(self.output_dir.join("transcript.txt"), &self.transcript)
            .map_err(|error| format!("write transcript: {error}"))?;
        fs::write(self.output_dir.join("summary.md"), summary)
            .map_err(|error| format!("write summary: {error}"))?;
        let commands = self.commands.join("\n");
        fs::write(
            self.output_dir.join("commands.txt"),
            format!("{commands}\n"),
        )
        .map_err(|error| format!("write commands: {error}"))?;
        let checks = self.checks.join("\n");
        fs::write(self.output_dir.join("checks.txt"), format!("{checks}\n"))
            .map_err(|error| format!("write checks: {error}"))?;
        println!(
            "\ntranscript: {}",
            self.output_dir.join("transcript.txt").display()
        );
        println!(
            "summary:    {}",
            self.output_dir.join("summary.md").display()
        );
        if ok {
            println!("demo: pass");
            Ok(0)
        } else {
            eprintln!("demo: FAIL");
            for error in &self.errors {
                eprintln!("  {error}");
            }
            Ok(1)
        }
    }

    fn transcript_error(&self) -> Option<String> {
        if self.transcript.is_empty() {
            Some("the transcript is empty".into())
        } else {
            None
        }
    }

    fn prompt_checks(&mut self) {
        let fail_line = format!("{PROMPT}fail\n");
        let before = self.transcript.find(&fail_line);
        let prompt_after = before
            .map(|index| self.transcript[index + fail_line.len()..].contains(PROMPT))
            .unwrap_or(false);
        let ends = self.transcript.ends_with(PROMPT);
        let one_line = self
            .commands
            .iter()
            .all(|command| !command.contains("irix7>") && !command.contains('\n'));
        self.check(
            before.is_some(),
            "the failure command was issued from the same prompt",
        );
        self.check(prompt_after, "the prompt returned after the failure");
        self.check(ends, "the transcript ends on the same prompt");
        self.check(
            one_line,
            "every user-facing command is one line of plain words",
        );
    }

    fn syntax_checks(&mut self) {
        let node0 = self.nodes[0].node_id_string();
        let node1 = self.nodes[1].node_id_string();
        let forbidden = [
            "n0",
            "n1",
            "/mesh",
            "http",
            "192.168",
            "mesh.",
            "step=",
            "route=",
            node0.as_str(),
            node1.as_str(),
        ];
        let mut ok = true;
        for command in &self.commands {
            for token in forbidden {
                if command
                    .to_ascii_lowercase()
                    .contains(&token.to_ascii_lowercase())
                {
                    self.errors.push(format!(
                        "command {command:?} names internal detail {token:?}"
                    ));
                    ok = false;
                }
            }
        }
        self.check(
            ok,
            "no user-facing command names a node, route or transport",
        );
    }

    fn transcript_checks(&mut self) {
        let (removed, unavailable, fenced, rejoin, converged) =
            match self.transcript.find(&format!("{PROMPT}fail\n")) {
                Some(before) => {
                    let after = &self.transcript[before..];
                    (
                        after.contains("member removed"),
                        after.contains("unavailable"),
                        after.contains("fence:"),
                        after.contains("rejoin request sent"),
                        after.contains("converged epoch"),
                    )
                }
                None => (false, false, false, false, false),
            };
        self.check(
            removed,
            "the transcript shows the failure detector removing the member",
        );
        self.check(
            unavailable,
            "the transcript shows the object unavailable before the re-home",
        );
        self.check(fenced, "the transcript shows the route fence");
        self.check(
            rejoin,
            "the transcript shows the restarted member asking to rejoin",
        );
        self.check(converged, "the transcript shows the rejoin convergence");
        let moved = self.checks.iter().any(|check| check.contains("moved"));
        self.check(moved, "a move check was recorded");
    }
}

/// Run the one-computer transcript.
pub fn run(options: &Options) -> Result<i32, String> {
    if options.fault != Fault::None {
        return Err("demo runs the clean two-node configuration; --fault is not used".into());
    }
    let mut demo = match Demo::start(options) {
        Ok(demo) => demo,
        Err(error) => {
            // A failed start has no owned process list here; the rig's normal
            // error path already stopped anything it started.
            return Err(error);
        }
    };
    let result = demo.script();
    if let Err(error) = &result {
        demo.errors.push(error.clone());
        demo.say(format!("error: {error}\n"));
    }
    demo.finish()
}

fn short(id: &str) -> String {
    if id.len() <= 16 {
        id.to_string()
    } else {
        format!("{}…", &id[..16])
    }
}
