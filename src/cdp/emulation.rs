//! CDP presentation settings validated before updating the runtime.
use super::*;
use crate::css::MediaType;

#[derive(Debug, Clone, Default)]
pub(super) struct State {
    base: Option<crate::css::MediaEnvironment>,
    media: Option<MediaType>,
    features: Vec<(String, String)>,
}

impl State {
    fn environment(&self) -> Result<crate::css::MediaEnvironment, JsonRpcError> {
        let mut environment = self.base.clone().unwrap_or_default();
        if let Some(media) = self.media {
            environment.media_type = media;
        }
        for (name, value) in &self.features {
            if value.is_empty() {
                continue;
            }
            if name == "prefers-color-scheme" {
                environment.color_scheme_dark = match value.as_str() {
                    "dark" => true,
                    "light" => false,
                    _ => return Err(invalid_params("unsupported color scheme".into())),
                };
            } else if !environment.set_feature(name, value) {
                return Err(invalid_params(format!("unsupported media feature: {name}")));
            }
        }
        Ok(environment)
    }
}

impl CdpSession {
    /// Returns host media settings before any CDP emulation overrides.
    pub fn host_media_environment(&self) -> crate::css::MediaEnvironment {
        self.media_emulation
            .base
            .clone()
            .unwrap_or_else(|| self.runtime.media_environment())
    }

    /// Replaces presentation-host settings while preserving CDP overrides.
    pub fn set_host_media_environment(
        &mut self,
        environment: crate::css::MediaEnvironment,
    ) -> Result<(), JsonRpcError> {
        let mut next = self.media_emulation.clone();
        next.base = Some(environment);
        let environment = next.environment()?;
        self.runtime
            .set_media_environment(environment)
            .map_err(js_error)?;
        self.media_emulation = next;
        Ok(())
    }

