//! Owned editing state for the GUI address bar.
//!
//! The state separates the URL of the page currently shown from the text the
//! user is editing. Editing never performs I/O: committing returns the typed
//! URL once as [`UrlBarOutcome::Navigate`], and the host decides how to load it.
//! Caret and selection positions are byte offsets on `char` boundaries.

use std::ops::Range;

/// Editing keys understood by the address bar, independent of the windowing
/// library.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum UrlBarKey {
    Backspace,
    Delete,
    Left,
    Right,
    Home,
    End,
    Enter,
    Escape,
}

/// Result of feeding one operation to the address bar.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum UrlBarOutcome {
    /// The bar is not editing, so the operation was not consumed.
    Ignored,
    /// The operation was consumed; the bar may need repainting.
    Edited,
    /// Editing ended with this URL text; the host should navigate to it.
    Navigate(String),
    /// Editing was abandoned and the page URL is shown again.
    Cancelled,
}

/// Address bar state: the shown page's URL plus an optional edit in progress.
#[derive(Clone, Debug)]
pub(super) struct UrlBar {
    page_url: String,
    draft: Option<Draft>,
}

#[derive(Clone, Debug)]
struct Draft {
    text: String,
    caret: usize,
    anchor: usize,
}

impl UrlBar {
    /// Creates a bar that shows `page_url` and is not editing.
    pub(super) fn new(page_url: impl Into<String>) -> Self {
        Self {
            page_url: page_url.into(),
            draft: None,
        }
    }

    /// Returns the URL of the page currently shown.
    pub(super) fn page_url(&self) -> &str {
        &self.page_url
    }

    /// Records the URL of the page now shown. An edit in progress is kept, so
    /// page-initiated navigation does not overwrite what the user is typing.
    pub(super) fn set_page_url(&mut self, url: impl Into<String>) {
        self.page_url = url.into();
    }

    /// Returns whether the user is editing the bar.
    pub(super) fn is_editing(&self) -> bool {
        self.draft.is_some()
    }

    /// Returns the text to draw: the edit in progress, or the page URL.
    pub(super) fn display_text(&self) -> &str {
        self.draft
            .as_ref()
            .map_or(self.page_url.as_str(), |draft| draft.text.as_str())
    }

    /// Returns the caret offset while editing.
    pub(super) fn caret(&self) -> Option<usize> {
        self.draft.as_ref().map(|draft| draft.caret)
    }

    /// Returns the selected byte range while editing, if it is non-empty.
    pub(super) fn selection(&self) -> Option<Range<usize>> {
        self.draft
            .as_ref()
            .filter(|draft| draft.caret != draft.anchor)
            .map(Draft::selected)
    }

    /// Starts editing from the page URL with all text selected, so typing
    /// replaces it. Focusing a bar that is already editing changes nothing.
    pub(super) fn focus(&mut self) -> UrlBarOutcome {
        if self.draft.is_none() {
            self.draft = Some(Draft {
                caret: self.page_url.len(),
                anchor: 0,
                text: self.page_url.clone(),
            });
        }
        UrlBarOutcome::Edited
    }

    /// Starts editing with `text` and the caret at its end, so a rejected or
    /// failed URL stays visible for correction.
    pub(super) fn edit(&mut self, text: impl Into<String>) {
        let text = text.into();
        let end = text.len();
        self.draft = Some(Draft {
            text,
            caret: end,
            anchor: end,
        });
    }

    /// Abandons the edit and shows the page URL again.
    pub(super) fn cancel(&mut self) -> UrlBarOutcome {
        match self.draft.take() {
            Some(_) => UrlBarOutcome::Cancelled,
            None => UrlBarOutcome::Ignored,
        }
    }

    /// Selects the whole edit text.
    pub(super) fn select_all(&mut self) -> UrlBarOutcome {
        let Some(draft) = &mut self.draft else {
            return UrlBarOutcome::Ignored;
        };
        draft.anchor = 0;
        draft.caret = draft.text.len();
        UrlBarOutcome::Edited
    }

