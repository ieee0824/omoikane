//! Pure custom element name classification for native definition state.

/// Valid autonomous custom element names under the current HTML/DOM rules.
pub(crate) fn valid_name(name: &str) -> bool {
    if !name.as_bytes().first().is_some_and(u8::is_ascii_lowercase)
        || !name.contains('-')
        || matches!(
            name,
            "annotation-xml"
                | "color-profile"
                | "font-face"
                | "font-face-src"
                | "font-face-uri"
                | "font-face-format"
                | "font-face-name"
                | "missing-glyph"
        )
    {
        return false;
    }
    // A leading ASCII lower alpha selects the first DOM local-name branch.
    // Apart from the exclusions below, all Unicode code points are permitted.
    !name.chars().any(|c| {
        c.is_ascii_uppercase()
            || matches!(c, '\0' | '\t' | '\n' | '\u{000c}' | '\r' | ' ' | '/' | '>')
    })
}
