//! Assemble trusted bootstrap sections before JavaScript parsing.
//!
//! Markers are explicit JavaScript comments at statement boundaries, inserted
//! during the reviewed migration. This is not a parser for arbitrary JavaScript;
//! the reserved marker prefix must not occur in strings or template literals.
use std::collections::HashSet;

/// Selects a global's sections without changing bytes outside the directives.
pub(super) fn select(source: &str, worker: bool) -> Result<String, &'static str> {
    const PREFIX: &str = "/* @omoikane-bootstrap ";
    let mut result = String::with_capacity(source.len());
    let mut cursor = 0;
    let mut active = None;
    let mut names = HashSet::new();
    while let Some(offset) = source[cursor..].find(PREFIX) {
        let start = cursor + offset;
        if active.is_none_or(|kind| kind == worker) {
            result.push_str(&source[cursor..start]);
        }
        let body_start = start + PREFIX.len();
        let end = body_start
            + source[body_start..]
                .find("*/")
                .ok_or("unclosed directive")?;
        let fields: Vec<_> = source[body_start..end].split_whitespace().collect();
        match fields.as_slice() {
            ["end"] => {
                if active.take().is_none() {
                    return Err("unmatched end");
                }
            }
            [kind @ ("window-only" | "worker-only"), name] => {
                if active.is_some() {
                    return Err("nested section");
                }
                if !name
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
                {
                    return Err("invalid section name");
                }
                if !names.insert((*kind, *name)) {
                    return Err("duplicate section");
                }
                active = Some(*kind == "worker-only");
            }
            _ => return Err("unknown directive"),
        }
        cursor = end + 2;
    }
    if active.is_some() {
        return Err("unclosed section");
    }
    result.push_str(&source[cursor..]);
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selection_preserves_common_text_and_utf8() {
        let input = "λ/* @omoikane-bootstrap window-only dom */D/* @omoikane-bootstrap end */中/* @omoikane-bootstrap worker-only child */C/* @omoikane-bootstrap end */\n";
        assert_eq!(select(input, false).unwrap(), "λD中\n");
        assert_eq!(select(input, true).unwrap(), "λ中C\n");
        assert_eq!(select("const a = 1;\n", true).unwrap(), "const a = 1;\n");
    }

    #[test]
    fn malformed_sections_fail_closed() {
        for source in [
            "/* @omoikane-bootstrap end */",
            "/* @omoikane-bootstrap window-only missing */code",
            "/* @omoikane-bootstrap unknown x */",
            "/* @omoikane-bootstrap worker-only a */ /* @omoikane-bootstrap window-only b */",
            "/* @omoikane-bootstrap worker-only a */ /* @omoikane-bootstrap end */ /* @omoikane-bootstrap worker-only a */",
            "/* @omoikane-bootstrap worker-only a! */",
            "/* @omoikane-bootstrap worker-only",
        ] {
            assert!(select(source, true).is_err(), "accepted {source}");
        }
    }

    #[test]
    fn embedded_window_source_retains_the_control_bytes() {
        use sha2::{Digest, Sha256};
        let source = select(super::super::DOM_BOOTSTRAP, false).unwrap();
        assert_eq!(
            format!("{:x}", Sha256::digest(source.as_bytes())),
            "fa60dc3b9627b1d8c8f63d3230c4bafa2af127cff459352d2035abc31640efe2"
        );
    }
}
