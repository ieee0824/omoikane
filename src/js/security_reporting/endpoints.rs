//! Structured dictionary parsing for document Reporting-Endpoints metadata.
//!
//! Fetch parses fields using RFC 9651. Reporting accepts String items as URLs;
//! other item types and parameters are validated but do not define endpoints.
use std::collections::BTreeMap;

pub(crate) fn parse_reporting_endpoints(input: &str, base: &str) -> BTreeMap<String, String> {
    let Ok(base) = url::Url::parse(base) else {
        return BTreeMap::new();
    };
    if !super::super::is_secure_context_url(base.as_str()) {
        return BTreeMap::new();
    }
    let mut parser = Parser {
        bytes: input.as_bytes(),
        position: 0,
    };
    let Some(values) = parser.dictionary() else {
        return BTreeMap::new();
    };
    values
        .into_iter()
        .filter_map(|(name, value)| {
            let endpoint = base.join(&value?).ok()?;
            (matches!(endpoint.scheme(), "http" | "https")
                && super::super::is_secure_context_url(endpoint.as_str()))
            .then(|| (name, endpoint.to_string()))
        })
        .collect()
}

// This borrowed view is local to one parse; resulting URLs and keys are owned.
struct Parser<'a> {
    bytes: &'a [u8],
    position: usize,
}
impl Parser<'_> {
    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.position).copied()
    }
    fn eat(&mut self, byte: u8) -> bool {
        if self.peek() != Some(byte) {
            return false;
        }
        self.position += 1;
        true
    }
    fn spaces(&mut self) {
        while self.eat(b' ') {}
    }
    fn ows(&mut self) {
        while matches!(self.peek(), Some(b' ' | b'\t')) {
            self.position += 1;
        }
    }
    fn key(&mut self) -> Option<String> {
        let start = self.position;
        if !self
            .peek()
            .is_some_and(|b| b.is_ascii_lowercase() || b == b'*')
        {
            return None;
        }
        self.position += 1;
        while self
            .peek()
            .is_some_and(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"_-.*".contains(&b))
        {
            self.position += 1;
        }
        String::from_utf8(self.bytes[start..self.position].to_vec()).ok()
    }
    fn string(&mut self) -> Option<String> {
        if !self.eat(b'"') {
            return None;
        }
        let mut result = String::new();
        loop {
            let mut byte = self.peek()?;
            self.position += 1;
            if byte == b'"' {
                return Some(result);
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
            result.push(byte as char);
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
                    let high = self.lower_hex_digit()?;
                    let low = self.lower_hex_digit()?;
                    decoded.push(high * 16 + low);
                }
                0x20..=0x7e => decoded.push(byte),
                _ => return None,
            }
        }
    }
    fn lower_hex_digit(&mut self) -> Option<u8> {
        let digit = match self.peek()? {
            byte @ b'0'..=b'9' => byte - b'0',
            byte @ b'a'..=b'f' => byte - b'a' + 10,
            _ => return None,
        };
        self.position += 1;
        Some(digit)
    }
    fn byte_sequence(&mut self) -> Option<()> {
        if !self.eat(b':') {
            return None;
        }
        let start = self.position;
        while self
            .peek()
            .is_some_and(|b| b.is_ascii_alphanumeric() || b"+/=".contains(&b))
        {
            self.position += 1;
        }
        if !self.eat(b':') {
            return None;
        }
        use base64::engine::general_purpose::{GeneralPurpose, GeneralPurposeConfig};
        use base64::{Engine, engine::DecodePaddingMode};
        // RFC 9651 section 4.2.7 permits omitted padding and non-zero pad bits.
        let config = GeneralPurposeConfig::new()
            .with_decode_padding_mode(DecodePaddingMode::Indifferent)
            .with_decode_allow_trailing_bits(true);
        GeneralPurpose::new(&base64::alphabet::STANDARD, config)
            .decode(&self.bytes[start..self.position - 1])
            .ok()?;
        Some(())
    }
    fn item(&mut self) -> Option<Option<String>> {
        if self.peek() == Some(b'"') {
            return self.string().map(Some);
        }
        if self.peek() == Some(b'%') {
            // Display Strings are a distinct structured type, not endpoint URLs.
            return self.display_string().map(|()| None);
        }
        if self.eat(b'?') {
            if !self.eat(b'0') && !self.eat(b'1') {
                return None;
            }
            return Some(None);
        }
        if self.peek() == Some(b':') {
            return self.byte_sequence().map(|()| None);
        }
        let date = self.eat(b'@');
        if date || self.peek().is_some_and(|b| b == b'-' || b.is_ascii_digit()) {
            self.eat(b'-');
            let start = self.position;
            while self.peek().is_some_and(|b| b.is_ascii_digit()) {
                self.position += 1;
            }
            let digits = self.position - start;
            if digits == 0 || digits > 15 {
                return None;
            }
            if !date && self.eat(b'.') {
                if digits > 12 {
                    return None;
                }
                let start = self.position;
                while self.peek().is_some_and(|b| b.is_ascii_digit()) {
                    self.position += 1;
                }
                if !(1..=3).contains(&(self.position - start)) {
                    return None;
                }
            }
            return Some(None);
        }
        if !self
            .peek()
            .is_some_and(|b| b.is_ascii_alphabetic() || b == b'*')
        {
            return None;
        }
        self.position += 1;
        while self
            .peek()
            .is_some_and(|b| b.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~:/".contains(&b))
        {
            self.position += 1;
        }
        Some(None)
    }
    fn parameters(&mut self) -> Option<()> {
        while self.eat(b';') {
            self.spaces();
            self.key()?;
            if self.eat(b'=') {
                self.item()?;
            }
        }
        Some(())
    }
    fn member(&mut self) -> Option<Option<String>> {
        let value = if self.eat(b'(') {
            self.spaces();
            while !self.eat(b')') {
                self.item()?;
                self.parameters()?;
                if self.peek() != Some(b')') && self.peek() != Some(b' ') {
                    return None;
                }
                self.spaces();
            }
            None
        } else {
            self.item()?
        };
        self.parameters()?;
        Some(value)
    }
    fn dictionary(&mut self) -> Option<BTreeMap<String, Option<String>>> {
        let mut result = BTreeMap::new();
        // Leading field whitespace permits SP only; between members OWS is valid.
        self.spaces();
        if self.peek().is_none() {
            return Some(result);
        }
        loop {
            let key = self.key()?;
            let value = if self.eat(b'=') {
                self.member()?
            } else {
                self.parameters()?;
                None
            };
            result.insert(key, value);
            self.ows();
            if self.peek().is_none() {
                return Some(result);
            }
            if !self.eat(b',') {
                return None;
            }
            self.ows();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_only_main(input: &str) {
        assert_eq!(
            parse_reporting_endpoints(input, "https://example.test/page"),
            BTreeMap::from([("main".into(), "https://example.test/reports".into())]),
            "{input:?}"
        );
    }

    #[test]
    fn byte_sequences_allow_missing_padding_and_non_zero_pad_bits() {
        for encoded in ["", "YQ==", "YQ=", "YQ", "YWI=", "YWI", "YR==", "YR", "YWJ="] {
            // Ignored parameters and dictionary entries still need to parse so
            // that the legitimate endpoint in the same header remains usable.
            assert_only_main(&format!(
                r#"binary=:{encoded}:, main="/reports";data=:{encoded}:"#
            ));
        }
    }

    #[test]
    fn invalid_byte_sequences_discard_the_whole_dictionary() {
        for encoded in [
            "Y", "=YQ==", "YQ===", "YWJj=", "YQ==AA", "Y Q==", "YQ==\n", "--__",
        ] {
            let input = format!(r#"main="/reports";data=:{encoded}:"#);
            assert!(
                parse_reporting_endpoints(&input, "https://example.test/page").is_empty(),
                "{input:?}"
            );
        }
    }

    #[test]
    fn valid_display_strings_and_dates_do_not_define_endpoints() {
        assert_only_main(
            r#"main="/reports";label=%"r%c3%a9sum%c3%a9";date=@-999999999999999,
            display=%"/not-an-endpoint", date=@0, list=(%"%00%22%25\\" @1)"#
                .replace('\n', "")
                .as_str(),
        );
        assert_only_main(r#"removed="/old", removed=%"/replacement", main="/reports""#);
    }

    #[test]
    fn malformed_display_strings_and_dates_discard_the_whole_dictionary() {
        for value in [
            r#"%"%C3%a9""#, // Hex letters must be lowercase.
            r#"%"%zz""#,
            r#"%"%a""#,
            r#"%"%""#,
            r#"%"%c0%af""#,    // Overlong UTF-8.
            r#"%"%ed%a0%80""#, // A surrogate cannot be encoded in UTF-8.
            r#"%"%ff""#,
            r#"%"é""#, // Non-ASCII bytes must be percent encoded.
            "%\"a\tb\"",
            "%\"unterminated",
            "@1.1",
            "@1000000000000000",
        ] {
            let input = format!(r#"main="/reports";ignored={value}"#);
            assert!(
                parse_reporting_endpoints(&input, "https://example.test/page").is_empty(),
                "{input:?}"
            );
        }
    }

    #[test]
    fn whitespace_obeys_field_and_member_boundaries() {
        assert_only_main("  main=\"/reports\"\t ,\t ignored=( 1 2 ); flag\t ");
        for input in [
            "\tmain=\"/reports\"",
            " \tmain=\"/reports\"",
            "main =\"/reports\"",
            "main= \"/reports\"",
            "main=\"/reports\";\tflag",
            "main=\"/reports\", ignored=(1\t2)",
        ] {
            assert!(
                parse_reporting_endpoints(input, "https://example.test/page").is_empty(),
                "{input:?}"
            );
        }
    }

    #[test]
    fn dictionary_accepts_parameters_and_ignores_non_string_members() {
        let result = parse_reporting_endpoints(
            r#"main="/reports";enabled, number=3, list=("/no"), main="/new", escaped="/a\\b""#,
            "https://example.test/page",
        );
        assert_eq!(
            result.get("main").map(String::as_str),
            Some("https://example.test/new")
        );
        assert_eq!(result.len(), 2);
    }
    #[test]
    fn malformed_dictionary_and_insecure_origins_do_not_install_endpoints() {
        for input in [
            r#"good="/ok", bad="unterminated"#,
            r#"good="/ok", BAD="/bad""#,
            r#"good="/ok", bad="/\q""#,
            r#"good="/ok","#,
        ] {
            assert!(
                parse_reporting_endpoints(input, "https://example.test/page").is_empty(),
                "{input}"
            );
        }
        assert!(
            parse_reporting_endpoints(
                r#"main="https://example.test/report""#,
                "http://insecure.test/page"
            )
            .is_empty()
        );
        assert!(
            parse_reporting_endpoints(
                r#"main="http://insecure.test/report""#,
                "https://example.test/page"
            )
            .is_empty()
        );
        assert_eq!(
            parse_reporting_endpoints(r#"main="/report""#, "http://localhost/page").len(),
            1
        );
    }
}