    /// Replaces the selection with `text`, dropping control characters.
    pub(super) fn insert_text(&mut self, text: &str) -> UrlBarOutcome {
        let Some(draft) = &mut self.draft else {
            return UrlBarOutcome::Ignored;
        };
        let text: String = text.chars().filter(|ch| !ch.is_control()).collect();
        draft.replace_selection(&text);
        UrlBarOutcome::Edited
    }

    /// Applies an editing key. `extend` keeps the selection anchor while the
    /// caret moves, as Shift does in text fields.
    pub(super) fn key(&mut self, key: UrlBarKey, extend: bool) -> UrlBarOutcome {
        let Some(draft) = &mut self.draft else {
            return UrlBarOutcome::Ignored;
        };
        match key {
            UrlBarKey::Enter => return self.commit(),
            UrlBarKey::Escape => return self.cancel(),
            UrlBarKey::Backspace | UrlBarKey::Delete if draft.caret != draft.anchor => {
                draft.replace_selection("");
            }
            UrlBarKey::Backspace => {
                let start = previous_boundary(&draft.text, draft.caret);
                draft.text.replace_range(start..draft.caret, "");
                draft.collapse(start);
            }
            UrlBarKey::Delete => {
                let end = next_boundary(&draft.text, draft.caret);
                draft.text.replace_range(draft.caret..end, "");
            }
            UrlBarKey::Left | UrlBarKey::Right if !extend && draft.caret != draft.anchor => {
                let selected = draft.selected();
                let target = if key == UrlBarKey::Left {
                    selected.start
                } else {
                    selected.end
                };
                draft.collapse(target);
            }
            UrlBarKey::Left => {
                draft.move_caret(previous_boundary(&draft.text, draft.caret), extend)
            }
            UrlBarKey::Right => draft.move_caret(next_boundary(&draft.text, draft.caret), extend),
            UrlBarKey::Home => draft.move_caret(0, extend),
            UrlBarKey::End => draft.move_caret(draft.text.len(), extend),
        }
        UrlBarOutcome::Edited
    }

    /// Ends editing and returns the trimmed text for navigation. Empty text
    /// keeps editing, because there is nothing to load.
    fn commit(&mut self) -> UrlBarOutcome {
        let Some(draft) = &self.draft else {
            return UrlBarOutcome::Ignored;
        };
        let url = draft.text.trim();
        if url.is_empty() {
            return UrlBarOutcome::Edited;
        }
        let url = url.to_string();
        self.draft = None;
        UrlBarOutcome::Navigate(url)
    }
}

impl Draft {
    fn selected(&self) -> Range<usize> {
        self.caret.min(self.anchor)..self.caret.max(self.anchor)
    }

    fn collapse(&mut self, offset: usize) {
        self.caret = offset;
        self.anchor = offset;
    }

    fn move_caret(&mut self, offset: usize, extend: bool) {
        self.caret = offset;
        if !extend {
            self.anchor = offset;
        }
    }

    fn replace_selection(&mut self, text: &str) {
        let selected = self.selected();
        self.text.replace_range(selected.clone(), text);
        self.collapse(selected.start + text.len());
    }
}

/// Why typed text was not sent to navigation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum UrlRejection {
    /// The text has no `scheme:` prefix; completing one is out of scope.
    MissingScheme,
    /// `javascript:` URLs are never run from the address bar.
    JavaScript,
}

/// Accepts committed text as a navigation target only when it is an absolute
/// URL with a scheme other than `javascript:`. The URL itself is validated by
/// the navigation path.
pub(super) fn navigation_target(text: &str) -> Result<&str, UrlRejection> {
    let scheme = text
        .split_once(':')
        .map(|(scheme, _)| scheme)
        .filter(|scheme| {
            let mut chars = scheme.chars();
            chars.next().is_some_and(|ch| ch.is_ascii_alphabetic())
                && chars.all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '+' | '-' | '.'))
        })
        .ok_or(UrlRejection::MissingScheme)?;
    if scheme.eq_ignore_ascii_case("javascript") {
        return Err(UrlRejection::JavaScript);
    }
    Ok(text)
}

