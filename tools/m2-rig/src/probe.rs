// SPDX-License-Identifier: GPL-3.0-only
//! The two-way connectivity probe.
//!
//! The rig drives each guest's kernel telnet console and asks its HTTP client
//! to fetch the peer's MCP status page.  The request crosses the guest's NAT,
//! the Rust relay, the peer's port forward and the peer's kernel TCP stack; the
//! response comes back over the same path.  The transcript is the evidence.

use std::io::{ErrorKind, Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpStream};
use std::time::{Duration, Instant};

use crate::rig::Node;

/// A successful probe.
#[derive(Clone, Debug)]
pub struct ProbeOutcome {
    /// HTTP status reported by the peer's MCP server.
    pub status: u16,
    /// Body length reported by the guest's HTTP client.
    pub bytes: u64,
    /// SHA-256 of the body, when the client printed one.
    pub sha256: Option<String>,
    /// Raw telnet transcript, including IAC negotiation.
    pub transcript: Vec<u8>,
    /// Printable transcript.
    pub text: String,
}

/// A failed probe, kept with its transcript so the report is still useful.
#[derive(Clone, Debug)]
pub struct ProbeError {
    /// What went wrong.
    pub message: String,
    /// Raw telnet transcript.
    pub transcript: Vec<u8>,
    /// Printable transcript.
    pub text: String,
}

impl ProbeError {
    fn new(message: impl Into<String>, transcript: Vec<u8>) -> Self {
        let text = clean_telnet(&transcript);
        Self {
            message: message.into(),
            transcript,
            text,
        }
    }
}

/// Ask `node` to fetch `http://host:port/` from the peer.
pub fn probe_http(
    node: &Node,
    host: Ipv4Addr,
    port: u16,
    timeout: Duration,
) -> Result<ProbeOutcome, ProbeError> {
    let address: SocketAddr = (Ipv4Addr::LOCALHOST, node.telnet_port).into();
    let mut stream =
        TcpStream::connect_timeout(&address, Duration::from_secs(5)).map_err(|error| {
            ProbeError::new(
                format!(
                    "connect to {} telnet {}: {error}",
                    node.name, node.telnet_port
                ),
                Vec::new(),
            )
        })?;
    stream
        .set_read_timeout(Some(Duration::from_millis(250)))
        .map_err(|error| ProbeError::new(format!("set read timeout: {error}"), Vec::new()))?;
    stream
        .set_write_timeout(Some(Duration::from_secs(2)))
        .map_err(|error| ProbeError::new(format!("set write timeout: {error}"), Vec::new()))?;

    let mut transcript = Vec::new();
    read_banner(&mut stream, &mut transcript, Duration::from_secs(5))?;

    let command = format!("http {host}:{port}/\r\n");
    stream.write_all(command.as_bytes()).map_err(|error| {
        ProbeError::new(
            format!("write {command:?} to {}: {error}", node.name),
            transcript.clone(),
        )
    })?;

    let deadline = Instant::now() + timeout;
    let mut seen_result = None;
    loop {
        let closed = match read_chunk(&mut stream, &mut transcript) {
            Ok(0) => true,
            Ok(_) => false,
            Err(error)
                if error.kind() == ErrorKind::WouldBlock || error.kind() == ErrorKind::TimedOut =>
            {
                false
            }
            Err(error) => {
                return Err(ProbeError::new(
                    format!("read from {}: {error}", node.name),
                    transcript,
                ))
            }
        };

        let text = clean_telnet(&transcript);
        if let Some(result) = analyze(&text) {
            if seen_result.is_none() {
                seen_result = Some(Instant::now());
            }
            match result {
                Ok((status, bytes)) => {
                    let sha256 = find_sha256(&text);
                    // The client prints `http dbg` one poll after the result.
                    // Wait a short quiet period so the evidence includes it,
                    // even if the peer closed immediately after the response.
                    if seen_result.is_some_and(|at| at.elapsed() >= Duration::from_millis(400))
                        || closed
                    {
                        return Ok(ProbeOutcome {
                            status,
                            bytes,
                            sha256,
                            transcript,
                            text,
                        });
                    }
                }
                Err(message) => {
                    return Err(ProbeError::new(message, transcript));
                }
            }
        } else if closed {
            break;
        }
        if Instant::now() >= deadline {
            let message = analyze(&clean_telnet(&transcript))
                .map(|result| match result {
                    Ok((status, bytes)) => {
                        format!("probe timed out after http {status} {bytes} bytes")
                    }
                    Err(message) => message,
                })
                .unwrap_or_else(|| "probe timed out with no HTTP result".into());
            return Err(ProbeError::new(message, transcript));
        }
    }

    Err(ProbeError::new(
        "telnet peer closed before an HTTP result",
        transcript,
    ))
}

