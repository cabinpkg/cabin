// node:test suite for the foundation-port loader (`npm test`).  The
// committed-tree tests pin that the website presents exactly the
// identities the cabin-port-publish tool would publish.
import assert from "node:assert/strict";
import { mkdir, mkdtemp, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { test } from "node:test";
import {
    loadPortsAsPackageRecords,
    loadPortsFromDir,
    scopedPackageName,
} from "./ports.ts";

test("every committed recipe loads as a canonical cabin-ports identity", async () => {
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

test("mixed-case recipe names lowercase like the publisher", async () => {
    const names = (await loadPortsAsPackageRecords()).map(
        (record) => record.name,
    );
    // cJSON and CLI11 are the committed mixed-case recipes; renaming
    // them is a deliberate identity change and should update this
    // test alongside cabin-port-publish.
    assert.ok(names.includes("cabin-ports/cjson"));
    assert.ok(names.includes("cabin-ports/cli11"));
    assert.ok(names.includes("cabin-ports/zlib"));
});

test("overlay port dependencies surface as scoped registry dependencies", async () => {
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

test("recipes publish the upstream version verbatim, sidecars ignored", async () => {
    const dir = await mkdtemp(join(tmpdir(), "cabin-ports-test-"));
    try {
        const recipeDir = join(dir, "zlib", "1.3.1");
        await mkdir(recipeDir, { recursive: true });
        await writeFile(
            join(recipeDir, "port.toml"),
            `[port]
name = "zlib"
version = "1.3.1"

[source]
type = "archive"
url = "HTTPS://example.com/zlib-1.3.1.tar.gz"
sha256 = "${"a".repeat(64)}"

[overlay]
manifest = "cabin.toml"
`,
        );
        await writeFile(join(recipeDir, "cabin.toml"), "");

        const unrevised = await loadPortsFromDir(dir);
        assert.equal(unrevised.length, 1);
        assert.equal(unrevised[0].version, "1.3.1");
        // The archive URL is surfaced in the publisher's normalized
        // spelling (the uppercase scheme the recipe used lowercases).
        assert.equal(
            unrevised[0].upstream?.archiveUrl,
            "https://example.com/zlib-1.3.1.tar.gz",
        );

        // A stray sidecar from the removed mechanism is just an
        // unknown file: the published version stays the upstream one.
        await writeFile(join(recipeDir, "packaging-revision"), "2\n");
        const withStray = await loadPortsFromDir(dir);
        assert.equal(withStray[0].name, "cabin-ports/zlib");
        assert.equal(withStray[0].version, "1.3.1");
    } finally {
        await rm(dir, { recursive: true, force: true });
    }
});

test("a migrated package directory loads from its committed manifest", async () => {
    const dir = await mkdtemp(join(tmpdir(), "cabin-ports-test-"));
    try {
        // A recipe and a migrated package coexist in one tree while
        // the collapse lands, so the loader must read both shapes.
        const recipeDir = join(dir, "zlib", "1.3.1");
        await mkdir(recipeDir, { recursive: true });
        await writeFile(
            join(recipeDir, "port.toml"),
            `[port]
name = "zlib"
version = "1.3.1"

[source]
type = "archive"
url = "https://example.com/zlib-1.3.1.tar.gz"
sha256 = "${"a".repeat(64)}"

[overlay]
manifest = "cabin.toml"
`,
        );
        await writeFile(join(recipeDir, "cabin.toml"), "");

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

        // An auxiliary directory with neither marker is skipped, as
        // the publisher and the builtin embed skip it.
        await mkdir(join(dir, "fmt", "scripts"), { recursive: true });

        const records = await loadPortsFromDir(dir);
        assert.equal(records.length, 2);
        const fmt = records.find((r) => r.name === "cabin-ports/fmt");
        assert.ok(fmt, "no fmt record");
        assert.equal(fmt.version, "12.2.0");
        // Normalized like the recipe path, and carrying the manifest's
        // own scoped dependency.
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

test("a port spanning both committed shapes fails the build", async () => {
    const dir = await mkdtemp(join(tmpdir(), "cabin-ports-test-"));
    try {
        const recipeDir = join(dir, "fmt", "12.2.0");
        await mkdir(recipeDir, { recursive: true });
        await writeFile(
            join(recipeDir, "port.toml"),
            `[port]
name = "fmt"
version = "12.2.0"

[source]
type = "archive"
url = "https://example.com/f.tar.gz"
sha256 = "${"a".repeat(64)}"

[overlay]
manifest = "cabin.toml"
`,
        );
        await writeFile(join(recipeDir, "cabin.toml"), "");
        const packageDir = join(dir, "fmt", "12.3.0");
        await mkdir(packageDir, { recursive: true });
        await writeFile(
            join(packageDir, "cabin.toml"),
            `[package]
name = "cabin-ports/fmt"
version = "12.3.0"

[package.upstream]
url = "https://example.com/f.tar.gz"
sha256 = "${"b".repeat(64)}"
format = "tar.gz"
`,
        );
        await assert.rejects(
            () => loadPortsFromDir(dir),
            /committed both as a recipe and as a package/,
        );
    } finally {
        await rm(dir, { recursive: true, force: true });
    }
});

test("a migrated package whose identity disagrees with its directory fails", async () => {
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
