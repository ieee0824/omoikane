use super::JsRuntime;

fn run_async_check(source: &str, result: &str) {
    let mut runtime = JsRuntime::new().unwrap();
    runtime.eval(source).unwrap();
    runtime.run_until_idle().unwrap();
    assert!(
        runtime.eval(result).unwrap().as_boolean().unwrap_or(false),
        "JavaScript check failed: {result}"
    );
}

#[test]
fn compression_streams_are_exposed_in_window_and_worker() {
    let mut runtime = JsRuntime::new().unwrap();
    assert!(
        runtime
            .eval(
                "typeof CompressionStream === 'function' && \
                 typeof DecompressionStream === 'function' && \
                 typeof __omoikane_compression_create === 'undefined' && \
                 typeof __omoikane_compression_write === 'undefined' && \
                 typeof __omoikane_compression_finish === 'undefined' && \
                 typeof __omoikane_compression_abort === 'undefined'",
            )
            .unwrap()
            .as_boolean()
            .unwrap_or(false)
    );

    runtime
        .eval(
            r#"globalThis.compressionWorkerResult = null;
               const source = encodeURIComponent(`
                 (async () => {
                   if (typeof CompressionStream !== "function" || typeof DecompressionStream !== "function") {
                     postMessage("missing"); return;
                   }
                   if (typeof __omoikane_compression_create !== "undefined" ||
                       typeof __omoikane_compression_write !== "undefined" ||
                       typeof __omoikane_compression_finish !== "undefined" ||
                       typeof __omoikane_compression_abort !== "undefined") {
                     postMessage("native hook leak"); return;
                   }
                   const stream = new DecompressionStream("deflate");
                   const reader = stream.readable.getReader();
                   const reading = (async () => {
                     const bytes = [];
                     while (true) {
                       const result = await reader.read();
                       if (result.done) return bytes;
                       bytes.push(...result.value);
                     }
                   })();
                   const writer = stream.writable.getWriter();
                   await writer.write(new Uint8Array([120,156,75,173,40,72,77,46,73,77,81,200,47,45,41,40,45,1,0,48,173,6,36]));
                   await writer.close();
                   postMessage(String.fromCharCode(...await reading));
                 })().catch(error => postMessage("error:" + error));
               `);
               const worker = new Worker('data:text/javascript,' + source);
               worker.onmessage = event => { compressionWorkerResult = event.data; }"#,
        )
        .unwrap();
    runtime.run_until_idle().unwrap();
    assert_eq!(
        runtime
            .eval("compressionWorkerResult === 'expected output'")
            .unwrap()
            .as_boolean(),
        Some(true),
    );
}

#[test]
fn compression_streams_round_trip_split_chunks() {
    run_async_check(
        r#"
        globalThis.compressionRoundTrips = false;
        async function collectBytes(readable) {
          const reader = readable.getReader();
          const chunks = [];
          let length = 0;
          while (true) {
            const { value, done } = await reader.read();
            if (done) break;
            if (!(value instanceof Uint8Array)) throw new Error("non-byte output");
            chunks.push(value);
            length += value.byteLength;
          }
          const output = new Uint8Array(length);
          let offset = 0;
          for (const chunk of chunks) {
            output.set(chunk, offset);
            offset += chunk.byteLength;
          }
          return output;
        }
        async function roundTrip(format) {
          const compressor = new CompressionStream(format);
          const compressedPromise = collectBytes(compressor.readable);
          const writer = compressor.writable.getWriter();
          await writer.write(new Uint8Array([115, 112, 108, 105, 116, 45]));
          const backing = new Uint8Array([0, 99, 104, 117, 110, 107, 115, 0]);
          await writer.write(new DataView(backing.buffer, 1, 6));
          await writer.close();
          const compressed = await compressedPromise;
          if (!compressed.byteLength) return false;

          const decompressor = new DecompressionStream(format);
          const decodedPromise = collectBytes(decompressor.readable);
          const decoder = decompressor.writable.getWriter();
          for (let offset = 0; offset < compressed.length; offset += 3) {
            await decoder.write(compressed.slice(offset, offset + 3));
          }
          await decoder.close();
          const decoded = await decodedPromise;
          return Array.from(decoded).join(",") ===
            "115,112,108,105,116,45,99,104,117,110,107,115";
        }
        async function roundTripEmpty(format) {
          const compressor = new CompressionStream(format);
          const compressedPromise = collectBytes(compressor.readable);
          await compressor.writable.getWriter().close();
          const compressed = await compressedPromise;
          const decompressor = new DecompressionStream(format);
          const decodedPromise = collectBytes(decompressor.readable);
          const writer = decompressor.writable.getWriter();
          await writer.write(compressed);
          await writer.close();
          return (await decodedPromise).byteLength === 0;
        }
        Promise.all([
          roundTrip("gzip"), roundTrip("deflate"),
          roundTripEmpty("gzip"), roundTripEmpty("deflate"),
        ])
          .then(values => { compressionRoundTrips = values.every(Boolean); });
        "#,
        "compressionRoundTrips",
    );
}

