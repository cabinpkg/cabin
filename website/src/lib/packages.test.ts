// node:test suite for the version navigation list (`npm test`).
import assert from "node:assert/strict";
import { test } from "node:test";
import { buildVersionList, type PackageRecord } from "./packages.ts";

function record(version: string): PackageRecord {
    return {
        name: "cabin-ports/zlib",
        version,
        manifest: { dependencies: [], features: null },
        upstream: null,
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
