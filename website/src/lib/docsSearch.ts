import Fuse, { type IFuseOptions } from "fuse.js";

// One document-level result per docs page. `content` holds the page's full
// plain text (markdown stripped) for full-text search. `excerpt` is a short
// preview rendered under the title.
export interface DocsSearchItem {
    title: string;
    href: string;
    excerpt: string;
    content: string;
}

export interface DocsSearch {
    search(query: string): DocsSearchItem[];
}

const SEARCH_OPTIONS = {
    ignoreLocation: true,
    threshold: 0.4,
    keys: [
        { name: "title", weight: 0.6 },
        { name: "content", weight: 0.35 },
        { name: "excerpt", weight: 0.05 },
    ],
} satisfies IFuseOptions<DocsSearchItem>;

export function createDocsSearch(items: DocsSearchItem[]): DocsSearch {
    const fuse = new Fuse(items, SEARCH_OPTIONS);

    function search(query: string): DocsSearchItem[] {
        const normalizedQuery = query.trim();
        if (!normalizedQuery) {
            return items;
        }

        return fuse.search(normalizedQuery).map((result) => result.item);
    }

    return { search };
}

export function getDocsSearchTitle(query: string): string {
    return query ? `Search results for "${query}"` : "Documentation";
}

export function getDocsSearchCountLabel(
    shown: number,
    total: number,
    query: string,
): string {
    if (total === 0) {
        return "No documentation is published yet.";
    }
    if (query === "") {
        return `${total} documentation ${pages(total)}`;
    }
    if (shown === 0) {
        return `No documentation matches "${query}".`;
    }
    return `Showing ${shown} of ${total} ${pages(total)} for "${query}"`;
}

export function getDocsEmptySearchMessage(query: string): string {
    return query
        ? `Try a shorter or different search term.`
        : "No documentation is published yet.";
}

function pages(count: number): string {
    return count === 1 ? "page" : "pages";
}
