use std::env;
use std::ffi::{CStr, CString};
use std::fs;
use std::os::raw::c_char;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant};

use base64::Engine;
use omoikane::ffi::{
    OmoikaneBrowser, omoikane_free, omoikane_get_content, omoikane_init, omoikane_last_error,
    omoikane_navigate, omoikane_screenshot_png_with_viewport, omoikane_set_insecure,
    omoikane_set_user_agent, omoikane_string_free,
};
use omoikane::paint::{RenderTimings, take_last_render_timings};

const DEFAULT_WIDTH: u32 = 1280;
const DEFAULT_HEIGHT: u32 = 720;

const SCREENSHOT_STACK_SIZE: usize = 64 * 1024 * 1024;

fn main() -> Result<(), String> {
    thread::Builder::new()
        .name("omoikane-screenshot".to_string())
        .stack_size(SCREENSHOT_STACK_SIZE)
        .spawn(run)
        .map_err(|error| format!("failed to start screenshot worker: {error}"))?
        .join()
        .map_err(|_| "screenshot worker panicked".to_string())?
}

fn run() -> Result<(), String> {
    let raw_args: Vec<String> = env::args().skip(1).collect();
    let mut args_iter = raw_args.iter().peekable();

    let mut insecure = false;
    let mut force_opacity = false;
    let mut firefox_user_agent = false;
    let mut dump_html = None;

    // Parse flags before positional arguments
    while let Some(arg) = args_iter.peek() {
        match arg.as_str() {
            "--insecure" | "-k" => {
                insecure = true;
                args_iter.next();
            }
            "--force-opacity" => {
                force_opacity = true;
                args_iter.next();
            }
            "--firefox-user-agent" => {
                firefox_user_agent = true;
                args_iter.next();
            }
            "--dump-html" => {
                args_iter.next();
                let Some(path) = args_iter.next() else {
                    return Err("--dump-html requires an output path".to_string());
                };
                dump_html = Some(PathBuf::from(path));
            }
            _ => break,
        }
    }

    let remaining: Vec<&str> = args_iter.map(String::as_str).collect();

    let url = remaining.first().copied().ok_or_else(usage)?;
    let output = remaining.get(1).copied().ok_or_else(usage)?;

    let width = parse_dimension(remaining.get(2).copied(), DEFAULT_WIDTH, "width")?;
    let height = parse_dimension(remaining.get(3).copied(), DEFAULT_HEIGHT, "height")?;
    let output_path = PathBuf::from(output);

    let url_c = CString::new(url).map_err(|_| "url contains interior NUL byte")?;

    // SAFETY: FFI calls are wrapped and pointers are validated before use.
    let browser = unsafe { omoikane_init() };
    if browser.is_null() {
        return Err("failed to initialize Omoikane browser".to_string());
    }

    if insecure {
        // SAFETY: `browser` is a valid handle and `insecure` is a plain bool.
        unsafe { omoikane_set_insecure(browser, true) };
    }

    if firefox_user_agent {
        let user_agent =
            CString::new("Mozilla/5.0 (X11; Linux x86_64; rv:140.0) Gecko/20100101 Firefox/140.0")
                .map_err(|_| "Firefox User-Agent contains interior NUL byte")?;
        // SAFETY: `browser` is valid and `user_agent` is a NUL-terminated string.
        if !unsafe { omoikane_set_user_agent(browser, user_agent.as_ptr()) } {
            unsafe { omoikane_free(browser) };
            return Err("failed to set Firefox User-Agent".to_string());
        }
    }

    let run_result = if force_opacity {
        omoikane::paint::with_force_opacity(|| {
            run_screenshot(
                browser,
                &url_c,
                url,
                &output_path,
                dump_html.as_ref(),
                width,
                height,
            )
        })
    } else {
        run_screenshot(browser, &url_c, url, &output_path, dump_html.as_ref(), width, height)
    };

    // SAFETY: `browser` was allocated by `omoikane_init`.
    unsafe { omoikane_free(browser) };
    run_result
}

fn run_screenshot(
    browser: *mut OmoikaneBrowser,
    url: &CString,
    url_display: &str,
    output_path: &PathBuf,
    dump_html: Option<&PathBuf>,
    width: u32,
    height: u32,
) -> Result<(), String> {
    let navigate_start = Instant::now();
    // SAFETY: `browser` is a valid handle and `url` is a NUL-terminated C string.
    let navigated = unsafe { omoikane_navigate(browser, url.as_ptr()) };
    if !navigated {
        return Err(last_error_message(browser));
    }
    let navigate_elapsed = navigate_start.elapsed();

    let render_start = Instant::now();
    // SAFETY: `browser` is a valid handle and dimensions are plain values.
    let png_b64_ptr = unsafe { omoikane_screenshot_png_with_viewport(browser, width, height) };
    if png_b64_ptr.is_null() {
        return Err(last_error_message(browser));
    }
    let render_elapsed = render_start.elapsed();

    let png_b64 = take_owned_string(png_b64_ptr)?;
    let png = base64::engine::general_purpose::STANDARD
        .decode(png_b64)
        .map_err(|error| format!("failed to decode screenshot payload: {error}"))?;

    if let Some(path) = dump_html {
        // SAFETY: `browser` is a valid handle. The returned string is released below.
        let html_ptr = unsafe { omoikane_get_content(browser) };
        let html = take_owned_string(html_ptr)?;
        fs::write(path, html).map_err(|error| format!("failed to write HTML dump: {error}"))?;
    }

    if let Some(parent) = output_path.parent()
        && !parent.as_os_str().is_empty()
    {
        fs::create_dir_all(parent)
            .map_err(|error| format!("failed to create output directory: {error}"))?;
    }
    fs::write(output_path, png).map_err(|error| format!("failed to write output PNG: {error}"))?;

    println!("{}", format_success_message(
        output_path,
        width,
        height,
        url_display,
        navigate_elapsed,
        render_elapsed,
    ));
    let timings = take_last_render_timings();
    println!("{}", format_render_phase_message(&timings));
    println!("{}", format_javascript_phase_message(&timings));
    Ok(())
}

