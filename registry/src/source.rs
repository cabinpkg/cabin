//! The source viewer's ranged-read policy (`docs/architecture.md`,
//! "Origins and roles"): pure and host-testable. The session-plane
//! source route proxies ranged R2 reads of a verified version's archive;
//! this module decides which `Range` headers are acceptable and how a
//! request maps onto the stored blob.
//!
//! The policy deliberately deviates from RFC 9110, which says to ignore
//! an absent or malformed `Range` and serve the whole representation:
//! the route exists to serve bounded slices, so an absent header is a
//! `400` and everything that is not a single bounded range within
//! [`MAX_RANGE_BYTES`] - multiple ranges, an open-ended
//! `bytes=<start>-`, an oversized span - is refused with `416` instead
//! of falling back to streaming the archive.

/// The largest slice one request may read. A per-request resource
/// bound, nothing more: sequential requests can still walk the whole
/// archive - the viewer needs exactly that, and a signed-in user could
/// mint a token and download the artifact anyway - but no single
/// request can stream a 16 MiB blob, and the viewer's common requests
/// (the 64 KiB end-of-central-directory tail, one central directory,
/// one file's compressed span) usually fit in one.
pub const MAX_RANGE_BYTES: u64 = 4 * 1024 * 1024;

/// A refused `Range` header: the status plus a fixed detail string that
/// never echoes request bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RangeRefusal {
    pub status: u16,
    pub detail: &'static str,
}

/// `400`: the header is required - without one the request asks for the
/// whole archive, which is exactly what the route refuses to stream.
pub const RANGE_REQUIRED: RangeRefusal = RangeRefusal {
    status: 400,
    detail: "a bounded range header is required",
};
/// `416`: a header was sent but refused by the policy.
pub const INVALID_RANGE: RangeRefusal = RangeRefusal {
    status: 416,
    detail: "the range header must be a single bytes=<start>-<end> or \
     bytes=-<suffix> range of at most 4 MiB",
};
/// The fixed `416` detail for a well-formed range starting at or past
/// the end of the archive.
pub const RANGE_UNSATISFIABLE: &str = "the range does not overlap the archive";

/// A parsed, policy-cleared `Range` header, not yet checked against the
/// archive size.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RangeRequest {
    /// `bytes=<start>-<end>`, inclusive on both ends.
    Bounded { start: u64, end: u64 },
    /// `bytes=-<len>`: the last `len` bytes. Accepted because the
    /// viewer's first request must read the end-of-central-directory
    /// tail before it knows the archive size; the length keeps it as
    /// bounded as any other range.
    Suffix { len: u64 },
}

/// A range resolved against the archive size: the exact R2 read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResolvedRange {
    pub offset: u64,
    pub length: u64,
}

/// Digits only: no sign, no whitespace, no empty string. Stricter than
/// `str::parse`, which tolerates a leading `+`.
fn parse_u64(value: &str) -> Option<u64> {
    if value.is_empty() || !value.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    value.parse().ok()
}

/// Parses a `Range` header under the policy above.
///
/// # Errors
///
/// [`RANGE_REQUIRED`] for an absent header; [`INVALID_RANGE`] for
/// anything but a single `bytes=` range, an open-ended range, an empty
/// suffix, or a length over [`MAX_RANGE_BYTES`]. (A multi-range's `,`
/// lands in one of the digit fields and fails there.)
pub fn parse_range(header: Option<&str>) -> Result<RangeRequest, RangeRefusal> {
    let spec = header
        .ok_or(RANGE_REQUIRED)?
        .strip_prefix("bytes=")
        .ok_or(INVALID_RANGE)?;
    let (start, end) = spec.split_once('-').ok_or(INVALID_RANGE)?;
    if start.is_empty() {
        let len = parse_u64(end).ok_or(INVALID_RANGE)?;
        if len == 0 || len > MAX_RANGE_BYTES {
            return Err(INVALID_RANGE);
        }
        return Ok(RangeRequest::Suffix { len });
    }
    let start = parse_u64(start).ok_or(INVALID_RANGE)?;
    let end = parse_u64(end).ok_or(INVALID_RANGE)?;
    let length = end
        .checked_sub(start)
        .and_then(|span| span.checked_add(1))
        .ok_or(INVALID_RANGE)?;
    if length > MAX_RANGE_BYTES {
        return Err(INVALID_RANGE);
    }
    Ok(RangeRequest::Bounded { start, end })
}

/// Resolves a parsed range against the archive size, HTTP-style: an end
/// past the last byte is clamped, a suffix longer than the archive is
/// the whole archive, and a start at or past the end is unsatisfiable
/// (`None`; the archive is never empty - a profile zip is at least its
/// 22-byte end record - so a zero-size row only arises from a corrupt
/// column and refuses fail-safe).
pub fn resolve_range(request: RangeRequest, size: u64) -> Option<ResolvedRange> {
    match request {
        RangeRequest::Bounded { start, end } => {
            if start >= size {
                return None;
            }
            let end = end.min(size - 1);
            Some(ResolvedRange {
                offset: start,
                length: end - start + 1,
            })
        }
        RangeRequest::Suffix { len } => {
            if size == 0 {
                return None;
            }
            let length = len.min(size);
            Some(ResolvedRange {
                offset: size - length,
                length,
            })
        }
    }
}

