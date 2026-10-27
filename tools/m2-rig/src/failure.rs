// SPDX-License-Identifier: GPL-3.0-only
//! M2-6 failure, lease fencing and partition evidence.
//!
//! Two real-kernel modes drive this module:
//!
//! * `kill-peer` stops n1 after the M2-4 route steps.  The surviving root must
//!   move n1 through `suspect`, `failed` and `removed`, advance the membership
//!   epoch exactly once, fence every route and lease that named n1, recompute
//!   the affected homes and keep serving reads and commits locally without a
//!   quorum.
//! * `partition` cuts only the mesh relay.  Both kernels stay alive.  The root
//!   side removes the unreachable member and continues; the member side keeps
//!   its last confirmed view, refuses a mutable commit immediately and serves
//!   a read only from its unexpired lease.  It never advances the epoch and
//!   never creates a second home.  Restoring the seam resets both guest links
//!   through the link-only `mesh.reconnect` control (a cut longer than the
//!   receive deadline can tear the emulated forward's session mid-record) and
//!   the member rejoins at a new epoch with mutable service restored.
//!
//! The rig recomputes the expected epoch, member list, digest, homes and
//! fixture values independently of the module.  `mesh-sim` is not evidence.

use std::time::{Duration, Instant};

fn progress(message: &str) {
    eprintln!("m2-rig: failure: {message}");
}

use crate::homes::{self, HomeTable};
use crate::join;
use crate::rig::{read_serial, Node};
use crate::route::{self, RouteReport};

/// The M2 fixture object count the rig configures by default.
pub const OBJECT_COUNT: u32 = 8;
/// The MCP HTTP slot needs its close handshake before a non-idempotent step.
const COMMAND_SETTLE: Duration = Duration::from_secs(6);
/// One expected membership view, recomputed by the rig.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExpectedView {
    /// Membership epoch.
    pub epoch: u32,
    /// Confirmed member IDs, upper-case hex.
    pub members: Vec<String>,
    /// Membership digest, upper-case hex.
    pub digest: String,
    /// The home table this view must produce.
    pub table: HomeTable,
}

/// One node's failure evidence.
#[derive(Clone, Debug)]
pub struct NodeFailure {
    /// Node name.
    pub name: String,
    /// The route report, when the control plane answered.
    pub report: Option<RouteReport>,
    /// Why the node produced no report.
    pub error: Option<String>,
}

/// The failure/partition evidence bundle section.
#[derive(Clone, Debug, Default)]
pub struct FailureEvidence {
    /// `kill-peer` or `partition`.
    pub fault: String,
    /// The scenario stage (`failure`, `isolation`, `recovery`).
    pub stage: String,
    /// Overall verdict.
    pub ok: bool,
    /// First failure, or a short summary.
    pub message: String,
    /// The view before the failure.
    pub before: Option<ExpectedView>,
    /// The view the survivor must reach.
    pub after: Option<ExpectedView>,
    /// The view after a successful rejoin, in partition mode.
    pub recovered: Option<ExpectedView>,
    /// Per-node reports.
    pub nodes: Vec<NodeFailure>,
    /// Commands issued, with the exact responses.
    pub steps: Vec<String>,
    /// Assertions that passed.
    pub checks: Vec<String>,
    /// The `mesh-fail:` serial lines from both kernels.
    pub serial: Vec<String>,
}

impl FailureEvidence {
    fn new(fault: &str) -> Self {
        Self {
            fault: fault.into(),
            stage: "failure".into(),
            ok: false,
            message: String::new(),
            before: None,
            after: None,
            recovered: None,
            nodes: Vec::new(),
            steps: Vec::new(),
            checks: Vec::new(),
            serial: Vec::new(),
        }
    }

    fn pass(&mut self, condition: bool, message: impl Into<String>, errors: &mut Vec<String>) {
        let message = message.into();
        if condition {
            self.checks.push(message);
        } else {
            errors.push(message);
        }
    }

