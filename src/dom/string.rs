//! Owned DOMString payloads at parser and JavaScript mutation boundaries.

/// Scalar strings move into the DOM; exact units are retained only when needed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum DomString {
    Scalar(String),
    Utf16(Vec<u16>),
}

impl DomString {
    pub(crate) fn from_utf16(units: Vec<u16>) -> Self {
        match String::from_utf16(&units) {
            Ok(value) => Self::Scalar(value),
            Err(_) => Self::Utf16(units),
        }
    }

    pub(crate) fn into_parts(self) -> (String, Option<Vec<u16>>) {
        match self {
            Self::Scalar(value) => (value, None),
            Self::Utf16(units) => (String::from_utf16_lossy(&units), Some(units)),
        }
    }

    pub(crate) fn is_empty(&self) -> bool {
        match self {
            Self::Scalar(value) => value.is_empty(),
            Self::Utf16(units) => units.is_empty(),
        }
    }
}
