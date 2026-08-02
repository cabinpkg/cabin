// node:test suite for the foundation-port loader (`npm test`).  The
// committed-tree tests pin that the website presents exactly the
// identities the cabin-port-publish tool would publish.
import assert from "node:assert/strict";
import { mkdir, mkdtemp, rm, symlink, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { test } from "node:test";
import {
    loadPortsAsPackageRecords,
    loadPortsFromDir,
    scopedPackageName,
} from "./ports.ts";

function packageManifest(name = "cabin-ports/fmt", version = "12.2.0"): string {
    return `[package]
name = "${name}"
version = "${version}"

[package.upstream]
url = "https://example.com/fmt-${version}.tar.gz"
sha256 = "${"b".repeat(64)}"
format = "tar.gz"
`;
}

test("every committed port loads as a canonical cabin-ports identity", async () => {
    const records = await loadPortsAsPackageRecords();
    assert.ok(records.length > 0);
    const identities = new Set(
        records.map((record) => `${record.name}@${record.version}`),
    );
    assert.equal(identities.size, records.length);
    for (const record of records) {
        assert.match(record.name, /^cabin-ports\/[a-z0-9][a-z0-9_-]*$/);
        assert.ok(record.upstream, `${record.name} has no upstream provenance`);
        // The published version is the upstream version, verbatim:
        // packaging corrections are registry revisions, never a
        // version-string suffix.
        assert.equal(record.version, record.upstream.version);
        assert.ok(record.upstream.archiveUrl.startsWith("https://"));
        assert.match(record.upstream.sha256, /^[0-9a-f]{64}$/);
    }
});

test("mixed-case port directories lowercase like the publisher", async () => {
    const names = (await loadPortsAsPackageRecords()).map(
        (record) => record.name,
    );
    // cJSON and CLI11 are the committed mixed-case directories, and
    // both must fold onto the lowercase scoped name; renaming them is
    // a deliberate identity change and should update this test
    // alongside cabin-port-publish.
    assert.ok(names.includes("cabin-ports/cjson"));
    assert.ok(names.includes("cabin-ports/cli11"));
    assert.ok(names.includes("cabin-ports/zlib"));
});

test("a committed port's scoped dependencies reach the package page", async () => {
    const records = await loadPortsAsPackageRecords();
    const libpng = records.find((record) =>
        record.name.startsWith("cabin-ports/libpng"),
    );
    assert.ok(libpng, "no libpng record");
    const metadata = libpng.metadata as {
        dependencies: Array<{ name: string; req: string }>;
    };
    assert.deepEqual(metadata.dependencies, [
        { name: "cabin-ports/zlib", req: "^1.3" },
    ]);
});

test("a port's registry dependencies read in both spellings", async () => {
    // The manifest publishes verbatim, so the page must read exactly
    // what will publish: the bare requirement string and the table
    // with a `version` key.
    const dir = await mkdtemp(join(tmpdir(), "cabin-ports-test-"));
    try {
        const portDir = join(dir, "apng", "1.0.0");
        await mkdir(portDir, { recursive: true });
        await writeFile(
            join(portDir, "cabin.toml"),
            `[package]
name = "cabin-ports/apng"
version = "1.0.0"

[package.upstream]
url = "https://example.com/apng-1.0.0.tar.gz"
sha256 = "${"a".repeat(64)}"
format = "tar.gz"

[dependencies]
"cabin-ports/zlib" = "^1.3"
"cabin-ports/fmt" = { version = "^12", default-features = false }
`,
        );

        const records = await loadPortsFromDir(dir);
        assert.equal(records.length, 1);
        const metadata = records[0].metadata as {
            dependencies: Array<{ name: string; req: string }>;
        };
        assert.deepEqual(metadata.dependencies, [
            { name: "cabin-ports/zlib", req: "^1.3" },
            { name: "cabin-ports/fmt", req: "^12" },
        ]);
    } finally {
        await rm(dir, { recursive: true, force: true });
    }
});

test("a blank dependency requirement is refused", async () => {
    // Cabin's manifest parser refuses an empty requirement, so a page
    // must never render one: the publisher would reject the manifest
    // the page claims to describe.
    const dir = await mkdtemp(join(tmpdir(), "cabin-ports-test-"));
    try {
        const portDir = join(dir, "apng", "1.0.0");
        await mkdir(portDir, { recursive: true });
        await writeFile(
            join(portDir, "cabin.toml"),
            `[package]
name = "cabin-ports/apng"
version = "1.0.0"

[package.upstream]
url = "https://example.com/apng-1.0.0.tar.gz"
sha256 = "${"a".repeat(64)}"
format = "tar.gz"

[dependencies]
"cabin-ports/zlib" = { version = "" }
`,
        );
        await assert.rejects(
            () => loadPortsFromDir(dir),
            /declares no version requirement/,
        );
    } finally {
        await rm(dir, { recursive: true, force: true });
    }
});

test("a port publishes the upstream version verbatim, sidecars ignored", async () => {
    const dir = await mkdtemp(join(tmpdir(), "cabin-ports-test-"));
    try {
        const portDir = join(dir, "zlib", "1.3.1");
        await mkdir(portDir, { recursive: true });
        await writeFile(
            join(portDir, "cabin.toml"),
            `[package]
name = "cabin-ports/zlib"
version = "1.3.1"

[package.upstream]
url = "https://example.com/zlib-1.3.1.tar.gz"
sha256 = "${"a".repeat(64)}"
format = "tar.gz"
`,
        );

        const unrevised = await loadPortsFromDir(dir);
        assert.equal(unrevised.length, 1);
        assert.equal(unrevised[0].version, "1.3.1");

        // A stray sidecar from the removed packaging-revision
        // mechanism is just an unknown file: the published version
        // stays the upstream one.
        await writeFile(join(portDir, "packaging-revision"), "2\n");
        const withStray = await loadPortsFromDir(dir);
        assert.equal(withStray[0].name, "cabin-ports/zlib");
        assert.equal(withStray[0].version, "1.3.1");
    } finally {
        await rm(dir, { recursive: true, force: true });
    }
});

test("a port directory loads from its committed manifest", async () => {
    const dir = await mkdtemp(join(tmpdir(), "cabin-ports-test-"));
    try {
        const zlibDir = join(dir, "zlib", "1.3.1");
        await mkdir(zlibDir, { recursive: true });
        await writeFile(
            join(zlibDir, "cabin.toml"),
            `[package]
name = "cabin-ports/zlib"
version = "1.3.1"

[package.upstream]
url = "https://example.com/zlib-1.3.1.tar.gz"
sha256 = "${"a".repeat(64)}"
format = "tar.gz"
`,
        );

        const packageDir = join(dir, "fmt", "12.2.0");
        await mkdir(packageDir, { recursive: true });
        await writeFile(
            join(packageDir, "cabin.toml"),
            `[package]
name = "cabin-ports/fmt"
version = "12.2.0"

[package.upstream]
url = "HTTPS://example.com/fmt-12.2.0.tar.gz"
sha256 = "${"b".repeat(64)}"
format = "tar.gz"

[dependencies]
"cabin-ports/zlib" = "^1.3"
`,
        );

        // An auxiliary directory with no manifest is skipped, as the
        // publisher skips it.
        await mkdir(join(dir, "fmt", "scripts"), { recursive: true });

        const records = await loadPortsFromDir(dir);
        assert.equal(records.length, 2);
        const fmt = records.find((r) => r.name === "cabin-ports/fmt");
        assert.ok(fmt, "no fmt record");
        assert.equal(fmt.version, "12.2.0");
        // The archive URL is surfaced in the publisher's normalized
        // spelling (the uppercase scheme the manifest used lowercases),
        // and the manifest's own scoped dependency rides through.
        assert.equal(
            fmt.upstream?.archiveUrl,
            "https://example.com/fmt-12.2.0.tar.gz",
        );
        assert.deepEqual(
            (
                fmt.metadata as {
                    dependencies: Array<{ name: string; req: string }>;
                }
            ).dependencies,
            [{ name: "cabin-ports/zlib", req: "^1.3" }],
        );
        // The manifest carries no display metadata; those fields stay
        // null so their UI sections hide.
        assert.equal(fmt.description, null);
        assert.equal(fmt.license, null);
        assert.ok(records.some((r) => r.name === "cabin-ports/zlib"));
    } finally {
        await rm(dir, { recursive: true, force: true });
    }
});

test("a port whose identity disagrees with its directory fails", async () => {
    const dir = await mkdtemp(join(tmpdir(), "cabin-ports-test-"));
    try {
        const packageDir = join(dir, "fmt", "12.2.0");
        await mkdir(packageDir, { recursive: true });
        await writeFile(
            join(packageDir, "cabin.toml"),
            `[package]
name = "cabin-ports/notfmt"
version = "12.2.0"

[package.upstream]
url = "https://example.com/fmt-12.2.0.tar.gz"
sha256 = "${"b".repeat(64)}"
format = "tar.gz"
`,
        );
        await assert.rejects(
            () => loadPortsFromDir(dir),
            /disagrees with its directory identity/,
        );
    } finally {
        await rm(dir, { recursive: true, force: true });
    }
});

test("a symlinked port directory fails the build", async () => {
    // The publisher refuses one (plan.rs::read_sorted_dirs) because a
    // link sources published metadata from outside the committed tree.
    const dir = await mkdtemp(join(tmpdir(), "cabin-ports-test-"));
    try {
        const outside = join(dir, "outside", "fmt", "12.2.0");
        await mkdir(outside, { recursive: true });
        await writeFile(join(outside, "cabin.toml"), packageManifest());
        const ports = join(dir, "ports");
        await mkdir(ports, { recursive: true });
        await symlink(join(dir, "outside", "fmt"), join(ports, "fmt"));
        await assert.rejects(
            () => loadPortsFromDir(ports),
            /is a symlinked directory/,
        );
    } finally {
        await rm(dir, { recursive: true, force: true });
    }
});

test("a symlinked manifest fails the build", async () => {
    // Following it would let published metadata come from outside the
    // version directory; the publisher refuses it too.
    const dir = await mkdtemp(join(tmpdir(), "cabin-ports-test-"));
    try {
        await writeFile(join(dir, "outside.toml"), packageManifest());
        const portDir = join(dir, "ports", "fmt", "12.2.0");
        await mkdir(portDir, { recursive: true });
        await symlink(join(dir, "outside.toml"), join(portDir, "cabin.toml"));
        await assert.rejects(
            () => loadPortsFromDir(join(dir, "ports")),
            /is not a regular file/,
        );
    } finally {
        await rm(dir, { recursive: true, force: true });
    }
});

test("a dangling or directory cabin.toml symlink is skipped, not diagnosed", async () => {
    // `plan.rs::discover_ports` uses `is_file()`, which FOLLOWS links:
    // neither of these resolves to a regular file, so the publisher
    // skips the version directory. Throwing here would fail the site
    // build on a tree that publishes fine.
    const dir = await mkdtemp(join(tmpdir(), "cabin-ports-test-"));
    try {
        const dangling = join(dir, "fmt", "12.2.0");
        await mkdir(dangling, { recursive: true });
        await symlink(join(dir, "nowhere.toml"), join(dangling, "cabin.toml"));

        const toDir = join(dir, "fmt", "12.3.0");
        await mkdir(join(toDir, "target"), { recursive: true });
        await symlink(join(toDir, "target"), join(toDir, "cabin.toml"));

        const good = join(dir, "zlib", "1.3.1");
        await mkdir(good, { recursive: true });
        await writeFile(
            join(good, "cabin.toml"),
            packageManifest("cabin-ports/zlib", "1.3.1"),
        );

        const records = await loadPortsFromDir(dir);
        assert.deepEqual(
            records.map((r) => r.name),
            ["cabin-ports/zlib"],
        );
    } finally {
        await rm(dir, { recursive: true, force: true });
    }
});

test("a version above MAX_SAFE_INTEGER but within u64 still loads", async () => {
    // node-semver rejects integers above Number.MAX_SAFE_INTEGER, which
    // Rust's u64-backed parser accepts; the loader must not be the
    // stricter of the two.
    const dir = await mkdtemp(join(tmpdir(), "cabin-ports-test-"));
    try {
        const version = "9007199254740993.0.0";
        const portDir = join(dir, "fmt", version);
        await mkdir(portDir, { recursive: true });
        await writeFile(
            join(portDir, "cabin.toml"),
            packageManifest("cabin-ports/fmt", version),
        );
        const records = await loadPortsFromDir(dir);
        assert.equal(records[0].version, version);
    } finally {
        await rm(dir, { recursive: true, force: true });
    }
});

test("a core version above u64::MAX fails the build", async () => {
    // Grammatically valid SemVer, but the publisher parses the core
    // numbers as u64 and refuses it - so a page for a version that can
    // never publish must not render.
    const dir = await mkdtemp(join(tmpdir(), "cabin-ports-test-"));
    try {
        const version = "18446744073709551616.0.0";
        const portDir = join(dir, "fmt", version);
        await mkdir(portDir, { recursive: true });
        await writeFile(
            join(portDir, "cabin.toml"),
            packageManifest("cabin-ports/fmt", version),
        );
        await assert.rejects(() => loadPortsFromDir(dir), /exceeds the u64/);
    } finally {
        await rm(dir, { recursive: true, force: true });
    }
});

test("the exact u64::MAX core version still loads", async () => {
    // The boundary itself is valid for the publisher, so the loader
    // must not be off by one.
    const dir = await mkdtemp(join(tmpdir(), "cabin-ports-test-"));
    try {
        const version = "18446744073709551615.0.0";
        const portDir = join(dir, "fmt", version);
        await mkdir(portDir, { recursive: true });
        await writeFile(
            join(portDir, "cabin.toml"),
            packageManifest("cabin-ports/fmt", version),
        );
        const records = await loadPortsFromDir(dir);
        assert.equal(records[0].version, version);
    } finally {
        await rm(dir, { recursive: true, force: true });
    }
});

test("a leading-v version fails the build", async () => {
    // node-semver's valid() accepts "v1.2.3"; Rust's parser does not.
    const dir = await mkdtemp(join(tmpdir(), "cabin-ports-test-"));
    try {
        const portDir = join(dir, "fmt", "v1.2.3");
        await mkdir(portDir, { recursive: true });
        await writeFile(
            join(portDir, "cabin.toml"),
            packageManifest("cabin-ports/fmt", "v1.2.3"),
        );
        await assert.rejects(
            () => loadPortsFromDir(dir),
            /is not a valid SemVer version/,
        );
    } finally {
        await rm(dir, { recursive: true, force: true });
    }
});

test("an empty ports tree fails the build", async () => {
    // The publisher refuses it rather than publishing nothing; a site
    // rendering zero package pages would look like a clean build of a
    // broken checkout.
    const dir = await mkdtemp(join(tmpdir(), "cabin-ports-test-"));
    try {
        await mkdir(join(dir, "ports"), { recursive: true });
        await assert.rejects(
            () => loadPortsFromDir(join(dir, "ports")),
            /No ports found under/,
        );
    } finally {
        await rm(dir, { recursive: true, force: true });
    }
});

test("a version that is not valid SemVer fails the build", async () => {
    // The publisher parses this into a typed semver::Version, so a
    // string the resolver could never match must not render a page.
    const dir = await mkdtemp(join(tmpdir(), "cabin-ports-test-"));
    try {
        const portDir = join(dir, "fmt", "12.2");
        await mkdir(portDir, { recursive: true });
        await writeFile(
            join(portDir, "cabin.toml"),
            packageManifest("cabin-ports/fmt", "12.2"),
        );
        await assert.rejects(
            () => loadPortsFromDir(dir),
            /is not a valid SemVer version/,
        );
    } finally {
        await rm(dir, { recursive: true, force: true });
    }
});

test("scopedPackageName rejects names outside the registry grammar", () => {
    assert.equal(scopedPackageName("cJSON", "test"), "cabin-ports/cjson");
    assert.equal(
        scopedPackageName("nlohmann_json", "test"),
        "cabin-ports/nlohmann_json",
    );
    assert.throws(() => scopedPackageName("-leading-dash", "test"));
    assert.throws(() => scopedPackageName("dotted.name", "test"));
    assert.throws(() => scopedPackageName("", "test"));
});
