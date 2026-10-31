//! Server Timing (Issue #1293).
//!
//! Parses `Server-Timing` response headers into the metrics exposed as
//! `PerformanceServerTiming` on Resource and Navigation Timing entries.

use super::*;
use crate::http::cors::{Origin as CorsOrigin, ResponseType};

/// One `Server-Timing` metric.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ServerTimingMetric {
    pub(crate) name: String,
    /// The `dur` parameter in milliseconds, or 0.
    pub(crate) duration: f64,
    /// The `desc` parameter, or the empty string.
    pub(crate) description: String,
}

impl ServerTimingMetric {
    fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "name": self.name,
            "duration": self.duration,
            "description": self.description,
        })
    }
}

/// Parses every `Server-Timing` field in `headers` (Server Timing §3).
///
/// Parsing stops at the first metric without a valid name. Characters that
/// do not fit the grammar after a metric name or a parameter value are
/// skipped up to the next `,` or `;`, and the first occurrence of each
/// parameter wins.
pub(crate) fn parse_server_timing(headers: &[(String, String)]) -> Vec<ServerTimingMetric> {
    let value = headers
        .iter()
        .filter(|(name, _)| name.eq_ignore_ascii_case("server-timing"))
        .map(|(_, value)| value.as_str())
        .collect::<Vec<_>>()
        .join(", ");
    let mut tokenizer = Tokenizer {
        input: value.as_bytes(),
        index: 0,
    };
    let mut metrics = Vec::new();
    while !tokenizer.is_consumed() {
        let Some(name) = tokenizer.token() else {
            break;
        };
        let mut metric = ServerTimingMetric {
            name,
            duration: 0.0,
            description: String::new(),
        };
        let (mut has_duration, mut has_description) = (false, false);
        tokenizer.skip_to_delimiter();
        while tokenizer.consume(b';') {
            let Some(parameter) = tokenizer.token() else {
                break;
            };
            let mut value = String::new();
            if tokenizer.consume(b'=') {
                value = tokenizer.token_or_quoted_string().unwrap_or_default();
                tokenizer.skip_to_delimiter();
            }
            if parameter.eq_ignore_ascii_case("dur") && !has_duration {
                has_duration = true;
                metric.duration = parse_duration(&value);
            } else if parameter.eq_ignore_ascii_case("desc") && !has_description {
                has_description = true;
                metric.description = value;
            }
        }
        metrics.push(metric);
        if !tokenizer.consume(b',') {
            break;
        }
    }
    metrics
}

/// A `dur` value as a finite number, or 0.
fn parse_duration(value: &str) -> f64 {
    let numeric = !value.is_empty()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'.' | b'e' | b'E' | b'+' | b'-'));
    numeric
        .then(|| value.parse::<f64>().ok())
        .flatten()
        .filter(|duration| duration.is_finite())
        .unwrap_or(0.0)
}

/// Whether a response passes the timing allow check (Resource Timing §4.5)
/// for `origin`: it is same-origin, or `Timing-Allow-Origin` lists the
/// origin or `*`.
pub(crate) fn timing_allowed(
    response_type: ResponseType,
    headers: &[(String, String)],
    origin: &CorsOrigin,
) -> bool {
    match response_type {
        ResponseType::Basic => true,
        ResponseType::Opaque | ResponseType::OpaqueRedirect => false,
        ResponseType::Cors => {
            let origin = origin.serialize();
            headers
                .iter()
                .filter(|(name, _)| name.eq_ignore_ascii_case("timing-allow-origin"))
                .flat_map(|(_, value)| value.split(','))
                .map(str::trim)
                .any(|value| value == "*" || value == origin)
        }
    }
}

/// The JSON array of metrics that script turns into `serverTiming`.
pub(crate) fn server_timing_json(metrics: &[ServerTimingMetric]) -> serde_json::Value {
    serde_json::Value::Array(metrics.iter().map(ServerTimingMetric::to_json).collect())
}

/// An HTTP header field tokenizer over optional whitespace, tokens and
/// quoted strings.
struct Tokenizer<'a> {
    input: &'a [u8],
    index: usize,
}

impl Tokenizer<'_> {
    fn is_consumed(&self) -> bool {
        self.index >= self.input.len()
    }

    fn peek(&self) -> Option<u8> {
        self.input.get(self.index).copied()
    }

    fn skip_whitespace(&mut self) {
        while matches!(self.peek(), Some(b' ' | b'\t')) {
            self.index += 1;
        }
    }

    /// Consumes `expected` with the whitespace around it.
    fn consume(&mut self, expected: u8) -> bool {
        self.skip_whitespace();
        if self.peek() != Some(expected) {
            return false;
        }
        self.index += 1;
        self.skip_whitespace();
        true
    }

    fn token(&mut self) -> Option<String> {
        self.skip_whitespace();
        let start = self.index;
        while self.peek().is_some_and(is_token_byte) {
            self.index += 1;
        }
        let token = (self.index > start)
            .then(|| String::from_utf8_lossy(&self.input[start..self.index]).into_owned());
        self.skip_whitespace();
        token
    }

    fn token_or_quoted_string(&mut self) -> Option<String> {
        self.skip_whitespace();
        if self.peek() == Some(b'"') {
            self.quoted_string()
        } else {
            self.token()
        }
    }

    fn quoted_string(&mut self) -> Option<String> {
        self.index += 1;
        let mut value = Vec::new();
        while let Some(byte) = self.peek() {
            self.index += 1;
            match byte {
                b'"' => {
                    self.skip_whitespace();
                    return Some(String::from_utf8_lossy(&value).into_owned());
                }
                b'\\' => {
                    let escaped = self.peek()?;
                    self.index += 1;
                    value.push(escaped);
                }
                _ => value.push(byte),
            }
        }
        None
    }

    /// Skips to the next `,` or `;` without consuming it.
    fn skip_to_delimiter(&mut self) {
        while self.peek().is_some_and(|byte| byte != b',' && byte != b';') {
            self.index += 1;
        }
    }
}

