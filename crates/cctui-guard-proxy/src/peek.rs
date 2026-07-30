//! Parsing the first bytes of a transparently-redirected connection to recover
//! the intended hostname: the SNI from a TLS `ClientHello`, or the `Host` header
//! from a plaintext HTTP request.

/// Parses a TLS `ClientHello` record and returns the SNI server name, or `None`
/// if the data isn't a `ClientHello` or carries no SNI extension.
#[must_use]
pub fn extract_sni(data: &[u8]) -> Option<String> {
    // TLS record: type(1) version(2) length(2) fragment
    if data.len() < 5 {
        return None;
    }
    // 0x16 = Handshake
    if data[0] != 0x16 {
        return None;
    }
    let length = u16::from_be_bytes([data[3], data[4]]) as usize;
    if data.len() < 5 + length {
        return None;
    }

    // Handshake: type(1) length(3) body
    if data.len() < 9 {
        return None;
    }
    // 0x01 = ClientHello
    if data[5] != 0x01 {
        return None;
    }
    let handshake_len = u32::from_be_bytes([0, data[6], data[7], data[8]]) as usize;
    if data.len() < 9 + handshake_len {
        return None;
    }

    // ClientHello: version(2) random(32) sessionID_len(1) sessionID
    //   cipher_suites_len(2) cipher_suites compression_len(1) compression
    //   extensions_len(2) extensions
    let mut pos = 9usize;
    pos += 2 + 32; // skip version + random
    if pos >= data.len() {
        return None;
    }

    let session_id_len = data[pos] as usize;
    pos += 1 + session_id_len;
    if pos + 2 > data.len() {
        return None;
    }

    let cipher_suites_len = u16::from_be_bytes([data[pos], data[pos + 1]]) as usize;
    pos += 2 + cipher_suites_len;
    if pos + 2 > data.len() {
        return None;
    }

    let compression_len = data[pos] as usize;
    pos += 1 + compression_len;
    if pos + 2 > data.len() {
        return None;
    }

    let extensions_len = u16::from_be_bytes([data[pos], data[pos + 1]]) as usize;
    pos += 2;
    let extensions_end = pos + extensions_len;
    if extensions_end > data.len() {
        return None;
    }

    // Extensions: type(2) length(2) data
    while pos < extensions_end {
        if pos + 4 > extensions_end {
            break;
        }
        let ext_type = u16::from_be_bytes([data[pos], data[pos + 1]]);
        let ext_len = u16::from_be_bytes([data[pos + 2], data[pos + 3]]) as usize;
        pos += 4;
        if pos + ext_len > extensions_end {
            break;
        }
        let ext_data = &data[pos..pos + ext_len];
        pos += ext_len;

        // 0x0000 = server_name extension
        if ext_type == 0x0000 {
            return parse_sni_extension(ext_data);
        }
    }

    None
}

/// SNI extension: `list_length(2) [name_type(1) name_length(2) name]…`
fn parse_sni_extension(data: &[u8]) -> Option<String> {
    if data.len() < 2 {
        return None;
    }
    let list_len = u16::from_be_bytes([data[0], data[1]]) as usize;
    let mut pos = 2usize;

    while pos < data.len() && pos - 2 < list_len {
        if pos + 3 > data.len() {
            break;
        }
        let name_type = data[pos];
        let name_len = u16::from_be_bytes([data[pos + 1], data[pos + 2]]) as usize;
        pos += 3;
        if pos + name_len > data.len() {
            break;
        }
        let name = &data[pos..pos + name_len];
        pos += name_len;

        // 0 = host_name
        if name_type == 0 {
            return String::from_utf8(name.to_vec()).ok();
        }
    }

    None
}

/// Parses the `Host` header from the start of a plaintext HTTP request. Returns
/// `None` if the data doesn't begin with an HTTP request-line or has no `Host`
/// header. The returned host has any port stripped (the transparent path
/// supplies the original-destination port).
#[must_use]
pub fn extract_http_host(data: &[u8]) -> Option<String> {
    const HP: &str = "host:";

    // Cheap guard: only treat this as HTTP if the first line is a request-line,
    // so we don't misread a binary protocol as HTTP.
    let first_line_end = data.iter().position(|&b| b == b'\n').unwrap_or(data.len());
    if !looks_like_http_request_line(&data[..first_line_end]) {
        return None;
    }

    let text = String::from_utf8_lossy(data);
    for line in text.replace("\r\n", "\n").split('\n') {
        if line.is_empty() {
            break; // end of headers
        }
        if line.len() >= HP.len() && line[..HP.len()].eq_ignore_ascii_case(HP) {
            let mut host = line[HP.len()..].trim().to_string();
            // Strip a port the client put in the Host header (unless bracketed IPv6).
            if !host.contains(']')
                && let Some(idx) = host.rfind(':')
            {
                host.truncate(idx);
            }
            return Some(host);
        }
    }
    None
}

