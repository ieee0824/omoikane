//! Live animation setup and native snapshots for the host's read-only demand query.

use super::*;
use crate::cdp::NextRendering;
use std::time::Duration;

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct RuntimePaintKey {
    document: usize,
    style: u64,
    layout: u64,
    paint: u64,
    scroll: u64,
    viewport: Rect,
    visual_viewport: VisualViewportState,
    window_scroll: (f32, f32),
    focus: Vec<usize>,
    focus_visible: Option<usize>,
    selection: Vec<usize>,
    visited_links: u64,
    images: Vec<(Arc<crate::paint::animation::ImagePlayback>, usize)>,
}

impl JsRuntime {
    /// Selects the browser's live animation clock rather than static snapshots.
    pub(crate) fn enable_live_css_animations(&mut self) {
        let mut state = self.host_state.borrow_mut();
        state.live_css_animations = true;
        state.mark_all_document_styles_dirty();
    }

    pub(crate) fn paint_state_key(&self) -> RuntimePaintKey {
        let state = self.host_state.borrow();
        let mut selection: Vec<_> = state
            .content_visibility_selection_nodes
            .iter()
            .copied()
            .collect();
        selection.sort_unstable();
        RuntimePaintKey {
            document: state.document.identity(),
            style: state.style_generation,
            layout: state.layout_generation,
            paint: state.paint_generation,
            scroll: state.scroll_generation,
            viewport: state.viewport,
            visual_viewport: state.visual_viewport,
            window_scroll: state.window_scroll,
            focus: state.focus_subjects.clone(),
            focus_visible: state.focus_visible_id,
            selection,
            visited_links: state.storage_manager.visited_generation(),
            images: state
                .visible_image_playbacks
                .iter()
                .map(|playback| {
                    (
                        Arc::clone(playback),
                        playback.frame_index(state.event_loop.rendering_time_ms() as u64),
                    )
                })
                .collect(),
        }
    }

