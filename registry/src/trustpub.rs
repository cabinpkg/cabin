//! GitHub Actions OIDC token verification for trusted publishing: the
//! exchange endpoint (future work) trades a verified Actions token for a
//! short-lived `trustpub` token bound to a `trustpub_configs` row
//! (`registry/docs/architecture.md`, "D1 is canonical").
//!
//! The JWT handling is deliberately manual and `RS256`-only: GitHub signs
//! Actions tokens with RSA keys published at a fixed JWKS URL, and a
//! hand-rolled verifier over the pure-Rust `rsa` crate is one code path
//! that runs natively in tests and on wasm32 alike - `ring`-backed JWT
//! crates do not build for the Worker target, and `SubtleCrypto` would
//! leave the verification logic untestable off wasm.

use base64ct::{Base64UrlUnpadded, Encoding as _};
use rsa::pkcs1v15::{Signature, VerifyingKey};
use rsa::signature::Verifier as _;
use rsa::{BoxedUint, RsaPublicKey};
use serde::Deserialize;
use sha2::Sha256;

/// The only issuer this module accepts; a token minted by any other
/// `OpenID` provider fails closed regardless of its signature.
pub const GITHUB_OIDC_ISSUER: &str = "https://token.actions.githubusercontent.com";

/// Where GitHub publishes the RSA keys that sign Actions OIDC tokens.
pub const GITHUB_JWKS_URL: &str = "https://token.actions.githubusercontent.com/.well-known/jwks";

/// The Cache API key for the fetched JWKS. A synthetic same-zone URL,
/// NOT [`GITHUB_JWKS_URL`]: the Cache API silently refuses to store
/// entries under an out-of-zone URL, which would turn every `put` into
/// a logged no-op and every verification into an origin fetch. Same
/// pattern as the artifact cache's `__cache` identity
/// (`registry/docs/architecture.md`, "The read plane and the edge
/// cache").
#[cfg(target_arch = "wasm32")]
const JWKS_CACHE_KEY: &str = "https://registry.cabinpkg.com/__cache/github-jwks";

/// The audience the registry's own exchange expects. Callers pass the
/// audience explicitly so a future verifier-pipeline audience can reuse
/// the module; this constant is the registry default, not a baked-in rule.
pub const DEFAULT_AUDIENCE: &str = "cabinpkg.com";

/// Clock skew tolerated on `exp` and `nbf`, in seconds.
const LEEWAY_SECONDS: i64 = 60;

/// How long a fetched JWKS stays served from the Cache API before the
/// next request refetches it. Short enough that GitHub key rotation is
/// picked up promptly; an unknown `kid` additionally forces one
/// cache-bypass refetch, so rotation inside the TTL still verifies.
#[cfg(target_arch = "wasm32")]
const JWKS_CACHE_TTL_SECS: u32 = 600;

/// Why a token was refused. Variants are deliberately distinct so the
/// future exchange endpoint can log a precise reason while answering the
/// client with one uniform refusal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VerifyError {
    /// The token is not three base64url-encoded segments of JSON.
    Malformed,
    /// The header's `alg` is not exactly the string `RS256` (`none`, a
    /// missing `alg`, and type garbage included).
    Algorithm,
    /// The header carries no usable `kid` (absent or not a string).
    MissingKid,
    /// The `kid` is in neither the cached JWKS nor a fresh refetch.
    UnknownKid,
    /// The JWK selected by `kid` is not a usable RSA public key.
    Key,
    /// The RSA signature does not verify over the token's own bytes.
    Signature,
    /// `iss` is not [`GITHUB_OIDC_ISSUER`].
    Issuer,
    /// `aud` does not contain the expected audience.
    Audience,
    /// `exp` is more than the leeway in the past.
    Expired,
    /// `nbf` is more than the leeway in the future.
    NotYetValid,
    /// A required claim is absent (the claim name is the payload).
    MissingClaim(&'static str),
    /// A claim is present but unusable, e.g. a non-numeric repository id.
    InvalidClaim(&'static str),
    /// The JWKS provider itself failed (fetch error, bad status,
    /// unparsable body).
    Provider(String),
}

/// The claims the exchange consumes, present and validated. Numeric ids
/// are the immutable GitHub identifiers the `trustpub_configs` schema
/// binds against; names are display-only there and deliberately not
/// carried here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GithubClaims {
    /// The token's unique id, the once-only replay guard's key
    /// (`trustpub_used_jtis`).
    pub jti: String,
    /// When the token stops verifying (Unix seconds): the authenticated
    /// `exp` plus the acceptance leeway, not the raw claim. The exchange
    /// writes it to `trustpub_used_jtis.expires_at` so a replay row
    /// outlives every instant at which [`verify`] would still accept the
    /// token - storing raw `exp` (or re-parsing the payload, or guessing
    /// a retention window) would reopen replay inside the leeway.
    pub verifiable_until: i64,
    pub repository_id: i64,
    pub repository_owner_id: i64,
    /// `owner/repo/.github/workflows/<file>@<ref>`.
    pub workflow_ref: String,
    /// The `ref` claim: the git ref the workflow ran on.
    pub git_ref: String,
    pub environment: Option<String>,
}

