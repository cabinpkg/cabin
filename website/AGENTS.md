# AGENTS.md - Cabin website

Rules for `website/`, the fully static Astro site for
[cabinpkg.com](https://cabinpkg.com). The repo-root `AGENTS.md` still
applies. Run all commands from `website/`. Node >= 22.18 (the account
test suite runs `.ts` files directly via Node's type stripping, default
from 22.18).

## Data sources

- Package pages are generated at build time from
  `../ports/<name>/<version>/` (curated foundation
  ports) - no database or API, and no live-registry build dependency.
  Every committed version directory is a provenance-bearing package (a
  single `cabin.toml` whose `[package.upstream]` supplies the record's
  provenance; its display fields are null - the manifest carries none -
  so those UI sections hide).  `src/lib/ports.ts` mirrors the
  `cabin-port-publish` identity
  rules: the record name is the scoped registry name
  `cabin-ports/<lowercase name>`, the version is the upstream version
  verbatim (packaging corrections are registry revisions, never a
  version-string suffix). Directories with no `cabin.toml` are
  skipped. `src/lib/packages.ts` does grouping,
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
  progressive enhancements. Every figure in the band is registry data,
  so the `<section data-registry-stats>` itself ships `hidden` and
  `src/scripts/home-stats.ts` reveals it only after a successful
  fetch - a script-less or offline visitor gets the page without the
  band rather than an empty strip.

## Commands

- `npm run dev` / `npm run typecheck` (`astro check`) / `npm run lint` (Biome) /
  `npm run fmt` (Biome `--write`) / `npm test` (`node --test` over
  `src/**/*.test.ts` and `scripts/**/*.test.mjs`).  `scripts/ci.sh` does not
  run `npm test`; CI does, so run it by hand.
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
   `dist/packages/cabin-ports/<name>/index.html` and `dist/packages.json`).
2. Never resolve repo paths via `import.meta.url`: under `astro build`,
   modules are bundled into `dist/.prerender/chunks/` at a different depth
   than `src/`, so relative offsets that work in `astro dev` break in the
   build. `src/lib/ports.ts` finds `ports/` by walking up
   from `process.cwd()` (cwd is `website/` both locally and in CI) - keep it
   cwd-based.

## Routing & data model

- Routes: `/packages/<group>/<name>` (latest) and
  `/packages/<group>/<name>/<version>`. A package name is exactly two
  non-empty slash-separated segments. The ports' group is the real registry
  scope: a port directory named `zlib` becomes
  `PackageRecord.name = "cabin-ports/zlib"` -> `/packages/cabin-ports/zlib`;
  the full quoted scoped name is what goes in a consumer's `cabin.toml`.
- Port pages have no README, edition, or publish date; those UI sections are
  conditionally hidden - don't render empty placeholders. The detail view
  lives in `src/components/package/`, routes in `src/pages/packages/`.
- The install snippet must use the quoted scoped registry form
  `"cabin-ports/<name>" = "=<upstream version>"` under `[dependencies]`
  (see `../docs/foundation-ports.md`, "Publishing ports as registry
  packages"). The dependency key needs quotes (it contains `/`); versions
  are plain upstream versions - the packaging-revision axis never appears
  in a requirement.

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
  (`.github/workflows/website.yml`) lints, builds, and runs `npm test`.
