export const SITE_URL = "https://cabinpkg.com";
export const SITE_NAME = "Cabin";
export const SITE_DESCRIPTION =
    "C/C++ package manager and build system, inspired by Cargo";

export const DEFAULT_SEARCH_PAGE = 1;
export const DEFAULT_SEARCH_PER_PAGE = 20;
export const SEARCH_PATH = "/search";
export const POLICIES_PATH = "/policies";

// Documentation full-text search.  The header search box switches to this
// target (and the docs index) when rendered in `searchMode="docs"`.
export const DOCS_SEARCH_PATH = "/docs/search";

// Query-string key carried to a docs page so it scrolls to and highlights the
// searched terms (see `src/scripts/docs.ts`).
export const DOCS_HIGHLIGHT_PARAM = "highlight";

// Docs now render inside this Astro site under `/docs/` (see
// `src/pages/docs/[...slug].astro`); they are no longer an external site.
export const DOCS_URL = "/docs/";
export const INSTALL_DOC_URL = "/docs/installation/";

export const EXTERNAL_URLS = {
    githubOrg: "https://github.com/cabinpkg",
    author: "https://github.com/ken-matsui",
    demoGif:
        "https://github.com/cabinpkg/cabin/releases/latest/download/demo.gif",
} as const;

export const NAV_LINKS = {
    docs: {
        label: "Docs",
        href: DOCS_URL,
    },
    github: {
        label: "GitHub Repository",
        href: EXTERNAL_URLS.githubOrg,
    },
} as const;

// The account pages (see "Account pages" in README.md). `/login` is the
// registry Worker's OAuth route mounted on this origin; the rest are
// static pages of this site.
export const ACCOUNT_URLS = {
    signIn: "/login",
    dashboard: "/dashboard",
    source: "/dashboard/source",
    package: "/dashboard/package",
    profile: "/settings/profile",
} as const;

// Shown on every sign-in affordance while the registry is in private
// alpha - the visitor must see this before being sent to GitHub.
export const SIGNIN_RESTRICTION =
    "The registry is in private alpha; sign-in is restricted to " +
    "allowlisted maintainer accounts.";

// Why the only sign-in option is GitHub: current policy, not
// architecture - the registry's identity schema is provider-neutral
// (registry/docs/architecture.md), but scope ownership today is proved
// against GitHub, so the copy says "currently" and must not promise
// GitHub forever or present it as a structural necessity. The claim
// wording mirrors the usage policy's ("proving control of the GitHub
// user or organization account with the same name").
export const SIGNIN_GITHUB_REASON =
    "Sign-in currently uses GitHub because Cabin package scopes map to " +
    "GitHub: a scope is claimed by proving control of the GitHub user " +
    "or organization account with the same name.";
