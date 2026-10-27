// SPDX-License-Identifier: GPL-3.0-only
//! The M2-2 join/epoch oracle.
//!
//! The two kernels print `mesh-join:` lines; this module parses them and checks
//! the properties the gate needs.  The snapshot digest is recomputed here from
//! the canonical encoding in M2-ADR-0005 rather than by calling `mesh-join`,
//! so a protocol or encoding mistake on both kernels does not hide itself in
//! the evidence.

use std::fmt::Write as _;
use std::path::PathBuf;

use crate::rig::{AuthorityKeys, BootFacts, Fault, Node};

/// One parsed `mesh-join: converged` line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JoinLine {
    /// `root` or `member`.
    pub role: String,
    /// The installed epoch.
    pub epoch: u32,
    /// The declared member count.
    pub members: usize,
    /// The declared snapshot digest, upper-case hex.
    pub digest: String,
    /// The declared member IDs, upper-case hex.
    pub snapshot: Vec<String>,
    /// The original line.
    pub raw: String,
}

impl JoinLine {
    /// Parse a `mesh-join: converged ...` line.
    pub fn parse(line: &str) -> Option<Self> {
        let mut role = None;
        let mut epoch = None;
        let mut members = None;
        let mut digest = None;
        let mut snapshot = None;
        for token in line.split_whitespace() {
            if let Some(value) = token.strip_prefix("role=") {
                role = Some(value.to_string());
            } else if let Some(value) = token.strip_prefix("epoch=") {
                epoch = value.parse().ok();
            } else if let Some(value) = token.strip_prefix("members=") {
                members = value.parse().ok();
            } else if let Some(value) = token.strip_prefix("digest=") {
                digest = Some(value.to_ascii_uppercase());
            } else if let Some(value) = token.strip_prefix("snapshot=") {
                snapshot = Some(
                    value
                        .split(',')
                        .map(|id| id.to_ascii_uppercase())
                        .collect::<Vec<_>>(),
                );
            }
        }
        Some(Self {
            role: role?,
            epoch: epoch?,
            members: members?,
            digest: digest?,
            // Above two members the guest writes a compact epoch line and the
            // rig reads the table over HTTP, so an absent snapshot is valid.
            snapshot: snapshot.unwrap_or_default(),
            raw: line.trim().to_string(),
        })
    }
}

/// Parse a `last_epoch=` value out of a request or local line.
pub fn field_u32(line: &str, key: &str) -> Option<u32> {
    line.split_whitespace()
        .find_map(|token| token.strip_prefix(key).and_then(|value| value.parse().ok()))
}

/// The canonical snapshot digest, independent of `mesh-join`.
///
/// ```text
/// "IRIX7-M2-MEMBERSHIP-v1" || "I7MS" || version=1 || 0 ||
/// epoch u32 LE || count u16 LE || 0 u16 || sorted NodeIds
/// digest = first 16 bytes of BLAKE3
/// ```
pub fn snapshot_digest(epoch: u32, members: &[String]) -> Option<String> {
    let mut sorted: Vec<[u8; 16]> = members
        .iter()
        .map(|member| decode_node_id(member))
        .collect::<Option<Vec<_>>>()?;
    sorted.sort_unstable();
    let mut bytes = Vec::with_capacity(43 + sorted.len() * 16);
    bytes.extend_from_slice(b"IRIX7-M2-MEMBERSHIP-v1");
    bytes.extend_from_slice(b"I7MS");
    bytes.push(1);
    bytes.push(0);
    bytes.extend_from_slice(&epoch.to_le_bytes());
    bytes.extend_from_slice(&(sorted.len() as u16).to_le_bytes());
    bytes.extend_from_slice(&0u16.to_le_bytes());
    for member in sorted {
        bytes.extend_from_slice(&member);
    }
    let hash = blake3::hash(&bytes);
    let mut out = String::with_capacity(32);
    for byte in &hash.as_bytes()[..16] {
        let _ = write!(out, "{byte:02X}");
    }
    Some(out)
}

fn decode_node_id(text: &str) -> Option<[u8; 16]> {
    if text.len() != 32 {
        return None;
    }
    let mut id = [0u8; 16];
    for (index, byte) in id.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&text[index * 2..index * 2 + 2], 16).ok()?;
    }
    Some(id)
}

