//! Bearer-token shapes: header parsing, the stored token hash, and scopes.
//!
//! Tokens are opaque `cabin_<base62>` strings; the database only ever stores
//! the SHA-256 hex of the full token, so a leaked database cannot be replayed
//! against the registry.

use std::fmt::Write as _;

use sha2::{Digest, Sha256};

/// A scope a token row may carry. Reads require no scope: any valid,
/// unrevoked token grants read access. Unknown scope strings grant nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    Publish,
    Yank,
}

/// What authentication attaches to a request once a token row matched.
#[derive(Debug)]
pub struct AuthContext {
    /// The token row id - safe to log, unlike the token or its hash.
    pub token_id: String,
    pub user_id: i64,
    pub scopes: Vec<Scope>,
    /// The user's quota plan (`users.plan`); `crate::quota` maps it to
    /// the enforced limits.
    pub plan: String,
    /// Publish token-bucket state from the token row, `None` for a token
    /// that has never published.
    pub bucket: Option<crate::quota::Bucket>,
}

/// Extracts the token from an `Authorization` header value, accepting only
/// the `Bearer` scheme (ASCII case-insensitive, per RFC 7235).
pub fn bearer_token(header: &str) -> Option<&str> {
    let (scheme, token) = header.split_once(' ')?;
    let token = token.trim();
    (scheme.eq_ignore_ascii_case("bearer") && !token.is_empty()).then_some(token)
}

/// Lowercase SHA-256 hex of the full token string - the `tokens.token_hash`
/// column value.
pub fn token_hash(token: &str) -> String {
    hex(&Sha256::digest(token.as_bytes()))
}

/// Lowercase hex of `bytes`.
pub fn hex(bytes: &[u8]) -> String {
    bytes
        .iter()
        .fold(String::with_capacity(bytes.len() * 2), |mut hex, byte| {
            let _ = write!(hex, "{byte:02x}");
            hex
        })
}

/// How many CSPRNG bytes back a freshly issued token.
pub const TOKEN_RANDOM_BYTES: usize = 32;

/// Base62 digits needed for 32 bytes: `ceil(256 / log2(62))`.
const TOKEN_BASE62_LEN: usize = 43;

const BASE62: &[u8; 62] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";

/// Formats a freshly issued token from CSPRNG bytes (the wasm glue draws
/// them from `crypto.getRandomValues`): `cabin_` plus the fixed-width
/// base62 rendering of the bytes as one big-endian integer, so all 256
/// bits survive verbatim and the charset stays URL- and header-safe.
#[allow(clippy::cast_possible_truncation)] // every div-mod quotient is < 256
pub fn format_token(bytes: &[u8; TOKEN_RANDOM_BYTES]) -> String {
    let mut num = *bytes;
    let mut digits = [0u8; TOKEN_BASE62_LEN];
    for digit in digits.iter_mut().rev() {
        // One big-integer div-mod by 62 over the big-endian byte string.
        let mut rem = 0u32;
        for byte in &mut num {
            let value = rem * 256 + u32::from(*byte);
            *byte = (value / 62) as u8;
            rem = value % 62;
        }
        *digit = BASE62[rem as usize];
    }
    let mut token = String::with_capacity("cabin_".len() + TOKEN_BASE62_LEN);
    token.push_str("cabin_");
    token.extend(digits.iter().map(|&digit| char::from(digit)));
    token
}

/// Parses the comma-separated `tokens.scopes` column, ignoring unknown names
/// (deny by default: an unknown scope never grants anything).
pub fn parse_scopes(scopes: &str) -> Vec<Scope> {
    scopes
        .split(',')
        .filter_map(|scope| match scope.trim() {
            "publish" => Some(Scope::Publish),
            "yank" => Some(Scope::Yank),
            _ => None,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bearer_token_parses_the_scheme_case_insensitively() {
        assert_eq!(bearer_token("Bearer cabin_abc"), Some("cabin_abc"));
        assert_eq!(bearer_token("bearer cabin_abc"), Some("cabin_abc"));
        assert_eq!(bearer_token("BEARER cabin_abc"), Some("cabin_abc"));
    }

    #[test]
    fn bearer_token_rejects_other_shapes() {
        assert_eq!(bearer_token(""), None);
        assert_eq!(bearer_token("Bearer"), None);
        assert_eq!(bearer_token("Bearer "), None);
        assert_eq!(bearer_token("Bearer  "), None);
        assert_eq!(bearer_token("Basic cabin_abc"), None);
        assert_eq!(bearer_token("cabin_abc"), None);
    }

    #[test]
    fn token_hash_is_lowercase_sha256_hex_of_the_full_string() {
        // Known SHA-256 vector.
        assert_eq!(
            token_hash("abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        let hash = token_hash("cabin_0123456789");
        assert_eq!(hash.len(), 64);
        assert!(
            hash.bytes()
                .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
        );
    }

    #[test]
    fn format_token_has_the_documented_shape() {
        let token = format_token(&[0xA5; 32]);
        assert_eq!(token.len(), "cabin_".len() + 43);
        let digits = token.strip_prefix("cabin_").expect("cabin_ prefix");
        assert!(
            digits.bytes().all(|b| BASE62.contains(&b)),
            "token: {token}"
        );
    }

    #[test]
    fn format_token_renders_known_values() {
        // The bytes are one big-endian integer, fixed-width with leading
        // zero digits preserved.
        let zeros = format_token(&[0; 32]);
        assert_eq!(zeros, format!("cabin_{}", "0".repeat(43)));
        let mut bytes = [0u8; 32];
        bytes[31] = 61;
        assert_eq!(format_token(&bytes), format!("cabin_{}z", "0".repeat(42)));
        bytes[31] = 62;
        assert_eq!(format_token(&bytes), format!("cabin_{}10", "0".repeat(41)));
    }

    #[test]
    fn format_token_consumes_every_input_byte() {
        // Flipping any single byte must change the token: the whole
        // 32-byte CSPRNG draw ends up in the rendered secret.
        let baseline = format_token(&[0; 32]);
        for position in 0..32 {
            let mut bytes = [0u8; 32];
            bytes[position] = 1;
            assert_ne!(format_token(&bytes), baseline, "byte {position}");
        }
    }

    #[test]
    fn parse_scopes_keeps_known_names_and_drops_the_rest() {
        assert_eq!(
            parse_scopes("publish,yank"),
            vec![Scope::Publish, Scope::Yank]
        );
        assert_eq!(
            parse_scopes(" publish , yank "),
            vec![Scope::Publish, Scope::Yank]
        );
        assert_eq!(parse_scopes("yank"), vec![Scope::Yank]);
        assert_eq!(parse_scopes(""), vec![]);
        assert_eq!(parse_scopes("admin,PUBLISH"), vec![]);
    }
}
