//! Deterministic system-font discovery and face selection from OpenType metadata.

use super::{Font, FontError, FontFamilyKey, FontStyle, FontVariantKey};
use rustybuzz::ttf_parser::{self, name_id};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock, PoisonError};

/// The installed face selected for loading, shaping and rasterization.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct SystemFontFace {
    /// Font file, including the collection container for TTC/OTC faces.
    pub path: PathBuf,
    /// Zero-based face index within the file.
    pub face_index: u32,
    /// Localized typographic, WWS and legacy family names from the name table.
    pub families: Vec<String>,
    /// Full face name, when present in the font metadata.
    pub full_name: Option<String>,
    /// OpenType weight, usually 100 through 900.
    pub weight: u16,
    /// OpenType style mapped to the supported CSS style values.
    pub style: FontStyle,
    /// OpenType width class; 5 is normal, lower values are condensed.
    pub width: u16,
}

impl SystemFontFace {
    /// Loads exactly this face, retaining its identity for diagnostics.
    pub fn load(&self) -> Result<Font, FontError> {
        let bytes = std::fs::read(&self.path)?;
        let mut font = Font::load_from_bytes_at_index(bytes, self.face_index)?;
        font.system_face = Some(self.clone());
        Ok(font)
    }
}

/// An immutable index of font metadata with a bounded cache of loaded faces.
///
/// Discovery visits each directory once and indexes all collection faces. Files
/// and face indices break ties deterministically, independently of `read_dir`.
/// Rebuild the database to observe fonts installed after discovery.
pub struct SystemFontDatabase {
    faces: Vec<SystemFontFace>,
    families: HashMap<String, Vec<usize>>,
    unreadable: Vec<PathBuf>,
    loaded: Mutex<HashMap<usize, Arc<Font>>>,
}

impl SystemFontDatabase {
    /// Discovers TTF, OTF, TTC and OTC fonts under the given directories.
    ///
    /// Missing directories and unreadable/malformed files do not prevent other
    /// installed fonts from loading. Rejected files are available for diagnosis.
    pub fn from_directories(directories: &[PathBuf]) -> Self {
        let mut files = Vec::new();
        let mut visited = HashSet::new();
        let mut unreadable = Vec::new();
        for dir in directories {
            collect_files(dir, &mut visited, &mut files, &mut unreadable);
        }
        files.sort();
        files.dedup();
        let mut faces = Vec::new();
        for path in files {
            let Ok(bytes) = std::fs::read(&path) else {
                unreadable.push(path);
                continue;
            };
            let face_count = ttf_parser::fonts_in_collection(&bytes);
            // fonts_in_collection reads only the header's declared count. A
            // collection needs one four-byte offset per face after its header;
            // reject truncated lists before using that count as a loop bound.
            if face_count.is_some_and(|count| {
                usize::try_from(count)
                    .map_or(true, |count| count > bytes.len().saturating_sub(12) / 4)
            }) {
                unreadable.push(path);
                continue;
            }
            let before = faces.len();
            for index in 0..face_count.unwrap_or(1) {
                let Ok(face) = ttf_parser::Face::parse(&bytes, index) else {
                    continue;
                };
                let mut families = Vec::new();
                // Prefer typographic names for display, but match localized and
                // legacy aliases too (including collection faces on macOS).
                for id in [
                    name_id::TYPOGRAPHIC_FAMILY,
                    name_id::WWS_FAMILY,
                    name_id::FAMILY,
                ] {
                    for name in face.names().into_iter().filter(|n| n.name_id == id) {
                        if let Some(value) = decode_name(name)
                            && !value.trim().is_empty()
                            && !families.contains(&value)
                        {
                            families.push(value);
                        }
                    }
                }
                if families.is_empty() {
                    continue;
                }
                let full_name = face
                    .names()
                    .into_iter()
                    .filter(|n| n.name_id == name_id::FULL_NAME)
                    .find_map(decode_name);
                faces.push(SystemFontFace {
                    path: path.clone(),
                    face_index: index,
                    families,
                    full_name,
                    weight: face.weight().to_number(),
                    width: face.width().to_number(),
                    style: if face.is_oblique() {
                        FontStyle::Oblique
                    } else if face.is_italic() {
                        FontStyle::Italic
                    } else {
                        FontStyle::Normal
                    },
                });
            }
            if faces.len() == before {
                unreadable.push(path);
            }
        }
        let mut families = HashMap::<String, Vec<usize>>::new();
        for (index, face) in faces.iter().enumerate() {
            for name in &face.families {
                let indices = families.entry(family_name(name)).or_default();
                if indices.last() != Some(&index) {
                    indices.push(index);
                }
            }
        }
        unreadable.sort();
        unreadable.dedup();
        Self {
            faces,
            families,
            unreadable,
            loaded: Mutex::new(HashMap::new()),
        }
    }

    /// All discovered faces, including their source paths and style metadata.
    pub fn faces(&self) -> &[SystemFontFace] {
        &self.faces
    }

