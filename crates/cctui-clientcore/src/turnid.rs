const DIGITS: &[u8; 16] = b"0123456789abcdef";

/// Mint a turn id from an explicit millisecond timestamp and 16 random bytes.
///
/// `UUIDv7` rather than v4 so the ids sort by send time, which keeps them useful
/// as a debugging trail and as a DB index key. Randomness is a parameter so the
/// layout stays testable against the `TypeScript` original.
#[must_use]
pub fn turn_id_from(ts_millis: u64, random: [u8; 16]) -> String {
    let mut bytes = random;
    for (i, b) in bytes.iter_mut().take(6).enumerate() {
        *b = ((ts_millis >> (8 * (5 - i))) & 0xff) as u8;
    }
    bytes[6] = (bytes[6] & 0x0f) | 0x70;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let mut hex = String::with_capacity(32);
    for b in bytes {
        hex.push(char::from(DIGITS[usize::from(b >> 4)]));
        hex.push(char::from(DIGITS[usize::from(b & 0x0f)]));
    }
    format!("{}-{}-{}-{}-{}", &hex[0..8], &hex[8..12], &hex[12..16], &hex[16..20], &hex[20..])
}