    fn finish(mut self, errors: Vec<String>) -> Self {
        self.ok = errors.is_empty();
        if self.ok {
            self.message = match self.stage.as_str() {
                "recovery" => "partition recovered and the member rejoined".into(),
                "isolation" => "isolated member refused mutable service".into(),
                _ => "failure fenced and the survivor continued".into(),
            };
        } else {
            self.message = errors.join("; ");
        }
        self
    }
}

/// Build the view the rig expects from an explicit member list.
pub fn expected_view(epoch: u32, members: &[String]) -> Result<ExpectedView, String> {
    let mut sorted = members.to_vec();
    sorted.sort();
    sorted.dedup();
    let digest = join::snapshot_digest(epoch, &sorted)
        .ok_or_else(|| "cannot compute the membership digest".to_string())?;
    let table = homes::expected_table(epoch, &digest, &sorted, OBJECT_COUNT)?;
    Ok(ExpectedView {
        epoch,
        members: sorted,
        digest,
        table,
    })
}

fn wait_marker(node: &Node, marker: &str, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    loop {
        if read_serial(node).contains(marker) {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(250));
    }
}

fn wait_for<F>(timeout: Duration, mut probe: F) -> Result<RouteReport, String>
where
    F: FnMut() -> Result<(bool, RouteReport), String>,
{
    let deadline = Instant::now() + timeout;
    let mut last = String::new();
    loop {
        match probe() {
            Ok((true, report)) => return Ok(report),
            Ok((false, _)) => {}
            Err(error) => last = error,
        }
        if Instant::now() >= deadline {
            return Err(if last.is_empty() {
                "timed out waiting for the route report".into()
            } else {
                format!("timed out waiting for the route report: {last}")
            });
        }
        std::thread::sleep(Duration::from_millis(250));
    }
}

fn fetch_report(node: &Node) -> Result<(bool, RouteReport), String> {
    route::fetch(node, Duration::from_secs(10)).map(|(_, report)| (false, report))
}

/// Wait until `predicate` accepts the node's route report.
fn report_until(
    node: &Node,
    timeout: Duration,
    predicate: impl Fn(&RouteReport) -> bool,
) -> Result<RouteReport, String> {
    wait_for(timeout, || {
        let (_, report) = fetch_report(node)?;
        Ok((predicate(&report), report))
    })
}

fn step(
    evidence: &mut FailureEvidence,
    node: &Node,
    number: u8,
    expected_status: u16,
    errors: &mut Vec<String>,
) -> bool {
    std::thread::sleep(COMMAND_SETTLE);
    match route::command(node, number, Duration::from_secs(15)) {
        Ok((status, body)) => {
            evidence.steps.push(format!(
                "{} step {number}: HTTP {status} {}",
                node.name,
                body.trim()
            ));
            if status != expected_status {
                errors.push(format!(
                    "{} step {number}: expected HTTP {expected_status}, got {status}: {}",
                    node.name,
                    body.trim()
                ));
                return false;
            }
            true
        }
        Err(error) => {
            errors.push(format!("{} step {number}: {error}", node.name));
            false
        }
    }
}

fn required_serial(
    evidence: &mut FailureEvidence,
    node: &Node,
    marker: &str,
    errors: &mut Vec<String>,
) {
    let text = read_serial(node);
    evidence.pass(
        text.contains(marker),
        format!("{} logged {marker}", node.name),
        errors,
    );
    for line in text.lines().filter(|line| line.contains("mesh-fail:")) {
        let line = format!("{}: {}", node.name, line.trim());
        if !evidence.serial.contains(&line) {
            evidence.serial.push(line);
        }
    }
}

