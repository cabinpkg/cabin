import Fuse, { type IFuseOptions } from "fuse.js";
import type { PackageListItem } from "./types";

export type LinkablePackageListItem = PackageListItem & { href: string };

export interface PackageSearch {
    search(query: string): PackageListItem[];
    suggestions(query: string, limit: number): LinkablePackageListItem[];
}

const SEARCH_OPTIONS = {
    ignoreLocation: true,
    threshold: 0.35,
    keys: [
        { name: "name", weight: 0.7 },
        { name: "description", weight: 0.2 },
        { name: "version", weight: 0.06 },
        { name: "edition", weight: 0.04 },
    ],
} satisfies IFuseOptions<PackageListItem>;

export function createPackageSearch(
    packages: PackageListItem[],
): PackageSearch {
    const fuse = new Fuse(packages, SEARCH_OPTIONS);

    function search(query: string): PackageListItem[] {
        const normalizedQuery = query.trim();
        if (!normalizedQuery) {
            return packages;
        }

        return fuse.search(normalizedQuery).map((result) => result.item);
    }

    function suggestions(
        query: string,
        limit: number,
    ): LinkablePackageListItem[] {
        if (limit <= 0) {
            return [];
        }

        return search(query).filter(isLinkablePackage).slice(0, limit);
    }

    return { search, suggestions };
}

function isLinkablePackage(
    pack: PackageListItem,
): pack is LinkablePackageListItem {
    return pack.href !== null;
}
