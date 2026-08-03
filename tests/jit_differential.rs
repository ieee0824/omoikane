//! Gate 2-1 differential boundary for issue #522.
//!
//! The feature is deliberately test-only. The two backends run the same
//! source and expose the same value/error boundary, but Omoikane keeps its
//! existing Boa path as the only production engine. This gives later Gate 2
//! work a place to compare an interpreter fallback or a future JIT without
//! adding a second heap or changing the runtime default.

#[cfg(feature = "jit-differential")]
mod differential {
    use boa_engine::{Context, Source};
    use omoikane::js::JsRuntime;

    #[derive(Debug, PartialEq)]
    enum Outcome {
        Value(serde_json::Value),
        Error { kind: String, marker: String },
    }

    trait DifferentialBackend {
        fn name(&self) -> &'static str;
        fn eval_serialized(&mut self, source: &str) -> Result<String, String>;
    }

    struct StandaloneBoa {
        context: Context,
    }

    impl StandaloneBoa {
        fn new() -> Self {
            Self {
                context: Context::default(),
            }
        }
    }

    impl DifferentialBackend for StandaloneBoa {
        fn name(&self) -> &'static str {
            "boa-interpreter"
        }

        fn eval_serialized(&mut self, source: &str) -> Result<String, String> {
            let value = self
                .context
                .eval(Source::from_bytes(source))
                .map_err(|error| error.to_string())?;
            value
                .as_string()
                .map(|value| value.to_std_string_escaped())
                .ok_or_else(|| format!("{} returned a non-string result: {value:?}", self.name()))
        }
    }

    struct OmoikaneBoa {
        runtime: JsRuntime,
    }

    impl OmoikaneBoa {
        fn new() -> Self {
            Self {
                runtime: JsRuntime::new().expect("Omoikane Boa runtime should build"),
            }
        }
    }

    impl DifferentialBackend for OmoikaneBoa {
        fn name(&self) -> &'static str {
            "omoikane-boa"
        }

        fn eval_serialized(&mut self, source: &str) -> Result<String, String> {
            let value = self
                .runtime
                .eval(source)
                .map_err(|error| error.to_string())?;
            value
                .as_string()
                .map(|value| value.to_std_string_escaped())
                .ok_or_else(|| format!("{} returned a non-string result: {value:?}", self.name()))
        }
    }

    fn normalize_error(error: &str) -> Outcome {
        let kind = [
            "AggregateError",
            "EvalError",
            "RangeError",
            "ReferenceError",
            "SyntaxError",
            "TypeError",
            "URIError",
            "Error",
        ]
        .iter()
        .find(|kind| error.contains(**kind))
        .copied()
        .unwrap_or("UnknownError")
        .to_owned();
        let marker = if error.contains("gate2-error") {
            "gate2-error"
        } else {
            "unclassified"
        };
        Outcome::Error {
            kind,
            marker: marker.to_owned(),
        }
    }

    fn evaluate<B: DifferentialBackend>(backend: &mut B, source: &str) -> Outcome {
        match backend.eval_serialized(source) {
            Ok(serialized) => {
                Outcome::Value(serde_json::from_str(&serialized).unwrap_or_else(|error| {
                    panic!(
                        "{} returned invalid JSON {serialized:?}: {error}",
                        backend.name()
                    )
                }))
            }
            Err(error) => normalize_error(&error),
        }
    }

    fn compare_case(name: &str, source: &str) {
        let mut standalone = StandaloneBoa::new();
        let mut omoikane = OmoikaneBoa::new();
        let expected = evaluate(&mut standalone, source);
        let actual = evaluate(&mut omoikane, source);
        assert_eq!(
            actual, expected,
            "differential mismatch in {name}: standalone={expected:?}, omoikane={actual:?}"
        );
    }

    #[cfg(feature = "jit-differential")]
    #[test]
    fn same_programs_match_across_the_interpreter_boundary() {
        let programs = [
            (
                "arith-loop",
                "JSON.stringify((() => { let sum = 0; for (let i = 0; i < 100; i++) sum += i; return { sum }; })())",
            ),
            (
                "nested-call",
                "JSON.stringify((() => { function add(a, b) { return a + b; } function twice(value) { return add(value, value); } return { value: twice(21) }; })())",
            ),
            (
                "closure-capture",
                "JSON.stringify((() => { function makeCounter(start) { let value = start; return () => ++value; } const next = makeCounter(40); return { first: next(), second: next() }; })())",
            ),
            (
                "control-flow",
                "JSON.stringify((() => { let value = 0; for (let i = 0; i < 10; i++) { if (i % 2 === 0) continue; value += i; } return { value }; })())",
            ),
            (
                "side-effect",
                "JSON.stringify((() => { const events = []; const target = { value: 1 }; function bump(object) { object.value += 4; events.push(object.value); } bump(target); delete target.value; target.value = 9; return { value: target.value, events }; })())",
            ),
            (
                "closure-allocation",
                "JSON.stringify((() => { let sum = 0; for (let i = 0; i < 256; i++) { const add = value => value + i; sum = (sum + add(i)) % 1000003; } return { sum }; })())",
            ),
            (
                "object-allocation",
                "JSON.stringify((() => { let sum = 0; for (let i = 0; i < 256; i++) { const object = { x: i, y: i + 1 }; sum = (sum + object.x + object.y) % 1000003; } return { sum }; })())",
            ),
            (
                "array-allocation",
                "JSON.stringify((() => { const values = []; let sum = 0; for (let i = 0; i < 256; i++) { if (values.length >= 32) values.length = 0; values.push(i); sum += values[i & 31]; } return { length: values.length, sum }; })())",
            ),
            (
                "string-concat",
                "JSON.stringify((() => { let value = ''; for (let i = 0; i < 256; i++) { value += 'ab'; if (value.length > 64) value = ''; } return { length: value.length, suffix: value.slice(-4) }; })())",
            ),
            (
                "primitive-string",
                "JSON.stringify((() => { let property = 0; let method = 0; for (let i = 0; i < 256; i++) { property += 'abc'.length; method += 'abc'.charCodeAt(i & 2); } return { property, method }; })())",
            ),
            (
                "exception-propagation",
                "JSON.stringify((() => { try { (() => { throw new RangeError('gate2-error'); })(); return { caught: false }; } catch (error) { return { caught: true, name: error.name, message: error.message }; } })())",
            ),
        ];

        for (name, source) in programs {
            compare_case(name, source);
        }
    }

    #[cfg(feature = "jit-differential")]
    #[test]
    fn uncaught_errors_cross_the_same_host_boundary() {
        let source = "throw new TypeError('gate2-error')";
        let mut standalone = StandaloneBoa::new();
        let mut omoikane = OmoikaneBoa::new();

        assert_eq!(
            evaluate(&mut standalone, source),
            evaluate(&mut omoikane, source),
            "uncaught exception classification must remain stable"
        );
    }

    #[test]
    fn allocation_pressure_survives_forced_collection() {
        const ALLOCATION_PROBE: &str = r#"
            (() => {
              let checksum = 0;
              for (let round = 0; round < 64; round++) {
                const objects = [];
                for (let i = 0; i < 128; i++) {
                  const object = { value: i, text: 'ab' + i };
                  objects.push(() => object.value + object.text.length);
                }
                checksum = (checksum + objects[round & 127]()) % 1000003;
              }
              return checksum;
            })()
            "#;
        let mut runtime = JsRuntime::new().expect("Omoikane runtime should build");
        let mut last = None;
        for _ in 0..4 {
            last = Some(
                runtime
                    .eval(ALLOCATION_PROBE)
                    .expect("allocation probe should finish within the bounded workload")
                    .as_number()
                    .expect("allocation probe should return a number"),
            );
            boa_gc::force_collect();
        }
        assert_eq!(last, Some(2_262.0));
    }
}

#[cfg(not(feature = "jit-differential"))]
#[test]
fn feature_off_keeps_the_existing_omoikane_engine_default() {
    use omoikane::js::JsRuntime;

    let mut runtime = JsRuntime::new().expect("Omoikane runtime should build");
    let value = runtime
        .eval("1 + 2")
        .expect("default runtime should evaluate a basic program");
    assert_eq!(value.as_number(), Some(3.0));
}
