//! Local Structured Field Item parsing for COOP/COEP response headers.
//!
//! Results are owned. The borrowed parser exists only during a single parse.
//! Unknown parameters still require valid RFC 9651 Bare Item syntax; only the
//! String form of the final `report-to` parameter supplies an endpoint name.

/// A valid token field with its optional reporting endpoint group name.
pub(super) struct PolicyField {
    pub(super) token: String,
    pub(super) reporting_endpoint: Option<String>,
}

pub(super) fn policy_field(headers: &[(String, String)], name: &str) -> Option<PolicyField> {
    let values = headers
        .iter()
        .filter(|(key, _)| key.eq_ignore_ascii_case(name))
        .map(|(_, value)| value.trim_matches([' ', '\t']))
        .collect::<Vec<_>>();
    if values.is_empty() {
        return None;
    }
    let combined = values.join(", ");
    let mut parser = Parser {
        bytes: combined.as_bytes(),
        position: 0,
    };
    parser.spaces();
    let token = parser.token()?;
    let reporting_endpoint = parser.parameters()?;
    parser.spaces();
    (parser.peek().is_none()).then_some(PolicyField {
        token,
        reporting_endpoint,
    })
}

struct Parser<'a> {
    bytes: &'a [u8],
    position: usize,
}

impl Parser<'_> {
    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.position).copied()
    }

    fn eat(&mut self, byte: u8) -> bool {
        if self.peek() == Some(byte) {
            self.position += 1;
            true
        } else {
            false
        }
    }

    fn spaces(&mut self) {
        while self.eat(b' ') {}
    }

    fn token(&mut self) -> Option<String> {
        let start = self.position;
        if !self
            .peek()
            .is_some_and(|byte| byte.is_ascii_alphabetic() || byte == b'*')
        {
            return None;
        }
        self.position += 1;
        while self.peek().is_some_and(|byte| {
            byte.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~:/".contains(&byte)
        }) {
            self.position += 1;
        }
        String::from_utf8(self.bytes[start..self.position].to_vec()).ok()
    }

    fn key(&mut self) -> Option<String> {
        let start = self.position;
        if !self
            .peek()
            .is_some_and(|byte| byte.is_ascii_lowercase() || byte == b'*')
        {
            return None;
        }
        self.position += 1;
        while self.peek().is_some_and(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || b"_-.*".contains(&byte)
        }) {
            self.position += 1;
        }
        String::from_utf8(self.bytes[start..self.position].to_vec()).ok()
    }

    fn string(&mut self) -> Option<String> {
        if !self.eat(b'"') {
            return None;
        }
        let mut output = String::new();
        loop {
            let mut byte = self.peek()?;
            self.position += 1;
            if byte == b'"' {
                return Some(output);
            }
            if byte == b'\\' {
                byte = self.peek()?;
                self.position += 1;
                if byte != b'"' && byte != b'\\' {
                    return None;
                }
            } else if !(0x20..=0x7e).contains(&byte) {
                return None;
            }
            output.push(byte as char);
        }
    }

    fn display_string(&mut self) -> Option<()> {
        if !self.eat(b'%') || !self.eat(b'"') {
            return None;
        }
        let mut decoded = Vec::new();
        loop {
            let byte = self.peek()?;
            self.position += 1;
            match byte {
                b'"' => return std::str::from_utf8(&decoded).ok().map(|_| ()),
                b'%' => {
                    let high = self.lower_hex()?;
                    let low = self.lower_hex()?;
                    decoded.push(high * 16 + low);
                }
                0x20..=0x7e => decoded.push(byte),
                _ => return None,
            }
        }
    }

    fn lower_hex(&mut self) -> Option<u8> {
        let result = match self.peek()? {
            digit @ b'0'..=b'9' => digit - b'0',
            digit @ b'a'..=b'f' => digit - b'a' + 10,
            _ => return None,
        };
        self.position += 1;
        Some(result)
    }

    fn byte_sequence(&mut self) -> Option<()> {
        use base64::engine::general_purpose::{GeneralPurpose, GeneralPurposeConfig};
        use base64::{Engine, engine::DecodePaddingMode};

        if !self.eat(b':') {
            return None;
        }
        let start = self.position;
        while self
            .peek()
            .is_some_and(|byte| byte.is_ascii_alphanumeric() || b"+/=".contains(&byte))
        {
            self.position += 1;
        }
        let end = self.position;
        if !self.eat(b':') {
            return None;
        }
        // RFC 9651 accepts missing padding and non-zero pad bits.
        let config = GeneralPurposeConfig::new()
            .with_decode_padding_mode(DecodePaddingMode::Indifferent)
            .with_decode_allow_trailing_bits(true);
        GeneralPurpose::new(&base64::alphabet::STANDARD, config)
            .decode(&self.bytes[start..end])
            .ok()?;
        Some(())
    }

    fn number(&mut self, date: bool) -> Option<()> {
        self.eat(b'-');
        let start = self.position;
        while self.peek().is_some_and(|byte| byte.is_ascii_digit()) {
            self.position += 1;
        }
        let digits = self.position - start;
        if !(1..=15).contains(&digits) {
            return None;
        }
        if !date && self.eat(b'.') {
            if digits > 12 {
                return None;
            }
            let start = self.position;
            while self.peek().is_some_and(|byte| byte.is_ascii_digit()) {
                self.position += 1;
            }
            if !(1..=3).contains(&(self.position - start)) {
                return None;
            }
        }
        Some(())
    }

    fn bare_item(&mut self) -> Option<Option<String>> {
        match self.peek()? {
            b'"' => self.string().map(Some),
            b'%' => self.display_string().map(|()| None),
            b':' => self.byte_sequence().map(|()| None),
            b'?' => {
                self.position += 1;
                (self.eat(b'0') || self.eat(b'1')).then_some(None)
            }
            b'@' => {
                self.position += 1;
                self.number(true).map(|()| None)
            }
            b'-' | b'0'..=b'9' => self.number(false).map(|()| None),
            _ => self.token().map(|_| None),
        }
    }

    fn parameters(&mut self) -> Option<Option<String>> {
        let mut endpoint = None;
        while self.eat(b';') {
            self.spaces();
            let key = self.key()?;
            let value = if self.eat(b'=') {
                self.bare_item()?
            } else {
                None
            };
            if key == "report-to" {
                // Structured parameters use the last occurrence, even when it
                // changes a previous String parameter to another valid type.
                endpoint = value;
            }
        }
        Some(endpoint)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(values: &[&str]) -> Option<PolicyField> {
        let headers = values
            .iter()
            .map(|value| ("COEP".into(), (*value).into()))
            .collect::<Vec<_>>();
        policy_field(&headers, "coep")
    }

    #[test]
    fn valid_http_whitespace_and_parameters_preserve_the_policy_token() {
        for value in [
            "require-corp",
            " require-corp ",
            "\trequire-corp\t",
            " \trequire-corp",
            "require-corp\t ",
            "require-corp; foo=bar",
            "require-corp;require-corp",
            "require-corp;flag;integer=-123;decimal=123.456;boolean=?0;date=@-123",
            "require-corp;binary=:YR:;text=%\"%c3%a9\"",
        ] {
            let field = parse(&[value]).unwrap_or_else(|| panic!("{value:?}"));
            assert_eq!(field.token, "require-corp");
            assert_eq!(field.reporting_endpoint, None);
        }
    }

    #[test]
    fn invalid_item_types_and_parameters_discard_the_whole_policy() {
        for value in [
            "",
            "\"require-corp\"",
            ":cmVxdWlyZS1jb3Jw:",
            "?1",
            "123",
            "(require-corp)",
            "require-corp;",
            "require-corp;\tfoo=bar",
            "require-corp require-corp",
            "require-corp,require-corp",
            "\u{b}require-corp\u{b}",
            "require-corp\r",
            "require-corp;key=\"bad\\escape\"",
            "require-corp;Key=valid",
            "require-corp;key=1.1234",
            "require-corp;key=1234567890123456",
            "require-corp;key=@1.0",
            "require-corp;key=?2",
            "require-corp;key=:A:",
            "require-corp;key=%\"%FF\"",
            "require-corp;key=%\"%ff\"",
        ] {
            assert!(parse(&[value]).is_none(), "{value:?}");
        }
    }

    #[test]
    fn repeated_headers_are_combined_before_item_parsing() {
        for values in [
            vec!["require-corp", "require-corp"],
            vec!["", "require-corp"],
            vec!["require-corp", ""],
        ] {
            assert!(parse(&values).is_none(), "{values:?}");
        }
        let field = parse(&["require-corp; report-to=\"data:", "\""]).unwrap();
        assert_eq!(field.token, "require-corp");
        assert_eq!(field.reporting_endpoint.as_deref(), Some("data:, "));
    }

    #[test]
    fn only_the_last_string_report_to_parameter_names_an_endpoint() {
        let field = parse(&[r#"same-origin;report-to="first";report-to="escaped\"\\""#]).unwrap();
        assert_eq!(field.reporting_endpoint.as_deref(), Some("escaped\"\\"));
        for suffix in ["token", "?1", "42", "%\"display\"", ":YQ:"] {
            let input = format!("same-origin;report-to=\"first\";report-to={suffix}");
            assert_eq!(
                parse(&[&input]).unwrap().reporting_endpoint,
                None,
                "{suffix}"
            );
        }
        assert_eq!(
            parse(&["same-origin;report-to=\"first\";report-to"])
                .unwrap()
                .reporting_endpoint,
            None
        );
    }
}
