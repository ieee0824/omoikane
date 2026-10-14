//! Module to read the list of test suites from disk.

use std::{
    collections::HashMap,
    ffi::OsStr,
    fs,
    path::{Path, PathBuf},
};

use color_eyre::{
    Result,
    eyre::{OptionExt, WrapErr},
};
use cow_utils::CowUtils;
use rustc_hash::{FxBuildHasher, FxHashMap};
use serde::Deserialize;

use crate::{HarnessFile, Ignored};

use super::{Harness, Locale, Phase, Test, TestSuite};

/// Representation of the YAML metadata in Test262 tests.
#[derive(Debug, Clone, Deserialize)]
pub(super) struct MetaData {
    pub(super) description: Box<str>,
    pub(super) esid: Option<Box<str>>,
    #[allow(dead_code)]
    pub(super) es5id: Option<Box<str>>,
    pub(super) es6id: Option<Box<str>>,
    #[serde(default)]
    pub(super) info: Box<str>,
    #[serde(default)]
    pub(super) features: Box<[Box<str>]>,
    #[serde(default)]
    pub(super) includes: Box<[Box<str>]>,
    #[serde(default)]
    pub(super) flags: Box<[TestFlag]>,
    #[serde(default)]
    pub(super) negative: Option<Negative>,
    #[serde(default)]
    pub(super) locale: Locale,
}

/// Negative test information structure.
#[derive(Debug, Clone, Deserialize)]
pub(super) struct Negative {
    pub(super) phase: Phase,
    #[serde(rename = "type")]
    pub(super) error_type: ErrorType,
}

/// All possible error types
#[derive(Debug, Copy, Clone, Deserialize, PartialEq, Eq)]
#[allow(clippy::enum_variant_names)] // Better than appending `rename` to all variants
pub(super) enum ErrorType {
    Test262Error,
    SyntaxError,
    ReferenceError,
    RangeError,
    TypeError,
    EvalError,
}

impl ErrorType {
    pub(super) const fn as_str(self) -> &'static str {
        match self {
            Self::Test262Error => "Test262Error",
            Self::SyntaxError => "SyntaxError",
            Self::ReferenceError => "ReferenceError",
            Self::RangeError => "RangeError",
            Self::TypeError => "TypeError",
            Self::EvalError => "EvalError",
        }
    }
}

/// Individual test flag.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) enum TestFlag {
    OnlyStrict,
    NoStrict,
    Module,
    Raw,
    Async,
    Generated,
    #[serde(rename = "CanBlockIsFalse")]
    CanBlockIsFalse,
    #[serde(rename = "CanBlockIsTrue")]
    CanBlockIsTrue,
    #[serde(rename = "non-deterministic")]
    NonDeterministic,
}

/// Reads the Test262 defined bindings.
pub(super) fn read_harness(test262_path: &Path) -> Result<Harness> {
    let mut includes: HashMap<Box<str>, HarnessFile, FxBuildHasher> = FxHashMap::default();

    let harness_path = &test262_path.join("harness");

    read_harness_dir(harness_path, harness_path, &mut includes)?;

    let assert = includes
        .remove("assert.js")
        .ok_or_eyre("failed to load harness file `assert.js`")?;
    let sta = includes
        .remove("sta.js")
        .ok_or_eyre("failed to load harness file `sta.js`")?;
    let doneprint_handle = includes
        .remove("doneprintHandle.js")
        .ok_or_eyre("failed to load harness file `donePrintHandle.js`")?;

    Ok(Harness {
        assert,
        sta,
        doneprint_handle,
        includes,
    })
}

fn read_harness_dir(
    harness_root: &Path,
    directory_name: &Path,
    includes: &mut HashMap<Box<str>, HarnessFile, FxBuildHasher>,
) -> Result<()> {
    for entry in fs::read_dir(directory_name).wrap_err("error reading the harness directory")? {
        let entry = entry?;
        let entry_path = entry.path();

        if entry.file_type()?.is_dir() {
            read_harness_dir(harness_root, &entry_path, includes)?;
            continue;
        }

        let key = entry_path
            .strip_prefix(harness_root)
            .wrap_err("invalid harness file path")?;

        includes.insert(key.to_string_lossy().into(), read_harness_file(entry_path)?);
    }

    Ok(())
}

fn read_harness_file(path: PathBuf) -> Result<HarnessFile> {
    let content = fs::read_to_string(path.as_path())
        .wrap_err_with(|| format!("error reading the harness file `{}`", path.display()))?;

    Ok(HarnessFile {
        content: content.into_boxed_str(),
        path: path.into_boxed_path(),
    })
}

