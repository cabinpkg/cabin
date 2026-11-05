// node:test suite for reading the build's catalog input (`npm test`).
import assert from "node:assert/strict";
import { mkdtempSync, readFileSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { test } from "node:test";
import { loadCatalog, parseCatalog } from "./catalog.ts";

const FIXTURE = readFileSync(
    join(process.cwd(), "fixtures", "catalog.json"),
    "utf8",
);

test("the committed fixture parses in the registry's order", () => {
    const packages = parseCatalog(FIXTURE);
    assert.deepEqual(
        packages.map((pack) => ({
            name: pack.name,
            latest: pack.latest.version,
            versions: pack.versions.map((record) => record.version),
        })),
        [
            {
                name: "acme/widget",
                latest: "2.1.0",
                versions: ["3.0.0-rc.1", "2.1.0", "1.0.0"],
            },
            { name: "cabin-ports/zlib", latest: "1.3.1", versions: ["1.3.1"] },
        ],
    );
    assert.deepEqual(packages[1].versions[0].upstream, {
        archiveUrl:
            "https://github.com/madler/zlib/releases/download/v1.3.1/zlib-1.3.1.tar.gz",
        checksum: `sha256:${"9".repeat(64)}`,
    });
    assert.equal(packages[0].versions[0].upstream, null);
});

test("a document outside the catalog contract fails the build", () => {
    const version = { version: "1.0.0", revision: "0123456789abcdef" };
    for (const [label, document] of [
        ["another schema", { schema: 2, packages: [] }],
        [
            "a latest it does not list",
            {
                schema: 1,
                packages: [
                    { name: "a/b", latest: "2.0.0", versions: [version] },
                ],
            },
        ],
        [
            "a non-https upstream, which would render as a link",
            {
                schema: 1,
                packages: [
                    {
                        name: "a/b",
                        latest: "1.0.0",
                        versions: [
                            {
                                ...version,
                                upstream: {
                                    url: "javascript:alert(1)",
                                    checksum: "sha256:0",
                                },
                            },
                        ],
                    },
                ],
            },
        ],
    ] as const) {
        assert.throws(() => parseCatalog(JSON.stringify(document)), label);
    }
});

test("the unavailable state lists no packages and needs no catalog", () => {
    const directory = mkdtempSync(join(tmpdir(), "catalog-"));
    const provenance = { source: "registry", state: "unavailable" };
    writeFileSync(
        join(directory, "provenance.json"),
        JSON.stringify(provenance),
    );
    assert.deepEqual(loadCatalog(directory), { provenance, packages: [] });
});

test("a missing input names the fetch step", () => {
    const directory = mkdtempSync(join(tmpdir(), "catalog-"));
    assert.throws(() => loadCatalog(directory), /npm run fetch:catalog/);
});
