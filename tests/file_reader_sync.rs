use omoikane::js::JsRuntime;

#[test]
fn file_reader_sync_is_worker_only_and_reads_owned_blob_bytes() {
    let mut runtime = JsRuntime::new().expect("runtime");
    runtime
        .eval(
            r#"globalThis.workerResult = null;
               const source = `
                 const reader = new FileReaderSync();
                 const blob = new Blob([new Uint8Array([65, 66])], {type:'text/plain'});
                 postMessage([
                   typeof FileReaderSync,
                   [...new Uint8Array(reader.readAsArrayBuffer(blob))],
                   reader.readAsBinaryString(blob),
                   reader.readAsText(blob),
                   reader.readAsDataURL(blob)
                 ]);`;
               const worker = new Worker('data:text/javascript,' + encodeURIComponent(source));
               worker.onmessage = event => workerResult = event.data;
               worker.onerror = event => { workerResult = ['error', event.message]; event.preventDefault(); };"#,
        )
        .expect("start worker");
    runtime.run_until_idle().expect("worker jobs");
    assert_eq!(
        runtime
            .eval("JSON.stringify([typeof FileReaderSync, workerResult])")
            .expect("read worker result")
            .as_string()
            .expect("JSON string")
            .to_std_string_escaped(),
        "[\"undefined\",[\"function\",[65,66],\"AB\",\"AB\",\"data:text/plain;base64,QUI=\"]]"
    );
}
