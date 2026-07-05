/**
 * Documentation navigation, ported from the repository-root `mkdocs.yml` nav.
 *
 * A page's `slug` is its `docs/<slug>.md` basename, except the docs home
 * (`docs/index.md`) whose slug is the empty string and renders at `/docs/`.
 * `DOCS_NAV` drives the sidebar; `DOCS_PAGES` is the flat, ordered reading
 * sequence used for page titles and previous/next links.
 */

export interface DocsNavLink {
    label: string;
    slug: string;
}

export interface DocsNavSection {
    title: string;
    items: DocsNavLink[];
}

export const DOCS_HOME: DocsNavLink = { label: "Home", slug: "" };

export const DOCS_NAV: DocsNavSection[] = [
    {
        title: "Getting started",
        items: [{ label: "Installation", slug: "installation" }],
    },
    {
        title: "Overview",
        items: [
            {
                label: "Cargo-inspired interface",
                slug: "cargo-inspired-interface",
            },
            { label: "Creating a new package", slug: "new-and-init" },
            { label: "Architecture", slug: "architecture" },
        ],
    },
    {
        title: "Manifest and configuration",
        items: [
            { label: "cabin.toml reference", slug: "manifest" },
            { label: "Configuration files", slug: "config" },
            { label: "Environment variables", slug: "environment-variables" },
            { label: "Build profiles", slug: "profiles" },
            { label: "Language standards", slug: "language-standards" },
            { label: "Toolchains", slug: "toolchains" },
        ],
    },
    {
        title: "Targets and building",
        items: [
            { label: "Targets", slug: "targets" },
            { label: "Checking", slug: "check" },
            { label: "Compiler wrappers", slug: "compiler-cache" },
            { label: "Testing", slug: "testing" },
            { label: "Formatting", slug: "fmt" },
            { label: "Static analysis", slug: "tidy" },
        ],
    },
    {
        title: "Dependencies",
        items: [
            { label: "Dependency kinds", slug: "dependency-kinds" },
            { label: "System dependencies", slug: "system-dependencies" },
            {
                label: "Target / platform-specific dependencies",
                slug: "target-dependencies",
            },
            { label: "Features", slug: "features" },
            { label: "cabin.lock reference", slug: "lockfile" },
            {
                label: "Patch, override, and source replacement",
                slug: "patch-overrides",
            },
            { label: "Vendoring and offline mode", slug: "vendoring-offline" },
            { label: "Source artifacts", slug: "artifacts" },
            { label: "Foundation ports", slug: "foundation-ports" },
        ],
    },
    {
        title: "Workspaces and observability",
        items: [
            { label: "Workspaces", slug: "workspaces" },
            {
                label: "Metadata, tree, and explain",
                slug: "metadata-tree-explain",
            },
        ],
    },
    {
        title: "Distribution and registry interface",
        items: [
            {
                label: "Package archive and canonical metadata",
                slug: "package-format",
            },
            { label: "Local JSON package index", slug: "package-index" },
            { label: "Registry design", slug: "registry-design" },
            { label: "CLI distribution artifacts", slug: "distribution" },
        ],
    },
    {
        title: "Design",
        items: [
            {
                label: "Standard compatibility specification",
                slug: "design/standard-compatibility/spec",
            },
            {
                label: "Standard compatibility registry index",
                slug: "design/standard-compatibility/registry-index",
            },
            {
                label: "Standard compatibility publish lints",
                slug: "design/standard-compatibility/publish-lints",
            },
            {
                label: "Standard compatibility preference mode",
                slug: "design/standard-compatibility/preference-mode",
            },
        ],
    },
];

export const DOCS_PAGES: DocsNavLink[] = [
    DOCS_HOME,
    ...DOCS_NAV.flatMap((section) => section.items),
];

const DOCS_EDIT_BASE = "https://github.com/cabinpkg/cabin/edit/main/docs";

/** Collection entry id (`index` for the home page) -> docs slug. */
export function slugForId(id: string): string {
    return id === "index" ? "" : id;
}

/** Docs slug -> the page's URL path (`/docs/` or `/docs/<slug>/`). */
export function docPath(slug: string): string {
    return slug === "" ? "/docs/" : `/docs/${slug}/`;
}

/** "Edit this page on GitHub" target, mirroring the old `edit_uri`. */
export function docEditUrl(id: string): string {
    return `${DOCS_EDIT_BASE}/${id}.md`;
}

export interface DocMeta {
    slug: string;
    title: string;
}

export function getDocMeta(id: string): DocMeta {
    const slug = slugForId(id);
    if (slug === "") {
        return { slug, title: "Documentation" };
    }
    const page = DOCS_PAGES.find((entry) => entry.slug === slug);
    return { slug, title: page?.label ?? slug };
}

export function getAdjacentDocs(slug: string): {
    prev?: DocsNavLink;
    next?: DocsNavLink;
} {
    const index = DOCS_PAGES.findIndex((entry) => entry.slug === slug);
    if (index < 0) {
        return {};
    }
    return {
        prev: index > 0 ? DOCS_PAGES[index - 1] : undefined,
        next: index < DOCS_PAGES.length - 1 ? DOCS_PAGES[index + 1] : undefined,
    };
}

/**
 * Build-time guard: the sidebar and the rendered docs collection must describe
 * exactly the same set of pages.  Drift (a new `docs/*.md` without a nav entry,
 * or a nav entry without a page) fails the build instead of shipping a broken
 * sidebar or an unreachable route.
 */
export function assertDocsNavMatches(entryIds: string[]): void {
    const navIds = new Set(
        DOCS_PAGES.map((entry) => (entry.slug === "" ? "index" : entry.slug)),
    );
    const docIds = new Set(entryIds);

    const missingFromNav = [...docIds].filter((id) => !navIds.has(id)).sort();
    const missingFromDocs = [...navIds].filter((id) => !docIds.has(id)).sort();

    if (missingFromNav.length === 0 && missingFromDocs.length === 0) {
        return;
    }

    const lines = ["Docs navigation and docs/*.md are out of sync."];
    if (missingFromNav.length > 0) {
        lines.push(
            `  Pages missing from DOCS_NAV (src/lib/docsNav.ts): ${missingFromNav.join(", ")}`,
        );
    }
    if (missingFromDocs.length > 0) {
        lines.push(
            `  Nav entries with no docs/*.md page: ${missingFromDocs.join(", ")}`,
        );
    }
    throw new Error(lines.join("\n"));
}
