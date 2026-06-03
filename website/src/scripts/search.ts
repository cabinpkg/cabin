import { SEARCH_PATH, SITE_NAME } from "../lib/constants";
import { debounce } from "../lib/debounce";
import { formatEdition, formatRelativeTime } from "../lib/format";
import { fetchPackageIndex } from "../lib/packageIndex";
import { createPackageSearch } from "../lib/packageSearch";
import { getPaginationItems, type PaginationItem } from "../lib/pagination";
import {
    createSearchUrl,
    getEmptySearchMessage,
    getSearchCountLabel,
    getSearchTitle,
    paginateItems,
    parseSearchParams,
    type SearchState,
} from "../lib/search";
import type { PackageListItem } from "../lib/types";

const INPUT_DEBOUNCE_MS = 200;

let state: SearchState = parseSearchParams(
    new URLSearchParams(window.location.search),
);
let packageSearch = createPackageSearch([]);

const input = document.getElementById("site-search");
const form = input?.closest("form") ?? null;
const title = document.getElementById("search-title");
const count = document.getElementById("result-count");
const results = document.getElementById("results");
const emptyState = document.getElementById("empty-state");
const emptyMessage = document.getElementById("empty-message");
const pagination = document.getElementById("pagination");
const linkCardTemplate = document.getElementById("package-card-template");
const staticCardTemplate = document.getElementById(
    "package-card-template-static",
);

if (input instanceof HTMLInputElement) {
    input.value = state.query;
    input.addEventListener("input", debounce(handleInput, INPUT_DEBOUNCE_MS));
}

// This script is only loaded on /search. On non-search pages the header
// search stays a plain GET form that submits to /search?q=...
if (form instanceof HTMLFormElement) {
    form.addEventListener("submit", (event) => {
        event.preventDefault();
    });
}

if (pagination) {
    pagination.addEventListener("click", handlePaginationClick);
}

updateTitle(state.query);

fetchPackageIndex()
    .then((packages) => {
        packageSearch = createPackageSearch(packages);
        renderSearch();
    })
    .catch((error: Error) => {
        results?.replaceChildren();
        pagination?.classList.add("hidden");
        if (count) {
            count.textContent = "Unable to load package index.";
        }
        if (emptyState && emptyMessage) {
            emptyState.classList.remove("hidden");
            emptyMessage.textContent = `The static package index could not be loaded: ${error.message}`;
        }
    });

function handleInput() {
    if (!(input instanceof HTMLInputElement)) {
        return;
    }
    applyState({ ...state, query: input.value.trim(), page: 1 });
}

function handlePaginationClick(event: MouseEvent) {
    if (
        event.button !== 0 ||
        event.metaKey ||
        event.ctrlKey ||
        event.shiftKey ||
        event.altKey
    ) {
        return;
    }
    const target = event.target;
    if (!(target instanceof Element)) {
        return;
    }
    const anchor = target.closest("a");
    if (!(anchor instanceof HTMLAnchorElement)) {
        return;
    }
    const url = new URL(anchor.href, window.location.origin);
    if (url.origin !== window.location.origin || url.pathname !== SEARCH_PATH) {
        return;
    }
    event.preventDefault();
    applyState(parseSearchParams(url.searchParams));
}

function applyState(next: SearchState) {
    state = next;
    history.replaceState(null, "", createSearchUrl(state));
    if (input instanceof HTMLInputElement && input.value !== state.query) {
        input.value = state.query;
    }
    updateTitle(state.query);
    renderSearch();
}

function renderSearch() {
    const filtered = packageSearch.search(state.query);
    const paged = paginateItems(filtered, state.page, state.perPage);

    if (count) {
        count.textContent = getSearchCountLabel(paged, state.query);
    }

    results?.replaceChildren(...paged.items.map(createPackageCard));

    if (emptyState && emptyMessage) {
        emptyState.classList.toggle("hidden", paged.total > 0);
        emptyMessage.textContent = getEmptySearchMessage(state.query);
    }

    renderPagination(paged.page, paged.totalPages);
}

function updateTitle(query: string) {
    const nextTitle = getSearchTitle(query);
    if (title) {
        title.textContent = nextTitle;
    }
    document.title = `${nextTitle} | ${SITE_NAME}`;
}

function createPackageCard(pack: PackageListItem): Node {
    const template = pack.href === null ? staticCardTemplate : linkCardTemplate;

    if (!(template instanceof HTMLTemplateElement)) {
        return document.createTextNode("");
    }

    const card = template.content.firstElementChild?.cloneNode(true);

    if (!(card instanceof HTMLElement)) {
        return document.createTextNode("");
    }

    if (pack.href !== null) {
        if (card instanceof HTMLAnchorElement) {
            card.href = pack.href;
        }
        card.setAttribute("aria-label", `View ${pack.name}`);
    }

    setField(card, "name", pack.name);
    setField(card, "version", pack.version);
    setField(card, "published", formatRelativeTime(pack.published_at));

    const edition = card.querySelector('[data-package-field="edition"]');
    if (edition) {
        const editionLabel = formatEdition(pack.edition);
        edition.textContent = editionLabel;
        edition.classList.toggle("hidden", !editionLabel);
    }

    const description = card.querySelector(
        '[data-package-field="description"]',
    );
    if (description) {
        description.textContent = pack.description;
        description.classList.toggle("hidden", !pack.description);
    }

    return card;
}

function setField(parent: Element, field: string, value: string) {
    const element = parent.querySelector(`[data-package-field="${field}"]`);
    if (element) {
        element.textContent = value;
    }
}

function renderPagination(currentPage: number, totalPages: number) {
    if (!pagination) {
        return;
    }

    pagination.replaceChildren();
    pagination.classList.toggle("hidden", totalPages <= 1);

    if (totalPages <= 1) {
        return;
    }

    const paginationItems = getPaginationItems({
        query: state.query,
        currentPage,
        perPage: state.perPage,
        totalPages,
    });

    pagination.append(...paginationItems.map(createPaginationControl));
}

function createPaginationControl(item: PaginationItem) {
    if (item.type === "ellipsis") {
        const ellipsis = document.createElement("span");
        ellipsis.className = "px-2 py-2 text-slate-500";
        ellipsis.textContent = item.label;
        return ellipsis;
    }

    if (item.disabled) {
        const span = document.createElement("span");
        span.className = "page-link page-link-disabled";
        span.textContent = item.label;
        span.setAttribute("aria-disabled", "true");
        return span;
    }

    const link = document.createElement("a");
    link.className = ["page-link", item.current ? "page-link-current" : ""]
        .filter(Boolean)
        .join(" ");
    link.textContent = item.label;
    link.href = item.href;

    if (item.current) {
        link.setAttribute("aria-current", "page");
    }

    return link;
}
