import Combobox from "@github/combobox-nav";
import { SEARCH_PATH } from "../lib/constants";
import { getPackageSuggestions } from "../lib/search";
import type { PackageListItem } from "../lib/types";

const SUGGESTION_LIMIT = 8;
const INPUT_DEBOUNCE_MS = 150;
const BLUR_HIDE_DELAY_MS = 150;

if (!isSearchPage()) {
    initHeaderTypeahead();
}

function isSearchPage(): boolean {
    const path = window.location.pathname.replace(/\/+$/, "");
    return path === SEARCH_PATH;
}

function initHeaderTypeahead() {
    const inputEl = document.getElementById("site-search");
    const listEl = document.getElementById("site-search-suggestions");

    if (
        !(inputEl instanceof HTMLInputElement) ||
        !(listEl instanceof HTMLUListElement)
    ) {
        return;
    }

    const input: HTMLInputElement = inputEl;
    const list: HTMLUListElement = listEl;

    input.setAttribute("role", "combobox");
    input.setAttribute("aria-autocomplete", "list");
    input.setAttribute("aria-expanded", "false");
    input.setAttribute("aria-controls", list.id);
    input.setAttribute("aria-haspopup", "listbox");

    const combobox = new Combobox(input, list, {
        tabInsertsSuggestions: false,
        firstOptionSelectionMode: "none",
    });
    let cache: Promise<PackageListItem[]> | null = null;
    let pendingQuery = "";
    let started = false;

    function fetchPackages(): Promise<PackageListItem[]> {
        if (cache === null) {
            cache = fetch("/packages.json", {
                headers: { accept: "application/json" },
            })
                .then((response) => {
                    if (!response.ok) {
                        throw new Error(`HTTP ${response.status}`);
                    }
                    return response.json() as Promise<PackageListItem[]>;
                })
                .catch((error: Error) => {
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
        pack: PackageListItem & { href: string },
        index: number,
    ): HTMLLIElement {
        const item = document.createElement("li");
        item.id = `site-search-suggestion-${index}`;
        item.setAttribute("role", "option");
        item.dataset.href = pack.href;
        item.className =
            "aria-selected:bg-sky-500/20 aria-selected:text-white hover:bg-sky-500/10";

        const link = document.createElement("a");
        link.href = pack.href;
        link.tabIndex = -1;
        link.className = "block px-4 py-2 text-sm text-slate-200 transition";

        const name = document.createElement("span");
        name.className = "block truncate font-semibold";
        name.textContent = pack.name;
        link.appendChild(name);

        const description = pack.description.trim();
        const versionText = pack.version ? `v${pack.version}` : "";
        const meta = [versionText, description].filter(Boolean).join(" — ");
        if (meta) {
            const metaLine = document.createElement("span");
            metaLine.className = "block truncate text-xs text-slate-400";
            metaLine.textContent = meta;
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

        let packages: PackageListItem[];
        try {
            packages = await fetchPackages();
        } catch {
            hide();
            return;
        }

        if (pendingQuery !== query) {
            return;
        }

        const suggestions = getPackageSuggestions(
            packages,
            query,
            SUGGESTION_LIMIT,
        );

        if (suggestions.length === 0) {
            list.replaceChildren();
            hide();
            return;
        }

        const linkable = suggestions.filter(
            (pack): pack is PackageListItem & { href: string } =>
                pack.href !== null,
        );

        if (linkable.length === 0) {
            list.replaceChildren();
            hide();
            return;
        }

        list.replaceChildren(
            ...linkable.map((pack, index) => createOption(pack, index)),
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
        if (originalEvent instanceof MouseEvent) {
            // Let the nested <a> handle mouse navigation natively so that
            // modifier-clicks and middle-clicks behave as the user expects.
            return;
        }
        window.location.assign(href);
    });
}

function debounce(fn: () => void, ms: number): () => void {
    let timer: ReturnType<typeof setTimeout> | undefined;
    return () => {
        if (timer !== undefined) {
            clearTimeout(timer);
        }
        timer = setTimeout(fn, ms);
    };
}
