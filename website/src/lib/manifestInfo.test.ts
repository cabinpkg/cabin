// node:test suite for the normalized manifest info (`npm test`).
// Extraction mirrors the registry index encoding
// (crates/cabin-index/src/loader.rs) and the manifest parser's
// structural rules (crates/cabin-manifest/src/parse/dependency.rs);
// the fixtures pin both directions - what publishes must extract, and
// what the publisher rejects must fail.
import assert from "node:assert/strict";
import { test } from "node:test";
import { parse as parseToml } from "smol-toml";
import { extractManifestInfo, groupDependencies } from "./manifestInfo.ts";

const CONTEXT = "test manifest";

function extract(toml: string) {
    return extractManifestInfo(parseToml(toml), CONTEXT);
}

test("every dependency kind, source, condition, and flag extracts", () => {
    const info = extract(`
[dependencies]
"cabin-ports/zlib" = "^1.3"
"cabin-ports/fmt" = { version = "^12", optional = true, default-features = false, features = ["simd"], ignore-interface-standard = true }
openssl = { version = ">=3", system = true }

[dev-dependencies]
"cabin-ports/catch2" = { version = "^3" }

[target.'cfg(os = "linux")'.dependencies]
systemd = { version = ">=240", system = true }

[target.'cfg(family = "unix")'.dev-dependencies]
"cabin-ports/googletest" = "^1.17"
`);
    assert.deepEqual(info.dependencies, [
        {
            name: "cabin-ports/zlib",
            kind: "normal",
            source: "registry",
            req: "^1.3",
            condition: null,
            optional: false,
            features: [],
            defaultFeatures: true,
            ignoreInterfaceStandard: false,
        },
        {
            name: "cabin-ports/fmt",
            kind: "normal",
            source: "registry",
            req: "^12",
            condition: null,
            optional: true,
            features: ["simd"],
            defaultFeatures: false,
            ignoreInterfaceStandard: true,
        },
        {
            name: "openssl",
            kind: "normal",
            source: "system",
            req: ">=3",
            condition: null,
            optional: false,
            features: [],
            defaultFeatures: true,
            ignoreInterfaceStandard: false,
        },
        {
            name: "cabin-ports/catch2",
            kind: "dev",
            source: "registry",
            req: "^3",
            condition: null,
            optional: false,
            features: [],
            defaultFeatures: true,
            ignoreInterfaceStandard: false,
        },
        {
            name: "cabin-ports/googletest",
            kind: "dev",
            source: "registry",
            req: "^1.17",
            condition: 'family = "unix"',
            optional: false,
            features: [],
            defaultFeatures: true,
            ignoreInterfaceStandard: false,
        },
        {
            name: "systemd",
            kind: "normal",
            source: "system",
            req: ">=240",
            condition: 'os = "linux"',
            optional: false,
            features: [],
            defaultFeatures: true,
            ignoreInterfaceStandard: false,
        },
    ]);
});

test("named build targets are not dependency tables", () => {
    const info = extract(`
[target.fmt]
type = "library"
sources = ["src/format.cc"]
`);
    assert.deepEqual(info.dependencies, []);
});

test("a padded cfg key still parses as a condition, trimmed", () => {
    // The manifest parser trims both the key and the inner
    // expression; without the trim these dependencies would silently
    // vanish from the page.
    const info = extract(`
[target.' cfg( os = "linux" ) '.dependencies]
"cabin-ports/zlib" = "^1.3"
`);
    assert.equal(info.dependencies.length, 1);
    assert.equal(info.dependencies[0].condition, 'os = "linux"');
});

test("dependencies under a named target fail loudly, like the parser", () => {
    // TargetSpecificDependenciesNotSupported: almost always a typo of
    // the cfg form, and the one case the manifest parser refuses to
    // swallow - so the site must not render "no dependencies" for it.
    assert.throws(
        () =>
            extract(`
[target.png.dependencies]
"cabin-ports/zlib" = "^1.3"
`),
        /is not supported/,
    );
});

