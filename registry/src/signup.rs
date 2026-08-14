//! The sign-up account-age gate: a first sign-in may only create a
//! Cabin account once the GitHub account behind it is at least
//! [`MIN_ACCOUNT_AGE_DAYS`] old (a throwaway-account speed bump).
//! Existing Cabin accounts are never re-checked - the gate exists to
//! slow account *creation*, and locking out a signed-up user over a
//! profile field would punish the wrong action.

/// Minimum GitHub account age, in days, before a first sign-in may
/// create a Cabin account.
pub const MIN_ACCOUNT_AGE_DAYS: u64 = 30;

const SECS_PER_DAY: u64 = 86_400;

/// The gate's verdict for one sign-in callback.
#[derive(Debug, PartialEq, Eq)]
pub enum Gate {
    /// An existing Cabin account, or a new one whose GitHub account is
    /// old enough.
    Proceed,
    /// A new Cabin account behind a too-young GitHub account: refuse,
    /// naming the first UTC date on which the whole day is eligible -
    /// rounded up to the next midnight, so the date shown to the user
    /// is never a false promise, whatever their sign-in's time of day.
    IneligibleUntil(String),
    /// A new Cabin account whose GitHub profile is missing a parseable
    /// `created_at`. The real GitHub API always sends one, so this
    /// only arises from a mangled response - refuse uniformly (fail
    /// closed) rather than guess an age.
    Deny,
}

/// Decides one sign-in callback: `existing_user` is whether the GitHub
/// id already resolves to a Cabin account, `created_at` the profile's
/// `created_at` field as GitHub sent it, `now_secs` the current epoch
/// seconds.
#[must_use]
pub fn gate(existing_user: bool, created_at: Option<&str>, now_secs: u64) -> Gate {
    if existing_user {
        return Gate::Proceed;
    }
    let Some(created_secs) = created_at.and_then(parse_utc_secs) else {
        return Gate::Deny;
    };
    let eligible_secs = created_secs + MIN_ACCOUNT_AGE_DAYS * SECS_PER_DAY;
    if now_secs >= eligible_secs {
        return Gate::Proceed;
    }
    let (y, m, d) = civil_from_days(eligible_secs.div_ceil(SECS_PER_DAY));
    Gate::IneligibleUntil(format!("{y:04}-{m:02}-{d:02}"))
}

/// Epoch seconds of a strict `YYYY-MM-DDTHH:MM:SSZ` UTC timestamp (the
/// one shape GitHub's REST API emits), `None` for anything else -
/// offsets, fractional seconds, and calendar-invalid dates included.
/// Years before 1970 are rejected so the day arithmetic stays in u64;
/// GitHub accounts postdate 2007.
fn parse_utc_secs(value: &str) -> Option<u64> {
    let bytes = value.as_bytes();
    if bytes.len() != 20
        || bytes[4] != b'-'
        || bytes[7] != b'-'
        || bytes[10] != b'T'
        || bytes[13] != b':'
        || bytes[16] != b':'
        || bytes[19] != b'Z'
    {
        return None;
    }
    let field = |range: core::ops::Range<usize>| -> Option<u64> {
        let digits = &value[range];
        digits
            .bytes()
            .all(|byte| byte.is_ascii_digit())
            .then(|| digits.parse().ok())?
    };
    let (year, month, day) = (field(0..4)?, field(5..7)?, field(8..10)?);
    let (hour, minute, second) = (field(11..13)?, field(14..16)?, field(17..19)?);
    if year < 1970
        || !(1..=12).contains(&month)
        || day == 0
        || day > days_in_month(year, month)
        || hour > 23
        || minute > 59
        || second > 59
    {
        return None;
    }
    Some(days_from_civil(year, month, day) * SECS_PER_DAY + hour * 3_600 + minute * 60 + second)
}

fn days_in_month(year: u64, month: u64) -> u64 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        _ => {
            if year.is_multiple_of(4) && (!year.is_multiple_of(100) || year.is_multiple_of(400)) {
                29
            } else {
                28
            }
        }
    }
}

/// Days since 1970-01-01 of a civil UTC date (Hinnant's
/// `days_from_civil`, restricted to dates from the epoch on so the
/// arithmetic never leaves u64).
fn days_from_civil(year: u64, month: u64, day: u64) -> u64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = year / 400;
    let year_of_era = year - era * 400;
    let day_of_year = (153 * (if month > 2 { month - 3 } else { month + 9 }) + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

/// `(year, month, day)` of a days-since-epoch count (Hinnant's
/// `civil_from_days`).
fn civil_from_days(days: u64) -> (u64, u64, u64) {
    let z = days + 719_468;
    let era = z / 146_097;
    let day_of_era = z - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_shifted = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_shifted + 2) / 5 + 1;
    let month = if month_shifted < 10 {
        month_shifted + 3
    } else {
        month_shifted - 9
    };
    (if month <= 2 { year + 1 } else { year }, month, day)
}

#[cfg(test)]
mod tests {
    use super::*;