    pub(super) fn emulation_set_media(&mut self, params: &Value) -> Result<Value, JsonRpcError> {
        let mut next = self.media_emulation.clone();
        next.base
            .get_or_insert_with(|| self.runtime.media_environment());
        if let Some(media) = params.get("media") {
            next.media = match media.as_str() {
                Some("") => None,
                Some("screen") => Some(MediaType::Screen),
                Some("print") => Some(MediaType::Print),
                _ => return Err(invalid_params("unsupported media type".into())),
            };
        }
        if let Some(features) = params.get("features") {
            let features = features
                .as_array()
                .ok_or_else(|| invalid_params("features must be an array".into()))?;
            next.features = features
                .iter()
                .map(|feature| {
                    Ok((
                        require_string(feature, "name")?,
                        require_string(feature, "value")?,
                    ))
                })
                .collect::<Result<_, JsonRpcError>>()?;
        }
        let environment = next.environment()?;
        self.runtime
            .set_media_environment(environment)
            .map_err(js_error)?;
        self.media_emulation = next;
        Ok(json!({}))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cdp_media_overrides_update_queries_and_validate_atomically() {
        let mut session = CdpSession::new().unwrap();
        session
            .dispatch(
                "Emulation.setEmulatedMedia",
                json!({
                    "media":"print", "features":[{"name":"prefers-reduced-motion","value":"reduce"}]
                }),
            )
            .unwrap();
        assert_eq!(session.runtime.eval("matchMedia('print').matches && matchMedia('(prefers-reduced-motion: reduce)').matches").unwrap().as_boolean(), Some(true));
        let previous = session.runtime.media_environment();
        let error = session
            .dispatch(
                "Emulation.setEmulatedMedia",
                json!({
                    "media":"screen", "features":[{"name":"pointer","value":"invalid"}]
                }),
            )
            .unwrap_err();
        assert_eq!(error.code, -32602);
        assert_eq!(session.runtime.media_environment(), previous);
        session
            .dispatch("Emulation.setEmulatedMedia", json!({"media":""}))
            .unwrap();
        assert_eq!(
            session
                .runtime
                .eval("matchMedia('screen').matches")
                .unwrap()
                .as_boolean(),
            Some(true)
        );
    }
    #[test]
    fn cdp_media_update_during_page_startup_survives_commit() {
        use std::future::Future;
        use std::task::{Context, Poll, Waker};

        let mut session = CdpSession::new().unwrap();
        let (task, pending) = session.prepare_document_page_task(
            "http://example.test/", "<script>globalThis.mediaChanges = 0; matchMedia('(prefers-reduced-motion: reduce)').addEventListener('change', () => mediaChanges++);</script>",
            1, "null", &[], None,
        ).unwrap();
        let mut task = Box::pin(task);
        let mut context = Context::from_waker(Waker::noop());
        let started = std::time::Instant::now();
        let completed = loop {
            assert!(started.elapsed() < std::time::Duration::from_secs(10));
            if let Poll::Ready(completed) = task.as_mut().poll(&mut context) {
                break completed;
            }
        };
        session
            .dispatch(
                "Emulation.setEmulatedMedia",
                json!({"features":[{"name":"prefers-reduced-motion","value":"reduce"}]}),
            )
            .unwrap();
        session
            .commit_document_page_task(completed, pending)
            .unwrap();
        assert_eq!(
            session.runtime.eval("mediaChanges").unwrap().as_number(),
            Some(0.0)
        );
        session.runtime.run_animation_frame(16).unwrap();
        assert_eq!(
            session
                .runtime
                .eval(
                    "matchMedia('(prefers-reduced-motion: reduce)').matches && mediaChanges === 1"
                )
                .unwrap()
                .as_boolean(),
            Some(true)
        );
    }

    #[test]
    fn cdp_viewport_update_between_startup_and_commit_uses_latest_dimensions() {
        use std::future::Future;
        use std::task::{Context, Poll, Waker};

        let mut session = CdpSession::new().unwrap();
        session.set_viewport(640, 480);
        let (task, pending) = session.prepare_document_page_task(
            "http://example.test/",
            "<style>#probe {color:red} @media (min-width:1000px) {#probe {color:green}}</style><div id=probe>viewport</div><script>globalThis.initialWidth = innerWidth; globalThis.viewportChanges = 0; globalThis.viewportQuery = matchMedia('(width:640px)'); viewportQuery.onchange = () => viewportChanges++;</script>",
            1, "null", &[], None,
        ).unwrap();
        let mut task = Box::pin(task);
        let mut context = Context::from_waker(Waker::noop());
        let started = std::time::Instant::now();
        let completed = loop {
            assert!(started.elapsed() < std::time::Duration::from_secs(10));
            if let Poll::Ready(completed) = task.as_mut().poll(&mut context) {
                break completed;
            }
        };
        session.set_viewport(1024, 768);
        session
            .commit_document_page_task(completed, pending)
            .unwrap();
        assert_eq!(session.runtime.eval("initialWidth === 640 && innerWidth === 1024 && innerHeight === 768 && screen.width === 1024 && !viewportQuery.matches && matchMedia('(width:1024px)').matches && viewportChanges === 0 && getComputedStyle(document.getElementById('probe')).color === 'rgb(0, 128, 0)'").unwrap().as_boolean(), Some(true));
        session.runtime.run_animation_frame(16).unwrap();
        assert_eq!(
            session.runtime.eval("viewportChanges").unwrap().as_number(),
            Some(1.0)
        );
    }

    #[test]
    fn cdp_media_overrides_reset_and_survive_navigation_before_scripts() {
        let mut session = CdpSession::new().unwrap();
        session.dispatch("Emulation.setEmulatedMedia", json!({"media":"print","features":[{"name":"prefers-reduced-motion","value":"reduce"}]})).unwrap();
        session.install_document("http://example.test/", "<script>globalThis.initialMedia = matchMedia('print').matches && matchMedia('(prefers-reduced-motion: reduce)').matches;</script>", 1, "null").unwrap();
        assert_eq!(
            session.runtime.eval("initialMedia").unwrap().as_boolean(),
            Some(true)
        );
        session
            .dispatch(
                "Emulation.setEmulatedMedia",
                json!({"media":"","features":[]}),
            )
            .unwrap();
        assert_eq!(session.runtime.eval("matchMedia('screen').matches && matchMedia('(prefers-reduced-motion: no-preference)').matches").unwrap().as_boolean(), Some(true));
    }
}
