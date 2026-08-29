# registry/AGENTS.md

Context-routing guide for the hosted registry Worker. The goal is to keep
the files read per change small: start from the narrowest module that owns
the behavior, open documents by section, and follow the links instead of
restating their invariants.

## Authoritative documents

- `../docs/remote-registry.md` — the protocol: routes, authentication,
  status codes, the error envelope. Client-visible behavior is specified
  here.
- `docs/architecture.md` — the hosted service: storage, origins and roles,
  credential planes, the write path, the verification lifecycle, the
  governor and breaker, code layout.
- `docs/runbook.md` — operations: provisioning, wipe, deploys, incident
  procedure. Needed for ops work, not for code changes.

Do not read `README.md` or these documents end to end by default; jump to
the section the change needs.

## Where behavior lives

Domain modules compile and unit-test on the host target; put behavior,
policy, and validation changes there. The wasm32-only `*glue*` modules are
Cloudflare runtime integration — binding access, D1/R2/cache I/O, dispatch —
and are not the default owner of domain behavior: keep glue a thin caller.

| Area | Modules |
| --- | --- |
| Route matching, role-per-hostname, path validation | `src/routes.rs` |
| Publish validation and policy | `src/publish.rs` (+ `src/names.rs`, `src/quota.rs`; runtime write path in `src/glue.rs`) |
| Scope claims and membership rules | `src/claim.rs` |
| Bearer tokens, scopes, auth header | `src/auth.rs` |
| Login-session tokens (`cabin login`) | `src/session_tokens.rs` |
| Trusted publishing (Actions OIDC) | `src/trustpub.rs` |
| Verification lifecycle and read gate | `src/verify.rs` |
| Served JSON documents and errors | `src/documents.rs`, `src/error.rs`, `src/stats.rs`, `src/user_api.rs` |
| SQL statements (one authoritative home) | `src/sql.rs`, validated by `tests/sql_validation.rs` |
| Cost governor / budget breaker | `src/governor.rs`, `src/breaker.rs` |
| Browser cookies, CSRF, session plane | `src/session.rs` (runtime in `src/web_glue.rs`) |
| Cloudflare runtime glue (wasm32) | `src/glue.rs` (dispatch, read plane, Bearer planes), `src/web_glue.rs` (OAuth/session), `src/backup_glue.rs`, `src/governor_client.rs`, `src/governor_do.rs` |

Unless the task is about them, do not read or touch the governor and
breaker, the backup modules, `migrations/` (append-only, applied manually),
`wrangler.jsonc` (deployment), or the root-workspace `crates/xtask-*`
registry tooling.

## Checks

This directory is a standalone workspace: the root `cargo ci` gate does not
cover it. From `registry/`:

```sh
cargo fmt --all -- --check
cargo test
cargo clippy --all-targets -- -D warnings
cargo clippy --target wasm32-unknown-unknown -- -D warnings
```

From the repository root, the CI-only lexical guards: `cargo check-sql`,
`cargo check-r2`, `cargo check-deploy`. For changes to dispatch, routing, or
another end-to-end surface, also run the local smoke test:
`CABIN_REGISTRY_SMOKE_TOKEN=cabin_smoke cargo registry-smoke`.
