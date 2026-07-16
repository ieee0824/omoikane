use std::env;
use std::fs;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use omoikane::css::{Origin, StyleResolver, parse_stylesheet};
use omoikane::dom::{Node, NodeHandle, NodeType};
use omoikane::html::TreeBuilder;
use omoikane::layout::{Rect, layout_tree};
use omoikane::paint::paint_layout;
use serde::Serialize;
use sha1::{Digest, Sha1};

const DEFAULT_FIXTURE: &str = "tests/fixtures/anonymized-render-benchmark/page.html";
const DEFAULT_ITERATIONS: usize = 10;
const DEFAULT_WARMUP: usize = 3;
const VIEWPORT_WIDTH: f32 = 1280.0;
const VIEWPORT_HEIGHT: f32 = 720.0;

#[derive(Clone, Copy, Default)]
struct Sample {
    html_parse: Duration,
    style: Duration,
    layout: Duration,
    paint: Duration,
    png_encode: Duration,
    total: Duration,
}

#[derive(Serialize)]
struct Statistics {
    min_ms: f64,
    median_ms: f64,
    mean_ms: f64,
    p95_ms: f64,
}

#[derive(Serialize)]
struct ModeReport {
    iterations: usize,
    html_parse: Statistics,
    style: Statistics,
    layout: Statistics,
    paint: Statistics,
    png_encode: Statistics,
    total: Statistics,
    png_bytes: usize,
    png_sha1: String,
}

#[derive(Serialize)]
struct BenchmarkReport {
    fixture: String,
    viewport: [u32; 2],
    warmup_iterations: usize,
    cold: ModeReport,
    warm_dom: ModeReport,
}

fn main() -> Result<(), String> {
    let options = Options::parse(env::args().skip(1))?;
    let html = fs::read_to_string(&options.fixture)
        .map_err(|error| format!("failed to read {}: {error}", options.fixture.display()))?;
    let viewport = Rect {
        x: 0.0,
        y: 0.0,
        width: VIEWPORT_WIDTH,
        height: VIEWPORT_HEIGHT,
    };

    for _ in 0..options.warmup {
        let document = TreeBuilder::parse(&html).document();
        let _ = render_pipeline(&document, viewport)?;
    }

    let mut cold_samples = Vec::with_capacity(options.iterations);
    let mut cold_outputs = Vec::with_capacity(options.iterations);
    for _ in 0..options.iterations {
        let total_start = Instant::now();
        let parse_start = Instant::now();
        let document = TreeBuilder::parse(&html).document();
        let html_parse = parse_start.elapsed();
        let (mut sample, png) = render_pipeline(&document, viewport)?;
        sample.html_parse = html_parse;
        sample.total = total_start.elapsed();
        cold_samples.push(sample);
        cold_outputs.push(png);
    }

    let warm_document = TreeBuilder::parse(&html).document();
    let mut warm_samples = Vec::with_capacity(options.iterations);
    let mut warm_outputs = Vec::with_capacity(options.iterations);
    for _ in 0..options.iterations {
        let total_start = Instant::now();
        let (mut sample, png) = render_pipeline(&warm_document, viewport)?;
        sample.total = total_start.elapsed();
        warm_samples.push(sample);
        warm_outputs.push(png);
    }

    let cold = report_mode(&cold_samples, &cold_outputs)?;
    let warm_dom = report_mode(&warm_samples, &warm_outputs)?;
    if cold.png_sha1 != warm_dom.png_sha1 {
        return Err("cold and warm DOM renders produced different PNG output".to_string());
    }

    let report = BenchmarkReport {
        fixture: options.fixture.display().to_string(),
        viewport: [VIEWPORT_WIDTH as u32, VIEWPORT_HEIGHT as u32],
        warmup_iterations: options.warmup,
        cold,
        warm_dom,
    };
    println!(
        "{}",
        serde_json::to_string_pretty(&report)
            .map_err(|error| format!("failed to encode report: {error}"))?
    );
    Ok(())
}

fn render_pipeline(document: &NodeHandle, viewport: Rect) -> Result<(Sample, Vec<u8>), String> {
    let mut sample = Sample::default();

    let style_start = Instant::now();
    let mut resolver = StyleResolver::new();
    resolver.set_viewport(viewport.width, viewport.height);
    for css in inline_stylesheets(document) {
        let sheet = parse_stylesheet(&css)
            .map_err(|error| format!("failed to parse benchmark stylesheet: {error}"))?;
        resolver.add_stylesheet(Origin::Author, sheet);
    }
    sample.style = style_start.elapsed();

    let layout_start = Instant::now();
    let layout = layout_tree(document, &mut resolver, viewport)
        .ok_or_else(|| "benchmark layout produced no root box".to_string())?;
    sample.layout = layout_start.elapsed();

    let paint_start = Instant::now();
    let canvas = paint_layout(&layout, &mut resolver, viewport);
    sample.paint = paint_start.elapsed();

    let encode_start = Instant::now();
    let png = canvas.encode_png();
    sample.png_encode = encode_start.elapsed();

    Ok((sample, png))
}

