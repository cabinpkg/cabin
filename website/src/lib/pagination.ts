import { clamp } from "./clamp";
import { DEFAULT_SEARCH_PER_PAGE } from "./constants";
import { createSearchUrl } from "./search";

export type PaginationItem =
    | {
          type: "previous" | "next";
          key: string;
          label: string;
          page: number;
          href: string;
          current: false;
          disabled: false;
      }
    | {
          type: "previous" | "next";
          key: string;
          label: string;
          page: number;
          href: null;
          current: false;
          disabled: true;
      }
    | {
          type: "page";
          key: string;
          label: string;
          page: number;
          href: string;
          current: boolean;
          disabled: false;
      }
    | {
          type: "ellipsis";
          key: string;
          label: "...";
          page: null;
          href: null;
          current: false;
          disabled: true;
      };

export interface PaginationModelOptions {
    query?: string;
    currentPage: number;
    perPage?: number;
    totalPages: number;
}

function getVisiblePageNumbers(
    currentPage: number,
    totalPages: number,
): Array<number | null> {
    const safeTotalPages = Math.max(1, totalPages);
    const safeCurrentPage = clamp(currentPage, 1, safeTotalPages);
    const pages = new Set([1, safeTotalPages]);

    for (
        let pageNumber = Math.max(1, safeCurrentPage - 1);
        pageNumber <= Math.min(safeTotalPages, safeCurrentPage + 1);
        pageNumber += 1
    ) {
        pages.add(pageNumber);
    }

    const sorted = [...pages].sort((first, second) => first - second);
    const visible: Array<number | null> = [];

    for (const pageNumber of sorted) {
        const previous = visible[visible.length - 1];
        if (typeof previous === "number" && pageNumber - previous > 1) {
            visible.push(null);
        }
        visible.push(pageNumber);
    }

    return visible;
}

export function getPaginationItems({
    query = "",
    currentPage,
    perPage = DEFAULT_SEARCH_PER_PAGE,
    totalPages,
}: PaginationModelOptions): PaginationItem[] {
    const safeTotalPages = Math.max(1, totalPages);
    const safeCurrentPage = clamp(currentPage, 1, safeTotalPages);
    const previousPage = Math.max(1, safeCurrentPage - 1);
    const nextPage = Math.min(safeTotalPages, safeCurrentPage + 1);
    const previousDisabled = safeCurrentPage === 1;
    const nextDisabled = safeCurrentPage === safeTotalPages;
    const items: PaginationItem[] = [
        getDirectionalPaginationItem({
            type: "previous",
            label: "Previous",
            page: previousPage,
            disabled: previousDisabled,
            query,
            perPage,
        }),
    ];

    getVisiblePageNumbers(safeCurrentPage, safeTotalPages).forEach(
        (pageNumber, index) => {
            if (pageNumber === null) {
                items.push({
                    type: "ellipsis",
                    key: `ellipsis-${index}`,
                    label: "...",
                    page: null,
                    href: null,
                    current: false,
                    disabled: true,
                });
                return;
            }

            items.push({
                type: "page",
                key: `page-${pageNumber}`,
                label: String(pageNumber),
                page: pageNumber,
                href: createSearchUrl({ query, page: pageNumber, perPage }),
                current: pageNumber === safeCurrentPage,
                disabled: false,
            });
        },
    );

    items.push(
        getDirectionalPaginationItem({
            type: "next",
            label: "Next",
            page: nextPage,
            disabled: nextDisabled,
            query,
            perPage,
        }),
    );

    return items;
}

function getDirectionalPaginationItem({
    type,
    label,
    page,
    disabled,
    query,
    perPage,
}: {
    type: "previous" | "next";
    label: string;
    page: number;
    disabled: boolean;
    query: string;
    perPage: number;
}): PaginationItem {
    if (disabled) {
        return {
            type,
            key: type,
            label,
            page,
            href: null,
            current: false,
            disabled: true,
        };
    }

    return {
        type,
        key: type,
        label,
        page,
        href: createSearchUrl({ query, page, perPage }),
        current: false,
        disabled: false,
    };
}
