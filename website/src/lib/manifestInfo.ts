// A catalog version's dependency and feature declarations, mapped for
// rendering.  The catalog copies them verbatim from the registry's stored
// index entry (crates/cabin-index/src/loader.rs `RawVersion`): registry
// dependencies by kind, a `cfg(...)` condition as its inner expression
// in `target`, and the `[features]` table.  The registry validated all
// of it at publish, so only the shape this module reads is checked.

export type DependencyKind = "normal" | "dev";

export interface DependencyInfo {
    name: string;
    kind: DependencyKind;
    /** The version requirement as the index stores it. */
    req: string;
    /** Inner expression of a `cfg(...)` condition, or null when unconditional. */
    condition: string | null;
    optional: boolean;
    /** Features this edge requests on the dependency. */
    features: string[];
    /** false when the edge disables the dependency's default features. */
    defaultFeatures: boolean;
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
    /** Every declared feature, in the stored (name) order. */
    entries: FeatureInfo[];
}

export interface PackageManifestInfo {
    dependencies: DependencyInfo[];
    /**
     * null when the version declares no features (publish omits an
     * empty table), so callers hide the section.
     */
    features: PackageFeatures | null;
}

export function versionManifestInfo(
    entry: Record<string, unknown>,
    context: string,
): PackageManifestInfo {
    return {
        dependencies: [
            ...registryDependencies(entry.dependencies, "normal", context),
            ...registryDependencies(entry["dev-dependencies"], "dev", context),
        ],
        features: features(entry.features, context),
    };
}

/**
 * Groups dependencies for rendering: the unconditional group first,
 * then one group per `cfg(...)` condition in lexical order, so the
 * rendered page is deterministic.
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

// `{ name: "req" | { version, optional?, features?, default-features?,
// target? } }`
function registryDependencies(
    value: unknown,
    kind: DependencyKind,
    context: string,
): DependencyInfo[] {
    return Object.entries(table(value, kind, context)).map(([name, spec]) => {
        if (typeof spec === "string") {
            return {
                name,
                kind,
                req: spec,
                condition: null,
                optional: false,
                features: [],
                defaultFeatures: true,
            };
        }
        if (
            !isRecord(spec) ||
            typeof spec.version !== "string" ||
            !optional(spec.optional, "boolean") ||
            !optional(spec["default-features"], "boolean") ||
            !optional(spec.target, "string") ||
            !(spec.features === undefined || isStringList(spec.features))
        ) {
            throw new Error(`Malformed dependency ${name} in ${context}.`);
        }
        return {
            name,
            kind,
            req: spec.version,
            condition: (spec.target as string | undefined) ?? null,
            optional: spec.optional === true,
            features: (spec.features as string[] | undefined) ?? [],
            defaultFeatures: spec["default-features"] !== false,
        };
    });
}

// `{ default?: [...], features?: { name: [...] } }` (cabin-core
// `Features`), each part omitted when empty.
function features(value: unknown, context: string): PackageFeatures | null {
    if (value === undefined) {
        return null;
    }
    const block = table(value, "features", context);
    const defaults = block.default ?? [];
    if (
        !isStringList(defaults) ||
        Object.keys(block).some(
            (key) => key !== "default" && key !== "features",
        )
    ) {
        throw new Error(`Malformed default features in ${context}.`);
    }
    return {
        default: defaults,
        entries: Object.entries(table(block.features, "features", context)).map(
            ([name, list]) => {
                if (!isStringList(list)) {
                    throw new Error(`Malformed feature ${name} in ${context}.`);
                }
                return { name, enables: list.map(featureReference) };
            },
        ),
    };
}

function featureReference(entry: string): FeatureReference {
    if (entry.startsWith("dep:")) {
        return { kind: "optional-dependency", dependency: entry.slice(4) };
    }
    const slash = entry.lastIndexOf("/");
    return slash === -1
        ? { kind: "feature", feature: entry }
        : {
              kind: "dependency-feature",
              dependency: entry.slice(0, slash),
              feature: entry.slice(slash + 1),
          };
}

function table(
    value: unknown,
    what: string,
    context: string,
): Record<string, unknown> {
    if (value === undefined) {
        return {};
    }
    if (!isRecord(value)) {
        throw new Error(`Malformed ${what} table in ${context}.`);
    }
    return value;
}

function optional(value: unknown, type: "boolean" | "string"): boolean {
    return value === undefined || typeof value === type;
}

function isStringList(value: unknown): value is string[] {
    return (
        Array.isArray(value) &&
        value.every((entry) => typeof entry === "string")
    );
}

export function isRecord(value: unknown): value is Record<string, unknown> {
    return typeof value === "object" && value !== null && !Array.isArray(value);
}
