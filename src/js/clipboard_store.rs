//! Thread-safe clipboard ownership without references to a Document or JS heap.

use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex};

/// A normalized format and its immutable, owned bytes.
#[derive(Clone, Debug)]
pub(super) struct Representation {
    pub(super) format: String,
    pub(super) bytes: Arc<[u8]>,
}

/// Format names and a lossless version token; payload bytes remain in the store.
#[cfg(test)]
#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub(super) struct Metadata {
    pub(super) generation: String,
    pub(super) formats: Vec<String>,
}

/// An owned representation transferred across the private JS/host boundary.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub(super) struct WireRepresentation {
    pub(super) format: String,
    pub(super) bytes: Vec<u8>,
}

#[derive(Debug, Default)]
struct Data {
    generation: u64,
    representations: Vec<Representation>,
}

#[derive(Clone, Debug, Default)]
pub(super) struct HostClipboard(Arc<Mutex<Data>>);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct GenerationExhausted;

impl std::fmt::Display for GenerationExhausted {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("clipboard generation space exhausted")
    }
}

impl std::error::Error for GenerationExhausted {}

#[cfg(test)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ReadError {
    Stale,
    NotFound,
}

impl HostClipboard {
    fn with_data<T>(&self, action: impl FnOnce(&mut Data) -> T) -> T {
        let mut data = self.0.lock().unwrap_or_else(|error| error.into_inner());
        action(&mut data)
    }

    /// Reads a text snapshot, releasing the store lock before decoding bytes.
    pub(super) fn read_text(&self) -> String {
        let bytes = self.with_data(|data| {
            data.representations
                .iter()
                .find(|representation| representation.format == "text/plain")
                .map(|representation| Arc::clone(&representation.bytes))
        });
        bytes
            .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
            .unwrap_or_default()
    }

    /// Replaces the current item with one UTF-8 text representation.
    pub(super) fn write_text(&self, text: String) -> Result<(), GenerationExhausted> {
        self.replace(vec![Representation {
            format: "text/plain".to_owned(),
            bytes: text.into_bytes().into(),
        }])
        .map(|_| ())
    }

    /// Atomically replaces all representations and advances the change token.
    /// Input conversion and format validation belong to the calling API bridge.
    pub(super) fn replace(
        &self,
        representations: Vec<Representation>,
    ) -> Result<u64, GenerationExhausted> {
        self.with_data(|data| {
            let generation = data.generation.checked_add(1).ok_or(GenerationExhausted)?;
            data.representations = representations;
            data.generation = generation;
            Ok(generation)
        })
    }

    /// Replaces the clipboard from values validated by serde at the host boundary.
    pub(super) fn replace_wire(
        &self,
        representations: Vec<WireRepresentation>,
    ) -> Result<u64, GenerationExhausted> {
        self.replace(
            representations
                .into_iter()
                .map(|representation| Representation {
                    format: representation.format,
                    bytes: representation.bytes.into(),
                })
                .collect(),
        )
    }

    /// Copies one atomic clipboard snapshot into transport-owned values.
    pub(super) fn snapshot_wire(&self) -> Vec<WireRepresentation> {
        self.with_data(|data| {
            data.representations
                .iter()
                .map(|representation| WireRepresentation {
                    format: representation.format.clone(),
                    bytes: representation.bytes.to_vec(),
                })
                .collect()
        })
    }

    /// Reads names and a generation without copying or decoding payload bytes.
    #[cfg(test)]
    pub(super) fn metadata(&self) -> Metadata {
        self.with_data(|data| Metadata {
            generation: data.generation.to_string(),
            formats: data
                .representations
                .iter()
                .map(|representation| representation.format.clone())
                .collect(),
        })
    }

    /// Checks the token even when a caller already cached a representation.
    #[cfg(test)]
    pub(super) fn is_current(&self, generation: u64) -> bool {
        self.with_data(|data| data.generation == generation)
    }

    /// Gets owned payload bytes only if the item still has the requested token.
    #[cfg(test)]
    pub(super) fn read_representation(
        &self,
        generation: u64,
        format: &str,
    ) -> Result<Arc<[u8]>, ReadError> {
        self.with_data(|data| {
            if data.generation != generation {
                return Err(ReadError::Stale);
            }
            data.representations
                .iter()
                .find(|representation| representation.format == format)
                .map(|representation| Arc::clone(&representation.bytes))
                .ok_or(ReadError::NotFound)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn representation(format: &str, bytes: &[u8]) -> Representation {
        Representation {
            format: format.to_owned(),
            bytes: Arc::from(bytes),
        }
    }

    #[test]
    fn metadata_and_payloads_remain_snapshots_after_replacement() {
        let clipboard = HostClipboard::default();
        let first = clipboard
            .replace(vec![representation(
                "web application/example",
                &[0, 128, 255],
            )])
            .unwrap();
        let metadata = clipboard.metadata();
        let bytes = clipboard
            .read_representation(first, "web application/example")
            .unwrap();
        clipboard.write_text("new text".to_owned()).unwrap();
        assert_eq!(metadata.formats, ["web application/example"]);
        assert_eq!(&*bytes, &[0, 128, 255]);
        assert_eq!(clipboard.read_text(), "new text");
        assert!(!clipboard.is_current(first));
        assert_eq!(
            clipboard.read_representation(first, "web application/example"),
            Err(ReadError::Stale)
        );
    }

    #[test]
    fn identical_writes_still_invalidate_earlier_reads() {
        let clipboard = HostClipboard::default();
        clipboard.write_text("same".to_owned()).unwrap();
        let first = clipboard.metadata().generation.parse().unwrap();
        clipboard.write_text("same".to_owned()).unwrap();
        assert!(!clipboard.is_current(first));
        let current = clipboard.metadata().generation.parse().unwrap();
        assert_eq!(
            clipboard.read_representation(current, "text/html"),
            Err(ReadError::NotFound)
        );
    }

    #[test]
    fn text_replaces_other_formats_and_nontext_has_no_plain_text() {
        let clipboard = HostClipboard::default();
        clipboard
            .replace(vec![representation("image/png", &[0, 1, 2])])
            .unwrap();
        assert_eq!(clipboard.read_text(), "");
        clipboard.write_text("あ😀".to_owned()).unwrap();
        assert_eq!(clipboard.metadata().formats, ["text/plain"]);
        assert_eq!(clipboard.read_text(), "あ😀");
    }

    #[test]
    fn exhausted_generation_does_not_commit_partial_contents() {
        let clipboard = HostClipboard::default();
        clipboard.write_text("original".to_owned()).unwrap();
        clipboard.with_data(|data| data.generation = u64::MAX);
        let metadata = clipboard.metadata();
        assert_eq!(metadata.generation, u64::MAX.to_string());
        assert_eq!(
            clipboard.write_text("replacement".to_owned()),
            Err(GenerationExhausted)
        );
        assert_eq!(clipboard.read_text(), "original");
        assert_eq!(clipboard.metadata(), metadata);
    }
}
