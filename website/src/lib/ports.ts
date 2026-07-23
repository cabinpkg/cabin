import { existsSync } from "node:fs";
import { readdir, readFile, stat } from "node:fs/promises";
import { dirname, join } from "node:path";
import { parse as parseToml } from "smol-toml";
import type { PackageRecord } from "./types";

// The foundation-port recipes live inside the cabin-port crate at
// crates/cabin-port/ports/.  Resolve that directory by walking up from the
// current working directory to the nearest ancestor that contains it, so it
// works whether the build runs from website/ (local `npm run build`, CI) or the
// repo root.  We avoid import.meta.url because `astro build` bundles this
// module into dist/.prerender/chunks/ at a different depth than this source
// file.
const PORTS_SUBPATH = join("crates", "cabin-port", "ports");
function resolvePortsDir(): string {
    let dir = process.cwd();
    let parent = dirname(dir);
    while (dir !== parent) {
        const candidate = join(dir, PORTS_SUBPATH);
        if (existsSync(candidate)) {
            return candidate;
        }
        dir = parent;
        parent = dirname(dir);
    }
    const rootCandidate = join(dir, PORTS_SUBPATH);
    return existsSync(rootCandidate)
        ? rootCandidate
        : join(process.cwd(), PORTS_SUBPATH);
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
            `Ports directory not found at ${PORTS_DIR}. Expected crates/cabin-port/ports/ in the cabin repository.`,
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
    // Restrict port names to the Cargo-style alphabet (ASCII letters,
    // digits, "_", "-").  This is stricter than Cabin's own grammar, which
    // also permits "."; a dotted name would render as a TOML dotted key in
    // the install snippet ("foo.bar = { ... }" is a nested table, not a
    // dependency named "foo.bar") and would also break the two-segment route.
    if (!/^[A-Za-z0-9_-]+$/.test(port.name)) {
        throw new Error(
            `Port name "${port.name}" in ${portTomlPath} is invalid; allowed characters are ASCII letters, digits, "_", and "-".`,
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
