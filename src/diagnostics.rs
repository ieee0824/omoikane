//! Formatting helpers for opt-in performance diagnostics.

/// Formats a JSON trace record in memory before it reaches an unbuffered sink.
///
/// Callers keep their existing trace enablement checks and append the newline
/// when writing. Formatting the value here avoids a separate sink write for
/// every JSON fragment and preserves the existing event-name/JSON wire format.
pub fn json_record(event: &str, record: &serde_json::Value) -> String {
    format!("{event} {record}")
}

#[cfg(test)]
mod tests {
    use super::json_record;
    use std::io::{self, Write};

    #[derive(Default)]
    struct Sink {
        writes: usize,
        bytes: Vec<u8>,
    }

    impl Write for Sink {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            self.writes += 1;
            self.bytes.extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn preformatted_json_preserves_one_record_with_at_most_two_sink_writes() {
        let value = serde_json::json!({
            "source": "日本語\nsource",
            "callback": {"major": {"collections": 2, "total_ms": 19.5}},
            "jobs": [null, true, "escaped \\\" value"],
        });
        let mut direct = Sink::default();
        writeln!(direct, "OMOIKANE_TASK {value}").unwrap();
        let mut buffered = Sink::default();
        writeln!(buffered, "{}", json_record("OMOIKANE_TASK", &value)).unwrap();
        assert_eq!(direct.bytes, buffered.bytes);
        assert!(direct.writes > buffered.writes);
        assert!(buffered.writes <= 2, "writes={}", buffered.writes);
        assert_eq!(
            buffered.bytes.iter().filter(|byte| **byte == b'\n').count(),
            1
        );
        let decoded: serde_json::Value =
            serde_json::from_slice(&buffered.bytes[b"OMOIKANE_TASK ".len()..]).unwrap();
        assert_eq!(decoded, value);
    }
}
