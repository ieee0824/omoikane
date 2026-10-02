//! Native page-clock synchronization and demand-driven frame composition.

use super::*;

#[derive(PartialEq)]
pub(super) struct PresentedChrome {
    layout: ChromeLayout,
    url_bar: UrlBar,
}

pub(super) struct PaintTrace {
    sequence: u64,
    started_at: Instant,
}

impl PaintTrace {
    pub(super) fn new(started_at: Instant) -> Self {
        Self {
            sequence: 0,
            started_at,
        }
    }

    fn record(&mut self) {
        self.sequence += 1;
        let record = json!({
            "sequence": self.sequence,
            "elapsed_ms": self.started_at.elapsed().as_millis(),
        });
        eprintln!("OMOIKANE_PAINT {record}");
    }
}

impl BrowserApp {
    /// Returns elapsed milliseconds since the last clock update, or zero
    /// immediately after switching documents, so old idle time is discarded.
    pub(super) fn begin_page_frame(&mut self, now: Instant) -> u64 {
        let delta = self.frame_scheduler.begin_frame(now);
        self.page_delta_for_document(delta)
    }

    /// Advances the input clock and returns elapsed milliseconds; a new
    /// document receives zero rather than the previous document's idle time.
    fn advance_page_clock(&mut self, now: Instant) -> u64 {
        let delta = self.frame_scheduler.advance_time(now);
        self.page_delta_for_document(delta)
    }

    fn page_delta_for_document(&mut self, delta: u64) -> u64 {
        let document = self.session.document_generation();
        if self.clock_document == document {
            delta
        } else {
            self.clock_document = document;
            0
        }
    }

    fn rebase_clock_if_document_changed(&mut self) {
        if self.clock_document != self.session.document_generation() {
            self.advance_page_clock(Instant::now());
        }
    }

    pub(super) fn sync_page_time_before_input(&mut self, now: Instant) {
        let elapsed = self.advance_page_clock(now);
        if let Err(error) = self.session.advance_tasks(elapsed) {
            report_gui_failure(self.error_reporter.as_deref(), GuiFailure::Frame, &error);
            eprintln!("page clock advancement failed: {error}");
        }
        self.rebase_clock_if_document_changed();
    }

    fn should_present(&self, painted: bool, os_exposure: bool, chrome: &PresentedChrome) -> bool {
        painted || os_exposure || self.last_chrome.as_ref() != Some(chrome)
    }

    fn update_page_frame(
        &mut self,
        viewport: (u32, u32),
        elapsed_ms: u64,
    ) -> Result<bool, Box<dyn Error>> {
        let result: Result<bool, Box<dyn Error>> = match viewport {
            (0, _) | (_, 0) => {
                // A window no taller than the toolbar has no page area to paint.
                self.session
                    .drive_event_loop(elapsed_ms)
                    .map(|_| false)
                    .map_err(Into::into)
            }
            (width, height) => self
                .frame_cache
                .update(&mut self.session, width, height, elapsed_ms)
                .map_err(Into::into),
        };
        result.map_err(|error| {
            report_gui_failure(self.error_reporter.as_deref(), GuiFailure::Frame, &error);
            error
        })
    }

    pub(super) fn draw(
        &mut self,
        elapsed_ms: u64,
        os_exposure: bool,
    ) -> Result<(), Box<dyn Error>> {
        let Some(window) = self.window.clone() else {
            return Ok(());
        };
        let size = window.inner_size();
        if size.width == 0 || size.height == 0 {
            return Ok(());
        }
        let layout = self.chrome_layout();
        let painted = self.update_page_frame(layout.page_viewport(), elapsed_ms)?;
        if painted && let Some(trace) = &mut self.paint_trace {
            trace.record();
        }
        self.rebase_clock_if_document_changed();
        self.sync_find_document();
        self.sync_url_bar();
        let title = find_window_title(
            document_window_title(&mut self.session).map_err(|error| {
                report_gui_failure(self.error_reporter.as_deref(), GuiFailure::Frame, &error);
                error
            })?,
            self.find_ui.as_ref(),
        );
        if let Some(title) = changed_window_title(&mut self.window_title, title) {
            window.set_title(&title);
        }
        let chrome = PresentedChrome {
            layout,
            url_bar: self.url_bar.clone(),
        };
        if self.should_present(painted, os_exposure, &chrome) {
            self.present(&layout)?;
            self.last_chrome = Some(chrome);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn page_frame_errors_are_reported_with_and_without_page_area() {
        use omoikane::error_reporting::EventStore;
        for viewport in [(0, 100), (100, 0), (100, 100)] {
            let directory = std::env::temp_dir().join(format!(
                "omoikane-frame-errors-{}-{}-{}",
                std::process::id(),
                viewport.0,
                viewport.1,
            ));
            std::fs::create_dir_all(&directory).unwrap();
            let database = directory.join("events.sqlite");
            let config = ReporterConfig::from_values(Some("record-only"), None).unwrap();
            let reporter = Arc::new(
                ErrorReporter::new(&config, database.clone(), RetentionPolicy::default()).unwrap(),
            );
            let mut app = BrowserApp::new("data:text/html,<body>initial</body>").unwrap();
            app.error_reporter = Some(Arc::clone(&reporter));
            app.session
                .dispatch(
                    "Runtime.evaluate",
                    json!({"expression":
                        "setTimeout(()=>location.href='http://127.0.0.1:0/unreachable',1)"
                    }),
                )
                .unwrap();
            assert!(app.update_page_frame(viewport, 1).is_err());
            reporter.flush().unwrap();
            let store = EventStore::open(&database, RetentionPolicy::default()).unwrap();
            let event = GuiFailure::Frame.event();
            assert!(store.get(event.fingerprint()).unwrap().is_some());
            drop(store);
            drop(app);
            drop(reporter);
            std::fs::remove_dir_all(directory).unwrap();
        }
    }

    #[test]
    fn navigation_after_idle_rebases_new_document_timers() {
        let mut app = BrowserApp::new("data:text/html,<body>initial</body>").unwrap();
        let initial = Instant::now();
        app.begin_page_frame(initial);
        app.session.dispatch("Page.navigate", json!({"url":
            "data:text/html,<body><script>window.fired=false;setTimeout(()=>fired=true,500)</script>"})).unwrap();
        let resumed = initial + Duration::from_secs(10);
        assert_eq!(app.begin_page_frame(resumed), 0);
        app.session.drive_event_loop(0).unwrap();
        let delta = app.begin_page_frame(resumed + Duration::from_millis(499));
        app.session.drive_event_loop(delta).unwrap();
        let value = app
            .session
            .dispatch("Runtime.evaluate", json!({"expression":"fired"}))
            .unwrap();
        assert_eq!(value["result"]["value"], json!(false));
        let delta = app.begin_page_frame(resumed + Duration::from_millis(500));
        app.session.drive_event_loop(delta).unwrap();
        let value = app
            .session
            .dispatch("Runtime.evaluate", json!({"expression":"fired"}))
            .unwrap();
        assert_eq!(value["result"]["value"], json!(true));
    }
}
