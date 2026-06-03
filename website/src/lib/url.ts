/**
 * Parse a value as an absolute http(s) URL. Returns the parsed URL, or null if
 * the value is not a non-empty string, does not parse, or is not http/https.
 *
 * Shared by README image filtering (`markdown.ts`, which drops non-http(s)
 * `src`) and package-metadata link validation (`packages.ts`).
 */
export function parseHttpUrl(value: unknown): URL | null {
    if (typeof value !== "string" || value.trim() === "") {
        return null;
    }

    try {
        const url = new URL(value);
        return url.protocol === "http:" || url.protocol === "https:"
            ? url
            : null;
    } catch {
        return null;
    }
}