/// Per-node join lines from one boot.
#[derive(Clone, Debug)]
pub struct JoinNodeEvidence {
    /// `n0` or `n1`.
    pub name: String,
    /// The role the node reported.
    pub role: Option<String>,
    /// The `mesh-join: enabled` line.
    pub enabled: Option<String>,
    /// The `mesh-join: disabled` line.
    pub disabled: Option<String>,
    /// The `mesh-join: local` line.
    pub local: Option<String>,
    /// The first `mesh-join: request` line.
    pub request: Option<String>,
    /// Every `mesh-join: confirm` line.
    pub confirms: Vec<String>,
    /// The first `mesh-join: converged` line.
    pub converged: Option<String>,
    /// Every `mesh-join: ack` line (the root received the ack).
    pub acks: Vec<String>,
    /// Every `mesh-join: ack-sent` line (the member sent the ack).
    pub acks_sent: Vec<String>,
    /// The first `mesh-join: duplicate` line.
    pub duplicate: Option<String>,
    /// The first `mesh-join: refused` line.
    pub refused: Option<String>,
    /// The first `mesh-join: timeout` line.
    pub timeout: Option<String>,
    /// The first `mesh-join: reject` line.
    pub reject: Option<String>,
}

impl JoinNodeEvidence {
    /// Capture one node's join facts.
    pub fn capture(node: &Node, facts: &BootFacts) -> Self {
        let converged = facts
            .mesh_join_converged
            .as_deref()
            .and_then(JoinLine::parse);
        Self {
            name: node.name.clone(),
            role: converged.map(|line| line.role),
            enabled: facts.mesh_join_enabled.clone(),
            disabled: facts.mesh_join_disabled.clone(),
            local: facts.mesh_join_local.clone(),
            request: facts.mesh_join_request.clone(),
            confirms: facts.mesh_join_confirms.clone(),
            converged: facts.mesh_join_converged.clone(),
            acks: facts.mesh_join_acks.clone(),
            acks_sent: facts.mesh_join_acks_sent.clone(),
            duplicate: facts.mesh_join_duplicate.clone(),
            refused: facts.mesh_join_refused.clone(),
            timeout: facts.mesh_join_timeout.clone(),
            reject: facts.mesh_join_reject.clone(),
        }
    }
}

/// The member restart/rejoin evidence.
#[derive(Clone, Debug)]
pub struct RestartEvidence {
    /// The restarted node.
    pub node: String,
    /// The first transcript, copied before the restart.
    pub boot1_log: PathBuf,
    /// The epoch before the restart.
    pub first_epoch: u32,
    /// The epoch after the rejoin.
    pub rejoin_epoch: u32,
    /// The digest before the restart.
    pub first_digest: String,
    /// The digest after the rejoin.
    pub rejoin_digest: String,
    /// The `last_epoch=` in the member's rejoin request.
    pub request_last_epoch: Option<u32>,
    /// The root's confirmation epoch after the rejoin.
    pub confirm_epoch: Option<u32>,
    /// Whether the epoch and digest are unchanged.
    pub same_epoch: bool,
}

/// Everything the join section of the evidence bundle needs.
#[derive(Clone, Debug)]
pub struct JoinEvidence {
    /// Whether the expected join result was observed.
    pub ok: bool,
    /// Human-readable result or failure.
    pub message: String,
    /// The fault mode.
    pub fault: String,
    /// The authority root `NodeId`.
    pub authority: String,
    /// The lab authority public key.
    pub authority_key: String,
    /// The converged epoch, when one was observed.
    pub epoch: Option<u32>,
    /// The converged digest, when one was observed.
    pub digest: Option<String>,
    /// The converged member list, when one was observed.
    pub members: Vec<String>,
    /// The digest recomputed by the rig.
    pub expected_digest: Option<String>,
    /// Per-node join lines.
    pub nodes: Vec<JoinNodeEvidence>,
    /// The member restart/rejoin evidence.
    pub restart: Option<RestartEvidence>,
}

/// Facts gathered after restarting the member.
pub struct RestartInput {
    /// The node that was restarted.
    pub node: String,
    /// The first transcript.
    pub boot1_log: PathBuf,
    /// The converged line before the restart.
    pub first: JoinLine,
    /// The converged line after the rejoin.
    pub rejoin: JoinLine,
    /// The `last_epoch=` in the member's rejoin request.
    pub request_last_epoch: Option<u32>,
    /// The root's confirmation epoch after the rejoin.
    pub confirm_epoch: Option<u32>,
}

