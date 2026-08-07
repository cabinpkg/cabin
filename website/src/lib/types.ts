export interface PackageRecord {
    name: string;
    version: string;
    description: string | null;
    edition: string | null;
    license: string | null;
    metadata: unknown;
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
    dependencies: unknown[];
    dependencyCount: number;
    links: PackageLinks;
}
