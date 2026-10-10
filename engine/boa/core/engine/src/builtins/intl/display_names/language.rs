//! Compound language names follow the locale's CLDR composition patterns.
use super::DisplayNames;

/// ICU4X only stores two- and three-letter languages. Validate a longer
/// language's remaining subtags with an unknown-language placeholder instead.
pub(super) fn parse_locale(code: &str) -> Option<icu_locale::Locale> {
    let language = code.split('-').next()?;
    if (5..=8).contains(&language.len()) && language.bytes().all(|byte| byte.is_ascii_alphabetic())
    {
        format!("und{}", &code[language.len()..]).parse().ok()
    } else {
        code.parse().ok()
    }
}

impl DisplayNames {
    pub(super) fn language(&self, code: &str) -> Option<String> {
        let locale = parse_locale(code)?.id;
        let language = code.split('-').next()?.to_owned();
        let script = locale.script.map(|value| value.to_string());
        let region = locale.region.map(|value| value.to_string());
        let mut consumed_script = false;
        let mut consumed_region = false;
        let dialect = if self.language_display == "dialect" {
            self.dialect(
                &language,
                script.as_deref(),
                region.as_deref(),
                &mut consumed_script,
                &mut consumed_region,
            )
        } else {
            None
        };
        let base = dialect.or_else(|| self.component("language", &language))?;
        let mut qualifiers = Vec::new();
        if !consumed_script {
            if let Some(script) = script {
                qualifiers.push(self.component("scriptContext", &script)?);
            }
        }
        if !consumed_region {
            if let Some(region) = region {
                qualifiers.push(self.component("region", &region)?);
            }
        }
        for variant in locale.variants.iter() {
            qualifiers.push(self.component("variant", &variant.to_string().to_ascii_uppercase())?);
        }
        if qualifiers.is_empty() {
            return Some(base);
        }
        let separator = self.names.get().records.get("pattern/localeSeparator")?;
        let qualifiers = qualifiers
            .into_iter()
            .reduce(|left, right| compose(separator, &left, &right))?;
        let pattern = self.names.get().records.get("pattern/localePattern")?;
        Some(compose(pattern, &base, &qualifiers))
    }

    fn dialect(
        &self,
        language: &str,
        script: Option<&str>,
        region: Option<&str>,
        used_script: &mut bool,
        used_region: &mut bool,
    ) -> Option<String> {
        if let (Some(script), Some(region)) = (script, region) {
            if let Some(value) = self.label("language", &format!("{language}-{script}-{region}")) {
                *used_script = true;
                *used_region = true;
                return Some(value.to_owned());
            }
        }
        if let Some(script) = script {
            if let Some(value) = self.label("language", &format!("{language}-{script}")) {
                *used_script = true;
                return Some(value.to_owned());
            }
        }
        if let Some(region) = region {
            if let Some(value) = self.label("language", &format!("{language}-{region}")) {
                *used_region = true;
                return Some(value.to_owned());
            }
        }
        None
    }

    fn component(&self, kind: &str, code: &str) -> Option<String> {
        self.label(kind, code)
            .map(str::to_owned)
            .or_else(|| (self.fallback == "code").then(|| code.to_owned()))
    }
}

fn compose(pattern: &str, first: &str, second: &str) -> String {
    // Split first: replacement text is data, never parsed as another placeholder.
    let mut result = String::new();
    let mut remaining = pattern;
    while let Some(index) = remaining.find('{') {
        result.push_str(&remaining[..index]);
        remaining = &remaining[index..];
        if let Some(tail) = remaining.strip_prefix("{0}") {
            result.push_str(first);
            remaining = tail;
        } else if let Some(tail) = remaining.strip_prefix("{1}") {
            result.push_str(second);
            remaining = tail;
        } else {
            result.push('{');
            remaining = &remaining[1..];
        }
    }
    result.push_str(remaining);
    result
}
