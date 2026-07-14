//! `data:` URI parsing (RFC 2397).
//!
//! A `data:` URI inlines a resource directly in the URL rather than pointing at
//! a network location:
//!
//! ```text
//! data:[<mediatype>][;base64],<data>
//! ```
//!
//! This module decodes such URIs into a media type plus the raw payload bytes,
//! so callers (e.g. inline `<script src="data:...">` execution, or `data:`
//! image loading) can consume them without a network round-trip.

use base64::Engine;

/// A parsed `data:` URI.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DataUri {
    /// The media type without parameters (e.g. `text/javascript`). Defaults to
    /// `text/plain` when the URI omits a media type, per RFC 2397.
    pub mime_type: String,
    /// The fully decoded payload bytes (percent-decoded, and base64-decoded when
    /// the `;base64` flag is present).
    pub data: Vec<u8>,
}

/// Parses a `data:` URI (RFC 2397) into its media type and decoded bytes.
///
/// Returns `None` if the input is not a `data:` URI, lacks the required comma
/// separator, or contains base64 data that fails to decode.
///
/// Decoding rules:
/// - The scheme match is case-insensitive (`data:` / `DATA:`).
/// - The payload after the comma is percent-decoded first. This handles URIs
///   whose data section is itself percent-encoded (as Acid3's base64 vectors
///   are).
/// - When the `;base64` flag is present, ASCII whitespace (spaces, tabs, CR,
///   LF) in the percent-decoded payload is stripped before base64 decoding, so
///   line-wrapped base64 is accepted.
/// - When the flag is absent, the percent-decoded bytes are the payload.
///
/// # Examples
///
/// ```
/// use omoikane::http::parse_data_uri;
///
/// let parsed = parse_data_uri("data:text/javascript,d1%20%3D%20'one'%3B").unwrap();
/// assert_eq!(parsed.mime_type, "text/javascript");
/// assert_eq!(parsed.data, b"d1 = 'one';");
/// ```
pub fn parse_data_uri(uri: &str) -> Option<DataUri> {
    let payload = match uri.get(..5) {
        Some(prefix) if prefix.eq_ignore_ascii_case("data:") => &uri[5..],
        _ => return None,
    };

    let comma = payload.find(',')?;
    let metadata = &payload[..comma];
    let data_section = &payload[comma + 1..];

    let mut mime_type = String::from("text/plain");
    let mut is_base64 = false;
    for (index, part) in metadata.split(';').enumerate() {
        if index == 0 {
            // The media type is the first token only when it looks like
            // `type/subtype`; otherwise the default (text/plain) stands.
            if part.contains('/') {
                mime_type = part.trim().to_string();
            }
        } else if part.eq_ignore_ascii_case("base64") {
            is_base64 = true;
        }
    }

    let percent_decoded = percent_decode_bytes(data_section);
    let data = if is_base64 {
        let filtered: Vec<u8> = percent_decoded
            .into_iter()
            .filter(|byte| !byte.is_ascii_whitespace())
            .collect();
        base64::engine::general_purpose::STANDARD
            .decode(filtered)
            .ok()?
    } else {
        percent_decoded
    };

    Some(DataUri { mime_type, data })
}

/// Percent-decodes a string into raw bytes, leaving malformed escapes literal.
fn percent_decode_bytes(input: &str) -> Vec<u8> {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0usize;
    while index < bytes.len() {
        if bytes[index] == b'%' && index + 2 < bytes.len()
            && let (Some(high), Some(low)) =
                (hex_value(bytes[index + 1]), hex_value(bytes[index + 2]))
            {
                out.push((high << 4) | low);
                index += 3;
                continue;
            }
        out.push(bytes[index]);
        index += 1;
    }
    out
}

fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn decoded_string(uri: &str) -> String {
        let parsed = parse_data_uri(uri).expect("data: URI should parse");
        String::from_utf8(parsed.data).expect("payload should be valid UTF-8")
    }

    #[test]
    fn rejects_non_data_uri() {
        assert!(parse_data_uri("http://example.com/x.js").is_none());
        assert!(parse_data_uri("data:no-comma").is_none());
    }

    #[test]
    fn scheme_match_is_case_insensitive() {
        let parsed = parse_data_uri("DATA:text/plain,hi").unwrap();
        assert_eq!(parsed.data, b"hi");
    }

    #[test]
    fn defaults_to_text_plain_without_mediatype() {
        let parsed = parse_data_uri("data:,hello").unwrap();
        assert_eq!(parsed.mime_type, "text/plain");
        assert_eq!(parsed.data, b"hello");
    }

    // The five Acid3 (test 97) data: URI vectors, each defining one variable.

    #[test]
    fn acid3_d1_escaped() {
        let uri = "data:text/javascript,d1%20%3D%20'one'%3B";
        let parsed = parse_data_uri(uri).unwrap();
        assert_eq!(parsed.mime_type, "text/javascript");
        assert_eq!(decoded_string(uri), "d1 = 'one';");
    }

    #[test]
    fn acid3_d2_base64() {
        let uri = "data:text/javascript;base64,ZDIgPSAndHdvJzs%3D";
        assert_eq!(decoded_string(uri), "d2 = 'two';");
    }

    #[test]
    fn acid3_d3_base64_percent_encoded() {
        let uri = "data:text/javascript;base64,%5a%44%4d%67%50%53%41%6e%64%47%68%79%5a%57%55%6e%4f%77%3D%3D";
        assert_eq!(decoded_string(uri), "d3 = 'three';");
    }

    #[test]
    fn acid3_d4_base64_with_whitespace() {
        let uri = "data:text/javascript;base64,%20ZD%20Qg%0D%0APS%20An%20Zm91cic%0D%0A%207%20";
        assert_eq!(decoded_string(uri), "d4 = 'four';");
    }

    #[test]
    fn acid3_d5_escaped_with_backslash() {
        // %5C is a literal backslash, so the payload decodes to the JS source
        // d5 = 'five\\u0027s'; which, when executed, yields the string five's.
        let uri = "data:text/javascript,d5%20%3D%20'five%5Cu0027s'%3B";
        assert_eq!(decoded_string(uri), "d5 = 'five\\u0027s';");
    }
}
