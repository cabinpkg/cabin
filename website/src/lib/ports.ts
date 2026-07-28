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

// The registry identity every recipe publishes under, mirroring the
// cabin-port-publish conversion (crates/cabin-port-publish): the site must
// present exactly the names and versions the registry serves, without a
// live-registry build dependency.
const REGISTRY_SCOPE = "cabin-ports";
const REVISION_FILENAME = "packaging-revision";

interface PortTomlPort {
    name: string;
    version: string;
    description?: string;
    license?: string;
    homepage?: string;
    upstream?: string;
}

interface PortTomlSource {
    url?: string;
    sha256?: string;
}

interface PortTomlOverlay {
    manifest?: string;
}

interface PortToml {
    port: PortTomlPort;
    source?: PortTomlSource;
    overlay?: PortTomlOverlay;
}

export function loadPortsAsPackageRecords(): Promise<PackageRecord[]> {
    return loadPortsFromDir(PORTS_DIR);
}

// Exported with an explicit directory so tests can exercise the
// loader (packaging-revision sidecars in particular) against fixture
// trees; the site itself always loads the committed recipes.
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
    // Two distinct recipe names must not fold onto one scoped name
    // (cJSON/ next to cjson/ would silently merge under
    // cabin-ports/cjson) - the publisher rejects that, so must we.
    const scopedOwners = new Map<string, string>();

    for (const portName of await listDirectories(portsDir)) {
        const portDir = join(portsDir, portName);
        for (const version of await listDirectories(portDir)) {
            const portTomlPath = join(portDir, version, "port.toml");
            const record = await loadPortRecord(portTomlPath);
            const owner = scopedOwners.get(record.name);
            if (owner !== undefined && owner !== portName) {
                throw new Error(
                    `Ports "${owner}" and "${portName}" both convert to "${record.name}"; converted names must stay distinct.`,
                );
            }
            scopedOwners.set(record.name, portName);
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
    const scopedName = scopedPackageName(port.name, portTomlPath);

    const archiveUrl = stringOrNull(parsed.source?.url);
    const sha256 = stringOrNull(parsed.source?.sha256);
    if (archiveUrl === null || sha256 === null) {
        throw new Error(
            `Missing [source].url or [source].sha256 in ${portTomlPath}.`,
        );
    }
    // Mirror the committed-recipe provenance rules the publisher
    // enforces: credential-free HTTPS archives pinned by a lowercase
    // 64-hex SHA-256.  Parse the URL rather than prefix-matching it -
    // the publisher's URL parser normalizes e.g. an uppercase scheme,
    // and this loader must accept whatever it accepts.
    let parsedUrl: URL;
    try {
        parsedUrl = new URL(archiveUrl);
    } catch {
        throw new Error(
            `[source].url in ${portTomlPath} is not a valid URL: "${archiveUrl}".`,
        );
    }
    if (parsedUrl.protocol !== "https:") {
        throw new Error(
            `[source].url in ${portTomlPath} must be an https:// URL; got "${archiveUrl}".`,
        );
    }
    if (parsedUrl.username !== "" || parsedUrl.password !== "") {
        throw new Error(
            `[source].url in ${portTomlPath} must not embed credentials.`,
        );
    }
    if (!/^[0-9a-f]{64}$/.test(sha256)) {
        throw new Error(
            `[source].sha256 in ${portTomlPath} must be a lowercase 64-character hex digest.`,
        );
    }

    const revision = await readPackagingRevision(dirname(portTomlPath));
    const version = publishedVersion(port.version, revision, portTomlPath);
    const dependencies = await overlayDependencies(
        parsed.overlay,
        portTomlPath,
    );

    const homepage = stringOrNull(port.homepage);
    const repository = stringOrNull(port.upstream);

    return {
        name: scopedName,
        version,
        description: stringOrNull(port.description),
        edition: null,
        license: stringOrNull(port.license),
        metadata: {
            package: {
                ...(homepage !== null ? { homepage } : {}),
                ...(repository !== null ? { repository } : {}),
            },
            dependencies,
        },
        published_at: null,
        readme: null,
        repository,
        upstream: {
            version: port.version,
            archiveUrl: parsedUrl.toString(),
            sha256,
        },
    };
}

// The published dependency set comes from the overlay manifest, not
// port.toml: the publisher rewrites the overlay's inter-port
// `{ port = true }` edges into scoped registry dependencies
// (cabin-port-publish::convert::rewrite_port_dependencies), so the
// page's dependency count must read the same file or it under-counts
// (libpng depends on zlib).
async function overlayDependencies(
    overlay: PortTomlOverlay | undefined,
    portTomlPath: string,
): Promise<Array<{ name: string; req: string }>> {
    const manifest = stringOrNull(overlay?.manifest);
    if (manifest === null) {
        throw new Error(`Missing [overlay].manifest in ${portTomlPath}.`);
    }
    const overlayPath = join(dirname(portTomlPath), manifest);
    let raw: string;
    try {
        raw = await readFile(overlayPath, "utf-8");
    } catch (error) {
        throw new Error(
            `Failed to read overlay manifest at ${overlayPath}: ${errorMessage(error)}`,
        );
    }
    let parsed: { dependencies?: Record<string, unknown> };
    try {
        parsed = parseToml(raw) as typeof parsed;
    } catch (error) {
        throw new Error(
            `Failed to parse TOML at ${overlayPath}: ${errorMessage(error)}`,
        );
    }
    const dependencies = parsed.dependencies;
    if (dependencies === undefined) {
        return [];
    }
    return Object.entries(dependencies).map(([name, spec]) => {
        if (
            typeof spec === "object" &&
            spec !== null &&
            (spec as { port?: unknown }).port === true
        ) {
            const req = (spec as { version?: unknown }).version;
            if (typeof req !== "string" || req === "") {
                throw new Error(
                    `Port dependency "${name}" in ${overlayPath} has no version requirement.`,
                );
            }
            return { name: scopedPackageName(name, overlayPath), req };
        }
        // Non-port dependency forms pass through unrewritten, exactly
        // as the publisher leaves them.
        return {
            name,
            req: typeof spec === "string" ? spec : "",
        };
    });
}

// The publisher's identity rule (cabin-port-publish::convert::
// scoped_package_name): lowercase the recipe name and require the
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

// The published version: the upstream version verbatim, or
// `<version>+cabin.<n>` when the recipe carries a packaging-revision
// sidecar (mirroring cabin-port-publish::plan).  A revision on top of
// upstream build metadata is rejected, as the publisher rejects it.
export function publishedVersion(
    upstreamVersion: string,
    revision: number | null,
    context: string,
): string {
    if (revision === null) {
        return upstreamVersion;
    }
    if (upstreamVersion.includes("+")) {
        throw new Error(
            `${context} declares a packaging revision, but upstream version "${upstreamVersion}" already carries build metadata.`,
        );
    }
    return `${upstreamVersion}+cabin.${revision}`;
}

// The sidecar must hold an integer >= 1; the unrevised publication is
// revision zero and is spelled by removing the file.  The digits-only
// grammar and the u32 ceiling mirror the publisher's parse
// (cabin-port-publish::plan::read_revision), so a sidecar this loader
// accepts can never be one the tool refuses to publish.
export function parsePackagingRevision(text: string, context: string): number {
    // ASCII-only trim on purpose: String.trim would also strip
    // U+FEFF, which the publisher's Rust trim does not - the explicit
    // shared grammar keeps the two parsers agreeing.
    const trimmed = text
        .replace(/^[\t\n\f\r ]+/, "")
        .replace(/[\t\n\f\r ]+$/, "");
    if (!/^[0-9]+$/.test(trimmed)) {
        throw new Error(`${context} must hold an integer >= 1.`);
    }
    const revision = Number.parseInt(trimmed, 10);
    if (revision > 4294967295) {
        throw new Error(`${context} must hold an integer that fits in u32.`);
    }
    if (revision < 1) {
        throw new Error(
            `${context} must hold an integer >= 1; the unrevised publication is revision zero.`,
        );
    }
    return revision;
}

async function readPackagingRevision(
    recipeDir: string,
): Promise<number | null> {
    const path = join(recipeDir, REVISION_FILENAME);
    let text: string;
    try {
        text = await readFile(path, "utf-8");
    } catch (error) {
        if (isNotFound(error)) {
            return null;
        }
        throw new Error(`Failed to read ${path}: ${errorMessage(error)}`);
    }
    return parsePackagingRevision(text, path);
}

function isNotFound(error: unknown): boolean {
    return (
        typeof error === "object" &&
        error !== null &&
        (error as NodeJS.ErrnoException).code === "ENOENT"
    );
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
