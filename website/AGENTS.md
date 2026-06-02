# AGENTS.md — Cabin website

Operational guide for agents working in `website/` (the static
[cabinpkg.com](https://cabinpkg.com) site). The repo-root
[`AGENTS.md`](../AGENTS.md) and [`CLAUDE.md`](../CLAUDE.md) still
apply; this file adds website-specific knowledge. Run all commands
from `website/`.

## What this is

A fully static Astro build. Package pages are generated **at build
time from the `crates/cabin-port/ports/` directory** (curated
foundation port recipes) — not from a database or API. The
documentation pages are generated from the repository-root
[`docs/`](../docs/) Markdown and render at `/docs/` (see **Docs**
below).

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
- `yarn build` — `yarn typecheck && astro build && yarn verify`;
  writes the static site to `dist/`.
- `yarn typecheck` — `astro check`.
- `yarn lint` — Biome. `yarn fmt` — Biome format `--write`.
- `yarn verify` — runs `verify:csp` + `verify:docs-links` against the
  built `dist/` (run after a build).
- `yarn verify:csp` — fails if any built HTML has an inline `<script>`.
- `yarn verify:docs-links` — fails if a built docs page has an
  unresolved `/docs/...` link or an un-rewritten relative `*.md` link.
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

## Docs

The canonical docs are the repository-root [`docs/`](../docs/) Markdown
files — they are **not** moved into this project. They render here as a
content collection:

- `src/content.config.ts` defines the `docs` collection with a `glob`
  loader: `pattern: "*.md", base: "../docs"`. `base` is relative to the
  Astro project root, so it resolves to `<repo-root>/docs`. `*.md`
  (top-level only) matches the flat published pages and structurally
  skips the git-ignored `docs/superpowers/` agent workspace.
- `src/pages/docs/[...slug].astro` renders each entry: `index.md` →
  `/docs/`, `<name>.md` → `/docs/<name>/`.
- `src/lib/docsNav.ts` is the sidebar nav, ported from the old
  `mkdocs.yml`. Add every new `docs/*.md` page here, or the build's
  `assertDocsNavMatches` guard fails.
- `src/lib/remark-docs-links.ts` rewrites the docs' relative `*.md`
  cross-links (e.g. `manifest.md#targets` → `/docs/manifest/#targets`).
  It is wired in via `markdown.processor` (the `unified()` pipeline) in
  `astro.config.ts` — without that, content links are not rewritten.
  Code highlighting (Shiki) is built in with no extra config; the
  heading ids and the clickable heading anchors come from the
  explicitly configured `rehype-slug` + `rehype-autolink-headings`
  (slug first, so it supplies the ids the autolink step wraps).
- `src/layouts/DocsLayout.astro` is the docs shell (sidebar, prose,
  on-this-page TOC, prev/next, edit link). Its interactivity
  (copy-link headings, TOC scroll-spy, keyboard-scrollable tables)
  lives in `src/scripts/docs.ts`, loaded as an external `<script src>`
  (kept external by `vite.build.assetsInlineLimit: 0`) so it satisfies
  the no-inline-script CSP check.

Same build-time gotcha as ports data: route/collection errors surface
only during `astro build`, not `astro check`. Always run a full clean
build (`/bin/rm -rf dist .astro && yarn build`).

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

### `docs.cabinpkg.com` cutover (one-time, manual)

Docs used to be a separate MkDocs site published to GitHub Pages at
`docs.cabinpkg.com`; they now render here at `cabinpkg.com/docs/`.
Deleting `mkdocs.yml` and the old `docs.yml` workflow does **not** stop
the old subdomain — the `gh-pages` branch and the repo's Pages setting
keep serving the frozen MkDocs site. To finish the migration, repoint
`docs.cabinpkg.com` in a single step so the links to it (still
referenced in the root `README.md` and `INSTALL.md`) never 404:

1. **Stand up the Cloudflare redirect first**, then retire Pages — or do
   both as one DNS cutover. Add a redirect `docs.cabinpkg.com/*` →
   `https://cabinpkg.com/docs/*` (301, path-preserving): the root `/`
   maps to `/docs/` (the old `index.md` home), and browsers re-apply any
   `#fragment` across the redirect, so deep links and anchors keep
   working. Bringing the redirect up before (or with) the Pages teardown
   avoids a window where the subdomain serves nothing.
2. **Disable GitHub Pages** for the repo (Settings → Pages) and delete
   the `gh-pages` branch so the stale MkDocs site stops serving.
   (Removing `docs/CNAME` was cosmetic — it only fed `mkdocs gh-deploy`;
   the live `gh-pages` branch keeps its own copy.)

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
- `src/content.config.ts` — `docs` content collection (root `docs/`).
- `src/pages/docs/[...slug].astro` — docs route; `src/lib/docsNav.ts` —
  sidebar nav; `src/lib/remark-docs-links.ts` — `*.md` link rewriter;
  `src/layouts/DocsLayout.astro` — docs shell;
  `scripts/verify-docs-links.mjs` — docs link check.
