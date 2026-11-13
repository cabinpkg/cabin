//! meta: service state.

statements! {
    /// One `meta` row by key.
    META_VALUE = "SELECT value FROM meta WHERE key = ?1";

    /// Upserts one `meta` row.
    UPSERT_META =
        "INSERT INTO meta (key, value) VALUES (?1, ?2) \
         ON CONFLICT (key) DO UPDATE SET value = excluded.value";
}
