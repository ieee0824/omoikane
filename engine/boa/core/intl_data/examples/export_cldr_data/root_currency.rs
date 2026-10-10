use super::{currency, support::*};
use boa_intl_data::{
    NumberPatternField, NumberPatterns, OmoikaneGlobalCurrencyPatternsV1,
    OmoikaneGlobalNumberPatternsV1,
};
use icu_provider_blob::export::BlobExporter;
use serde_json::{Map, Value};

fn children(node: &Value) -> Result<&[Value]> {
    node.get("children")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .ok_or_else(|| "missing root XML children".into())
}

fn child<'a>(node: &'a Value, tag: &str) -> Option<&'a Value> {
    node.get("children")
        .and_then(Value::as_array)?
        .iter()
        .find(|item| string(item, "tag") == Some(tag))
}

fn alias_target<'a>(alias: &'a Value, element: &str, attribute: &str) -> Result<&'a str> {
    if string(&alias["attributes"], "source") != Some("locale") {
        return Err("unsupported root alias source".into());
    }
    let path = text(&alias["attributes"]["path"])?;
    let prefix = format!("../{element}[@{attribute}='");
    path.strip_prefix(&prefix)
        .and_then(|rest| rest.strip_suffix("']"))
        .ok_or_else(|| "unsupported root alias path".into())
}

fn system<'a>(formats: &'a Value, nu: &str, depth: usize) -> Result<&'a Value> {
    if depth > 2 {
        return Err("cyclic root currency alias".into());
    }
    let node = formats
        .get(nu)
        .ok_or("root currency numbering system is missing")?;
    if let Some(alias) = child(node, "alias") {
        return system(
            formats,
            alias_target(alias, "currencyFormats", "numberSystem")?,
            depth + 1,
        );
    }
    Ok(node)
}

fn normal_length(node: &Value) -> Result<&Value> {
    children(node)?
        .iter()
        .find(|item| {
            string(item, "tag") == Some("currencyFormatLength")
                && item["attributes"].get("type").is_none()
        })
        .ok_or_else(|| "root currency standard length is missing".into())
}

fn format_node<'a>(length: &'a Value, kind: &str, depth: usize) -> Result<&'a Value> {
    if depth > 2 {
        return Err("cyclic root currency format alias".into());
    }
    let node = children(length)?
        .iter()
        .find(|item| {
            string(item, "tag") == Some("currencyFormat")
                && string(&item["attributes"], "type") == Some(kind)
        })
        .ok_or("root currency format is missing")?;
    if let Some(alias) = child(node, "alias") {
        return format_node(
            length,
            alias_target(alias, "currencyFormat", "type")?,
            depth + 1,
        );
    }
    Ok(node)
}

fn pattern_map(node: &Value) -> Result<Map<String, Value>> {
    let length = normal_length(node)?;
    let mut result = Map::new();
    for kind in ["standard", "accounting"] {
        for pattern in children(format_node(length, kind, 0)?)? {
            if string(pattern, "tag") != Some("pattern") {
                continue;
            }
            let name = if let Some(alt) = string(&pattern["attributes"], "alt") {
                format!("{kind}-{alt}")
            } else {
                kind.to_owned()
            };
            result.insert(name, serde_json::json!({"raw": text(&pattern["value"])?}));
        }
    }
    Ok(result)
}

fn spacing(node: &Value, latin: &Value) -> Result<Value> {
    let selected = child(node, "currencySpacing").ok_or("root currency spacing is missing")?;
    let selected = if child(selected, "alias").is_some() {
        child(latin, "currencySpacing").ok_or("root Latin spacing is missing")?
    } else {
        selected
    };
    let mut result = Map::new();
    for group in children(selected)? {
        let mut fields = Map::new();
        for field in children(group)? {
            fields.insert(text(&field["tag"])?.to_owned(), field["value"].clone());
        }
        result.insert(text(&group["tag"])?.to_owned(), Value::Object(fields));
    }
    Ok(Value::Object(result))
}

fn units(node: &Value, latin: &Value) -> Result<Value> {
    let mut result = Map::new();
    for selected in [latin, node] {
        for pattern in children(selected)? {
            if string(pattern, "tag") == Some("unitPattern") {
                result.insert(
                    text(&pattern["attributes"]["count"])?.to_owned(),
                    pattern["value"].clone(),
                );
            }
        }
    }
    Ok(Value::Object(result))
}

pub fn export(source: &Source, exporter: &BlobExporter<'_>, counts: &mut Counts) -> Result<()> {
    let root = source.read("root_number_symbols.json")?;
    let formats = &root["currencyFormatsSource"];
    let latin = system(formats, "latn", 0)?;
    let mut count = 0;
    for nu in object(&root["numberingSystemSymbols"])?.keys() {
        let node = system(formats, nu, 0)?;
        let append = child(node, "currencyPatternAppendISO")
            .or_else(|| child(latin, "currencyPatternAppendISO"));
        let record = serde_json::json!({ "patterns": pattern_map(node)?, "currencySpacing": spacing(node, latin)?,
            "unitPatterns": units(node, latin)?, "currencyPatternAppendISO": append.and_then(|value| value.get("value")) });
        put::<OmoikaneGlobalCurrencyPatternsV1>(exporter, "und", nu, currency::patterns(&record)?)?;
        count += 1;
    }
    counts.insert("OmoikaneGlobalCurrencyPatternsV1", count);
    Ok(())
}

fn number_system<'a>(formats: &'a Value, kind: &str, nu: &str, depth: usize) -> Result<&'a Value> {
    if depth > 2 {
        return Err("cyclic root number format alias".into());
    }
    let node = formats.get(nu).ok_or("root number format is missing")?;
    if let Some(alias) = child(node, "alias") {
        return number_system(
            formats,
            kind,
            alias_target(alias, &format!("{kind}Formats"), "numberSystem")?,
            depth + 1,
        );
    }
    Ok(node)
}

fn number_pattern<'a>(formats: &'a Value, kind: &str, nu: &str) -> Result<&'a str> {
    let node = number_system(formats, kind, nu, 0)?;
    let length_tag = format!("{kind}FormatLength");
    let length = children(node)?
        .iter()
        .find(|item| {
            string(item, "tag") == Some(length_tag.as_str())
                && item["attributes"].get("type").is_none()
        })
        .ok_or("root standard number format length is missing")?;
    let format =
        child(length, &format!("{kind}Format")).ok_or("root standard number format is missing")?;
    text(&child(format, "pattern").ok_or("root standard number pattern is missing")?["value"])
}

pub fn export_number_patterns(
    source: &Source,
    exporter: &BlobExporter<'_>,
    counts: &mut Counts,
) -> Result<()> {
    let root = source.read("root_number_symbols.json")?;
    let mut count = 0;
    for nu in object(&root["numberingSystemSymbols"])?.keys() {
        let values = NumberPatternField::ALL.map(|field| {
            number_pattern(
                &root["numberFormatsSource"][field.as_str()],
                field.as_str(),
                nu,
            )
        });
        let values = values.into_iter().collect::<Result<Vec<_>>>()?;
        let values = values.into_iter().map(Some).collect::<Vec<_>>();
        put::<OmoikaneGlobalNumberPatternsV1>(
            exporter,
            "und",
            nu,
            NumberPatterns {
                fields: fields(&values)?,
            },
        )?;
        count += 1;
    }
    counts.insert("OmoikaneGlobalNumberPatternsV1", count);
    Ok(())
}
