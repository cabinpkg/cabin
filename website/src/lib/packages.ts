import semver from "semver";
import type { GetAllPackagesQuery } from "../../graphql";
import { HASURA_ENDPOINT, PACKAGE_FETCH_PAGE_SIZE } from "./constants";
import { formatEdition, stringifyValue } from "./format";
import { getHasuraClient } from "./hasuraClient";
import type {
    NormalizedPackageMetadata,
    PackageLinks,
    PackageSearchIndexItem,
} from "./types";

export type PackageRecord = GetAllPackagesQuery["packages"][number];

export interface PackageRouteParts {
    group: string;
    name: string;
}

export interface PackageVersionRouteParts extends PackageRouteParts {
    version: string;
}

export interface PackageDetailData {
    pack: PackageRecord;
    versionCount: number;
}

let packageCache: Promise<PackageRecord[]> | undefined;

export function fetchAllPackages(
    pageSize = PACKAGE_FETCH_PAGE_SIZE,
): Promise<PackageRecord[]> {
    if (pageSize === PACKAGE_FETCH_PAGE_SIZE) {
        packageCache ??= fetchAllPackagesUncached(pageSize);
        return packageCache;
    }

    return fetchAllPackagesUncached(pageSize);
}

async function fetchAllPackagesUncached(
    pageSize: number,
): Promise<PackageRecord[]> {
    if (!Number.isInteger(pageSize) || pageSize <= 0) {
        throw new Error(`Package page size must be positive. Got ${pageSize}.`);
    }

    const client = getHasuraClient();
    const packages: PackageRecord[] = [];

    for (let offset = 0; ; offset += pageSize) {
        let page: GetAllPackagesQuery;

        try {
            page = await client.getAllPackages({
                limit: pageSize,
                offset,
            });
        } catch (error) {
            const message =
                error instanceof Error ? error.message : String(error);
            throw new Error(
                `Failed to fetch packages from Hasura (${HASURA_ENDPOINT}) at offset ${offset}: ${message}`,
            );
        }

        if (!Array.isArray(page.packages)) {
            throw new Error(
                `Hasura returned an invalid package page at offset ${offset}.`,
            );
        }

        packages.push(...page.packages);

        if (page.packages.length < pageSize) {
            return packages;
        }
    }
}

export function groupPackagesByName(
    packages: PackageRecord[],
): Map<string, PackageRecord[]> {
    const grouped = new Map<string, PackageRecord[]>();

    for (const pack of packages) {
        if (!pack.name) {
            continue;
        }

        const versions = grouped.get(pack.name) ?? [];
        versions.push(pack);
        grouped.set(pack.name, versions);
    }

    return grouped;
}

export function selectLatestPackage(packages: PackageRecord[]): PackageRecord {
    if (packages.length === 0) {
        throw new Error("Cannot select latest package from an empty list.");
    }

    return [...packages].sort(comparePackageVersions)[0];
}

export function comparePackageVersions(
    first: PackageRecord,
    second: PackageRecord,
): number {
    const firstVersion = semver.valid(first.version);
    const secondVersion = semver.valid(second.version);

    if (firstVersion !== null && secondVersion !== null) {
        const semverCompare = semver.rcompare(firstVersion, secondVersion);
        if (semverCompare !== 0) {
            return semverCompare;
        }
    }

    const publishedCompare =
        dateToTime(second.published_at) - dateToTime(first.published_at);
    if (publishedCompare !== 0) {
        return publishedCompare;
    }

    const stringCompare = String(second.version).localeCompare(
        String(first.version),
    );
    if (stringCompare !== 0) {
        return stringCompare;
    }

    return String(first.id).localeCompare(String(second.id));
}

export async function getLatestPackages(): Promise<PackageRecord[]> {
    const grouped = groupPackagesByName(await fetchAllPackages());

    return Array.from(grouped.values())
        .map(selectLatestPackage)
        .sort((first, second) => first.name.localeCompare(second.name));
}

