import { existsSync } from "node:fs";
import { readdir, readFile, stat } from "node:fs/promises";
import { dirname, join } from "node:path";
import { parse as parseToml } from "smol-toml";
import type { PackageRecord } from "./types";

// The ports/ directory lives at the repo root, a sibling of this website
// project. Resolve it by walking up from the current working directory to the
// nearest ancestor that contains a ports/ directory, so it works whether the
// build runs from website/ (local `yarn build`, CI) or the repo root. We avoid
// import.meta.url because `astro build` bundles this module into
// dist/.prerender/chunks/ at a different depth than this source file.
function resolvePortsDir(): string {
    let dir = process.cwd();
    let parent = dirname(dir);
    while (dir !== parent) {
        const candidate = join(dir, "ports");
        if (existsSync(candidate)) {
            return candidate;
        }
        dir = parent;
        parent = dirname(dir);
    }
    const rootCandidate = join(dir, "ports");
    return existsSync(rootCandidate)
        ? rootCandidate
        : join(process.cwd(), "ports");
}

const PORTS_DIR = resolvePortsDir();

interface PortTomlPort {
    name: string;
    version: string;
    description?: string;
    license?: string;
    homepage?: string;
    upstream?: string;
}

interface PortToml {
    port: PortTomlPort;
}

export async function loadPortsAsPackageRecords(): Promise<PackageRecord[]> {
    if (!(await directoryExists(PORTS_DIR))) {
        throw new Error(
            `Ports directory not found at ${PORTS_DIR}. Expected the cabin ports/ directory at the repo root.`,
        );
    }

    const records: PackageRecord[] = [];
    const seen = new Set<string>();

    for (const portName of await listDirectories(PORTS_DIR)) {
        const portDir = join(PORTS_DIR, portName);
        for (const version of await listDirectories(portDir)) {
            const portTomlPath = join(portDir, version, "port.toml");
            const record = await loadPortRecord(portTomlPath);
            const key = `${record.name}@${record.version}`;
            if (seen.has(key)) {
                throw new Error(
                    `Duplicate port entry ${key} encountered at ${portTomlPath}.`,
                );
            }
            seen.add(key);
            records.push(record);
        }
    }

    return records;
}

async function loadPortRecord(portTomlPath: string): Promise<PackageRecord> {
    let raw: string;
    try {
        raw = await readFile(portTomlPath, "utf-8");
    } catch (error) {
        throw new Error(
            `Failed to read port.toml at ${portTomlPath}: ${errorMessage(error)}`,
        );
    }

    let parsed: PortToml;
    try {
        parsed = parseToml(raw) as unknown as PortToml;
    } catch (error) {
        throw new Error(
            `Failed to parse TOML at ${portTomlPath}: ${errorMessage(error)}`,
        );
    }

    const port = parsed.port;
    if (!port || typeof port.name !== "string" || port.name === "") {
        throw new Error(`Missing [port].name in ${portTomlPath}.`);
    }
    if (typeof port.version !== "string" || port.version === "") {
        throw new Error(`Missing [port].version in ${portTomlPath}.`);
    }
    if (port.name.includes("/")) {
        throw new Error(
            `Port name "${port.name}" in ${portTomlPath} must not contain "/".`,
        );
    }

    const homepage = stringOrNull(port.homepage);
    const repository = stringOrNull(port.upstream);

    return {
        name: `ports/${port.name}`,
        version: port.version,
        description: stringOrNull(port.description),
        edition: null,
        license: stringOrNull(port.license),
        metadata: {
            package: {
                ...(homepage !== null ? { homepage } : {}),
                ...(repository !== null ? { repository } : {}),
            },
            dependencies: [],
        },
        published_at: null,
        readme: null,
        repository,
    };
}

async function listDirectories(parent: string): Promise<string[]> {
    const entries = await readdir(parent, { withFileTypes: true });
    return entries
        .filter((entry) => entry.isDirectory())
        .map((entry) => entry.name)
        .sort();
}

async function directoryExists(path: string): Promise<boolean> {
    try {
        return (await stat(path)).isDirectory();
    } catch {
        return false;
    }
}

function stringOrNull(value: unknown): string | null {
    return typeof value === "string" && value !== "" ? value : null;
}

function errorMessage(error: unknown): string {
    return error instanceof Error ? error.message : String(error);
}
