// SPDX-License-Identifier: GPL-3.0-only
//! Timeouts for the rig's waits.
//!
//! Every wait in the rig takes its duration from [`secs`].  The default scale is
//! 1.0, so the gate behaves as it always has.  `M2_RIG_TIMEOUT_SCALE` multiplies
//! every wait; it exists to test whether a flaky gate run is caused by the rig's
//! own deadlines being too tight for a loaded host.  A run with a scale above 1
//! is not a gate result and must be reported as such.

use std::sync::OnceLock;
use std::time::Duration;

fn scale() -> f64 {
    static SCALE: OnceLock<f64> = OnceLock::new();
    *SCALE.get_or_init(|| {
        std::env::var("M2_RIG_TIMEOUT_SCALE")
            .ok()
            .and_then(|value| value.parse::<f64>().ok())
            .filter(|value| value.is_finite() && *value > 0.0)
            .unwrap_or(1.0)
    })
}

/// A wait of `seconds` at the configured scale.
pub fn secs(seconds: u64) -> Duration {
    Duration::from_secs_f64(seconds as f64 * scale())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_scale_is_identity() {
        if std::env::var("M2_RIG_TIMEOUT_SCALE").is_err() {
            assert_eq!(secs(7), Duration::from_secs(7));
        }
    }
}
