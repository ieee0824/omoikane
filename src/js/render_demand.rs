//! Live animation setup and native snapshots for the host's read-only demand query.

use super::*;
use crate::cdp::NextRendering;
use std::time::{Duration, SystemTime};

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
        let timer_delay = state
            .event_loop
            .next_timer_delay_ms()
            .map(Duration::from_millis);
        let cookie_delay = if state.worker_id.is_some()
            || state.shared_worker.is_global()
            || state.worklet.is_child_global()
            || state.cookie_change_callbacks.is_empty()
        {
            // Worker globals can retain bootstrap-private CookieStore callbacks
            // after removing the page-facing API. They do not deliver change
            // events, and scheduling them for cookie expiry would keep their
            // owner Window in EveryFrame mode for the cookie's lifetime.
            None
        } else {
            let cookie_store = state.cookie_store.lock().unwrap();
            // Another Window can remove an expired cookie from the shared jar.
            // Keep this idle Window runnable until it consumes that change.
            if cookie_store.change_sequence() != state.cookie_change_sequence {
                Some(Duration::ZERO)
            } else {
                cookie_store.next_expiry_delay_at(SystemTime::now())
            }
        };
        match (timer_delay, cookie_delay) {
            (Some(page_delay), Some(wall_delay)) => NextRendering::AfterBoth {
                page_delay,
                wall_delay,
            },
            (Some(delay), None) => NextRendering::After(delay),
            (None, Some(delay)) => NextRendering::AfterWall(delay),
            (None, None) => NextRendering::Idle,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expiring_cookie_wakes_an_otherwise_idle_window() {
        let document = crate::html::TreeBuilder::parse("<body></body>").document();
        let mut runtime =
            JsRuntime::with_document_and_url(document, "https://example.test/page").unwrap();
        runtime
            .eval("cookieStore.set({name:'wake', value:'1', expires:Date.now()+60000})")
            .unwrap();
        runtime.run_until_idle().unwrap();

        let NextRendering::AfterWall(delay) = runtime.next_rendering() else {
            panic!("an expiring cookie should wake the idle renderer");
        };
        assert!(delay > Duration::ZERO && delay <= Duration::from_secs(60));
    }

    #[test]
    fn script_timer_and_cookie_expiry_keep_separate_deadlines() {
        let document = crate::html::TreeBuilder::parse("<body></body>").document();
        let mut runtime =
            JsRuntime::with_document_and_url(document, "https://example.test/page").unwrap();
        runtime
            .eval(
                "cookieStore.set({name:'wake', value:'1', expires:Date.now()+60000}); setTimeout(() => {}, 30000)",
            )
            .unwrap();
        runtime.run_until_idle().unwrap();

        let NextRendering::AfterBoth {
            page_delay,
            wall_delay,
        } = runtime.next_rendering()
        else {
            panic!("script timer and cookie expiry must retain separate clock sources");
        };
        assert_eq!(page_delay, Duration::from_secs(30));
        assert!(wall_delay > Duration::ZERO && wall_delay <= Duration::from_secs(60));
    }

    #[test]
    fn shared_cookie_expiration_wakes_another_idle_window() {
        let shared = Arc::new(Mutex::new(crate::http::CookieJar::new()));
        let url = "https://example.test/page";
        let document = || crate::html::TreeBuilder::parse("<body></body>").document();
        let mut first = JsRuntime::with_document_and_url(document(), url).unwrap();
        let mut second = JsRuntime::with_document_and_url(document(), url).unwrap();
        first.set_shared_cookie_store(Arc::clone(&shared));
        second.set_shared_cookie_store(Arc::clone(&shared));
        second
            .eval(
                "globalThis.cookieDeletions = []; cookieStore.onchange = event => cookieDeletions.push(...event.deleted.map(cookie => cookie.name));",
            )
            .unwrap();
        assert_eq!(second.next_rendering(), NextRendering::Idle);
        struct WakeCounter(std::sync::atomic::AtomicUsize);
        impl std::task::Wake for WakeCounter {
            fn wake(self: Arc<Self>) {
                self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            }
        }
        let wake_counter = Arc::new(WakeCounter(std::sync::atomic::AtomicUsize::new(0)));
        let changes = shared
            .lock()
            .unwrap()
            .subscribe_changes(std::task::Waker::from(Arc::clone(&wake_counter)));
        let other_thread_store = Arc::clone(&shared);
        std::thread::spawn(move || {
            other_thread_store.lock().unwrap().add_from_header_for_url(
                "shared=one; Max-Age=60; Secure; Path=/",
                &url.parse().unwrap(),
            );
        })
        .join()
        .unwrap();
        assert_eq!(wake_counter.0.load(std::sync::atomic::Ordering::SeqCst), 1);
        assert!(
            changes.take_changed(),
            "an idle host must receive the external mutation"
        );
        assert_eq!(
            second.next_rendering(),
            NextRendering::AfterWall(Duration::ZERO)
        );
        second.tick(0).unwrap();
        let NextRendering::AfterWall(delay) = second.next_rendering() else {
            panic!("the second Window should schedule the shared cookie expiry");
        };
        assert!(delay > Duration::ZERO);
        let before_expiry_sequence = shared.lock().unwrap().change_sequence();

        // Advance the jar's explicit sweep time rather than sleeping; the
        // separate integration test covers a real wall-clock deadline.
        assert_eq!(
            shared
                .lock()
                .unwrap()
                .evict_expired_at(SystemTime::now() + Duration::from_secs(61)),
            1
        );
        assert_eq!(
            shared.lock().unwrap().change_sequence(),
            before_expiry_sequence + 1
        );
        assert_eq!(wake_counter.0.load(std::sync::atomic::Ordering::SeqCst), 2);
        assert!(
            changes.take_changed(),
            "an expiry sweep must wake each subscribed host"
        );
        first.tick(0).unwrap();
        assert_eq!(shared.lock().unwrap().len(), 0);
        assert_eq!(
            second.next_rendering(),
            NextRendering::AfterWall(Duration::ZERO)
        );
        second.tick(0).unwrap();
        assert!(
            second
                .eval("cookieDeletions.join(',') === 'shared'")
                .unwrap()
                .to_boolean()
        );
        assert_eq!(second.next_rendering(), NextRendering::Idle);
        second.tick(0).unwrap();
        assert_eq!(
            shared.lock().unwrap().change_sequence(),
            before_expiry_sequence + 1
        );
        assert!(
            second
                .eval("cookieDeletions.join(',') === 'shared'")
                .unwrap()
                .to_boolean()
        );
    }

    #[test]
    fn invisible_shared_cookie_change_does_not_keep_a_window_awake() {
        let shared = Arc::new(Mutex::new(crate::http::CookieJar::new()));
        let document = crate::html::TreeBuilder::parse("<body></body>").document();
        let mut runtime =
            JsRuntime::with_document_and_url(document, "https://example.test/page").unwrap();
        runtime.set_shared_cookie_store(Arc::clone(&shared));
        runtime
            .eval("globalThis.changes = 0; cookieStore.onchange = () => changes++")
            .unwrap();
        assert_eq!(runtime.next_rendering(), NextRendering::Idle);

        shared.lock().unwrap().add_from_header_for_url(
            "foreign=one; Secure; Path=/",
            &"https://other.test/".parse().unwrap(),
        );
        assert_eq!(
            runtime.next_rendering(),
            NextRendering::AfterWall(Duration::ZERO)
        );
        runtime.tick(0).unwrap();
        assert_eq!(runtime.next_rendering(), NextRendering::Idle);
        assert!(runtime.eval("changes === 0").unwrap().to_boolean());
    }

    #[test]
    fn a_secure_worker_global_does_not_spin_for_cookie_expiration() {
        let url = "https://example.test/page";
        let document = crate::html::TreeBuilder::parse("<body></body>").document();
        let mut runtime = JsRuntime::with_document_and_url(document, url).unwrap();
        assert!(
            !runtime
                .host_state
                .borrow()
                .cookie_change_callbacks
                .is_empty()
        );
        runtime
            .eval("__omoikane_install_worker_global('https://example.test/worker.js', '1')")
            .unwrap();
        assert!(
            runtime
                .eval("typeof cookieStore === 'undefined'")
                .unwrap()
                .to_boolean()
        );
        runtime.host_state.borrow_mut().worker_id = Some(1);
        let cookie_store = Arc::clone(&runtime.host_state.borrow().cookie_store);
        cookie_store.lock().unwrap().add_from_header_for_url(
            "background=one; Max-Age=60; Secure; Path=/",
            &url.parse().unwrap(),
        );
        assert_eq!(runtime.next_rendering(), NextRendering::Idle);
    }

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
