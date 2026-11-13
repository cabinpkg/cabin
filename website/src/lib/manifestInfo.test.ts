// node:test suite for mapping a catalog version's index-entry blocks
// (crates/cabin-index/src/loader.rs `RawVersion`) for rendering
// (`npm test`).
import assert from "node:assert/strict";
import { test } from "node:test";
import { groupDependencies, versionManifestInfo } from "./manifestInfo.ts";

const CONTEXT = "acme/widget@1.0.0";

test("every dependency kind, condition, and flag maps", () => {
    const info = versionManifestInfo(
        {
            dependencies: {
                "cabin-ports/zlib": "^1.3",
                "cabin-ports/fmt": {
                    version: "^12",
                    optional: true,
                    "default-features": false,
                    features: ["simd"],
                    target: 'os = "linux"',
                },
            },
            "dev-dependencies": { "cabin-ports/catch2": { version: "^3" } },
        },
        CONTEXT,
    );
    const plain = {
        condition: null,
        optional: false,
        features: [],
        defaultFeatures: true,
    };
    assert.deepEqual(info.dependencies, [
        {
            ...plain,
            name: "cabin-ports/zlib",
            kind: "normal",
            req: "^1.3",
        },
        {
            name: "cabin-ports/fmt",
            kind: "normal",
            req: "^12",
            condition: 'os = "linux"',
            optional: true,
            features: ["simd"],
            defaultFeatures: false,
        },
        {
            ...plain,
            name: "cabin-ports/catch2",
            kind: "dev",
            req: "^3",
        },
    ]);
    // No features block: the section is hidden rather than claiming none.
    assert.equal(info.features, null);
});

// The stored `features` block is cabin-core `Features`
// (docs/package-format.md): `default` plus the nested `features` map.
test("feature entries classify dep:, the last slash, and local names", () => {
    const info = versionManifestInfo(
        {
            features: {
                default: ["fast"],
                features: {
                    fast: ["cabin-ports/zlib/simd", "extra"],
                    fmt: ["dep:cabin-ports/fmt"],
                    extra: [],
                },
            },
        },
        CONTEXT,
    );
    assert.deepEqual(info.features, {
        default: ["fast"],
        entries: [
            {
                name: "fast",
                enables: [
                    {
                        kind: "dependency-feature",
                        dependency: "cabin-ports/zlib",
                        feature: "simd",
                    },
                    { kind: "feature", feature: "extra" },
                ],
            },
            {
                name: "fmt",
                enables: [
                    {
                        kind: "optional-dependency",
                        dependency: "cabin-ports/fmt",
                    },
                ],
            },
            { name: "extra", enables: [] },
        ],
    });
    // Publish omits an empty `default`.
    assert.deepEqual(
        versionManifestInfo({ features: { features: { simd: [] } } }, CONTEXT)
            .features,
        { default: [], entries: [{ name: "simd", enables: [] }] },
    );
});

test("a block outside the index shape fails the build", () => {
    for (const [label, entry] of [
        ["a non-table dependencies block", { dependencies: [] }],
        ["a dependency without a version", { dependencies: { a: {} } }],
        [
            "a non-boolean optional flag",
            { dependencies: { a: { version: "^1", optional: "yes" } } },
        ],
        ["a non-list feature", { features: { features: { x: "y" } } }],
        // The manifest's flat `[features]` table is not the stored shape.
        ["a flat features table", { features: { default: [], x: [] } }],
    ] as const) {
        assert.throws(
            () => versionManifestInfo(entry, CONTEXT),
            new RegExp(CONTEXT),
            label,
        );
    }
});

test("grouping renders unconditional first, then conditions lexically", () => {
    const info = versionManifestInfo(
        {
            dependencies: {
                "cabin-ports/zlib": {
                    version: "^1.3",
                    target: 'family = "unix"',
                },
                "cabin-ports/fmt": "^12",
                "cabin-ports/systemd": {
                    version: ">=240",
                    target: 'os = "linux"',
                },
            },
        },
        CONTEXT,
    );
    assert.deepEqual(
        groupDependencies(info.dependencies).map((group) => ({
            condition: group.condition,
            names: group.entries.map((entry) => entry.name),
        })),
        [
            { condition: null, names: ["cabin-ports/fmt"] },
            { condition: 'family = "unix"', names: ["cabin-ports/zlib"] },
            { condition: 'os = "linux"', names: ["cabin-ports/systemd"] },
        ],
    );
});
