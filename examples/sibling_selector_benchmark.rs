use std::hint::black_box;
use std::time::Instant;

use omoikane::css::{ComputedValue, Origin, StyleResolver, parse_stylesheet};
use omoikane::dom::{Node, NodeHandle, NodeType};
use omoikane::html::TreeBuilder;

const SIBLINGS: usize = 2_000;
const ITERATIONS: usize = 20;

fn main() -> Result<(), String> {
    let mut html = String::from("<!doctype html><html><body><main><span class=anchor></span>");
    for _ in 1..SIBLINGS - 1 {
        html.push_str("<span></span>");
    }
    html.push_str("<span class=target></span></main></body></html>");
    let document = TreeBuilder::parse(&html).document();
    let target = find_by_class(&document, "target").ok_or("target element not found")?;
    let stylesheet = parse_stylesheet(".anchor ~ .target { padding-left: 17px; }")
        .map_err(|error| error.to_string())?;

    let mut samples = Vec::with_capacity(ITERATIONS);
    let mut checksum = 0usize;
    for _ in 0..ITERATIONS {
        let mut resolver = StyleResolver::new();
        resolver.add_stylesheet(Origin::Author, stylesheet.clone());
        let start = Instant::now();
        let style = resolver.computed_style(&target);
        samples.push(start.elapsed().as_secs_f64() * 1_000.0);
        if let Some(ComputedValue::Px(value)) = style.get("padding-left") {
            checksum = checksum.wrapping_add(*value as usize);
        }
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

fn find_by_class(node: &NodeHandle, class: &str) -> Option<NodeHandle> {
    if node.node_type() == NodeType::Element
        && node
            .get_attribute("class")
            .is_some_and(|value| value.split_whitespace().any(|item| item == class))
    {
        return Some(node.clone());
    }
    node.child_nodes()
        .into_iter()
        .find_map(|child| find_by_class(&child, class))
}
