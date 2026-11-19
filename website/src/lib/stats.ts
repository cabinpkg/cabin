// Client for the registry's stats endpoints, mounted on this
// site's origin under /api/v1/stats (api/src/web_glue.rs,
// `respond_stats`). Totals cover verified versions only, and the
// registry-wide download count may lag by that endpoint's cache TTL.
// Dependency-free like lib/account.ts; `null` covers every failure
// (unreachable, non-2xx, unexpected shape), so callers render nothing
// rather than partial numbers. During the registry's private alpha the
// endpoints answer only admitted bearer callers, so these fetches fail
// for every visitor and the figures stay hidden.

import type { FetchLike } from "./account.ts";

export interface RegistryStats {
    packages: number;
    versions: number;
    downloads: number;
}

async function getJson(path: string, fetchFn: FetchLike): Promise<unknown> {
    try {
        const response = await fetchFn(path);
        return response.ok ? await response.json() : null;
    } catch {
        return null;
    }
}

export async function getRegistryStats(
    fetchFn: FetchLike = fetch,
): Promise<RegistryStats | null> {
    const body = await getJson("/api/v1/stats", fetchFn);
    if (typeof body !== "object" || body === null) {
        return null;
    }
    const { packages, versions, downloads } = body as Record<string, unknown>;
    if (
        typeof packages !== "number" ||
        typeof versions !== "number" ||
        typeof downloads !== "number"
    ) {
        return null;
    }
    return { packages, versions, downloads };
}

export interface DayDownloads {
    // A UTC day, YYYY-MM-DD.
    day: string;
    downloads: number;
}

export interface PackageDownloads {
    downloads: number;
    // Every UTC day from where counting started, or the first of the
    // last 90, to today, oldest first; a day without downloads is 0.
    days: DayDownloads[];
}

function isCount(value: unknown): value is number {
    return Number.isSafeInteger(value) && (value as number) >= 0;
}

// `name` is the package's `<scope>/<name>`.
export async function getPackageDownloads(
    name: string,
    fetchFn: FetchLike = fetch,
): Promise<PackageDownloads | null> {
    const body = await getJson(`/api/v1/stats/packages/${name}`, fetchFn);
    if (typeof body !== "object" || body === null) {
        return null;
    }
    const { downloads, days } = body as Record<string, unknown>;
    if (
        !isCount(downloads) ||
        !Array.isArray(days) ||
        !days.every((entry: unknown) => {
            const { day, downloads } = (entry ?? {}) as Record<string, unknown>;
            return (
                typeof day === "string" &&
                /^\d{4}-\d{2}-\d{2}$/.test(day) &&
                isCount(downloads)
            );
        })
    ) {
        return null;
    }
    return { downloads, days: days as DayDownloads[] };
}
