import {
    DOCS_HIGHLIGHT_PARAM,
    DOCS_SEARCH_PATH,
    SITE_NAME,
} from "../lib/constants";
import { debounce } from "../lib/debounce";
import { fetchDocsIndex } from "../lib/docsIndex";
import {
    createDocsSearch,
    type DocsSearch,
    getDocsEmptySearchMessage,
    getDocsSearchCountLabel,
    getDocsSearchTitle,
} from "../lib/docsSearch";

const INPUT_DEBOUNCE_MS = 200;

const input = document.getElementById("site-search");
const form = input?.closest("form") ?? null;
const title = document.getElementById("docs-search-title");
const count = document.getElementById("docs-result-count");
const results = document.getElementById("docs-results");
const emptyState = document.getElementById("docs-empty-state");
const emptyMessage = document.getElementById("docs-empty-message");

// The page server-renders one <li> per docs page. We keep a reference to each,
// keyed by href, then reorder/filter them from search results rather than
// rebuilding markup in JS.
const nodesByHref = new Map<string, HTMLLIElement>();
if (results) {
    for (const node of Array.from(results.children)) {
        if (node instanceof HTMLLIElement && node.dataset.docHref) {
            nodesByHref.set(node.dataset.docHref, node);
        }
    }
}
const total = nodesByHref.size;

let query = new URLSearchParams(window.location.search).get("q")?.trim() ?? "";
let docsSearch: DocsSearch | null = null;

if (input instanceof HTMLInputElement) {
    input.value = query;
    input.addEventListener("input", debounce(handleInput, INPUT_DEBOUNCE_MS));
}

// On non-results pages the header search is a plain GET form. Here the page
// owns the input, so suppress the navigation and search in place.
if (form instanceof HTMLFormElement) {
    form.addEventListener("submit", (event) => {
        event.preventDefault();
    });
}

updateTitle(query);

fetchDocsIndex()
    .then((items) => {
        docsSearch = createDocsSearch(items);
        render();
    })
    .catch((error: Error) => {
        // Mirror the package /search failure path: the live search is dead, so
        // clear the now-misleading browse list and announce the error through
        // the aria-live empty-state region rather than the silent count line.
        results?.replaceChildren();
        if (count) {
            count.textContent = "Unable to load the documentation index.";
        }
        if (emptyState && emptyMessage) {
            emptyState.classList.remove("hidden");
            emptyMessage.textContent = `The documentation index could not be loaded: ${error.message}`;
        }
    });

function handleInput() {
    if (!(input instanceof HTMLInputElement)) {
        return;
    }
    applyQuery(input.value.trim());
}

function applyQuery(next: string) {
    query = next;
    history.replaceState(null, "", searchUrl(query));
    if (input instanceof HTMLInputElement && input.value !== query) {
        input.value = query;
    }
    updateTitle(query);
    render();
}

function render() {
    if (!docsSearch || !results) {
        return;
    }

    const matched = docsSearch.search(query);
    const orderedNodes = matched
        .map((item) => nodesByHref.get(item.href))
        .filter((node): node is HTMLLIElement => node !== undefined);

    // Carry the query to each result so the docs page scrolls to and highlights
    // the match on arrival (consumed by src/scripts/docs.ts).
    for (const node of orderedNodes) {
        const base = node.dataset.docHref;
        const anchor = node.querySelector("a");
        if (base && anchor) {
            anchor.href = query
                ? `${base}?${DOCS_HIGHLIGHT_PARAM}=${encodeURIComponent(query)}`
                : base;
        }
    }

    results.replaceChildren(...orderedNodes);

    if (count) {
        count.textContent = getDocsSearchCountLabel(
            orderedNodes.length,
            total,
            query,
        );
    }

    if (emptyState && emptyMessage) {
        emptyState.classList.toggle("hidden", orderedNodes.length > 0);
        emptyMessage.textContent = getDocsEmptySearchMessage(query);
    }
}

function updateTitle(value: string) {
    const nextTitle = getDocsSearchTitle(value);
    if (title) {
        title.textContent = nextTitle;
    }
    document.title = `${nextTitle} | ${SITE_NAME}`;
}

function searchUrl(value: string): string {
    if (value === "") {
        return DOCS_SEARCH_PATH;
    }
    const params = new URLSearchParams({ q: value });
    return `${DOCS_SEARCH_PATH}?${params.toString()}`;
}