fn format_render_phase_message(timings: &RenderTimings) -> String {
    let milliseconds = |duration: Duration| duration.as_secs_f64() * 1_000.0;
    format!(
        "render phases: stylesheets={:.1}ms fonts={:.1}ms javascript={:.1}ms timers={:.1}ms style-refresh={:.1}ms layout={:.1}ms paint={:.1}ms png-encode={:.1}ms",
        milliseconds(timings.stylesheets), milliseconds(timings.fonts),
        milliseconds(timings.javascript), milliseconds(timings.timers),
        milliseconds(timings.style_refresh), milliseconds(timings.layout),
        milliseconds(timings.paint), milliseconds(timings.png_encode),
    )
}

fn format_javascript_phase_message(timings: &RenderTimings) -> String {
    let milliseconds = |duration: Duration| duration.as_secs_f64() * 1_000.0;
    format!(
        "javascript phases: runtime-init={:.1}ms document-scripts={:.1}ms load-events={:.1}ms",
        milliseconds(timings.javascript_runtime_init),
        milliseconds(timings.javascript_document_scripts),
        milliseconds(timings.javascript_load_events),
    )
}

fn format_success_message(
    output_path: &Path,
    width: u32,
    height: u32,
    url: &str,
    navigate_elapsed: Duration,
    render_elapsed: Duration,
) -> String {
    format!(
        "saved screenshot: {} ({}x{}) url={} navigate={:.1}ms render={:.1}ms",
        output_path.display(),
        width,
        height,
        url,
        navigate_elapsed.as_secs_f64() * 1_000.0,
        render_elapsed.as_secs_f64() * 1_000.0,
    )
}

fn parse_dimension(raw: Option<&str>, default_value: u32, label: &str) -> Result<u32, String> {
    match raw {
        Some(value) => value
            .parse::<u32>()
            .map_err(|error| format!("invalid {label} '{value}': {error}")),
        None => Ok(default_value),
    }
}

fn last_error_message(browser: *mut OmoikaneBrowser) -> String {
    // SAFETY: `browser` is a valid handle.
    let error_ptr = unsafe { omoikane_last_error(browser) };
    if error_ptr.is_null() {
        return "operation failed with unknown error".to_string();
    }
    take_owned_string(error_ptr).unwrap_or_else(|_| "failed to read error message".to_string())
}

fn take_owned_string(ptr: *mut c_char) -> Result<String, String> {
    if ptr.is_null() {
        return Err("received null pointer".to_string());
    }
    // SAFETY: `ptr` is expected to point to a valid NUL-terminated string allocated by Omoikane.
    let raw = unsafe { CStr::from_ptr(ptr) };
    let value = raw.to_string_lossy().into_owned();
    // SAFETY: `ptr` was allocated by Omoikane via `CString::into_raw`.
    unsafe { omoikane_string_free(ptr) };
    Ok(value)
}

fn usage() -> String {
    "usage: cargo run --example screenshot -- [--insecure|-k] [--force-opacity] [--firefox-user-agent] [--dump-html <output.html>] <url> <output.png> [width] [height]"
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use omoikane::paint::RenderTimings;

    #[test]
    fn success_message_includes_url_and_phase_timings() {
        assert_eq!(
            format_success_message(
                &PathBuf::from("/tmp/test.png"),
                1280,
                720,
                "https://x.com/",
                Duration::from_micros(842_345),
                Duration::from_micros(126_749),
            ),
            "saved screenshot: /tmp/test.png (1280x720) url=https://x.com/ navigate=842.3ms render=126.7ms"
        );
    }

    #[test]
    fn render_phase_message_includes_each_pipeline_stage() {
        let timings = RenderTimings {
            stylesheets: Duration::from_micros(1_250),
            fonts: Duration::from_micros(2_500),
            javascript: Duration::from_micros(3_750),
            javascript_runtime_init: Duration::from_micros(500),
            javascript_document_scripts: Duration::from_micros(2_000),
            javascript_load_events: Duration::from_micros(1_250),
            timers: Duration::from_micros(4_000),
            style_refresh: Duration::from_micros(5_250),
            layout: Duration::from_micros(6_500),
            paint: Duration::from_micros(7_750),
            png_encode: Duration::from_micros(8_000),
        };
        assert_eq!(
            format_render_phase_message(&timings),
            "render phases: stylesheets=1.2ms fonts=2.5ms javascript=3.8ms timers=4.0ms style-refresh=5.2ms layout=6.5ms paint=7.8ms png-encode=8.0ms"
        );
        assert_eq!(
            format_javascript_phase_message(&timings),
            "javascript phases: runtime-init=0.5ms document-scripts=2.0ms load-events=1.2ms"
        );
    }
}
