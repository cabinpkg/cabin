import type { PackageListItem } from "./types";

// Fetch the statically generated search index (`/packages.json`). Shared by the
// /search page script and the header typeahead; each layers its own caching or
// error-UI handling on top.
export async function fetchPackageIndex(): Promise<PackageListItem[]> {
    const response = await fetch("/packages.json", {
        headers: { accept: "application/json" },
    });
    if (!response.ok) {
        throw new Error(`HTTP ${response.status}`);
    }
    return (await response.json()) as PackageListItem[];
}
