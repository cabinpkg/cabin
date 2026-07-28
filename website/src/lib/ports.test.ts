// node:test suite for the foundation-port loader (`npm test`).  The
// committed-tree tests pin that the website presents exactly the
// identities the cabin-port-publish tool would publish; the fixture
// tests cover the packaging-revision sidecar wiring, which no
// committed recipe exercises.
import assert from "node:assert/strict";
import { mkdir, mkdtemp, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { test } from "node:test";
import {
    loadPortsAsPackageRecords,
    loadPortsFromDir,
    parsePackagingRevision,
    publishedVersion,
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
        // No committed recipe carries a packaging-revision sidecar, so
        // every published version is the upstream version verbatim.
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

test("a packaging-revision sidecar appends +cabin.N to the published version", async () => {
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

        await writeFile(join(recipeDir, "packaging-revision"), "2\n");
        const revised = await loadPortsFromDir(dir);
        assert.equal(revised[0].name, "cabin-ports/zlib");
        assert.equal(revised[0].version, "1.3.1+cabin.2");
        assert.equal(revised[0].upstream?.version, "1.3.1");

        await writeFile(join(recipeDir, "packaging-revision"), "0");
        await assert.rejects(() => loadPortsFromDir(dir));
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

test("publishedVersion appends the packaging revision as build metadata", () => {
    assert.equal(publishedVersion("1.3.1", null, "test"), "1.3.1");
    assert.equal(publishedVersion("1.3.1", 2, "test"), "1.3.1+cabin.2");
    assert.throws(() => publishedVersion("1.3.1+build", 1, "test"));
});

test("parsePackagingRevision accepts integers >= 1 and rejects the rest", () => {
    assert.equal(parsePackagingRevision("1", "test"), 1);
    assert.equal(parsePackagingRevision("12\n", "test"), 12);
    assert.throws(() => parsePackagingRevision("0", "test"));
    assert.throws(() => parsePackagingRevision("-1", "test"));
    assert.throws(() => parsePackagingRevision("+1", "test"));
    assert.throws(() => parsePackagingRevision("4294967296", "test"));
    // The explicit ASCII whitespace grammar shared with the publisher:
    // neither JS-only (U+FEFF) nor Rust-only (U+0085) whitespace is
    // trimmed.
    assert.throws(() => parsePackagingRevision("\uFEFF1", "test"));
    assert.throws(() => parsePackagingRevision("\u00851", "test"));
    // Vertical tab is outside Rust's is_ascii_whitespace and outside
    // this trim class: both parsers reject a VT-wrapped revision.
    assert.throws(() => parsePackagingRevision("\u000B1", "test"));
    assert.throws(() => parsePackagingRevision("1.5", "test"));
    assert.throws(() => parsePackagingRevision("one", "test"));
    assert.throws(() => parsePackagingRevision("", "test"));
});
