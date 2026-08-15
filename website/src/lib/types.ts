import type { PackageManifestInfo } from "./manifestInfo.ts";

export interface PackageRecord {
    name: string;
    version: string;
    description: string | null;
    edition: string | null;
    license: string | null;
    // Registry-record fields the ports tree does not carry (package
    // links and the like); normalizePackageMetadata reads it, and the
    // Links card renders its explicit empty state for ports.
    metadata: unknown;
    // Normalized dependency and feature declarations
    // (src/lib/manifestInfo.ts), extracted once at load time so pages
    // and future version comparison share one shape.
    manifest: PackageManifestInfo;
    published_at: string | null;
    readme: string | null;
    repository: string | null;
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
    description: string;
    edition: string;
    published_at: string;
    href: string | null;
}

export type PackageSearchIndexItem = PackageListItem;

export interface PackageLinks {
    homepage?: string;
    documentation?: string;
    repository?: string;
}

export interface NormalizedPackageMetadata {
    links: PackageLinks;
}
