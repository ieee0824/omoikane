//! Emits reproducible builtin availability and runtime initialization samples.
use omoikane::js::JsRuntime;
use serde_json::json;
use std::time::Instant;

fn main() {
    let mut samples = Vec::new();
    let mut last = None;
    for _ in 0..20 {
        let started = Instant::now();
        let runtime = JsRuntime::new().expect("initialize runtime");
        samples.push(started.elapsed().as_micros() as u64);
        last = Some(runtime);
    }
    let mut runtime = last.unwrap();
    let result = runtime.eval(r#"function observe(callback) {
        try { return {status: "ok", value: callback()}; }
        catch (error) { return {status: "error", name: error.name, message: error.message}; }
    }
    JSON.stringify({
        features: {
            Intl: typeof Intl, Segmenter: typeof Intl.Segmenter,
            DurationFormat: typeof Intl.DurationFormat, Temporal: typeof Temporal,
            Iterator: typeof Iterator, iteratorMap: typeof [].values().map,
            FinalizationRegistry: typeof FinalizationRegistry,
            DisposableStack: typeof DisposableStack,
            AsyncDisposableStack: typeof AsyncDisposableStack,
            dispose: typeof Symbol.dispose, asyncDispose: typeof Symbol.asyncDispose,
            errorIsError: typeof Error.isError, regexpEscape: typeof RegExp.escape,
            fromBase64: typeof Uint8Array.fromBase64, toHex: typeof Uint8Array.prototype.toHex,
            arrayFromAsync: typeof Array.fromAsync
        },
        locales: ['en-US','ja-JP','de-DE'].map(locale => ({
            locale,
            currency: observe(() => new Intl.NumberFormat(locale, {style:'currency', currency:'USD'}).format(1234.5)),
            number: observe(() => (1234.5).toLocaleString(locale)),
            date: observe(() => new Date(0).toLocaleDateString(locale, {timeZone:'UTC'}))
        }))
    })"#).expect("probe builtins");
    let output: serde_json::Value =
        serde_json::from_str(&result.as_string().unwrap().to_std_string_escaped()).unwrap();
    println!(
        "{}",
        json!({"initialization_microseconds": samples, "probe": output})
    );
}