test("workspace = false beside a version is a plain registry dep", () => {
    // ordinary_dep_from_table routes (no path, version, workspace
    // != true) to the version source, so this legal spelling must
    // extract rather than fail the build.
    const info = extract(`
[dependencies]
"cabin-ports/zlib" = { version = "^1.3", workspace = false }
`);
    assert.equal(info.dependencies.length, 1);
    assert.equal(info.dependencies[0].source, "registry");
    assert.equal(info.dependencies[0].req, "^1.3");
});

test("a blank system requirement is legal and unconstrained", () => {
    // cabin-system-deps probes an empty/whitespace requirement as
    // "any version"; throwing here would fail the build on a
    // manifest Cabin publishes.
    const info = extract(`
[dependencies]
openssl = { version = "", system = true }
`);
    assert.equal(info.dependencies.length, 1);
    assert.equal(info.dependencies[0].source, "system");
    assert.equal(info.dependencies[0].req, "");
});

test("cfg toolchain and profile tables pass without dependencies", () => {
    // The other two RawConditionalTargetTable fields are real
    // manifest surface, not unknown keys.
    const info = extract(`
[target.'cfg(os = "linux")'.profile]
cxxflags = ["-O2"]

[target.'cfg(os = "linux")'.toolchain]
cxx = "clang++"
`);
    assert.deepEqual(info.dependencies, []);
});

test("feature entries classify by the cabin-core grammar", () => {
    const info = extract(`
[features]
default = ["simd"]
simd = []
full = ["simd", "ssl"]
ssl = ["dep:openssl", "dep:fmtlib/fmt"]
forward = ["cabin-ports/zlib/simd", "fmt/json"]
`);
    assert.ok(info.features);
    assert.deepEqual(info.features.default, ["simd"]);
    assert.deepEqual(info.features.entries, [
        { name: "simd", enables: [] },
        {
            name: "full",
            enables: [
                { kind: "feature", feature: "simd" },
                { kind: "feature", feature: "ssl" },
            ],
        },
        {
            name: "ssl",
            enables: [
                { kind: "optional-dependency", dependency: "openssl" },
                // `dep:` names may be scoped; the prefix wins before
                // any slash logic.
                { kind: "optional-dependency", dependency: "fmtlib/fmt" },
            ],
        },
        {
            name: "forward",
            enables: [
                // The LAST slash splits dependency from feature, so a
                // scoped dependency's forwarding entry keeps its scope.
                {
                    kind: "dependency-feature",
                    dependency: "cabin-ports/zlib",
                    feature: "simd",
                },
                {
                    kind: "dependency-feature",
                    dependency: "fmt",
                    feature: "json",
                },
            ],
        },
    ]);
});

test("absent tables extract as empty or null, never invented", () => {
    const info = extract(`
[package]
name = "cabin-ports/fmt"
version = "12.2.0"
`);
    assert.deepEqual(info.dependencies, []);
    // No [features] table is null - the page hides the section rather
    // than claiming "no features" about metadata predating the field.
    assert.equal(info.features, null);
});

test("a declared-but-empty features table stays distinguishable", () => {
    const info = extract(`
[features]
default = []
`);
    assert.deepEqual(info.features, { default: [], entries: [] });
});

