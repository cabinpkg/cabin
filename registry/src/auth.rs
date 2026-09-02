//! Bearer-token shapes: header parsing, the stored token hash, and scopes.
//!
//! Tokens are opaque `cabin_tp_`/`cabin_ses_`-prefixed strings; the database
//! only ever stores the SHA-256 hex of the full token, so a leaked database
//! cannot be replayed against the registry.

use std::fmt::Write as _;

use sha2::{Digest, Sha256};

/// A scope a token row may carry. Reads require no scope: any valid,
/// unrevoked token grants read access. Unknown scope strings grant nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    Publish,
    Yank,
    /// The operator's admin-plane scope: list pending versions,
    /// download their artifacts, and drive the governor. Verdicts take
    /// no registry token - their credential is the verify workflow's
    /// OIDC JWT (`crate::trustpub::VERIFIER_AUDIENCE`).
    Verify,
}

/// What authentication attaches to a request once a token row matched.
#[derive(Debug)]
pub struct AuthContext {
    /// The token row id - safe to log, unlike the token or its hash.
    pub token_id: String,
    /// The registry-native `users.id` the token belongs to - never a
    /// provider account id, which lives only in `identities`.
    pub user_id: i64,
    pub scopes: Vec<Scope>,
    /// The user's quota class (`users.quota_class`); `crate::quota` maps
    /// it to the enforced limits.
    pub quota_class: String,
    /// `tokens.scope_limit`: `None` is an unlimited token; `Some` confines
    /// every write-side operation to packages under exactly that scope
    /// (see [`AuthContext::scope_limit_refuses`]).
    pub scope_limit: Option<String>,
    /// The owning user's own class (`users.quota_class`), which sizes
    /// the publish bucket: the bucket lives on the user row and every
    /// token the user holds draws on it, so a token-level class grant
    /// (`quota_class` above) raises resource limits but cannot resize
    /// it - two rates on one balance would let one token refill what
    /// another spends.
    pub user_quota_class: String,
    /// Publish token-bucket state from the user row, `None` for a user
    /// that has never published.
    pub bucket: Option<crate::quota::Bucket>,
}

impl AuthContext {
    /// Whether the token's `scope_limit` forbids a write under `scope`.
    /// Compares only against the token's own row, so the refusal can
    /// never become an oracle about the registry; callers answer it
    /// with the same uniform 403 as a scope-membership miss. Read-side
    /// routes never consult this.
    pub fn scope_limit_refuses(&self, scope: &str) -> bool {
        self.scope_limit
            .as_deref()
            .is_some_and(|limit| limit != scope)
    }
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

/// Formats a trusted-publishing exchange token: `cabin_tp_` plus the
/// base64url (unpadded) rendering of the CSPRNG bytes (the wasm glue
/// draws them from `crypto.getRandomValues`). The distinct prefix
/// keeps a leaked CI log grep-ably different from a login-session
/// token; nothing parses the shape back - authentication is the
/// hash lookup either way.
pub fn format_trustpub_token(bytes: &[u8; TOKEN_RANDOM_BYTES]) -> String {
    use base64ct::{Base64UrlUnpadded, Encoding as _};
    format!("cabin_tp_{}", Base64UrlUnpadded::encode_string(bytes))
}

/// Formats a login-session token: `cabin_ses_` plus the base64url
/// (unpadded) rendering of the CSPRNG bytes - [`format_trustpub_token`]'s
/// shape with its own grep-able prefix, for the same reason.
pub fn format_session_token(bytes: &[u8; TOKEN_RANDOM_BYTES]) -> String {
    use base64ct::{Base64UrlUnpadded, Encoding as _};
    format!("cabin_ses_{}", Base64UrlUnpadded::encode_string(bytes))
}

/// Parses the comma-separated `tokens.scopes` column, ignoring unknown names
/// (deny by default: an unknown scope never grants anything).
pub fn parse_scopes(scopes: &str) -> Vec<Scope> {
    scopes
        .split(',')
        .filter_map(|scope| match scope.trim() {
            "publish" => Some(Scope::Publish),
            "yank" => Some(Scope::Yank),
            "verify" => Some(Scope::Verify),
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
    fn format_trustpub_token_has_the_documented_shape() {
        // 32 bytes render as ceil(32 / 3) * 4 - 1 = 43 unpadded
        // base64url characters.
        let token = format_trustpub_token(&[0xA5; 32]);
        let digits = token.strip_prefix("cabin_tp_").expect("cabin_tp_ prefix");
        assert_eq!(digits.len(), 43);
        assert!(
            digits
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_'),
            "token: {token}"
        );
        // Every input byte reaches the rendered secret.
        let baseline = format_trustpub_token(&[0; 32]);
        for position in 0..32 {
            let mut bytes = [0u8; 32];
            bytes[position] = 1;
            assert_ne!(format_trustpub_token(&bytes), baseline, "byte {position}");
        }
    }

    #[test]
    fn format_session_token_has_the_documented_shape() {
        let token = format_session_token(&[0xA5; 32]);
        let digits = token.strip_prefix("cabin_ses_").expect("cabin_ses_ prefix");
        assert_eq!(digits.len(), 43);
        assert!(
            digits
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_'),
            "token: {token}"
        );
        // Every input byte reaches the rendered secret.
        let baseline = format_session_token(&[0; 32]);
        for position in 0..32 {
            let mut bytes = [0u8; 32];
            bytes[position] = 1;
            assert_ne!(format_session_token(&bytes), baseline, "byte {position}");
        }
    }

    #[test]
    fn scope_limit_refuses_only_a_mismatched_scope() {
        let auth = |scope_limit: Option<&str>| AuthContext {
            token_id: "t".to_owned(),
            user_id: 1,
            scopes: vec![Scope::Publish],
            quota_class: "default".to_owned(),
            scope_limit: scope_limit.map(str::to_owned),
            user_quota_class: "default".to_owned(),
            bucket: None,
        };
        // An unlimited token writes anywhere.
        assert!(!auth(None).scope_limit_refuses("cabin-ports"));
        assert!(!auth(Some("cabin-ports")).scope_limit_refuses("cabin-ports"));
        assert!(auth(Some("cabin-ports")).scope_limit_refuses("other"));
        // Exact string equality: no prefix or case slack.
        assert!(auth(Some("cabin-ports")).scope_limit_refuses("cabin-port"));
        assert!(auth(Some("cabin-ports")).scope_limit_refuses("Cabin-Ports"));
        assert!(auth(Some("")).scope_limit_refuses("cabin-ports"));
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
        assert_eq!(parse_scopes("verify"), vec![Scope::Verify]);
        assert_eq!(
            parse_scopes("publish,yank,verify"),
            vec![Scope::Publish, Scope::Yank, Scope::Verify]
        );
        assert_eq!(parse_scopes(""), vec![]);
        assert_eq!(parse_scopes("admin,PUBLISH,VERIFY"), vec![]);
    }
}
