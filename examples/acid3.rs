//! Acid3 baseline runner.
//!
//! Boots a local fixture HTTP server (serving `tests/fixtures/acid3` with the
//! exact status codes / Content-Types acid3.acidtests.org uses), loads the
//! Acid3 page through the Omoikane engine, drives the test loop, and prints the
//! resulting score and per-test failure log.
//!
//! Usage:
//!     cargo run --example acid3
//!     cargo run --example acid3 -- --faithful     # only faithful load emulation
//!     cargo run --example acid3 -- --direct        # only direct-drive baseline
//!
//! This runner does NOT modify the engine; it only exercises existing public
//! APIs (`omoikane::html`, `omoikane::http`, `omoikane::js`) to capture an
//! honest baseline. The shared server + driver live in
//! `tests/acid3_common/harness.rs`, reused verbatim by the integration test.

#[path = "../tests/acid3_common/harness.rs"]
mod harness;

use harness::{Acid3Run, DriveMode, FixtureServer, run_acid3};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let run_faithful = args.is_empty() || args.iter().any(|a| a == "--faithful" || a == "--all");
    let run_direct = args.is_empty() || args.iter().any(|a| a == "--direct" || a == "--all");

    let server = FixtureServer::start();
    println!("Acid3 fixture server: {}", server.acid3_url());
    println!();

    if run_faithful {
        let run = run_acid3(&server.base_url(), DriveMode::Faithful);
        report(
            "FAITHFUL LOAD EMULATION (onload + setTimeout/advance)",
            &run,
        );
    }

    if run_direct {
        let run = run_acid3(&server.base_url(), DriveMode::DirectDrive);
        report(
            "DIRECT DRIVE (update() called directly, bypasses setTimeout)",
            &run,
        );
    }
}

fn report(title: &str, run: &Acid3Run) {
    let bar = "=".repeat(72);
    println!("{bar}");
    println!("{title}");
    println!("{bar}");
    println!("acid3.html HTTP status : {}", run.page_status);
    println!("page size              : {} bytes", run.html_bytes);
    println!("typeof update          : {}", run.update_typeof);
    println!("document script errors : {}", run.script_errors.len());
    for (i, e) in run.script_errors.iter().enumerate() {
        println!("    [{i}] {}", truncate(e, 300));
    }
    println!("loop iterations driven : {}", run.iterations);
    println!("drive errors           : {}", run.drive_errors.len());
    for (i, e) in run.drive_errors.iter().enumerate() {
        println!("    [{i}] {}", truncate(e, 300));
    }

    let total = run
        .total
        .map(|t| t.to_string())
        .unwrap_or_else(|| "?".into());
    let score = run
        .score
        .map(|s| s.to_string())
        .unwrap_or_else(|| "?".into());
    println!();
    println!(">>> SCORE: {score}/{total}");
    println!(
        ">>> reached test index : {}",
        run.index
            .map(|i| i.to_string())
            .unwrap_or_else(|| "?".into())
    );
    println!(">>> #score element text : {:?}", run.score_text);
    println!();
    match &run.log {
        Some(log) if !log.is_empty() => {
            println!("--- failure log ---");
            println!("{log}");
        }
        Some(_) => println!("--- failure log empty ---"),
        None => println!("--- failure log unavailable (log global not defined) ---"),
    }
    println!();
}

fn truncate(s: &str, max: usize) -> String {
    let one_line = s.replace('\n', " \\n ");
    if one_line.chars().count() <= max {
        one_line
    } else {
        let truncated: String = one_line.chars().take(max).collect();
        format!("{truncated}…")
    }
}
