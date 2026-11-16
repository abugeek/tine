//! Endpoint and schema identifier types.

/// Current schema version for persisted JSON files (`Preset`, `EndpointProfile`, `AppSettings`).
pub const SCHEMA_VERSION: u32 = 1;

/// Maximum length of a sanitized endpoint ID segment.
pub const MAX_ENDPOINT_ID_LEN: usize = 96;

/// Fixed-capacity stack buffer holding a sanitized endpoint identifier.
///
/// Replaces any character outside `[A-Za-z0-9._-{}-]` with `'_'` so the identifier
/// is safe for both Windows named file mappings (`Global\...`) and filenames.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct SanitizedId {
    buf: [u8; MAX_ENDPOINT_ID_LEN],
    len: usize,
}

impl SanitizedId {
    /// Sanitizes an arbitrary endpoint ID string into a stack-allocated ASCII buffer.
    #[must_use]
    pub fn from_raw(raw: &str) -> Self {
        let mut buf = [0u8; MAX_ENDPOINT_ID_LEN];
        let mut len = 0usize;
        for b in raw.bytes() {
            if len >= MAX_ENDPOINT_ID_LEN {
                break;
            }
            let safe = match b {
                b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'.' | b'_' | b'-' | b'{' | b'}' => b,
                _ => b'_',
            };
            if let Some(slot) = buf.get_mut(len) {
                *slot = safe;
                len += 1;
            }
        }
        Self { buf, len }
    }

    /// Returns the sanitized identifier as a `&str`.
    #[must_use]
    pub fn as_str(&self) -> &str {
        let slice = self.buf.get(..self.len).unwrap_or(&[]);
        // SAFETY: `from_raw` only writes ASCII bytes (`0x20..=0x7E`), which are always valid UTF-8.
        unsafe { core::str::from_utf8_unchecked(slice) }
    }
}

impl core::fmt::Display for SanitizedId {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(self.as_str())
    }
}
