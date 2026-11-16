//! Tiny stable 64-bit anon hash (FNV-1a) for machine identifiers in
//! diagnostics output. Not cryptographic; only for correlation-safe reporting.

/// FNV-1a 64-bit hash, hex-encoded (16 chars).
#[must_use]
pub fn fnv64_hex(s: &str) -> String {
    const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;
    let mut hash = FNV_OFFSET;
    for byte in s.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    format!("{hash:016x}")
}

#[cfg(test)]
mod tests {
    use super::fnv64_hex;

    #[test]
    fn known_vector() {
        // FNV-1a 64 canonical check vectors.
        assert_eq!(fnv64_hex(""), "cbf29ce484222325");
        assert_eq!(fnv64_hex("a"), "af63dc4c8601ec8c");
        assert_eq!(fnv64_hex("foobar"), "85944171f73967e8");
    }

    #[test]
    fn output_is_stable_and_opaque() {
        let secret = "{a4992bfd-fe8f-4ba2-99c1-7e1a1f4a0f00}";
        let h = fnv64_hex(secret);
        assert_eq!(h.len(), 16);
        assert!(!h.contains("a4992bfd"));
        assert_eq!(h, fnv64_hex(secret));
        assert_ne!(h, fnv64_hex("other"));
    }
}
