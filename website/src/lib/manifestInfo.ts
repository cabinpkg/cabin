// Normalized dependency and feature information extracted from a
// package MANIFEST: dependencies split by kind (normal / dev), system
// dependencies as their own source, `cfg(...)` conditions flattened to
// per-entry inner expressions the way the registry index encodes them
// (crates/cabin-index/src/loader.rs `RawIndexPackageDepTable.target`),
// and the `[features]` table with its entries classified by the
// cabin-core grammar. One shape for the package pages today and
// version comparison later, instead of each surface re-reading raw
// TOML. Index-document-only fields (`standards`, `links`, `yanked`,
// revisions) are deliberately outside this model; extend it when a
// surface needs them.
//
// Structural checks only, mirroring the manifest parser's rules
// (crates/cabin-manifest/src/parse/dependency.rs and the
// feature-entry grammar in crates/cabin-core/src/config.rs): shapes
// the publisher rejects fail the build here. Semantic validation -
// unknown feature references, cycles, `dep:` on a non-optional
// dependency, requirement syntax - stays the publisher's job.

export type DependencyKind = "normal" | "dev";

export interface DependencyInfo {
    name: string;
    kind: DependencyKind;
    /** `registry` resolves through the registry; `system` probes pkg-config. */
    source: "registry" | "system";
    /**
     * The version requirement as the manifest spells it (free-form
     * pkg-config spelling for system deps, where blank means
     * unconstrained). The index stores the parsed
     * `semver::VersionReq`'s serialization instead, so comparing
     * against index metadata must normalize first.
     */
    req: string;
    /** Inner expression of a `cfg(...)` condition, or null when unconditional. */
    condition: string | null;
    optional: boolean;
    /** Features this edge requests on the dependency. */
    features: string[];
    /** false when the edge disables the dependency's default features. */
    defaultFeatures: boolean;
    /** true when the edge opts out of the standard-compatibility check. */
    ignoreInterfaceStandard: boolean;
}

// One entry of a feature's list, classified by the cabin-core grammar
// (crates/cabin-core/src/config.rs `FeatureEntry::parse`): `dep:` is
// checked first (the name may be scoped), then the LAST `/` splits
// dependency from feature (`cabin-ports/zlib/simd` reads as dependency
// `cabin-ports/zlib` + feature `simd`), else a local feature name.
export type FeatureReference =
    | { kind: "feature"; feature: string }
    | { kind: "optional-dependency"; dependency: string }
    | { kind: "dependency-feature"; dependency: string; feature: string };

export interface FeatureInfo {
    name: string;
    enables: FeatureReference[];
}

export interface PackageFeatures {
    /** The reserved `default` list (empty when not declared). */
    default: string[];
    /** Every non-`default` feature, in manifest order. */
    entries: FeatureInfo[];
}

export interface PackageManifestInfo {
    dependencies: DependencyInfo[];
    /**
     * null when the manifest declares no `[features]` table at all,
     * so callers hide the section instead of asserting "no features";
     * a declared-but-empty table stays distinguishable and renders.
     */
    features: PackageFeatures | null;
}

/**
 * Extracts the normalized info from a smol-toml-parsed manifest.
 * Throws on shapes the publisher's manifest parser rejects.
 */
export function extractManifestInfo(
    parsed: unknown,
    context: string,
): PackageManifestInfo {
    const manifest = asRecord(parsed) ?? {};
    const dependencies: DependencyInfo[] = [
        ...dependencyTable(manifest.dependencies, "normal", null, context),
        ...dependencyTable(manifest["dev-dependencies"], "dev", null, context),
        ...conditionedDependencies(manifest.target, context),
    ];
    return {
        dependencies,
        features: featuresTable(manifest.features, context),
    };
}

/**
 * Groups dependencies for rendering: the unconditional group first,
 * then one group per `cfg(...)` condition in lexical order (manifest
 * key order is author-chosen; sorting keeps the rendered page
 * deterministic and stable across edits that only reorder tables).
 */
export function groupDependencies(
    dependencies: DependencyInfo[],
): Array<{ condition: string | null; entries: DependencyInfo[] }> {
    const groups = new Map<string | null, DependencyInfo[]>();
    for (const dependency of dependencies) {
        const entries = groups.get(dependency.condition) ?? [];
        entries.push(dependency);
        groups.set(dependency.condition, entries);
    }
    const conditions = [...groups.keys()].filter(
        (condition): condition is string => condition !== null,
    );
    conditions.sort();
    const ordered: Array<{
        condition: string | null;
        entries: DependencyInfo[];
    }> = [];
    const unconditional = groups.get(null);
    if (unconditional) {
        ordered.push({ condition: null, entries: unconditional });
    }
    for (const condition of conditions) {
        const entries = groups.get(condition);
        if (entries) {
            ordered.push({ condition, entries });
        }
    }
    return ordered;
}