    /// Files or existing directories which could not be read as font sources.
    pub fn unreadable_paths(&self) -> &[PathBuf] {
        &self.unreadable
    }

    fn select_index(&self, family: &str, variant: FontVariantKey) -> Option<usize> {
        let family = family_name(family);
        let candidates = super::generic_family_fonts(&family);
        if !candidates.is_empty() {
            return candidates
                .into_iter()
                .find_map(|name| self.select_index(name, variant));
        }
        self.families
            .get(&family)?
            .iter()
            .copied()
            .min_by_key(|index| {
                let face = &self.faces[*index];
                // This API requests normal stretch. Match width before style
                // and weight; condensed aliases must not beat a normal face.
                let width = if face.width <= 5 {
                    (0, 5 - face.width)
                } else {
                    (1, face.width - 5)
                };
                (
                    width,
                    style_rank(variant.style, face.style),
                    weight_rank(variant.weight.0, face.weight),
                    *index,
                )
            })
    }

    /// Selects a face for one family (or a generic family), weight and style.
    ///
    /// Family names match metadata, not filename substrings. Missing weights
    /// follow CSS Fonts matching, including the 400–500 special case.
    pub fn select(&self, family: &str, variant: FontVariantKey) -> Option<&SystemFontFace> {
        self.select_index(family, variant)
            .map(|index| &self.faces[index])
    }

    /// Loads and shares the selected face; caching never substitutes another style.
    pub fn load(&self, family: &str, variant: FontVariantKey) -> Result<Arc<Font>, FontError> {
        let index = self
            .select_index(family, variant)
            .ok_or_else(|| FontError::Other(format!("System font '{family}' not found")))?;
        {
            let loaded = self.loaded.lock().unwrap_or_else(PoisonError::into_inner);
            if let Some(font) = loaded.get(&index) {
                return Ok(Arc::clone(font));
            }
        }
        let font = Arc::new(self.faces[index].load()?);
        let mut loaded = self.loaded.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(existing) = loaded.get(&index) {
            return Ok(Arc::clone(existing));
        }
        if loaded.len() >= 20
            && let Some(key) = loaded.keys().next().copied()
        {
            loaded.remove(&key);
        }
        loaded.insert(index, Arc::clone(&font));
        Ok(font)
    }
}

fn decode_name(name: ttf_parser::name::Name<'_>) -> Option<String> {
    if name.platform_id == ttf_parser::PlatformId::Macintosh && name.encoding_id == 0 {
        // ttf-parser decodes Unicode records only. Legacy Macintosh fonts can
        // provide their family names exclusively in Mac Roman (encoding 0).
        encoding_rs::MACINTOSH
            .decode_without_bom_handling_and_without_replacement(name.name)
            .map(|value| value.into_owned())
    } else {
        name.to_string()
    }
}

fn collect_files(
    dir: &Path,
    visited: &mut HashSet<PathBuf>,
    files: &mut Vec<PathBuf>,
    unreadable: &mut Vec<PathBuf>,
) {
    let Ok(canonical) = dir.canonicalize() else {
        return;
    };
    if !visited.insert(canonical.clone()) {
        return;
    }
    let Ok(entries) = std::fs::read_dir(&canonical) else {
        unreadable.push(canonical);
        return;
    };
    for entry in entries {
        let Ok(entry) = entry else { continue };
        let path = entry.path();
        if path.is_dir() {
            collect_files(&path, visited, files, unreadable);
        } else if path.extension().and_then(|v| v.to_str()).is_some_and(|v| {
            ["ttf", "otf", "ttc", "otc"]
                .iter()
                .any(|ext| v.eq_ignore_ascii_case(ext))
        }) {
            files.push(path.canonicalize().unwrap_or(path));
        }
    }
}

fn family_name(name: &str) -> String {
    name.trim().trim_matches(['\'', '"']).to_lowercase()
}

pub(super) fn style_rank(target: FontStyle, candidate: FontStyle) -> u8 {
    if target == candidate {
        return 0;
    }
    match (target, candidate) {
        (FontStyle::Normal, FontStyle::Oblique)
        | (FontStyle::Italic, FontStyle::Oblique)
        | (FontStyle::Oblique, FontStyle::Italic) => 1,
        _ => 2,
    }
}

// CSS Fonts 4 §5.2: choose the search group before distance within that group.
pub(super) fn weight_rank(target: u16, candidate: u16) -> (u8, u16) {
    if (400..=500).contains(&target) {
        if (target..=500).contains(&candidate) {
            (0, candidate - target)
        } else if candidate < target {
            (1, target - candidate)
        } else {
            (2, candidate - target)
        }
    } else if target < 400 {
        if candidate <= target {
            (0, target - candidate)
        } else {
            (1, candidate - target)
        }
    } else if candidate >= target {
        (0, candidate - target)
    } else {
        (1, target - candidate)
    }
}

