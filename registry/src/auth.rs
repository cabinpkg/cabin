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
    Sha256::digest(token.as_bytes())
        .iter()
        .fold(String::with_capacity(64), |mut hex, byte| {
            let _ = write!(hex, "{byte:02x}");
            hex
        })
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
