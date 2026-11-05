import { type Catalog, type CatalogPackage, loadCatalog } from "./catalog.ts";
import type { PackageRecord, PackageSearchIndexItem } from "./types.ts";

export type { PackageRecord };

export interface PackageRouteParts {
    group: string;
    name: string;
}

export interface PackageVersionRouteParts extends PackageRouteParts {
    version: string;
}

export interface PackageVersionListItem {
    version: string;
    href: string;
    isLatest: boolean;
}

export interface PackageDetailData {
    pack: PackageRecord;
    versions: PackageVersionListItem[];
}

let catalogCache: Catalog | undefined;

// Loaded once per build; every page renders the same catalog.
export function getCatalog(): Catalog {
    catalogCache ??= loadCatalog();
    return catalogCache;
}

// The navigation between a package's versions, in the registry's order
// (descending SemVer precedence), marking the version it names latest.
export function buildVersionList(
    pack: CatalogPackage,
): PackageVersionListItem[] {
    return pack.versions.map((record) => ({
        version: record.version,
        href: getPackageVersionHref(pack.name, record.version),
        isLatest: record === pack.latest,
    }));
}

export function getPackageSearchIndex(): PackageSearchIndexItem[] {
    return getCatalog().packages.map((pack) => ({
        name: pack.name,
        version: pack.latest.version,
        href: getPackageHrefOrNull(pack.name),
    }));
}

export function getPackageStaticPaths() {
    return getCatalog().packages.flatMap((pack) => {
        const parts = getPackageRouteParts(pack.name);
        return parts === null
            ? []
            : [
                  {
                      params: parts,
                      props: {
                          pack: pack.latest,
                          versions: buildVersionList(pack),
                      },
                  },
              ];
    });
}

export function getPackageVersionStaticPaths(): Array<{
    params: PackageVersionRouteParts;
    props: PackageDetailData;
}> {
    return getCatalog().packages.flatMap((pack) => {
        const parts = getPackageRouteParts(pack.name);
        if (parts === null) {
            return [];
        }
        const versions = buildVersionList(pack);
        return pack.versions.map((record) => ({
            params: { ...parts, version: record.version },
            props: { pack: record, versions },
        }));
    });
}

export function getPackageHref(packageName: string): string {
    const parts = getPackageRouteParts(packageName);

    if (parts === null) {
        throw new Error(
            `Package name "${packageName}" cannot be represented by /packages/<group>/<name>.`,
        );
    }

    return `/packages/${encodeURIComponent(parts.group)}/${encodeURIComponent(parts.name)}`;
}

export function getPackageVersionHref(
    packageName: string,
    version: string,
): string {
    const parts = getPackageRouteParts(packageName);

    if (parts === null) {
        throw new Error(
            `Package name "${packageName}" cannot be represented by /packages/<group>/<name>/<version>.`,
        );
    }

    return `/packages/${encodeURIComponent(parts.group)}/${encodeURIComponent(parts.name)}/${encodeURIComponent(version)}`;
}

function getPackageRouteParts(packageName: string): PackageRouteParts | null {
    const parts = packageName.split("/");

    if (parts.length !== 2 || parts.some((part) => part.length === 0)) {
        return null;
    }

    return {
        group: parts[0],
        name: parts[1],
    };
}

function getPackageHrefOrNull(packageName: string): string | null {
    try {
        return getPackageHref(packageName);
    } catch {
        return null;
    }
}
