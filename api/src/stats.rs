//! JSON composition for the public stats plane (`GET /api/v1/stats`
//! and `GET /api/v1/stats/packages/<scope>/<name>`): pure and
//! host-testable, mirroring [`crate::documents`] for the read plane.
//! The totals cover **verified** versions only - the same exposure
//! rule as the read plane - and sum the versions' download counters;
//! a package's days are the request-log pass's
//! ([`crate::download_log`]).

/// The registry-wide totals the public stats endpoint serves.
pub struct RegistryTotals {
    pub packages: u64,
    pub versions: u64,
    pub downloads: u64,
}

/// `GET /api/v1/stats`.
pub fn summary_json(totals: &RegistryTotals) -> String {
    serde_json::json!({
        "packages": totals.packages,
        "versions": totals.versions,
        "downloads": totals.downloads,
    })
    .to_string()
}

/// `GET /api/v1/stats/packages/<scope>/<name>`: the package's download
/// total, then its downloads on each UTC day `PACKAGE_DAILY_DOWNLOADS`
/// answers, oldest first.
pub fn package_json(downloads: u64, days: &[(String, u64)]) -> String {
    let days: Vec<_> = days
        .iter()
        .map(|(day, downloads)| serde_json::json!({ "day": day, "downloads": downloads }))
        .collect();
    serde_json::json!({ "downloads": downloads, "days": days }).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn summary_json_is_the_documented_shape() {
        let totals = RegistryTotals {
            packages: 2,
            versions: 5,
            downloads: 1_234,
        };
        assert_eq!(
            summary_json(&totals),
            r#"{"packages":2,"versions":5,"downloads":1234}"#
        );
    }

    #[test]
    fn an_empty_registry_answers_zeros() {
        let totals = RegistryTotals {
            packages: 0,
            versions: 0,
            downloads: 0,
        };
        assert_eq!(
            summary_json(&totals),
            r#"{"packages":0,"versions":0,"downloads":0}"#
        );
    }

    #[test]
    fn package_json_is_the_documented_shape() {
        let days = [("2026-11-18".to_owned(), 0), ("2026-11-19".to_owned(), 3)];
        assert_eq!(
            package_json(12, &days),
            r#"{"downloads":12,"days":[{"day":"2026-11-18","downloads":0},{"day":"2026-11-19","downloads":3}]}"#
        );
    }
}
