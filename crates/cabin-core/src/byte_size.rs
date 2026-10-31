//! Byte-size values such as the `[gc] max-size` budget.
//!
//! The parser lives in `cabin-core` because config parsing and the CLI
//! both read the same spelling.

use thiserror::Error;

/// Parse a byte count: a non-negative integer with an optional unit,
/// `B`, decimal `KB` / `MB` / `GB` / `TB`, or binary `KiB` / `MiB` /
/// `GiB` / `TiB`.  Whitespace between the number and the unit is
/// allowed; units are case-sensitive.
///
/// # Errors
/// Returns [`ByteSizeParseError`] for an empty value, a missing or
/// malformed number, an unknown unit, or a value that overflows `u64`.
pub fn parse_byte_size(value: &str) -> Result<u64, ByteSizeParseError> {
    let invalid = || ByteSizeParseError {
        value: value.to_owned(),
    };
    let trimmed = value.trim();
    let digits = trimmed
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(trimmed.len());
    let number: u64 = trimmed[..digits].parse().map_err(|_| invalid())?;
    let multiplier: u64 = match trimmed[digits..].trim_start() {
        "" | "B" => 1,
        "KB" => 1_000,
        "MB" => 1_000_000,
        "GB" => 1_000_000_000,
        "TB" => 1_000_000_000_000,
        "KiB" => 1 << 10,
        "MiB" => 1 << 20,
        "GiB" => 1 << 30,
        "TiB" => 1 << 40,
        _ => return Err(invalid()),
    };
    number.checked_mul(multiplier).ok_or_else(invalid)
}

/// A value [`parse_byte_size`] rejected.
#[derive(Debug, Error, PartialEq, Eq)]
#[error(
    "invalid size {value:?}: expected a whole number with an optional unit (B, KB, MB, GB, TB, KiB, MiB, GiB, TiB)"
)]
pub struct ByteSizeParseError {
    pub value: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_plain_decimal_and_binary_units() {
        assert_eq!(parse_byte_size("0"), Ok(0));
        assert_eq!(parse_byte_size("512"), Ok(512));
        assert_eq!(parse_byte_size("512B"), Ok(512));
        assert_eq!(parse_byte_size("10 MB"), Ok(10_000_000));
        assert_eq!(parse_byte_size("10MiB"), Ok(10 << 20));
        assert_eq!(parse_byte_size(" 2 GiB "), Ok(2 << 30));
        assert_eq!(parse_byte_size("1TB"), Ok(1_000_000_000_000));
    }

    #[test]
    fn rejects_malformed_values() {
        for bad in [
            "",
            "GiB",
            "-1",
            "1.5GiB",
            "10 gb",
            "10 GiBs",
            "99999999999999999999",
        ] {
            assert!(parse_byte_size(bad).is_err(), "{bad:?} should be rejected");
        }
        assert!(parse_byte_size("20000000TiB").is_err(), "overflow");
    }
}
