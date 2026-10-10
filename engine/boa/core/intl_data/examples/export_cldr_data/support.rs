use boa_intl_data::{PluralCategory, PluralPatterns, StringFields};
use icu_provider::{
    dynutil::UpcastDataPayload,
    export::{DataExporter, ExportMarker},
    prelude::*,
};
use icu_provider_blob::export::BlobExporter;
use serde_json::{Map, Value};
use std::{
    collections::BTreeMap,
    error::Error,
    fs::OpenOptions,
    io::Write,
    path::{Path, PathBuf},
    process::Command,
};

pub type Result<T> = std::result::Result<T, Box<dyn Error>>;
pub type Counts = BTreeMap<&'static str, usize>;

pub struct Source {
    directory: PathBuf,
    pub provenance: Value,
}

impl Source {
    pub fn open(directory: PathBuf) -> Result<Self> {
        let validator = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../icu_provider/scripts/intl-cldr/validate_payloads.py");
        let status = Command::new("python3")
            .arg(validator)
            .arg(&directory)
            .status()?;
        if !status.success() {
            return Err("CLDR input validation failed".into());
        }
        let provenance =
            serde_json::from_slice(&std::fs::read(directory.join("provenance.json"))?)?;
        Ok(Self {
            directory,
            provenance,
        })
    }

    pub fn read(&self, name: &str) -> Result<Value> {
        Ok(serde_json::from_slice(&std::fs::read(
            self.directory.join(name),
        )?)?)
    }

    pub fn write_report(&self, output: &Path, report: &Path, counts: &Counts) -> Result<()> {
        let description = serde_json::json!({
            "schemaVersion": 1, "source": self.provenance["source"],
            "rootSource": self.provenance["rootSource"],
            "inputOutputFiles": self.provenance["outputFiles"],
            "generatorFiles": self.provenance["generatorFiles"],
            "numberRangeSupplementalInput": self.provenance["inputFiles"][
                "cldr-core/supplemental/pluralRanges.json"],
            "markerCounts": counts, "postcardBytes": std::fs::metadata(output)?.len(),
            "postcardSha256": sha256(output)?,
            "encoding": "ICU4X 2.0 BlobExporter postcard with zero-copy typed schemas",
            "coverage": "all 739 CLDR JSON locales and all 77 numeric numbering systems",
        });
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(report)?;
        serde_json::to_writer_pretty(&mut file, &description)?;
        file.write_all(b"\n")?;
        Ok(())
    }
}

pub fn object(value: &Value) -> Result<&Map<String, Value>> {
    value
        .as_object()
        .ok_or_else(|| "expected CLDR object".into())
}
pub fn text(value: &Value) -> Result<&str> {
    value.as_str().ok_or_else(|| "expected CLDR string".into())
}
pub fn string<'a>(value: &'a Value, key: &str) -> Option<&'a str> {
    value.get(key).and_then(Value::as_str)
}

pub fn fields(values: &[Option<&str>]) -> Result<StringFields<'static>> {
    StringFields::from_fields(values).ok_or_else(|| "too many serialized string fields".into())
}

pub fn plurals(value: &Value, prefix: &str) -> Result<PluralPatterns<'static>> {
    let values = PluralCategory::ALL.map(|category| {
        value
            .get(format!("{prefix}{}", category.as_str()))
            .and_then(Value::as_str)
    });
    Ok(PluralPatterns {
        fields: fields(&values)?,
    })
}

pub fn put<M: DataMarker>(
    exporter: &BlobExporter<'_>,
    locale: &str,
    attributes: &str,
    value: M::DataStruct,
) -> Result<()>
where
    ExportMarker: UpcastDataPayload<M>,
{
    let locale: DataLocale = locale.parse()?;
    let attributes = DataMarkerAttributes::try_from_str(attributes)
        .map_err(|error| format!("invalid marker attributes {attributes:?}: {error:?}"))?;
    let id = DataIdentifierBorrowed::for_marker_attributes_and_locale(attributes, &locale);
    let payload =
        <ExportMarker as UpcastDataPayload<M>>::upcast(DataPayload::<M>::from_owned(value));
    exporter.put_payload(M::INFO, id, &payload)?;
    Ok(())
}

fn sha256(path: &Path) -> Result<String> {
    let result = Command::new("python3")
        .arg("-c")
        .arg(
            "import hashlib,sys; print(hashlib.sha256(open(sys.argv[1], 'rb').read()).hexdigest())",
        )
        .arg(path)
        .output()?;
    if !result.status.success() {
        return Err("postcard SHA256 calculation failed".into());
    }
    Ok(String::from_utf8(result.stdout)?.trim().to_owned())
}
