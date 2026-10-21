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

/// Emits a test-only input probe on one process-wide monotonic timeline.
/// Callers must guard both this call and JSON construction with TRACE_INPUT.
/// Wall-clock pairing connects screenshots/server evidence without assuming
/// that separate processes share an Instant epoch.
pub fn input_probe(mut record: serde_json::Value) {
    use std::time::{SystemTime, UNIX_EPOCH};
    let origin = input_probe_clock();
    let before = origin.elapsed().as_nanos() as u64;
    let unix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_nanos() as u64);
    let after = origin.elapsed().as_nanos() as u64;
    record["mono_ns"] = serde_json::json!(before);
    record["unix_ns"] = serde_json::json!(unix);
    record["clock_width_ns"] = serde_json::json!(after.saturating_sub(before));
    eprintln!("{}", json_record("OMOIKANE_NATIVE_SCROLL_PROBE", &record));
}

/// Returns a span boundary on the same timeline as [`input_probe`].
/// Calls are restricted to opt-in instrumentation paths.
pub fn input_probe_boundary() -> u64 {
    // Read through the same helper clock rather than creating a second epoch.
    input_probe_clock().elapsed().as_nanos() as u64
}

fn input_probe_clock() -> &'static std::time::Instant {
    static ORIGIN: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
    ORIGIN.get_or_init(std::time::Instant::now)
}
