# Security Policy

## Scope

This policy covers the Cabin client - the Rust crates in this repository and the released
`cabin` binaries - and the hosted registry service under `registry/` (the Cloudflare Worker
behind `registry.cabinpkg.com` plus its browser and API planes
mounted on `cabinpkg.com` - `/login`, `/callback`, and `/api/*` - including authentication,
publish/yank/verification APIs, and stored package data).

## Reporting a Vulnerability

Please go to <https://github.com/cabinpkg/cabin/security/advisories/new> to report a vulnerability.
