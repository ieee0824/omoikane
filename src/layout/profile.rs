//! Opt-in, pass-local layout phase diagnostics.

use std::cell::RefCell;
use std::sync::OnceLock;
use std::time::Instant;

#[derive(Clone, Copy)]
pub(super) enum Phase {
    Tree,
    Element,
    FlexCollect,
    FlexLayout,
    IntrinsicRaw,
    InlineCollect,
    TextMeasure,
    FontSelect,
}

#[derive(Default)]
struct Counter {
    calls: u64,
    ms: f64,
}

#[derive(Default)]
struct Profile {
    tree: Counter,
    element: Counter,
    flex_collect: Counter,
    flex_layout: Counter,
    intrinsic_raw: Counter,
    intrinsic_hits: u64,
    inline_collect: Counter,
    text_measure: Counter,
    font_select: Counter,
}

thread_local! {
    static PASSES: RefCell<Vec<Profile>> = const { RefCell::new(Vec::new()) };
    #[cfg(test)]
    static TEST_TLS_TOUCHES: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

fn with_passes<T>(operation: impl FnOnce(&RefCell<Vec<Profile>>) -> T) -> T {
    #[cfg(test)]
    TEST_TLS_TOUCHES.with(|touches| touches.set(touches.get().saturating_add(1)));
    PASSES.with(operation)
}

fn enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| std::env::var_os("OMOIKANE_TRACE_LAYOUT_PHASES").is_some())
}

pub(super) struct Pass(bool);

impl Pass {
    pub(super) fn begin() -> Self {
        let active = enabled();
        if active {
            with_passes(|passes| passes.borrow_mut().push(Profile::default()));
        }
        Self(active)
    }
}

impl Drop for Pass {
    fn drop(&mut self) {
        if !self.0 {
            return;
        }
        let profile = with_passes(|passes| passes.borrow_mut().pop());
        let Some(p) = profile else { return };
        eprintln!(
            "{}",
            crate::diagnostics::json_record(
                "OMOIKANE_LAYOUT_PHASES",
                &serde_json::json!({
                    "inclusive_nested": true,
                    "tree": {"calls": p.tree.calls, "ms": p.tree.ms},
                    "layout_element": {"calls": p.element.calls, "ms": p.element.ms},
                    "flex_collect": {"calls": p.flex_collect.calls, "ms": p.flex_collect.ms},
                    "flex_layout": {"calls": p.flex_layout.calls, "ms": p.flex_layout.ms},
                    "intrinsic_raw": {"calls": p.intrinsic_raw.calls, "ms": p.intrinsic_raw.ms},
                    "intrinsic_hits": {"calls": p.intrinsic_hits},
                    "inline_collect": {"calls": p.inline_collect.calls, "ms": p.inline_collect.ms},
                    "text_measure": {"calls": p.text_measure.calls, "ms": p.text_measure.ms},
                    "font_select": {"calls": p.font_select.calls, "ms": p.font_select.ms},
                }),
            )
        );
    }
}

pub(super) struct Scope {
    phase: Phase,
    started: Option<Instant>,
}

impl Scope {
    pub(super) fn begin(phase: Phase) -> Self {
        Self::begin_if(enabled(), phase)
    }

    fn begin_if(enabled: bool, phase: Phase) -> Self {
        let active = enabled && with_passes(|passes| !passes.borrow().is_empty());
        Self {
            phase,
            started: active.then(Instant::now),
        }
    }
}

impl Drop for Scope {
    fn drop(&mut self) {
        let Some(started) = self.started else { return };
        let ms = started.elapsed().as_secs_f64() * 1000.0;
        with_passes(|passes| {
            let mut passes = passes.borrow_mut();
            let Some(p) = passes.last_mut() else {
                return;
            };
            let counter = match self.phase {
                Phase::Tree => &mut p.tree,
                Phase::Element => &mut p.element,
                Phase::FlexCollect => &mut p.flex_collect,
                Phase::FlexLayout => &mut p.flex_layout,
                Phase::IntrinsicRaw => &mut p.intrinsic_raw,
                Phase::InlineCollect => &mut p.inline_collect,
                Phase::TextMeasure => &mut p.text_measure,
                Phase::FontSelect => &mut p.font_select,
            };
            counter.calls = counter.calls.saturating_add(1);
            counter.ms += ms;
        });
    }
}

pub(super) fn record_intrinsic_hit() {
    record_intrinsic_hit_if(enabled());
}

fn record_intrinsic_hit_if(enabled: bool) {
    if !enabled {
        return;
    }
    with_passes(|passes| {
        if let Some(profile) = passes.borrow_mut().last_mut() {
            profile.intrinsic_hits = profile.intrinsic_hits.saturating_add(1);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disabled_scope_and_hit_do_not_touch_tls() {
        TEST_TLS_TOUCHES.with(|touches| touches.set(0));
        let scope = Scope::begin_if(false, Phase::Tree);
        record_intrinsic_hit_if(false);
        assert!(scope.started.is_none());
        TEST_TLS_TOUCHES.with(|touches| assert_eq!(touches.get(), 0));
    }

    #[test]
    fn nested_scopes_are_owned_by_the_innermost_pass() {
        with_passes(|passes| passes.borrow_mut().push(Profile::default()));
        {
            let _scope = Scope::begin_if(true, Phase::Element);
        }
        record_intrinsic_hit_if(true);
        with_passes(|passes| {
            let profile = passes.borrow_mut().pop().unwrap();
            assert_eq!(profile.element.calls, 1);
            assert!(profile.element.ms >= 0.0);
            assert_eq!(profile.intrinsic_hits, 1);
        });
    }
}
