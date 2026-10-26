// SPDX-License-Identifier: GPL-3.0-only
//! The DS1386 NVRAM image the rig hands to each iris instance.
//!
//! The layout is the kernel's `kernel/src/nvram.rs`: the PROM environment is
//! NUL-separated `name=value` strings starting at byte 0x10, and the station
//! address lives at sparse byte index 314.  The environment must end before
//! 314 so the two never overlap.  The rest of the 8 KiB chip is zero, which is
//! what the CI rigs use; the kernel repairs a blank RTC and environment.

/// DS1386 image size.
pub const IMAGE_SIZE: usize = 8192;
/// First environment byte.
pub const ENV_START: usize = 0x10;
/// Sparse MAC byte index.
pub const MAC_INDEX: usize = 314;
/// The environment must end before the MAC slot.
pub const ENV_LIMIT: usize = MAC_INDEX - ENV_START;

/// Render an environment and station address into a DS1386 image.
///
/// The environment is emitted in order, NUL separated, with a final empty
/// string.  The caller is responsible for a coherent key set; this function
/// only enforces the layout.
pub fn image(env: &[(&str, &str)], mac: [u8; 6]) -> Result<[u8; IMAGE_SIZE], String> {
    let mut bytes = Vec::new();
    for (name, value) in env {
        if name.is_empty() || name.contains('=') || name.contains('\0') {
            return Err(format!("invalid environment name {name:?}"));
        }
        if value.contains('\0') {
            return Err(format!("invalid environment value for {name}"));
        }
        bytes.extend_from_slice(name.as_bytes());
        bytes.push(b'=');
        bytes.extend_from_slice(value.as_bytes());
        bytes.push(0);
    }
    bytes.push(0);
    if bytes.len() >= ENV_LIMIT {
        return Err(format!(
            "environment is {} bytes, limit is {} before the MAC slot",
            bytes.len(),
            ENV_LIMIT
        ));
    }

    let mut image = [0u8; IMAGE_SIZE];
    image[ENV_START..ENV_START + bytes.len()].copy_from_slice(&bytes);
    image[MAC_INDEX..MAC_INDEX + 6].copy_from_slice(&mac);
    Ok(image)
}

/// Parse a rendered environment back into key/value pairs.
///
/// This mirrors the kernel's reader: it stops at the first empty string.  The
/// host tests use it to prove the image is laid out the way the kernel reads
/// it, and the evidence bundle records exactly what each guest was given.
pub fn parse(image: &[u8]) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut position = ENV_START;
    while position < IMAGE_SIZE {
        let start = position;
        while position < IMAGE_SIZE && image[position] != 0 {
            position += 1;
        }
        if position == start {
            break;
        }
        let word = &image[start..position];
        if let Some(equals) = word.iter().position(|byte| *byte == b'=') {
            let name = String::from_utf8_lossy(&word[..equals]).into_owned();
            let value = String::from_utf8_lossy(&word[equals + 1..]).into_owned();
            out.push((name, value));
        }
        position += 1;
    }
    out
}

/// The MAC stored in an image.
#[cfg(test)]
pub fn mac(image: &[u8]) -> [u8; 6] {
    let mut out = [0u8; 6];
    out.copy_from_slice(&image[MAC_INDEX..MAC_INDEX + 6]);
    out
}

/// Format a MAC in the colon-separated lower-case form used by the NVRAM.
pub fn mac_string(mac: &[u8; 6]) -> String {
    format!(
        "{:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
        mac[0], mac[1], mac[2], mac[3], mac[4], mac[5]
    )
}

/// Parse a colon-separated MAC.
pub fn parse_mac(text: &str) -> Result<[u8; 6], String> {
    let mut out = [0u8; 6];
    let parts: Vec<&str> = text.split(':').collect();
    if parts.len() != 6 {
        return Err(format!("MAC {text:?} does not have six octets"));
    }
    for (slot, part) in out.iter_mut().zip(parts) {
        *slot = u8::from_str_radix(part, 16)
            .map_err(|_| format!("MAC {text:?} has a non-hex octet"))?;
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    const MAC: [u8; 6] = [0x08, 0x00, 0x69, 0x4d, 0x32, 0x00];

    fn sample() -> [u8; IMAGE_SIZE] {
        image(
            &[
                ("mcp.url", "http://192.168.0.1:18089/mcp.module"),
                ("mcp.sha256", "abc"),
                ("mesh.peer", "192.168.0.1:9712"),
                ("mesh.authority", "08:00:69:4d:32:00"),
                ("mesh.epoch", "1"),
                ("console", "serial0"),
            ],
            MAC,
        )
        .unwrap()
    }

    #[test]
    fn environment_lands_at_0x10_and_mac_at_314() {
        let image = sample();
        assert_eq!(&image[ENV_START..ENV_START + 3], b"mcp");
        assert_eq!(mac(&image), MAC);
        let parsed = parse(&image);
        assert_eq!(parsed[0].0, "mcp.url");
        assert_eq!(parsed[1].1, "abc");
        assert_eq!(parsed[5].0, "console");
    }

    #[test]
    fn environment_is_bounded_before_the_mac() {
        let long = "x".repeat(ENV_LIMIT);
        assert!(image(&[("k", &long)], MAC).is_err());
    }

    #[test]
    fn full_two_node_mesh_environment_fits() {
        let sha = "a".repeat(64);
        let key = "b".repeat(64);
        let image = image(
            &[
                ("mcp.url", "http://192.168.0.1:65535/mcp.module"),
                ("mcp.sha256", &sha),
                ("console", "serial0"),
                ("mesh.peer", "192.168.0.1:9712"),
                ("mesh.authority", "08:00:69:4d:32:00"),
                ("mesh.route", "2"),
                ("mesh.fail", "1"),
                ("mesh.key", &key),
            ],
            MAC,
        )
        .expect("the full M2 environment fits before the MAC slot");
        let parsed = parse(&image);
        assert_eq!(parsed.len(), 8);
        assert!(parsed
            .iter()
            .any(|(name, value)| name == "mesh.route" && value == "2"));
        assert!(parsed
            .iter()
            .any(|(name, value)| name == "mesh.fail" && value == "1"));
    }

    #[test]
    fn full_eight_node_shard_environment_fits() {
        let sha = "a".repeat(64);
        let key = "b".repeat(64);
        // The largest member: public key, shard count and the route opt-in.
        let image = image(
            &[
                ("mcp.url", "http://192.168.7.1:11171/mcp.module"),
                ("mcp.sha256", &sha),
                ("console", "serial0"),
                ("mesh.peer", "192.168.7.1:1"),
                ("mesh.authority", "08:00:69:4d:32:00"),
                ("mesh.pub", &key),
                ("mesh.shard", "128"),
                ("mesh.route", "1"),
            ],
            MAC,
        )
        .expect("the M3 shard member environment fits before the MAC slot");
        let parsed = parse(&image);
        assert_eq!(parsed.len(), 8);
        assert!(parsed
            .iter()
            .any(|(name, value)| name == "mesh.shard" && value == "128"));
    }

    #[test]
    fn mac_round_trip() {
        let text = mac_string(&MAC);
        assert_eq!(text, "08:00:69:4d:32:00");
        assert_eq!(parse_mac(&text).unwrap(), MAC);
    }

    #[test]
    fn blank_image_has_no_environment() {
        assert!(parse(&[0u8; IMAGE_SIZE]).is_empty());
    }
}
