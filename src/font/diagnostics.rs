//! Opt-in, thread-scoped records of the faces actually used by layout and paint.

use super::{Font, FontFamilyKey, FontStyle, FontVariantKey, SystemFontFace};
use std::cell::RefCell;

/// A primary-face decision made while measuring or painting text.
#[derive(Clone, Debug, serde::Serialize)]
pub struct FontSelectionRecord {
    /// `layout` or `paint`.
    pub phase: String,
    /// Ordered CSS family names, after parsing and case normalization.
    pub requested_families: Vec<String>,
    /// Requested numeric CSS weight.
    pub requested_weight: u16,
    /// Requested CSS style.
    pub requested_style: FontStyle,
    /// `system`, `web`, `provided`, or `none` (placeholder rendering).
    pub source: String,
    /// Actual installed file, collection index and metadata, for a system face.
    pub selected: Option<SystemFontFace>,
    /// Consecutive uses of this same decision.
    pub uses: usize,
}

thread_local! {
    static RECORDS: RefCell<Option<Vec<FontSelectionRecord>>> = const { RefCell::new(None) };
}

/// Captures actual primary-face decisions on the calling thread while running `f`.
///
/// Nested calls and panics restore the previous diagnostic scope. This does not
/// change font selection or expose installed paths to page JavaScript. Other
/// threads are unaffected. No records are allocated outside an active scope.
pub fn with_font_selection_diagnostics<T>(f: impl FnOnce() -> T) -> (T, Vec<FontSelectionRecord>) {
    struct Restore(Option<Vec<FontSelectionRecord>>);
    impl Drop for Restore {
        fn drop(&mut self) {
            RECORDS.with(|slot| *slot.borrow_mut() = self.0.take());
        }
    }
    let _restore = Restore(RECORDS.with(|slot| slot.replace(Some(Vec::new()))));
    let result = f();
    let records = RECORDS.with(|slot| slot.take().expect("active diagnostic scope"));
    (result, records)
}

pub(super) fn record(
    phase: &str,
    family: Option<FontFamilyKey>,
    variant: FontVariantKey,
    font: Option<&Font>,
    web: bool,
) {
    RECORDS.with(|slot| {
        let mut slot = slot.borrow_mut();
        let Some(records) = slot.as_mut() else { return };
        let selected = font.and_then(Font::system_face).cloned();
        let source = if selected.is_some() {
            "system"
        } else if web {
            "web"
        } else if font.is_some() {
            "provided"
        } else {
            "none"
        };
        let families = family
            .map(|key| key.families().to_vec())
            .unwrap_or_default();
        if let Some(last) = records.last_mut()
            && last.phase == phase
            && last.requested_families == families
            && last.requested_weight == variant.weight.0
            && last.requested_style == variant.style
            && last.source == source
            && last.selected == selected
        {
            last.uses += 1;
        } else {
            records.push(FontSelectionRecord {
                phase: phase.to_string(),
                requested_families: families,
                requested_weight: variant.weight.0,
                requested_style: variant.style,
                source: source.to_string(),
                selected,
                uses: 1,
            });
        }
    });
}
