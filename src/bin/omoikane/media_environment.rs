//! Capture native display and input settings before evaluating media queries.
use super::*;

impl BrowserApp {
    /// Starts the initial document after the presentation snapshot is installed.
    pub(super) fn start_initial_navigation(&mut self) -> Result<(), omoikane::cdp::JsonRpcError> {
        if let Some(url) = self.initial_url.take() {
            self.session
                .dispatch("Page.navigate", json!({ "url": url }))?;
            self.rebase_clock_if_document_changed();
        }
        Ok(())
    }

    pub(super) fn start_media_preference_monitor(&mut self) {
        let Some(window) = self.window.as_ref() else {
            return;
        };
        let window = Arc::downgrade(window);
        match omoikane::platform_media::MediaPreferenceMonitor::start(
            self.media_preferences.clone(),
            Duration::from_secs(2),
            move || {
                if let Some(window) = window.upgrade() {
                    window.request_redraw();
                }
            },
        ) {
            Ok(monitor) => self.media_preference_monitor = Some(monitor),
            Err(error) => eprintln!("media preference monitor failed: {error}"),
        }
    }

    fn receive_media_preferences(&mut self) -> bool {
        let update = self
            .media_preference_monitor
            .as_ref()
            .and_then(|monitor| monitor.take_latest());
        if let Some(update) = update {
            self.pointer_devices.set_native(update.pointers);
            self.media_preferences = update;
            true
        } else {
            false
        }
    }

    pub(super) fn sync_media_environment(&mut self) {
        self.receive_media_preferences();
        let Some(window) = &self.window else {
            return;
        };
        let mut environment = self.session.host_media_environment();
        environment.resolution_dppx = window.scale_factor() as f32;
        if let Some(monitor) = window.current_monitor() {
            let size = monitor.size().to_logical::<f64>(monitor.scale_factor());
            environment.device_width = Some(size.width as f32);
            environment.device_height = Some(size.height as f32);
        }
        environment.color_scheme_dark = window.theme() == Some(winit::window::Theme::Dark);
        environment.set_feature("prefers-reduced-motion", "no-preference");
        environment.set_feature("prefers-contrast", "no-preference");
        environment.set_feature("forced-colors", "none");
        environment.set_feature("prefers-reduced-transparency", "no-preference");
        environment.set_feature("inverted-colors", "none");
        self.media_preferences.apply_to(&mut environment);
        self.pointer_devices
            .snapshot(desktop_pointer_fallback())
            .apply_to(&mut environment);
        environment.set_feature(
            "display-mode",
            if self.native_fullscreen {
                "fullscreen"
            } else {
                "browser"
            },
        );
        if let Err(error) = self.session.set_host_media_environment(environment) {
            eprintln!("media environment update failed: {error}");
        }
    }

    pub(super) fn update_media_environment_for_event(&mut self, event: &WindowEvent) {
        if self.receive_media_preferences() {
            self.sync_media_environment();
        }
        match event {
            WindowEvent::Touch(touch) => {
                if self.pointer_devices.observe(
                    touch.device_id,
                    omoikane::platform_media::PointerCapabilities {
                        coarse: true,
                        ..Default::default()
                    },
                ) {
                    self.sync_media_environment();
                }
            }
            WindowEvent::CursorMoved { device_id, .. }
            | WindowEvent::MouseInput { device_id, .. }
            | WindowEvent::MouseWheel { device_id, .. } => {
                if self
                    .pointer_devices
                    .observe(*device_id, desktop_pointer_fallback())
                {
                    self.sync_media_environment();
                }
            }
            WindowEvent::ScaleFactorChanged { .. } | WindowEvent::ThemeChanged(_) => {
                self.sync_media_environment()
            }
            _ => {}
        }
    }
}

impl BrowserApp {
    pub(super) fn update_media_environment_for_device(
        &mut self,
        id: DeviceId,
        event: &DeviceEvent,
    ) {
        match event {
            DeviceEvent::Added | DeviceEvent::Removed => {
                if matches!(event, DeviceEvent::Removed) {
                    self.pointer_devices.remove(&id);
                }
                self.pointer_devices
                    .set_native(omoikane::platform_media::capture_pointer_capabilities());
                self.sync_media_environment();
            }
            _ => {}
        }
    }
}

fn desktop_pointer_fallback() -> omoikane::platform_media::PointerCapabilities {
    omoikane::platform_media::PointerCapabilities {
        fine: true,
        fine_hover: true,
        ..Default::default()
    }
}