// `[target.'cfg(...)'.dependencies]` / `.dev-dependencies`: the
// manifest nests conditional tables under the cfg key; the index
// document flattens them to per-entry conditions, and so does this.
// Named build targets (`[target.png]`) carry no dependency tables -
// the manifest parser rejects dependencies under one as an almost-
// certain typo of the cfg form (TargetSpecificDependenciesNotSupported),
// so finding them fails the build here too instead of dropping them.
// Cfg keys are sorted for a deterministic flat order.
function conditionedDependencies(
    target: unknown,
    context: string,
): DependencyInfo[] {
    if (target === undefined) {
        return [];
    }
    const tables = asRecord(target);
    if (tables === null) {
        throw new Error(`[target] in ${context} must be a table.`);
    }
    const dependencies: DependencyInfo[] = [];
    for (const key of Object.keys(tables).sort()) {
        const condition = cfgInnerExpression(key);
        const table = asRecord(tables[key]);
        if (table === null) {
            throw new Error(`[target.${key}] in ${context} must be a table.`);
        }
        if (condition === null) {
            for (const kind of [
                "dependencies",
                "dev-dependencies",
                "system-dependencies",
            ]) {
                if (kind in table) {
                    throw new Error(
                        `[target.${key}.${kind}] in ${context} is not supported; conditional dependencies use the [target.'cfg(...)'.${kind}] form.`,
                    );
                }
            }
            continue;
        }
        // The parser's cfg-table shape is deny_unknown_fields with
        // exactly these four keys; anything else - notably
        // `system-dependencies`, since a conditional system dep is
        // spelled `system = true` inside the `dependencies` table -
        // is a publisher rejection, not data to drop.
        for (const field of Object.keys(table)) {
            if (!CONDITIONAL_TARGET_KEYS.has(field)) {
                throw new Error(
                    `[target.${key}] in ${context} declares an unknown field "${field}".`,
                );
            }
        }
        dependencies.push(
            ...dependencyTable(
                table.dependencies,
                "normal",
                condition,
                context,
            ),
            ...dependencyTable(
                table["dev-dependencies"],
                "dev",
                condition,
                context,
            ),
        );
    }
    return dependencies;
}

// The inner expression of a `cfg(...)` target key, verbatim (the
// registry canonicalizes it; re-implementing that grammar here would
// duplicate the parser this loader exists to avoid). Trimmed on both
// levels like the manifest parser (`is_cfg_expression` trims the key,
// `parse_cfg` trims the inner expression), so a padded key is still a
// condition rather than silently becoming a named target.
function cfgInnerExpression(key: string): string | null {
    const trimmed = key.trim();
    return trimmed.startsWith("cfg(") && trimmed.endsWith(")")
        ? trimmed.slice("cfg(".length, -1).trim()
        : null;
}

function dependencyTable(
    table: unknown,
    kind: DependencyKind,
    condition: string | null,
    context: string,
): DependencyInfo[] {
    if (table === undefined) {
        return [];
    }
    const record = asRecord(table);
    if (record === null) {
        // A present-but-malformed section is a publisher rejection;
        // treating it as absent would render "no dependencies" for a
        // manifest whose dependency declaration exists.
        throw new Error(
            `The ${kind === "dev" ? "dev-dependencies" : "dependencies"} table in ${context} must be a table.`,
        );
    }
    return Object.entries(record).map(([name, spec]) =>
        dependencyEntry(name, spec, kind, condition, context),
    );
}