fn read_banner(
    stream: &mut TcpStream,
    transcript: &mut Vec<u8>,
    timeout: Duration,
) -> Result<(), ProbeError> {
    let deadline = Instant::now() + timeout;
    loop {
        match read_chunk(stream, transcript) {
            Ok(0) => {
                return Err(ProbeError::new(
                    "telnet peer closed before the console banner",
                    transcript.clone(),
                ))
            }
            Ok(_) => {}
            Err(error)
                if error.kind() == ErrorKind::WouldBlock || error.kind() == ErrorKind::TimedOut => {
            }
            Err(error) => {
                return Err(ProbeError::new(
                    format!("read console banner: {error}"),
                    transcript.clone(),
                ))
            }
        }
        let text = clean_telnet(transcript);
        if text.contains("sash> ") || text.contains("kernel> ") {
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err(ProbeError::new(
                "timed out waiting for the console prompt",
                transcript.clone(),
            ));
        }
    }
}

fn read_chunk(stream: &mut TcpStream, transcript: &mut Vec<u8>) -> std::io::Result<usize> {
    let mut buffer = [0u8; 4096];
    match stream.read(&mut buffer) {
        Ok(count) => {
            transcript.extend_from_slice(&buffer[..count]);
            Ok(count)
        }
        Err(error) => Err(error),
    }
}

/// Analyse the printable transcript.
///
/// The first prompt after the command is printed by the command parser before
/// the deferred HTTP fetch runs, so a prompt alone is not the result.  A result
/// is the last `http: ` line that is not `fetching` or `usage`.
fn analyze(text: &str) -> Option<Result<(u16, u64), String>> {
    let mut result = None;
    for line in text.lines() {
        let line = line.trim();
        // The kernel prints the result after the prompt on the same line:
        // `sash> http: 200 39 bytes`.  The echoed command uses `http ` (no
        // colon), so finding `http: ` anywhere in the line is unambiguous.
        let Some(index) = line.find("http: ") else {
            continue;
        };
        let rest = &line[index + "http: ".len()..];
        if rest == "fetching" || rest.starts_with("usage:") {
            continue;
        }
        if let Some(after) = rest.strip_prefix("200 ") {
            let bytes = after
                .split_whitespace()
                .next()
                .and_then(|word| word.parse::<u64>().ok())
                .unwrap_or(0);
            result = Some(Ok((200, bytes)));
        } else {
            result = Some(Err(format!("guest HTTP client reported: http: {rest}")));
        }
    }
    result
}

fn find_sha256(text: &str) -> Option<String> {
    text.lines().find_map(|line| {
        let line = line.trim();
        let index = line.find("sha256: ")?;
        Some(line[index + "sha256: ".len()..].trim().to_string())
    })
}

/// Remove telnet negotiation and non-printable bytes for the evidence text.
pub fn clean_telnet(raw: &[u8]) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut index = 0usize;
    while index < raw.len() {
        let byte = raw[index];
        if byte == 0xFF {
            // IAC: two or three bytes depending on the command.
            index += 1;
            if index >= raw.len() {
                break;
            }
            let command = raw[index];
            index += 1;
            if (251..=254).contains(&command) && index < raw.len() {
                index += 1;
            }
            continue;
        }
        index += 1;
        match byte {
            b'\r' => {}
            b'\n' | b'\t' => out.push(byte as char),
            0x20..=0x7e => out.push(byte as char),
            _ => out.push('.'),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "sash> http 192.168.0.1:18092/\nhttp: fetching\nsash> http: 200 39 bytes\nsha256: 6b99ac2eea5f72a8f9008c29592fe82a2b3274a57542bc447c4cff2fc1438984\nsash> http dbg rx=0000007B\n";

    #[test]
    fn parses_a_200_result() {
        assert_eq!(analyze(SAMPLE), Some(Ok((200, 39))));
        assert_eq!(
            find_sha256(SAMPLE).as_deref(),
            Some("6b99ac2eea5f72a8f9008c29592fe82a2b3274a57542bc447c4cff2fc1438984")
        );
    }

    #[test]
    fn parses_an_error() {
        let text = "sash> http x\nhttp: fetching\nsash> http: receive timed out\nsash> http dbg\n";
        assert_eq!(
            analyze(text),
            Some(Err(
                "guest HTTP client reported: http: receive timed out".into()
            ))
        );
    }

    #[test]
    fn cleaning_strips_iac() {
        let raw = b"\xff\xfb\x01\xff\xfb\x03\r\nIRIX 7 sash\r\nsash> ";
        assert_eq!(clean_telnet(raw), "\nIRIX 7 sash\nsash> ");
    }
}
