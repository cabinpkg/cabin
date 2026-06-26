import Combobox from "@github/combobox-nav";
import { DOCS_HIGHLIGHT_PARAM } from "../lib/constants";
import { debounce } from "../lib/debounce";
import { fetchDocsIndex } from "../lib/docsIndex";
import { createDocsSearch } from "../lib/docsSearch";
import { fetchPackageIndex } from "../lib/packageIndex";
import { createPackageSearch } from "../lib/packageSearch";

const SUGGESTION_LIMIT = 8;
const INPUT_DEBOUNCE_MS = 150;
const BLUR_HIDE_DELAY_MS = 150;

// A single header suggestion, decoupled from the package- vs docs-specific
// item shapes so the dropdown UI stays one implementation.
interface Suggestion {
    href: string;
    title: string;
    meta: string;
}

type SuggestFn = (query: string, limit: number) => Suggestion[];

initHeaderTypeahead();

function initHeaderTypeahead() {
    const inputEl = document.getElementById("site-search");
    const listEl = document.getElementById("site-search-suggestions");

    if (
        !(inputEl instanceof HTMLInputElement) ||
        !(listEl instanceof HTMLUListElement)
    ) {
        return;
    }

    const form = inputEl.closest("form");
    if (!(form instanceof HTMLFormElement)) {
        return;
    }

    // A results page (/search, /docs/search) drives this input through its own
    // script; the header typeahead defers to it.
    if (form.dataset.typeahead === "off") {
        return;
    }

    const input: HTMLInputElement = inputEl;
    const list: HTMLUListElement = listEl;
    const loadSuggest =
        form.dataset.searchMode === "docs"
            ? loadDocsSuggest
            : loadPackageSuggest;

    const combobox = new Combobox(input, list, {
        tabInsertsSuggestions: false,
        firstOptionSelectionMode: "none",
    });
    let cache: Promise<SuggestFn> | null = null;
    let pendingQuery = "";
    let started = false;

    function suggest(): Promise<SuggestFn> {
        if (cache === null) {
            cache = loadSuggest().catch((error: Error) => {
                cache = null;
                throw error;
            });
        }
        return cache;
    }

    function show() {
        if (list.hidden) {
            list.hidden = false;
            input.setAttribute("aria-expanded", "true");
        }
        if (!started) {
            combobox.start();
            started = true;
        }
    }

    function hide() {
        if (started) {
            combobox.clearSelection();
            combobox.stop();
            started = false;
        }
        if (!list.hidden) {
            list.hidden = true;
            input.setAttribute("aria-expanded", "false");
        }
        input.removeAttribute("aria-activedescendant");
    }

    function createOption(
        suggestion: Suggestion,
        index: number,
    ): HTMLLIElement {
        const item = document.createElement("li");
        item.id = `site-search-suggestion-${index}`;
        item.setAttribute("role", "option");
        item.dataset.href = suggestion.href;
        item.className =
            "aria-selected:bg-sky-500/20 aria-selected:text-white hover:bg-sky-500/10";

        const link = document.createElement("a");
        link.href = suggestion.href;
        link.tabIndex = -1;
        link.className = "block px-4 py-2 text-sm text-slate-200 transition";

        const name = document.createElement("span");
        name.className = "block truncate font-semibold";
        name.textContent = suggestion.title;
        link.appendChild(name);

        if (suggestion.meta) {
            const metaLine = document.createElement("span");
            metaLine.className = "block truncate text-xs text-slate-400";
            metaLine.textContent = suggestion.meta;
            link.appendChild(metaLine);
        }

        item.appendChild(link);
        return item;
    }

    async function update() {
        const query = input.value.trim();
        pendingQuery = query;

        if (query === "") {
            list.replaceChildren();
            hide();
            return;
        }

        let suggestFn: SuggestFn;
        try {
            suggestFn = await suggest();
        } catch {
            hide();
            return;
        }

        if (pendingQuery !== query) {
            return;
        }

        const suggestions = suggestFn(query, SUGGESTION_LIMIT);

        if (suggestions.length === 0) {
            list.replaceChildren();
            hide();
            return;
        }

        list.replaceChildren(
            ...suggestions.map((suggestion, index) =>
                createOption(suggestion, index),
            ),
        );
        show();
    }

    input.addEventListener("input", debounce(update, INPUT_DEBOUNCE_MS));

    input.addEventListener("focus", () => {
        if (input.value.trim() !== "" && list.children.length > 0) {
            show();
        }
    });

    input.addEventListener("blur", () => {
        window.setTimeout(hide, BLUR_HIDE_DELAY_MS);
    });

    input.addEventListener("keydown", (event) => {
        if (event.key === "Escape" && !list.hidden) {
            event.preventDefault();
            hide();
        }
    });

    list.addEventListener("combobox-commit", (event) => {
        const target = event.target;
        if (!(target instanceof HTMLElement)) {
            return;
        }
        const href = target.dataset.href;
        if (!href) {
            return;
        }
        const originalEvent =
            event instanceof CustomEvent
                ? (event.detail as { event?: Event } | undefined)?.event
                : undefined;
        hide();
        if (isAnchorMouseCommit(originalEvent)) {
            // Let the nested <a> handle mouse navigation natively so that
            // modifier-clicks and middle-clicks behave as the user expects.
            return;
        }
        window.location.assign(href);
    });
}

async function loadPackageSuggest(): Promise<SuggestFn> {
    const search = createPackageSearch(await fetchPackageIndex());
    return (query, limit) =>
        search.suggestions(query, limit).map((pack) => ({
            href: pack.href,
            title: pack.name,
            meta: [
                pack.version ? `v${pack.version}` : "",
                pack.description.trim(),
            ]
                .filter(Boolean)
                .join(" - "),
        }));
}

async function loadDocsSuggest(): Promise<SuggestFn> {
    const search = createDocsSearch(await fetchDocsIndex());
    return (query, limit) => {
        // Carry the query so the docs page scrolls to and highlights the match
        // on arrival (consumed by src/scripts/docs.ts).
        const suffix = `?${DOCS_HIGHLIGHT_PARAM}=${encodeURIComponent(query)}`;
        return search
            .search(query)
            .slice(0, limit)
            .map((doc) => ({
                href: `${doc.href}${suffix}`,
                title: doc.title,
                meta: doc.excerpt,
            }));
    };
}

function isAnchorMouseCommit(event: Event | undefined): boolean {
    return (
        event instanceof MouseEvent &&
        event.target instanceof Element &&
        event.target.closest("a") instanceof HTMLAnchorElement
    );
}
