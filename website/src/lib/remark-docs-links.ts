/**
 * Rewrites the canonical Markdown cross-links so they resolve against the
 * Astro docs route.
 *
 * The docs are authored as a flat set of sibling files under `docs/` and link
 * to each other with bare, relative `*.md` targets, optionally carrying a
 * `#fragment`.  Those pages render at `/docs/<name>/`, so `manifest.md#targets`
 * becomes `/docs/manifest/#targets` and the docs home (`index.md`) becomes
 * `/docs/`.
 *
 * Absolute URLs (any scheme), protocol-relative and root-relative paths, and
 * in-page fragments are left untouched.
 */

interface MarkdownNode {
    type: string;
    url?: string;
    children?: MarkdownNode[];
}

// Matches a leading URL scheme (`https:`, `mailto:`, ...), a protocol-relative
// `//`, a root-relative `/`, or an in-page `#`.  Those targets must not be
// rewritten.
const EXTERNAL_OR_ABSOLUTE = /^(?:[a-z][a-z0-9+.-]*:|\/\/|\/|#)/i;
// Only bare sibling-file targets (`manifest.md`, `manifest.md#x`) are
// rewritten.  A target with a directory component (`../manifest.md`,
// `guides/x.md`) has no `/docs/<name>/` route; leaving it untouched
// lets `verify:docs-links` report the author's original target instead
// of a mangled rewrite like `/docs/../manifest/`.
const MARKDOWN_TARGET = /^([^/]+?)\.md(#.*)?$/;

export function remarkDocsLinks() {
    return (tree: MarkdownNode): void => {
        rewriteLinks(tree);
    };
}

function rewriteLinks(node: MarkdownNode): void {
    if (node.type === "link" && node.url !== undefined) {
        node.url = rewriteUrl(node.url);
    }
    if (node.children) {
        for (const child of node.children) {
            rewriteLinks(child);
        }
    }
}

function rewriteUrl(url: string): string {
    if (EXTERNAL_OR_ABSOLUTE.test(url)) {
        return url;
    }
    const match = MARKDOWN_TARGET.exec(url);
    if (!match) {
        return url;
    }
    const [, name, fragment = ""] = match;
    const slug = name === "index" ? "" : `${name}/`;
    return `/docs/${slug}${fragment}`;
}
