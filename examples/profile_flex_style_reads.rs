//! Offline warm-cache flex layout timing with deterministic geometry output.
//!
//! Run with `cargo run --locked --example profile_flex_style_reads -- 512`.
//! Use the same build profile, fonts, and machine when comparing revisions.
//! The optional argument sets iterations per sample; nine samples follow five
//! warmups. Output includes all rectangles so comparisons can verify geometry.
//! This synthetic fixture performs no network requests and sets no CI threshold.
use omoikane::css::{Origin, StyleResolver, parse_stylesheet};
use omoikane::dom::NodeHandle;
use omoikane::layout::{Rect, layout_tree};
use std::time::Instant;

fn main() {
    let iterations = std::env::args()
        .nth(1)
        .map(|value| value.parse::<usize>().expect("positive iteration count"))
        .unwrap_or(8);
    assert!(iterations > 0);
    let root = NodeHandle::element("main");
    for _ in 0..64 {
        let item = NodeHandle::element("div");
        for label in ["first", "second"] {
            let span = NodeHandle::element("span");
            span.append_child(NodeHandle::text(label));
            item.append_child(span);
        }
        root.append_child(item);
    }
    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(
        Origin::Author,
        parse_stylesheet(
            "main{display:flex;flex-wrap:wrap;width:800px;gap:4px}\
         div{display:flex;width:90px;height:20px;align-items:center}\
         span{font-size:12px;line-height:16px;color:navy;padding:1px;margin-left:2px}",
        )
        .unwrap(),
    );
    let viewport = Rect {
        x: 0.0,
        y: 0.0,
        width: 800.0,
        height: 600.0,
    };
    let capture = |resolver: &mut StyleResolver| {
        let layout = layout_tree(&root, resolver, viewport).unwrap();
        layout
            .children
            .iter()
            .flat_map(|item| {
                std::iter::once(item.dimensions.content)
                    .chain(item.children.iter().map(|child| child.dimensions.content))
            })
            .collect::<Vec<_>>()
    };
    let expected = capture(&mut resolver);
    for _ in 0..5 {
        assert_eq!(capture(&mut resolver), expected);
    }
    let mut samples = Vec::new();
    for _ in 0..9 {
        let start = Instant::now();
        for _ in 0..iterations {
            assert_eq!(capture(&mut resolver), expected);
        }
        samples.push(start.elapsed().as_nanos() / iterations as u128);
    }
    samples.sort_unstable();
    let checksum: f32 = expected
        .iter()
        .map(|r| r.x + r.y + r.width + r.height)
        .sum();
    let geometry: Vec<_> = expected
        .iter()
        .map(|r| [r.x, r.y, r.width, r.height])
        .collect();
    println!(
        "{{\"median_ns_per_layout\":{},\"rectangles\":{},\"geometry_checksum\":{},\"samples_ns\":{:?},\"geometry\":{:?}}}",
        samples[samples.len() / 2],
        expected.len(),
        checksum,
        samples,
        geometry
    );
}