/// Build the join evidence for one run.
pub fn evaluate(
    nodes: &[Node],
    facts: &[BootFacts],
    keys: &AuthorityKeys,
    fault: Fault,
    restart: Option<&RestartInput>,
) -> JoinEvidence {
    let authority = nodes[0].node_id_string();
    let node_evidence: Vec<JoinNodeEvidence> = nodes
        .iter()
        .zip(facts.iter())
        .map(|(node, facts)| JoinNodeEvidence::capture(node, facts))
        .collect();
    let mut evidence = JoinEvidence {
        ok: false,
        message: String::new(),
        fault: fault.as_str().to_string(),
        authority,
        authority_key: keys.public_hex.clone(),
        epoch: None,
        digest: None,
        members: Vec::new(),
        expected_digest: None,
        nodes: node_evidence,
        restart: None,
    };
    let mut errors = Vec::new();
    match fault {
        Fault::NoPeer => {
            for node in &evidence.nodes {
                if node.enabled.is_some() {
                    errors.push(format!("{}: mesh-join ran without mesh.peer", node.name));
                }
                if node.disabled.is_some() {
                    errors.push(format!("{}: unexpected mesh-join disabled line", node.name));
                }
                if node.converged.is_some() {
                    errors.push(format!("{}: converged without mesh.peer", node.name));
                }
            }
            if errors.is_empty() {
                evidence.message =
                    "mesh.peer absent: no mesh listener and no join; M1/M2-0 markers preserved"
                        .into();
            }
        }
        Fault::WrongAuthority => {
            let root = &evidence.nodes[0];
            let member = &evidence.nodes[1];
            if root.confirms.is_empty() {
                errors.push("n0: the root did not sign a confirmation".into());
            }
            if member.converged.is_some() {
                errors.push("n1: installed a view despite the wrong authority key".into());
            }
            match &member.reject {
                Some(line) if line.contains("reason=bad-signature") => {}
                Some(line) => errors.push(format!("n1: unexpected reject line: {line}")),
                None => errors.push("n1: no join reject line".into()),
            }
            if errors.is_empty() {
                evidence.message =
                    "wrong authority key: the member rejected the root's signature and kept its seed view"
                        .into();
            }
        }
        Fault::None | Fault::NoRoute | Fault::KillPeer | Fault::Partition => {
            let root = match evidence.nodes[0]
                .converged
                .as_deref()
                .and_then(JoinLine::parse)
            {
                Some(line) => line,
                None => {
                    errors.push("n0: no mesh-join converged line".into());
                    JoinLine {
                        role: String::new(),
                        epoch: 0,
                        members: 0,
                        digest: String::new(),
                        snapshot: Vec::new(),
                        raw: String::new(),
                    }
                }
            };
            let member = match evidence.nodes[1]
                .converged
                .as_deref()
                .and_then(JoinLine::parse)
            {
                Some(line) => line,
                None => {
                    errors.push("n1: no mesh-join converged line".into());
                    JoinLine {
                        role: String::new(),
                        epoch: 0,
                        members: 0,
                        digest: String::new(),
                        snapshot: Vec::new(),
                        raw: String::new(),
                    }
                }
            };
            errors.extend(check_pair(
                &root,
                &member,
                &evidence.authority,
                &nodes[1].node_id_string(),
            ));
            if evidence.nodes[0].request.is_some() {
                errors.push("n0: the root sent a join request".into());
            }
            if evidence.nodes[1].request.is_none() {
                errors.push("n1: no join request line".into());
            }
            if evidence.nodes[0].confirms.is_empty() {
                errors.push("n0: no confirmation line".into());
            }
            if evidence.nodes[0].acks.is_empty() {
                errors.push("n0: no join ack from the member".into());
            }
            if evidence.nodes[1].acks_sent.is_empty() {
                errors.push("n1: no join ack-sent line".into());
            }
            if errors.is_empty() {
                evidence.epoch = Some(root.epoch);
                evidence.digest = Some(root.digest.clone());
                evidence.members = root.snapshot.clone();
                evidence.expected_digest = snapshot_digest(root.epoch, &root.snapshot);
                if evidence.expected_digest.as_deref() != Some(root.digest.as_str()) {
                    errors.push(format!(
                        "converged digest {} does not match the rig's canonical digest {}",
                        root.digest,
                        evidence.expected_digest.as_deref().unwrap_or("?")
                    ));
                }
            }
            if let Some(restart) = restart {
                match check_restart(restart, &root) {
                    Ok(restart_evidence) => evidence.restart = Some(restart_evidence),
                    Err(error) => errors.push(error),
                }
            }
            if errors.is_empty() {
                evidence.message = format!(
                    "both kernels converged on epoch {} with {} members and digest {}",
                    root.epoch, root.members, root.digest
                );
            }
        }
    }
    evidence.ok = errors.is_empty();
    if !evidence.ok {
        evidence.message = errors.join("; ");
    }
    evidence
}

