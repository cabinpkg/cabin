import { existsSync } from "node:fs";
import { readdir, readFile, stat } from "node:fs/promises";
import { dirname, join } from "node:path";
import { parse as parseToml } from "smol-toml";
import type { PackageRecord } from "./types";

// The foundation ports live inside the cabin-port crate at
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

// The registry identity every port publishes under, mirroring the
// cabin-port-publish conversion (crates/cabin-port-publish): the site must
// present exactly the names and versions the registry serves, without a
// live-registry build dependency.
const REGISTRY_SCOPE = "cabin-ports";

export function loadPortsAsPackageRecords(): Promise<PackageRecord[]> {
    return loadPortsFromDir(PORTS_DIR);
}

// Exported with an explicit directory so tests can exercise the
// loader against fixture trees; the site itself always loads the
// committed ports tree.
export async function loadPortsFromDir(
    portsDir: string,
): Promise<PackageRecord[]> {
    if (!(await directoryExists(portsDir))) {
        throw new Error(
            `Ports directory not found at ${portsDir}. Expected crates/cabin-port/ports/ in the cabin repository.`,
        );
    }

    const records: PackageRecord[] = [];
    const seen = new Set<string>();
    // Two distinct port directory names must not fold onto one scoped
    // name (cJSON/ next to cjson/ would silently merge under
    // cabin-ports/cjson) - the publisher rejects that, so must we.
    const scopedOwners = new Map<string, string>();

    for (const portName of await listDirectories(portsDir)) {
        const portDir = join(portsDir, portName);
        for (const version of await listDirectories(portDir)) {
            const manifestPath = join(portDir, version, "cabin.toml");
            if (!existsSync(manifestPath)) {
                // An auxiliary directory (the publisher skips these
                // too).
                continue;
            }
            const record = await loadPackageRecord(
                manifestPath,
                portName,
                version,
            );
            const owner = scopedOwners.get(record.name);
            if (owner !== undefined && owner !== portName) {
                throw new Error(
                    `Ports "${owner}" and "${portName}" both publish as "${record.name}"; published names must stay distinct.`,
                );
            }
            scopedOwners.set(record.name, portName);
            const key = `${record.name}@${record.version}`;
            if (seen.has(key)) {
                throw new Error(
                    `Duplicate port entry ${key} encountered at ${manifestPath}.`,
                );
            }
            seen.add(key);
            records.push(record);
        }
    }

    return records;
}

// A migrated port: the committed manifest already carries the
// canonical scoped identity and a complete [package.upstream] block,
// so nothing is converted.  The manifest has no description, license,
// or homepage fields, so those stay null and their UI sections hide.
async function loadPackageRecord(
    manifestPath: string,
    portName: string,
    versionDir: string,
): Promise<PackageRecord> {
    let raw: string;
    try {
        raw = await readFile(manifestPath, "utf-8");
    } catch (error) {
        throw new Error(
            `Failed to read cabin.toml at ${manifestPath}: ${errorMessage(error)}`,
        );
    }
    let parsed: {
        package?: {
            name?: string;
            version?: string;
            upstream?: { url?: string; sha256?: string };
        };
        dependencies?: Record<string, unknown>;
    };
    try {
        parsed = parseToml(raw) as typeof parsed;
    } catch (error) {
        throw new Error(
            `Failed to parse TOML at ${manifestPath}: ${errorMessage(error)}`,
        );
    }

    const name = stringOrNull(parsed.package?.name);
    if (name === null) {
        throw new Error(`Missing [package].name in ${manifestPath}.`);
    }
    // The directory layout is the identity: a moved or mislabeled
    // directory fails the build instead of publishing a page under a
    // surprising identity, exactly as the publisher refuses it.
    const expectedName = scopedPackageName(portName, manifestPath);
    if (name !== expectedName) {
        throw new Error(
            `${manifestPath} declares "${name}", which disagrees with its directory identity "${expectedName}".`,
        );
    }
    const version = stringOrNull(parsed.package?.version);
    if (version === null) {
        throw new Error(`Missing [package].version in ${manifestPath}.`);
    }
    if (version !== versionDir) {
        throw new Error(
            `${manifestPath} declares version ${version}, which disagrees with its "${versionDir}/" directory.`,
        );
    }

    const archiveUrl = stringOrNull(parsed.package?.upstream?.url);
    const sha256 = stringOrNull(parsed.package?.upstream?.sha256);
    if (archiveUrl === null || sha256 === null) {
        throw new Error(
            `Missing [package.upstream] url or sha256 in ${manifestPath}.`,
        );
    }
    const parsedUrl = parseProvenanceUrl(archiveUrl, manifestPath);
    if (!/^[0-9a-f]{64}$/.test(sha256)) {
        throw new Error(
            `[package.upstream].sha256 in ${manifestPath} must be a lowercase 64-character hex digest.`,
        );
    }

    const dependencies = Object.entries(parsed.dependencies ?? {}).map(
        ([depName, spec]) => dependencyRequirement(depName, spec, manifestPath),
    );

    return {
        name,
        version,
        description: null,
        edition: null,
        license: null,
        metadata: { package: {}, dependencies },
        published_at: null,
        readme: null,
        repository: null,
        upstream: { version, archiveUrl: parsedUrl.toString(), sha256 },
    };
}

