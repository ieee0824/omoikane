use std::hint::black_box;
use std::time::Instant;

use omoikane::css::{ComputedValue, Origin, StyleResolver, parse_stylesheet};
use omoikane::dom::{Node, NodeHandle, NodeType};
use omoikane::html::TreeBuilder;

const SIBLINGS: usize = 2_000;
const ITERATIONS: usize = 10;

fn main() -> Result<(), String> {
    let mut html = String::from("<!doctype html><html><body><main>");
    for index in 0..SIBLINGS {
        html.push_str(&format!("<span data-index=\"{index}\"></span>"));
    }
    html.push_str("</main></body></html>");
    let document = TreeBuilder::parse(&html).document();
    let mut spans = Vec::with_capacity(SIBLINGS);
    collect_spans(&document, &mut spans);
    let stylesheet = parse_stylesheet("span:nth-child(odd) { padding-left: 13px; }")
        .map_err(|error| error.to_string())?;

    let mut samples = Vec::with_capacity(ITERATIONS);
    let mut checksum = 0usize;
    for _ in 0..ITERATIONS {
        let mut resolver = StyleResolver::new();
        resolver.add_stylesheet(Origin::Author, stylesheet.clone());
        let start = Instant::now();
        for span in &spans {
            let style = resolver.computed_style(span);
            if let Some(ComputedValue::Px(value)) = style.get("padding-left") {
                checksum = checksum.wrapping_add(*value as usize);
            }
        }
        samples.push(start.elapsed().as_secs_f64() * 1_000.0);
    }
    black_box(checksum);
    samples.sort_by(f64::total_cmp);
    let median = (samples[ITERATIONS / 2 - 1] + samples[ITERATIONS / 2]) / 2.0;
    println!(
        "{{\n  \"siblings\": {SIBLINGS},\n  \"iterations\": {ITERATIONS},\n  \"min_ms\": {},\n  \"median_ms\": {median},\n  \"checksum\": {checksum}\n}}",
        samples[0]
    );
    Ok(())
}

fn collect_spans(node: &NodeHandle, output: &mut Vec<NodeHandle>) {
    if node.node_type() == NodeType::Element && node.tag_name().as_deref() == Some("span") {
        output.push(node.clone());
    }
    for child in node.child_nodes() {
        collect_spans(&child, output);
    }
}
