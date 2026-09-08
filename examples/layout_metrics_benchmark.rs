//! Measures repeated CSSOM geometry reads on one element in a stable document.
//!
//! Run with `cargo run --example layout_metrics_benchmark -- 500 1000`.
//! The JSON report counts full native geometry acquisitions. Lightweight epoch
//! checks are part of the elapsed time but are not counted as acquisitions.

use omoikane::html::TreeBuilder;
use omoikane::js::{JsRuntime, SandboxConfig};
use std::time::Duration;

fn main() {
    let count: usize = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "500".into())
        .parse()
        .unwrap();
    let iterations: usize = std::env::args()
        .nth(2)
        .unwrap_or_else(|| "1000".into())
        .parse()
        .unwrap();
    let document = TreeBuilder::parse(&format!("<html><head><style>div {{ width: 210px; height: 40px; padding: 4px; border: 2px solid black; }}</style></head><body>{}</body></html>", "<div>x</div>".repeat(count))).document();
    let mut runtime = JsRuntime::with_document_and_sandbox(
        document,
        SandboxConfig {
            timeout: Duration::from_secs(60),
            max_loop_iterations: u64::MAX,
        },
    )
    .unwrap();
    runtime.eval("globalThis.target = document.body.lastElementChild; document.body.offsetWidth; const nativeMetrics = __omoikane_layout_metrics; globalThis.metricCalls = 0; globalThis.__omoikane_layout_metrics = id => { metricCalls++; return nativeMetrics(id); };").unwrap();
    let result = runtime.eval(&format!(r#"(() => {{
        let sum = 0;
        const start = performance.now();
        for (let i = 0; i < {iterations}; i++) {{
            sum += target.offsetWidth + target.offsetHeight + target.clientWidth + target.scrollHeight + target.getBoundingClientRect().width;
        }}
        return JSON.stringify({{elements: {count}, iterations: {iterations}, elapsedMs: performance.now() - start, nativeMetricsCalls: metricCalls, sum}});
    }})()"#)).unwrap().as_string().unwrap().to_std_string_escaped();
    println!("{result}");
}
