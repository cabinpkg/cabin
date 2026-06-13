import type { DocsSearchItem } from "./docsSearch";

// Fetch the statically generated docs search index (`/docs-search.json`).
// Shared by the /docs/search page script and the header typeahead (in docs
// mode); each layers its own caching or error handling on top.
export async function fetchDocsIndex(): Promise<DocsSearchItem[]> {
    const response = await fetch("/docs-search.json", {
        headers: { accept: "application/json" },
    });
    if (!response.ok) {
        throw new Error(`HTTP ${response.status}`);
    }
    return (await response.json()) as DocsSearchItem[];
}
