type uuid = string;

export interface Package {
    id: uuid;
    name: string;
    version: string;
    description: string;
    edition: string;
    authors: string[];
    repository: string;
    license: string;
    metadata: Record<string, unknown>;
}