/// Check that the root and member agree and that the member is the other node.
pub fn check_pair(
    root: &JoinLine,
    member: &JoinLine,
    root_id: &str,
    member_id: &str,
) -> Vec<String> {
    let mut errors = Vec::new();
    if root.role != "root" {
        errors.push(format!("n0: role is {:?}, expected root", root.role));
    }
    if member.role != "member" {
        errors.push(format!("n1: role is {:?}, expected member", member.role));
    }
    if root.epoch != member.epoch {
        errors.push(format!(
            "epoch mismatch: n0={} n1={}",
            root.epoch, member.epoch
        ));
    }
    if root.epoch <= 1 {
        errors.push(format!(
            "epoch {} does not advance past the seed",
            root.epoch
        ));
    }
    if root.digest != member.digest {
        errors.push(format!(
            "digest mismatch: n0={} n1={}",
            root.digest, member.digest
        ));
    }
    if root.members != 2 || member.members != 2 {
        errors.push(format!(
            "member count is not two: n0={} n1={}",
            root.members, member.members
        ));
    }
    let mut expected = vec![root_id.to_ascii_uppercase(), member_id.to_ascii_uppercase()];
    expected.sort();
    if root.snapshot != expected {
        errors.push(format!(
            "n0 snapshot {:?} is not the two derived NodeIds {:?}",
            root.snapshot, expected
        ));
    }
    if member.snapshot != expected {
        errors.push(format!(
            "n1 snapshot {:?} is not the two derived NodeIds {:?}",
            member.snapshot, expected
        ));
    }
    errors
}

/// Check the member restart/rejoin evidence.
pub fn check_restart(restart: &RestartInput, root: &JoinLine) -> Result<RestartEvidence, String> {
    let request_last_epoch = restart.request_last_epoch;
    let mut evidence = RestartEvidence {
        node: restart.node.clone(),
        boot1_log: restart.boot1_log.clone(),
        first_epoch: restart.first.epoch,
        rejoin_epoch: restart.rejoin.epoch,
        first_digest: restart.first.digest.clone(),
        rejoin_digest: restart.rejoin.digest.clone(),
        request_last_epoch,
        confirm_epoch: restart.confirm_epoch,
        same_epoch: false,
    };
    let mut errors = Vec::new();
    if restart.first.epoch != restart.rejoin.epoch {
        errors.push(format!(
            "rejoin epoch changed: first={} rejoin={}",
            restart.first.epoch, restart.rejoin.epoch
        ));
    }
    if restart.first.digest != restart.rejoin.digest {
        errors.push(format!(
            "rejoin digest changed: first={} rejoin={}",
            restart.first.digest, restart.rejoin.digest
        ));
    }
    if restart.first.snapshot != restart.rejoin.snapshot {
        errors.push("rejoin member list changed".into());
    }
    if restart.rejoin.epoch != root.epoch || restart.rejoin.digest != root.digest {
        errors.push("rejoined member is not on the root's current epoch".into());
    }
    if root.epoch != restart.first.epoch {
        errors.push("the root advanced its epoch across the member restart".into());
    }
    if let Some(last_epoch) = restart.request_last_epoch {
        if last_epoch >= restart.rejoin.epoch {
            errors.push(format!(
                "the rejoin request started at epoch {last_epoch}, not the seed below {}",
                restart.rejoin.epoch
            ));
        }
    } else {
        errors.push("no last_epoch in the rejoin request".into());
    }
    if let Some(confirm) = restart.confirm_epoch {
        if confirm != restart.first.epoch {
            errors.push(format!(
                "the root re-confirmed epoch {confirm}, not {}",
                restart.first.epoch
            ));
        }
    } else {
        errors.push("no confirmation after the rejoin".into());
    }
    evidence.same_epoch = restart.first.epoch == restart.rejoin.epoch
        && restart.first.digest == restart.rejoin.digest;
    if errors.is_empty() {
        Ok(evidence)
    } else {
        Err(errors.join("; "))
    }
}

