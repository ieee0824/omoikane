//! Optional per-rendering-opportunity diagnostics for synchronous page work.

use std::cell::RefCell;
use std::sync::OnceLock;
use std::time::Instant;

fn raf_enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| std::env::var_os("OMOIKANE_TRACE_RAF").is_some())
}

fn source_label(source: &str) -> String {
    match url::Url::parse(source) {
        Ok(mut url) if matches!(url.scheme(), "http" | "https") => {
            let _ = url.set_username("");
            let _ = url.set_password(None);
            url.set_query(None);
            url.set_fragment(None);
            url.to_string()
        }
        Ok(url) => format!("{}:", url.scheme()),
        Err(_) => "<inline-or-local>".into(),
    }
}

pub(super) fn raf_source(context: &boa_engine::Context) -> Option<serde_json::Value> {
    if !raf_enabled() {
        return None;
    }
    // requestAnimationFrame is a direct public native binding, so the first
    // JavaScript frame is its caller; no trusted-bootstrap frame is skipped.
    let frame = context.stack_trace().next()?;
    let location = frame.position();
    let position = location.position?;
    let file = match &location.path {
        boa_engine::vm::SourcePath::Path(path) => source_label(&path.to_string_lossy()),
        _ => "<anonymous>".into(),
    };
    Some(serde_json::json!({
        "file": file, "line": position.line_number(), "column": position.column_number(),
    }))
}

pub(super) fn record_raf_schedule(
    id: u64,
    document: Option<usize>,
    source: Option<serde_json::Value>,
) {
    if raf_enabled() {
        eprintln!(
            "OMOIKANE_RAF_SCHEDULE {}",
            serde_json::json!({
                "id": id, "document_id": document, "source": source,
            })
        );
    }
}

pub(super) fn start_raf_callback() -> Option<Instant> {
    raf_enabled().then(Instant::now)
}

pub(super) fn record_raf_execution(id: u64, document: Option<usize>, started: Option<Instant>) {
    if let Some(started) = started {
        eprintln!(
            "OMOIKANE_RAF_EXECUTE {}",
            serde_json::json!({
                "id": id, "document_id": document,
                "duration_ms": started.elapsed().as_secs_f64() * 1000.0,
            })
        );
    }
}

#[derive(Default)]
struct FrameSample {
    document_id: usize,
    scroll_events_ms: f64,
    media_ms: f64,
    raf_call_ms: f64,
    jobs_ms: f64,
    layout_calls: usize,
    layout_rebuilds: usize,
    layout_ms: f64,
}

thread_local! {
    // A stack keeps nested rendering opportunities and documents separate.
    static FRAMES: RefCell<Vec<FrameSample>> = const { RefCell::new(Vec::new()) };
}

pub(super) struct FrameScope {
    started: Option<Instant>,
}

impl FrameScope {
    pub(super) fn begin(enabled: bool, document_id: usize) -> Self {
        if enabled {
            FRAMES.with(|frames| {
                frames.borrow_mut().push(FrameSample {
                    document_id,
                    ..FrameSample::default()
                })
            });
        }
        Self {
            started: enabled.then(Instant::now),
        }
    }

    pub(super) fn stage_start(&self) -> Option<Instant> {
        self.started.map(|_| Instant::now())
    }

    pub(super) fn record_scroll_events(&self, started: Option<Instant>) {
        self.record(started, |sample, ms| sample.scroll_events_ms += ms);
    }

    pub(super) fn record_media(&self, started: Option<Instant>) {
        self.record(started, |sample, ms| sample.media_ms += ms);
    }

    pub(super) fn record_raf_call(&self, started: Option<Instant>) {
        self.record(started, |sample, ms| sample.raf_call_ms += ms);
    }

    pub(super) fn record_jobs(&self, started: Option<Instant>) {
        self.record(started, |sample, ms| sample.jobs_ms += ms);
    }

    fn record(&self, started: Option<Instant>, update: impl FnOnce(&mut FrameSample, f64)) {
        if let Some(started) = started {
            let ms = started.elapsed().as_secs_f64() * 1000.0;
            FRAMES.with(|frames| {
                if let Some(sample) = frames.borrow_mut().last_mut() {
                    update(sample, ms);
                }
            });
        }
    }

