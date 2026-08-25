# Cabin website instructions

The repository-root `AGENTS.md` also applies. Run website commands from this
directory.

## Static architecture

- Keep the public website static. Generate package pages at build time from the
  committed foundation ports under `../ports/`; do not add a live-registry
  dependency to static generation.
- Keep registry statistics and other runtime data that enhance public pages as
  progressive enhancements. Those pages must retain meaningful script-less and
  offline content. Keep fetch-only UI hidden until its request succeeds.

## Package identity

- Treat package names as exactly two non-empty slash-separated segments:
  `<group>/<name>`. A foundation port directory named `<name>` has the real
  registry identity `cabin-ports/<lowercase-name>` and routes under
  `/packages/cabin-ports/<lowercase-name>`; version routes append
  `/<upstream-version>`.
- Consumer snippets must quote the scoped dependency key and require the
  upstream version: `"cabin-ports/<lowercase-name>" = "=<upstream-version>"`.
  Display and require that upstream version verbatim. Packaging revisions stay
  separate from versions and must not appear in version strings or
  requirements. See `../docs/foundation-ports.md`.
- Leave metadata absent when foundation-port manifests do not provide it. Do
  not synthesize values or render empty placeholders.

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
  build-time data loading. After changing routes or build-time loaders, run a
  clean production build: `/bin/rm -rf dist .astro && npm run build`.
- Build-time code that needs repository-root paths must resolve them from
  `process.cwd()`. Do not derive them from `import.meta.url`; Astro relocates
  bundled modules under `dist/.prerender/chunks/`.

## Verification

- Use the website checks required by the root `AGENTS.md`: `npm ci`,
  `npm run lint`, `npm run build`, and `npm test`.
- `npm run build` runs type checking, the Astro production build, and
  `npm run verify`. The verify scripts check generated `dist/` output for CSP,
  docs links, progressive independence, and meaningful script-less content.
