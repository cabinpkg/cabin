//! Every SQL statement the Worker executes, in one place. (Operational
//! scripts under `scripts/` run their own SQL through `wrangler d1`;
//! this module owns the service's execution paths only.)
//!
//! All execution goes through D1 `prepare`, and every runtime value
//! rides a `?N` bind - parameterization is what injection safety rests
//! on; the few fixed queries take no input at all. These consts are the
//! single home
//! of the executed strings so the host-target validation test
//! (`tests/sql_validation/`) can prepare each one against the real,
//! from-zero migrated schema - catching typos, wrong column names, and
//! schema drift at test time - and so the CI guard
//! (`cargo check-sql`) can keep new call sites from bypassing it.
//! See `docs/architecture.md`, "Why no ORM".

/// Declares one documented `pub const` per statement and collects the
/// module's statements into its `STATEMENTS` group, one entry of
/// [`ALL`], so the validation test cannot silently miss one. `literal`
/// (not `expr`) on purpose: computed SQL has no business here.
macro_rules! statements {
    ($($(#[$doc:meta])* $name:ident = $sql:literal;)+) => {
        $($(#[$doc])* pub const $name: &str = $sql;)+

        /// This module's executed statements, one group of [`super::ALL`].
        #[cfg(not(target_arch = "wasm32"))]
        pub(super) static STATEMENTS: &[&str] = &[$($name),+];
    };
}

/// Declares the statement modules and re-exports every statement at
/// `sql::` - call sites and the `cargo check-sql` guard spell statements
/// `sql::NAME` - collecting each module's group into [`ALL`] in the same
/// breath: a module cannot be declared without its statements joining
/// the validation set.
macro_rules! groups {
    ($($module:ident),+ $(,)?) => {
        $(pub mod $module;)+
        $(pub use $module::*;)+

        /// Every executed statement, for `tests/sql_validation/`; the
        /// deployed Worker only ever uses the individual consts.
        #[cfg(not(target_arch = "wasm32"))]
        pub static ALL: &[&[&str]] = &[$($module::STATEMENTS),+];
    };
}

groups!(
    auth, backup, downloads, meta, packages, quota, scopes, trustpub
);
