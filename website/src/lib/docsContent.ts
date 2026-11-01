import { DOCS_PAGES, docPath, getDocMeta, slugForId } from "./docsNav";
import type { DocsSearchItem } from "./docsSearch";

// Build-time only: turns docs content-collection entries into the search index.
// Lives outside the endpoint/page so both the static `/docs-search.json` route
// and the server-rendered /docs/search browse list share one source of truth.

// Minimal entry shape we depend on, so this module stays decoupled from the
// `astro:content` types. `body` is the raw Markdown of the page.
export interface DocsEntry {
    id: string;
    body?: string;
}

const EXCERPT_MAX_LENGTH = 200;

export function buildDocsSearchItems(entries: DocsEntry[]): DocsSearchItem[] {
    return [...entries]
        .sort((a, b) => readingOrder(a.id) - readingOrder(b.id))
        .map(toDocsSearchItem);
}

function toDocsSearchItem(entry: DocsEntry): DocsSearchItem {
    const { slug, title } = getDocMeta(entry.id);
    const content = markdownToPlainText(entry.body ?? "");
    return {
        title,
        href: docPath(slug),
        excerpt: buildExcerpt(content),
        content,
    };
}

// Position of a page in the curated reading order, so the index (and the
// server-rendered browse list built from it) stays deterministic.
function readingOrder(id: string): number {
    const slug = slugForId(id);
    const index = DOCS_PAGES.findIndex((page) => page.slug === slug);
    return index < 0 ? Number.MAX_SAFE_INTEGER : index;
}

// CommonMark matches reference-link labels case-insensitively after collapsing
// internal whitespace.  We also drop inline-code backticks so a label written as
// `[`shlex`]` matches its `[`shlex`]: ...` definition once code spans are flattened.
function normalizeRefLabel(label: string): string {
    return label.replace(/`/g, "").replace(/\s+/g, " ").trim().toLowerCase();
}

// Strip Markdown to readable plain text for full-text indexing and previews.
// Intentionally lossy: structure is discarded, prose is kept.
export function markdownToPlainText(markdown: string): string {
    // Reference-link labels (`[shlex]: https://...`) so only genuine reference
    // usages get unwrapped below; literal bracketed syntax such as
    // `[dependencies]` or `[target.'cfg(...)'.dependencies]` keeps its brackets.
    const referenceLabels = new Set<string>();
    for (const match of markdown.matchAll(/^\s{0,3}\[([^\]]+)\]:\s*\S+/gm)) {
        referenceLabels.add(normalizeRefLabel(match[1]));
    }

    return (
        markdown
            // Drop only the fence delimiter lines (```lang / ~~~), keeping the
            // code contents: example file paths, manifest snippets, and command
            // lines are prime full-text search targets in C/C++ build-tool docs.
            .replace(/^[ \t]*(?:```|~~~)[^\n]*$/gm, " ")
            .replace(/`([^`]+)`/g, "$1") // inline code -> keep the identifier text
            .replace(/<!--[\s\S]*?-->/g, " ") // HTML comments
            .replace(/^\s{0,3}\[[^\]]+\]:\s*\S+(?:\s+"[^"]*")?\s*$/gm, " ") // reference link definitions
            .replace(/!\[[^\]]*\]\([^)]*\)/g, " ") // images
            .replace(/\[([^\]]+)\]\([^)]*\)/g, "$1") // inline links -> link text
            // Unwrap only genuine reference-link usages; literal bracketed syntax
            // (TOML tables, cfg expressions) keeps its brackets for exact search.
            .replace(/\[([^\]]+)\](?!\()/g, (whole, label) =>
                referenceLabels.has(normalizeRefLabel(label)) ? label : whole,
            )
            .replace(/^\s{0,3}#{1,6}\s+/gm, "") // ATX heading markers
            .replace(/^\s{0,3}>\s?/gm, "") // blockquote markers
            .replace(/^\s*[-*+]\s+/gm, "") // unordered list bullets
            .replace(/^\s*\d+\.\s+/gm, "") // ordered list markers
            .replace(/^[\s:|-]+$/gm, " ") // table separator rows
            .replace(/\|/g, " ") // remaining table pipes
            // Collapse whitespace *before* unwrapping emphasis so soft-wrapped
            // **bold** / *italic* spans match (these docs hard-wrap prose).
            .replace(/\s+/g, " ")
            // Unwrap emphasis / strikethrough by matching delimiter *pairs* that
            // wrap text, so lone literal `*` / `~` in flattened code survive for
            // exact search (e.g. `packages/*`, `SCCACHE_*`, `~1.2.3`,
            // `~/.config/...`). `_` is never stripped; it is an identifier char,
            // and CommonMark treats intraword `_` as literal anyway.
            .replace(/\*\*([^*\s](?:[^*]*[^*\s])?)\*\*/g, "$1") // bold
            .replace(/\*([^*\s](?:[^*]*[^*\s])?)\*/g, "$1") // italic
            .replace(/~~([^~\s](?:[^~]*[^~\s])?)~~/g, "$1") // strikethrough
            .trim()
    );
}

export function buildExcerpt(
    text: string,
    maxLength = EXCERPT_MAX_LENGTH,
): string {
    if (text.length <= maxLength) {
        return text;
    }
    const slice = text.slice(0, maxLength);
    const lastSpace = slice.lastIndexOf(" ");
    const trimmed = slice.slice(0, lastSpace > 0 ? lastSpace : maxLength);
    return `${trimmed.trimEnd()} ...`;
}
