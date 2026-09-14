use std::collections::HashMap;
use std::io::Write;
use std::mem;

use boa_engine::object::builtins::{JsArray, JsArrayBuffer, JsUint8Array};
use boa_engine::{Context, JsError, JsNativeError, JsResult, JsValue, js_string};
use flate2::Compression;
use flate2::write::{GzDecoder, GzEncoder, ZlibDecoder, ZlibEncoder};

use super::{string_argument, take_monotonic_id, with_host_state};

enum Codec {
    GzipEncoder(GzEncoder<Vec<u8>>),
    ZlibEncoder(ZlibEncoder<Vec<u8>>),
    GzipDecoder(GzDecoder<Vec<u8>>),
    ZlibDecoder(ZlibDecoder<Vec<u8>>),
}

struct WriteOutcome {
    bytes: Vec<u8>,
    error: Option<String>,
}

impl Codec {
    fn write(&mut self, input: &[u8]) -> WriteOutcome {
        let result = match self {
            Self::GzipEncoder(codec) => codec.write_all(input),
            Self::ZlibEncoder(codec) => codec.write_all(input),
            Self::GzipDecoder(codec) => codec.write_all(input),
            Self::ZlibDecoder(codec) => codec.write_all(input),
        };
        let bytes = match self {
            Self::GzipEncoder(codec) => mem::take(codec.get_mut()),
            Self::ZlibEncoder(codec) => mem::take(codec.get_mut()),
            Self::GzipDecoder(codec) => mem::take(codec.get_mut()),
            Self::ZlibDecoder(codec) => mem::take(codec.get_mut()),
        };
        WriteOutcome {
            bytes,
            error: result.err().map(|error| error.to_string()),
        }
    }

    fn finish(self) -> std::io::Result<Vec<u8>> {
        match self {
            Self::GzipEncoder(codec) => codec.finish(),
            Self::ZlibEncoder(codec) => codec.finish(),
            Self::GzipDecoder(codec) => codec.finish(),
            Self::ZlibDecoder(codec) => codec.finish(),
        }
    }
}

pub(super) struct Store {
    next_id: u64,
    codecs: HashMap<u64, Codec>,
}

impl Store {
    pub(super) fn new() -> Self {
        Self {
            next_id: 1,
            codecs: HashMap::new(),
        }
    }

    fn create(&mut self, operation: &str, format: &str) -> Result<u64, String> {
        if self.next_id > 9_007_199_254_740_991 {
            return Err("compression stream id space exhausted".to_string());
        }
        let codec = match (operation, format) {
            ("compress", "gzip") => {
                Codec::GzipEncoder(GzEncoder::new(Vec::new(), Compression::default()))
            }
            ("compress", "deflate") => {
                Codec::ZlibEncoder(ZlibEncoder::new(Vec::new(), Compression::default()))
            }
            ("decompress", "gzip") => Codec::GzipDecoder(GzDecoder::new(Vec::new())),
            ("decompress", "deflate") => Codec::ZlibDecoder(ZlibDecoder::new(Vec::new())),
            _ => return Err("unsupported compression format".to_string()),
        };
        let id = take_monotonic_id(&mut self.next_id, "compression stream")?;
        self.codecs.insert(id, codec);
        Ok(id)
    }

    fn write(&mut self, id: u64, input: &[u8]) -> Result<WriteOutcome, String> {
        let outcome = self
            .codecs
            .get_mut(&id)
            .ok_or_else(|| "compression stream is no longer active".to_string())?
            .write(input);
        if outcome.error.is_some() {
            self.codecs.remove(&id);
        }
        Ok(outcome)
    }

    fn finish(&mut self, id: u64) -> Result<Vec<u8>, String> {
        self.codecs
            .remove(&id)
            .ok_or_else(|| "compression stream is no longer active".to_string())?
            .finish()
            .map_err(|error| error.to_string())
    }

    fn abort(&mut self, id: u64) {
        self.codecs.remove(&id);
    }
}

fn stream_id(value: Option<&JsValue>, context: &mut Context) -> JsResult<u64> {
    let value = value.cloned().unwrap_or_default().to_number(context)?;
    if !value.is_finite() || value < 1.0 || value.fract() != 0.0 || value > 9_007_199_254_740_991.0
    {
        return Err(JsNativeError::typ()
            .with_message("invalid compression stream id")
            .into());
    }
    Ok(value as u64)
}

fn uint8_array_bytes(value: Option<&JsValue>, context: &mut Context) -> JsResult<Vec<u8>> {
    let object = value.and_then(JsValue::as_object).ok_or_else(|| {
        JsError::from(JsNativeError::typ().with_message("compression chunk must be a Uint8Array"))
    })?;
    let view = JsUint8Array::from_object(object.clone()).map_err(|_| {
        JsError::from(JsNativeError::typ().with_message("compression chunk must be a Uint8Array"))
    })?;
    let offset = view.byte_offset(context)?;
    let length = view.byte_length(context)?;
    let buffer = view
        .buffer(context)?
        .as_object()
        .and_then(|buffer| JsArrayBuffer::from_object(buffer.clone()).ok())
        .ok_or_else(|| {
            JsError::from(JsNativeError::typ().with_message("compression chunk has no ArrayBuffer"))
        })?;
    let data = buffer.data().ok_or_else(|| {
        JsError::from(JsNativeError::typ().with_message("compression chunk buffer is detached"))
    })?;
    data.get(offset..)
        .and_then(|tail| tail.get(..length))
        .map(<[u8]>::to_vec)
        .ok_or_else(|| {
            JsError::from(JsNativeError::typ().with_message("compression chunk is out of bounds"))
        })
}

pub(super) fn create_native(
    _this: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let operation = string_argument(args.first(), "", context)?;
    let format = string_argument(args.get(1), "", context)?;
    let id = with_host_state(|state| {
        state
            .borrow_mut()
            .compression_streams
            .create(&operation, &format)
            .map_err(|message| JsNativeError::typ().with_message(message).into())
    })?;
    Ok(JsValue::from(id as f64))
}

pub(super) fn write_native(
    _this: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let id = stream_id(args.first(), context)?;
    let input = uint8_array_bytes(args.get(1), context)?;
    let outcome = with_host_state(|state| {
        state
            .borrow_mut()
            .compression_streams
            .write(id, &input)
            .map_err(|message| JsNativeError::typ().with_message(message).into())
    })?;
    let bytes: JsValue = JsUint8Array::from_iter(outcome.bytes, context)?.into();
    let error = outcome
        .error
        .map_or_else(JsValue::null, |message| js_string!(message).into());
    Ok(JsArray::from_iter([bytes, error], context).into())
}

pub(super) fn finish_native(
    _this: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let id = stream_id(args.first(), context)?;
    let output = with_host_state(|state| {
        state
            .borrow_mut()
            .compression_streams
            .finish(id)
            .map_err(|message| JsNativeError::typ().with_message(message).into())
    })?;
    Ok(JsUint8Array::from_iter(output, context)?.into())
}

pub(super) fn abort_native(
    _this: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let id = stream_id(args.first(), context)?;
    with_host_state(|state| {
        state.borrow_mut().compression_streams.abort(id);
        Ok(JsValue::undefined())
    })
}
