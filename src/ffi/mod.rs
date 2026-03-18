//! C FFI surface for embedding the browser engine from other languages.

use std::cell::RefCell;
use std::ffi::{CStr, CString, c_char};

use serde_json::json;

use crate::cdp::CdpSession;

/// Opaque browser handle for the C ABI.
#[repr(C)]
pub struct OmoikaneBrowser {
    _private: [u8; 0],
}

struct OmoikaneBrowserHandle {
    session: RefCell<CdpSession>,
    last_error: RefCell<Option<String>>,
}

impl OmoikaneBrowserHandle {
    fn new() -> Result<Self, String> {
        Ok(Self {
            session: RefCell::new(CdpSession::new()?),
            last_error: RefCell::new(None),
        })
    }

    fn set_error(&self, message: impl Into<String>) {
        *self.last_error.borrow_mut() = Some(message.into());
    }

    fn clear_error(&self) {
        *self.last_error.borrow_mut() = None;
    }
}

/// Creates a new browser handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn omoikane_init() -> *mut OmoikaneBrowser {
    match OmoikaneBrowserHandle::new() {
        Ok(browser) => Box::into_raw(Box::new(browser)) as *mut OmoikaneBrowser,
        Err(_) => std::ptr::null_mut(),
    }
}

/// Destroys a browser handle previously created by [`omoikane_init`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn omoikane_free(browser: *mut OmoikaneBrowser) {
    if browser.is_null() {
        return;
    }

    // SAFETY: `browser` was created by `Box::into_raw` in `omoikane_init`.
    unsafe {
        drop(Box::from_raw(browser as *mut OmoikaneBrowserHandle));
    }
}

/// Navigates the active page to `url`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn omoikane_navigate(
    browser: *mut OmoikaneBrowser,
    url: *const c_char,
) -> bool {
    let Some(browser) = browser_from_ptr(browser) else {
        return false;
    };
    let Some(url) = string_from_ptr(url) else {
        browser.set_error("url must be a valid UTF-8 string");
        return false;
    };

    let result = browser
        .session
        .borrow_mut()
        .dispatch("Page.navigate", json!({ "url": url }));

    match result {
        Ok(_) => {
            browser.clear_error();
            true
        }
        Err(error) => {
            browser.set_error(error.message);
            false
        }
    }
}

/// Evaluates JavaScript in the current page and returns a JSON payload string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn omoikane_evaluate(
    browser: *mut OmoikaneBrowser,
    expression: *const c_char,
) -> *mut c_char {
    let Some(browser) = browser_from_ptr(browser) else {
        return std::ptr::null_mut();
    };
    let Some(expression) = string_from_ptr(expression) else {
        browser.set_error("expression must be a valid UTF-8 string");
        return std::ptr::null_mut();
    };

    let result = browser.session.borrow_mut().dispatch(
        "Runtime.evaluate",
        json!({
            "expression": expression,
            "returnByValue": true,
        }),
    );

    match result.and_then(|value| {
        serde_json::to_string(&value).map_err(|error| crate::cdp::JsonRpcError {
            code: -32000,
            message: error.to_string(),
        })
    }) {
        Ok(serialized) => {
            browser.clear_error();
            into_c_string(serialized)
        }
        Err(error) => {
            browser.set_error(error.message);
            std::ptr::null_mut()
        }
    }
}

/// Returns the current document serialized as HTML.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn omoikane_get_content(browser: *mut OmoikaneBrowser) -> *mut c_char {
    let Some(browser) = browser_from_ptr(browser) else {
        return std::ptr::null_mut();
    };

    let result = browser
        .session
        .borrow_mut()
        .dispatch("DOM.getDocument", json!({}));
    let html = match result.and_then(|document| {
        let node_id =
            document["root"]["nodeId"]
                .as_u64()
                .ok_or_else(|| crate::cdp::JsonRpcError {
                    code: -32000,
                    message: "DOM.getDocument did not return a root nodeId".to_string(),
                })?;
        browser
            .session
            .borrow_mut()
            .dispatch("DOM.getOuterHTML", json!({ "nodeId": node_id }))
    }) {
        Ok(value) => value["outerHTML"].as_str().map(ToString::to_string),
        Err(error) => {
            browser.set_error(error.message);
            return std::ptr::null_mut();
        }
    };

    match html {
        Some(html) => {
            browser.clear_error();
            into_c_string(html)
        }
        None => {
            browser.set_error("DOM.getOuterHTML did not return outerHTML");
            std::ptr::null_mut()
        }
    }
}