fn inline_stylesheets(node: &NodeHandle) -> Vec<String> {
    let mut stylesheets = Vec::new();
    collect_inline_stylesheets(node, &mut stylesheets);
    stylesheets
}

fn collect_inline_stylesheets(node: &NodeHandle, output: &mut Vec<String>) {
    if node.tag_name().as_deref() == Some("style") {
        let css = node
            .child_nodes()
            .into_iter()
            .filter(|child| child.node_type() == NodeType::Text)
            .filter_map(|child| child.data())
            .collect::<String>();
        output.push(css);
        return;
    }
    for child in node.child_nodes() {
        collect_inline_stylesheets(&child, output);
    }
}

fn report_mode(samples: &[Sample], outputs: &[Vec<u8>]) -> Result<ModeReport, String> {
    let first = outputs
        .first()
        .ok_or_else(|| "at least one benchmark iteration is required".to_string())?;
    let digest = sha1_hex(first);
    if outputs.iter().any(|output| sha1_hex(output) != digest) {
        return Err("benchmark iterations produced different PNG output".to_string());
    }
    Ok(ModeReport {
        iterations: samples.len(),
        html_parse: statistics(samples.iter().map(|sample| sample.html_parse)),
        style: statistics(samples.iter().map(|sample| sample.style)),
        layout: statistics(samples.iter().map(|sample| sample.layout)),
        paint: statistics(samples.iter().map(|sample| sample.paint)),
        png_encode: statistics(samples.iter().map(|sample| sample.png_encode)),
        total: statistics(samples.iter().map(|sample| sample.total)),
        png_bytes: first.len(),
        png_sha1: digest,
    })
}

fn statistics(durations: impl Iterator<Item = Duration>) -> Statistics {
    let mut values: Vec<f64> = durations.map(|duration| duration.as_secs_f64() * 1000.0).collect();
    values.sort_by(f64::total_cmp);
    let mean = values.iter().sum::<f64>() / values.len() as f64;
    let median = if values.len().is_multiple_of(2) {
        (values[values.len() / 2 - 1] + values[values.len() / 2]) / 2.0
    } else {
        values[values.len() / 2]
    };
    let p95_index = ((values.len() as f64 * 0.95).ceil() as usize).saturating_sub(1);
    Statistics {
        min_ms: values[0],
        median_ms: median,
        mean_ms: mean,
        p95_ms: values[p95_index],
    }
}

fn sha1_hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha1::digest(bytes))
}

struct Options {
    fixture: PathBuf,
    iterations: usize,
    warmup: usize,
}

impl Options {
    fn parse(mut args: impl Iterator<Item = String>) -> Result<Self, String> {
        let mut fixture = PathBuf::from(DEFAULT_FIXTURE);
        let mut iterations = DEFAULT_ITERATIONS;
        let mut warmup = DEFAULT_WARMUP;
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--fixture" => {
                    fixture = PathBuf::from(args.next().ok_or("--fixture requires a path")?);
                }
                "--iterations" => {
                    iterations = parse_positive(args.next(), "--iterations")?;
                }
                "--warmup" => {
                    warmup = parse_non_negative(args.next(), "--warmup")?;
                }
                "--help" | "-h" => return Err(usage()),
                _ => return Err(format!("unknown argument: {arg}\n{}", usage())),
            }
        }
        Ok(Self { fixture, iterations, warmup })
    }
}

fn parse_positive(raw: Option<String>, flag: &str) -> Result<usize, String> {
    let value = parse_non_negative(raw, flag)?;
    if value == 0 {
        return Err(format!("{flag} must be greater than zero"));
    }
    Ok(value)
}

fn parse_non_negative(raw: Option<String>, flag: &str) -> Result<usize, String> {
    raw.ok_or_else(|| format!("{flag} requires a number"))?
        .parse::<usize>()
        .map_err(|error| format!("invalid {flag}: {error}"))
}

fn usage() -> String {
    "usage: cargo run --release --example render_benchmark -- [--fixture PATH] [--iterations N] [--warmup N]".to_string()
}