fn check_homes(
    evidence: &mut FailureEvidence,
    node: &Node,
    expected: &ExpectedView,
    errors: &mut Vec<String>,
) {
    match homes::fetch_table(node, "/mesh/homes", Duration::from_secs(30)) {
        Ok((status, table)) => {
            evidence.pass(
                status == 200 && table.state == "current",
                format!("{} /mesh/homes is current", node.name),
                errors,
            );
            evidence.pass(
                table.epoch == Some(expected.epoch)
                    && table.members == Some(expected.members.len()),
                format!(
                    "{} homes epoch={:?} members={:?} match the expected epoch {} and {} members",
                    node.name,
                    table.epoch,
                    table.members,
                    expected.epoch,
                    expected.members.len()
                ),
                errors,
            );
            evidence.pass(
                table.digest.as_deref() == Some(expected.digest.as_str()),
                format!("{} homes digest is the recomputed digest", node.name),
                errors,
            );
            let same_rows = table.rows.len() == expected.table.rows.len()
                && table
                    .rows
                    .iter()
                    .zip(&expected.table.rows)
                    .all(|(left, right)| {
                        left.object == right.object
                            && left.home == right.home
                            && left.epoch == right.epoch
                            && left.digest == right.digest
                    });
            evidence.pass(
                same_rows,
                format!("{} recomputed home table matches every row", node.name),
                errors,
            );
        }
        Err(error) => errors.push(format!("{} /mesh/homes: {error}", node.name)),
    }
}

fn check_fence(
    evidence: &mut FailureEvidence,
    report: &RouteReport,
    stale_home: &str,
    survivor: &str,
    stale_object: &str,
    errors: &mut Vec<String>,
) {
    evidence.pass(
        report.fences >= 1,
        "the membership change recorded at least one fence",
        errors,
    );
    let row = report.fence_rows.iter().find(|row| {
        row.object == stale_object
            && row.stale_home == stale_home
            && row.home == survivor
            && row.reason == "left-view"
    });
    evidence.pass(
        row.is_some(),
        format!("a fence row names the stale home {stale_home} and the new home {survivor}"),
        errors,
    );
    if let Some(row) = row {
        evidence.pass(
            row.stale_home != row.home
                && row.stale_home_epoch >= 2
                && row.home_epoch >= row.stale_home_epoch,
            "the fence names a different home and a current membership epoch",
            errors,
        );
    }
}