/// `tchar` from RFC 9110 §5.6.2.
fn is_token_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&byte)
}

impl JsRuntime {
    /// Exposes the main Document's `Server-Timing` response header on its
    /// navigation entry.
    pub fn install_navigation_server_timing(&mut self, headers: &[(String, String)]) {
        let metrics = server_timing_json(&parse_server_timing(headers));
        let document_id = self.host_state.borrow().document.identity();
        let result = self.eval_in_document_realm(
            document_id,
            &format!(
                "if (typeof globalThis.__omoikane_set_navigation_server_timing === 'function') \
                 globalThis.__omoikane_set_navigation_server_timing({metrics});"
            ),
        );
        self.record_error_from("server timing", result);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(value: &str) -> Vec<(String, f64, String)> {
        parse_server_timing(&[("Server-Timing".to_string(), value.to_string())])
            .into_iter()
            .map(|metric| (metric.name, metric.duration, metric.description))
            .collect()
    }

    fn metric(name: &str, duration: f64, description: &str) -> (String, f64, String) {
        (name.to_string(), duration, description.to_string())
    }

    #[test]
    fn server_timing_follows_the_header_grammar() {
        assert_eq!(parse(""), []);
        assert_eq!(parse(",="), []);
        assert_eq!(
            parse("metric;dur=123.4;desc=description"),
            [metric("metric", 123.4, "description")]
        );
        assert_eq!(
            parse(r#"metric;desc="descr;,=iption";dur="123.4""#),
            [metric("metric", 123.4, "descr;,=iption")]
        );
        assert_eq!(
            parse("metric ; DuR = 123.4 ; DeSc = description"),
            [metric("metric", 123.4, "description")]
        );
        assert_eq!(
            parse("metric1;dur=12.3,metric2;desc=two"),
            [metric("metric1", 12.3, ""), metric("metric2", 0.0, "two")]
        );
    }

    #[test]
    fn server_timing_skips_invalid_parameters() {
        assert_eq!(
            parse("metric;dur=foo;dur=567.8"),
            [metric("metric", 0.0, "")]
        );
        assert_eq!(parse("metric;dur=;dur=123.4"), [metric("metric", 0.0, "")]);
        assert_eq!(
            parse("metric;desc=d1 d2;dur=1"),
            [metric("metric", 1.0, "d1")]
        );
        assert_eq!(
            parse(r#"metric==   ""foo;dur=123.4"#),
            [metric("metric", 123.4, "")]
        );
        assert_eq!(parse("metric;foo dur=12"), [metric("metric", 0.0, "")]);
        assert_eq!(parse("metric;dur foo=12"), [metric("metric", 0.0, "")]);
        assert_eq!(parse(r#"metric;desc="\\"#), [metric("metric", 0.0, "")]);
        assert_eq!(parse(r#"metric;desc=\""\"#), [metric("metric", 0.0, "")]);
        assert_eq!(parse("metric;dur=inf"), [metric("metric", 0.0, "")]);
        assert_eq!(
            parse("metric;desc=\"\t description \t\""),
            [metric("metric", 0.0, "\t description \t")]
        );
        assert_eq!(
            parse("metric;\tdesc=\ttrimmed\t;dur=\t42\t"),
            [metric("metric", 42.0, "trimmed")]
        );
    }

    #[test]
    fn multiple_fields_are_combined_in_order() {
        let headers = [
            ("server-timing".to_string(), "a;dur=1".to_string()),
            ("Content-Type".to_string(), "text/html".to_string()),
            ("Server-Timing".to_string(), "b;dur=2".to_string()),
        ];
        let names: Vec<_> = parse_server_timing(&headers)
            .into_iter()
            .map(|metric| metric.name)
            .collect();
        assert_eq!(names, ["a", "b"]);
    }

    #[test]
    fn timing_allow_origin_gates_cross_origin_responses() {
        let origin = CorsOrigin::from_url(&"https://example.com/".parse().unwrap());
        let header = |value: &str| vec![("Timing-Allow-Origin".to_string(), value.to_string())];
        assert!(timing_allowed(ResponseType::Basic, &[], &origin));
        assert!(!timing_allowed(ResponseType::Cors, &[], &origin));
        assert!(timing_allowed(ResponseType::Cors, &header("*"), &origin));
        assert!(timing_allowed(
            ResponseType::Cors,
            &header("https://other.test, https://example.com"),
            &origin
        ));
        assert!(!timing_allowed(
            ResponseType::Cors,
            &header("https://other.test"),
            &origin
        ));
        assert!(!timing_allowed(ResponseType::Opaque, &header("*"), &origin));
    }
}