/// A JWKS document. Individual keys are lenient by design: a non-RSA or
/// otherwise alien key elsewhere in GitHub's set must not make the whole
/// set unusable, so per-key requirements are checked only on the key the
/// token's `kid` selects.
#[derive(Debug, Clone, Deserialize)]
pub struct JwkSet {
    keys: Vec<Jwk>,
}

#[derive(Debug, Clone, Deserialize)]
struct Jwk {
    #[serde(default)]
    kty: Option<String>,
    #[serde(default)]
    kid: Option<String>,
    #[serde(default)]
    n: Option<String>,
    #[serde(default)]
    e: Option<String>,
}

/// The source of GitHub's signing keys. Production is the Cache-API-backed
/// [`GithubJwks`]; tests substitute a static fixture.
// async fn without a Send bound is fine here: the Worker runtime is
// single-threaded, and the host-side consumer is the test executor.
#[allow(async_fn_in_trait)]
pub trait JwksProvider {
    /// The current key set. `bypass_cache` forces a fetch from the
    /// origin, skipping (and refreshing) any cached copy - the unknown-kid
    /// retry path.
    ///
    /// # Errors
    ///
    /// [`VerifyError::Provider`] when the set cannot be produced.
    async fn jwks(&self, bypass_cache: bool) -> Result<JwkSet, VerifyError>;
}

/// Verifies a GitHub Actions OIDC token end to end: structure, `RS256`
/// signature against the provider's JWKS (with one cache-bypass refetch
/// when the `kid` is unknown), then claims in a fixed order - issuer,
/// audience, `exp`/`nbf` with leeway against the injected clock, and the
/// required-claim set.
///
/// `now_unix_seconds` is the caller's clock; the module never reads time
/// itself, so tests pin it and the wasm caller derives it from
/// `Date::now` - which yields MILLISECONDS, so divide by 1000 first or
/// every token reads as expired.
///
/// # Errors
///
/// A [`VerifyError`] naming the first check that failed.
pub async fn verify(
    token: &str,
    provider: &impl JwksProvider,
    expected_audience: &str,
    now_unix_seconds: i64,
) -> Result<GithubClaims, VerifyError> {
    let parts: Vec<&str> = token.split('.').collect();
    let [header_b64, payload_b64, signature_b64] = parts.as_slice() else {
        return Err(VerifyError::Malformed);
    };

    // Header fields are extracted individually, like the claims below,
    // so a wrong-typed `kid` cannot preempt the `alg` check with a
    // `Malformed` answer.
    let header: serde_json::Map<String, serde_json::Value> =
        serde_json::from_slice(&b64url(header_b64)?).map_err(|_| VerifyError::Malformed)?;
    if header.get("alg").and_then(serde_json::Value::as_str) != Some("RS256") {
        return Err(VerifyError::Algorithm);
    }
    let kid = header
        .get("kid")
        .and_then(serde_json::Value::as_str)
        .ok_or(VerifyError::MissingKid)?;

    let mut set = provider.jwks(false).await?;
    if find_key(&set, kid).is_none() {
        // The one deliberate rotation-recovery refetch. A hostile caller
        // can force this origin fetch with a fresh random `kid` per
        // request, so the exchange endpoint must keep its callers behind
        // the registry's admission control / rate limiting before this
        // verifier gets public traffic - coalescing or negative-caching
        // here would trade away the bounded exactly-one-refetch contract.
        set = provider.jwks(true).await?;
    }
    let jwk = find_key(&set, kid).ok_or(VerifyError::UnknownKid)?;

    // The signature covers the token's own base64 bytes, exactly as
    // received - never a re-encoding.
    let signing_input = &token[..header_b64.len() + 1 + payload_b64.len()];
    let signature = Signature::try_from(b64url(signature_b64)?.as_slice())
        .map_err(|_| VerifyError::Signature)?;
    verifying_key(jwk)?
        .verify(signing_input.as_bytes(), &signature)
        .map_err(|_| VerifyError::Signature)?;

    validate_claims(&b64url(payload_b64)?, expected_audience, now_unix_seconds)
}

fn b64url(part: &str) -> Result<Vec<u8>, VerifyError> {
    Base64UrlUnpadded::decode_vec(part).map_err(|_| VerifyError::Malformed)
}

fn find_key<'a>(set: &'a JwkSet, kid: &str) -> Option<&'a Jwk> {
    set.keys.iter().find(|key| key.kid.as_deref() == Some(kid))
}

