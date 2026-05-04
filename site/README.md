## Website

[![SecurityHeaders.io](https://securityheadersiobadges.azurewebsites.net/create/badge?domain=https://cabinpkg.com)](https://securityheaders.io/?q=https://cabinpkg.com&hide=on&followRedirects=on)

A package registry website for Cabin, a package manager and build system for C++.

### Architecture

This site is a fully static Astro build. Package data is fetched from
`https://cabin.hasura.app/v1/graphql` at build time, package detail pages are
pre-rendered, and `/packages.json` is generated for client-side search.

The output in `dist/` can be served by Cloudflare Pages, Cloudflare Workers
Static Assets, or any static file host. No Next.js, Vercel runtime, SSR adapter,
API routes, or server functions are required.

### Development

Install Node.js dependencies:

```bash
yarn install
```

Start the local Astro dev server:

```bash
yarn dev
```

`yarn dev` regenerates GraphQL types from Hasura before starting Astro, so a
fresh checkout works without a separate `yarn generate` step. Astro serves the
site at [`localhost:4321`](http://localhost:4321) by default.

### Build and preview

```bash
yarn lint
yarn typecheck
yarn build
yarn preview
```

`yarn build` regenerates GraphQL types, runs Astro type checking, fetches package
data from Hasura, verifies that generated HTML has no inline scripts, and writes
the static site to `dist/`.

Biome is used for TypeScript, JavaScript, CSS, and config files. Astro component
files are excluded from Biome because this setup relies on Astro's own parser and
type checker for `.astro`; run `yarn typecheck` directly, or rely on `yarn build`,
which runs `astro check` before building.

### Cloudflare deployment

`wrangler.jsonc` is configured for Workers Static Assets with `./dist` as the
asset directory. Build before deploying:

```bash
yarn build
yarn wrangler deploy
```

No deploy workflow is included because Cloudflare account and project secrets
vary by environment.

### Static search

`/search` is a static page. In the browser it reads `q`, `page`, and `perPage`
from the URL, fetches `/packages.json`, filters package names with
case-insensitive substring matching, and renders pagination links by updating
the query string. The browser does not call Hasura.

### Package detail routes

Each package gets two statically generated detail routes:

- `/packages/<group>/<name>` renders the latest version.
- `/packages/<group>/<name>/<version>` renders that exact version.

Both are pre-rendered at build time from the same Hasura package data and share
their markup through `src/components/package/PackageDetailView.astro`.

### Known limitation

Package detail routes use `/packages/<group>/<name>`, matching the previous site
and Cabin's current two-segment package naming. Packages with names that do not
fit exactly one slash are included in `/packages.json` but do not get a generated
detail page; the search UI renders them as non-clickable cards.
