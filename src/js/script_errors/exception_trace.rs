//! Bounded host diagnostics; page-visible ErrorEvent muting remains unchanged.
use super::ExceptionReport;
use std::{cell::Cell, sync::OnceLock};

const MESSAGE_CHARS: usize = 4096;
const SOURCE_CHARS: usize = 2048;
thread_local! {
    static REMAINING: Cell<usize> = const { Cell::new(128) };
    #[cfg(test)]
    static CAPTURE: std::cell::RefCell<Option<Vec<serde_json::Value>>> = const { std::cell::RefCell::new(None) };
}

/// Writes a bounded diagnostic without evaluating author objects or changing delivery.
pub(super) fn record(
    document: usize,
    report: &ExceptionReport,
    muted: bool,
    error: &boa_engine::JsError,
    origin: (&boa_engine::JsString, u32, u32),
) {
    #[cfg(test)]
    if CAPTURE.with(|slot| {
        if let Some(records) = slot.borrow_mut().as_mut() {
            records.push(record_data(document, report, muted, error, origin));
            true
        } else {
            false
        }
    }) {
        return;
    }
    static ENABLED: OnceLock<bool> = OnceLock::new();
    if !*ENABLED.get_or_init(|| std::env::var_os("OMOIKANE_TRACE_CONSOLE").is_some())
        || !REMAINING.with(|remaining| {
            let count = remaining.get();
            remaining.set(count.saturating_sub(1));
            count != 0
        })
    {
        return;
    }
    eprintln!(
        "{}",
        crate::diagnostics::json_record(
            "OMOIKANE_EXCEPTION",
            &record_data(document, report, muted, error, origin)
        )
    );
}

fn preview(value: &boa_engine::JsString, limit: usize) -> (String, bool) {
    let mut characters = char::decode_utf16(value.iter())
        .map(|character| character.unwrap_or(char::REPLACEMENT_CHARACTER));
    let text = characters.by_ref().take(limit).collect();
    (text, characters.next().is_some())
}

fn record_data(
    document: usize,
    report: &ExceptionReport,
    muted: bool,
    error: &boa_engine::JsError,
    origin: (&boa_engine::JsString, u32, u32),
) -> serde_json::Value {
    let (message, message_truncated) = preview(&report.details.message, MESSAGE_CHARS);
    let (source, source_truncated) = source_preview(&report.details.filename);
    let (internal_message, internal_message_truncated) = internal_message(error);
    let (internal_source, internal_source_truncated) = source_preview(origin.0);
    serde_json::json!({
        "document_id":document, "message":message, "message_truncated":message_truncated,
        "source":source, "source_truncated":source_truncated,
        "line":report.details.line, "column":report.details.column,
        "canceled":!report.not_canceled, "muted":muted,
        "internal_message":internal_message, "internal_message_truncated":internal_message_truncated,
        "internal_source":internal_source, "internal_source_truncated":internal_source_truncated,
        "internal_line":origin.1, "internal_column":origin.2,
        "internal_frames":internal_frames(error)
    })
}

fn internal_frames(error: &boa_engine::JsError) -> Vec<serde_json::Value> {
    error.source_locations().take(12).map(|(source, line, column)| {
        let (source, truncated) = source_preview(&boa_engine::JsString::from(source.unwrap_or_default()));
        serde_json::json!({"source":source,"source_truncated":truncated,"line":line,"column":column})
    }).collect()
}

/// Reads only intrinsic strings or own data descriptors, never accessors or prototypes.
fn internal_message(error: &boa_engine::JsError) -> (Option<String>, bool) {
    if let Some(error) = error.as_native() {
        let mut chars = error.message().chars();
        let message = chars.by_ref().take(MESSAGE_CHARS).collect();
        return (Some(message), chars.next().is_some());
    }
    let Some(value) = error.as_opaque() else {
        return (None, false);
    };
    let message = value.as_string().or_else(|| {
        value
            .as_object()?
            .borrow()
            .properties()
            .get(&boa_engine::js_string!("message").into())?
            .value()?
            .as_string()
    });
    match message {
        Some(message) => {
            let (text, truncated) = preview(&message, MESSAGE_CHARS);
            (Some(text), truncated)
        }
        None => (None, false),
    }
}