/// Parse the `last_epoch=` field of a request line.
pub fn request_last_epoch(line: &str) -> Option<u32> {
    field_u32(line, "last_epoch=")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(byte: u8) -> String {
        format!("{byte:02X}").repeat(16)
    }

    #[test]
    fn parses_a_converged_line() {
        let line = "mesh-join: converged role=member epoch=2 members=2 \
                    digest=50D2F9696E00259437DF3A5ACCEB0BEF snapshot=01010101010101010101010101010101,02020202020202020202020202020202";
        let parsed = JoinLine::parse(line).expect("line");
        assert_eq!(parsed.role, "member");
        assert_eq!(parsed.epoch, 2);
        assert_eq!(parsed.members, 2);
        assert_eq!(parsed.digest, "50D2F9696E00259437DF3A5ACCEB0BEF");
        assert_eq!(parsed.snapshot.len(), 2);
    }

    #[test]
    fn parses_a_compact_converged_line() {
        let line = "mesh-join: converged role=root epoch=8 members=8 \
                    digest=50D2F9696E00259437DF3A5ACCEB0BEF compact=1";
        let parsed = JoinLine::parse(line).expect("compact line");
        assert_eq!(parsed.role, "root");
        assert_eq!(parsed.epoch, 8);
        assert_eq!(parsed.members, 8);
        assert_eq!(parsed.digest, "50D2F9696E00259437DF3A5ACCEB0BEF");
        assert!(parsed.snapshot.is_empty());
    }

    #[test]
    fn canonical_digest_matches_the_protocol_vector() {
        // `mesh-join` pins the digest for NodeId([1;16]), NodeId([2;16]) at
        // epoch 2 in its own test.  The rig must compute the same value from
        // the documented bytes, independently of the crate.
        assert_eq!(
            snapshot_digest(2, &[id(1), id(2)]).as_deref(),
            Some("50D2F9696E00259437DF3A5ACCEB0BEF")
        );
    }

    #[test]
    fn pair_check_rejects_disagreement() {
        let root = JoinLine {
            role: "root".into(),
            epoch: 2,
            members: 2,
            digest: "AA".into(),
            snapshot: vec![id(1), id(2)],
            raw: String::new(),
        };
        let mut member = root.clone();
        member.role = "member".into();
        assert!(check_pair(&root, &member, &id(1), &id(2)).is_empty());
        member.digest = "BB".into();
        assert!(!check_pair(&root, &member, &id(1), &id(2)).is_empty());
        member = root.clone();
        member.role = "member".into();
        member.snapshot = vec![id(1)];
        assert!(!check_pair(&root, &member, &id(1), &id(2)).is_empty());
    }

    #[test]
    fn restart_check_requires_the_same_epoch_and_digest() {
        let first = JoinLine {
            role: "member".into(),
            epoch: 2,
            members: 2,
            digest: "AA".into(),
            snapshot: vec![id(1), id(2)],
            raw: String::new(),
        };
        let mut rejoin = first.clone();
        rejoin.raw = "mesh-join: converged role=member epoch=2".into();
        let input = RestartInput {
            node: "n1".into(),
            boot1_log: PathBuf::from("serial-n1-boot1.log"),
            first: first.clone(),
            rejoin: rejoin.clone(),
            request_last_epoch: Some(1),
            confirm_epoch: Some(2),
        };
        assert!(check_restart(&input, &first).is_ok());
        rejoin.epoch = 3;
        let input = RestartInput {
            node: "n1".into(),
            boot1_log: PathBuf::from("serial-n1-boot1.log"),
            first: first.clone(),
            rejoin,
            request_last_epoch: Some(1),
            confirm_epoch: Some(2),
        };
        assert!(check_restart(&input, &first).is_err());
    }
}