    pub(super) fn finish(mut self, callbacks_ms: f64) {
        if self.started.take().is_some() {
            emit_frame_sample(callbacks_ms, true);
        }
    }
}

impl Drop for FrameScope {
    fn drop(&mut self) {
        if let Some(started) = self.started.take() {
            emit_frame_sample(started.elapsed().as_secs_f64() * 1000.0, false);
        }
    }
}

fn emit_frame_sample(callbacks_ms: f64, completed: bool) {
    let sample = FRAMES.with(|frames| frames.borrow_mut().pop());
    if let Some(sample) = sample {
        eprintln!(
            "OMOIKANE_FRAME_CALLBACK_DETAIL {}",
            serde_json::json!({
                "document_id": sample.document_id,
                "callbacks_ms": callbacks_ms,
                "completed": completed,
                "scroll_events_ms": sample.scroll_events_ms,
                "media_ms": sample.media_ms,
                "raf_call_ms": sample.raf_call_ms,
                "jobs_ms": sample.jobs_ms,
                "ensure_layout_calls": sample.layout_calls,
                "ensure_layout_rebuilds": sample.layout_rebuilds,
                "ensure_layout_ms": sample.layout_ms,
            })
        );
    }
}

pub(super) struct LayoutSample {
    started: Option<Instant>,
    rebuilt: bool,
}

impl LayoutSample {
    pub(super) fn begin() -> Self {
        let enabled = FRAMES.with(|frames| !frames.borrow().is_empty());
        Self {
            started: enabled.then(Instant::now),
            rebuilt: false,
        }
    }

    pub(super) fn mark_rebuild(&mut self) {
        self.rebuilt = true;
    }
}

impl Drop for LayoutSample {
    fn drop(&mut self) {
        let Some(started) = self.started else {
            return;
        };
        let ms = started.elapsed().as_secs_f64() * 1000.0;
        FRAMES.with(|frames| {
            if let Some(sample) = frames.borrow_mut().last_mut() {
                sample.layout_calls += 1;
                sample.layout_rebuilds += usize::from(self.rebuilt);
                sample.layout_ms += ms;
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn raf_source_labels_omit_credentials_queries_fragments_and_inline_code() {
        assert_eq!(
            source_label("https://user:password@example.test/render.js?token=secret#part"),
            "https://example.test/render.js"
        );
        assert_eq!(
            source_label("data:text/javascript,secret_inline_code"),
            "data:"
        );
        assert_eq!(source_label("/private/user/path.js"), "<inline-or-local>");
    }

    fn frame_depth() -> usize {
        FRAMES.with(|frames| frames.borrow().len())
    }

    #[test]
    fn disabled_scope_has_no_frame_or_clock() {
        assert_eq!(frame_depth(), 0);
        let scope = FrameScope::begin(false, 1);
        assert!(scope.stage_start().is_none());
        assert!(LayoutSample::begin().started.is_none());
        scope.finish(0.0);
        assert_eq!(frame_depth(), 0);
    }

    #[test]
    fn nested_scopes_and_aborted_scope_pop_once() {
        assert_eq!(frame_depth(), 0);
        let outer = FrameScope::begin(true, 11);
        {
            let _aborted = FrameScope::begin(true, 22);
            assert_eq!(frame_depth(), 2);
            FRAMES.with(|frames| assert_eq!(frames.borrow().last().unwrap().document_id, 22));
        }
        assert_eq!(frame_depth(), 1);
        FRAMES.with(|frames| assert_eq!(frames.borrow().last().unwrap().document_id, 11));
        outer.finish(0.0);
        assert_eq!(frame_depth(), 0);
    }

    #[test]
    fn layout_sample_counts_calls_and_rebuilds_in_active_frame() {
        assert_eq!(frame_depth(), 0);
        let scope = FrameScope::begin(true, 33);
        drop(LayoutSample::begin());
        let mut rebuild = LayoutSample::begin();
        rebuild.mark_rebuild();
        drop(rebuild);
        FRAMES.with(|frames| {
            let frames = frames.borrow();
            let sample = frames.last().unwrap();
            assert_eq!(sample.layout_calls, 2);
            assert_eq!(sample.layout_rebuilds, 1);
        });
        scope.finish(0.0);
        assert_eq!(frame_depth(), 0);
    }
}