    const THIRTY_DAYS: u64 = MIN_ACCOUNT_AGE_DAYS * SECS_PER_DAY;
    // 2008-01-14T04:33:35Z, GitHub's canonical example `created_at`.
    const OCTOCAT_CREATED: &str = "2008-01-14T04:33:35Z";
    const OCTOCAT_SECS: u64 = 1_200_285_215;

    #[test]
    fn an_existing_user_always_proceeds() {
        // The field is not even consulted: young, missing, and
        // malformed profiles all sign in as before.
        assert_eq!(gate(true, Some(OCTOCAT_CREATED), 0), Gate::Proceed);
        assert_eq!(gate(true, None, 0), Gate::Proceed);
        assert_eq!(gate(true, Some("garbage"), 0), Gate::Proceed);
    }

    #[test]
    fn a_new_user_needs_thirty_days_exactly() {
        let eligible = OCTOCAT_SECS + THIRTY_DAYS;
        assert_eq!(gate(false, Some(OCTOCAT_CREATED), eligible), Gate::Proceed);
        assert_eq!(
            gate(false, Some(OCTOCAT_CREATED), eligible - 1),
            Gate::IneligibleUntil("2008-02-14".to_owned())
        );
    }

    #[test]
    fn the_named_date_rounds_up_to_a_full_utc_day() {
        // Created 04:33:35 on Jan 14: thirty days end mid-day Feb 13,
        // so the first *whole* eligible day is Feb 14.
        assert_eq!(
            gate(false, Some(OCTOCAT_CREATED), 0),
            Gate::IneligibleUntil("2008-02-14".to_owned())
        );
        // Created exactly at midnight: the instant is a midnight
        // itself, and that day is named as-is.
        assert_eq!(
            gate(false, Some("2008-01-14T00:00:00Z"), 0),
            Gate::IneligibleUntil("2008-02-13".to_owned())
        );
    }

    #[test]
    fn the_named_date_crosses_month_and_year_ends() {
        assert_eq!(
            gate(false, Some("2025-12-15T12:00:00Z"), 0),
            Gate::IneligibleUntil("2026-01-15".to_owned())
        );
        // Across a leap February: thirty days from mid-day Jan 31
        // 2024 end mid-day Mar 1, so the named day is Mar 2.
        assert_eq!(
            gate(false, Some("2024-01-31T12:00:00Z"), 0),
            Gate::IneligibleUntil("2024-03-02".to_owned())
        );
    }

    #[test]
    fn a_missing_or_malformed_created_at_denies_a_new_user() {
        assert_eq!(gate(false, None, 0), Gate::Deny);
        for malformed in [
            "",
            "garbage",
            "2008-01-14",
            "2008-01-14T04:33:35",
            "2008-01-14T04:33:35+00:00",
            "2008-01-14T04:33:35.000Z",
            // Each separator position individually (a right-length
            // input wrong in only that byte), and a trailing space in
            // place of the `Z`.
            "2008x01-14T04:33:35Z",
            "2008-01x14T04:33:35Z",
            "2008-01-14 04:33:35Z",
            "2008-01-14T04x33:35Z",
            "2008-01-14T04:33x35Z",
            "2008-01-14T04:33:35 ",
            // The digit guard alone must catch this: `u64::from_str`
            // accepts a leading `+`, so without the guard `+4` would
            // parse as hour 4.
            "2008-01-14T+4:33:35Z",
            "2008-13-14T04:33:35Z",
            "2008-00-14T04:33:35Z",
            "2008-01-00T04:33:35Z",
            "2008-01-32T04:33:35Z",
            "2008-01-14T24:33:35Z",
            "2008-01-14T04:60:35Z",
            "2008-01-14T04:33:60Z",
            "2023-02-29T00:00:00Z",
            "2100-02-29T00:00:00Z",
            "1969-12-31T23:59:59Z",
            "20o8-01-14T04:33:35Z",
            "+008-01-14T04:33:35Z",
        ] {
            assert_eq!(gate(false, Some(malformed), 0), Gate::Deny, "{malformed:?}");
        }
    }

    #[test]
    fn timestamps_parse_to_the_documented_epochs() {
        assert_eq!(parse_utc_secs("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(parse_utc_secs(OCTOCAT_CREATED), Some(OCTOCAT_SECS));
        // Leap days in years divisible by four exist, century
        // non-leap-years' do not (2000's does, 2100's does not).
        assert_eq!(parse_utc_secs("2024-02-29T00:00:00Z"), Some(1_709_164_800));
        assert!(parse_utc_secs("2000-02-29T00:00:00Z").is_some());
        assert!(parse_utc_secs("2100-02-29T00:00:00Z").is_none());
    }

    #[test]
    fn civil_and_days_round_trip() {
        for days in (0..60_000).step_by(97) {
            let (y, m, d) = civil_from_days(days);
            assert_eq!(days_from_civil(y, m, d), days, "{y:04}-{m:02}-{d:02}");
        }
    }
}
