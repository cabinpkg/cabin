// Checks out the cabin-ports/ports commit that ../ports-revision pins
// into .ports/, the catalog src/lib/ports.ts and verify-static-content.mjs
// read. A reused checkout is one already at that exact commit with an
// unmodified tree; any failure fails the build rather than rendering an
// empty or stale catalog.
import { execFileSync } from "node:child_process";
import { existsSync, readFileSync, rmSync } from "node:fs";
import { fileURLToPath } from "node:url";

const REPOSITORY = "https://github.com/cabin-ports/ports";

const revision = readFileSync(
    new URL("../../ports-revision", import.meta.url),
    "utf8",
).trim();
if (!/^[0-9a-f]{40}$/.test(revision)) {
    throw new Error(
        `ports-revision must hold one full commit id, not ${JSON.stringify(revision)}`,
    );
}

// Hooks and `git rebase -x` export GIT_DIR and friends, which would
// point every command below at the enclosing cabin repository.
const env = { ...process.env };
for (const name of execFileSync("git", ["rev-parse", "--local-env-vars"], {
    encoding: "utf8",
}).split("\n")) {
    delete env[name];
}

const checkout = fileURLToPath(new URL("../.ports", import.meta.url));
const git = (...args) =>
    execFileSync("git", ["-C", checkout, ...args], {
        encoding: "utf8",
        env,
        stdio: ["ignore", "pipe", "inherit"],
    }).trim();

// Without its own .git, `git -C` would answer for the enclosing cabin
// checkout; an interrupted fetch leaves one without a HEAD.
let current = null;
if (existsSync(`${checkout}/.git`)) {
    try {
        if (git("status", "--porcelain", "--untracked-files=all") === "") {
            current = git("rev-parse", "--verify", "--quiet", "HEAD");
        }
    } catch {}
}
if (current !== revision) {
    rmSync(checkout, { recursive: true, force: true });
    execFileSync("git", ["init", "--quiet", checkout], { env });
    // Only a commit on main: its history is never rewritten or deleted,
    // so every pin a cabin commit names stays fetchable.
    git("fetch", "--quiet", REPOSITORY, "main");
    try {
        git("merge-base", "--is-ancestor", revision, "FETCH_HEAD");
    } catch {
        throw new Error(`${revision} is not on ${REPOSITORY}'s main`);
    }
    git("-c", "advice.detachedHead=false", "checkout", "--quiet", revision);
}
console.log(`ports: ${REPOSITORY} at ${revision}`);
