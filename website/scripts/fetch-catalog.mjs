// Fetches the registry's package catalog (docs/remote-registry.md, "The
// catalog") into .catalog/, the one package-data input the build renders
// (src/lib/catalog.ts), with its provenance.  The request is anonymous,
// like every visitor's: the registry's private-alpha refusal is the
// unavailable state, and any other non-200 response fails the build
// rather than rendering a stale or empty catalog.
//
// `--check` runs after the build: it fetches again and fails when the
// registry no longer serves what the build rendered, so a build whose
// catalog went stale while it ran fails instead of deploying over the
// output of a newer one.
import { createHash } from "node:crypto";
import { mkdir, readFile, rm, writeFile } from "node:fs/promises";
import { fileURLToPath } from "node:url";

export const CATALOG_URL = "https://registry.cabinpkg.com/catalog.json";
const CATALOG_DIR = ".catalog";
// Far above the whole corpus's catalog; it bounds what a misbehaving
// response can make the build buffer.
const MAX_BYTES = 64 * 1024 * 1024;
const TIMEOUT_MS = 60_000;
const ATTEMPTS = 3;

// The catalog's state at the registry: `{ state: "available", body,
// sha256 }` for a 200, `{ state: "unavailable" }` for the private-alpha
// refusal.  Transport failures (before or during the body) and 5xx
// responses are retried; every other outcome is final.
export async function fetchCatalog({
    fetchImpl = fetch,
    retryDelayMs = 2_000,
} = {}) {
    for (let attempt = 1; ; attempt++) {
        let response;
        let body;
        try {
            response = await fetchImpl(CATALOG_URL, {
                headers: { accept: "application/json" },
                signal: AbortSignal.timeout(TIMEOUT_MS),
            });
            if (response.status >= 500 && attempt < ATTEMPTS) {
                await response.body?.cancel();
                await sleep(retryDelayMs * attempt);
                continue;
            }
            body = await readCapped(response);
        } catch (error) {
            if (error instanceof TooLarge) {
                throw error;
            }
            if (attempt === ATTEMPTS) {
                throw new Error(`GET ${CATALOG_URL} failed: ${error.message}`);
            }
            await sleep(retryDelayMs * attempt);
            continue;
        }
        if (response.status === 200) {
            return { state: "available", body, sha256: sha256(body) };
        }
        if (response.status === 401 && isPrivateAlphaRefusal(body)) {
            return { state: "unavailable" };
        }
        throw new Error(
            `GET ${CATALOG_URL} answered ${response.status}; only 200 or the private-alpha 401 can build the site.`,
        );
    }
}

// The registry's uniform private-alpha 401 (api/src/error.rs
// PRIVATE_ALPHA_CODE); a 401 without that code is an unexpected refusal.
function isPrivateAlphaRefusal(body) {
    try {
        const envelope = JSON.parse(body.toString("utf8"));
        return envelope?.errors?.[0]?.code === "registry_private_alpha";
    } catch {
        return false;
    }
}

class TooLarge extends Error {}

async function readCapped(response) {
    const chunks = [];
    let total = 0;
    for await (const chunk of response.body ?? []) {
        total += chunk.length;
        if (total > MAX_BYTES) {
            throw new TooLarge(
                `GET ${CATALOG_URL} returned more than ${MAX_BYTES} bytes.`,
            );
        }
        chunks.push(chunk);
    }
    return Buffer.concat(chunks);
}

function sha256(bytes) {
    return createHash("sha256").update(bytes).digest("hex");
}

function sleep(ms) {
    return new Promise((resolve) => setTimeout(resolve, ms));
}

// What the built site records of its catalog (served as
// /catalog-provenance.json): enough to tell whether the registry still
// serves it.
export function provenanceOf(fetched) {
    return fetched.state === "available"
        ? {
              source: "registry",
              url: CATALOG_URL,
              state: "available",
              sha256: fetched.sha256,
          }
        : { source: "registry", url: CATALOG_URL, state: "unavailable" };
}

async function fetchIntoCatalogDir() {
    const fetched = await fetchCatalog();
    await rm(CATALOG_DIR, { recursive: true, force: true });
    await mkdir(CATALOG_DIR);
    if (fetched.state === "available") {
        await writeFile(`${CATALOG_DIR}/catalog.json`, fetched.body);
    }
    const provenance = provenanceOf(fetched);
    await writeFile(
        `${CATALOG_DIR}/provenance.json`,
        `${JSON.stringify(provenance)}\n`,
    );
    console.log(`catalog: ${provenance.state} (${CATALOG_URL})`);
}

async function checkStillCurrent() {
    const built = await readFile(`${CATALOG_DIR}/provenance.json`, "utf8");
    const current = `${JSON.stringify(provenanceOf(await fetchCatalog()))}\n`;
    if (built !== current) {
        throw new Error(
            `The registry's catalog changed during this build (built ${built.trim()}, now ${current.trim()}); refusing to deploy a stale catalog.`,
        );
    }
    console.log("catalog: still current");
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
    await (process.argv[2] === "--check"
        ? checkStillCurrent()
        : fetchIntoCatalogDir());
}
