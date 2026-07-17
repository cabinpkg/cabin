// Client for the registry's public stats endpoint, mounted on this
// site's origin at /api/v1/stats (registry/docs/architecture.md,
// "Download counts"). Totals cover verified versions only, and the
// download count is approximate - it may lag by the endpoint's cache
// TTL. Dependency-free like lib/account.ts; `null` covers every
// failure (unreachable, non-2xx, unexpected shape), so callers render
// nothing rather than partial numbers.

export interface RegistryStats {
    packages: number;
    versions: number;
    downloads: number;
}

export async function getRegistryStats(): Promise<RegistryStats | null> {
    let response: Response;
    try {
        response = await fetch("/api/v1/stats");
    } catch {
        return null;
    }
    if (!response.ok) {
        return null;
    }
    let body: unknown;
    try {
        body = await response.json();
    } catch {
        return null;
    }
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
