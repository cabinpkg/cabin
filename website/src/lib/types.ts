import type { PackageManifestInfo } from "./manifestInfo.ts";

export interface PackageRecord {
    name: string;
    version: string;
    // Normalized dependency and feature declarations
    // (src/lib/manifestInfo.ts), extracted once at load time so pages
    // and future version comparison share one shape.
    manifest: PackageManifestInfo;
    upstream: PackageUpstream | null;
}

// Committed provenance for a package that repackages an upstream
// release (the cabin-ports packages): the upstream version, plus the
// pinned source archive.
export interface PackageUpstream {
    version: string;
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