fn source_preview(value: &boa_engine::JsString) -> (String, bool) {
    let (source, truncated) = preview(value, SOURCE_CHARS);
    if truncated {
        return ("<truncated-source>".into(), true);
    }
    let label = super::super::dom_removal_trace::source_label(&source);
    if label.chars().take(SOURCE_CHARS + 1).count() > SOURCE_CHARS {
        ("<truncated-source>".into(), true)
    } else {
        (label, false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::js::{
        JsRuntime,
        script_errors::{WorkerErrorReport, report_exception_with_muting},
    };
    use boa_engine::{JsError, JsNativeError, JsString};

    fn capture<T>(operation: impl FnOnce() -> T) -> (T, Vec<serde_json::Value>) {
        struct Reset;
        impl Drop for Reset {
            fn drop(&mut self) {
                CAPTURE.with(|slot| {
                    slot.replace(None);
                });
            }
        }
        assert!(
            CAPTURE
                .with(|slot| slot.replace(Some(Vec::new())))
                .is_none()
        );
        let reset = Reset;
        let result = operation();
        let reports = CAPTURE.with(|slot| slot.take().unwrap());
        drop(reset);
        (result, reports)
    }

    #[test]
    fn exception_trace_observes_canceled_error_without_repeating_author_conversion() {
        let mut runtime = JsRuntime::new().unwrap();
        runtime.eval("globalThis.reads=0;globalThis.seen='';addEventListener('error',e=>{seen=e.message;e.preventDefault()});globalThis.target=new EventTarget();target.addEventListener('probe',()=>{throw {get message(){reads++;return 'trace-probe'}}});").unwrap();
        let (result, reports) = capture(|| {
            runtime.eval(
                "target.dispatchEvent(new Event('probe')) && reads===1 && seen==='trace-probe'",
            )
        });
        assert!(result.unwrap().to_boolean());
        assert_eq!(reports.len(), 1);
        assert_eq!(reports[0]["message"], "trace-probe");
        assert_eq!(reports[0]["canceled"], true);
        assert_eq!(reports[0]["muted"], false);
        assert!(reports[0]["internal_message"].is_null());
    }

    #[test]
    fn exception_trace_preserves_muted_error_delivery_and_fields() {
        let mut runtime = JsRuntime::new().unwrap();
        runtime.eval("globalThis.seen='';addEventListener('error',e=>{seen=e.message+':'+e.filename+':'+e.lineno+':'+e.colno+':'+(e.error===null);e.preventDefault()});").unwrap();
        let document = runtime.host_state.borrow().document.identity();
        let (result, reports) = capture(|| {
            runtime.with_active_host(|context| {
                let error = JsError::from(JsNativeError::typ().with_message("private error"));
                report_exception_with_muting(context, &error, document, true)
            })
        });
        assert!(!result.unwrap());
        assert_eq!(
            runtime
                .eval("seen")
                .unwrap()
                .as_string()
                .unwrap()
                .to_std_string_lossy(),
            "Script error.::0:0:true"
        );
        assert_eq!(reports.len(), 1);
        assert_eq!(reports[0]["message"], "Script error.");
        assert_eq!(reports[0]["source"], "<anonymous-or-local>");
        assert_eq!(reports[0]["line"], 0);
        assert_eq!(reports[0]["column"], 0);
        assert_eq!(reports[0]["muted"], true);
        assert_eq!(reports[0]["internal_message"], "private error");
        assert_eq!(reports[0]["canceled"], true);
    }

    #[test]
    fn exception_trace_captured_locations_preserve_nearest_frame_without_author_stack_reads() {
        let mut runtime = JsRuntime::new().unwrap();
        for body in [
            "const value=null;value()",
            "throw {get stack(){stackReads++;throw 'unexpected getter'}}",
        ] {
            let source = format!(
                "globalThis.stackReads=0;function leaf(){{{body}}}function middle(){{leaf()}}function outer(){{middle()}}outer()"
            );
            let error = runtime.eval(&source).unwrap_err();
            let locations: Vec<_> = error.source_locations().collect();
            assert!(locations.len() >= 3, "{locations:?}");
            assert_eq!(error.source_location(), locations.first().cloned());
            assert_eq!(
                error.clone().source_locations().collect::<Vec<_>>(),
                locations
            );
            assert!(
                locations
                    .iter()
                    .all(|(_, line, column)| *line > 0 && *column > 0)
            );
            assert!(internal_frames(&error).len() <= 12);
            assert_eq!(runtime.eval("stackReads").unwrap().as_number(), Some(0.0));
        }
        let empty = JsError::from(JsNativeError::typ());
        assert_eq!(empty.source_location(), None);
        assert_eq!(empty.source_locations().count(), 0);
    }

    #[test]
    fn exception_trace_opaque_data_message_does_not_call_to_string_or_getters() {
        let mut runtime = JsRuntime::new().unwrap();
        let value = runtime.eval("globalThis.reads=0;({message:'own detail',get name(){reads++;throw 1},toString(){reads++;throw 2}})").unwrap();
        let error = JsError::from_opaque(value);
        assert_eq!(internal_message(&error), (Some("own detail".into()), false));
        assert_eq!(runtime.eval("reads").unwrap().as_number(), Some(0.0));
        let value = runtime.eval("({get message(){reads++;throw 3}})").unwrap();
        assert_eq!(
            internal_message(&JsError::from_opaque(value)),
            (None, false)
        );
        assert_eq!(runtime.eval("reads").unwrap().as_number(), Some(0.0));
    }

    #[test]
    fn exception_trace_bounds_unicode_and_redacts_source_credentials() {
        let mut report = ExceptionReport {
            not_canceled: true,
            details: WorkerErrorReport {
                message: JsString::from("🦀".repeat(MESSAGE_CHARS + 1)),
                filename: JsString::from(
                    "https://user:secret@example.test/a.js?token=secret#secret",
                ),
                line: 17,
                column: 23,
            },
        };
        let error =
            JsError::from(JsNativeError::typ().with_message("🦀".repeat(MESSAGE_CHARS + 1)));
        let data_for = |report: &ExceptionReport| {
            record_data(5, report, false, &error, (&report.details.filename, 17, 23))
        };
        let data = data_for(&report);
        assert_eq!(
            data["message"].as_str().unwrap(),
            "🦀".repeat(MESSAGE_CHARS)
        );
        assert_eq!(data["message_truncated"], true);
        assert_eq!(
            data["internal_message"].as_str().unwrap(),
            "🦀".repeat(MESSAGE_CHARS)
        );
        assert_eq!(data["internal_message_truncated"], true);
        assert_eq!(data["internal_source"], "https://example.test/a.js");
        assert_eq!(data["internal_line"], 17);
        assert_eq!(data["internal_column"], 23);
        assert_eq!(data["source"], "https://example.test/a.js");
        assert_eq!(data["line"], 17);
        assert_eq!(data["column"], 23);
        assert_eq!(data["canceled"], false);
        report.details.filename = JsString::from("a".repeat(SOURCE_CHARS + 1));
        let data = data_for(&report);
        assert_eq!(data["source"], "<truncated-source>");
        assert_eq!(data["source_truncated"], true);
        report.details.filename =
            JsString::from(format!("https://example.test/{}", "界".repeat(1000)));
        let data = data_for(&report);
        assert_eq!(data["source"], "<truncated-source>");
        assert_eq!(data["source_truncated"], true);
        assert_eq!(
            preview(&JsString::from("short"), MESSAGE_CHARS),
            ("short".into(), false)
        );
    }
}