fn verifying_key(jwk: &Jwk) -> Result<VerifyingKey<Sha256>, VerifyError> {
    if jwk.kty.as_deref() != Some("RSA") {
        return Err(VerifyError::Key);
    }
    let component = |value: &Option<String>| {
        value
            .as_deref()
            .and_then(|value| Base64UrlUnpadded::decode_vec(value).ok())
            .ok_or(VerifyError::Key)
    };
    let n = BoxedUint::from_be_slice_vartime(&component(&jwk.n)?);
    let e = BoxedUint::from_be_slice_vartime(&component(&jwk.e)?);
    let key = RsaPublicKey::new(n, e).map_err(|_| VerifyError::Key)?;
    Ok(VerifyingKey::new(key))
}

/// The ordered claim checks over the raw JSON object. Claims are
/// extracted individually at their own step - never through one typed
/// deserialization up front - so neither a missing nor a wrong-typed
/// later claim can preempt the mandated order (a wrong issuer must
/// answer [`VerifyError::Issuer`] even when `repository_id` is garbage).
fn validate_claims(
    payload: &[u8],
    expected_audience: &str,
    now: i64,
) -> Result<GithubClaims, VerifyError> {
    let claims: serde_json::Map<String, serde_json::Value> =
        serde_json::from_slice(payload).map_err(|_| VerifyError::Malformed)?;
    if claims.get("iss").and_then(serde_json::Value::as_str) != Some(GITHUB_OIDC_ISSUER) {
        return Err(VerifyError::Issuer);
    }
    if !audience_contains(claims.get("aud"), expected_audience) {
        return Err(VerifyError::Audience);
    }
    // A token without exp would never expire, so exp is required even
    // though it is a timestamp check rather than an identity claim.
    let exp = timestamp_claim(&claims, "exp")?.ok_or(VerifyError::MissingClaim("exp"))?;
    if now >= exp.saturating_add(LEEWAY_SECONDS) {
        return Err(VerifyError::Expired);
    }
    if let Some(nbf) = timestamp_claim(&claims, "nbf")?
        && now < nbf.saturating_sub(LEEWAY_SECONDS)
    {
        return Err(VerifyError::NotYetValid);
    }
    Ok(GithubClaims {
        jti: string_claim(&claims, "jti")?,
        verifiable_until: exp.saturating_add(LEEWAY_SECONDS),
        repository_id: id_claim(&claims, "repository_id")?,
        repository_owner_id: id_claim(&claims, "repository_owner_id")?,
        workflow_ref: string_claim(&claims, "workflow_ref")?,
        git_ref: string_claim(&claims, "ref")?,
        // Optional means absent-is-fine; anything present - null
        // included - must be a usable environment name.
        environment: match claims.get("environment") {
            None => None,
            Some(serde_json::Value::String(environment)) => Some(environment.clone()),
            Some(_) => return Err(VerifyError::InvalidClaim("environment")),
        },
    })
}

/// One-or-many `aud`: RFC 7519 allows a single string or an array of
/// strings (`StringOrURI` values). An array carrying a non-string
/// element is not a valid `aud` claim at all, so it contains nothing;
/// the same goes for a wrong-typed `aud`.
fn audience_contains(aud: Option<&serde_json::Value>, expected: &str) -> bool {
    match aud {
        Some(serde_json::Value::String(aud)) => aud == expected,
        Some(serde_json::Value::Array(auds)) => {
            auds.iter().all(serde_json::Value::is_string)
                && auds.iter().any(|aud| aud.as_str() == Some(expected))
        }
        _ => false,
    }
}

fn timestamp_claim(
    claims: &serde_json::Map<String, serde_json::Value>,
    name: &'static str,
) -> Result<Option<i64>, VerifyError> {
    claims
        .get(name)
        .map(|value| value.as_i64().ok_or(VerifyError::InvalidClaim(name)))
        .transpose()
}

fn string_claim(
    claims: &serde_json::Map<String, serde_json::Value>,
    name: &'static str,
) -> Result<String, VerifyError> {
    claims
        .get(name)
        .ok_or(VerifyError::MissingClaim(name))?
        .as_str()
        .ok_or(VerifyError::InvalidClaim(name))
        .map(str::to_owned)
}

/// GitHub has sent the numeric ids both as JSON numbers and as strings;
/// accept either and fail per-claim on anything else.
fn id_claim(
    claims: &serde_json::Map<String, serde_json::Value>,
    name: &'static str,
) -> Result<i64, VerifyError> {
    match claims.get(name).ok_or(VerifyError::MissingClaim(name))? {
        serde_json::Value::Number(id) => id.as_i64().ok_or(VerifyError::InvalidClaim(name)),
        serde_json::Value::String(id) => id.parse().map_err(|_| VerifyError::InvalidClaim(name)),
        _ => Err(VerifyError::InvalidClaim(name)),
    }
}