fn previous_boundary(text: &str, offset: usize) -> usize {
    text[..offset]
        .chars()
        .next_back()
        .map_or(offset, |ch| offset - ch.len_utf8())
}

fn next_boundary(text: &str, offset: usize) -> usize {
    text[offset..]
        .chars()
        .next()
        .map_or(offset, |ch| offset + ch.len_utf8())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn editing(text: &str) -> UrlBar {
        let mut bar = UrlBar::new(text);
        bar.focus();
        bar.key(UrlBarKey::End, false);
        bar
    }

    #[test]
    fn navigation_accepts_only_absolute_urls_without_javascript() {
        for url in [
            "http://127.0.0.1:8000/a",
            "https://example.test/",
            "data:text/html,x",
            "about:blank",
        ] {
            assert_eq!(navigation_target(url), Ok(url));
        }
        for text in [
            "example.test",
            "127.0.0.1:8000/a",
            "/path",
            "1http://x",
            ":x",
            "a b:c",
        ] {
            assert_eq!(
                navigation_target(text),
                Err(UrlRejection::MissingScheme),
                "{text}"
            );
        }
        for text in ["javascript:alert(1)", "JavaScript:void 0"] {
            assert_eq!(navigation_target(text), Err(UrlRejection::JavaScript));
        }
    }

    #[test]
    fn edit_shows_given_text_with_caret_at_end() {
        let mut bar = UrlBar::new("https://page.test/");
        bar.edit("bad url");
        assert_eq!(bar.display_text(), "bad url");
        assert_eq!(bar.caret(), Some(7));
        assert_eq!(bar.selection(), None);
    }

    #[test]
    fn typing_after_focus_replaces_the_page_url_and_enter_navigates_once() {
        let mut bar = UrlBar::new("http://127.0.0.1:8000/before");
        assert!(!bar.is_editing());
        assert_eq!(bar.focus(), UrlBarOutcome::Edited);
        assert_eq!(bar.selection(), Some(0..28));

        assert_eq!(
            bar.insert_text("http://127.0.0.1:8000/after"),
            UrlBarOutcome::Edited
        );
        assert_eq!(bar.display_text(), "http://127.0.0.1:8000/after");
        // The shown page does not change until the host reports navigation.
        assert_eq!(bar.page_url(), "http://127.0.0.1:8000/before");

        assert_eq!(
            bar.key(UrlBarKey::Enter, false),
            UrlBarOutcome::Navigate("http://127.0.0.1:8000/after".to_string())
        );
        assert!(!bar.is_editing());
        assert_eq!(bar.display_text(), "http://127.0.0.1:8000/before");
        assert_eq!(bar.key(UrlBarKey::Enter, false), UrlBarOutcome::Ignored);

        bar.set_page_url("http://127.0.0.1:8000/after");
        assert_eq!(bar.display_text(), "http://127.0.0.1:8000/after");
    }

    #[test]
    fn operations_are_ignored_until_the_bar_is_focused() {
        let mut bar = UrlBar::new("about:blank");
        assert_eq!(bar.insert_text("x"), UrlBarOutcome::Ignored);
        assert_eq!(bar.key(UrlBarKey::Backspace, false), UrlBarOutcome::Ignored);
        assert_eq!(bar.select_all(), UrlBarOutcome::Ignored);
        assert_eq!(bar.cancel(), UrlBarOutcome::Ignored);
        assert_eq!(bar.display_text(), "about:blank");
        assert_eq!(bar.caret(), None);
    }

    #[test]
    fn escape_restores_the_page_url_and_refocus_starts_a_fresh_edit() {
        let mut bar = editing("https://example.test/");
        bar.insert_text("typo");
        assert_eq!(bar.key(UrlBarKey::Escape, false), UrlBarOutcome::Cancelled);
        assert_eq!(bar.display_text(), "https://example.test/");

        bar.focus();
        assert_eq!(bar.display_text(), "https://example.test/");
        assert_eq!(bar.selection(), Some(0..21));
    }

    #[test]
    fn page_navigation_while_editing_keeps_the_draft() {
        let mut bar = editing("https://a.test/");
        bar.insert_text("x");
        bar.set_page_url("https://b.test/");
        assert_eq!(bar.display_text(), "https://a.test/x");
        assert_eq!(bar.cancel(), UrlBarOutcome::Cancelled);
        assert_eq!(bar.display_text(), "https://b.test/");
    }

    #[test]
    fn edits_respect_multibyte_character_boundaries() {
        let mut bar = editing("https://例え.test/😀");
        assert_eq!(bar.key(UrlBarKey::Backspace, false), UrlBarOutcome::Edited);
        assert_eq!(bar.display_text(), "https://例え.test/");

        bar.key(UrlBarKey::Home, false);
        for _ in 0.."https://".chars().count() {
            bar.key(UrlBarKey::Right, false);
        }
        bar.key(UrlBarKey::Delete, false);
        assert_eq!(bar.display_text(), "https://え.test/");
        bar.key(UrlBarKey::Right, true);
        assert_eq!(bar.selection(), Some(8..11));
        bar.insert_text("ä");
        assert_eq!(bar.display_text(), "https://ä.test/");
        assert_eq!(bar.caret(), Some(10));
        bar.key(UrlBarKey::Left, false);
        assert_eq!(bar.caret(), Some(8));
    }

    #[test]
    fn selection_extends_with_shift_and_collapses_to_its_edge() {
        let mut bar = editing("abcdef");
        bar.key(UrlBarKey::Left, true);
        bar.key(UrlBarKey::Left, true);
        assert_eq!(bar.selection(), Some(4..6));
        bar.key(UrlBarKey::Left, false);
        assert_eq!((bar.caret(), bar.selection()), (Some(4), None));

        bar.key(UrlBarKey::Home, true);
        assert_eq!(bar.selection(), Some(0..4));
        bar.key(UrlBarKey::Right, false);
        assert_eq!((bar.caret(), bar.selection()), (Some(4), None));

        bar.key(UrlBarKey::End, true);
        assert_eq!(bar.key(UrlBarKey::Backspace, false), UrlBarOutcome::Edited);
        assert_eq!(bar.display_text(), "abcd");

        bar.select_all();
        bar.key(UrlBarKey::Delete, false);
        assert_eq!(bar.display_text(), "");
    }

    #[test]
    fn caret_stays_put_at_the_ends_of_the_text() {
        let mut bar = editing("ab");
        bar.key(UrlBarKey::Right, false);
        bar.key(UrlBarKey::Delete, false);
        assert_eq!((bar.display_text(), bar.caret()), ("ab", Some(2)));
        bar.key(UrlBarKey::Home, false);
        bar.key(UrlBarKey::Left, false);
        bar.key(UrlBarKey::Backspace, false);
        assert_eq!((bar.display_text(), bar.caret()), ("ab", Some(0)));
    }

    #[test]
    fn control_characters_are_dropped_and_commit_trims_whitespace() {
        let mut bar = editing("");
        bar.insert_text("\t https://x.test/\n\u{7f} ");
        assert_eq!(bar.display_text(), " https://x.test/ ");
        assert_eq!(
            bar.key(UrlBarKey::Enter, false),
            UrlBarOutcome::Navigate("https://x.test/".to_string())
        );
    }

    #[test]
    fn committing_blank_text_keeps_editing_without_navigating() {
        let mut bar = editing("https://x.test/");
        bar.select_all();
        bar.insert_text("   ");
        assert_eq!(bar.key(UrlBarKey::Enter, false), UrlBarOutcome::Edited);
        assert!(bar.is_editing());
        assert_eq!(bar.page_url(), "https://x.test/");
    }
}