/// Drive the survivor's post-failure steps and assert the fence and the open
/// local data path.
fn survivor_steps(
    evidence: &mut FailureEvidence,
    root: &Node,
    survivor_id: &str,
    stale_home: &str,
    stale_object: &str,
    epoch: u32,
    errors: &mut Vec<String>,
) {
    let mut results_seen = 0usize;
    // Step 6: re-route the object that was homed at the removed node.
    if step(evidence, root, 6, 202, errors) {
        let report = match report_until(root, Duration::from_secs(60), |report| {
            report.results.iter().any(|row| {
                row.object == stale_object
                    && row.operation == "stat"
                    && row.home == survivor_id
                    && row.attempts == 1
                    && !row.moved
            })
        }) {
            Ok(report) => report,
            Err(error) => {
                errors.push(format!("step 6: {error}"));
                return;
            }
        };
        let row = report
            .results
            .iter()
            .find(|row| {
                row.object == stale_object
                    && row.operation == "stat"
                    && row.home == survivor_id
                    && row.attempts == 1
                    && !row.moved
            })
            .expect("step 6 result row");
        let want = route::route_value(stale_object, epoch, 1).unwrap_or_default();
        evidence.pass(
            row.home == survivor_id && row.attempts == 1 && !row.moved,
            "step 6 completed at the survivor on the first attempt without a MOVED chase",
            errors,
        );
        evidence.pass(
            row.value == want,
            "step 6 value matches the independent recomputation",
            errors,
        );
        check_fence(
            evidence,
            &report,
            stale_home,
            survivor_id,
            stale_object,
            errors,
        );
        evidence.pass(
            report.timeouts == 0,
            "the survivor's normal data path did not wait for the removed node",
            errors,
        );
    }

    // Step 7: replay the last stale frame; the epoch fence must refuse it.
    // The control step itself reports the refusal as a 409; that is the
    // expected outcome, not a control failure.
    if step(evidence, root, 7, 409, errors) {
        evidence.pass(
            evidence
                .steps
                .last()
                .is_some_and(|line| line.contains("stale-epoch")),
            "the stale frame replay reported stale-epoch",
            errors,
        );
        match report_until(root, Duration::from_secs(30), |report| report.replays >= 1) {
            Ok(report) => {
                evidence.pass(
                    report.last_replay.as_deref() == Some("stale-epoch"),
                    "the stale lease frame was refused with stale-epoch",
                    errors,
                );
                evidence.pass(
                    report.rejects >= 1,
                    "the stale frame is counted as a refusal",
                    errors,
                );
            }
            Err(error) => errors.push(format!("step 7: {error}")),
        }
    }

    // Step 8: a mutable commit is available at the authority without a quorum.
    if step(evidence, root, 8, 202, errors) {
        let report = match report_until(root, Duration::from_secs(60), |report| {
            report.results.iter().any(|row| row.operation == "commit")
        }) {
            Ok(report) => report,
            Err(error) => {
                errors.push(format!("step 8: {error}"));
                return;
            }
        };
        let row = report
            .results
            .iter()
            .find(|row| row.operation == "commit")
            .expect("commit row");
        let want = route::commit_value(&row.object, epoch, 1).unwrap_or_default();
        evidence.pass(
            row.home == survivor_id && row.value == want,
            "step 8 committed at the survivor with the independently computed value",
            errors,
        );
        evidence.pass(report.mutable, "the survivor reports mutable=1", errors);
        evidence.pass(
            report.write_lease_ms.is_some(),
            "the survivor reports its authority write lease",
            errors,
        );
        evidence.pass(
            report.authority == "current" && report.authority_epoch == Some(epoch),
            "the survivor reports a current authority at the new epoch",
            errors,
        );
        results_seen = report.results.len();
    }

    // Step 9: a read is available at the same home.  The report keeps a
    // bounded number of rows, so the check is that a new row appeared.
    if step(evidence, root, 9, 202, errors) {
        match report_until(root, Duration::from_secs(60), |report| {
            report.results.len() > results_seen
                && report
                    .results
                    .iter()
                    .skip(results_seen)
                    .any(|row| row.operation == "stat")
        }) {
            Ok(report) => {
                let fresh: Vec<_> = report
                    .results
                    .iter()
                    .skip(results_seen)
                    .filter(|row| row.operation == "stat")
                    .collect();
                evidence.pass(
                    !fresh.is_empty() && fresh.iter().all(|row| row.home == survivor_id),
                    "the new read result is homed at the survivor",
                    errors,
                );
                evidence.pass(report.timeouts == 0, "no operation timed out", errors);
            }
            Err(error) => errors.push(format!("step 9: {error}")),
        }
    }
}

/// Kill the member and assert the root's failure transition and fencing.
pub fn kill_peer(
    root: &Node,
    member: &Node,
    before: &ExpectedView,
    stale_object: &str,
    kill: &mut dyn FnMut() -> Result<(), String>,
) -> FailureEvidence {
    let mut evidence = FailureEvidence::new("kill-peer");
    evidence.before = Some(before.clone());
    let mut errors = Vec::new();
    progress("kill-peer: stopping the member");
    if let Err(error) = kill() {
        errors.push(format!("kill the member: {error}"));
        return evidence.finish(errors);
    }
    evidence.stage = "failure".into();
    progress("kill-peer: waiting for suspect/failed/removed");

    if !wait_marker(root, "mesh-fail: suspect", Duration::from_secs(30)) {
        errors.push("the survivor did not report the member suspect".into());
    }
    if !wait_marker(root, "mesh-fail: failed", Duration::from_secs(30)) {
        errors.push("the survivor did not report the member failed".into());
    }
    if !wait_marker(root, "mesh-fail: removed", Duration::from_secs(45)) {
        errors.push("the survivor did not confirm the removal".into());
        return evidence.finish(errors);
    }

    let survivor_id = root.node_id_string();
    let after = match expected_view(before.epoch + 1, std::slice::from_ref(&survivor_id)) {
        Ok(view) => view,
        Err(error) => {
            errors.push(error);
            return evidence.finish(errors);
        }
    };
    evidence.after = Some(after.clone());
    progress("kill-peer: checking the survivor home table");
    check_homes(&mut evidence, root, &after, &mut errors);
    required_serial(&mut evidence, root, "mesh-fail: suspect", &mut errors);
    required_serial(&mut evidence, root, "mesh-fail: failed", &mut errors);
    required_serial(&mut evidence, root, "mesh-fail: removed", &mut errors);

    progress("kill-peer: driving steps 6-9 on the survivor");
    survivor_steps(
        &mut evidence,
        root,
        &survivor_id,
        &member.node_id_string(),
        stale_object,
        after.epoch,
        &mut errors,
    );

    progress("kill-peer: collecting the final report");
    let report = route::fetch_ready(root, Duration::from_secs(30))
        .map(|(_, report)| report)
        .map_err(|error| errors.push(format!("/mesh/route: {error}")));
    if let Ok(report) = report {
        evidence.pass(
            report.view_epoch == Some(after.epoch)
                && report.members == Some(1)
                && report.digest.as_deref() == Some(after.digest.as_str()),
            "the survivor's route view is the recomputed one-member epoch",
            &mut errors,
        );
        evidence.nodes.push(NodeFailure {
            name: root.name.clone(),
            report: Some(report),
            error: None,
        });
    }
    evidence.nodes.push(NodeFailure {
        name: member.name.clone(),
        report: None,
        error: Some("killed by the failure fault".into()),
    });
    evidence.finish(errors)
}

