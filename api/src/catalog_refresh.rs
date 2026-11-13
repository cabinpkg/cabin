//! Keeping cabinpkg.com's package pages on the catalog the registry serves
//! (`docs/remote-registry.md`, "The catalog"). The website's deploy build
//! reads `/catalog.json` anonymously and publishes what it rendered at
//! `<WEB_ORIGIN>/catalog-provenance.json`; the breaker cron compares that
//! record with the registry's current answer every pass and, on a mismatch,
//! triggers the website's deploy hook.

use serde::Deserialize;

/// The meta row recording `<target> <count>`: the catalog the last
/// deploy-hook trigger aimed at and how many triggers it has had, or empty
/// once the site caught up.
pub const META_KEY: &str = "website_refresh";

/// Triggers per catalog before the cron only logs: a site build that keeps
/// failing would otherwise spend a build every pass.
pub const MAX_TRIGGERS: u32 = 3;

/// What an anonymous `GET /catalog.json` answers, which is what the
/// website's deploy build renders.
#[derive(Debug, PartialEq, Eq)]
pub enum Target {
    /// The private-alpha refusal.
    Unavailable,
    /// The served catalog: the lowercase hex SHA-256 of its body.
    Available(String),
}

impl Target {
    fn token(&self) -> &str {
        match self {
            Target::Unavailable => "unavailable",
            Target::Available(sha256) => sha256,
        }
    }
}

/// `website/scripts/fetch-catalog.mjs` `provenanceOf`; other fields are
/// ignored.
#[derive(Deserialize)]
struct Provenance {
    source: String,
    state: String,
    sha256: Option<String>,
}

/// Whether the deployed site's `catalog-provenance.json` records `target`.
/// A missing or malformed record, or one from a fixture build, never does.
pub fn is_current(provenance: &str, target: &Target) -> bool {
    let Ok(record) = serde_json::from_str::<Provenance>(provenance) else {
        return false;
    };
    record.source == "registry"
        && match target {
            Target::Unavailable => record.state == "unavailable",
            Target::Available(sha256) => {
                record.state == "available" && record.sha256.as_deref() == Some(sha256)
            }
        }
}

/// The meta value recording one more trigger for `target`, or `None` once
/// `stored` holds [`MAX_TRIGGERS`] for it. Any other stored value, a
/// different catalog or the empty caught-up marker, starts the count over.
pub fn next_trigger(stored: Option<&str>, target: &Target) -> Option<String> {
    let token = target.token();
    let count = stored
        .and_then(|value| {
            value
                .strip_prefix(token)?
                .strip_prefix(' ')?
                .parse::<u32>()
                .ok()
        })
        .unwrap_or(0);
    (count < MAX_TRIGGERS).then(|| format!("{token} {}", count + 1))
}

#[cfg(test)]
mod tests {
    use super::*;

    const SHA: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

    #[test]
    fn only_the_registry_record_of_the_target_is_current() {
        let unavailable = r#"{"source":"registry","url":"https://registry.cabinpkg.com/catalog.json","state":"unavailable"}"#;
        let available = format!(
            r#"{{"source":"registry","url":"https://registry.cabinpkg.com/catalog.json","state":"available","sha256":"{SHA}"}}"#
        );
        let target = Target::Available(SHA.to_owned());
        assert!(is_current(unavailable, &Target::Unavailable));
        assert!(is_current(&available, &target));
        assert!(!is_current(unavailable, &target));
        assert!(!is_current(&available, &Target::Unavailable));
        assert!(!is_current(&available, &Target::Available("0".repeat(64))));
        // A fixture build deployed by mistake and garbage read as stale.
        assert!(!is_current(
            r#"{"source":"fixture","state":"unavailable"}"#,
            &Target::Unavailable
        ));
        assert!(!is_current("<!doctype html>", &Target::Unavailable));
    }

    #[test]
    fn triggers_are_bounded_per_catalog() {
        let target = Target::Available(SHA.to_owned());
        let mut stored = None;
        for count in 1..=MAX_TRIGGERS {
            let next = next_trigger(stored.as_deref(), &target).expect("under the bound");
            assert_eq!(next, format!("{SHA} {count}"));
            stored = Some(next);
        }
        assert_eq!(next_trigger(stored.as_deref(), &target), None);
        // A new catalog, or the site catching up in between, starts over.
        assert_eq!(
            next_trigger(stored.as_deref(), &Target::Unavailable).as_deref(),
            Some("unavailable 1")
        );
        assert_eq!(next_trigger(Some(""), &target), Some(format!("{SHA} 1")));
    }
}
