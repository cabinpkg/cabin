// node:test suite for package version ordering (`npm test`).
import assert from "node:assert/strict";
import { test } from "node:test";
import {
    compareBuildMetadata,
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
        metadata: { dependencies: [] },
        published_at: null,
        readme: null,
        repository: null,
        upstream: null,
    };
}

test("packaging revisions order numerically, not lexicographically", () => {
    // Resolvers prefer the highest packaging revision of an otherwise
    // equal version; +cabin.10 must outrank +cabin.2.
    assert.ok(
        comparePackageVersions(
            record("1.3.1+cabin.10"),
            record("1.3.1+cabin.2"),
        ) < 0,
    );
    assert.ok(
        comparePackageVersions(
            record("1.3.1+cabin.2"),
            record("1.3.1+cabin.10"),
        ) > 0,
    );
    assert.ok(
        comparePackageVersions(record("1.3.1+cabin.1"), record("1.3.1")) < 0,
    );
    assert.ok(
        comparePackageVersions(record("1.3.2"), record("1.3.1+cabin.9")) < 0,
    );
});

test("compareBuildMetadata follows semver::BuildMetadata ordering", () => {
    assert.ok(compareBuildMetadata(["cabin", "2"], ["cabin", "10"]) < 0);
    assert.ok(compareBuildMetadata(["cabin", "10"], ["cabin", "2"]) > 0);
    assert.equal(compareBuildMetadata(["cabin", "2"], ["cabin", "2"]), 0);
    assert.ok(compareBuildMetadata([], ["cabin", "1"]) < 0);
    assert.ok(compareBuildMetadata(["2"], ["alpha"]) < 0);
    // Rust tiebreakers the naive numeric/locale comparisons miss:
    // equal values with more leading zeros rank higher, and
    // alphanumeric identifiers compare bytewise ("Z" < "a").
    assert.ok(compareBuildMetadata(["01"], ["1"]) > 0);
    assert.ok(compareBuildMetadata(["1"], ["01"]) < 0);
    assert.ok(compareBuildMetadata(["Z"], ["a"]) < 0);
    assert.ok(compareBuildMetadata(["a"], ["Z"]) > 0);
});

test("latest-version selection matches the Rust resolver on exotic build metadata", () => {
    assert.ok(
        comparePackageVersions(record("1.0.0+01"), record("1.0.0+1")) < 0,
    );
    assert.ok(comparePackageVersions(record("1.0.0+a"), record("1.0.0+Z")) < 0);
});
