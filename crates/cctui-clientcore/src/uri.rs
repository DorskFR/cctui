const UNRESERVED: &str = "-_.!~*'()";
const DIGITS: &[u8; 16] = b"0123456789ABCDEF";

/// `encodeURIComponent`: the session ids these hrefs carry may contain slashes.
#[must_use]
pub fn encode_uri_component(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if c.is_ascii_alphanumeric() || UNRESERVED.contains(c) {
            out.push(c);
        } else {
            let mut buf = [0u8; 4];
            for b in c.encode_utf8(&mut buf).bytes() {
                out.push('%');
                out.push(char::from(DIGITS[usize::from(b >> 4)]));
                out.push(char::from(DIGITS[usize::from(b & 0x0f)]));
            }
        }
    }
    out
}
