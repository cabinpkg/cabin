# AGENTS.md - Cabin website

Rules for `website/`, the fully static Astro site for
[cabinpkg.com](https://cabinpkg.com). The repo-root `AGENTS.md` still
applies. Run all commands from `website/`. Node >= 22.18 (the account
test suite runs `.ts` files directly via Node's type stripping, default
from 22.18).

## Data sources

- Package pages are generated at build time from
  `../crates/cabin-port/ports/<name>/<version>/port.toml` (curated
  foundation-port recipes) - no database or API. `src/lib/ports.ts` loads
  one `PackageRecord` per `port.toml`; `src/lib/packages.ts` does grouping,
  latest-version selection, route generation, and the search index (loader
  memoized: one disk read per build). `src/pages/packages.json.ts` is the
  search-index endpoint.
- Docs pages render the repository-root `../docs/` Markdown - the files are
  NOT moved into this project. `src/content.config.ts` defines the `docs`
  collection with a glob loader (`pattern: "*.md", base: "../docs"`,
  top-level only, which structurally skips the git-ignored
  `docs/superpowers/`); `src/pages/docs/[...slug].astro` renders each entry
  (`index.md` -> `/docs/`, `<name>.md` -> `/docs/<name>/`).
- The homepage's registry stats band (packages/versions/downloads) and
  the dashboard's download figures are same-origin fetches of the
  registry's public `/api/v1/stats` endpoint and the session packages
  API (`registry/docs/architecture.md`, "Download counts"); both are
  progressive enhancements - the static HTML renders without them.

## Commands

- `npm run dev` / `npm run typecheck` (`astro check`) / `npm run lint` (Biome) /
  `npm run fmt` (Biome `--write`).
- `npm run build` = `npm run typecheck && astro build && npm run verify`; writes
  the static site to `dist/`.
- `npm run verify` = `verify:csp` (fails on any inline `<script>` in built
  HTML) + `verify:docs-links` (fails on unresolved `/docs/...` or
  un-rewritten relative `*.md` links), both against the built `dist/`.

## Build-time gotchas

1. `npm run typecheck` passing does NOT mean the build passes. Data-loading,
   `getStaticPaths`, and content-collection errors surface only during
   `astro build`, never during `astro check`. After any change to data
   loading or routes, run a full clean build and confirm the output:
   `/bin/rm -rf dist .astro && npm run build` (expect
   `dist/packages/ports/<name>/index.html` and `dist/packages.json`).
2. Never resolve repo paths via `import.meta.url`: under `astro build`,
   modules are bundled into `dist/.prerender/chunks/` at a different depth
   than `src/`, so relative offsets that work in `astro dev` break in the
   build. `src/lib/ports.ts` finds `crates/cabin-port/ports/` by walking up
   from `process.cwd()` (cwd is `website/` both locally and in CI) - keep it
   cwd-based.

## Routing & data model

- Routes: `/packages/<group>/<name>` (latest) and
  `/packages/<group>/<name>/<version>`. A package name is exactly two
  non-empty slash-separated segments. Ports use a synthetic `ports/` group:
  a `port.toml` named `zlib` becomes `PackageRecord.name = "ports/zlib"` ->
  `/packages/ports/zlib`; the bare port name (group prefix stripped) is what
  goes in a consumer's `cabin.toml`.
- Port pages have no README, edition, or publish date; those UI sections are
  conditionally hidden - don't render empty placeholders. The detail view
  lives in `src/components/package/`, routes in `src/pages/packages/`.
- The install snippet must use the bundled-port form
  `<name> = { port = true, version = "<v>" }` under `[dependencies]` (see
  `../docs/foundation-ports.md`), not the old registry `"name" = "version"`
  form.

## Docs rendering

- `src/lib/docsNav.ts` is the sidebar nav. Add every new `docs/*.md` page
  there, or the build's `assertDocsNavMatches` guard fails.
- `src/lib/remark-docs-links.ts` rewrites the docs' relative `*.md`
  cross-links (`manifest.md#targets` -> `/docs/manifest/#targets`); it is
  wired in via `markdown.processor` in `astro.config.ts` - without that
  wiring, content links are not rewritten. Heading ids and
  clickable anchors come from `rehype-slug` + `rehype-autolink-headings`
  (slug first); Shiki code highlighting needs no extra config.
- `src/layouts/DocsLayout.astro` is the docs shell; its interactivity lives
  in `src/scripts/docs.ts`, loaded as an external `<script src>` (kept
  external by `vite.build.assetsInlineLimit: 0` in `astro.config.ts`) so it
  passes the no-inline-script CSP check.

## Conventions & deploy

- Biome: 4-space indent, double quotes, recommended ruleset (so e.g. no
  `while (true)`). It lints `.ts`/`.js`/`.css`/config but excludes `.astro`
  files (Astro's own parser and `astro check` cover those).
- Deploy: Cloudflare Workers Static Assets serving `./dist`
  (`wrangler.jsonc`); `npm run build && npx wrangler deploy`. No deploy
  workflow is committed (account/secrets vary by environment); CI
  (`.github/workflows/website.yml`) only lints and builds.
