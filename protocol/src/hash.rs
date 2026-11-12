//! Content hashing helpers.
//!
//! The protocol defines a file's content hash as the lowercase hex SHA-256 of
//! its UTF-8 bytes. The same function is used for certificate fingerprints.

use sha2::{Digest, Sha256};

/// Lowercase hex SHA-256 of arbitrary bytes.
pub fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    hex_encode(&digest)
}

/// Content hash of a file, exactly as defined by the protocol.
pub fn content_hash(content: &[u8]) -> String {
    sha256_hex(content)
}

/// True when `content` hashes to `expected` (case-insensitive).
pub fn verify_content_hash(content: &[u8], expected: &str) -> bool {
    let actual = content_hash(content);
    actual.eq_ignore_ascii_case(expected)
}

/// Lowercase hexadecimal encoding.
pub fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0x0f) as usize] as char);
    }
    out
}

/// Decode a hexadecimal string. Accepts upper and lower case; rejects odd
/// lengths and non-hex characters.
pub fn hex_decode(input: &str) -> Result<Vec<u8>, HexError> {
    if !input.len().is_multiple_of(2) {
        return Err(HexError::OddLength);
    }
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len() / 2);
    for pair in bytes.as_chunks::<2>().0 {
        let hi = hex_value(pair[0]).ok_or(HexError::InvalidCharacter)?;
        let lo = hex_value(pair[1]).ok_or(HexError::InvalidCharacter)?;
        out.push((hi << 4) | lo);
    }
    Ok(out)
}

/// Decode a 32-byte SHA-256 digest from hex.
pub fn decode_sha256_hex(input: &str) -> Result<[u8; 32], HexError> {
    let bytes = hex_decode(input)?;
    bytes.try_into().map_err(|_| HexError::WrongLength)
}

fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

/// Errors from [`hex_decode`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HexError {
    OddLength,
    InvalidCharacter,
    WrongLength,
}

impl std::fmt::Display for HexError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            HexError::OddLength => f.write_str("hex string has an odd number of characters"),
            HexError::InvalidCharacter => f.write_str("hex string contains a non-hex character"),
            HexError::WrongLength => f.write_str("hex string has the wrong length"),
        }
    }
}

impl std::error::Error for HexError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sha256_matches_known_vector() {
        assert_eq!(
            content_hash(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(
            content_hash(b"hello\n"),
            "5891b5b522d5df086d0ff0b110fbd9d21bb4fc7163af34d08286a2e846f6be03"
        );
    }

    #[test]
    fn verify_is_case_insensitive() {
        let hash = content_hash(b"stylos");
        assert!(verify_content_hash(b"stylos", &hash));
        assert!(verify_content_hash(b"stylos", &hash.to_uppercase()));
        assert!(!verify_content_hash(b"other", &hash));
    }

    #[test]
    fn hex_roundtrip() {
        let raw = [0u8, 1, 15, 16, 127, 128, 255];
        let encoded = hex_encode(&raw);
        assert_eq!(encoded, "00010f107f80ff");
        assert_eq!(hex_decode(&encoded).unwrap(), raw);
        assert_eq!(hex_decode("00FF").unwrap(), vec![0, 255]);
        assert_eq!(hex_decode("abc"), Err(HexError::OddLength));
        assert_eq!(hex_decode("zz"), Err(HexError::InvalidCharacter));
    }

    #[test]
    fn sha256_decoding() {
        let hash = content_hash(b"x");
        let pin = decode_sha256_hex(&hash).unwrap();
        assert_eq!(pin.len(), 32);
        assert!(decode_sha256_hex("00").is_err());
    }
}
