// SPDX-License-Identifier: GPL-3.0-only
//! Small statistics helpers: percentiles, mean and a markdown row printer.

/// A finished latency series, in nanoseconds.
pub struct Series {
    /// Sorted samples.
    samples: Vec<u64>,
}

impl Series {
    /// Sort the samples and keep them.
    pub fn new(mut samples: Vec<u64>) -> Self {
        samples.sort_unstable();
        Self { samples }
    }

    /// The sample at a percentile, nearest-rank.
    pub fn pct(&self, pct: f64) -> u64 {
        if self.samples.is_empty() {
            return 0;
        }
        let index = ((pct / 100.0) * (self.samples.len() - 1) as f64).round() as usize;
        self.samples[index.min(self.samples.len() - 1)]
    }

    /// The fastest sample.
    pub fn min(&self) -> u64 {
        *self.samples.first().unwrap_or(&0)
    }
}

/// Microseconds from nanoseconds, with three decimals for sub-microsecond work.
pub fn us(ns: u64) -> f64 {
    ns as f64 / 1000.0
}

/// Milliseconds from nanoseconds, with three decimals.
pub fn ms(ns: u64) -> f64 {
    ns as f64 / 1_000_000.0
}

/// Print a markdown table header.
pub fn header(columns: &[&str]) {
    let mut line = String::from("|");
    let mut rule = String::from("|");
    for column in columns {
        line.push(' ');
        line.push_str(column);
        line.push_str(" |");
        rule.push_str("---|");
    }
    println!("{line}");
    println!("{rule}");
}

/// Print one markdown row.
pub fn row(cells: &[String]) {
    let mut line = String::from("|");
    for cell in cells {
        line.push(' ');
        line.push_str(cell);
        line.push_str(" |");
    }
    println!("{line}");
}

/// A one-line host description for reproducibility.
pub fn host_line() -> String {
    let cpu = std::fs::read_to_string("/proc/cpuinfo")
        .ok()
        .and_then(|text| {
            text.lines()
                .find(|line| line.starts_with("model name"))
                .map(|line| line.split(':').nth(1).unwrap_or("").trim().to_string())
        })
        .unwrap_or_else(|| "unknown CPU".into());
    let kernel = std::fs::read_to_string("/proc/sys/kernel/osrelease")
        .map(|text| text.trim().to_string())
        .unwrap_or_else(|_| "unknown".into());
    format!("host: {cpu}; kernel {kernel}")
}
