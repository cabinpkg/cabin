//! Download-telemetry batching policy (`docs/architecture.md`,
//! "Download counts"): pure and host-testable. The wasm glue buffers
//! per-version download counts in isolate memory and flushes them to D1
//! in one batch when this policy says so, replacing the old
//! one-D1-write-per-download pattern. Telemetry is approximate by
//! contract - an isolate that dies with a non-empty buffer loses those
//! counts - and never part of the hard accounting ledger.

/// Flush once this many distinct versions have pending counts, so one
/// D1 batch stays small.
pub const FLUSH_MAX_PENDING: usize = 50;

/// Flush when a download arrives this long after the last flush.
/// There is deliberately no timer: a lone count buffered on a quiet
/// isolate waits for the next download (or is lost with the isolate -
/// approximate by contract). The `DOWNLOAD_FLUSH_INTERVAL_MS` env var
/// overrides it; the smoke test pins 0 so every download flushes
/// immediately and stays observable.
pub const FLUSH_INTERVAL_MS: f64 = 30_000.0;

/// Whether the buffer should flush now.
pub fn should_flush(pending_versions: usize, elapsed_ms: f64, interval_ms: f64) -> bool {
    pending_versions > 0 && (pending_versions >= FLUSH_MAX_PENDING || elapsed_ms >= interval_ms)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_buffers_never_flush() {
        assert!(!should_flush(0, 0.0, FLUSH_INTERVAL_MS));
        assert!(!should_flush(
            0,
            FLUSH_INTERVAL_MS * 10.0,
            FLUSH_INTERVAL_MS
        ));
    }

    #[test]
    fn size_and_time_thresholds_are_exact() {
        assert!(!should_flush(FLUSH_MAX_PENDING - 1, 0.0, FLUSH_INTERVAL_MS));
        assert!(should_flush(FLUSH_MAX_PENDING, 0.0, FLUSH_INTERVAL_MS));
        assert!(!should_flush(1, FLUSH_INTERVAL_MS - 1.0, FLUSH_INTERVAL_MS));
        assert!(should_flush(1, FLUSH_INTERVAL_MS, FLUSH_INTERVAL_MS));
    }

    #[test]
    fn a_zero_interval_flushes_every_event() {
        assert!(should_flush(1, 0.0, 0.0));
    }
}