    /// Reads pending queues and native state, without polling futures or
    /// constructing computed styles. Background work is initially polled each
    /// frame until the worker/worklet/module queues have drained.
    pub(crate) fn next_rendering(&self) -> NextRendering {
        let state = self.host_state.borrow();
        let visual_work = !state.page_hidden
            && (state.pending_media_query_report
                || state.event_loop.has_pending_animation_frames()
                || !state.smooth_scrolls.is_empty()
                || state
                    .visible_image_playbacks
                    .iter()
                    .any(|playback| playback.running(state.event_loop.rendering_time_ms() as u64))
                || state.document_styles.values().any(|entry| {
                    entry.resolver.as_ref().is_some_and(|resolver| {
                        resolver.has_running_transitions() || resolver.has_running_animations()
                    })
                }));
        // Completed downloads still need an opportunity to resume their promise.
        let modules = !self.module_loader.pending.borrow().is_empty();
        let workers = state.workers.values().any(|worker| {
            let worker = worker.borrow();
            !worker.terminated
                && (!worker.outgoing.is_empty()
                    || worker.startup_error.is_some()
                    || worker.runtime.next_rendering() != NextRendering::Idle)
        });
        if visual_work || modules || workers || state.worklet.has_pending_render_work() {
            return NextRendering::EveryFrame;
        }
        state
            .event_loop
            .next_timer_delay_ms()
            .map(|delay| NextRendering::After(Duration::from_millis(delay)))
            .unwrap_or(NextRendering::Idle)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shared_visited_history_invalidates_the_private_paint_key() {
        let url = "http://127.0.0.1/page";
        let document = crate::html::TreeBuilder::parse("<style>body{margin:0}a{display:block;width:40px;height:40px;background:red}a:visited{background:green}</style><a href='/destination'></a>").document();
        let mut runtime = JsRuntime::with_document_and_url(document, url).unwrap();
        runtime.set_viewport(100.0, 100.0);
        assert_eq!(
            runtime.paint_current_document().unwrap().pixel(20, 20),
            Some(crate::paint::Color::rgb(255, 0, 0))
        );
        let presented = runtime.paint_state_key();
        let storage = runtime.host_state.borrow().storage_manager.clone();
        let source = VisitSource::new(StorageOrigin::from_url(url).unwrap(), url).unwrap();
        storage.record_page_navigation(
            "http://127.0.0.1/destination",
            "http://127.0.0.1/destination",
            source,
        );
        assert_ne!(runtime.paint_state_key(), presented);
        assert_eq!(
            runtime.paint_current_document().unwrap().pixel(20, 20),
            Some(crate::paint::Color::rgb(0, 128, 0))
        );
    }

    #[test]
    fn background_http_fetch_remains_pollable_until_consumed() {
        use crate::test_support::http_fixture::{
            ACCEPT_TIMEOUT, READ_TIMEOUT, accept_with_timeout, bind_loopback, read_request_headers,
        };
        use std::io::Write;
        use std::sync::mpsc;
        let listener = bind_loopback().unwrap();
        let url = format!("http://{}/module.js", listener.local_addr().unwrap());
        let (started_tx, started_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let server = std::thread::spawn(move || {
            let mut stream = accept_with_timeout(&listener, ACCEPT_TIMEOUT).unwrap();
            read_request_headers(&mut stream, READ_TIMEOUT).unwrap();
            started_tx.send(()).unwrap();
            release_rx.recv_timeout(Duration::from_secs(5)).unwrap();
            let body = "export const result=42;";
            write!(stream,"HTTP/1.1 200 OK\r\nContent-Type: text/javascript\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",body.len()).unwrap();
        });
        let runtime = JsRuntime::new().unwrap();
        let pool =
            module_fetch::ModuleFetchPool::new(runtime.host_state.borrow().cookie_store.clone())
                .unwrap();
        let mut fetch = pool.fetch(url.clone(), false, None);
        let key = (runtime.document().identity(), url);
        runtime
            .module_loader
            .pending
            .borrow_mut()
            .insert(key.clone(), fetch.clone());
        started_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_eq!(runtime.next_rendering(), NextRendering::EveryFrame);
        release_tx.send(()).unwrap();
        server.join().unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        let mut context = std::task::Context::from_waker(std::task::Waker::noop());
        let result = loop {
            if let std::task::Poll::Ready(result) =
                std::pin::Pin::new(&mut fetch).poll(&mut context)
            {
                break result.unwrap();
            }
            assert!(
                std::time::Instant::now() < deadline,
                "HTTP download did not settle"
            );
            std::thread::sleep(Duration::from_millis(1));
        };
        assert_eq!(result.response.body(), b"export const result=42;");
        assert_eq!(runtime.next_rendering(), NextRendering::EveryFrame);
        runtime.module_loader.pending.borrow_mut().remove(&key);
        assert_eq!(runtime.next_rendering(), NextRendering::Idle);
    }

    #[test]
    fn worklet_timers_are_polled_until_the_isolated_runtime_finishes() {
        let mut runtime = JsRuntime::new().unwrap();
        runtime.eval("CSS.paintWorklet.addModule('data:text/javascript,'+encodeURIComponent(\"setTimeout(()=>registerWorklet('finished'),100)\"))").unwrap();
        runtime.run_until_idle().unwrap();
        assert_eq!(runtime.next_rendering(), NextRendering::EveryFrame);
        runtime.run_animation_frame(99).unwrap();
        assert_eq!(runtime.next_rendering(), NextRendering::EveryFrame);
        runtime.run_animation_frame(1).unwrap();
        assert_eq!(runtime.next_rendering(), NextRendering::Idle);
        let result = runtime
            .eval("CSS.paintWorklet.registeredNames.join(',')")
            .unwrap();
        assert_eq!(
            result
                .to_string(&mut runtime.context)
                .unwrap()
                .to_std_string_escaped(),
            "finished"
        );
    }

    #[test]
    fn completed_module_fetch_stays_pollable_until_its_owner_consumes_the_result() {
        let runtime = JsRuntime::new().unwrap();
        let key = (
            runtime.document().identity(),
            "http://module.test/module.js".to_string(),
        );
        let fetch = module_fetch::ModuleFetch::default();
        runtime
            .module_loader
            .pending
            .borrow_mut()
            .insert(key.clone(), fetch.clone());
        assert_eq!(runtime.next_rendering(), NextRendering::EveryFrame);
        fetch.cancel();
        // The download is complete but the suspended import has not resumed.
        assert_eq!(runtime.next_rendering(), NextRendering::EveryFrame);
        runtime.module_loader.pending.borrow_mut().remove(&key);
        assert_eq!(runtime.next_rendering(), NextRendering::Idle);
    }
}
