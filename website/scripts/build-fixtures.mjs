// Builds the site once per catalog state into dist-fixtures/<state>/
// and runs `npm run verify` on each: "available" from the committed
// fixtures/catalog.json, "unavailable" as the registry's private-alpha
// refusal leaves it.  The website gate runs this instead of
// `npm run build`, so it needs no network and no live registry.
//
// A fixture never reaches a deploy: this writes no dist/ (the directory
// wrangler deploys), `npm run build` refetches .catalog/ from the
// registry before building, and its final `fetch-catalog.mjs --check`
// fails on any provenance but the registry's current one.
import { execFileSync } from "node:child_process";
import { createHash } from "node:crypto";
import { mkdir, readFile, rm, writeFile } from "node:fs/promises";

// npm's own entry point, so the steps below spawn without a shell on
// every platform (npm is a cmd shim on Windows).
const npm = process.env.npm_execpath;
if (!npm) {
    throw new Error("Run this through `npm run build:fixtures`.");
}
const run = (args, env = {}) =>
    execFileSync(process.execPath, [npm, ...args], {
        stdio: "inherit",
        env: { ...process.env, ...env },
    });

const catalog = await readFile("fixtures/catalog.json");
const states = {
    available: {
        source: "fixture",
        state: "available",
        sha256: createHash("sha256").update(catalog).digest("hex"),
    },
    unavailable: { source: "fixture", state: "unavailable" },
};

for (const [state, provenance] of Object.entries(states)) {
    await rm(".catalog", { recursive: true, force: true });
    await mkdir(".catalog");
    if (provenance.state === "available") {
        await writeFile(".catalog/catalog.json", catalog);
    }
    await writeFile(
        ".catalog/provenance.json",
        `${JSON.stringify(provenance)}\n`,
    );
    const outDir = `dist-fixtures/${state}`;
    run(["exec", "--", "astro", "build", "--outDir", outDir]);
    run(["run", "verify"], { SITE_DIST: outDir });
}
