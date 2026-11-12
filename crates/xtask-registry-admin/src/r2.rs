//! Cloudflare's R2 REST object API, which the wipe's sweep lists and
//! deletes through: wrangler exposes no command for listing a bucket,
//! and no bulk mode for deleting one.

use anyhow::{Context, Result, bail};
use serde::Deserialize;

/// Cloudflare's API.
pub(crate) const API_BASE: &str = "https://api.cloudflare.com/client/v4";

/// One page of the R2 REST listing.  `key` is required: a row that
/// does not carry one is not an answer about this bucket.  The API
/// leaves `result_info` out of a listing's last page, and a `null` is
/// not that.
#[derive(Deserialize)]
struct Page {
    success: bool,
    result: Vec<Object>,
    #[serde(default, deserialize_with = "present")]
    result_info: Option<PageInfo>,
}

/// `deserialize_with` for an R2 listing's `result_info`, so only a
/// missing key reads as `None`.
fn present<'de, D: serde::Deserializer<'de>, T: Deserialize<'de>>(
    value: D,
) -> Result<Option<T>, D::Error> {
    T::deserialize(value).map(Some)
}

#[derive(Deserialize)]
struct Object {
    key: String,
}

#[derive(Deserialize)]
struct PageInfo {
    is_truncated: Option<bool>,
}

fn get(agent: &ureq::Agent, url: &str, token: &str) -> Result<String> {
    Ok(agent
        .get(url)
        .set("Authorization", &format!("Bearer {token}"))
        .call()?
        .into_string()?)
}

/// One account's buckets through the R2 REST object API.
pub(crate) struct Buckets {
    agent: ureq::Agent,
    url: String,
    token: String,
}

impl Buckets {
    /// `base` is the API root that `accounts/<id>/r2/buckets` hangs off.
    pub(crate) fn new(base: &str, account: &str, token: String) -> Self {
        Self {
            agent: crate::agent(),
            url: format!("{base}/accounts/{account}/r2/buckets"),
            token,
        }
    }

    /// The keys of one listing page of `bucket`, at most 500 of them.
    /// An unsuccessful or malformed answer is an error, never an empty
    /// page, and so is an empty page whose metadata does not say the
    /// listing ended: only a successful empty last page proves nothing
    /// is there, and the API marks the last page by leaving the
    /// metadata out.
    pub(crate) fn keys(&self, bucket: &str) -> Result<Vec<String>> {
        let url = format!("{}/{bucket}/objects?per_page=500", self.url);
        let body = get(&self.agent, &url, &self.token)
            .with_context(|| format!("listing {bucket} failed"))?;
        let page: Page = serde_json::from_str(&body)
            .with_context(|| format!("unexpected R2 list response: {body}"))?;
        let unfinished = page
            .result_info
            .is_some_and(|info| info.is_truncated != Some(false));
        if !page.success || (page.result.is_empty() && unfinished) {
            bail!("unexpected R2 list response: {body}");
        }
        Ok(page.result.into_iter().map(|object| object.key).collect())
    }

    /// Deletes one key, reading only the status, so the sweep proves each
    /// delete by listing again.  A key with a `.` or `..` segment is
    /// refused unsent: URL parsing resolves those segments, and the
    /// request would reach another path of the API - another key, or the
    /// bucket itself.
    pub(crate) fn delete(&self, bucket: &str, key: &str) -> Result<()> {
        if key
            .split('/')
            .any(|segment| segment == "." || segment == "..")
        {
            bail!(
                "{key} in {bucket} has a dot segment, which no registry writer produces and no \
                 API URL carries verbatim: investigate it, and never delete it through wrangler \
                 or the REST API"
            );
        }
        self.agent
            .delete(&format!(
                "{}/{bucket}/objects/{}",
                self.url,
                encode_key(key)
            ))
            .set("Authorization", &format!("Bearer {}", self.token))
            .call()
            .with_context(|| format!("deleting {key} from {bucket} failed"))?;
        Ok(())
    }
}

/// `key.split("/").map(encodeURIComponent).join("/")`: the separators
/// stay separators and everything else is escaped.
fn encode_key(key: &str) -> String {
    key.split('/')
        .map(encode_uri_component)
        .collect::<Vec<_>>()
        .join("/")
}

/// `encodeURIComponent`.
fn encode_uri_component(value: &str) -> String {
    use std::fmt::Write as _;

    let mut encoded = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || b"-_.!~*'()".contains(&byte) {
            encoded.push(char::from(byte));
        } else {
            let _ = write!(encoded, "%{byte:02X}");
        }
    }
    encoded
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_key_encoder_keeps_separators() {
        assert_eq!(encode_key("uploads/abc.zip"), "uploads/abc.zip");
        assert_eq!(encode_key("uploads/"), "uploads/");
        assert_eq!(encode_key("a b/c+d"), "a%20b/c%2Bd");
        assert_eq!(encode_key("a/b?c=d&e"), "a/b%3Fc%3Dd%26e");
        assert_eq!(encode_key("a%41"), "a%2541");
        assert_eq!(encode_key("x/-_.!~*\'()"), "x/-_.!~*\'()");
    }

    #[test]
    fn a_listing_row_without_a_key_is_not_absence() {
        let page = |body: &str| serde_json::from_str::<Page>(body);
        assert!(page(r#"{"success":true,"result":[{"key":"a"}]}"#).is_ok());
        assert!(page(r#"{"success":true,"result":[]}"#).is_ok());
        for refused in [
            r#"{"success":true,"result":[null]}"#,
            r#"{"success":true,"result":[{"size":1}]}"#,
            r#"{"success":true,"result":["a"]}"#,
            r#"{"success":true}"#,
        ] {
            assert!(page(refused).is_err(), "accepted {refused}");
        }
    }
}