static SYSTEM_FONTS: OnceLock<Arc<SystemFontDatabase>> = OnceLock::new();

/// Returns the process-wide system-font inventory, discovered on first use.
pub fn system_font_database() -> Arc<SystemFontDatabase> {
    #[cfg(test)]
    if let Some(db) = TEST_DATABASE.with(|slot| slot.borrow().clone()) {
        return db;
    }
    Arc::clone(SYSTEM_FONTS.get_or_init(|| {
        Arc::new(SystemFontDatabase::from_directories(
            &super::system_font_dirs(),
        ))
    }))
}

/// Selects an installed face while retaining file and collection index information.
pub fn find_system_font_face(family: &str, variant: FontVariantKey) -> Option<SystemFontFace> {
    system_font_database().select(family, variant).cloned()
}

pub(crate) enum SelectedFont<'a> {
    Web(&'a Font),
    System(Arc<Font>),
}

impl AsRef<Font> for SelectedFont<'_> {
    fn as_ref(&self) -> &Font {
        match self {
            Self::Web(font) => font,
            Self::System(font) => font.as_ref(),
        }
    }
}

/// The same primary-face decision is used by layout, text paint and form controls.
pub(crate) fn select_text_font<'a>(
    phase: &str,
    family: Option<FontFamilyKey>,
    variant: FontVariantKey,
    web_fonts: Option<&'a super::WebFontRegistry>,
    fallbacks: &[Arc<Font>],
) -> Option<SelectedFont<'a>> {
    let selected = select_text_font_impl(family, variant, web_fonts, fallbacks);
    let font = selected
        .as_ref()
        .map(AsRef::as_ref)
        .or_else(|| fallbacks.first().map(Arc::as_ref));
    super::diagnostics::record(
        phase,
        family,
        variant,
        font,
        matches!(selected, Some(SelectedFont::Web(_))),
    );
    selected
}

fn select_text_font_impl<'a>(
    family: Option<FontFamilyKey>,
    variant: FontVariantKey,
    web_fonts: Option<&'a super::WebFontRegistry>,
    fallbacks: &[Arc<Font>],
) -> Option<SelectedFont<'a>> {
    // Explicit caller-supplied fonts (including strict image fixtures) retain
    // their existing authority. Installed defaults carry their system identity.
    let use_system = fallbacks.iter().any(|font| font.system_face.is_some());
    if let Some(family) = family {
        for name in family.families().iter() {
            if let Some(font) = web_fonts
                .and_then(|registry| registry.select_best(name, variant.weight, variant.style))
            {
                return Some(SelectedFont::Web(font));
            }
            if use_system && let Ok(font) = system_font_database().load(name, variant) {
                return Some(SelectedFont::System(font));
            }
        }
    }
    if use_system
        && variant != FontVariantKey::normal()
        && let Some(face) = fallbacks.first().and_then(|font| font.system_face.as_ref())
        && let Some(name) = face.families.first()
        && let Ok(font) = system_font_database().load(name, variant)
    {
        return Some(SelectedFont::System(font));
    }
    None
}

#[cfg(test)]
thread_local! {
    static TEST_DATABASE: std::cell::RefCell<Option<Arc<SystemFontDatabase>>> = const { std::cell::RefCell::new(None) };
}

#[cfg(test)]
pub(crate) fn with_test_database<T>(database: SystemFontDatabase, f: impl FnOnce() -> T) -> T {
    struct Restore(Option<Arc<SystemFontDatabase>>);
    impl Drop for Restore {
        fn drop(&mut self) {
            TEST_DATABASE.with(|slot| *slot.borrow_mut() = self.0.take());
        }
    }
    let _restore = Restore(TEST_DATABASE.with(|slot| slot.replace(Some(Arc::new(database)))));
    f()
}

pub(super) fn parse_family_list(value: &str) -> Vec<String> {
    let mut families = Vec::new();
    let mut name = String::new();
    let mut quote = None;
    let mut escaped = false;
    for ch in value.chars() {
        if escaped {
            name.push(ch);
            escaped = false;
        } else if ch == '\\' {
            escaped = true;
        } else if quote == Some(ch) {
            quote = None;
        } else if quote.is_none() && matches!(ch, '\'' | '"') {
            quote = Some(ch);
        } else if quote.is_none() && ch == ',' {
            if !name.trim().is_empty() {
                families.push(name.trim().to_string());
            }
            name.clear();
        } else {
            name.push(ch);
        }
    }
    if !name.trim().is_empty() {
        families.push(name.trim().to_string());
    }
    families
}

pub(crate) fn load_default_text_fonts_shared() -> Vec<Arc<Font>> {
    let database = system_font_database();
    let mut fonts = Vec::new();
    for family in super::default_text_font_families() {
        if let Ok(font) = database.load(family, FontVariantKey::normal())
            && !fonts.iter().any(|old| Arc::ptr_eq(old, &font))
        {
            fonts.push(font);
        }
    }
    fonts
}