/// Reads a test suite in the given path.
pub(super) fn read_suite(
    path: &Path,
    ignored: &Ignored,
    mut ignore_suite: bool,
) -> Result<TestSuite> {
    let name = path
        .file_name()
        .and_then(OsStr::to_str)
        .ok_or_eyre("invalid path for test suite")?;

    ignore_suite |= ignored.contains_test(path);

    let mut suites = Vec::new();
    let mut tests = Vec::new();

    // TODO: iterate in parallel
    for entry in path.read_dir().wrap_err("could not retrieve entry")? {
        let entry = entry?;
        let filetype = entry.file_type().wrap_err("could not retrieve file type")?;

        if filetype.is_dir() {
            suites.push(
                read_suite(entry.path().as_path(), ignored, ignore_suite).wrap_err_with(|| {
                    let path = entry.path();
                    let suite = path.display();
                    format!("error reading sub-suite {suite}")
                })?,
            );
            continue;
        }

        let path = entry.path();

        if path.extension() != Some(OsStr::new("js")) {
            // Ignore files that aren't executable.
            continue;
        }

        if path
            .file_stem()
            .is_some_and(|stem| stem.as_encoded_bytes().ends_with(b"FIXTURE"))
        {
            // Ignore files that are fixtures.
            continue;
        }

        let mut test = read_test(&path).wrap_err_with(|| {
            let path = entry.path();
            let suite = path.display();
            format!("error reading test {suite}")
        })?;

        if ignore_suite
            || ignored.contains_any_flag(test.flags)
            || ignored.contains_test(&test.path)
            || test
                .features
                .iter()
                .any(|feat| ignored.contains_feature(feat))
        {
            test.set_ignored();
        }
        tests.push(test);
    }

    Ok(TestSuite {
        name: name.into(),
        path: Box::from(path),
        suites: suites.into_boxed_slice(),
        tests: tests.into_boxed_slice(),
    })
}

/// Reads information about a given test case.
pub(super) fn read_test(path: &Path) -> Result<Test> {
    let name = path
        .file_stem()
        .and_then(OsStr::to_str)
        .ok_or_eyre("invalid path for test")?;

    let metadata = read_metadata(path)?;

    Test::new(name, path, metadata)
}

/// Reads the metadata from the input test code.
fn read_metadata(test: &Path) -> Result<MetaData> {
    let code = fs::read_to_string(test)?;

    let (_, metadata) = code
        .split_once("/*---")
        .ok_or_eyre("invalid test metadata")?;
    let (metadata, _) = metadata
        .split_once("---*/")
        .ok_or_eyre("invalid test metadata")?;
    let metadata = metadata.cow_replace('\r', "\n");

    parse_metadata(&metadata)
}

/// Deserializes a Test262 metadata block after its source delimiters are removed.
fn parse_metadata(metadata: &str) -> Result<MetaData> {
    // Test262 has bare numeric identifiers such as `es6id: 19.4`.
    // Preserve serde_yaml's typed-string treatment of those scalars.
    let config = noyalib::ParserConfig::serde_yaml_compat().plain_scalar_strings(true);
    noyalib::from_str_with_config(metadata, &config).map_err(Into::into)
}

#[cfg(test)]
mod metadata_tests {
    use super::{ErrorType, TestFlag, parse_metadata};

    #[test]
    fn parses_test262_flow_lists_block_text_and_negative_metadata() {
        let metadata = parse_metadata(
            r#"description: An ordinary Test262 test.
esid: sec-example
es6id: 19.4
info: |
  First line.
  Second line.
includes: [assert.js, compareArray.js]
features: [Symbol, regexp-v-flag]
flags: [onlyStrict, async]
negative:
  phase: parse
  type: SyntaxError
locale: [en-US, ja-JP]
"#,
        )
        .expect("Test262 metadata");

        assert_eq!(&*metadata.description, "An ordinary Test262 test.");
        assert_eq!(metadata.esid.as_deref(), Some("sec-example"));
        assert_eq!(metadata.es6id.as_deref(), Some("19.4"));
        assert_eq!(&*metadata.info, "First line.\nSecond line.\n");
        assert_eq!(
            metadata
                .includes
                .iter()
                .map(AsRef::as_ref)
                .collect::<Vec<&str>>(),
            ["assert.js", "compareArray.js"]
        );
        assert_eq!(
            metadata
                .features
                .iter()
                .map(AsRef::as_ref)
                .collect::<Vec<&str>>(),
            ["Symbol", "regexp-v-flag"]
        );
        assert_eq!(&*metadata.flags, &[TestFlag::OnlyStrict, TestFlag::Async]);
        assert_eq!(
            metadata
                .negative
                .as_ref()
                .expect("negative metadata")
                .error_type,
            ErrorType::SyntaxError
        );
        assert_eq!(
            metadata
                .locale
                .locale
                .iter()
                .map(AsRef::as_ref)
                .collect::<Vec<&str>>(),
            ["en-US", "ja-JP"]
        );
    }

    #[test]
    fn rejects_missing_required_test262_description() {
        assert!(parse_metadata("features: [Symbol]\n").is_err());
    }
}