export async function getPackageSearchIndex(): Promise<
    PackageSearchIndexItem[]
> {
    return (await getLatestPackages()).map(toPackageSearchIndexItem);
}

export async function getPackageStaticPaths() {
    const grouped = groupPackagesByName(await fetchAllPackages());

    return Array.from(grouped.entries())
        .flatMap(([packageName, versions]) => {
            const parts = getPackageRouteParts(packageName);

            if (parts === null) {
                return [];
            }

            return [
                {
                    params: parts,
                    props: {
                        pack: selectLatestPackage(versions),
                        versionCount: versions.length,
                    },
                },
            ];
        })
        .sort((first, second) =>
            first.props.pack.name.localeCompare(second.props.pack.name),
        );
}

export async function getPackageVersionStaticPaths() {
    const grouped = groupPackagesByName(await fetchAllPackages());
    const paths: Array<{
        params: PackageVersionRouteParts;
        props: PackageDetailData;
    }> = [];
    const seen = new Set<string>();

    for (const [packageName, versions] of grouped) {
        const parts = getPackageRouteParts(packageName);

        if (parts === null) {
            continue;
        }

        for (const pack of versions) {
            const version = stringifyValue(pack.version);

            if (version === "") {
                continue;
            }

            if (version.includes("/")) {
                throw new Error(
                    `Package "${packageName}" has a version "${version}" that contains "/", which cannot be represented as a single /packages/<group>/<name>/<version> route segment.`,
                );
            }

            const key = `${parts.group}/${parts.name}@${version}`;
            if (seen.has(key)) {
                continue;
            }
            seen.add(key);

            paths.push({
                params: { ...parts, version },
                props: {
                    pack,
                    versionCount: versions.length,
                },
            });
        }
    }

    return paths.sort((first, second) => {
        const nameCompare = first.props.pack.name.localeCompare(
            second.props.pack.name,
        );
        if (nameCompare !== 0) {
            return nameCompare;
        }
        return first.params.version.localeCompare(second.params.version);
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

export function normalizePackageMetadata(
    metadata: unknown,
): NormalizedPackageMetadata {
    const record = isRecord(metadata) ? metadata : {};
    const dependencies = Array.isArray(record.dependencies)
        ? record.dependencies
        : [];

    return {
        dependencies,
        dependencyCount: dependencies.length,
        links: getPackageLinks(record.package),
    };
}

export { formatEdition };

function toPackageSearchIndexItem(pack: PackageRecord): PackageSearchIndexItem {
    return {
        name: pack.name,
        version: pack.version,
        description: pack.description ?? "",
        edition: stringifyValue(pack.edition),
        published_at: stringifyValue(pack.published_at),
        href: getPackageHrefOrNull(pack.name),
    };
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

function getPackageLinks(value: unknown): PackageLinks {
    if (!isRecord(value)) {
        return {};
    }

    const links: PackageLinks = {};
    const homepage = getSafeExternalUrl(value.homepage);
    const documentation = getSafeExternalUrl(value.documentation);
    const repository = getSafeExternalUrl(value.repository);

    if (homepage) {
        links.homepage = homepage;
    }
    if (documentation) {
        links.documentation = documentation;
    }
    if (repository) {
        links.repository = repository;
    }

    return links;
}

function getSafeExternalUrl(value: unknown): string | undefined {
    if (typeof value !== "string" || value.trim() === "") {
        return undefined;
    }

    try {
        const url = new URL(value);
        return url.protocol === "http:" || url.protocol === "https:"
            ? url.toString()
            : undefined;
    } catch {
        return undefined;
    }
}

function dateToTime(value: unknown): number {
    const time = Date.parse(stringifyValue(value));
    return Number.isFinite(time) ? time : 0;
}

function isRecord(value: unknown): value is Record<string, unknown> {
    return typeof value === "object" && value !== null && !Array.isArray(value);
}
