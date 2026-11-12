//! Pairing code format.
//!
//! Codes are six characters from an unambiguous alphabet (no `0`, `1`, `I`,
//! `L`, `O` or `U`), displayed in two groups of three: `K7Q-4MZ`. The human
//! reads the code on the device and approves it on the server's web page, so
//! the format is chosen for reading aloud and typing on a phone.

/// Characters allowed in a pairing code. 30 symbols.
pub const PAIRING_CODE_ALPHABET: &[u8] = b"23456789ABCDEFGHJKMNPQRSTVWXYZ";

/// Number of symbols in a pairing code.
pub const PAIRING_CODE_LEN: usize = 6;

/// Pairing codes expire after ten minutes (the protocol allows at most that).
pub const PAIRING_TTL_SECONDS: i64 = 10 * 60;

/// Build a code from the first six bytes of `bytes`.
///
/// Returns `None` when fewer than six bytes are supplied. Callers pass bytes
/// from a cryptographic random source; tests pass fixed bytes.
pub fn pairing_code_from_bytes(bytes: &[u8]) -> Option<String> {
    if bytes.len() < PAIRING_CODE_LEN {
        return None;
    }
    let mut chars = String::with_capacity(PAIRING_CODE_LEN + 1);
    for (index, byte) in bytes.iter().take(PAIRING_CODE_LEN).enumerate() {
        if index == 3 {
            chars.push('-');
        }
        let symbol = PAIRING_CODE_ALPHABET[(*byte as usize) % PAIRING_CODE_ALPHABET.len()];
        chars.push(symbol as char);
    }
    Some(chars)
}

/// Format six symbols as `XXX-XXX`. Accepts lower case and either with or
/// without the dash; returns `None` when the input is not a valid code.
pub fn normalize_pairing_code(input: &str) -> Option<String> {
    let mut symbols = String::with_capacity(PAIRING_CODE_LEN);
    for byte in input.bytes() {
        if byte == b'-' || byte == b' ' {
            continue;
        }
        let upper = byte.to_ascii_uppercase();
        if !PAIRING_CODE_ALPHABET.contains(&upper) {
            return None;
        }
        symbols.push(upper as char);
    }
    if symbols.len() != PAIRING_CODE_LEN {
        return None;
    }
    Some(format!("{}-{}", &symbols[..3], &symbols[3..]))
}

/// True when both strings are valid codes that represent the same code.
pub fn pairing_code_matches(left: &str, right: &str) -> bool {
    match (normalize_pairing_code(left), normalize_pairing_code(right)) {
        (Some(left), Some(right)) => left == right,
        _ => false,
    }
}

/// Alias for [`normalize_pairing_code`], kept because "format" reads better at
/// call sites that already have symbols.
pub fn format_pairing_code(input: &str) -> Option<String> {
    normalize_pairing_code(input)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn code_has_expected_shape() {
        let code = pairing_code_from_bytes(&[0, 1, 2, 3, 4, 5]).unwrap();
        assert_eq!(code.len(), 7);
        assert_eq!(code.as_bytes()[3], b'-');
        assert!(code
            .bytes()
            .filter(|b| *b != b'-')
            .all(|b| PAIRING_CODE_ALPHABET.contains(&b)));
    }

    #[test]
    fn code_is_deterministic() {
        let bytes = [7u8, 31, 255, 0, 88, 12, 9];
        let code = pairing_code_from_bytes(&bytes).unwrap();
        assert_eq!(code, pairing_code_from_bytes(&bytes).unwrap());
        assert_eq!(normalize_pairing_code(&code).unwrap(), code);
        assert_eq!(normalize_pairing_code(&code.to_lowercase()).unwrap(), code);
        assert_eq!(
            normalize_pairing_code(&code.replace('-', "")).unwrap(),
            code
        );
    }

    #[test]
    fn short_input_is_rejected() {
        assert!(pairing_code_from_bytes(&[1, 2, 3]).is_none());
    }

    #[test]
    fn normalization_rejects_ambiguous_symbols() {
        // 0, 1, I, L, O, U are not in the alphabet.
        for bad in ["0ABC23", "1ABC23", "IABC23", "LABC23", "OABC23", "UABC23"] {
            assert!(normalize_pairing_code(bad).is_none(), "{bad}");
        }
        assert!(normalize_pairing_code("ABC").is_none());
        assert!(normalize_pairing_code("ABCDEFG").is_none());
        assert!(normalize_pairing_code("K7Q-4MZ").is_some());
        assert!(normalize_pairing_code("k7q 4mz").is_some());
    }

    #[test]
    fn matching_ignores_case_and_dash() {
        assert!(pairing_code_matches("K7Q-4MZ", "k7q4mz"));
        assert!(!pairing_code_matches("K7Q-4MZ", "K7Q-4MY"));
        assert!(!pairing_code_matches("nope", "K7Q-4MZ"));
    }
}
