//! Runs the pinned text-shadow reftests and retains every outcome and PNG.
//! Usage: cargo run --locked --example text_shadow_wpt -- WPT_ROOT OUTPUT_DIR

use std::path::{Path, PathBuf};

use omoikane::dom::Node;
use omoikane::html::TreeBuilder;
use omoikane::layout::Rect;
use omoikane::paint::{Canvas, Color, Image, diff_canvases, render_document_with_base_path};

fn render(path: &Path) -> Result<Canvas, Box<dyn std::error::Error>> {
    let source = std::fs::read_to_string(path)?;
    let document = TreeBuilder::parse(&source).document();
    let rendered = render_document_with_base_path(
        &document,
        Rect {
            width: 800.0,
            height: 600.0,
            ..Rect::default()
        },
        path.parent().ok_or("case has no parent directory")?,
    )
    .map_err(|error| format!("{error:?}"))?;
    // Reftests compare the browser's white page, including transparent surfaces.
    let image =
        Image::new(800, 600, rendered.into_pixels()).map_err(|error| format!("{error:?}"))?;
    let mut canvas = Canvas::new(800, 600);
    canvas.fill_rect(
        Rect {
            width: 800.0,
            height: 600.0,
            ..Rect::default()
        },
        Color::rgb(255, 255, 255),
    );
    canvas.draw_image(&image, 0.0, 0.0);
    Ok(canvas)
}

fn ink_pixels(canvas: &Canvas) -> usize {
    canvas
        .pixels()
        .chunks_exact(4)
        .filter(|p| **p != [255, 255, 255, 255])
        .count()
}

fn fuzzy_limits(document: &omoikane::dom::NodeHandle) -> Option<((usize, usize), (usize, usize))> {
    let content = document
        .query_selector("meta[name=fuzzy]")?
        .get_attribute("content")?;
    let (delta, pixels) = content.split_once(';')?;
    let range = |text: &str| {
        let (a, b) = text.trim().split_once('-')?;
        Some((a.parse().ok()?, b.parse().ok()?))
    };
    Some((range(delta)?, range(pixels)?))
}

fn compare(actual: &Canvas, expected: &Canvas) -> (usize, u8) {
    let mut changed = 0;
    let mut max_delta = 0;
    for (a, b) in actual
        .pixels()
        .chunks_exact(4)
        .zip(expected.pixels().chunks_exact(4))
    {
        if a != b {
            changed += 1;
        }
        for (a, b) in a.iter().zip(b) {
            max_delta = max_delta.max(a.abs_diff(*b));
        }
    }
    (changed, max_delta)
}

fn run_case(
    path: &Path,
    output: &Path,
) -> Result<Vec<serde_json::Value>, Box<dyn std::error::Error>> {
    let source = std::fs::read_to_string(path)?;
    let document = TreeBuilder::parse(&source).document();
    let fuzzy = fuzzy_limits(&document);
    let mut nodes = vec![document];
    let mut links = Vec::new();
    while let Some(node) = nodes.pop() {
        nodes.extend(node.child_nodes().into_iter().rev());
        if node.has_tag_name("link") {
            links.push(node);
        }
    }
    let references = links
        .into_iter()
        .filter_map(|link| {
            let relation = link.get_attribute("rel")?;
            if relation != "match" && relation != "mismatch" {
                return None;
            }
            Some((relation, link.get_attribute("href")?))
        })
        .collect::<Vec<_>>();
    if references.is_empty() {
        return Ok(Vec::new());
    }
    let name = path
        .file_stem()
        .and_then(|name| name.to_str())
        .ok_or("invalid case name")?;
    let actual = render(path)?;
    std::fs::write(
        output.join(format!("{name}.actual.png")),
        actual.encode_png(),
    )?;
    let mut results = Vec::new();
    for (index, (relation, reference)) in references.iter().enumerate() {
        let expected = render(&path.parent().unwrap().join(reference))?;
        let (changed, max_delta) = compare(&actual, &expected);
        let passed = if relation == "match" {
            changed == 0
                || fuzzy.is_some_and(|((d0, d1), (p0, p1))| {
                    (d0..=d1).contains(&(max_delta as usize)) && (p0..=p1).contains(&changed)
                })
        } else {
            changed > 0
        };
        let (diff, _) = diff_canvases(&actual, &expected);
        std::fs::write(
            output.join(format!("{name}.{index}.expected.png")),
            expected.encode_png(),
        )?;
        std::fs::write(
            output.join(format!("{name}.{index}.diff.png")),
            diff.encode_png(),
        )?;
        let actual_ink = ink_pixels(&actual);
        let expected_ink = ink_pixels(&expected);
        let empty_svg = actual_ink == 0 && expected_ink == 0 && name.starts_with("svg-");
        results.push(serde_json::json!({
            "path": path.file_name().unwrap().to_string_lossy(), "reference": reference,
            "relation": relation, "status": if empty_svg {"UNVERIFIED"} else if passed {"PASS"} else {"FAIL"},
            "changed_pixels": changed, "maximum_channel_delta": max_delta,
            "comparison": "white page RGBA; only upstream-declared fuzzy limits applied",
            "upstream_fuzzy": fuzzy, "actual_ink_pixels":actual_ink, "expected_ink_pixels":expected_ink,
        }));
    }
    Ok(results)
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args_os().skip(1);
    let root = PathBuf::from(args.next().ok_or("missing WPT_ROOT")?);
    let output = PathBuf::from(args.next().ok_or("missing OUTPUT_DIR")?);
    let revision = std::fs::read_to_string("tests/wpt/revision.txt")?;
    let actual_revision = std::process::Command::new("git")
        .arg("-C")
        .arg(&root)
        .args(["rev-parse", "HEAD"])
        .output()?;
    if !actual_revision.status.success()
        || String::from_utf8(actual_revision.stdout)?.trim() != revision.trim()
    {
        return Err("WPT checkout is not the pinned revision".into());
    }
    std::fs::create_dir_all(&output)?;
    let mut paths = std::fs::read_dir(root.join("css/css-text-decor/text-shadow"))?
        .collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "html"))
        .collect::<Vec<_>>();
    paths.sort();
    let mut results = Vec::new();
    for path in paths {
        match run_case(&path, &output) {
            Ok(records) => results.extend(records),
            Err(error) => results.push(serde_json::json!({"path":path.file_name().unwrap().to_string_lossy(), "status":"ERROR", "error":error.to_string()})),
        }
    }
    let pass = results
        .iter()
        .filter(|result| result["status"] == "PASS")
        .count();
    let report = serde_json::json!({"revision":revision.trim(), "viewport":[800,600], "pass":pass, "total":results.len(), "results":results});
    std::fs::write(
        output.join("report.json"),
        serde_json::to_vec_pretty(&report)?,
    )?;
    println!(
        "text-shadow reftests: {pass}/{} comparisons PASS; report={}",
        report["total"],
        output.join("report.json").display()
    );
    if pass != report["total"].as_u64().unwrap() as usize {
        std::process::exit(1);
    }
    Ok(())
}