test("shapes the publisher rejects fail the build", () => {
    const cases: Array<[string, string]> = [
        [
            "a table without a version",
            "[dependencies]\nfmt = { optional = true }",
        ],
        ["a blank requirement", '[dependencies]\nfmt = "  "'],
        [
            "a system dependency without a version",
            "[dependencies]\nopenssl = { system = true }",
        ],
        [
            "a non-boolean system flag",
            '[dependencies]\nopenssl = { version = ">=3", system = "yes" }',
        ],
        ["a non-table dependencies section", 'dependencies = "oops"'],
        // smol-toml parses a datetime into a Date object, which must
        // not read as an empty table.
        [
            "a datetime dependencies section",
            "dependencies = 1979-05-27T07:32:00Z",
        ],
        ["a datetime features section", "features = 1979-05-27T07:32:00Z"],
        ["a non-table dev-dependencies section", 'dev-dependencies = "oops"'],
        ["a non-table target section", 'target = "oops"'],
        [
            "a non-table cfg target entry",
            '[target]\n\'cfg(os = "linux")\' = "oops"',
        ],
        [
            "an unknown cfg target field",
            '[target.\'cfg(os = "linux")\'.system-dependencies]\nopenssl = { version = ">=3" }',
        ],
        ["a path-traversal dependency name", '[dependencies]\n"../evil" = "1"'],
        ["a dependency name with two slashes", '[dependencies]\n"a/b/c" = "1"'],
        [
            "a dependency name with an uppercase scope",
            '[dependencies]\n"Scope/zlib" = "1"',
        ],
        ["a hidden-file dependency name", '[dependencies]\n".hidden" = "1"'],
        [
            "a default entry outside the feature-name grammar",
            '[features]\ndefault = ["bad name"]',
        ],
        [
            "a dep: entry in the default list",
            '[features]\ndefault = ["dep:ssl"]',
        ],
        [
            "features alongside system = true",
            '[dependencies]\nopenssl = { version = ">=3", system = true, features = ["x"] }',
        ],
        [
            "optional alongside system = true",
            '[dependencies]\nopenssl = { version = ">=3", system = true, optional = true }',
        ],
        [
            "a non-boolean optional",
            '[dependencies]\nfmt = { version = "^12", optional = "yes" }',
        ],
        [
            "a non-string-list features",
            '[dependencies]\nfmt = { version = "^12", features = [1] }',
        ],
        [
            "path alongside system = true",
            '[dependencies]\nopenssl = { version = ">=3", system = true, path = "p" }',
        ],
        [
            "a path dependency",
            '[dependencies]\nfmt = { version = "^12", path = "../fmt" }',
        ],
        [
            "a workspace dependency",
            "[dependencies]\nfmt = { workspace = true }",
        ],
        [
            "an explicitly disabled workspace dependency",
            "[dependencies]\nfmt = { workspace = false }",
        ],
        [
            "a non-boolean workspace flag",
            '[dependencies]\nfmt = { version = "^12", workspace = "yes" }',
        ],
        [
            "an unknown dependency field",
            '[dependencies]\nfmt = { version = "^12", registry = "x" }',
        ],
        [
            "optional on a dev dependency",
            '[dev-dependencies]\nfmt = { version = "^12", optional = true }',
        ],
        [
            "an empty name in a dependency features list",
            '[dependencies]\nfmt = { version = "^12", features = [""] }',
        ],
        ["a non-string feature entry", "[features]\nx = [1]"],
        ["an empty feature entry", '[features]\nx = [""]'],
        ["a bare dep: entry", '[features]\nx = ["dep:"]'],
        ["an empty forwarding side", '[features]\nx = ["fmt/"]'],
        // The cabin-core entry grammar: identifier charset and at most
        // one scope slash in a dependency reference.
        [
            "an entry outside the identifier charset",
            '[features]\nx = ["we ird!"]',
        ],
        ["a multi-slash forwarding entry", '[features]\nx = ["a/b/c/d"]'],
        ["a multi-slash dep: entry", '[features]\nx = ["dep:a/b/c"]'],
        ["a feature name outside the grammar", '[features]\n"bad name" = []'],
    ];
    for (const [label, toml] of cases) {
        assert.throws(() => extract(toml), new RegExp(CONTEXT), label);
    }
});

test("grouping renders unconditional first, then conditions lexically", () => {
    const info = extract(`
[target.'cfg(os = "linux")'.dependencies]
systemd = { version = ">=240", system = true }

[target.'cfg(family = "unix")'.dependencies]
"cabin-ports/zlib" = "^1.3"

[dependencies]
"cabin-ports/fmt" = "^12"
`);
    const groups = groupDependencies(info.dependencies);
    assert.deepEqual(
        groups.map((group) => ({
            condition: group.condition,
            names: group.entries.map((entry) => entry.name),
        })),
        [
            { condition: null, names: ["cabin-ports/fmt"] },
            { condition: 'family = "unix"', names: ["cabin-ports/zlib"] },
            { condition: 'os = "linux"', names: ["systemd"] },
        ],
    );
});
