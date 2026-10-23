// SPDX-License-Identifier: GPL-3.0-only
//! Inclusive byte ranges for the HTTP and TLS clients.
//!
//! A range is the inclusive pair `start-end` of an HTTP `Range: bytes=`
//! header.  Only the single-range form is supported; a suffix range
//! (`-500`) and a multi-range set are outside the console's needs.
//!
//! The module is pure logic with no kernel dependencies, so
//! `kernel/tests/range.rs` compiles it on the host.

/// An inclusive byte range `start..=end`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ByteRange {
    start: u32,
    end: u32,
}

impl ByteRange {
    /// Build a range, rejecting `end < start`.
    pub const fn new(start: u32, end: u32) -> Option<Self> {
        if start <= end {
            Some(Self { start, end })
        } else {
            None
        }
    }

    /// First byte offset of the range.
    pub const fn start(self) -> u32 {
        self.start
    }

    /// Last byte offset of the range, inclusive.
    pub const fn end(self) -> u32 {
        self.end
    }

    /// Number of body bytes a `206 Partial Content` response must carry.
    pub const fn byte_len(self) -> u64 {
        self.end as u64 - self.start as u64 + 1
    }

    /// Parse the argument after `range=`: the `start-end` form only.
    pub fn parse(spec: &str) -> Option<Self> {
        let (start, end) = spec.split_once('-')?;
        Self::new(start.parse().ok()?, end.parse().ok()?)
    }

    /// Write the header value `bytes=start-end` into `out`, returning its
    /// length, or `None` when `out` is too small.
    pub fn write_value(self, out: &mut [u8]) -> Option<usize> {
        let mut n = 0;
        append(out, &mut n, b"bytes=")?;
        append_decimal(out, &mut n, self.start)?;
        append(out, &mut n, b"-")?;
        append_decimal(out, &mut n, self.end)?;
        Some(n)
    }
}

/// Check a response against the range that was requested and return the body
/// length it must have carried.
///
/// A range fetch is only complete when the server answered `206` and the body
/// is exactly the requested length.  A `Content-Length` that disagrees with
/// the request is a protocol error even if the bytes that arrived happen to
/// add up, because it means the server sent a different range.
pub fn check_response(
    status: u16,
    body_len: usize,
    content_length: Option<usize>,
    range: ByteRange,
) -> Result<usize, &'static str> {
    let want = usize::try_from(range.byte_len()).map_err(|_| "range too large")?;
    if status != 206 {
        return Err("range not satisfiable");
    }
    if content_length.is_some_and(|len| len != want) {
        return Err("range length mismatch");
    }
    if body_len != want {
        return Err("short range body");
    }
    Ok(want)
}

/// Append `data`, returning `None` when the buffer has no room.
pub fn append(buf: &mut [u8], len: &mut usize, data: &[u8]) -> Option<()> {
    if *len + data.len() > buf.len() {
        return None;
    }
    buf[*len..*len + data.len()].copy_from_slice(data);
    *len += data.len();
    Some(())
}

/// Append an unsigned decimal number, returning `None` when the buffer has no
/// room.
pub fn append_decimal(buf: &mut [u8], len: &mut usize, mut v: u32) -> Option<()> {
    let mut digits = [0u8; 10];
    let mut n = 0;
    if v == 0 {
        digits[0] = b'0';
        n = 1;
    }
    while v > 0 && n < digits.len() {
        digits[n] = b'0' + (v % 10) as u8;
        v /= 10;
        n += 1;
    }
    for i in (0..n).rev() {
        append(buf, len, &digits[i..i + 1])?;
    }
    Some(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ranges_reject_an_inverted_pair() {
        assert_eq!(ByteRange::new(9, 4), None);
        assert!(ByteRange::new(4, 4).is_some());
    }

    #[test]
    fn byte_len_counts_both_ends() {
        assert_eq!(ByteRange::new(0, 0).unwrap().byte_len(), 1);
        assert_eq!(ByteRange::new(0, 99).unwrap().byte_len(), 100);
        assert_eq!(
            ByteRange::new(0, u32::MAX).unwrap().byte_len(),
            u64::from(u32::MAX) + 1
        );
    }

    #[test]
    fn parse_accepts_the_start_end_form() {
        let range = ByteRange::parse("1024-2047").unwrap();
        assert_eq!((range.start(), range.end()), (1024, 2047));
        assert_eq!(range.byte_len(), 1024);
        assert_eq!(ByteRange::parse("7-7").unwrap().byte_len(), 1);
    }

    #[test]
    fn parse_rejects_malformed_specs() {
        for spec in [
            "-500", "0-", "0", "10-9", "a-b", "0-1-2", "", " 0-1", "0-1 ",
        ] {
            assert!(ByteRange::parse(spec).is_none(), "{spec:?} was accepted");
        }
    }

    #[test]
    fn writes_the_header_value() {
        let mut buf = [0u8; 32];
        let n = ByteRange::new(7, 42)
            .unwrap()
            .write_value(&mut buf)
            .unwrap();
        assert_eq!(&buf[..n], b"bytes=7-42");
    }

    #[test]
    fn writes_decimal_without_leading_zeroes() {
        let mut buf = [0u8; 8];
        let mut n = 0;
        append_decimal(&mut buf, &mut n, 0).unwrap();
        assert_eq!(&buf[..n], b"0");

        let mut n = 0;
        append_decimal(&mut buf, &mut n, 1_000_000).unwrap();
        assert_eq!(&buf[..n], b"1000000");

        let mut wide = [0u8; 10];
        let mut n = 0;
        append_decimal(&mut wide, &mut n, u32::MAX).unwrap();
        assert_eq!(&wide[..n], b"4294967295");
    }

    #[test]
    fn a_206_with_the_exact_length_passes() {
        let range = ByteRange::new(0, 99).unwrap();
        assert_eq!(check_response(206, 100, Some(100), range), Ok(100));
        assert_eq!(check_response(206, 100, None, range), Ok(100));
    }

    #[test]
    fn a_full_200_response_is_rejected() {
        let range = ByteRange::new(0, 99).unwrap();
        assert_eq!(
            check_response(200, 528, Some(528), range),
            Err("range not satisfiable")
        );
    }

    #[test]
    fn a_response_of_a_different_range_is_rejected() {
        let range = ByteRange::new(0, 99).unwrap();
        assert_eq!(
            check_response(206, 50, Some(50), range),
            Err("range length mismatch")
        );
        assert_eq!(
            check_response(206, 100, Some(200), range),
            Err("range length mismatch")
        );
    }

    #[test]
    fn a_short_body_is_rejected_even_when_the_header_agrees() {
        let range = ByteRange::new(0, 99).unwrap();
        assert_eq!(
            check_response(206, 99, None, range),
            Err("short range body")
        );
        assert_eq!(
            check_response(206, 101, Some(100), range),
            Err("short range body")
        );
    }

    #[test]
    fn a_short_buffer_is_an_error_not_a_panic() {
        let mut buf = [0u8; 5];
        assert_eq!(ByteRange::new(0, 1).unwrap().write_value(&mut buf), None);

        let mut buf = [0u8; 2];
        let mut n = 0;
        assert_eq!(append_decimal(&mut buf, &mut n, 100), None);
    }
}
