import { SITE_NAME } from "../lib/constants";
import { formatEdition, formatRelativeTime } from "../lib/format";
import { getPaginationItems, type PaginationItem } from "../lib/pagination";
import {
    filterPackages,
    getEmptySearchMessage,
    getSearchCountLabel,
    getSearchTitle,
    paginateItems,
    parseSearchParams,
} from "../lib/search";
import type { PackageListItem } from "../lib/types";

const params = parseSearchParams(new URLSearchParams(window.location.search));

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

updateTitle(params.query);

fetch("/packages.json", {
    headers: { accept: "application/json" },
})
    .then((response) => {
        if (!response.ok) {
            throw new Error(`HTTP ${response.status}`);
        }
        return response.json() as Promise<PackageListItem[]>;
    })
    .then((packages) => renderSearch(packages))
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

function renderSearch(packages: PackageListItem[]) {
    const filtered = filterPackages(packages, params.query);
    const paged = paginateItems(filtered, params.page, params.perPage);

    if (count) {
        count.textContent = getSearchCountLabel(paged, params.query);
    }

    results?.replaceChildren(...paged.items.map(createPackageCard));

    if (emptyState && emptyMessage) {
        emptyState.classList.toggle("hidden", paged.total > 0);
        emptyMessage.textContent = getEmptySearchMessage(params.query);
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
    setField(card, "edition", formatEdition(pack.edition));
    setField(card, "published", formatRelativeTime(pack.published_at));

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
        query: params.query,
        currentPage,
        perPage: params.perPage,
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
