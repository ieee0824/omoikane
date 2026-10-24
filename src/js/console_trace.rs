//! Optional bounded output of already-formatted console messages.
use std::cell::Cell;
use std::sync::OnceLock;

const MESSAGE_CHARS: usize = 4096;
thread_local! {
    static ERRORS: Cell<usize> = const { Cell::new(0) };
    static OTHER: Cell<usize> = const { Cell::new(0) };
}

/// Emits no author callbacks; the console binding has already formatted values.
pub(super) fn record(context: &boa_engine::Context, message: &str) {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    if !*ENABLED.get_or_init(|| std::env::var_os("OMOIKANE_TRACE_CONSOLE").is_some()) {
        return;
    }
    let allowed = if message.starts_with("[error]") {
        ERRORS.with(|count| take_budget(count, 128))
    } else {
        OTHER.with(|count| take_budget(count, 32))
    };
    if allowed {
        eprintln!(
            "{}",
            crate::diagnostics::json_record(
                "OMOIKANE_CONSOLE",
                &record_data(super::caller_document_id(context), message)
            )
        );
    }
}

fn take_budget(count: &Cell<usize>, limit: usize) -> bool {
    if count.get() == limit {
        return false;
    }
    count.set(count.get() + 1);
    true
}

fn record_data(document_id: Option<usize>, message: &str) -> serde_json::Value {
    let mut characters = message.chars();
    let preview: String = characters.by_ref().take(MESSAGE_CHARS).collect();
    serde_json::json!({"document_id":document_id,"message":preview,"truncated":characters.next().is_some()})
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn console_preview_keeps_unicode_and_bounds_output_on_characters() {
        let message = "あ".repeat(MESSAGE_CHARS + 1);
        let data = record_data(Some(17), &message);
        assert_eq!(data["document_id"], 17);
        assert_eq!(
            data["message"].as_str().unwrap().chars().count(),
            MESSAGE_CHARS
        );
        assert_eq!(data["truncated"], true);
        assert_eq!(
            record_data(None, "[error] boom\nline")["message"],
            "[error] boom\nline"
        );
        assert_eq!(record_data(None, "plain")["truncated"], false);
    }

    #[test]
    fn console_formats_author_values_once_and_retains_the_complete_buffer() {
        let mut runtime = super::super::JsRuntime::new().unwrap();
        assert!(runtime.eval("globalThis.calls=0;console.error({toString(){calls++;return 'probe'}});calls===1").unwrap().to_boolean());
        assert_eq!(runtime.console_logs(), vec!["[error] probe"]);
    }
}
