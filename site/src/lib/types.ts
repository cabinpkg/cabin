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
