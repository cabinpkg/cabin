# Cabin website instructions

The repository-root `AGENTS.md` also applies. Run website commands from this
directory.

## Static architecture

- Keep the public website static. Generate package pages at build time from the
  registry's catalog (`../docs/remote-registry.md`, "The catalog"), which
  `npm run build` and `npm run dev` fetch anonymously into `.catalog/`
  (`scripts/fetch-catalog.mjs`). The registry decides which packages and
  versions exist, which is latest, and their order; do not re-derive any of it
  here, authenticate the fetch, or add another package-data source. The
  private-alpha `401` renders the unavailable state: no package pages.
- `npm run build:fixtures` builds both catalog states from
  `fixtures/catalog.json` into `dist-fixtures/`. A fixture must never reach
  `dist/` or a deploy.
- Keep registry statistics and other runtime data that enhance public pages as
  progressive enhancements. Those pages must retain meaningful script-less and
  offline content. Keep fetch-only UI hidden until its request succeeds.

## UI

- Avoid redundant secondary labels and descriptions. Let layout and visual
  hierarchy clarify structure, while preserving essential context,
  instructions, and accessible labels.

## Package identity

- Treat package names as exactly two non-empty slash-separated segments:
  `<scope>/<name>`, routed under `/packages/<scope>/<name>`; version routes
  append `/<version>`.
- Consumer snippets must quote the scoped dependency key and require the
  version: `"<scope>/<name>" = "=<version>"`. Display and require that version
  verbatim. Packaging revisions stay separate from versions and must not appear
  in version strings or requirements.
- Leave metadata absent when the catalog does not provide it. Do not
  synthesize values or render empty placeholders.

## Docs rendering

- Render the repository-root `../docs/*.md` files; do not maintain a website
  copy. Add each new top-level docs page to `src/lib/docsNav.ts`.
- Preserve the Markdown processor's relative `.md` link rewriting and linked
  heading IDs when changing docs rendering. `npm run verify:docs-links` checks
  the generated links.
- Built HTML must not contain inline scripts under the site's CSP. Keep docs
  interactivity in external scripts and preserve
  `vite.build.assetsInlineLimit: 0`.

## Build-time traps

- `npm run typecheck` (`astro check`) does not exercise static paths or
  build-time data loading. After changing routes or build-time loaders, run
  `npm run build:fixtures` and a clean production build:
  `/bin/rm -rf dist .astro .catalog && npm run build`.
- Build-time code that needs repository-root paths must resolve them from
  `process.cwd()`. Do not derive them from `import.meta.url`; Astro relocates
  bundled modules under `dist/.prerender/chunks/`.

## Verification

- The website gate referenced by the root `AGENTS.md` is `npm ci`,
  `npm run lint`, `npm test`, and `npm run build:fixtures`.
- `npm run build:fixtures` runs type checking, then the Astro build and
  `npm run verify` once per catalog state. The verify scripts check the
  generated output for CSP, docs links, progressive independence, and
  meaningful script-less content, with package-page expectations taken from
  the catalog the build read. `npm run build`, the deploy build, does the same
  for the live catalog into `dist/`.
