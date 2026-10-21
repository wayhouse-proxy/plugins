//! Standard base64 decoding (the host returns response bodies base64-encoded).

/// Decodes padded or unpadded standard base64; `None` on any other byte.
pub fn decode(s: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(s.len() / 4 * 3 + 3);
    let (mut acc, mut bits) = (0u32, 0u32);
    for b in s.trim_end_matches('=').bytes() {
        let v = match b {
            b'A'..=b'Z' => b - b'A',
            b'a'..=b'z' => b - b'a' + 26,
            b'0'..=b'9' => b - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            _ => return None,
        };
        acc = (acc << 6) | u32::from(v);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
            acc &= (1 << bits) - 1;
        }
    }
    // A lone trailing symbol carries only 6 bits: not a whole byte.
    (bits < 6).then_some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_padded_and_unpadded() {
        assert_eq!(decode("aGVsbG8=").unwrap(), b"hello");
        assert_eq!(decode("aGVsbG8").unwrap(), b"hello");
        assert_eq!(decode("").unwrap(), b"");
    }

    #[test]
    fn rejects_garbage() {
        assert!(decode("a$b=").is_none());
        assert!(decode("a").is_none());
    }
}
