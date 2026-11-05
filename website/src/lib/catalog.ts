import { readFileSync } from "node:fs";
import { join } from "node:path";
import { isRecord, versionManifestInfo } from "./manifestInfo.ts";
import type { PackageRecord, PackageUpstream } from "./types.ts";

// The build's one package-data input: .catalog/, written by
// scripts/fetch-catalog.mjs from the registry's GET /catalog.json, or,
// for the fixture builds alone, by scripts/build-fixtures.mjs.  Which
// packages and versions exist, each version's current revision, and the
// ordering are the registry's (docs/remote-registry.md, "The catalog");
// this module only checks the document's shape and maps it for
// rendering.  Resolved against the working directory, website/: Astro
// bundles this module under dist/.prerender/chunks/, where
// import.meta.url points elsewhere.
const CATALOG_DIR = join(process.cwd(), ".catalog");

export interface CatalogProvenance {
    source: "registry" | "fixture";
    state: "available" | "unavailable";
    url?: string;
    sha256?: string;
}

export interface CatalogPackage {
    name: string;
    // The version the registry names latest, one of `versions`.
    latest: PackageRecord;
    // Descending SemVer precedence, as the registry orders them.
    versions: PackageRecord[];
}

export interface Catalog {
    provenance: CatalogProvenance;
    // Bytewise by name, as the registry orders them; empty when the
    // registry does not serve the catalog anonymously.
    packages: CatalogPackage[];
}

export function loadCatalog(directory = CATALOG_DIR): Catalog {
    const read = (file: string) => {
        try {
            return readFileSync(join(directory, file), "utf8");
        } catch (error) {
            throw new Error(
                `Cannot read ${join(directory, file)}; run \`npm run fetch:catalog\`. ${error instanceof Error ? error.message : error}`,
            );
        }
    };
    const provenance = JSON.parse(read("provenance.json")) as CatalogProvenance;
    switch (provenance.state) {
        case "unavailable":
            return { provenance, packages: [] };
        case "available":
            return { provenance, packages: parseCatalog(read("catalog.json")) };
        default:
            throw new Error(
                `Unknown catalog state ${JSON.stringify(provenance.state)}.`,
            );
    }
}

export function parseCatalog(text: string): CatalogPackage[] {
    const document: unknown = JSON.parse(text);
    if (
        !isRecord(document) ||
        document.schema !== 1 ||
        !Array.isArray(document.packages)
    ) {
        throw new Error("The catalog is not a schema-1 catalog document.");
    }
    return document.packages.map(catalogPackage);
}

function catalogPackage(entry: unknown): CatalogPackage {
    if (
        !isRecord(entry) ||
        typeof entry.name !== "string" ||
        typeof entry.latest !== "string" ||
        !Array.isArray(entry.versions)
    ) {
        throw new Error(
            `Malformed catalog package ${JSON.stringify(entry).slice(0, 200)}.`,
        );
    }
    const name = entry.name;
    const versions = entry.versions.map((version) =>
        packageRecord(name, version),
    );
    const latest = versions.find((record) => record.version === entry.latest);
    if (latest === undefined) {
        throw new Error(
            `Catalog package ${name} names latest ${entry.latest}, which it does not list.`,
        );
    }
    return { name, latest, versions };
}

function packageRecord(name: string, entry: unknown): PackageRecord {
    if (!isRecord(entry) || typeof entry.version !== "string") {
        throw new Error(`Malformed catalog version of ${name}.`);
    }
    const context = `${name}@${entry.version}`;
    return {
        name,
        version: entry.version,
        manifest: versionManifestInfo(entry, context),
        upstream: upstream(entry.upstream, context),
    };
}

function upstream(value: unknown, context: string): PackageUpstream | null {
    if (value === undefined) {
        return null;
    }
    if (
        !isRecord(value) ||
        typeof value.url !== "string" ||
        typeof value.checksum !== "string"
    ) {
        throw new Error(`Malformed upstream block in ${context}.`);
    }
    // Rendered as a link, so only an https URL may become an href; the
    // registry already refuses anything else at publish.
    if (new URL(value.url).protocol !== "https:") {
        throw new Error(`The upstream URL of ${context} is not https.`);
    }
    return { archiveUrl: value.url, checksum: value.checksum };
}
