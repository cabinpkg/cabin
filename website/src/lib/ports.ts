import { existsSync } from "node:fs";
import { lstat, readdir, readFile, stat } from "node:fs/promises";
import { dirname, join } from "node:path";
import { parse as parseToml } from "smol-toml";
import { extractManifestInfo } from "./manifestInfo.ts";
import type { PackageRecord } from "./types";

// The foundation ports live at the repository root under ports/.
// Resolve that directory by walking up from the
// current working directory to the nearest ancestor that contains it, so it
// works whether the build runs from website/ (local `npm run build`, CI) or the
// repo root.  We avoid import.meta.url because `astro build` bundles this
// module into dist/.prerender/chunks/ at a different depth than this source
// file.
const PORTS_SUBPATH = "ports";
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
// xtask-port-publish conversion (crates/xtask-port-publish): the site must
// present exactly the names and versions the registry serves, without a
// live-registry build dependency.
const REGISTRY_SCOPE = "cabin-ports";

// The publisher's core version components are Rust `u64`s, so a
// grammatically valid number above this is still refused there.
const U64_MAX = 18446744073709551615n;

// The official SemVer 2.0.0 grammar (semver.org), which is what the
// publisher's `semver::Version` parser implements.  Capture groups 1-3
// are the core numbers, range-checked separately below.
const SEMVER =
    /^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)(?:-((?:0|[1-9]\d*|\d*[a-zA-Z-][0-9a-zA-Z-]*)(?:\.(?:0|[1-9]\d*|\d*[a-zA-Z-][0-9a-zA-Z-]*))*))?(?:\+([0-9a-zA-Z-]+(?:\.[0-9a-zA-Z-]+)*))?$/;

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
            `Ports directory not found at ${portsDir}. Expected ports/ in the cabin repository.`,
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
            // Discovery mirrors `plan.rs::discover_ports`, whose
            // `is_file()` FOLLOWS links: a version directory whose
            // `cabin.toml` does not resolve to a regular file - absent,
            // dangling, or a directory - is skipped, not diagnosed.
            // Being stricter here would fail the site build on a tree
            // that publishes fine.
            const resolved = await statOrNull(manifestPath);
            if (resolved === null || !resolved.isFile()) {
                continue;
            }
            // It resolves to a file, so the publisher WOULD load it -
            // and `plan.rs::load_package` then refuses a symlink,
            // because the manifest carries the package's identity and
            // provenance and must be the committed file itself.
            const marker = await lstat(manifestPath);
            if (!marker.isFile()) {
                throw new Error(
                    `${manifestPath} is not a regular file; a port's manifest must be committed directly, not through a symlink.`,
                );
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

    // The publisher refuses an empty tree rather than publishing
    // nothing; a site that silently rendered zero package pages would
    // look like a successful build of a broken checkout.
    if (records.length === 0) {
        throw new Error(`No ports found under ${portsDir}.`);
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
            upstream?: { url?: string; checksum?: string };
        };
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
    // The publisher parses this into a typed `semver::Version`, so a
    // string the resolver could never match must fail the build here
    // rather than render a package page nobody can depend on.
    //
    // The SemVer 2.0.0 grammar itself, NOT node-semver's `valid()`,
    // which accepts a leading `v` and surrounding whitespace Rust
    // rejects and rejects integers above `Number.MAX_SAFE_INTEGER`
    // Rust accepts.  The core numbers are then range-checked with
    // `BigInt` - unbounded, so this stays exact rather than trading
    // one precision limit for another - because the publisher parses
    // them as `u64`.
    //
    // Ceiling, deliberate: this is the version only.  Dependency and
    // feature declarations get the same treatment via
    // src/lib/manifestInfo.ts - structural shape checks mirroring the
    // publisher's rejections, never semantics - and full manifest
    // validity (targets, standards, feature resolution) stays the
    // publisher's job: reimplementing Cabin's manifest parser in
    // TypeScript is the duplication this loader exists to avoid.
    const core = SEMVER.exec(version);
    if (core === null) {
        throw new Error(
            `${manifestPath} declares version "${version}", which is not a valid SemVer version.`,
        );
    }
    if (core.slice(1, 4).some((part) => BigInt(part) > U64_MAX)) {
        throw new Error(
            `${manifestPath} declares version "${version}", whose major, minor or patch number exceeds the u64 the publisher parses it as.`,
        );
    }

    const archiveUrl = stringOrNull(parsed.package?.upstream?.url);
    const checksum = stringOrNull(parsed.package?.upstream?.checksum);
    if (archiveUrl === null || checksum === null) {
        throw new Error(
            `Missing [package.upstream] url or checksum in ${manifestPath}.`,
        );
    }
    const parsedUrl = parseProvenanceUrl(archiveUrl, manifestPath);
    if (!/^sha256:[0-9a-f]{64}$/.test(checksum)) {
        throw new Error(
            `[package.upstream].checksum in ${manifestPath} must be \`sha256:\` followed by a lowercase 64-character hex digest.`,
        );
    }

    return {
        name,
        version,
        description: null,
        edition: null,
        license: null,
        metadata: { package: {} },
        manifest: extractManifestInfo(parsed, manifestPath),
        published_at: null,
        readme: null,
        repository: null,
        upstream: { version, archiveUrl: parsedUrl.toString(), checksum },
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

// The publisher's identity rule (xtask-port-publish::plan::
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

// Committed regular directories only, matching the publisher
// (plan.rs::read_sorted_dirs): a symlinked name or version directory
// would source published metadata from outside the committed tree, so
// it fails the build rather than being skipped.  `readdir`'s
// `isDirectory()` is false for a symlink, so a plain filter would drop
// one silently instead of diagnosing it.
async function listDirectories(parent: string): Promise<string[]> {
    const entries = await readdir(parent, { withFileTypes: true });
    const names: string[] = [];
    for (const entry of entries) {
        if (entry.isSymbolicLink()) {
            const target = join(parent, entry.name);
            if (await directoryExists(target)) {
                throw new Error(
                    `${target} is a symlinked directory; ports must be committed as regular directories.`,
                );
            }
            continue;
        }
        if (entry.isDirectory()) {
            names.push(entry.name);
        }
    }
    return names.sort();
}

// `stat` follows symlinks, matching Rust's `Path::is_file()`.
async function statOrNull(path: string) {
    try {
        return await stat(path);
    } catch {
        return null;
    }
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