/// The production provider: GitHub's JWKS through the Worker Cache API
/// with a [`JWKS_CACHE_TTL_SECS`] TTL. Cache errors read as misses - a
/// broken cache costs an origin fetch, never a refused token - and every
/// origin fetch refreshes the cached copy, so the unknown-kid bypass
/// also heals the cache after a key rotation.
#[cfg(target_arch = "wasm32")]
pub struct GithubJwks;

#[cfg(target_arch = "wasm32")]
impl JwksProvider for GithubJwks {
    async fn jwks(&self, bypass_cache: bool) -> Result<JwkSet, VerifyError> {
        let cache = worker::Cache::default();
        if !bypass_cache
            && let Ok(Some(mut cached)) = cache.get(JWKS_CACHE_KEY, false).await
            && let Ok(set) = cached.json::<JwkSet>().await
        {
            return Ok(set);
        }

        let provider_err = |detail: String| VerifyError::Provider(detail);
        let headers = worker::Headers::new();
        headers
            .set("accept", "application/json")
            .map_err(|err| provider_err(err.to_string()))?;
        let mut init = worker::RequestInit::new();
        init.with_method(worker::Method::Get).with_headers(headers);
        let request = worker::Request::new_with_init(GITHUB_JWKS_URL, &init)
            .map_err(|err| provider_err(err.to_string()))?;
        let mut response = worker::Fetch::Request(request)
            .send()
            .await
            .map_err(|err| provider_err(err.to_string()))?;
        if response.status_code() != 200 {
            return Err(provider_err(format!(
                "jwks fetch answered {}",
                response.status_code()
            )));
        }
        let body = response
            .text()
            .await
            .map_err(|err| provider_err(err.to_string()))?;
        let set: JwkSet =
            serde_json::from_str(&body).map_err(|err| provider_err(err.to_string()))?;

        // The Cache API only stores responses carrying a max-age; the
        // directive lives on the internal copy only and never reaches a
        // client.
        if let Ok(mut for_cache) = worker::Response::ok(body) {
            let stored = for_cache
                .headers_mut()
                .set("cache-control", &format!("max-age={JWKS_CACHE_TTL_SECS}"))
                .is_ok();
            if stored && let Err(err) = cache.put(JWKS_CACHE_KEY, for_cache).await {
                worker::console_error!("caching the github jwks failed: {err}");
            }
        }
        Ok(set)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rsa::RsaPrivateKey;
    use rsa::pkcs1v15::SigningKey;
    use rsa::signature::{SignatureEncoding as _, Signer as _};
    use rsa::traits::PublicKeyParts as _;
    use serde_json::{Value, json};
    use std::cell::Cell;
    use std::future::Future;
    use std::pin::pin;
    use std::sync::OnceLock;
    use std::task::{Context, Poll, Waker};

    const NOW: i64 = 1_700_000_000;
    const KID_A: &str = "key-a";
    const KID_B: &str = "key-b";

    /// The test provider's futures are always immediately ready, so one
    /// poll with a no-op waker is a complete executor.
    fn block_on<F: Future>(future: F) -> F::Output {
        let mut future = pin!(future);
        let mut cx = Context::from_waker(Waker::noop());
        match future.as_mut().poll(&mut cx) {
            Poll::Ready(value) => value,
            Poll::Pending => unreachable!("test futures never pend"),
        }
    }

    struct TestKey {
        signing: SigningKey<Sha256>,
        jwk: Value,
    }

    fn generate_key(kid: &str, seed: u8) -> TestKey {
        use rand::SeedableRng as _;
        let mut rng = rand::rngs::ChaCha8Rng::from_seed([seed; 32]);
        let private = RsaPrivateKey::new(&mut rng, 2048).expect("keygen");
        let public = RsaPublicKey::from(&private);
        // JWK carries minimal big-endian bytes; BoxedUint pads to limb
        // precision, so strip the leading zeros it adds.
        let minimal = |bytes: &[u8]| {
            let start = bytes.iter().position(|&byte| byte != 0).unwrap_or(0);
            Base64UrlUnpadded::encode_string(&bytes[start..])
        };
        let jwk = json!({
            "kty": "RSA",
            "use": "sig",
            "alg": "RS256",
            "kid": kid,
            "n": minimal(&public.n().to_be_bytes()),
            "e": minimal(&public.e().to_be_bytes()),
        });
        TestKey {
            signing: SigningKey::new(private),
            jwk,
        }
    }

    fn key_a() -> &'static TestKey {
        static KEY: OnceLock<TestKey> = OnceLock::new();
        KEY.get_or_init(|| generate_key(KID_A, 42))
    }

