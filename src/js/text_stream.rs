//! WHATWG encoding and stateful decoders owned by JavaScript stream closures.

use boa_engine::{
    Context, Finalize, JsData, JsNativeError, JsObject, JsResult, JsValue, Trace, js_string,
    object::builtins::{
        JsArray, JsArrayBuffer, JsDataView, JsSharedArrayBuffer, JsTypedArray, JsUint8Array,
    },
};
use encoding_rs::{CoderResult, Decoder, DecoderResult, Encoding, REPLACEMENT};

#[derive(Trace, Finalize, JsData)]
struct DecoderState {
    // encoding_rs holds only native data. The owning JS object supplies the
    // lifetime, so abandoned streams need no persistent host registry entry.
    #[unsafe_ignore_trace]
    decoder: Option<Decoder>,
    fatal: bool,
}

pub(super) fn create_native(
    _: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let label = args[0].to_string(context)?.to_std_string_escaped();
    let encoding = Encoding::for_label(label.as_bytes())
        .filter(|encoding| *encoding != REPLACEMENT)
        .ok_or_else(|| JsNativeError::range().with_message("Unsupported encoding label"))?;
    let decoder = if args[2].to_boolean() {
        encoding.new_decoder_without_bom_handling()
    } else {
        encoding.new_decoder_with_bom_removal()
    };
    let name: JsValue = js_string!(encoding.name().to_ascii_lowercase()).into();
    let handle = JsObject::from_proto_and_data(
        None,
        DecoderState {
            decoder: Some(decoder),
            fatal: args[1].to_boolean(),
        },
    );
    Ok(JsArray::from_iter([handle.into(), name], context).into())
}

// Web IDL's copy of an AllowSharedBufferSource reads internal slots, including
// view offsets, without consulting page-defined buffer/byteLength getters.
// Detached buffers contribute an empty byte sequence (Encoding Standard).
fn input_bytes(value: &JsValue, context: &mut Context) -> JsResult<Vec<u8>> {
    let object = value.as_object().ok_or_else(|| {
        JsNativeError::typ().with_message("TextDecoderStream chunk must be a BufferSource")
    })?;
    let (buffer, range) = if let Ok(view) = JsTypedArray::from_object(object.clone()) {
        (
            view.buffer(context)?,
            Some((view.byte_offset(context)?, view.byte_length(context)?)),
        )
    } else if let Ok(view) = JsDataView::from_object(object.clone()) {
        let buffer = view.buffer(context)?;
        if let Some(buffer) = buffer.as_object()
            && let Ok(buffer) = JsArrayBuffer::from_object(buffer.clone())
            && buffer.data().is_none()
        {
            if !buffer.is_fixed_length() {
                return Err(JsNativeError::typ()
                    .with_message("Resizable buffers are not allowed")
                    .into());
            }
            return Ok(Vec::new());
        }
        (
            buffer,
            Some((
                view.byte_offset(context)? as usize,
                view.byte_length(context)? as usize,
            )),
        )
    } else {
        (value.clone(), None)
    };
    let object = buffer.as_object().expect("buffer is an object");
    if let Ok(buffer) = JsArrayBuffer::from_object(object.clone()) {
        if !buffer.is_fixed_length() {
            return Err(JsNativeError::typ()
                .with_message("Resizable buffers are not allowed")
                .into());
        }
        let Some(data) = buffer.data() else {
            return Ok(Vec::new());
        };
        let (offset, length) = range.unwrap_or((0, data.len()));
        return data
            .get(offset..)
            .and_then(|data| data.get(..length))
            .map(<[u8]>::to_vec)
            .ok_or_else(|| {
                JsNativeError::typ()
                    .with_message("BufferSource is out of bounds")
                    .into()
            });
    }
    if let Ok(buffer) = JsSharedArrayBuffer::from_object(object.clone()) {
        if !buffer.is_fixed_length() {
            return Err(JsNativeError::typ()
                .with_message("Growable buffers are not allowed")
                .into());
        }
        let (offset, length) = range.unwrap_or((0, buffer.byte_length()));
        return buffer.copy_bytes(offset, length).ok_or_else(|| {
            JsNativeError::typ()
                .with_message("BufferSource is out of bounds")
                .into()
        });
    }
    Err(JsNativeError::typ()
        .with_message("TextDecoderStream chunk must be a BufferSource")
        .into())
}

pub(super) fn decode_native(
    _: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let last = args[2].to_boolean();
    let input = if last {
        Vec::new()
    } else {
        input_bytes(&args[1], context)?
    };
    let handle = args[0]
        .as_object()
        .ok_or_else(|| JsNativeError::typ().with_message("Invalid decoder"))?;
    let mut state = handle
        .downcast_mut::<DecoderState>()
        .ok_or_else(|| JsNativeError::typ().with_message("Invalid decoder"))?;
    let fatal = state.fatal;
    let decoder = state
        .decoder
        .as_mut()
        .ok_or_else(|| JsNativeError::typ().with_message("Decoder is finished"))?;
    let mut output = String::new();
    let mut remaining = input.as_slice();
    let mut buffer = [0; 8192];
    loop {
        let (finished, read, written) = if fatal {
            let (result, read, written) =
                decoder.decode_to_utf8_without_replacement(remaining, &mut buffer, last);
            match result {
                DecoderResult::Malformed(_, _) => {
                    state.decoder = None;
                    return Err(JsNativeError::typ()
                        .with_message("Invalid encoded data")
                        .into());
                }
                DecoderResult::InputEmpty => (true, read, written),
                DecoderResult::OutputFull => (false, read, written),
            }
        } else {
            let (result, read, written, _) = decoder.decode_to_utf8(remaining, &mut buffer, last);
            (result == CoderResult::InputEmpty, read, written)
        };
        output.push_str(std::str::from_utf8(&buffer[..written]).expect("decoder emits UTF-8"));
        remaining = &remaining[read..];
        if finished {
            break;
        }
    }
    if last {
        state.decoder = None;
    }
    drop(state);
    Ok(js_string!(output).into())
}

pub(super) fn encode_native(
    _: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let input = args[0].to_string(context)?.to_std_string_lossy();
    Ok(JsUint8Array::from_iter(input.into_bytes(), context)?.into())
}
