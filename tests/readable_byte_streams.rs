use omoikane::{html::TreeBuilder, js::JsRuntime};

fn runtime() -> JsRuntime {
    JsRuntime::with_document(TreeBuilder::parse("<main></main>").document()).expect("runtime")
}

fn json(runtime: &mut JsRuntime, source: &str) -> String {
    runtime
        .eval(source)
        .expect("evaluate stream probe")
        .as_string()
        .expect("JSON string")
        .to_std_string_escaped()
}

#[test]
fn byte_stream_byob_reader_fills_the_supplied_view() {
    let mut runtime = runtime();
    runtime
        .eval(
            r#"globalThis.result = [];
               const stream = new ReadableStream({
                 type: 'bytes',
                 start(controller) {
                   result.push(controller instanceof ReadableByteStreamController);
                   controller.enqueue(new Uint8Array([1, 2, 3, 4]));
                   controller.close();
                 }
               });
               const reader = stream.getReader({mode: 'byob'});
               result.push(reader instanceof ReadableStreamBYOBReader);
               reader.read(new Uint8Array(3)).then(first => {
                 result.push([...first.value], first.done);
                 return reader.read(new Uint8Array(3));
               }).then(second => result.push([...second.value], second.done));"#,
        )
        .expect("start BYOB reads");
    runtime.run_jobs().expect("BYOB jobs");
    assert_eq!(
        json(&mut runtime, "JSON.stringify(result)"),
        "[true,true,[1,2,3],false,[4],false]"
    );
}

#[test]
fn body_and_blob_streams_are_byte_streams() {
    let mut runtime = runtime();
    runtime
        .eval(
            r#"globalThis.result = [];
               Promise.all([
                 new Response(new Uint8Array([7, 8])).body.getReader({mode:'byob'}).read(new Uint8Array(4)),
                 new Blob([new Uint8Array([9, 10])]).stream().getReader({mode:'byob'}).read(new Uint8Array(4))
               ]).then(values => result.push(values.map(value => [...value.value])));"#,
        )
        .expect("read body streams");
    runtime.run_jobs().expect("body stream jobs");
    assert_eq!(
        json(&mut runtime, "JSON.stringify(result)"),
        "[[[7,8],[9,10]]]"
    );
}

#[test]
fn stream_from_and_async_iteration_observe_cancel_policy() {
    let mut runtime = runtime();
    runtime
        .eval(
            r#"globalThis.result = [];
               (async () => {
                 const values = [];
                 for await (const value of ReadableStream.from([1, 2, 3])) values.push(value);
                 result.push(values);
                 let cancelled = false;
                 const stream = new ReadableStream({
                   start(controller) { controller.enqueue('x'); controller.enqueue('y'); },
                   cancel() { cancelled = true; }
                 });
                 for await (const value of stream.values({preventCancel:true})) { result.push(value); break; }
                 result.push(cancelled, stream.locked);
               })();"#,
        )
        .expect("iterate streams");
    runtime.run_jobs().expect("iteration jobs");
    assert_eq!(
        json(&mut runtime, "JSON.stringify(result)"),
        "[[1,2,3],\"x\",false,false]"
    );
}

#[test]
fn auto_allocate_exposes_a_byob_request_to_pull() {
    let mut runtime = runtime();
    runtime
        .eval(
            r#"globalThis.result = [];
               const stream = new ReadableStream({
                 type: 'bytes', autoAllocateChunkSize: 4,
                 pull(controller) {
                   const request = controller.byobRequest;
                   result.push(request instanceof ReadableStreamBYOBRequest, request.view.byteLength);
                   request.view.set([5, 6]);
                   request.respond(2);
                   controller.close();
                 }
               });
               stream.getReader().read().then(value => result.push([...value.value], value.done));"#,
        )
        .expect("start auto-allocated read");
    runtime.run_jobs().expect("auto-allocation jobs");
    assert_eq!(
        json(&mut runtime, "JSON.stringify(result)"),
        "[true,4,[5,6],false]"
    );
}
