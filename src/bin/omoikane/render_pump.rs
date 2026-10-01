//! Native page-clock synchronization and demand-driven frame composition.

use super::*;

impl BrowserApp {
    /// New documents start at their own time origin. Never charge their timers
    /// for idle time or blocking navigation belonging to the previous document.
    pub(super) fn consume_page_delta(&mut self, now: Instant, rendering: bool) -> u64 {
        let delta = if rendering {
            self.frame_scheduler.begin_frame(now)
        } else {
            self.frame_scheduler.advance_time(now)
        };
        let document = self.session.document_generation();
        if self.clock_document == document {
            delta
        } else {
            self.clock_document = document;
            0
        }
    }

    pub(super) fn sync_page_time_before_input(&mut self, now: Instant) {
        let elapsed = self.consume_page_delta(now, false);
        if let Err(error) = self.session.advance_tasks(elapsed) {
            report_gui_failure(self.error_reporter.as_deref(), GuiFailure::Frame, &error);
            eprintln!("page clock advancement failed: {error}");
        }
        if self.clock_document != self.session.document_generation() {
            self.consume_page_delta(Instant::now(), false);
        }
    }

    pub(super) fn draw(
        &mut self,
        elapsed_ms: u64,
        force_present: bool,
    ) -> Result<(), Box<dyn Error>> {
        let Some(window) = self.window.clone() else {
            return Ok(());
        };
        let size = window.inner_size();
        if size.width == 0 || size.height == 0 {
            return Ok(());
        }
        let layout = self.chrome_layout();
        let painted = match layout.page_viewport() {
            (0, _) | (_, 0) => {
                self.session.drive_event_loop(elapsed_ms)?;
                false
            }
            (width, height) => self
                .frame_cache
                .update(&mut self.session, width, height, elapsed_ms)
                .map_err(|error| {
                    report_gui_failure(self.error_reporter.as_deref(), GuiFailure::Frame, &error);
                    error
                })?,
        };
        if painted {
            self.paint_sequence += 1;
            if self.trace_paint {
                let record = json!({"sequence":self.paint_sequence,
                    "elapsed_ms":self.started_at.elapsed().as_millis()});
                eprintln!("OMOIKANE_PAINT {record}");
            }
        }
        if self.clock_document != self.session.document_generation() {
            self.consume_page_delta(Instant::now(), false);
        }
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
        let chrome = (layout, self.url_bar.clone());
        if painted || force_present || self.last_chrome.as_ref() != Some(&chrome) {
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
    fn navigation_after_idle_rebases_new_document_timers() {
        let mut app = BrowserApp::new("data:text/html,<body>initial</body>").unwrap();
        let initial = app.started_at;
        app.consume_page_delta(initial, true);
        app.session.dispatch("Page.navigate", json!({"url":
            "data:text/html,<body><script>window.fired=false;setTimeout(()=>fired=true,500)</script>"})).unwrap();
        let resumed = initial + Duration::from_secs(10);
        assert_eq!(app.consume_page_delta(resumed, true), 0);
        app.session.drive_event_loop(0).unwrap();
        let delta = app.consume_page_delta(resumed + Duration::from_millis(499), true);
        app.session.drive_event_loop(delta).unwrap();
        let value = app
            .session
            .dispatch("Runtime.evaluate", json!({"expression":"fired"}))
            .unwrap();
        assert_eq!(value["result"]["value"], json!(false));
        let delta = app.consume_page_delta(resumed + Duration::from_millis(500), true);
        app.session.drive_event_loop(delta).unwrap();
        let value = app
            .session
            .dispatch("Runtime.evaluate", json!({"expression":"fired"}))
            .unwrap();
        assert_eq!(value["result"]["value"], json!(true));
    }
}