function dependencyEntry(
    name: string,
    spec: unknown,
    kind: DependencyKind,
    condition: string | null,
    context: string,
): DependencyInfo {
    if (!validPackageName(name)) {
        throw new Error(
            `Dependency name "${name}" in ${context} is outside the package-name grammar.`,
        );
    }
    if (typeof spec === "string") {
        return {
            name,
            kind,
            source: "registry",
            req: requirement(name, spec, context),
            condition,
            optional: false,
            features: [],
            defaultFeatures: true,
            ignoreInterfaceStandard: false,
        };
    }
    const table = asRecord(spec);
    if (table === null) {
        throw new Error(
            `Dependency "${name}" in ${context} must be a requirement string or a table.`,
        );
    }
    // The manifest parser's raw table is deny_unknown_fields; a key
    // outside its surface is a publisher rejection, so it fails here
    // instead of being silently ignored.
    for (const key of Object.keys(table)) {
        if (!DEPENDENCY_TABLE_KEYS.has(key)) {
            throw new Error(
                `Dependency "${name}" in ${context} declares an unknown field "${key}".`,
            );
        }
    }
    if (booleanField(name, table, "system", false, context)) {
        // The manifest parser rejects every other source and flag
        // alongside `system = true`, and requires the version (the
        // pkg-config requirement) - mirror both.
        for (const field of [
            "path",
            "workspace",
            "features",
            "default-features",
            "optional",
            "ignore-interface-standard",
        ]) {
            if (field in table) {
                throw new Error(
                    `System dependency "${name}" in ${context} declares "${field}", which is incompatible with system = true.`,
                );
            }
        }
        // Unlike a registry requirement, the pkg-config requirement
        // may be blank: cabin-system-deps `build_constraints` probes
        // an empty or whitespace-only requirement as unconstrained,
        // so only a missing/non-string version is a rejection.
        if (typeof table.version !== "string") {
            throw new Error(
                `System dependency "${name}" in ${context} declares no version requirement.`,
            );
        }
        return {
            name,
            kind,
            source: "system",
            req: table.version,
            condition,
            optional: false,
            features: [],
            defaultFeatures: true,
            ignoreInterfaceStandard: false,
        };
    }
    // A path dependency cannot publish, whatever else it carries.
    // `workspace` selects the unpublishable workspace source only
    // when true: `workspace = false` alongside a version parses as a
    // plain version dependency (ordinary_dep_from_table), and alone
    // it leaves the entry without a source - the requirement check
    // below rejects that like the parser does.
    if ("path" in table) {
        throw new Error(
            `Dependency "${name}" in ${context} declares "path"; published packages carry versioned registry dependencies only.`,
        );
    }
    if (booleanField(name, table, "workspace", false, context)) {
        throw new Error(
            `Dependency "${name}" in ${context} declares "workspace"; published packages carry versioned registry dependencies only.`,
        );
    }
    const optional = booleanField(name, table, "optional", false, context);
    // `optional` is meaningful on normal-kind dependencies only
    // (OptionalNotSupportedForKind).
    if (optional && kind !== "normal") {
        throw new Error(
            `Dependency "${name}" in ${context} declares "optional" on a ${kind} dependency; only normal dependencies can be optional.`,
        );
    }
    return {
        name,
        kind,
        source: "registry",
        req: requirement(name, table.version, context),
        condition,
        optional,
        features: featureList(name, table.features, context),
        defaultFeatures: booleanField(
            name,
            table,
            "default-features",
            true,
            context,
        ),
        ignoreInterfaceStandard: booleanField(
            name,
            table,
            "ignore-interface-standard",
            false,
            context,
        ),
    };
}

const DEPENDENCY_TABLE_KEYS = new Set([
    "path",
    "version",
    "workspace",
    "system",
    "optional",
    "features",
    "default-features",
    "ignore-interface-standard",
]);

// The full `[target.'cfg(...)']` surface (the parser's
// RawConditionalTargetTable): toolchain and profile are real fields
// this page has no use for, so they pass silently rather than
// failing manifests that publish.
const CONDITIONAL_TARGET_KEYS = new Set([
    "dependencies",
    "dev-dependencies",
    "toolchain",
    "profile",
]);

// The dependency-name grammar (cabin-core model.rs
// `PackageName::new`): at most one scope slash, the scope
// GitHub-login-shaped (lowercase/digits/hyphens, at most 39 bytes, no
// edge hyphen), the package part path-safe (ASCII
// letters/digits/_/./-, no leading dot or hyphen - which also covers
// "." and "..").
const PACKAGE_SCOPE = /^[a-z0-9-]{1,39}$/;
const PATH_SAFE_NAME = /^[A-Za-z0-9_.-]+$/;

function validPackageName(name: string): boolean {
    const parts = name.split("/");
    if (parts.length > 2) {
        return false;
    }
    if (parts.length === 2) {
        const scope = parts[0];
        if (
            !PACKAGE_SCOPE.test(scope) ||
            scope.startsWith("-") ||
            scope.endsWith("-")
        ) {
            return false;
        }
    }
    const base = parts[parts.length - 1];
    return (
        PATH_SAFE_NAME.test(base) &&
        !base.startsWith(".") &&
        !base.startsWith("-")
    );
}

// A missing or blank requirement fails the build: Cabin trims, then
// refuses the empty string, and an entry without a version cannot
// publish.
function requirement(name: string, value: unknown, context: string): string {
    if (typeof value !== "string" || value.trim() === "") {
        throw new Error(
            `Dependency "${name}" in ${context} declares no version requirement.`,
        );
    }
    return value;
}

