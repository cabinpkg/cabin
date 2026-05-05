import {
    DEFAULT_SEARCH_PAGE,
    DEFAULT_SEARCH_PER_PAGE,
    SEARCH_PATH,
} from "./constants";
import type { PackageListItem } from "./types";

export interface SearchState {
    query: string;
    page: number;
    perPage: number;
}

export interface PaginationResult<T> {
    items: T[];
    total: number;
    page: number;
    perPage: number;
    totalPages: number;
    first: number;
    last: number;
}

export function parseSearchParams(params: URLSearchParams): SearchState {
    return {
        query: (params.get("q") || "").trim(),
        page: parsePositiveInteger(params.get("page"), DEFAULT_SEARCH_PAGE),
        perPage: parsePositiveInteger(
            params.get("perPage"),
            DEFAULT_SEARCH_PER_PAGE,
        ),
    };
}

export function parsePositiveInteger(
    value: string | null,
    fallback: number,
): number {
    const parsed = Number(value);
    return Number.isInteger(parsed) && parsed > 0 ? parsed : fallback;
}

export function createSearchUrl(state: SearchState): string {
    const query = state.query.trim();
    const params = new URLSearchParams();

    if (query !== "") {
        params.set("q", query);
    }
    if (state.page > DEFAULT_SEARCH_PAGE) {
        params.set("page", String(state.page));
    }
    if (state.perPage !== DEFAULT_SEARCH_PER_PAGE) {
        params.set("perPage", String(state.perPage));
    }

    const queryString = params.toString();
    return queryString ? `${SEARCH_PATH}?${queryString}` : SEARCH_PATH;
}

export function filterPackages(
    packages: PackageListItem[],
    query: string,
): PackageListItem[] {
    const normalizedQuery = query.trim().toLowerCase();
    if (normalizedQuery === "") {
        return packages;
    }

    return packages.filter((pack) =>
        pack.name.toLowerCase().includes(normalizedQuery),
    );
}

export function paginateItems<T>(
    items: T[],
    page: number,
    perPage: number,
): PaginationResult<T> {
    const total = items.length;
    const totalPages = Math.max(1, Math.ceil(total / perPage));
    const safePage = Math.min(Math.max(1, page), totalPages);
    const start = (safePage - 1) * perPage;
    const pagedItems = items.slice(start, start + perPage);

    return {
        items: pagedItems,
        total,
        page: safePage,
        perPage,
        totalPages,
        first: total === 0 ? 0 : start + 1,
        last: Math.min(start + perPage, total),
    };
}

export function getSearchTitle(query: string): string {
    return query ? `Search results for "${query}"` : "All packages";
}

export function getSearchCountLabel(
    pagination: Pick<PaginationResult<unknown>, "first" | "last" | "total">,
    query: string,
): string {
    if (pagination.total === 0) {
        return query
            ? `No packages match "${query}".`
            : "No packages are available.";
    }

    return `Displaying ${pagination.first}-${pagination.last} of ${pagination.total} packages`;
}

export function getEmptySearchMessage(query: string): string {
    return query
        ? `We couldn't find any packages matching "${query}". Try a different search term.`
        : "No packages are available at the moment.";
}
