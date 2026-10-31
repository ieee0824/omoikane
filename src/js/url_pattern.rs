//! URLPattern parsing and matching at the native URL boundary.

use super::*;
use serde::Serialize;
use urlpattern::UrlPatternOptions;
use urlpattern::parser::RegexSyntax;
use urlpattern::quirks::{self, EcmaRegexp, StringOrInit};

#[derive(Serialize)]
struct ComponentResult {
    input: String,
    groups: HashMap<String, Option<String>>,
}

#[derive(Serialize)]
struct MatchResult {
    protocol: ComponentResult,
    username: ComponentResult,
    password: ComponentResult,
    hostname: ComponentResult,
    port: ComponentResult,
    pathname: ComponentResult,
    search: ComponentResult,
    hash: ComponentResult,
}

fn pattern_input(encoded: &str) -> JsResult<StringOrInit<'_>> {
    serde_json::from_str(encoded).map_err(|error| {
        JsNativeError::typ()
            .with_message(format!("invalid URLPattern input: {error}"))
            .into()
    })
}

fn optional_string(encoded: &str) -> JsResult<Option<String>> {
    serde_json::from_str(encoded).map_err(|error| {
        JsNativeError::typ()
            .with_message(format!("invalid URLPattern base URL: {error}"))
            .into()
    })
}

fn options(ignore_case: bool) -> UrlPatternOptions {
    UrlPatternOptions {
        regex_syntax: RegexSyntax::EcmaScript,
        ignore_case,
    }
}

fn rust_options(ignore_case: bool) -> UrlPatternOptions {
    UrlPatternOptions {
        regex_syntax: RegexSyntax::Rust,
        ignore_case,
    }
}

fn rust_pattern(
    encoded: &str,
    encoded_base: &str,
    ignore_case: bool,
) -> JsResult<urlpattern::UrlPattern> {
    let input = pattern_input(encoded)?;
    let base = optional_string(encoded_base)?;
    let init = quirks::process_construct_pattern_input(input, base.as_deref())
        .map_err(|error| JsNativeError::typ().with_message(error.to_string()))?;
    urlpattern::UrlPattern::parse(init, rust_options(ignore_case))
        .map_err(|error| JsNativeError::typ().with_message(error.to_string()).into())
}

fn component(result: urlpattern::UrlPatternComponentResult) -> ComponentResult {
    ComponentResult {
        input: result.input,
        groups: result.groups,
    }
}

fn match_result(result: urlpattern::UrlPatternResult) -> MatchResult {
    MatchResult {
        protocol: component(result.protocol),
        username: component(result.username),
        password: component(result.password),
        hostname: component(result.hostname),
        port: component(result.port),
        pathname: component(result.pathname),
        search: component(result.search),
        hash: component(result.hash),
    }
}

/// Parses a pattern or executes it against an already converted match input.
///
/// All Web IDL conversion happens in the bootstrap. This boundary receives
/// owned JSON snapshots, so getters are observed exactly once before parsing.
pub(super) fn native(_: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let operation = string_argument(args.first(), "", context)?;
    let encoded_pattern = string_argument(args.get(1), "{}", context)?;
    let encoded_pattern_base = string_argument(args.get(2), "null", context)?;
    let ignore_case = args.get(3).and_then(JsValue::as_boolean).unwrap_or(false);
    if operation == "construct" {
        let init = pattern_input(&encoded_pattern)?;
        let base = optional_string(&encoded_pattern_base)?;
        let init = quirks::process_construct_pattern_input(init, base.as_deref())
            .map_err(|error| JsNativeError::typ().with_message(error.to_string()))?;
        let parsed = quirks::parse_pattern::<EcmaRegexp>(init, options(ignore_case))
            .map_err(|error| JsNativeError::typ().with_message(error.to_string()))?;
        return serde_json::to_string(&parsed)
            .map(|value| js_string!(value).into())
            .map_err(|error| {
                JsNativeError::error()
                    .with_message(error.to_string())
                    .into()
            });
    }

    if operation == "validate-rust" {
        rust_pattern(&encoded_pattern, &encoded_pattern_base, ignore_case)?;
        return Ok(js_string!("true").into());
    }

    if operation != "match" && operation != "exec-rust" {
        return Err(JsNativeError::typ()
            .with_message("invalid URLPattern operation")
            .into());
    }
    let encoded_input = string_argument(args.get(4), "{}", context)?;
    let encoded_input_base = string_argument(args.get(5), "null", context)?;
    let input = pattern_input(&encoded_input)?;
    let base = optional_string(&encoded_input_base)?;
    let Some((input, _)) = quirks::process_match_input(input, base.as_deref())
        .map_err(|error| JsNativeError::typ().with_message(error.to_string()))?
    else {
        return Ok(js_string!("null").into());
    };
    if operation == "exec-rust" {
        let result = rust_pattern(&encoded_pattern, &encoded_pattern_base, ignore_case)?
            .exec(input)
            .map_err(|error| JsNativeError::typ().with_message(error.to_string()))?
            .map(match_result);
        return serde_json::to_string(&result)
            .map(|value| js_string!(value).into())
            .map_err(|error| {
                JsNativeError::error()
                    .with_message(error.to_string())
                    .into()
            });
    }
    let result = quirks::parse_match_input(input);
    serde_json::to_string(&result)
        .map(|value| js_string!(value).into())
        .map_err(|error| {
            JsNativeError::error()
                .with_message(error.to_string())
                .into()
        })
}
