use sha2::{Digest, Sha256};

use super::{ErrorCategory, ErrorCode, SafeContext};

/// The canonical fingerprint input format. Bump this when its fields or
/// normalization rules change; existing database rows keep their old prefix.
const DOMAIN: &[u8] = b"omoikane-error-fingerprint-v1\0";

pub(super) fn compute(category: ErrorCategory, code: ErrorCode, context: &SafeContext) -> String {
    let digest = Sha256::digest(canonical_input(category, code, context));
    let mut output = String::with_capacity(3 + digest.len() * 2);
    output.push_str("v1:");
    for byte in digest {
        use std::fmt::Write;
        write!(&mut output, "{byte:02x}").expect("writing to String cannot fail");
    }
    output
}

pub(super) fn canonical_input(
    category: ErrorCategory,
    code: ErrorCode,
    context: &SafeContext,
) -> Vec<u8> {
    let mut bytes = DOMAIN.to_vec();
    append_field(&mut bytes, category.as_str());
    append_field(&mut bytes, code.as_str());
    for (key, value) in context.values() {
        append_field(&mut bytes, key);
        append_field(&mut bytes, value);
    }
    bytes
}

fn append_field(output: &mut Vec<u8>, field: &str) {
    let len = u32::try_from(field.len()).expect("allowlisted fingerprint field fits in u32");
    output.extend_from_slice(&len.to_be_bytes());
    output.extend_from_slice(field.as_bytes());
}
