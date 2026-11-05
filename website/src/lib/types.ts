import type { PackageManifestInfo } from "./manifestInfo.ts";

export interface PackageRecord {
    name: string;
    version: string;
    // Dependency and feature declarations (src/lib/manifestInfo.ts).
    manifest: PackageManifestInfo;
    upstream: PackageUpstream | null;
}

// Declared provenance for a package built from an upstream release: the
// pinned source archive.
export interface PackageUpstream {
    archiveUrl: string;
    // Algorithm-prefixed checksum of the pinned archive
    // ("sha256:<64 lowercase hex>"), as the manifest spells it.
    checksum: string;
}

export interface PackageListItem {
    name: string;
    version: string;
    href: string | null;
}

export type PackageSearchIndexItem = PackageListItem;
