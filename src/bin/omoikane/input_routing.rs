//! Routes native keyboard, IME and pointer input between the page and the
//! browser chrome (find-in-page UI and address bar).
//!
//! Keys and IME go to exactly one [`InputTarget`]. Mouse input goes to the page
//! only over the page area, except that a button pressed in the page keeps the
//! page as its target until released, and Pointer Lock sends mouse input to
//! the page. Each touch keeps the target selected by its starting region.

use omoikane::platform_input::{PlatformMouseButton, PlatformTouchPhase};
use serde_json::json;
use winit::event::{Touch, TouchPhase};

use super::BrowserApp;
use super::chrome_layout::{ChromeLayout, WindowRegion};
use super::toolbar_paint::url_field;

use super::url_bar::{UrlBarKey, UrlBarOutcome, UrlRejection, navigation_target};

/// The single receiver of keyboard and IME input.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum InputTarget {
    Page,
    Find,
    UrlBar,
}

/// What a cursor movement means for the page.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum PagePointer {
    /// Move the page pointer to these CSS pixels.
    Move(f64, f64),
    /// The cursor just left the page for the toolbar.
    Leave,
    /// The page is not affected.
    Ignore,
}

impl BrowserApp {
    /// Forwards a native contact only when its gesture belongs to the page.
    pub(super) fn dispatch_touch(
        &mut self,
        touch: Touch,
    ) -> Result<(), omoikane::cdp::JsonRpcError> {
        let phase = match touch.phase {
            TouchPhase::Started => PlatformTouchPhase::Started,
            TouchPhase::Moved => PlatformTouchPhase::Moved,
            TouchPhase::Ended => PlatformTouchPhase::Ended,
            TouchPhase::Cancelled => PlatformTouchPhase::Cancelled,
        };
        let Some((x, y)) = self.route_touch(
            self.chrome_layout(),
            touch.id,
            phase,
            touch.location.x,
            touch.location.y,
        ) else {
            return Ok(());
        };
        self.input.touch(&mut self.session, touch.id, phase, x, y)
    }

    /// Routes each contact according to where it started. A tap on the URL
    /// field focuses it; a page contact stays with the page through its end,
    /// including movements over the toolbar. Unmatched contacts are ignored.
    pub(super) fn route_touch(
        &mut self,
        layout: ChromeLayout,
        id: u64,
        phase: PlatformTouchPhase,
        x: f64,
        y: f64,
    ) -> Option<(f64, f64)> {
        let to_page = match phase {
            PlatformTouchPhase::Started => {
                self.page_touches.remove(&id);
                match layout.region_at(x, y) {
                    WindowRegion::Page { .. } => {
                        self.url_bar.cancel();
                        self.page_touches.insert(id);
                        true
                    }
                    WindowRegion::Toolbar { .. } => {
                        if url_field(layout.toolbar(), layout.scale()).contains(x, y) {
                            self.focus_url_bar();
                        }
                        false
                    }
                    WindowRegion::Outside => false,
                }
            }
            PlatformTouchPhase::Moved => self.page_touches.contains(&id),
            PlatformTouchPhase::Ended | PlatformTouchPhase::Cancelled => {
                self.page_touches.remove(&id)
            }
        };
        to_page.then(|| layout.page_point(x, y))
    }

    /// Returns the current key/IME target. Editing the address bar and the
    /// find UI are kept mutually exclusive, so the address bar wins only
    /// while it is the one being edited.
    pub(super) fn input_target(&self) -> InputTarget {
        if self.url_bar.is_editing() {
            InputTarget::UrlBar
        } else if self.find_ui.is_some() {
            InputTarget::Find
        } else {
            InputTarget::Page
        }
    }

    /// Starts editing the address bar and closes the find UI.
    pub(super) fn focus_url_bar(&mut self) {
        if self.find_ui.is_some() {
            if let Err(error) = self.find_action("stop") {
                eprintln!("find-in-page failed: {error}");
            }
            self.find_ui = None;
        }
        self.url_bar.focus();
    }

    /// Handles a real key for the browser chrome. Returns `true` when the key
    /// was consumed and must not reach the page.
    pub(super) fn handle_chrome_key(
        &mut self,
        key: &str,
        text: Option<&str>,
        pressed: bool,
    ) -> bool {
        if !pressed && self.chrome_key_releases.remove(key) {
            return true;
        }
        let shortcut = (self.modifiers.control || self.modifiers.meta) && !self.modifiers.alt;
        if shortcut && key.eq_ignore_ascii_case("l") {
            if pressed {
                self.chrome_key_releases.insert(key.to_string());
                self.focus_url_bar();
            }
            return true;
        }
        if shortcut && pressed && key.eq_ignore_ascii_case("f") {
            // Opening find moves key input away from the address bar.
            self.url_bar.cancel();
        }
        match self.input_target() {
            InputTarget::UrlBar => self.handle_url_bar_key(key, text, pressed),
            InputTarget::Find | InputTarget::Page => self.handle_find_key(key, text, pressed),
        }
    }

