//! The inspection caps and their environment-variable
//! configuration.  The mechanism (what each cap bounds) is public
//! contract; the values are configuration - the GitHub Actions
//! workflow passes them through as repository variables, which
//! arrive as empty strings when unset, so empty means "use the
//! default".

use thiserror::Error;

/// `VERIFY_RATIO_CAP`: decompressed bytes allowed per compressed
/// byte.
const VERIFY_RATIO_CAP: &str = "VERIFY_RATIO_CAP";
/// `VERIFY_ABS_CAP_BYTES`: absolute decompressed-total cap in
/// bytes.
const VERIFY_ABS_CAP_BYTES: &str = "VERIFY_ABS_CAP_BYTES";
/// `VERIFY_MAX_ENTRIES`: zip entry count cap.
const VERIFY_MAX_ENTRIES: &str = "VERIFY_MAX_ENTRIES";
/// `VERIFY_MAX_PATH_LEN`: per-entry path length cap in bytes.
const VERIFY_MAX_PATH_LEN: &str = "VERIFY_MAX_PATH_LEN";

/// Inspection caps.  The decompressed-total cap for one archive is
/// `min(max(ratio_cap x compressed_size, floor), abs_cap_bytes)`,
/// where the floor covers the zip framing the entry cap permits
/// (`crate::scan`): zip framing (local and central headers, the
/// EOCD) is fixed overhead that does not compress, so without the
/// floor a small legitimate archive of many tiny files would trip
/// the ratio cap.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    pub ratio_cap: u64,
    pub abs_cap_bytes: u64,
    pub max_entries: usize,
    pub max_path_len: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Limits {
            ratio_cap: 10,
            abs_cap_bytes: 256 * 1024 * 1024,
            max_entries: 10_000,
            max_path_len: 256,
        }
    }
}

/// A limit variable that is set but does not parse.  Misconfiguration
/// must fail the run (leaving versions pending), never silently fall
/// back to a default.
#[derive(Debug, Error, PartialEq, Eq)]
#[error("invalid {name} value {value:?}: expected a positive integer")]
pub struct LimitsError {
    name: &'static str,
    value: String,
}

/// Read [`Limits`] from the environment via `get` (a closure so
/// tests can stub the environment).  Unset and empty values use the
/// default.
///
/// # Errors
///
/// Returns [`LimitsError`] when a set, non-empty value is not a
/// positive integer.
pub fn limits_from_env(get: impl Fn(&str) -> Option<String>) -> Result<Limits, LimitsError> {
    let defaults = Limits::default();
    Ok(Limits {
        ratio_cap: parse_var(&get, VERIFY_RATIO_CAP, defaults.ratio_cap)?,
        abs_cap_bytes: parse_var(&get, VERIFY_ABS_CAP_BYTES, defaults.abs_cap_bytes)?,
        max_entries: parse_var(&get, VERIFY_MAX_ENTRIES, defaults.max_entries)?,
        max_path_len: parse_var(&get, VERIFY_MAX_PATH_LEN, defaults.max_path_len)?,
    })
}

/// Zero-valued caps reject everything and can only be
/// misconfiguration, so they fail like garbage does - as does a
/// value that overflows the cap's integer type.
fn parse_var<T: TryFrom<u64>>(
    get: impl Fn(&str) -> Option<String>,
    name: &'static str,
    default: T,
) -> Result<T, LimitsError> {
    let Some(value) = get(name).filter(|value| !value.is_empty()) else {
        return Ok(default);
    };
    match value.parse::<u64>() {
        Ok(parsed) if parsed != 0 => T::try_from(parsed).map_err(|_| LimitsError { name, value }),
        _ => Err(LimitsError { name, value }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env<'a>(pairs: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<String> + 'a {
        move |name| {
            pairs
                .iter()
                .find(|(key, _)| *key == name)
                .map(|(_, value)| (*value).to_owned())
        }
    }

    #[test]
    fn unset_environment_yields_defaults() {
        let limits = limits_from_env(env(&[])).unwrap();
        assert_eq!(limits, Limits::default());
        assert_eq!(limits.ratio_cap, 10);
        assert_eq!(limits.abs_cap_bytes, 256 * 1024 * 1024);
        assert_eq!(limits.max_entries, 10_000);
        assert_eq!(limits.max_path_len, 256);
    }

    #[test]
    fn empty_values_mean_unset() {
        // Unset GitHub repository variables arrive as empty strings.
        let limits = limits_from_env(env(&[
            (VERIFY_RATIO_CAP, ""),
            (VERIFY_ABS_CAP_BYTES, ""),
            (VERIFY_MAX_ENTRIES, ""),
            (VERIFY_MAX_PATH_LEN, ""),
        ]))
        .unwrap();
        assert_eq!(limits, Limits::default());
    }

    #[test]
    fn set_values_override_defaults() {
        let limits = limits_from_env(env(&[
            (VERIFY_RATIO_CAP, "3"),
            (VERIFY_ABS_CAP_BYTES, "1024"),
            (VERIFY_MAX_ENTRIES, "5"),
            (VERIFY_MAX_PATH_LEN, "64"),
        ]))
        .unwrap();
        assert_eq!(
            limits,
            Limits {
                ratio_cap: 3,
                abs_cap_bytes: 1024,
                max_entries: 5,
                max_path_len: 64,
            }
        );
    }

    #[test]
    fn garbage_and_zero_values_error() {
        for value in ["banana", "-1", "1.5", "0"] {
            let err = limits_from_env(env(&[(VERIFY_MAX_ENTRIES, value)])).unwrap_err();
            assert_eq!(
                err.to_string(),
                format!("invalid VERIFY_MAX_ENTRIES value {value:?}: expected a positive integer")
            );
        }
    }
}
