// node:test suite for the version navigation list (`npm test`).
import assert from "node:assert/strict";
import { test } from "node:test";
import {
    buildVersionList,
    getPackageArchiveUrl,
    getPackageSourceHref,
    type PackageRecord,
} from "./packages.ts";

function record(version: string): PackageRecord {
    return {
        name: "cabin-ports/zlib",
        version,
        revision: "0a1b2c3d4e5f6a7b",
        manifest: { dependencies: [], features: null },
        upstream: null,
        license: null,
    };
}

test("the version list keeps the registry's order and marks its latest", () => {
    // A newer pre-release precedes the latest release.
    const latest = record("2.0.0");
    const list = buildVersionList({
        name: "cabin-ports/zlib",
        latest,
        versions: [record("2.1.0-rc.1"), latest, record("1.3.1")],
    });
    assert.deepEqual(list, [
        {
            version: "2.1.0-rc.1",
            href: "/packages/cabin-ports/zlib/2.1.0-rc.1",
            isLatest: false,
        },
        {
            version: "2.0.0",
            href: "/packages/cabin-ports/zlib/2.0.0",
            isLatest: true,
        },
        {
            version: "1.3.1",
            href: "/packages/cabin-ports/zlib/1.3.1",
            isLatest: false,
        },
    ]);
});

test("a version's viewer page reads the archive the registry published", () => {
    assert.equal(
        getPackageSourceHref("cabin-ports/zlib", "1.3.1"),
        "/packages/cabin-ports/zlib/1.3.1/source",
    );
    // The registry's key for a revision, on its archive host.
    assert.equal(
        getPackageArchiveUrl(record("2.1.0-rc.1")),
        "https://static.cabinpkg.com/archives/cabin-ports/zlib/cabin-ports-zlib-2.1.0-rc.1-0a1b2c3d4e5f6a7b.zip",
    );
});
