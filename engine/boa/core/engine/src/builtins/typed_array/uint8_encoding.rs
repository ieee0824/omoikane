//! Native Uint8Array text encodings.
use super::{BuiltinTypedArray, TypedArray, TypedArrayKind, Uint8Array};
use crate::{
    Context, JsArgs, JsNativeError, JsResult, JsString, JsValue,
    builtins::array_buffer::utils::{SliceRef, SliceRefMut},
    object::JsObject,
    string::{JsStr, Latin1JsStringBuilder},
};
use std::sync::atomic::Ordering;

fn with_uint8_bytes<R>(
    array: &JsObject<TypedArray>,
    buffer_length: usize,
    operation: impl FnOnce(SliceRef<'_>) -> R,
) -> R {
    // `buffer_length` is the witness captured by `ValidateTypedArray`. No JavaScript runs
    // between validation and this borrow, and a growable shared buffer cannot shrink below it.
    let array = array.borrow();
    let array = array.data();
    let start = usize::try_from(array.byte_offset()).expect("typed array offset fits in usize");
    let length = usize::try_from(array.array_length(buffer_length))
        .expect("typed array length fits in usize");
    let buffer = array.viewed_array_buffer().as_buffer();
    let bytes = buffer
        .bytes_with_len(buffer_length)
        .expect("validated typed array buffer cannot be detached");
    operation(bytes.subslice(start..start + length))
}

fn with_uint8_bytes_mut<R>(
    array: &JsObject<TypedArray>,
    buffer_length: usize,
    operation: impl FnOnce(SliceRefMut<'_>) -> R,
) -> R {
    // Keep the typed-array metadata and backing store borrowed only for the native write. The
    // callers release both borrows before allocating a result object or constructing an error.
    let array = array.borrow();
    let array = array.data();
    let start = usize::try_from(array.byte_offset()).expect("typed array offset fits in usize");
    let length = usize::try_from(array.array_length(buffer_length))
        .expect("typed array length fits in usize");
    let mut buffer = array.viewed_array_buffer().as_buffer_mut();
    let mut bytes = buffer
        .bytes_with_len(buffer_length)
        .expect("validated typed array buffer cannot be detached");
    operation(bytes.subslice_mut(start..start + length))
}

fn read_byte(bytes: SliceRef<'_>, index: usize) -> u8 {
    match bytes {
        SliceRef::Slice(bytes) => bytes[index],
        SliceRef::AtomicSlice(bytes) => bytes[index].load(Ordering::Relaxed),
    }
}

fn write_byte(bytes: &mut SliceRefMut<'_>, index: usize, value: u8) {
    match bytes {
        SliceRefMut::Slice(bytes) => bytes[index] = value,
        SliceRefMut::AtomicSlice(bytes) => bytes[index].store(value, Ordering::Relaxed),
    }
}

/// Encodes this Uint8Array's bytes as lowercase hexadecimal.
/// <https://tc39.es/ecma262/#sec-uint8array.prototype.tohex>
pub(super) fn to_hex(this: &JsValue, _: &[JsValue], _: &mut Context) -> JsResult<JsValue> {
    let (array, buffer_length) = TypedArray::validate(this, Ordering::SeqCst)?;
    let _root = array.clone().root();
    {
        let data = array.borrow();
        if data.data().kind() != TypedArrayKind::Uint8 {
            return Err(JsNativeError::typ()
                .with_message("toHex requires a Uint8Array")
                .into());
        }
    }
    Ok(with_uint8_bytes(&array, buffer_length, encode_hex).into())
}

fn encode_hex(bytes: SliceRef<'_>) -> JsString {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut output = Latin1JsStringBuilder::with_capacity(bytes.len() * 2);
    for index in 0..bytes.len() {
        let byte = read_byte(bytes, index);
        output.push(HEX[usize::from(byte >> 4)]);
        output.push(HEX[usize::from(byte & 15)]);
    }
    output.build().expect("hex output is ASCII")
}

/// Decodes strict ASCII hexadecimal into an intrinsic Uint8Array.
/// <https://tc39.es/ecma262/#sec-uint8array.fromhex>
pub(super) fn from_hex(_: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let input = args
        .get_or_undefined(0)
        .as_string()
        .ok_or_else(|| JsNativeError::typ().with_message("fromHex requires a string"))?;
    if input.len() % 2 != 0 {
        return Err(JsNativeError::syntax()
            .with_message("hex string has odd length")
            .into());
    }
    let input = input.as_str();
    for offset in (0..input.len()).step_by(2) {
        hex_pair(input, offset)?;
    }
    let constructor = context
        .intrinsics()
        .constructors()
        .typed_uint8_array()
        .constructor();
    let array = BuiltinTypedArray::allocate::<Uint8Array>(
        &constructor.into(),
        (input.len() / 2) as u64,
        context,
    )?;
    let _root = array.clone().root();
    let array = array
        .downcast::<TypedArray>()
        .expect("allocated Uint8Array has typed array data");
    with_uint8_bytes_mut(&array, input.len() / 2, |mut bytes| {
        for offset in (0..input.len()).step_by(2) {
            let byte = hex_pair(input, offset).expect("hex input was validated before allocation");
            write_byte(&mut bytes, offset / 2, byte);
        }
    });
    Ok(array.upcast().into())
}

fn hex_pair(input: JsStr<'_>, offset: usize) -> JsResult<u8> {
    let high = hex_digit(u32::from(input.get_expect(offset)))?;
    let low = hex_digit(u32::from(input.get_expect(offset + 1)))?;
    Ok((high << 4) | low)
}

fn hex_digit(point: u32) -> JsResult<u8> {
    match point {
        0x30..=0x39 => Ok((point - 0x30) as u8),
        0x41..=0x46 => Ok((point - 0x41 + 10) as u8),
        0x61..=0x66 => Ok((point - 0x61 + 10) as u8),
        _ => Err(JsNativeError::syntax()
            .with_message("invalid hexadecimal digit")
            .into()),
    }
}

/// Decodes into this view, preserving successfully decoded bytes on syntax errors.
/// <https://tc39.es/ecma262/#sec-uint8array.prototype.setfromhex>
pub(super) fn set_from_hex(
    this: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let (array, buffer_length) = TypedArray::validate(this, Ordering::SeqCst)?;
    let _root = array.clone().root();
    let length = {
        let data = array.borrow();
        if data.data().kind() != TypedArrayKind::Uint8 {
            return Err(JsNativeError::typ()
                .with_message("setFromHex requires a Uint8Array")
                .into());
        }
        data.data().array_length(buffer_length)
    };
    let input = args
        .get_or_undefined(0)
        .as_string()
        .ok_or_else(|| JsNativeError::typ().with_message("setFromHex requires a string"))?;
    if input.len() % 2 != 0 {
        return Err(JsNativeError::syntax()
            .with_message("hex string has odd length")
            .into());
    }
    let count = usize::try_from(length)
        .expect("typed array length fits in usize")
        .min(input.len() / 2);
    let input = input.as_str();
    with_uint8_bytes_mut(&array, buffer_length, |mut bytes| -> JsResult<()> {
        for index in 0..count {
            let byte = hex_pair(input, index * 2)?;
            write_byte(&mut bytes, index, byte);
        }
        Ok(())
    })?;
    Ok(crate::object::ObjectInitializer::new(context)
        .property(
            crate::js_string!("read"),
            (count * 2) as f64,
            crate::property::Attribute::all(),
        )
        .property(
            crate::js_string!("written"),
            count as f64,
            crate::property::Attribute::all(),
        )
        .build()
        .into())
}

/// Encodes this view after evaluating the alphabet and padding options.
/// <https://tc39.es/ecma262/#sec-uint8array.prototype.tobase64>
pub(super) fn to_base64(
    this: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let array = this
        .as_object()
        .and_then(|value| value.downcast::<TypedArray>().ok())
        .filter(|value| value.borrow().data().kind() == TypedArrayKind::Uint8)
        .ok_or_else(|| JsNativeError::typ().with_message("toBase64 requires a Uint8Array"))?;
    let _root = array.root();
    let options = super::super::options::get_options_object(args.get_or_undefined(0))?;
    let _options_root = options.clone().root();
    let alphabet = options.get(crate::js_string!("alphabet"), context)?;
    let url = if alphabet.is_undefined()
        || alphabet
            .as_string()
            .is_some_and(|value| value == crate::js_string!("base64"))
    {
        false
    } else if alphabet
        .as_string()
        .is_some_and(|value| value == crate::js_string!("base64url"))
    {
        true
    } else {
        return Err(JsNativeError::typ()
            .with_message("invalid base64 alphabet")
            .into());
    };
    let omit_padding = options
        .get(crate::js_string!("omitPadding"), context)?
        .to_boolean();
    let (array, buffer_length) = TypedArray::validate(this, Ordering::SeqCst)?;
    Ok(with_uint8_bytes(&array, buffer_length, |bytes| {
        encode_base64(bytes, url, omit_padding)
    })
    .into())
}

fn encode_base64(bytes: SliceRef<'_>, url: bool, omit_padding: bool) -> JsString {
    let alphabet = if url {
        b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_"
    } else {
        b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/"
    };
    let complete_length = bytes.len() / 3 * 4;
    let remainder = bytes.len() % 3;
    let tail_length = if remainder == 0 {
        0
    } else if omit_padding {
        remainder + 1
    } else {
        4
    };
    let mut result = Latin1JsStringBuilder::with_capacity(complete_length + tail_length);
    for offset in (0..bytes.len()).step_by(3) {
        let chunk_length = (bytes.len() - offset).min(3);
        let second = if chunk_length > 1 {
            read_byte(bytes, offset + 1)
        } else {
            0
        };
        let third = if chunk_length > 2 {
            read_byte(bytes, offset + 2)
        } else {
            0
        };
        let bits = (u32::from(read_byte(bytes, offset)) << 16)
            | (u32::from(second) << 8)
            | u32::from(third);
        let encoded_length = chunk_length + 1;
        for index in 0..4 {
            if index < encoded_length {
                result.push(alphabet[((bits >> (18 - index * 6)) & 63) as usize]);
            } else if !omit_padding {
                result.push(b'=');
            }
        }
    }
    result.build().expect("base64 output is ASCII")
}

fn decode_options(
    value: &JsValue,
    context: &mut Context,
) -> JsResult<(bool, super::base64_decode::LastChunk)> {
    use super::base64_decode::LastChunk;
    let options = super::super::options::get_options_object(value)?;
    let _root = options.clone().root();
    let alphabet = options.get(crate::js_string!("alphabet"), context)?;
    let url = if alphabet.is_undefined()
        || alphabet
            .as_string()
            .is_some_and(|s| s == crate::js_string!("base64"))
    {
        false
    } else if alphabet
        .as_string()
        .is_some_and(|s| s == crate::js_string!("base64url"))
    {
        true
    } else {
        return Err(JsNativeError::typ()
            .with_message("invalid base64 alphabet")
            .into());
    };
    let handling = options.get(crate::js_string!("lastChunkHandling"), context)?;
    let mode = if handling.is_undefined()
        || handling
            .as_string()
            .is_some_and(|s| s == crate::js_string!("loose"))
    {
        LastChunk::Loose
    } else if handling
        .as_string()
        .is_some_and(|s| s == crate::js_string!("strict"))
    {
        LastChunk::Strict
    } else if handling
        .as_string()
        .is_some_and(|s| s == crate::js_string!("stop-before-partial"))
    {
        LastChunk::StopBeforePartial
    } else {
        return Err(JsNativeError::typ()
            .with_message("invalid lastChunkHandling")
            .into());
    };
    Ok((url, mode))
}

/// Decodes base64 using the intrinsic Uint8Array constructor.
/// <https://tc39.es/ecma262/#sec-uint8array.frombase64>
pub(super) fn from_base64(
    _: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let input = args
        .get_or_undefined(0)
        .as_string()
        .ok_or_else(|| JsNativeError::typ().with_message("fromBase64 requires a string"))?;
    let (url, mode) = decode_options(args.get_or_undefined(1), context)?;
    let input = input.as_str();
    let result = super::base64_decode::decode(input, url, mode, usize::MAX, |_, _| {});
    if result.error {
        return Err(JsNativeError::syntax()
            .with_message("invalid base64 input")
            .into());
    }
    let constructor = context
        .intrinsics()
        .constructors()
        .typed_uint8_array()
        .constructor();
    let array = BuiltinTypedArray::allocate::<Uint8Array>(
        &constructor.into(),
        result.written as u64,
        context,
    )?;
    let _root = array.clone().root();
    let array = array
        .downcast::<TypedArray>()
        .expect("allocated Uint8Array has typed array data");
    let decoded = with_uint8_bytes_mut(&array, result.written, |mut bytes| {
        super::base64_decode::decode(input, url, mode, usize::MAX, |index, byte| {
            write_byte(&mut bytes, index, byte);
        })
    });
    debug_assert!(!decoded.error);
    debug_assert_eq!(decoded.written, result.written);
    Ok(array.upcast().into())
}

/// Decodes into this view and retains decoded bytes before a syntax error.
/// <https://tc39.es/ecma262/#sec-uint8array.prototype.setfrombase64>
pub(super) fn set_from_base64(
    this: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let array = this
        .as_object()
        .and_then(|o| o.downcast::<TypedArray>().ok())
        .filter(|o| o.borrow().data().kind() == TypedArrayKind::Uint8)
        .ok_or_else(|| JsNativeError::typ().with_message("setFromBase64 requires a Uint8Array"))?;
    let _root = array.root();
    let input = args
        .get_or_undefined(0)
        .as_string()
        .ok_or_else(|| JsNativeError::typ().with_message("setFromBase64 requires a string"))?;
    let (url, mode) = decode_options(args.get_or_undefined(1), context)?;
    let (array, buffer_length) = TypedArray::validate(this, Ordering::SeqCst)?;
    let length = usize::try_from(array.borrow().data().array_length(buffer_length))
        .expect("typed array length fits in usize");
    if length == 0 {
        return Ok(decoded_count_object(0, 0, context));
    }
    let input = input.as_str();
    let result = with_uint8_bytes_mut(&array, buffer_length, |mut bytes| {
        super::base64_decode::decode(input, url, mode, length, |index, byte| {
            write_byte(&mut bytes, index, byte);
        })
    });
    if result.error {
        return Err(JsNativeError::syntax()
            .with_message("invalid base64 input")
            .into());
    }
    Ok(decoded_count_object(result.read, result.written, context))
}

fn decoded_count_object(read: usize, written: usize, context: &mut Context) -> JsValue {
    crate::object::ObjectInitializer::new(context)
        .property(
            crate::js_string!("read"),
            read as f64,
            crate::property::Attribute::all(),
        )
        .property(
            crate::js_string!("written"),
            written as f64,
            crate::property::Attribute::all(),
        )
        .build()
        .into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encoders_build_exact_latin1_strings() {
        let hex = encode_hex(SliceRef::Slice(&[0, 15, 255]));
        assert_eq!(hex, "000fff");
        assert_eq!(hex.capacity(), Some(hex.len()));

        let base64 = encode_base64(SliceRef::Slice(b"foobar"), false, false);
        assert_eq!(base64, "Zm9vYmFy");
        assert_eq!(base64.capacity(), Some(base64.len()));

        let unpadded = encode_base64(SliceRef::Slice(b"fo"), true, true);
        assert_eq!(unpadded, "Zm8");
        assert_eq!(unpadded.capacity(), Some(unpadded.len()));
    }
}
