use std::hint::black_box;
use std::time::Instant;

use omoikane::css::{ComputedValue, Origin, StyleResolver, parse_stylesheet};
use omoikane::dom::{Node, NodeHandle, NodeType};
use omoikane::html::TreeBuilder;

const ELEMENTS: usize = 1_000;
const RULES: usize = 1_000;
const ITERATIONS: usize = 10;

fn main() -> Result<(), String> {
    let mut css = String::new();
    for index in 0..RULES {
        css.push_str(&format!(
            ".rule-{index} {{ color: rgb({}, {}, {}); padding-left: {}px; }}\n",
            index % 255,
            (index * 3) % 255,
            (index * 7) % 255,
            index % 20
        ));
    }
    let stylesheet = parse_stylesheet(&css).map_err(|error| error.to_string())?;

    let mut html = String::from("<!doctype html><html><body><main>");
    for index in 0..ELEMENTS {
        html.push_str(&format!(
            "<div class=\"rule-{index}\" data-index=\"{index}\">item {index}</div>"
        ));
    }
    html.push_str("</main></body></html>");
    let document = TreeBuilder::parse(&html).document();
    let mut elements = Vec::with_capacity(ELEMENTS + 3);
    collect_elements(&document, &mut elements);

    let mut samples = Vec::with_capacity(ITERATIONS);
    let mut checksum = 0usize;
    for _ in 0..ITERATIONS {
        let mut resolver = StyleResolver::new();
        resolver.add_stylesheet(Origin::Author, stylesheet.clone());
        let start = Instant::now();
        for element in &elements {
            let style = resolver.computed_style(element);
            if let Some(ComputedValue::Px(value)) = style.get("padding-left") {
                checksum = checksum.wrapping_add(*value as usize);
            }
        }
        samples.push(start.elapsed().as_secs_f64() * 1_000.0);
    }
    black_box(checksum);
    samples.sort_by(f64::total_cmp);
    let median = (samples[ITERATIONS / 2 - 1] + samples[ITERATIONS / 2]) / 2.0;
    let mean = samples.iter().sum::<f64>() / samples.len() as f64;
    println!(
        "{{\n  \"elements\": {ELEMENTS},\n  \"rules\": {RULES},\n  \"iterations\": {ITERATIONS},\n  \"min_ms\": {},\n  \"median_ms\": {median},\n  \"mean_ms\": {mean},\n  \"checksum\": {checksum}\n}}",
        samples[0]
    );
    Ok(())
}

fn collect_elements(node: &NodeHandle, output: &mut Vec<NodeHandle>) {
    if node.node_type() == NodeType::Element {
        output.push(node.clone());
    }
    for child in node.child_nodes() {
        collect_elements(&child, output);
    }
}