    fn key_b() -> &'static TestKey {
        static KEY: OnceLock<TestKey> = OnceLock::new();
        KEY.get_or_init(|| generate_key(KID_B, 43))
    }

    struct TestJwks {
        cached: JwkSet,
        /// What a cache-bypass refetch returns; `None` repeats the
        /// cached set (the origin has nothing newer).
        fresh: Option<JwkSet>,
        bypass_calls: Cell<u32>,
    }

    impl TestJwks {
        fn new(cached: &[&TestKey], fresh: Option<&[&TestKey]>) -> Self {
            let set = |keys: &[&TestKey]| -> JwkSet {
                let keys: Vec<Value> = keys.iter().map(|key| key.jwk.clone()).collect();
                serde_json::from_value(json!({ "keys": keys })).expect("fixture jwks")
            };
            Self {
                cached: set(cached),
                fresh: fresh.map(set),
                bypass_calls: Cell::new(0),
            }
        }
    }

    impl JwksProvider for TestJwks {
        async fn jwks(&self, bypass_cache: bool) -> Result<JwkSet, VerifyError> {
            if bypass_cache {
                self.bypass_calls.set(self.bypass_calls.get() + 1);
                if let Some(fresh) = &self.fresh {
                    return Ok(fresh.clone());
                }
            }
            Ok(self.cached.clone())
        }
    }

    fn encode(value: &Value) -> String {
        Base64UrlUnpadded::encode_string(value.to_string().as_bytes())
    }

    fn sign_raw(key: &TestKey, header_json: &str, claims_json: &str) -> String {
        let signing_input = format!(
            "{}.{}",
            Base64UrlUnpadded::encode_string(header_json.as_bytes()),
            Base64UrlUnpadded::encode_string(claims_json.as_bytes())
        );
        let signature = key.signing.sign(signing_input.as_bytes());
        format!(
            "{signing_input}.{}",
            Base64UrlUnpadded::encode_string(&signature.to_bytes())
        )
    }

    fn sign_token(key: &TestKey, header: &Value, claims: &Value) -> String {
        sign_raw(key, &header.to_string(), &claims.to_string())
    }

    fn rs256_header(kid: &str) -> Value {
        json!({ "alg": "RS256", "typ": "JWT", "kid": kid })
    }

    fn base_claims() -> Value {
        json!({
            "iss": GITHUB_OIDC_ISSUER,
            "aud": DEFAULT_AUDIENCE,
            "exp": NOW + 600,
            "jti": "jti-0001",
            "repository_id": 119_684_778,
            "repository_owner_id": 35_998_702,
            "workflow_ref":
                "cabinpkg/cabin/.github/workflows/ports-publish.yml@refs/heads/main",
            "ref": "refs/heads/main",
        })
    }

    fn verify_claims(claims: &Value) -> Result<GithubClaims, VerifyError> {
        let provider = TestJwks::new(&[key_a()], None);
        let token = sign_token(key_a(), &rs256_header(KID_A), claims);
        let result = block_on(verify(&token, &provider, DEFAULT_AUDIENCE, NOW));
        assert_eq!(provider.bypass_calls.get(), 0, "no refetch on a known kid");
        result
    }

    #[test]
    fn happy_path_returns_the_validated_claims() {
        let claims = verify_claims(&base_claims()).expect("valid token");
        assert_eq!(
            claims,
            GithubClaims {
                jti: "jti-0001".into(),
                verifiable_until: NOW + 600 + LEEWAY_SECONDS,
                repository_id: 119_684_778,
                repository_owner_id: 35_998_702,
                workflow_ref: "cabinpkg/cabin/.github/workflows/ports-publish.yml@refs/heads/main"
                    .into(),
                git_ref: "refs/heads/main".into(),
                environment: None,
            }
        );
    }

    #[test]
    // One row per specified claim rule; splitting the table would hide
    // that these cases share one token-building path.
    #[allow(clippy::too_many_lines)]
    fn claim_validation_table() {
        struct Case {
            name: &'static str,
            mutate: fn(&mut Value),
            expect: Result<(), VerifyError>,
        }
        let cases = [
            Case {
                name: "wrong iss",
                mutate: |claims| claims["iss"] = json!("https://accounts.example.com"),
                expect: Err(VerifyError::Issuer),
            },
            Case {
                name: "missing iss",
                mutate: |claims| {
                    claims.as_object_mut().unwrap().remove("iss");
                },
                expect: Err(VerifyError::Issuer),
            },
            Case {
                name: "wrong aud",
                mutate: |claims| claims["aud"] = json!("someone-else.example"),
                expect: Err(VerifyError::Audience),
            },
            Case {
                name: "aud array containing the audience",
                mutate: |claims| claims["aud"] = json!(["other.example", DEFAULT_AUDIENCE]),
                expect: Ok(()),
            },
            Case {
                // RFC 7519: aud array elements are StringOrURI; a mixed
                // array is no valid aud even when the string is present.
                name: "aud array with a non-string element",
                mutate: |claims| claims["aud"] = json!([DEFAULT_AUDIENCE, false]),
                expect: Err(VerifyError::Audience),
            },
            Case {
                name: "aud array without the audience",
                mutate: |claims| claims["aud"] = json!(["other.example", "another.example"]),
                expect: Err(VerifyError::Audience),
            },
            Case {
                name: "empty aud array",
                mutate: |claims| claims["aud"] = json!([]),
                expect: Err(VerifyError::Audience),
            },
            Case {
                name: "missing aud",
                mutate: |claims| {
                    claims.as_object_mut().unwrap().remove("aud");
                },
                expect: Err(VerifyError::Audience),
            },
            Case {
                name: "expired beyond leeway",
                mutate: |claims| claims["exp"] = json!(NOW - 120),
                expect: Err(VerifyError::Expired),
            },
            Case {
                name: "expired by exactly the leeway",
                mutate: |claims| claims["exp"] = json!(NOW - LEEWAY_SECONDS),
                expect: Err(VerifyError::Expired),
            },
            Case {
                name: "expired within leeway",
                mutate: |claims| claims["exp"] = json!(NOW - 30),
                expect: Ok(()),
            },
            Case {
                name: "missing exp",
                mutate: |claims| {
                    claims.as_object_mut().unwrap().remove("exp");
                },
                expect: Err(VerifyError::MissingClaim("exp")),
            },
            Case {
                name: "nbf in the future beyond leeway",
                mutate: |claims| claims["nbf"] = json!(NOW + 120),
                expect: Err(VerifyError::NotYetValid),
            },
            Case {
                name: "nbf in the future within leeway",
                mutate: |claims| claims["nbf"] = json!(NOW + 30),
                expect: Ok(()),
            },
            Case {
                name: "nbf in the past",
                mutate: |claims| claims["nbf"] = json!(NOW - 600),
                expect: Ok(()),
            },
            Case {
                name: "missing jti",
                mutate: |claims| {
                    claims.as_object_mut().unwrap().remove("jti");
                },
                expect: Err(VerifyError::MissingClaim("jti")),
            },
            Case {
                name: "repository_id as a string",
                mutate: |claims| claims["repository_id"] = json!("119684778"),
                expect: Ok(()),
            },
            Case {
                name: "repository_id as a non-numeric string",
                mutate: |claims| claims["repository_id"] = json!("not-a-number"),
                expect: Err(VerifyError::InvalidClaim("repository_id")),
            },
            Case {
                name: "repository_owner_id as a string",
                mutate: |claims| claims["repository_owner_id"] = json!("35998702"),
                expect: Ok(()),
            },
            Case {
                name: "repository_id as a boolean",
                mutate: |claims| claims["repository_id"] = json!(false),
                expect: Err(VerifyError::InvalidClaim("repository_id")),
            },
            Case {
                // The mandated order holds even over type garbage: the
                // wrong issuer answers before the unusable id is looked at.
                name: "wrong iss with a garbage repository_id",
                mutate: |claims| {
                    claims["iss"] = json!("https://accounts.example.com");
                    claims["repository_id"] = json!(false);
                },
                expect: Err(VerifyError::Issuer),
            },
            Case {
                name: "wrong iss beats wrong aud",
                mutate: |claims| {
                    claims["iss"] = json!("https://accounts.example.com");
                    claims["aud"] = json!("someone-else.example");
                },
                expect: Err(VerifyError::Issuer),
            },
            Case {
                name: "wrong aud beats expiry",
                mutate: |claims| {
                    claims["aud"] = json!("someone-else.example");
                    claims["exp"] = json!(NOW - 600);
                },
                expect: Err(VerifyError::Audience),
            },
            Case {
                name: "missing repository_owner_id",
                mutate: |claims| {
                    claims
                        .as_object_mut()
                        .unwrap()
                        .remove("repository_owner_id");
                },
                expect: Err(VerifyError::MissingClaim("repository_owner_id")),
            },
            Case {
                name: "wrong-typed aud",
                mutate: |claims| claims["aud"] = json!(5),
                expect: Err(VerifyError::Audience),
            },
            Case {
                name: "exp as a string",
                mutate: |claims| claims["exp"] = json!("soon"),
                expect: Err(VerifyError::InvalidClaim("exp")),
            },
            Case {
                name: "nbf as a string",
                mutate: |claims| claims["nbf"] = json!("later"),
                expect: Err(VerifyError::InvalidClaim("nbf")),
            },
            Case {
                name: "environment as a number",
                mutate: |claims| claims["environment"] = json!(7),
                expect: Err(VerifyError::InvalidClaim("environment")),
            },
            Case {
                // Optional means absent, not null: a present null is as
                // unusable as any other wrong type.
                name: "environment as null",
                mutate: |claims| claims["environment"] = json!(null),
                expect: Err(VerifyError::InvalidClaim("environment")),
            },
            Case {
                name: "jti as a number",
                mutate: |claims| claims["jti"] = json!(42),
                expect: Err(VerifyError::InvalidClaim("jti")),
            },
            Case {
                name: "missing repository_id",
                mutate: |claims| {
                    claims.as_object_mut().unwrap().remove("repository_id");
                },
                expect: Err(VerifyError::MissingClaim("repository_id")),
            },
            Case {
                name: "missing workflow_ref",
                mutate: |claims| {
                    claims.as_object_mut().unwrap().remove("workflow_ref");
                },
                expect: Err(VerifyError::MissingClaim("workflow_ref")),
            },
            Case {
                name: "missing ref",
                mutate: |claims| {
                    claims.as_object_mut().unwrap().remove("ref");
                },
                expect: Err(VerifyError::MissingClaim("ref")),
            },
            Case {
                name: "environment present",
                mutate: |claims| claims["environment"] = json!("release"),
                expect: Ok(()),
            },
        ];
        for case in cases {
            let mut claims = base_claims();
            (case.mutate)(&mut claims);
            let result = verify_claims(&claims).map(|_| ());
            assert_eq!(result, case.expect, "case: {}", case.name);
        }
    }

    #[test]
    fn the_expected_audience_parameter_is_what_is_enforced() {
        // The audience is a parameter precisely so another consumer (the
        // future verifier pipeline) can reuse the module; a verifier that
        // hardcoded [`DEFAULT_AUDIENCE`] would pass every other test.
        let custom = "verifier.cabinpkg.com";
        let mut claims = base_claims();
        claims["aud"] = json!(custom);
        let token = sign_token(key_a(), &rs256_header(KID_A), &claims);
        let provider = TestJwks::new(&[key_a()], None);
        assert!(block_on(verify(&token, &provider, custom, NOW)).is_ok());
        assert_eq!(
            block_on(verify(&token, &provider, DEFAULT_AUDIENCE, NOW)),
            Err(VerifyError::Audience),
        );
    }

    #[test]
    fn alien_jwks_entries_are_tolerated_but_never_usable() {
        // A non-RSA or kid-less key elsewhere in the set must not make
        // the set unusable (the documented per-key leniency)...
        let set: JwkSet = serde_json::from_value(json!({
            "keys": [
                { "kty": "EC", "kid": "key-ec", "crv": "P-256", "x": "AQAB", "y": "AQAB" },
                { "kty": "RSA" },
                key_a().jwk,
            ],
        }))
        .expect("lenient jwks parse");
        let provider = TestJwks {
            cached: set,
            fresh: None,
            bypass_calls: Cell::new(0),
        };
        let token = sign_token(key_a(), &rs256_header(KID_A), &base_claims());
        assert!(block_on(verify(&token, &provider, DEFAULT_AUDIENCE, NOW)).is_ok());
        // ...but selecting the alien key by kid is a hard Key refusal.
        let token = sign_token(key_a(), &rs256_header("key-ec"), &base_claims());
        assert_eq!(
            block_on(verify(&token, &provider, DEFAULT_AUDIENCE, NOW)),
            Err(VerifyError::Key),
        );
    }

    #[test]
    fn string_and_integer_ids_parse_to_the_same_value() {
        let mut claims = base_claims();
        claims["repository_id"] = json!("119684778");
        claims["repository_owner_id"] = json!("35998702");
        let parsed = verify_claims(&claims).expect("string ids accepted");
        assert_eq!(parsed.repository_id, 119_684_778);
        assert_eq!(parsed.repository_owner_id, 35_998_702);
    }

    #[test]
    fn environment_claim_is_carried_through() {
        let mut claims = base_claims();
        claims["environment"] = json!("release");
        let parsed = verify_claims(&claims).expect("valid token");
        assert_eq!(parsed.environment.as_deref(), Some("release"));
    }

    #[test]
    fn alg_none_is_rejected() {
        let header = json!({ "alg": "none", "kid": KID_A });
        let token = format!("{}.{}.", encode(&header), encode(&base_claims()));
        let provider = TestJwks::new(&[key_a()], None);
        let result = block_on(verify(&token, &provider, DEFAULT_AUDIENCE, NOW));
        assert_eq!(result, Err(VerifyError::Algorithm));
    }

    #[test]
    fn alg_hs256_is_rejected() {
        let header = json!({ "alg": "HS256", "kid": KID_A });
        let forged = Base64UrlUnpadded::encode_string(b"not-an-rsa-signature");
        let token = format!("{}.{}.{forged}", encode(&header), encode(&base_claims()));
        let provider = TestJwks::new(&[key_a()], None);
        let result = block_on(verify(&token, &provider, DEFAULT_AUDIENCE, NOW));
        assert_eq!(result, Err(VerifyError::Algorithm));
    }

    #[test]
    fn missing_kid_is_rejected() {
        for header in [
            json!({ "alg": "RS256" }),
            json!({ "alg": "RS256", "kid": false }),
        ] {
            let token = sign_token(key_a(), &header, &base_claims());
            let provider = TestJwks::new(&[key_a()], None);
            let result = block_on(verify(&token, &provider, DEFAULT_AUDIENCE, NOW));
            assert_eq!(result, Err(VerifyError::MissingKid), "header: {header}");
        }
    }

    #[test]
    fn a_garbage_kid_cannot_preempt_the_alg_check() {
        // The header checks are ordered like the claims: alg answers
        // first even when kid is type garbage.
        let header = json!({ "alg": "none", "kid": false });
        let token = format!("{}.{}.", encode(&header), encode(&base_claims()));
        let provider = TestJwks::new(&[key_a()], None);
        let result = block_on(verify(&token, &provider, DEFAULT_AUDIENCE, NOW));
        assert_eq!(result, Err(VerifyError::Algorithm));
    }

    #[test]
    fn unknown_kid_verifies_after_one_refetch_finds_the_key() {
        let token = sign_token(key_b(), &rs256_header(KID_B), &base_claims());
        let provider = TestJwks::new(&[key_a()], Some(&[key_a(), key_b()]));
        let result = block_on(verify(&token, &provider, DEFAULT_AUDIENCE, NOW));
        assert!(result.is_ok(), "rotated key verifies after the refetch");
        assert_eq!(provider.bypass_calls.get(), 1, "exactly one bypass refetch");
    }

    #[test]
    fn unknown_kid_fails_after_one_unhelpful_refetch() {
        let token = sign_token(key_b(), &rs256_header(KID_B), &base_claims());
        let provider = TestJwks::new(&[key_a()], None);
        let result = block_on(verify(&token, &provider, DEFAULT_AUDIENCE, NOW));
        assert_eq!(result, Err(VerifyError::UnknownKid));
        assert_eq!(provider.bypass_calls.get(), 1, "exactly one bypass refetch");
    }

    #[test]
    fn a_swapped_payload_fails_the_signature() {
        let token = sign_token(key_a(), &rs256_header(KID_A), &base_claims());
        let (rest, signature) = token.rsplit_once('.').unwrap();
        let (header, _) = rest.split_once('.').unwrap();
        let mut tampered = base_claims();
        tampered["repository_id"] = json!(1);
        let token = format!("{header}.{}.{signature}", encode(&tampered));
        let provider = TestJwks::new(&[key_a()], None);
        let result = block_on(verify(&token, &provider, DEFAULT_AUDIENCE, NOW));
        assert_eq!(result, Err(VerifyError::Signature));
    }

    #[test]
    fn verification_covers_the_received_bytes_not_a_reencoding() {
        // Non-canonical JSON - insignificant whitespace, unusual member
        // order, an unknown member - signed over exactly these bytes. A
        // verifier that decoded and reserialized before checking the
        // signature would refuse this valid token.
        let header = format!("{{ \"kid\" : \"{KID_A}\" ,\n  \"typ\":\"JWT\", \"alg\":\"RS256\" }}");
        let claims = format!(
            "{{\n  \"ref\": \"refs/heads/main\",\n  \"jti\": \"jti-0001\" ,\n  \
             \"workflow_ref\": \"cabinpkg/cabin/.github/workflows/ports-publish.yml@refs/heads/main\",\n  \
             \"repository_owner_id\": 35998702,  \"repository_id\": 119684778,\n  \
             \"runner_environment\": \"github-hosted\",\n  \
             \"exp\": {exp},  \"aud\": \"{DEFAULT_AUDIENCE}\",\n  \"iss\": \"{GITHUB_OIDC_ISSUER}\"\n}}",
            exp = NOW + 600,
        );
        let token = sign_raw(key_a(), &header, &claims);
        let provider = TestJwks::new(&[key_a()], None);
        let parsed =
            block_on(verify(&token, &provider, DEFAULT_AUDIENCE, NOW)).expect("valid token");
        assert_eq!(parsed.jti, "jti-0001");
        assert_eq!(parsed.repository_id, 119_684_778);
    }

    #[test]
    fn a_two_segment_token_is_malformed() {
        let token = format!(
            "{}.{}",
            encode(&rs256_header(KID_A)),
            encode(&base_claims())
        );
        let provider = TestJwks::new(&[key_a()], None);
        let result = block_on(verify(&token, &provider, DEFAULT_AUDIENCE, NOW));
        assert_eq!(result, Err(VerifyError::Malformed));
    }
}
