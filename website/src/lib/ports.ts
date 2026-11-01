import { existsSync } from "node:fs";
import { lstat, readdir, readFile, stat } from "node:fs/promises";
import { dirname, join } from "node:path";
import { parse as parseToml } from "smol-toml";
import { extractManifestInfo } from "./manifestInfo.ts";
import type { PackageRecord } from "./types";

// The foundation ports live at the repository root under ports/. The
// nearest ancestor of the current working directory that contains it is
// used, so the build works from website/ or from the repo root.
// import.meta.url is unusable: `astro build` bundles this module into
// dist/.prerender/chunks/ at a different depth than this source file.
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

    for (const portName of await listDirectories(portsDir)) {
        const portDir = join(portsDir, portName);
        for (const version of await listDirectories(portDir)) {
            const manifestPath = join(portDir, version, "cabin.toml");
            // Discovery mirrors `plan.rs::discover_ports`, whose
            // `is_file()` follows links: a version directory whose
            // `cabin.toml` does not resolve to a regular file (absent,
            // dangling, or a directory) is skipped, not diagnosed, so
            // every tree that publishes also builds.
            const resolved = await statOrNull(manifestPath);
            if (resolved === null || !resolved.isFile()) {
                continue;
            }
            // It resolves to a file, so the publisher loads it, and
            // `plan.rs::load_package` refuses a symlink: the manifest
            // carries the package's identity and provenance and must be
            // the committed file itself.
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

// The committed manifest already carries the canonical scoped identity
// and a complete [package.upstream] block, so nothing is converted.
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
    // version it refuses fails the build here instead of rendering a
    // package page nobody can depend on.
    //
    // node-semver's `valid()` does not match Rust: it accepts a leading
    // `v` and surrounding whitespace that Rust rejects, and rejects
    // integers above `Number.MAX_SAFE_INTEGER` that Rust accepts.
    // `BigInt` keeps the u64 range check exact.
    //
    // Dependency and feature declarations get only the structural shape
    // checks in src/lib/manifestInfo.ts that mirror the publisher's
    // rejections; full manifest validity (targets, standards, feature
    // resolution) is the publisher's job, because reimplementing Cabin's
    // manifest parser in TypeScript is the duplication this loader
    // exists to avoid.
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
        manifest: extractManifestInfo(parsed, manifestPath),
        upstream: { version, archiveUrl: parsedUrl.toString(), checksum },
    };
}

// Provenance URLs must be credential-free HTTPS. They are parsed, not
// prefix-matched, so this loader accepts whatever the client's URL
// parser normalizes.
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
// scoped_package_name): the directory is spelled exactly as the
// package name, so a name the registry would reject fails the build
// here instead of publishing a page for a package that cannot exist.
export function scopedPackageName(portName: string, context: string): string {
    if (!/^[a-z0-9][a-z0-9_-]*$/.test(portName)) {
        throw new Error(
            `Port directory "${portName}" in ${context} is not a valid package name ([a-z0-9][a-z0-9_-]*).`,
        );
    }
    return `${REGISTRY_SCOPE}/${portName}`;
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
