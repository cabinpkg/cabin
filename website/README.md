## Website

[![SecurityHeaders.io](https://securityheadersiobadges.azurewebsites.net/create/badge?domain=https://cabinpkg.com)](https://securityheaders.io/?q=https://cabinpkg.com&hide=on&followRedirects=on)

A package registry website for Cabin, a package manager and build system for C/C++.

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

### Account pages

`/dashboard`, `/dashboard/source`, `/dashboard/package`,
`/settings/tokens`, `/settings/profile`, and `/login/denied`
are the registry account pages, styled after crates.io. They are static
pages like everything else: their scripts talk to the registry's session
user API, which the registry Worker mounts on this site's production
origin under `/api/*` (plus `/login` and `/callback` for the GitHub
OAuth flow) - see `registry/docs/architecture.md` in the repository. The
website itself holds no sessions and no secrets.

- The pages ship labeled as private alpha, and sign-in is restricted to
  an allowlist of maintainer accounts while the registry is in private
  alpha; every sign-in affordance says so.
- Without the registry routes mounted (or with the registry down), the
  account pages render their signed-out/error states and the rest of the
  site is unaffected; `yarn verify:progressive` enforces at build time
  that no marketing page's HTML depends on `/api/`.
- During `yarn dev`, an Astro dev-server proxy forwards `/api/*`,
  `/login`, and `/callback` to the production origin, so the pages'
  relative fetches resolve and cookies set by proxied responses land on
  localhost. The proxy is dev-only and absent from the production build.
  Signed-in flows still cannot be exercised locally: the OAuth callback
  URL and the host-only session cookie are pinned to the production
  origin, so GitHub always returns the browser to production and no
  session can be minted for localhost. Local development sees the
  signed-out and error states against real API responses; end-to-end
  sign-in happens on production.
- The session API client lives in `src/lib/account.ts`; `yarn test` runs
  its `node:test` suite directly on Node (>= 22.18).
- `/dashboard/source` is the source viewer: it range-reads a published
  version's zip archive through the registry's session source route and
  parses the container client-side (`src/lib/sourceArchive.ts`, tested
  the same way), rendering file contents as escaped text only.
- Signing out posts to the registry's `/api/v1/user/logout` (the session
  cookie is HttpOnly, so only the server's `Set-Cookie` can clear it),
  reached from the signed-in header menu.

### Static search

`/search` is a static page. In the browser it reads `q`, `page`, and `perPage`
from the URL, fetches `/packages.json`, searches the package index with Fuse.js,
and renders pagination links by updating the query string. The browser does not
call Hasura.

### Package detail routes

Each package gets two statically generated detail routes:

- `/packages/<group>/<name>` renders the latest version.
- `/packages/<group>/<name>/<version>` renders that exact version.

Both are pre-rendered at build time from the same Hasura package data and share
their markup through `src/components/package/PackageDetailView.astro`.

README Markdown is rendered with inline HTML disabled. README images must use
absolute `http:` or `https:` URLs to display; relative image URLs are rendered
without `src` so the browser does not request missing Cabin-local assets.

### Documentation

The documentation at `cabinpkg.com/docs/` is generated from the repository-root
`docs/*.md` Markdown — the canonical source, kept at the repo root rather than
copied into this project. A `docs` content collection (`src/content.config.ts`)
loads them, `src/pages/docs/[...slug].astro` renders each page, and
`src/lib/docsNav.ts` holds the sidebar nav. `yarn build` runs
`yarn verify:docs-links`, which fails the build on a broken internal docs link.

### Known limitation

Package detail routes use `/packages/<group>/<name>`, matching the previous site
and Cabin's current two-segment package naming. Packages with names that do not
fit exactly one slash are included in `/packages.json` but do not get a generated
detail page; the search UI renders them as non-clickable cards.
