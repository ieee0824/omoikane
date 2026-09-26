//! Best-effort panic observation that preserves the hook installed by the host.

use std::{
    cell::Cell,
    panic::{self, PanicHookInfo},
    sync::{Arc, Mutex, Once, OnceLock, Weak},
};

use super::{ErrorCategory, ErrorCode, ErrorReporter, ErrorSeverity, ExecutionSurface, RawEvent};

type HookTarget = (Weak<ErrorReporter>, ExecutionSurface);

static TARGET: OnceLock<Mutex<Option<HookTarget>>> = OnceLock::new();
static INSTALL: Once = Once::new();

thread_local! {
    static REPORTING: Cell<bool> = const { Cell::new(false) };
}

/// Wraps the current process panic hook once and sends only coarse, sanitized
/// panic metadata to an enabled reporter.
///
/// Repeated calls update the reporter without adding another hook wrapper.
/// The previous hook always runs after the best-effort report attempt. The
/// reporter is held weakly, so a dropped reporter cannot keep the process or
/// its storage worker alive. A later host hook replacement takes precedence.
/// Returns `true` only when this call installed the wrapper.
pub fn install_panic_reporter(reporter: &Arc<ErrorReporter>, surface: ExecutionSurface) -> bool {
    if !reporter.records_locally() {
        return false;
    }
    let target = TARGET.get_or_init(|| Mutex::new(None));
    let Ok(mut slot) = target.lock() else {
        return false;
    };
    *slot = Some((Arc::downgrade(reporter), surface));
    drop(slot);

    let mut installed = false;
    INSTALL.call_once(|| {
        let previous = panic::take_hook();
        panic::set_hook(Box::new(move |info| {
            report_panic(info);
            previous(info);
        }));
        installed = true;
    });
    installed
}

fn report_panic(info: &PanicHookInfo<'_>) {
    with_report_guard(|| {
        let destination = TARGET
            .get()
            .and_then(|target| target.lock().ok())
            .and_then(|target| target.clone());
        let Some((reporter, surface)) = destination else {
            return;
        };
        let Some(reporter) = reporter.upgrade() else {
            return;
        };
        let origin = info
            .location()
            .map(|location| coarse_origin(location.file()))
            .unwrap_or("other");
        let kind = if info.payload().is::<String>() || info.payload().is::<&'static str>() {
            "string"
        } else {
            "other"
        };
        reporter.report(
            RawEvent::new(
                ErrorCategory::Internal,
                ErrorSeverity::Critical,
                ErrorCode::new("UNEXPECTED_PANIC").expect("static panic code"),
                surface,
                "Unexpected panic",
                &[
                    ("operation", "execute"),
                    ("failure_kind", "internal"),
                    ("panic_origin", origin),
                    ("panic_kind", kind),
                ],
            )
            .sanitize(),
        );
    });
}

fn coarse_origin(file: &str) -> &'static str {
    if file.starts_with("engine/") || file.contains("/engine/") {
        "engine"
    } else if file.starts_with("tests/") || file.contains("/tests/") {
        "tests"
    } else if file.starts_with("src/") || file.contains("/src/") {
        "src"
    } else {
        "other"
    }
}

fn with_report_guard(action: impl FnOnce()) -> bool {
    REPORTING
        .try_with(|reporting| {
            if reporting.replace(true) {
                return false;
            }
            struct Reset<'a>(&'a Cell<bool>);
            impl Drop for Reset<'_> {
                fn drop(&mut self) {
                    self.0.set(false);
                }
            }
            let _reset = Reset(reporting);
            action();
            true
        })
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recursion_guard_skips_nested_reporting_and_recovers_afterward() {
        assert!(with_report_guard(|| {
            assert!(!with_report_guard(|| panic!("nested report must not run")));
        }));
        assert!(with_report_guard(|| {}));
    }

    #[test]
    fn absolute_paths_collapse_to_fixed_origin_buckets() {
        assert_eq!(coarse_origin("/home/private/project/src/lib.rs"), "src");
        assert_eq!(
            coarse_origin("/home/private/project/tests/case.rs"),
            "tests"
        );
        assert_eq!(
            coarse_origin("/home/private/project/engine/boa/src/lib.rs"),
            "engine"
        );
        assert_eq!(coarse_origin("/home/private/SECRET.rs"), "other");
    }
}
