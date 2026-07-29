// node:test suite for package version ordering (`npm test`).
import assert from "node:assert/strict";
import { test } from "node:test";
import { comparePackageVersions, type PackageRecord } from "./packages.ts";

function record(version: string): PackageRecord {
    return {
        name: "cabin-ports/zlib",
        version,
        description: null,
        edition: null,
        license: null,
        metadata: { dependencies: [] },
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
