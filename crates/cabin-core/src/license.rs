//! The SPDX license expression a package declares in `[package] license`.

use serde::{Deserialize, Serialize};

/// Strict SPDX syntax over every identifier on the SPDX license list,
/// deprecated ones included (`GPL-2.0`, and `GPL-2.0+` through the
/// GNU postfix option).  A published manifest is immutable, so an
/// identifier that was listed when it was published must not become
/// an error once the list deprecates it.
const MODE: spdx::ParseMode = spdx::ParseMode {
    allow_deprecated: true,
    allow_postfix_plus_on_gpl: true,
    ..spdx::ParseMode::STRICT
};

/// A valid SPDX license expression, kept exactly as declared.
///
/// A declaration, not a legal conclusion: Cabin validates the syntax
/// and the identifiers and otherwise never interprets the value.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "String")]
pub struct LicenseExpression(String);

impl LicenseExpression {
    /// Validate `value` as an SPDX license expression.
    ///
    /// # Errors
    /// Returns [`LicenseError`] when `value` is not valid SPDX
    /// expression syntax, names a license or exception that is
    /// neither on the SPDX license list nor a `LicenseRef-` /
    /// `AdditionRef-` user reference, or separates terms with
    /// anything but plain spaces.
    pub fn new(value: impl Into<String>) -> Result<Self, LicenseError> {
        let value = value.into();
        // `spdx` skips every Unicode whitespace character.  The value
        // is published verbatim, so a line break or an invisible
        // separator would reach every reader of the index.
        if value.chars().any(|c| c.is_whitespace() && c != ' ') {
            return Err(LicenseError {
                value,
                reason: "terms must be separated by plain spaces".to_owned(),
            });
        }
        match spdx::Expression::parse_mode(&value, MODE) {
            Ok(_) => Ok(Self(value)),
            Err(err) => {
                let reason = match value.get(err.span.clone()) {
                    Some(term) if !term.is_empty() => {
                        format!("{} (at {term:?}, byte {})", err.reason, err.span.start)
                    }
                    _ => err.reason.to_string(),
                };
                Err(LicenseError { value, reason })
            }
        }
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for LicenseExpression {
    type Error = LicenseError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

/// Why a value is not a valid SPDX license expression.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{value:?} is not a valid SPDX license expression: {reason}")]
pub struct LicenseError {
    value: String,
    reason: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_spdx_expressions_and_keeps_them_verbatim() {
        for value in [
            "MIT",
            "Apache-2.0",
            "GPL-3.0-or-later",
            "MIT OR Apache-2.0",
            "MIT AND BSD-3-Clause",
            "Apache-2.0 WITH LLVM-exception",
            "Apache-2.0 AND (GPL-3.0-or-later WITH GCC-exception-3.1)",
            "(MIT OR (Apache-2.0 AND (BSD-2-Clause OR Zlib)))",
            "Apache-2.0+",
            "LicenseRef-Vendor-EULA",
            "DocumentRef-spdx-tool-1.2:LicenseRef-MIT-Style-2",
            "MIT AND LicenseRef-Vendor-EULA",
            "GPL-2.0-only WITH AdditionRef-Vendor-Linking",
            // Deprecated on the SPDX list, still listed identifiers.
            "GPL-2.0",
            "GPL-2.0+",
            // Spacing is the declarer's; it is not normalized.
            "  MIT   OR Apache-2.0 ",
        ] {
            let license = LicenseExpression::new(value)
                .unwrap_or_else(|err| panic!("{value:?} must be accepted: {err}"));
            assert_eq!(license.as_str(), value);
        }
    }

    #[test]
    fn rejects_invalid_expressions() {
        for value in [
            "",
            "   ",
            "Apache2",
            // Identifiers are matched case-sensitively.
            "mit",
            "MIT OR",
            "MIT Apache-2.0",
            "(MIT OR Apache-2.0",
            "MIT OR Apache-2.0)",
            "MIT/Apache-2.0",
            "MIT, Apache-2.0",
            "MIT WITH Apache-2.0",
            "MIT WITH Not-An-Exception",
            "LicenseRef-a_b",
            "GPL-3.0-or-later+",
            "MIT\u{1b}[2K",
            // Only a plain space separates terms.
            "MIT OR Apache-2.0\n",
            "MIT\tOR Apache-2.0",
            "MIT OR\rApache-2.0",
            "MIT\u{a0}OR Apache-2.0",
            "MIT\u{2028}OR Apache-2.0",
        ] {
            assert!(LicenseExpression::new(value).is_err(), "{value:?}");
        }
    }

    #[test]
    fn error_quotes_the_value_and_names_the_offending_term() {
        let rendered = LicenseExpression::new("MIT OR Apache2")
            .unwrap_err()
            .to_string();
        assert!(
            rendered.starts_with(r#""MIT OR Apache2" is not a valid SPDX license expression: "#),
            "{rendered}"
        );
        assert!(
            rendered.ends_with(r#"(at "Apache2", byte 7)"#),
            "{rendered}"
        );
    }

    #[test]
    fn serde_round_trips_as_a_plain_string_and_revalidates() {
        let license =
            LicenseExpression::new("Apache-2.0 AND (GPL-3.0-or-later WITH GCC-exception-3.1)")
                .unwrap();
        let json = serde_json::to_string(&license).unwrap();
        assert_eq!(
            json,
            r#""Apache-2.0 AND (GPL-3.0-or-later WITH GCC-exception-3.1)""#
        );
        assert_eq!(
            serde_json::from_str::<LicenseExpression>(&json).unwrap(),
            license
        );
        let err = serde_json::from_str::<LicenseExpression>(r#""Apache2""#).unwrap_err();
        assert!(
            err.to_string()
                .contains("is not a valid SPDX license expression"),
            "{err}"
        );
    }
}
