//! JSON composition for the public stats plane (`GET /api/v1/stats`):
//! pure and host-testable, mirroring [`crate::documents`] for the read
//! plane. The totals cover **verified** versions only - the same
//! exposure rule as the read plane - and the download total is the
//! approximate served-download counter (`docs/architecture.md`,
//! "Download counts").

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
}