fn looks_like_http_request_line(line: &[u8]) -> bool {
    const METHODS: [&str; 9] =
        ["GET ", "POST ", "PUT ", "HEAD ", "DELETE ", "PATCH ", "OPTIONS ", "CONNECT ", "TRACE "];
    METHODS.iter().any(|m| line.len() >= m.len() && &line[..m.len()] == m.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_http_host_cases() {
        let cases: [(&str, Option<&str>); 6] = [
            ("GET / HTTP/1.1\r\nHost: example.com\r\n\r\n", Some("example.com")),
            ("POST /x HTTP/1.1\r\nHost: example.com:8080\r\n\r\n", Some("example.com")),
            ("GET / HTTP/1.1\r\nhOsT:  foo.bar \r\n\r\n", Some("foo.bar")),
            ("GET / HTTP/1.1\r\nAccept: */*\r\n\r\n", None),
            ("\x16\x03\x01\x00\x50binarygarbage", None),
            ("", None),
        ];
        for (data, want) in cases {
            assert_eq!(
                extract_http_host(data.as_bytes()).as_deref(),
                want,
                "extract_http_host({data:?})"
            );
        }
    }

    /// Builds a minimal TLS `ClientHello` carrying a single SNI `host_name`, so we
    /// can assert `extract_sni` round-trips a real handshake shape.
    fn build_client_hello(server_name: &str) -> Vec<u8> {
        let sni = server_name.as_bytes();

        // server_name extension body: list_len(2) name_type(1) name_len(2) name
        let mut ext_body = Vec::new();
        let name_len = sni.len() as u16;
        let list_len = name_len + 3; // type(1)+len(2)+name
        ext_body.extend_from_slice(&list_len.to_be_bytes());
        ext_body.push(0); // host_name
        ext_body.extend_from_slice(&name_len.to_be_bytes());
        ext_body.extend_from_slice(sni);

        // extension: type(2)=0x0000 len(2) body
        let mut ext = Vec::new();
        ext.extend_from_slice(&0u16.to_be_bytes());
        ext.extend_from_slice(&(ext_body.len() as u16).to_be_bytes());
        ext.extend_from_slice(&ext_body);

        // ClientHello body
        let mut ch = Vec::new();
        ch.extend_from_slice(&[0x03, 0x03]); // version
        ch.extend_from_slice(&[0u8; 32]); // random
        ch.push(0); // session id len
        ch.extend_from_slice(&2u16.to_be_bytes()); // cipher suites len
        ch.extend_from_slice(&[0x00, 0x2f]); // one cipher suite
        ch.push(1); // compression methods len
        ch.push(0); // null compression
        ch.extend_from_slice(&(ext.len() as u16).to_be_bytes()); // extensions len
        ch.extend_from_slice(&ext);

        // Handshake: type(1)=0x01 len(3) body
        let mut hs = Vec::new();
        hs.push(0x01);
        let hs_len = ch.len() as u32;
        hs.extend_from_slice(&hs_len.to_be_bytes()[1..]); // 3 bytes
        hs.extend_from_slice(&ch);

        // Record: type(1)=0x16 version(2) len(2) fragment
        let mut rec = Vec::new();
        rec.push(0x16);
        rec.extend_from_slice(&[0x03, 0x01]);
        rec.extend_from_slice(&(hs.len() as u16).to_be_bytes());
        rec.extend_from_slice(&hs);
        rec
    }

    #[test]
    fn extract_sni_from_client_hello() {
        let hello = build_client_hello("example.com");
        assert_eq!(extract_sni(&hello).as_deref(), Some("example.com"));
    }

    #[test]
    fn extract_sni_rejects_non_handshake() {
        assert_eq!(extract_sni(b"GET / HTTP/1.1\r\n"), None);
        assert_eq!(extract_sni(b""), None);
        assert_eq!(extract_sni(&[0x16, 0x03]), None);
    }
}
