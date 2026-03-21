use std::env;
use std::ffi::{CStr, CString};
use std::fs;
use std::os::raw::c_char;
use std::path::PathBuf;

use base64::Engine;
use omoikane::ffi::{
    OmoikaneBrowser, omoikane_free, omoikane_init, omoikane_last_error, omoikane_navigate,
    omoikane_screenshot_png_with_viewport, omoikane_string_free,
};

const DEFAULT_WIDTH: u32 = 1280;
const DEFAULT_HEIGHT: u32 = 720;

fn main() -> Result<(), String> {
    let mut args = env::args().skip(1);
    let Some(url) = args.next() else {
        return Err(usage());
    };
    let Some(output) = args.next() else {
        return Err(usage());
    };

    let width = parse_dimension(args.next(), DEFAULT_WIDTH, "width")?;
    let height = parse_dimension(args.next(), DEFAULT_HEIGHT, "height")?;
    let output_path = PathBuf::from(output);

    let url_c = CString::new(url.as_str()).map_err(|_| "url contains interior NUL byte")?;

    // SAFETY: FFI calls are wrapped and pointers are validated before use.
    let browser = unsafe { omoikane_init() };
    if browser.is_null() {
        return Err("failed to initialize Omoikane browser".to_string());
    }

    let run_result = run_screenshot(browser, &url_c, &output_path, width, height);

    // SAFETY: `browser` was allocated by `omoikane_init`.
    unsafe { omoikane_free(browser) };
    run_result
}

fn run_screenshot(
    browser: *mut OmoikaneBrowser,
    url: &CString,
    output_path: &PathBuf,
    width: u32,
    height: u32,
) -> Result<(), String> {
    // SAFETY: `browser` is a valid handle and `url` is a NUL-terminated C string.
    let navigated = unsafe { omoikane_navigate(browser, url.as_ptr()) };
    if !navigated {
        return Err(last_error_message(browser));
    }

    // SAFETY: `browser` is a valid handle and dimensions are plain values.
    let png_b64_ptr = unsafe { omoikane_screenshot_png_with_viewport(browser, width, height) };
    if png_b64_ptr.is_null() {
        return Err(last_error_message(browser));
    }

    let png_b64 = take_owned_string(png_b64_ptr)?;
    let png = base64::engine::general_purpose::STANDARD
        .decode(png_b64)
        .map_err(|error| format!("failed to decode screenshot payload: {error}"))?;

    if let Some(parent) = output_path.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent)
                .map_err(|error| format!("failed to create output directory: {error}"))?;
        }
    }
    fs::write(output_path, png).map_err(|error| format!("failed to write output PNG: {error}"))?;

    println!(
        "saved screenshot: {} ({}x{})",
        output_path.display(),
        width,
        height
    );
    Ok(())
}

fn parse_dimension(raw: Option<String>, default_value: u32, label: &str) -> Result<u32, String> {
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
    "usage: cargo run --example screenshot -- <url> <output.png> [width] [height]".to_string()
}