// Shared provenance-URL rule: credential-free HTTPS, parsed rather
// than prefix-matched so this loader accepts whatever the client's
// URL parser normalizes.
function parseProvenanceUrl(archiveUrl: string, context: string): URL {
    let parsedUrl: URL;
    try {
        parsedUrl = new URL(archiveUrl);
    } catch {
        throw new Error(
            `The upstream URL in ${context} is not a valid URL: "${archiveUrl}".`,
        );
    }
    if (parsedUrl.protocol !== "https:") {
        throw new Error(
            `The upstream URL in ${context} must be an https:// URL; got "${archiveUrl}".`,
        );
    }
    if (parsedUrl.username !== "" || parsedUrl.password !== "") {
        throw new Error(
            `The upstream URL in ${context} must not embed credentials.`,
        );
    }
    return parsedUrl;
}

// One dependency entry, in either spelling a published port manifest
// can carry: the bare requirement string and the table with a
// `version` key.  The publisher publishes the manifest verbatim, so
// the page reads exactly what will publish.  This is a shape check, not
// a SemVer parse: it refuses a missing or blank requirement (Cabin
// trims, then refuses the empty string), and leaves malformed
// requirement syntax to the publisher and the registry.
function dependencyRequirement(
    name: string,
    spec: unknown,
    context: string,
): { name: string; req: string } {
    const req =
        typeof spec === "string"
            ? spec
            : typeof spec === "object" && spec !== null
              ? (spec as { version?: unknown }).version
              : undefined;
    if (typeof req !== "string" || req.trim() === "") {
        throw new Error(
            `Dependency "${name}" in ${context} declares no version requirement; port packages carry registry dependencies only.`,
        );
    }
    return { name, req };
}

// The publisher's identity rule (cabin-port-publish::plan::
// scoped_package_name): lowercase the port name and require the
// canonical registry grammar `[a-z0-9][a-z0-9_-]*`, so cJSON becomes
// cabin-ports/cjson and a name the registry would reject fails the
// build here instead of publishing a page for a package that cannot
// exist.
export function scopedPackageName(portName: string, context: string): string {
    const lower = portName.toLowerCase();
    if (!/^[a-z0-9][a-z0-9_-]*$/.test(lower)) {
        throw new Error(
            `Port name "${portName}" in ${context} does not lowercase to a canonical registry package name ([a-z0-9][a-z0-9_-]*).`,
        );
    }
    return `${REGISTRY_SCOPE}/${lower}`;
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