function booleanField(
    name: string,
    table: Record<string, unknown>,
    field: string,
    absent: boolean,
    context: string,
): boolean {
    const value = table[field];
    if (value === undefined) {
        return absent;
    }
    if (typeof value !== "boolean") {
        throw new Error(
            `Dependency "${name}" in ${context} declares a non-boolean "${field}".`,
        );
    }
    return value;
}

function featureList(name: string, value: unknown, context: string): string[] {
    if (value === undefined) {
        return [];
    }
    if (
        !Array.isArray(value) ||
        value.some((entry) => typeof entry !== "string" || entry === "")
    ) {
        throw new Error(
            `Dependency "${name}" in ${context} declares a "features" list with a non-string or empty name.`,
        );
    }
    return value as string[];
}

function featuresTable(
    value: unknown,
    context: string,
): PackageFeatures | null {
    if (value === undefined) {
        return null;
    }
    const table = asRecord(value);
    if (table === null) {
        throw new Error(`[features] in ${context} must be a table.`);
    }
    const features: PackageFeatures = { default: [], entries: [] };
    for (const [name, list] of Object.entries(table)) {
        if (
            !Array.isArray(list) ||
            list.some((entry) => typeof entry !== "string")
        ) {
            throw new Error(
                `Feature "${name}" in ${context} must be a list of strings.`,
            );
        }
        if (name === "default") {
            // `Features::validate` runs the feature-name grammar over
            // the default list too (each entry names a local feature,
            // so `dep:` / `x/y` shapes are outside it); whether the
            // named feature is declared is the semantic half left to
            // the publisher.
            for (const entry of list as string[]) {
                if (!FEATURE_NAME.test(entry)) {
                    throw new Error(
                        `The default feature list in ${context} has an entry "${entry}" outside the feature-name grammar.`,
                    );
                }
            }
            features.default = list as string[];
            continue;
        }
        if (!FEATURE_NAME.test(name)) {
            throw new Error(
                `Feature name "${name}" in ${context} is outside the feature-name grammar.`,
            );
        }
        features.entries.push({
            name,
            enables: (list as string[]).map((entry) =>
                classifyFeatureEntry(name, entry, context),
            ),
        });
    }
    return features;
}

// The feature-name and entry-identifier grammars the manifest parser
// enforces (cabin-core `Features::validate` / `FeatureEntry::parse`):
// names are ASCII letters/digits/_/-; entry identifiers additionally
// allow `.`; a dependency reference is a bare name or one scope slash.
const FEATURE_NAME = /^[A-Za-z0-9_-]+$/;
const ENTRY_IDENTIFIER = /^[A-Za-z0-9_.-]+$/;

function dependencyReference(value: string): boolean {
    const parts = value.split("/");
    return (
        parts.length <= 2 && parts.every((part) => ENTRY_IDENTIFIER.test(part))
    );
}

function classifyFeatureEntry(
    feature: string,
    entry: string,
    context: string,
): FeatureReference {
    const invalid = (): never => {
        throw new Error(
            `Feature "${feature}" in ${context} has an entry "${entry}" outside the feature-entry grammar.`,
        );
    };
    if (entry === "") {
        invalid();
    }
    const optionalDependency = entry.startsWith("dep:")
        ? entry.slice("dep:".length)
        : null;
    if (optionalDependency !== null) {
        if (!dependencyReference(optionalDependency)) {
            invalid();
        }
        return { kind: "optional-dependency", dependency: optionalDependency };
    }
    const lastSlash = entry.lastIndexOf("/");
    if (lastSlash !== -1) {
        const dependency = entry.slice(0, lastSlash);
        const forwarded = entry.slice(lastSlash + 1);
        if (
            !dependencyReference(dependency) ||
            !ENTRY_IDENTIFIER.test(forwarded)
        ) {
            invalid();
        }
        return { kind: "dependency-feature", dependency, feature: forwarded };
    }
    if (!ENTRY_IDENTIFIER.test(entry)) {
        invalid();
    }
    return { kind: "feature", feature: entry };
}

function asRecord(value: unknown): Record<string, unknown> | null {
    if (typeof value !== "object" || value === null || Array.isArray(value)) {
        return null;
    }
    // smol-toml models TOML datetimes as Date subclasses; only a
    // plain object is a table (a datetime would otherwise read as an
    // empty one and dodge the must-be-a-table rejections).
    const proto = Object.getPrototypeOf(value);
    if (proto !== Object.prototype && proto !== null) {
        return null;
    }
    return value as Record<string, unknown>;
}
