// node:test suite for package version ordering and the version
// navigation list (`npm test`).
import assert from "node:assert/strict";
import { test } from "node:test";
import {
    buildVersionList,
    comparePackageVersions,
    type PackageRecord,
} from "./packages.ts";

function record(version: string): PackageRecord {
    return {
        name: "cabin-ports/zlib",
        version,
        description: null,
        edition: null,
        license: null,
        metadata: { package: {} },
        manifest: { dependencies: [], features: null },
        published_at: null,
        readme: null,
        repository: null,
        upstream: null,
    };
}

test("plain semver ordering picks the latest version", () => {
    assert.ok(comparePackageVersions(record("1.3.2"), record("1.3.1")) < 0);
    assert.ok(comparePackageVersions(record("1.3.1"), record("1.3.2")) > 0);
    assert.ok(
        comparePackageVersions(record("2.0.0"), record("2.0.0-rc.1")) < 0,
    );
});

test("the version list navigates newest first with the latest marked", () => {
    const list = buildVersionList([
        record("1.3.1"),
        record("2.0.0-rc.1"),
        record("2.0.0"),
    ]);
    assert.deepEqual(list, [
        {
            version: "2.0.0",
            href: "/packages/cabin-ports/zlib/2.0.0",
            isLatest: true,
        },
        {
            version: "2.0.0-rc.1",
            href: "/packages/cabin-ports/zlib/2.0.0-rc.1",
            isLatest: false,
        },
        {
            version: "1.3.1",
            href: "/packages/cabin-ports/zlib/1.3.1",
            isLatest: false,
        },
    ]);
});
