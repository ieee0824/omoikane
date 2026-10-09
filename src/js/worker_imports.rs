//! Synchronous classic-worker script imports with owned fetched sources.
use super::*;

pub(super) fn register(context: &mut Context, bindings: &mut BootstrapBindings) -> JsResult<()> {
    register_private_callable(
        context,
        bindings,
        js_string!("__omoikane_import_scripts"),
        0,
        NativeFunction::from_copy_closure(import_scripts),
    )
}

fn import_scripts(_: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    // Complete argument conversion before starting network I/O or evaluation.
    let urls = args
        .iter()
        .map(|value| {
            value
                .to_string(context)
                .map(|value| value.to_std_string_escaped())
        })
        .collect::<JsResult<Vec<_>>>()?;
    let sources = with_host_state(|host| {
        let mut state = host.borrow_mut();
        let base = state.base_url.clone();
        let origin = script_errors::document_fetch_origin(&state, state.document.identity());
        let mut sources = Vec::with_capacity(urls.len());
        for url in urls {
            let Some((filename, source, _, muted_errors)) = script_errors::fetch_classic_source(
                &url,
                base.as_ref(),
                &origin,
                None,
                &mut state.http_client,
            ) else {
                return Ok(None);
            };
            sources.push((filename, source, muted_errors));
        }
        Ok(Some(sources))
    })?;
    let Some(sources) = sources else {
        return Ok(JsValue::from(false));
    };
    for (filename, source, muted_errors) in sources {
        let input = Source::from_reader(source.as_bytes(), Some(Path::new(&filename)));
        let metadata = script_errors::classic_script_metadata(muted_errors);
        let result = Script::parse_with_host_defined(input, None, metadata, context)
            .and_then(|script| script.evaluate(context));
        if let Err(error) = result {
            // Muted imported scripts cannot expose their thrown value to the
            // caller; the worker bootstrap turns false into NetworkError.
            if muted_errors
                && error.as_native().is_none_or(|error| {
                    !matches!(error.kind, boa_engine::JsNativeErrorKind::RuntimeLimit)
                })
            {
                return Ok(JsValue::from(false));
            }
            return Err(error);
        }
    }
    Ok(JsValue::from(true))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::http_fixture::{
        ACCEPT_TIMEOUT, FixtureWorker, READ_TIMEOUT, accept_with_timeout, bind_loopback,
        read_request_headers,
    };
    use std::io::Write;

    #[test]
    fn imported_non_cors_scripts_keep_muting_and_hide_thrown_values() {
        let listener = bind_loopback().unwrap();
        let address = listener.local_addr().unwrap();
        let server = FixtureWorker::spawn(move || {
            for body in [
                "globalThis.imported=()=>Promise.reject('private');",
                "throw { private: true };",
            ] {
                let mut stream = accept_with_timeout(&listener, ACCEPT_TIMEOUT).unwrap();
                read_request_headers(&mut stream, READ_TIMEOUT).unwrap();
                write!(stream,"HTTP/1.1 200 OK\r\nContent-Type: text/javascript\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",body.len(),body).unwrap();
            }
        });
        let mut runtime =
            JsRuntime::with_document_and_url(blank_html_document(), "http://127.0.0.1:1/worker.js")
                .unwrap();
        runtime.set_base_url(format!("http://{address}/").parse().unwrap());
        runtime.eval("globalThis.reports=[];addEventListener('unhandledrejection',e=>{reports.push(e.reason);e.preventDefault()});").unwrap();
        let url = JsValue::from(JsString::from(format!("http://{address}/import.js")));
        let imported = runtime
            .with_active_host(|context| {
                import_scripts(&JsValue::undefined(), &[url.clone()], context)
            })
            .unwrap();
        assert_eq!(imported.as_boolean(), Some(true));
        runtime.eval("imported()").unwrap();
        runtime.run_until_idle().unwrap();
        assert_eq!(
            runtime.eval("reports.length").unwrap().as_number(),
            Some(0.0)
        );
        let failed = runtime
            .with_active_host(|context| import_scripts(&JsValue::undefined(), &[url], context))
            .unwrap();
        assert_eq!(failed.as_boolean(), Some(false));
        server.join();
    }
}
