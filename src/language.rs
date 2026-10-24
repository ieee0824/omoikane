//! Browser language preferences derived from the host environment.

use std::sync::OnceLock;

const FALLBACK_LOCALE: &str = "en-US";

/// Browser-facing language preferences captured from the host environment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LanguagePreferences {
    languages: Vec<String>,
}

impl LanguagePreferences {
    fn from_locales<'a>(locales: impl IntoIterator<Item = &'a str>) -> Self {
        let mut languages = locales.into_iter().filter_map(normalize_locale).fold(
            Vec::new(),
            |mut languages, locale| {
                if !languages.contains(&locale) {
                    languages.push(locale);
                }
                languages
            },
        );
        if languages.is_empty() {
            languages.push(FALLBACK_LOCALE.to_string());
        }
        if let Some(language) = languages[0].split('-').next().map(str::to_string)
            && !languages.contains(&language)
        {
            languages.push(language);
        }
        Self { languages }
    }

    /// Most-preferred language exposed as `navigator.language`.
    pub(crate) fn primary(&self) -> &str {
        &self.languages[0]
    }

    /// Ordered languages exposed as `navigator.languages`.
    pub(crate) fn languages(&self) -> &[String] {
        &self.languages
    }

    /// Value sent in the HTTP `Accept-Language` request header.
    pub(crate) fn accept_language(&self) -> String {
        self.languages
            .iter()
            .enumerate()
            .map(|(index, language)| {
                if index == 0 {
                    language.clone()
                } else {
                    let quality = 10usize.saturating_sub(index).max(1);
                    format!("{language};q=0.{quality}")
                }
            })
            .collect::<Vec<_>>()
            .join(",")
    }
}

/// Returns the process-wide host language preferences.
pub(crate) fn preferences() -> &'static LanguagePreferences {
    static PREFERENCES: OnceLock<LanguagePreferences> = OnceLock::new();
    PREFERENCES.get_or_init(|| {
        let locales = sys_locale::get_locales().collect::<Vec<_>>();
        LanguagePreferences::from_locales(locales.iter().map(String::as_str))
    })
}

fn normalize_locale(raw: &str) -> Option<String> {
    let raw = raw.trim();
    if raw.is_empty() || raw.eq_ignore_ascii_case("c") || raw.eq_ignore_ascii_case("posix") {
        return None;
    }
    let without_suffix = raw.split(['.', '@']).next()?;
    let parts = without_suffix
        .split(['-', '_'])
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>();
    let language = parts.first()?;
    if !language.chars().all(|ch| ch.is_ascii_alphabetic()) {
        return None;
    }

    let mut normalized = Vec::with_capacity(parts.len());
    normalized.push(language.to_ascii_lowercase());
    for part in &parts[1..] {
        if !part.chars().all(|ch| ch.is_ascii_alphanumeric()) {
            return None;
        }
        normalized.push(match part.len() {
            2 if part.chars().all(|ch| ch.is_ascii_alphabetic()) => part.to_ascii_uppercase(),
            4 if part.chars().all(|ch| ch.is_ascii_alphabetic()) => {
                let mut chars = part.chars();
                let first = chars.next()?.to_ascii_uppercase();
                format!("{first}{}", chars.as_str().to_ascii_lowercase())
            }
            _ => part.to_ascii_lowercase(),
        });
    }
    Some(normalized.join("-"))
}

#[cfg(test)]
mod tests {
    use super::{LanguagePreferences, normalize_locale};

    #[test]
    fn normalizes_common_host_locale_forms() {
        assert_eq!(normalize_locale("ja_JP.UTF-8"), Some("ja-JP".to_string()));
        assert_eq!(
            normalize_locale("zh_Hant_TW"),
            Some("zh-Hant-TW".to_string())
        );
        assert_eq!(normalize_locale("C"), None);
    }

    #[test]
    fn builds_browser_language_values_from_locale() {
        let preferences = LanguagePreferences::from_locales(["ja_JP.UTF-8", "en_US", "ja_JP"]);
        assert_eq!(preferences.primary(), "ja-JP");
        assert_eq!(preferences.languages(), ["ja-JP", "en-US", "ja"]);
        assert_eq!(preferences.accept_language(), "ja-JP,en-US;q=0.9,ja;q=0.8");
    }

    #[test]
    fn falls_back_for_an_unusable_locale() {
        let preferences = LanguagePreferences::from_locales(["POSIX"]);
        assert_eq!(preferences.primary(), "en-US");
        assert_eq!(preferences.languages(), ["en-US", "en"]);
    }
}
