// declared_role: accessor, formatter, mapper, parser, predicate, validator
// intrinsic_surface_declarations:
//   - component: src/encoding.rs
//     role: intrinsic-surface
//     Domain: provider_byte_hash_time_primitives
//     Owns:
//       - "base64 encode/decode"
//       - "sha256_hex"
//       - "now_unix_ms"

use chrono::Utc;
use sha2::{Digest, Sha256};

const BASE64_TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

pub fn encode_base64(bytes: &[u8]) -> String {
    let mut out = String::new();
    for chunk in bytes.chunks(3) {
        let b0 = chunk[0];
        let b1 = *chunk.get(1).unwrap_or(&0);
        let b2 = *chunk.get(2).unwrap_or(&0);
        out.push(BASE64_TABLE[(b0 >> 2) as usize] as char);
        out.push(BASE64_TABLE[(((b0 & 0b0000_0011) << 4) | (b1 >> 4)) as usize] as char);
        out.push(if chunk.len() > 1 {
            BASE64_TABLE[(((b1 & 0b0000_1111) << 2) | (b2 >> 6)) as usize] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            BASE64_TABLE[(b2 & 0b0011_1111) as usize] as char
        } else {
            '='
        });
    }
    out
}

pub fn decode_base64(data: &str) -> Result<Vec<u8>, String> {
    let bytes = data.as_bytes();
    validate_base64_length(bytes)?;

    let mut out = Vec::with_capacity(bytes.len() / 4 * 3);
    for (chunk_index, chunk) in bytes.chunks(4).enumerate() {
        let decoded = decode_base64_chunk(chunk, is_final_chunk(chunk_index, bytes))?;
        append_decoded_base64_chunk(&mut out, decoded);
    }

    Ok(out)
}

#[derive(Clone, Copy)]
struct DecodedBase64Chunk {
    values: [u8; 4],
    padding: usize,
}

fn validate_base64_length(bytes: &[u8]) -> Result<(), String> {
    if bytes.len().is_multiple_of(4) {
        Ok(())
    } else {
        Err("base64 length must be a multiple of 4".to_string())
    }
}

fn is_final_chunk(chunk_index: usize, bytes: &[u8]) -> bool {
    chunk_index == bytes.len() / 4 - 1
}

fn decode_base64_chunk(chunk: &[u8], is_last: bool) -> Result<DecodedBase64Chunk, String> {
    let mut values = [0u8; 4];
    let mut padding_started = false;
    let mut padding = 0usize;

    for (index, byte) in chunk.iter().copied().enumerate() {
        let digit = decode_base64_digit(byte, is_last, index, padding_started)?;
        if digit.padded {
            padding_started = true;
            padding += 1;
        }
        values[index] = digit.value;
    }

    validate_base64_padding_count(padding)?;
    Ok(DecodedBase64Chunk { values, padding })
}

struct Base64Digit {
    value: u8,
    padded: bool,
}

fn decode_base64_digit(
    byte: u8,
    is_last_chunk: bool,
    index: usize,
    padding_started: bool,
) -> Result<Base64Digit, String> {
    if byte == b'=' {
        validate_base64_padding_position(is_last_chunk, index)?;
        return Ok(Base64Digit {
            value: 0,
            padded: true,
        });
    }
    validate_base64_non_padding_position(padding_started)?;
    let value = base64_value(byte).ok_or_else(|| "invalid base64 character".to_string())?;
    Ok(Base64Digit {
        value,
        padded: false,
    })
}

fn validate_base64_padding_position(is_last_chunk: bool, index: usize) -> Result<(), String> {
    if is_last_chunk && index >= 2 {
        Ok(())
    } else {
        Err("invalid base64 padding".to_string())
    }
}

fn validate_base64_non_padding_position(padding_started: bool) -> Result<(), String> {
    if padding_started {
        Err("invalid base64 padding".to_string())
    } else {
        Ok(())
    }
}

fn validate_base64_padding_count(padding: usize) -> Result<(), String> {
    if padding <= 2 {
        Ok(())
    } else {
        Err("invalid base64 padding".to_string())
    }
}

fn append_decoded_base64_chunk(out: &mut Vec<u8>, decoded: DecodedBase64Chunk) {
    let values = decoded.values;
    out.push((values[0] << 2) | (values[1] >> 4));
    if decoded.padding < 2 {
        out.push((values[1] << 4) | (values[2] >> 2));
    }
    if decoded.padding < 1 {
        out.push((values[2] << 6) | values[3]);
    }
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

pub fn now_unix_ms() -> u64 {
    Utc::now().timestamp_millis().max(0) as u64
}

fn base64_value(byte: u8) -> Option<u8> {
    match byte {
        b'A'..=b'Z' => Some(byte - b'A'),
        b'a'..=b'z' => Some(byte - b'a' + 26),
        b'0'..=b'9' => Some(byte - b'0' + 52),
        b'+' => Some(62),
        b'/' => Some(63),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_roundtrip_preserves_bytes() {
        let bytes = b"hello\0provider\xff";
        let encoded = encode_base64(bytes);

        assert_eq!(
            decode_base64(&encoded).expect("encoded bytes decode"),
            bytes
        );
    }

    #[test]
    fn base64_rejects_invalid_input() {
        assert!(decode_base64("not valid!").is_err());
        assert!(decode_base64("abc").is_err());
    }

    #[test]
    fn sha256_hex_matches_known_vector() {
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn now_unix_ms_is_monotone_non_decreasing() {
        let first = now_unix_ms();
        let second = now_unix_ms();

        assert!(second >= first);
    }
}
