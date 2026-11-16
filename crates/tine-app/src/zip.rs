//! Minimal ZIP (stored entries only) for the diagnostics report.
//!
//! No new dependency (`deny.toml` allow-list, D-013 binary-size budget):
//! local-file headers + central directory + EOCD with in-house CRC-32
//! (IEEE 802.3 polynomial, table-free_reflected implementation).

/// Computes the CRC-32 of `data` (the ZIP convention: init `0xFFFFFFFF`,
/// final xor `0xFFFFFFFF`).
#[must_use]
pub fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for &byte in data {
        crc ^= u32::from(byte);
        for _bit in 0..8 {
            let mask = (crc & 1).wrapping_neg();
            crc = (crc >> 1) ^ (0xEDB8_8320 & mask);
        }
    }
    !crc
}

/// Builds a ZIP archive from named entries using stored (uncompressed)
/// entries — valid for every unzip implementation; logs compress badly
/// anyway and CPU time here is irrelevant.
#[must_use]
pub fn stored_zip(entries: &[(String, Vec<u8>)]) -> Vec<u8> {
    let mut out: Vec<u8> = Vec::new();
    let mut central: Vec<u8> = Vec::new();
    for (name, data) in entries {
        let offset = u32_to_le(u32::try_from(out.len()).unwrap_or(0));
        let name_bytes = name.as_bytes();
        let crc = u32_to_le(crc32(data));
        let sizes = u32_to_le(data.len() as u32);
        // Local file header.
        out.extend_from_slice(&u32_to_le(0x0403_4b50));
        out.extend_from_slice(&u16_to_le(20)); // version needed
        out.extend_from_slice(&u16_to_le(1 << 11)); // UTF-8 names
        out.extend_from_slice(&u16_to_le(0)); // method 0 = stored
        out.extend_from_slice(&u16_to_le(0)); // mod time
        out.extend_from_slice(&u16_to_le(0)); // mod date
        out.extend_from_slice(&crc);
        out.extend_from_slice(&sizes); // compressed size
        out.extend_from_slice(&sizes); // uncompressed size
        out.extend_from_slice(&u16_to_le(u16::try_from(name_bytes.len()).unwrap_or(0)));
        out.extend_from_slice(&u16_to_le(0)); // extra len
        out.extend_from_slice(name_bytes);
        let file_start = out.len();
        out.extend_from_slice(data);
        debug_assert_eq!(out.len(), file_start + data.len());
        // Central directory record.
        central.extend_from_slice(&u32_to_le(0x0201_4b50));
        central.extend_from_slice(&u16_to_le(20)); // version made by
        central.extend_from_slice(&u16_to_le(20)); // version needed
        central.extend_from_slice(&u16_to_le(1 << 11)); // UTF-8 names
        central.extend_from_slice(&u16_to_le(0)); // method stored
        central.extend_from_slice(&u16_to_le(0)); // time
        central.extend_from_slice(&u16_to_le(0)); // date
        central.extend_from_slice(&crc);
        central.extend_from_slice(&sizes);
        central.extend_from_slice(&sizes);
        central.extend_from_slice(&u16_to_le(u16::try_from(name_bytes.len()).unwrap_or(0)));
        central.extend_from_slice(&u16_to_le(0)); // extra
        central.extend_from_slice(&u16_to_le(0)); // comment
        central.extend_from_slice(&u16_to_le(0)); // disk start
        central.extend_from_slice(&u16_to_le(0)); // internal attrs
        central.extend_from_slice(&u32_to_le(0)); // external attrs
        central.extend_from_slice(&offset);
        central.extend_from_slice(name_bytes);
    }
    let central_offset = out.len();
    let central_size = central.len() as u32;
    out.extend_from_slice(&central);
    // End of central directory.
    out.extend_from_slice(&u32_to_le(0x0605_4b50));
    out.extend_from_slice(&u16_to_le(0)); // disk
    out.extend_from_slice(&u16_to_le(0)); // disk with central dir
    out.extend_from_slice(&u16_to_le(u16::try_from(entries.len()).unwrap_or(0)));
    out.extend_from_slice(&u16_to_le(u16::try_from(entries.len()).unwrap_or(0)));
    out.extend_from_slice(&u32_to_le(central_size));
    out.extend_from_slice(&u32_to_le(central_offset as u32));
    out.extend_from_slice(&u16_to_le(0)); // comment len
    out
}

fn u16_to_le(v: u16) -> [u8; 2] {
    v.to_le_bytes()
}

fn u32_to_le(v: u32) -> [u8; 4] {
    v.to_le_bytes()
}

#[cfg(test)]
mod tests {
    use super::{crc32, stored_zip};

    /// Zip 2.0 spec: `PK\x03\x04` local, `PK\x01\x02` central, `PK\x05\x06` EOCD.
    #[test]
    fn zip_carries_the_expected_structure() {
        let entries = vec![
            ("diag.json".to_owned(), b"{\"ok\":true}".to_vec()),
            ("tine.log".to_owned(), b"hello log".to_vec()),
        ];
        let zip = stored_zip(&entries);
        assert_eq!(&zip[0..4], b"PK\x03\x04");
        let eocd = find_eocd(&zip).expect(" EOCD record missing");
        assert_eq!(&zip[eocd..eocd + 4], b"PK\x05\x06");
        let count = u16::from_le_bytes([zip[eocd + 10], zip[eocd + 11]]);
        assert_eq!(count, 2);
        // The central directory starts with a PK\x01\x02 record.
        let cd_offset = u32::from_le_bytes([zip[eocd + 16], zip[eocd + 17], zip[eocd + 18], zip[eocd + 19]]) as usize;
        assert_eq!(&zip[cd_offset..cd_offset + 4], b"PK\x01\x02");
        // Standard readers jump to the EOCD at `cd_offset + cd_size`; that
        // must land exactly on the record (regression: the EOCD used to
        // sit past its stated `cd_size`, making the archive unreadable in
        // `python.zipfile` and Windows Explorer).
        let cd_size = u32::from_le_bytes([zip[eocd + 12], zip[eocd + 13], zip[eocd + 14], zip[eocd + 15]]) as usize;
        assert_eq!(cd_offset + cd_size, eocd, "EOCD must sit at cd_offset+cd_size");
        assert_eq!(zip.len(), eocd + 22, "EOCD is the 22-byte tail");
    }

    #[test]
    fn stored_entries_round_trip_by_hand() {
        let payload = b"{\"windows\":\"11\"}".to_vec();
        let zip = stored_zip(&[("diag.json".to_owned(), payload.clone())]);
        // Local header: 30 bytes + name "diag.json" (8) then the payload.
        let start = 30 + "diag.json".len();
        assert_eq!(&zip[start..start + payload.len()], payload.as_slice());
    }

    #[test]
    fn crc32_known_vectors() {
        // IEEE 802.3 vectors.
        assert_eq!(crc32(b""), 0x0000_0000);
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
        assert_eq!(crc32(b"The quick brown fox jumps over the lazy dog"), 0x414F_A339);
    }

    fn find_eocd(zip: &[u8]) -> Option<usize> {
        zip.windows(4).rposition(|w| w == b"PK\x05\x06")
    }
}
