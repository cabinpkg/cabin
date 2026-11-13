## Website

[![SecurityHeaders.io](https://securityheadersiobadges.azurewebsites.net/create/badge?domain=https://cabinpkg.com)](https://securityheaders.io/?q=https://cabinpkg.com&hide=on&followRedirects=on)

A package registry website for Cabin, a package manager and build system for C/C++.

### Architecture

This site is a fully static Astro build. Package data comes from the hosted
registry's catalog, `https://registry.cabinpkg.com/catalog.json`
([`../docs/remote-registry.md`](../docs/remote-registry.md#the-catalog)):
`npm run build` and `npm run dev` first fetch it anonymously into `.catalog/`
(`npm run fetch:catalog`). A `200` renders every package it lists; the
registry's private-alpha `401` renders the unavailable state, with no package
pages and an empty `/packages.json`; any other response fails the build. After
building, `npm run build` fetches the catalog again and fails if it changed in
the meantime, so a stale build never deploys. Package detail pages are
pre-rendered, `/packages.json` is generated for client-side search, and
`/catalog-provenance.json` records which catalog the build rendered.

`npm run build:fixtures`, the CI build, needs no network: it builds the site
once per catalog state from `fixtures/catalog.json` into `dist-fixtures/` and
verifies each. It never writes `dist/`.

The output in `dist/` can be served by Cloudflare Pages, Cloudflare Workers
Static Assets, or any static file host. No SSR adapter, API routes, or server
functions are required.

### Development

Install Node.js dependencies:

```bash
npm install
```

Start the local Astro dev server:

```bash
npm run dev
```

`npm run dev` starts Astro, which serves the site at
[`localhost:4321`](http://localhost:4321) by default.

### Build and preview

```bash
npm run lint
npm run typecheck
npm run build
npm run preview
```

`npm run build` runs Astro type checking, verifies that generated HTML has no
inline scripts, and writes the static site to `dist/`. It needs network access
to the registry.

Biome is used for TypeScript, JavaScript, CSS, and config files. Astro component
files are excluded from Biome because this setup relies on Astro's own parser and
type checker for `.astro`; run `npm run typecheck` directly, or rely on `npm run build`,
which runs `astro check` before building.

### Cloudflare deployment

`wrangler.jsonc` is configured for Workers Static Assets with `./dist` as the
asset directory. Build before deploying:

```bash
npm run build
npx wrangler deploy
```

No deploy workflow is included because Cloudflare account and project secrets
vary by environment.

### Demo GIF

`public/demo.gif`, served at `https://cabinpkg.com/demo.gif` for the home page
and the repository README, records `../demo.tape`: a released Cabin building,
running, and printing the dependency tree of `examples/workspace-basic`, with
no registry access. It is a reviewed, committed asset; building the site needs
none of the recording tools. To regenerate it:

1. In a pull request, set `CABIN_RELEASE` in the `demo` job of
   `.github/workflows/website.yml` to the release to record. The job downloads
   that release's `x86_64-unknown-linux-gnu` archive and verifies its build
   provenance with `gh attestation verify`.
2. The job runs `cargo demo-check` without network access, then
   `cargo demo-record`, which fails unless every command in the recorded
   session exits 0 and shows its expected output, and uploads the GIF as the
   `demo.gif` artifact. It also runs when the tape, `demo.zsh`, the example, or
   `crates/xtask-demo` changes.
3. Download the artifact over `public/demo.gif`, review every frame, and
   commit it to the pull request. Merging deploys it with the rest of the site.

### Account pages

`/dashboard`, `/dashboard/source`, `/dashboard/package`,
`/settings/profile`, and `/login/denied`
are the registry account pages, styled after crates.io. They are static
pages like everything else: their scripts talk to the registry's session
user API, which the registry Worker mounts on this site's production
origin under `/api/*` (plus `/login` and `/callback` for the GitHub
OAuth flow) - see `api/README.md` in the repository. The
website itself holds no sessions and no secrets.

- The pages ship labeled as private alpha, and sign-in is restricted to
  an allowlist of maintainer accounts while the registry is in private
  alpha; every sign-in affordance says so.
- Without the registry routes mounted (or with the registry down), the
  account pages render their signed-out/error states and the rest of the
  site is unaffected; `npm run verify:progressive` enforces at build time
  that no marketing page's HTML depends on `/api/`.
- During `npm run dev`, an Astro dev-server proxy forwards `/api/*`,
  `/login`, and `/callback` to the production origin, so the pages'
  relative fetches resolve and cookies set by proxied responses land on
  localhost. The proxy is dev-only and absent from the production build.
  Signed-in flows still cannot be exercised locally: the OAuth callback
  URL and the host-only session cookie are pinned to the production
  origin, so GitHub always returns the browser to production and no
  session can be minted for localhost. Local development sees the
  signed-out and error states against real API responses; end-to-end
  sign-in happens on production.
- The session API client lives in `src/lib/account.ts`; `npm test` runs
  its `node:test` suite directly on Node (the version in `.node-version`).
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
and renders pagination links by updating the query string. The browser calls
no remote package API.

### Package detail routes

Each package gets two statically generated detail routes:

- `/packages/<group>/<name>` renders the latest version.
- `/packages/<group>/<name>/<version>` renders that exact version.

Both are pre-rendered at build time from the same catalog and share their
markup through `src/components/package/PackageDetailView.astro`.

### Documentation

The documentation at `cabinpkg.com/docs/` is generated from the repository-root
`docs/*.md` Markdown — the canonical source, kept at the repo root rather than
copied into this project. A `docs` content collection (`src/content.config.ts`)
loads them, `src/pages/docs/[...slug].astro` renders each page, and
`src/lib/docsNav.ts` holds the sidebar nav. `npm run build` runs
`npm run verify:docs-links`, which fails the build on a broken internal docs link.

### Known limitation

Package detail routes use `/packages/<group>/<name>`, matching Cabin's
two-segment package naming. Packages with names that do not
fit exactly one slash are included in `/packages.json` but do not get a generated
detail page; the search UI renders them as non-clickable cards.
