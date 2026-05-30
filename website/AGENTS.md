# AGENTS.md — Cabin website

Operational guide for agents working in `website/` (the static
[cabinpkg.com](https://cabinpkg.com) site). The repo-root
[`AGENTS.md`](../AGENTS.md) and [`CLAUDE.md`](../CLAUDE.md) still
apply; this file adds website-specific knowledge. Run all commands
from `website/`.

## What this is

A fully static Astro build. Package pages are generated **at build
time from the `crates/cabin-port/ports/` directory** (curated
foundation port recipes) — not from a database or API.

## Data source (read this first)

- Package data comes from
  `../crates/cabin-port/ports/<name>/<version>/port.toml`,
  loaded by `src/lib/ports.ts` → `PackageRecord[]`, consumed by
  `src/lib/packages.ts`. One record per `port.toml`.
- The Hasura GraphQL endpoint is **no longer used**. The GraphQL
  scaffolding (`codegen.ts`, `graphql/getAllPackages.gql`, the
  `graphql*`/`@graphql-codegen/*` deps) is intentionally kept
  dormant for possible future use — do not assume it is live.
  `yarn generate` (codegen against Hasura) is the only thing that
  still touches the network, and it is **not** part of `dev`/`build`.
- `README.md`'s "package data is fetched from Hasura at build time"
  wording predates this migration and is stale.

## Commands

- `yarn dev` — dev server (`astro dev`).
- `yarn build` — `yarn typecheck && astro build && yarn verify:csp`;
  writes the static site to `dist/`.
- `yarn typecheck` — `astro check`.
- `yarn lint` — Biome. `yarn fmt` — Biome format `--write`.
- `yarn verify:csp` — fails if any built HTML has an inline `<script>`.
- `yarn generate` — regenerate GraphQL types from Hasura (manual,
  dormant; not used by `dev`/`build`).
- Node >= 22.

## Build-time gotchas (learned the hard way)

1. **`yarn typecheck` passing does NOT mean the build passes.**
   Data-loading and `getStaticPaths` errors surface only during
   `astro build` (static route generation), not during `astro check`.
   After any change to data loading or routes, run a full clean build
   and confirm the output — never trust typecheck alone:

   ```bash
   /bin/rm -rf dist .astro && yarn build
   # expect: dist/packages/ports/<name>/index.html, dist/packages.json
   ```

2. **Never resolve repo paths via `import.meta.url`.** Under
   `astro build`, modules are bundled into `dist/.prerender/chunks/`
   at a different depth than `src/`, so a relative offset that works
   in `astro dev` (Vite serves source) resolves to the wrong
   directory in the build. `src/lib/ports.ts` finds the
   `crates/cabin-port/ports/` dir by walking up from `process.cwd()`
   — keep it cwd-based.

3. **The recipes live outside this project** in the cabin-port crate
   (`crates/cabin-port/ports/`). Both local `yarn build` and CI
   (`.github/workflows/website.yml`, `working-directory: website`)
   run with cwd = `website/`, so the `process.cwd()` walk-up lands on
   `../crates/cabin-port/ports`.

## Routing & data model

- Routes: `/packages/<group>/<name>` (latest) and
  `/packages/<group>/<name>/<version>`. A package name must be
  exactly two slash-separated, non-empty segments.
- Ports use a synthetic `ports/` group: a `port.toml` named `zlib`
  becomes `PackageRecord.name = "ports/zlib"` → `/packages/ports/zlib`.
  The **bare** port name (group prefix stripped) is what goes in a
  consumer's `cabin.toml`.
- Port pages have no README, edition, or publish date; those UI
  sections are conditionally hidden — don't render empty placeholders.
- The install snippet must use the bundled-port form,
  `<name> = { port = true, version = "<v>" }` under `[dependencies]`
  (see [`docs/foundation-ports.md`](../docs/foundation-ports.md)), not
  the old registry `"name" = "version"` form.

## Conventions

- Biome: 4-space indent, double quotes, recommended ruleset (so e.g.
  no `while (true)` — use a real loop condition). It lints
  `.ts`/`.js`/`.css`/config but **excludes `.astro`** files (Astro's
  own parser and `astro check` cover those).
- Commits: Conventional Commits, lowercase subject, ≤100 chars
  (commitlint, enforced repo-wide).

## Deploy

Cloudflare Workers Static Assets serving `./dist` (`wrangler.jsonc`).
Build then deploy: `yarn build && yarn wrangler deploy`. No deploy
workflow is committed (account/secrets vary by environment); CI
`website.yml` only lints and builds.

## Key files

- `src/lib/ports.ts` — scans
  `crates/cabin-port/ports/*/*/port.toml`, returns `PackageRecord[]`.
- `src/lib/packages.ts` — grouping, latest-version selection, route
  generation, search index. Memoizes the loader (one disk read per
  build).
- `src/lib/types.ts` — `PackageRecord` and related types.
- `src/components/package/` — detail view, meta grid, install snippet,
  README renderer.
- `src/pages/packages/[group]/…` — package routes;
  `src/pages/packages.json.ts` — search-index endpoint.
