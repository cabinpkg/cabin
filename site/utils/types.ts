export type uuid = string;

export interface User {
    id: uuid;
    name: string;
    user_name: string;
    avatar_url: string;
}

export interface Package {
    id: uuid;
    name: string;
    version: string;
    description: string;
    edition: number;
    authors: string[];
    repository: string;
    license: string;
    metadata: Record<string, unknown>;
    published_at: string;
}

export interface Position {
    first: number;
    last: number;
}
