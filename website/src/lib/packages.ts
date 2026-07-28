import semver from "semver";
import { formatEdition, stringifyValue } from "./format.ts";
import { loadPortsAsPackageRecords } from "./ports.ts";
import type {
    NormalizedPackageMetadata,
    PackageLinks,
    PackageRecord,
    PackageSearchIndexItem,
} from "./types.ts";
import { parseHttpUrl } from "./url.ts";

export type { PackageRecord };

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

export function fetchAllPackages(): Promise<PackageRecord[]> {
    packageCache ??= loadPortsAsPackageRecords();
    return packageCache;
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
        // semver comparison ignores build metadata, but packaging
        // revisions live there and resolvers prefer the highest
        // (docs/foundation-ports.md, "Packaging revisions") - break
        // the tie numerically so +cabin.10 outranks +cabin.2.
        const buildCompare = compareBuildMetadata(
            semver.parse(first.version)?.build ?? [],
            semver.parse(second.version)?.build ?? [],
        );
        if (buildCompare !== 0) {
            return -buildCompare;
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

    return String(first.name).localeCompare(String(second.name));
}

// Mirror of `semver::BuildMetadata`'s ordering, which is what the
// Rust resolver applies when it prefers the highest packaging
// revision: numeric identifiers compare by significant-digit count,
// then digit string, then original length (leading zeros rank
// higher on ties); numeric ranks below alphanumeric; alphanumeric
// compares bytewise (not locale collation); fewer identifiers rank
// lower.  Diverging here would make the site's "latest" disagree
// with what the resolver selects.
export function compareBuildMetadata(
    first: readonly string[],
    second: readonly string[],
): number {
    const length = Math.max(first.length, second.length);
    for (let index = 0; index < length; index++) {
        const a = first[index];
        const b = second[index];
        if (a === undefined) {
            return -1;
        }
        if (b === undefined) {
            return 1;
        }
        const aNumeric = /^[0-9]+$/.test(a);
        const bNumeric = /^[0-9]+$/.test(b);
        if (aNumeric && bNumeric) {
            const aDigits = a.replace(/^0+/, "");
            const bDigits = b.replace(/^0+/, "");
            if (aDigits.length !== bDigits.length) {
                return aDigits.length < bDigits.length ? -1 : 1;
            }
            if (aDigits !== bDigits) {
                return aDigits < bDigits ? -1 : 1;
            }
            if (a.length !== b.length) {
                return a.length < b.length ? -1 : 1;
            }
        } else if (aNumeric !== bNumeric) {
            return aNumeric ? -1 : 1;
        } else if (a !== b) {
            return a < b ? -1 : 1;
        }
    }
    return 0;
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

    return `/packages/${encodeURIComponent(parts.group)}/${encodeURIComponent(parts.name)}/${encodeVersionSegment(version)}`;
}

// Astro emits the [version] route with a literal `+` (its param
// sanitizer only escapes `#` and `?`), and on a static host
// /1.3.1+cabin.2 and /1.3.1%2Bcabin.2 name different resources - keep
// the emitted spelling so hrefs, canonical URLs, and the sitemap
// agree.
function encodeVersionSegment(version: string): string {
    return encodeURIComponent(version).replaceAll("%2B", "+");
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

const PACKAGE_LINK_KEYS = ["homepage", "documentation", "repository"] as const;

function getPackageLinks(value: unknown): PackageLinks {
    if (!isRecord(value)) {
        return {};
    }

    const links: PackageLinks = {};
    for (const key of PACKAGE_LINK_KEYS) {
        const url = getSafeExternalUrl(value[key]);
        if (url) {
            links[key] = url;
        }
    }

    return links;
}

function getSafeExternalUrl(value: unknown): string | undefined {
    return parseHttpUrl(value)?.toString();
}

function dateToTime(value: unknown): number {
    const time = Date.parse(stringifyValue(value));
    return Number.isFinite(time) ? time : 0;
}

function isRecord(value: unknown): value is Record<string, unknown> {
    return typeof value === "object" && value !== null && !Array.isArray(value);
}