    /// Edits the address bar with a pressed key. Releases not pressed here
    /// fall through so the page still sees the release of its own presses.
    fn handle_url_bar_key(&mut self, key: &str, text: Option<&str>, pressed: bool) -> bool {
        if !pressed {
            return false;
        }
        self.chrome_key_releases.insert(key.to_string());
        let modifiers = self.modifiers;
        let outcome = if modifiers.control || modifiers.meta {
            if key.eq_ignore_ascii_case("a") {
                self.url_bar.select_all()
            } else {
                UrlBarOutcome::Ignored
            }
        } else if let Some(edit) = url_bar_key(key) {
            self.url_bar.key(edit, modifiers.shift)
        } else if let Some(text) = text {
            self.url_bar.insert_text(text)
        } else {
            UrlBarOutcome::Ignored
        };
        if let UrlBarOutcome::Navigate(url) = outcome {
            self.navigate_from_url_bar(&url);
        }
        true
    }

    /// Loads a URL committed in the address bar through the page's normal
    /// navigation path. A rejected or failed URL goes back into the bar for
    /// correction, and the page and its shown URL stay unchanged.
    ///
    /// `Page.navigate` completes the request synchronously, so the event loop
    /// stops until the page has loaded. Making it asynchronous is out of
    /// scope for #1139.
    fn navigate_from_url_bar(&mut self, text: &str) {
        let result = match navigation_target(text) {
            Ok(url) => self
                .session
                .dispatch("Page.navigate", json!({ "url": url }))
                .map(drop)
                .map_err(|error| error.message),
            Err(UrlRejection::MissingScheme) => Err("URL must start with a scheme".into()),
            Err(UrlRejection::JavaScript) => Err("javascript: URLs are not run".into()),
        };
        match result {
            Ok(()) => self.url_bar.set_page_url(self.session.current_url()),
            Err(message) => {
                eprintln!("navigation to {text:?} failed: {message}");
                self.url_bar.edit(text);
            }
        }
    }

    /// Returns whether pointer input at the last cursor position belongs to
    /// the page.
    pub(super) fn pointer_targets_page(&self, layout: ChromeLayout) -> bool {
        let (x, y) = self.cursor_position;
        self.native_pointer_lock
            || !self.page_buttons.is_empty()
            || matches!(layout.region_at(x, y), WindowRegion::Page { .. })
    }

    /// Records a cursor movement and decides what the page receives.
    pub(super) fn route_cursor(&mut self, layout: ChromeLayout, x: f64, y: f64) -> PagePointer {
        self.cursor_position = (x, y);
        let in_page = self.pointer_targets_page(layout);
        let was_in_page = std::mem::replace(&mut self.cursor_in_page, in_page);
        if in_page {
            let (x, y) = layout.page_point(x, y);
            PagePointer::Move(x, y)
        } else if was_in_page {
            PagePointer::Leave
        } else {
            PagePointer::Ignore
        }
    }

    /// Routes a mouse button at the last cursor position. Returns `true` when
    /// the page must receive it. A left click on the URL field starts editing;
    /// a press in the page ends address bar editing.
    pub(super) fn route_mouse_button(
        &mut self,
        layout: ChromeLayout,
        button: PlatformMouseButton,
        pressed: bool,
    ) -> bool {
        if !pressed {
            let held = self.page_buttons.iter().position(|held| *held == button);
            if let Some(index) = held {
                self.page_buttons.swap_remove(index);
            }
            return held.is_some() || self.native_pointer_lock;
        }
        let (x, y) = self.cursor_position;
        let to_page = self.native_pointer_lock
            || match layout.region_at(x, y) {
                WindowRegion::Page { .. } => true,
                WindowRegion::Toolbar { .. } => {
                    if button == PlatformMouseButton::Left
                        && url_field(layout.toolbar(), layout.scale()).contains(x, y)
                    {
                        self.focus_url_bar();
                    }
                    false
                }
                WindowRegion::Outside => false,
            };
        if to_page {
            self.url_bar.cancel();
            self.page_buttons.push(button);
        }
        to_page
    }
}

/// Maps a DOM key name to an address bar editing key.
fn url_bar_key(key: &str) -> Option<UrlBarKey> {
    Some(match key {
        "Backspace" => UrlBarKey::Backspace,
        "Delete" => UrlBarKey::Delete,
        "ArrowLeft" => UrlBarKey::Left,
        "ArrowRight" => UrlBarKey::Right,
        "Home" => UrlBarKey::Home,
        "End" => UrlBarKey::End,
        "Enter" => UrlBarKey::Enter,
        "Escape" => UrlBarKey::Escape,
        _ => return None,
    })
}

#[cfg(test)]
#[path = "input_routing_tests.rs"]
mod tests;