/// Placeholder screenshot API for the stable ABI surface.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn omoikane_screenshot_png(_browser: *mut OmoikaneBrowser) -> *mut c_char {
    std::ptr::null_mut()
}

/// Returns the last error message for the browser handle, if any.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn omoikane_last_error(browser: *const OmoikaneBrowser) -> *mut c_char {
    let Some(browser) = browser_from_const_ptr(browser) else {
        return std::ptr::null_mut();
    };

    match browser.last_error.borrow().clone() {
        Some(error) => into_c_string(error),
        None => std::ptr::null_mut(),
    }
}

/// Frees a string allocated by this library.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn omoikane_string_free(value: *mut c_char) {
    if value.is_null() {
        return;
    }

    // SAFETY: `value` must have come from `CString::into_raw` in this module.
    unsafe {
        drop(CString::from_raw(value));
    }
}

fn browser_from_ptr<'a>(browser: *mut OmoikaneBrowser) -> Option<&'a OmoikaneBrowserHandle> {
    if browser.is_null() {
        None
    } else {
        // SAFETY: Caller promises a valid pointer for the duration of the call.
        Some(unsafe { &*(browser as *mut OmoikaneBrowserHandle) })
    }
}

fn browser_from_const_ptr<'a>(
    browser: *const OmoikaneBrowser,
) -> Option<&'a OmoikaneBrowserHandle> {
    if browser.is_null() {
        None
    } else {
        // SAFETY: Caller promises a valid pointer for the duration of the call.
        Some(unsafe { &*(browser as *const OmoikaneBrowserHandle) })
    }
}

fn string_from_ptr(value: *const c_char) -> Option<String> {
    if value.is_null() {
        return None;
    }

    // SAFETY: Caller promises a valid NUL-terminated C string.
    unsafe { CStr::from_ptr(value) }
        .to_str()
        .ok()
        .map(ToString::to_string)
}

fn into_c_string(value: String) -> *mut c_char {
    CString::new(value)
        .expect("FFI strings must not contain interior NUL bytes")
        .into_raw()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn to_c_string(value: &str) -> CString {
        CString::new(value).unwrap()
    }

    unsafe fn take_string(value: *mut c_char) -> String {
        assert!(!value.is_null());
        let owned = unsafe { CString::from_raw(value) };
        owned.into_string().unwrap()
    }

    #[test]
    fn ffi_can_navigate_evaluate_and_read_content() {
        let browser = unsafe { omoikane_init() };
        assert!(!browser.is_null());

        let url =
            to_c_string("data:text/html,<html><body><main id=\"app\">ffi</main></body></html>");
        let ok = unsafe { omoikane_navigate(browser, url.as_ptr()) };
        assert!(ok);

        let expression = to_c_string("document.getElementById('app').nodeName");
        let evaluated = unsafe { omoikane_evaluate(browser, expression.as_ptr()) };
        let payload = unsafe { take_string(evaluated) };
        assert!(payload.contains("\"MAIN\""));

        let content = unsafe { omoikane_get_content(browser) };
        let html = unsafe { take_string(content) };
        assert!(html.contains("<main id=\"app\">ffi</main>"));

        unsafe { omoikane_free(browser) };
    }

    #[test]
    fn ffi_exposes_last_error_for_invalid_javascript() {
        let browser = unsafe { omoikane_init() };
        assert!(!browser.is_null());

        let expression = to_c_string("(()");
        let evaluated = unsafe { omoikane_evaluate(browser, expression.as_ptr()) };
        assert!(evaluated.is_null());

        let error = unsafe { omoikane_last_error(browser) };
        let message = unsafe { take_string(error) };
        assert!(message.contains("SyntaxError"));

        unsafe { omoikane_free(browser) };
    }

    #[test]
    fn generated_header_includes_core_exports() {
        let header = fs::read_to_string("include/omoikane.h").unwrap();

        assert!(header.contains("omoikane_init"));
        assert!(header.contains("omoikane_navigate"));
        assert!(header.contains("omoikane_evaluate"));
        assert!(header.contains("omoikane_string_free"));
    }
}