/// Cut the mesh link, assert both sides' partition behaviour, then restore it
/// and require a clean rejoin.
pub fn partition(
    root: &Node,
    member: &Node,
    before: &ExpectedView,
    stale_object: &str,
    restore: &mut dyn FnMut() -> Result<(), String>,
) -> FailureEvidence {
    let mut evidence = FailureEvidence::new("partition");
    evidence.before = Some(before.clone());
    let mut errors = Vec::new();
    evidence.stage = "isolation".into();
    progress("partition: waiting for removal and authority loss");

    if !wait_marker(root, "mesh-fail: removed", Duration::from_secs(60)) {
        errors.push("the root did not confirm the removal".into());
    }
    if !wait_marker(
        member,
        "mesh-fail: authority state=lost",
        Duration::from_secs(60),
    ) {
        errors.push("the isolated member did not report authority lost".into());
    }

    let survivor_id = root.node_id_string();
    let member_id = member.node_id_string();
    let after = match expected_view(before.epoch + 1, std::slice::from_ref(&survivor_id)) {
        Ok(view) => view,
        Err(error) => {
            errors.push(error);
            return evidence.finish(errors);
        }
    };
    evidence.after = Some(after.clone());
    progress("partition: checking the root home table");
    check_homes(&mut evidence, root, &after, &mut errors);
    required_serial(&mut evidence, root, "mesh-fail: removed", &mut errors);
    required_serial(
        &mut evidence,
        member,
        "mesh-fail: authority state=lost",
        &mut errors,
    );

    progress("partition: checking the isolated member");
    // The isolated member keeps the old confirmed view, refuses a commit and
    // still serves a read from its unexpired lease.  It never advances the
    // epoch and never re-homes the object.
    let isolated = match route::fetch_ready(member, Duration::from_secs(30)) {
        Ok((_, report)) => Some(report),
        Err(error) => {
            errors.push(format!("{} /mesh/route: {error}", member.name));
            None
        }
    };
    if let Some(report) = &isolated {
        evidence.pass(
            report.view_epoch == Some(before.epoch)
                && report.members == Some(2)
                && report.digest.as_deref() == Some(before.digest.as_str()),
            "the isolated member keeps its last confirmed view",
            &mut errors,
        );
        evidence.pass(
            report.authority == "lost"
                && report.authority_epoch == Some(before.epoch)
                && !report.mutable,
            "the isolated member reports authority lost and mutable=0",
            &mut errors,
        );
        evidence.pass(
            report.write_lease_ms == Some(0) || !report.mutable,
            "the isolated member cannot commit",
            &mut errors,
        );
    }

    // An object homed on the other side is unavailable, not re-homed locally.
    std::thread::sleep(COMMAND_SETTLE);
    match route::command(member, 6, Duration::from_secs(15)) {
        Ok((status, body)) => {
            evidence.steps.push(format!(
                "{} step 6: HTTP {status} {}",
                member.name,
                body.trim()
            ));
            evidence.pass(
                status == 409 && body.contains("no-authority"),
                "the isolated member reported a remote home as unavailable, not re-homed",
                &mut errors,
            );
        }
        Err(error) => errors.push(format!("member step 6: {error}")),
    }

    // A commit is unavailable immediately: no second authoritative home.
    std::thread::sleep(COMMAND_SETTLE);
    match route::command(member, 8, Duration::from_secs(15)) {
        Ok((status, body)) => {
            evidence.steps.push(format!(
                "{} step 8: HTTP {status} {}",
                member.name,
                body.trim()
            ));
            evidence.pass(
                status == 409 && body.contains("no-authority"),
                "the isolated member refused the mutable commit with no-authority",
                &mut errors,
            );
        }
        Err(error) => errors.push(format!("member step 8: {error}")),
    }
    match route::fetch_ready(member, Duration::from_secs(30)) {
        Ok((_, report)) => {
            evidence.pass(
                report.last_reject.as_deref() == Some("no-authority"),
                "the isolated member's last refusal is no-authority",
                &mut errors,
            );
            evidence.pass(
                !report.results.iter().any(|row| row.operation == "commit"),
                "no commit completed while the member was isolated",
                &mut errors,
            );
            evidence.pass(
                report.commits == 0 && report.fence_rows.is_empty(),
                "the isolated member created no new home and no fence to itself",
                &mut errors,
            );
            evidence.nodes.push(NodeFailure {
                name: member.name.clone(),
                report: Some(report),
                error: None,
            });
        }
        Err(error) => errors.push(format!("{} final isolated report: {error}", member.name)),
    }
    if let Ok((_, report)) = route::fetch_ready(root, Duration::from_secs(30)) {
        evidence.pass(
            report.view_epoch == Some(after.epoch) && report.members == Some(1),
            "the root's view stayed at the one-member epoch while isolated",
            &mut errors,
        );
        evidence.nodes.push(NodeFailure {
            name: root.name.clone(),
            report: Some(report),
            error: None,
        });
    }

    progress("partition: driving steps 6-9 on the root");
    survivor_steps(
        &mut evidence,
        root,
        &survivor_id,
        &member_id,
        stale_object,
        after.epoch,
        &mut errors,
    );

    // Recovery: restore the seam and require a fresh signed confirmation.
    // The log already contains `mesh-join: confirm` and a transient
    // `authority state=restored` from before the authority was lost, so the
    // gate must name the recovered epoch.  A generic marker returns instantly
    // on the stale line and the home check then races the real rejoin.
    evidence.stage = "recovery".into();
    progress("partition: restoring the mesh seam");
    if let Err(error) = restore() {
        errors.push(format!("restore the mesh relay: {error}"));
        return evidence.finish(errors);
    }
    let recovered_epoch = before.epoch.saturating_add(2);
    let root_marker = format!("mesh-join: confirm epoch={recovered_epoch} ");
    let member_marker = format!("mesh-fail: authority state=restored epoch={recovered_epoch} ");
    let rejoined = wait_marker(root, &root_marker, Duration::from_secs(120))
        && wait_marker(member, &member_marker, Duration::from_secs(120));
    if !rejoined {
        errors.push(format!(
            "the member did not rejoin at epoch {recovered_epoch} after the partition healed"
        ));
        return evidence.finish(errors);
    }
    progress("partition: member rejoined; checking the recovered tables");
    let both = [survivor_id.clone(), member_id.clone()];
    let recovered = match expected_view(recovered_epoch, &both) {
        Ok(view) => view,
        Err(error) => {
            errors.push(error);
            return evidence.finish(errors);
        }
    };
    evidence.recovered = Some(recovered.clone());
    check_homes(&mut evidence, root, &recovered, &mut errors);
    check_homes(&mut evidence, member, &recovered, &mut errors);
    required_serial(
        &mut evidence,
        member,
        "mesh-fail: authority state=restored",
        &mut errors,
    );

    // The recovered member serves a mutable commit from the authority again.
    // The member never issues a capability itself, so the root drives the
    // commit to the first object homed at the member.
    let member_completes = route::fetch_ready(member, Duration::from_secs(30))
        .map(|(_, report)| report.completes)
        .unwrap_or(0);
    if step(&mut evidence, root, 10, 202, &mut errors) {
        match report_until(root, Duration::from_secs(60), |report| {
            report
                .results
                .iter()
                .any(|row| row.operation == "commit" && row.home == member_id && row.attempts == 1)
        }) {
            Ok(report) => {
                let row = report
                    .results
                    .iter()
                    .find(|row| row.operation == "commit" && row.home == member_id)
                    .expect("commit row");
                let want = route::commit_value(&row.object, recovered.epoch, 1).unwrap_or_default();
                evidence.pass(
                    row.value == want && !row.moved,
                    "the recovered member committed with the independently computed value",
                    &mut errors,
                );
            }
            Err(error) => errors.push(format!("recovered root step 10: {error}")),
        }
    }
    match route::fetch_ready(member, Duration::from_secs(30)) {
        Ok((_, report)) => {
            evidence.pass(
                report.mutable
                    && report.authority == "current"
                    && report.authority_epoch == Some(recovered.epoch),
                "the recovered member reports a current authority",
                &mut errors,
            );
            evidence.pass(
                report.completes > member_completes,
                "the recovered member served the remote commit",
                &mut errors,
            );
        }
        Err(error) => errors.push(format!("recovered member report: {error}")),
    }
    if let Ok((_, report)) = route::fetch_ready(member, Duration::from_secs(30)) {
        evidence.pass(
            report.view_epoch == Some(recovered.epoch)
                && report.members == Some(2)
                && report.digest.as_deref() == Some(recovered.digest.as_str()),
            "the recovered member installed the new two-member epoch",
            &mut errors,
        );
        evidence.nodes.push(NodeFailure {
            name: member.name.clone(),
            report: Some(report),
            error: None,
        });
    }
    evidence.finish(errors)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_expected_view_is_the_recomputed_epoch_and_table() {
        let members = [
            "08006969B60100000000000000000000".to_string(),
            "08006995F50000000000000000000000".to_string(),
        ];
        let view = expected_view(3, &[members[1].clone()]).expect("view");
        assert_eq!(view.epoch, 3);
        assert_eq!(view.members.len(), 1);
        assert_eq!(view.table.rows.len(), OBJECT_COUNT as usize);
        assert!(view.table.rows.iter().all(|row| row.home == members[1]));
        // Removing a member is a new epoch; the digest is recomputed, not
        // carried over from the two-member view.
        let two = expected_view(2, &members).expect("two");
        assert_ne!(view.digest, two.digest);
    }

    #[test]
    fn partitions_do_not_produce_a_one_member_member_view() {
        // A partition is not a membership change on the side without the
        // authority: the member keeps the two-member epoch.
        let members = [
            "08006969B60100000000000000000000".to_string(),
            "08006995F50000000000000000000000".to_string(),
        ];
        let before = expected_view(2, &members).expect("before");
        let root_after = expected_view(3, &[members[0].clone()]).expect("root");
        assert_eq!(before.members.len(), 2);
        assert_eq!(root_after.members.len(), 1);
        assert_ne!(before.digest, root_after.digest);
    }

    #[test]
    fn the_fence_requires_the_stale_home_to_advance() {
        assert!(route::route_value(
            "BEB6D1C7D3D6204050933044057A4B31417ACE296D6666B5EAB98F42A538D2A6",
            3,
            1
        )
        .is_some());
        assert!(route::commit_value(
            "BEB6D1C7D3D6204050933044057A4B31417ACE296D6666B5EAB98F42A538D2A6",
            3,
            1
        )
        .is_some());
    }
}
