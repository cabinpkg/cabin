//! The backup-replication alert (`docs/runbook.md`, "Disaster recovery"):
//! verified archive blobs replicate from BLOBS to BACKUP through a queue
//! the breaker cron drains. Pure and host-testable; the wasm32 glue
//! reads the count.

/// The backup-health alert for one breaker pass, `None` when healthy.
/// Raised while any verified-archive backup has sat in the replication
/// queue for over an hour, and when that count could not be read - a
/// backup system's classic failure mode is stopping silently, so
/// unhealthy states alert on every pass until resolved.
pub fn alert(stale_backups: Option<u64>) -> Option<String> {
    match stale_backups {
        Some(0) => None,
        Some(1) => Some(
            "1 verified archive blob is overdue for backup replication \
             (check the drain; `cargo registry-backup-backfill` from the repository \
             root recovers by hand)"
                .to_owned(),
        ),
        Some(n) => Some(format!(
            "{n} verified archive blobs are overdue for backup replication \
             (check the drain; `cargo registry-backup-backfill` from the repository \
             root recovers by hand)"
        )),
        None => Some("backup health could not be read from d1".to_owned()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn alert_covers_the_backup_backlog_and_an_unreadable_count() {
        assert_eq!(alert(Some(0)), None);
        let one = alert(Some(1)).unwrap();
        assert!(one.contains("1 verified archive blob is overdue"), "{one}");
        let two = alert(Some(2)).unwrap();
        assert!(
            two.contains("2 verified archive blobs are overdue"),
            "{two}"
        );
        let unreadable = alert(None).unwrap();
        assert!(unreadable.contains("could not be read"), "{unreadable}");
    }
}