/// The `206` response's `Content-Range: bytes <first>-<last>/<size>`.
pub fn content_range(resolved: ResolvedRange, size: u64) -> String {
    format!(
        "bytes {}-{}/{size}",
        resolved.offset,
        resolved.offset + resolved.length - 1
    )
}

/// The unsatisfiable `416` response's `Content-Range: bytes */<size>`,
/// which tells the viewer the actual size to retry with.
pub fn unsatisfiable_content_range(size: u64) -> String {
    format!("bytes */{size}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bounded_and_suffix_ranges_parse() {
        assert_eq!(
            parse_range(Some("bytes=0-21")),
            Ok(RangeRequest::Bounded { start: 0, end: 21 })
        );
        assert_eq!(
            parse_range(Some("bytes=100-100")),
            Ok(RangeRequest::Bounded {
                start: 100,
                end: 100
            })
        );
        assert_eq!(
            parse_range(Some("bytes=-65536")),
            Ok(RangeRequest::Suffix { len: 65_536 })
        );
        // Exactly the cap is allowed, from either end.
        assert_eq!(
            parse_range(Some(&format!("bytes=0-{}", MAX_RANGE_BYTES - 1))),
            Ok(RangeRequest::Bounded {
                start: 0,
                end: MAX_RANGE_BYTES - 1
            })
        );
        assert_eq!(
            parse_range(Some(&format!("bytes=-{MAX_RANGE_BYTES}"))),
            Ok(RangeRequest::Suffix {
                len: MAX_RANGE_BYTES
            })
        );
    }

    #[test]
    fn everything_else_is_refused() {
        // Absent, malformed, open-ended, multi-range, oversized, signed,
        // padded: the policy accepts exactly the two bounded forms.
        let over = MAX_RANGE_BYTES;
        for header in [
            "",
            "bytes=",
            "bytes=-",
            "bytes=0",
            "bytes=0-",
            "bytes=abc-5",
            "bytes=0-abc",
            "bytes=5-0",
            "bytes=0-5,10-20",
            "bytes=-5,10-20",
            "bytes= 0-5",
            "bytes=+0-5",
            "bytes=-0",
            "octets=0-5",
            "0-5",
            &format!("bytes=0-{over}"),
            &format!("bytes=-{}", over + 1),
            "bytes=0-99999999999999999999",
        ] {
            assert_eq!(parse_range(Some(header)), Err(INVALID_RANGE), "{header:?}");
            assert_eq!(INVALID_RANGE.status, 416);
        }
        // No header at all is the 400 - the client forgot the header -
        // not the 416 range refusal.
        assert_eq!(parse_range(None), Err(RANGE_REQUIRED));
        assert_eq!(RANGE_REQUIRED.status, 400);
    }

    #[test]
    fn resolution_clamps_ends_and_suffixes() {
        let range = |start, end| RangeRequest::Bounded { start, end };
        assert_eq!(
            resolve_range(range(0, 21), 100),
            Some(ResolvedRange {
                offset: 0,
                length: 22
            })
        );
        // An end past the last byte is clamped, HTTP-style.
        assert_eq!(
            resolve_range(range(90, 199), 100),
            Some(ResolvedRange {
                offset: 90,
                length: 10
            })
        );
        assert_eq!(
            resolve_range(RangeRequest::Suffix { len: 22 }, 100),
            Some(ResolvedRange {
                offset: 78,
                length: 22
            })
        );
        // A suffix longer than the archive is the whole archive.
        assert_eq!(
            resolve_range(RangeRequest::Suffix { len: 65_536 }, 100),
            Some(ResolvedRange {
                offset: 0,
                length: 100
            })
        );
    }

    #[test]
    fn starts_past_the_end_are_unsatisfiable() {
        for start in [100, 101, u64::MAX - 1] {
            assert_eq!(
                resolve_range(
                    RangeRequest::Bounded {
                        start,
                        end: u64::MAX - 1
                    },
                    100
                ),
                None,
                "start: {start}"
            );
        }
        // A zero-size row (a corrupt column; profile archives are never
        // empty) refuses everything.
        assert_eq!(
            resolve_range(RangeRequest::Bounded { start: 0, end: 0 }, 0),
            None
        );
        assert_eq!(resolve_range(RangeRequest::Suffix { len: 22 }, 0), None);
    }

    #[test]
    fn content_range_headers_match_the_http_shape() {
        assert_eq!(
            content_range(
                ResolvedRange {
                    offset: 78,
                    length: 22
                },
                100
            ),
            "bytes 78-99/100"
        );
        assert_eq!(unsatisfiable_content_range(100), "bytes */100");
    }
}