#[test]
fn decompression_streams_validate_input_and_buffersources() {
    run_async_check(
        r#"
        globalThis.decompressionValidation = false;
        const known = {
          deflate: new Uint8Array([120,156,75,173,40,72,77,46,73,77,81,200,47,45,41,40,45,1,0,48,173,6,36]),
          gzip: new Uint8Array([31,139,8,0,0,0,0,0,0,3,75,173,40,72,77,46,73,77,81,200,47,45,41,40,45,1,0,176,1,57,179,15,0,0,0]),
        };
        async function readAll(readable) {
          const reader = readable.getReader();
          const bytes = [];
          while (true) {
            const { value, done } = await reader.read();
            if (done) return bytes;
            if (!(value instanceof Uint8Array)) throw new Error("non-byte output");
            bytes.push(...value);
          }
        }
        async function decodeBufferSources(format) {
          const stream = new DecompressionStream(format);
          const reading = readAll(stream.readable);
          const writer = stream.writable.getWriter();
          const bytes = known[format];
          const first = bytes.slice(0, 8).buffer;
          const rest = bytes.slice(8);
          await writer.write(first);
          await writer.write(new DataView(rest.buffer, rest.byteOffset, rest.byteLength));
          await writer.close();
          return String.fromCharCode(...await reading) === "expected output";
        }
        async function rejectsAtClose(format, bytes) {
          const stream = new DecompressionStream(format);
          const reader = stream.readable.getReader();
          const writer = stream.writable.getWriter();
          writer.closed.catch(() => {});
          const reading = (async () => {
            try { while (!(await reader.read()).done) {} } catch (_) {}
          })();
          try {
            await writer.write(bytes);
            await writer.close();
            await reading;
            return false;
          } catch (error) {
            await reading;
            return error instanceof TypeError;
          }
        }
        async function rejectsBadChunk() {
          const stream = new CompressionStream("gzip");
          const reader = stream.readable.getReader();
          const writer = stream.writable.getWriter();
          writer.closed.catch(() => {});
          const read = reader.read().then(() => false, error => error instanceof TypeError);
          const write = writer.write({}).then(() => false, error => error instanceof TypeError);
          return await read && await write;
        }
        async function extraInputYieldsThenErrors() {
          const stream = new DecompressionStream("gzip");
          const reader = stream.readable.getReader();
          const writer = stream.writable.getWriter();
          writer.closed.catch(() => {});
          const extra = new Uint8Array(known.gzip.length + 1);
          extra.set(known.gzip); extra[extra.length - 1] = 1;
          const firstRead = reader.read();
          const write = writer.write(extra).then(() => false, error => error instanceof TypeError);
          const first = await firstRead;
          const nextErrors = await reader.read().then(() => false, error => error instanceof TypeError);
          return String.fromCharCode(...first.value) === "expected output" && await write && nextErrors;
        }
        (async () => {
          let constructorErrors = false;
          let coercionError = false;
          try { new CompressionStream("invalid"); } catch (error) { constructorErrors = error instanceof TypeError; }
          try { new DecompressionStream(); } catch (error) { constructorErrors = constructorErrors && error instanceof TypeError; }
          try { new CompressionStream({ toString() { throw new Error("coercion"); } }); }
          catch (error) { coercionError = error.message === "coercion"; }

          const truncated = known.gzip.slice(0, -1);
          const corrupt = known.deflate.slice(); corrupt[corrupt.length - 1] ^= 1;
          const values = await Promise.all([
            decodeBufferSources("gzip"),
            decodeBufferSources("deflate"),
            rejectsAtClose("gzip", truncated),
            rejectsAtClose("deflate", corrupt),
            rejectsBadChunk(),
            extraInputYieldsThenErrors(),
          ]);
          decompressionValidation = constructorErrors && coercionError && values.every(Boolean);
        })();
        "#,
        "decompressionValidation",
    );
}

#[test]
fn compression_streams_propagate_cancel_abort_and_backpressure() {
    run_async_check(
        r#"
        globalThis.compressionLifecycle = false;
        (async () => {
          const aborted = new CompressionStream("gzip");
          const abortReader = aborted.readable.getReader();
          const abortWriter = aborted.writable.getWriter();
          const abortReason = new Error("abort marker");
          const pendingRead = abortReader.read().then(() => false, error => error === abortReason);
          const closedAfterAbort = abortWriter.closed.then(() => false, error => error === abortReason);
          await abortWriter.abort(abortReason);

          const cancelled = new CompressionStream("deflate");
          const cancelReader = cancelled.readable.getReader();
          const cancelWriter = cancelled.writable.getWriter();
          const cancelReason = new Error("cancel marker");
          const closedAfterCancel = cancelWriter.closed.then(() => false, error => error === cancelReason);
          await cancelReader.cancel(cancelReason);
          const writeAfterCancel = cancelWriter.write(new Uint8Array([1]))
            .then(() => false, error => error === cancelReason);

          const pressured = new CompressionStream("gzip");
          const pressureWriter = pressured.writable.getWriter();
          const pressureClosed = pressureWriter.closed.catch(() => {});
          let settled = false;
          const write = pressureWriter.write(new Uint8Array(65536)).then(() => { settled = true; });
          await Promise.resolve();
          await Promise.resolve();
          const wasBlocked = !settled;
          const pressureReader = pressured.readable.getReader();
          const first = await pressureReader.read();
          await write;
          await pressureWriter.abort(new Error("cleanup"));
          await pressureClosed;

          compressionLifecycle =
            await pendingRead && await closedAfterAbort && await closedAfterCancel &&
            await writeAfterCancel && wasBlocked && !first.done && first.value.byteLength > 0;
        })();
        "#,
        "compressionLifecycle",
    );
}
