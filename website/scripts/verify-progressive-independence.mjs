// Progressive independence: the marketing pages (everything except the
// account pages, which exist to talk to the registry) must render from
// static HTML that never depends on the registry API - no link, form
// action, or embedded resource pointing under /api/. Prose is exempt on
// purpose (the docs legitimately document the API); this is an HTML-level
// guarantee about the markup, not about optional page scripts - the
// header's auth enhancer is an external script that only upgrades the
// page when the registry answers.
import { readFile } from "node:fs/promises";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { parse } from "parse5";
import { findHtmlFiles } from "./lib/find-html-files.mjs";

// The account pages, and only they, may reference the API. Exact
// allowlist by output path: /dashboard and /settings/** (note that
// /login/denied is a static page and must stay API-free).
function isAccountPage(relativePath) {
    return (
        relativePath === "dashboard/index.html" ||
        relativePath.startsWith("settings/")
    );
}

// A functional reference is an href/src/action/formaction attribute (of a
// real element) whose value points under /api/. The document is parsed
// with a spec-compliant HTML parser, so escaped prose, code samples, and
// comments never register - only actual markup does.
const TARGET_ATTRIBUTES = new Set(["href", "src", "action", "formaction"]);

export function apiReferences(html) {
    const references = [];
    const walk = (node) => {
        for (const attribute of node.attrs ?? []) {
            if (
                TARGET_ATTRIBUTES.has(attribute.name) &&
                attribute.value.includes("/api/")
            ) {
                references.push(`${attribute.name}=${attribute.value}`);
            }
        }
        // <template> children live on the content document fragment.
        if (node.content) {
            walk(node.content);
        }
        for (const child of node.childNodes ?? []) {
            walk(child);
        }
    };
    walk(parse(html));
    return references;
}

// The walk only runs when invoked as a script (npm run verify:progressive);
// the test suite imports apiReferences without touching dist/.
if (process.argv[1] === fileURLToPath(import.meta.url)) {
    const distDirectory = path.resolve("dist");
    const offenders = [];

    for (const filePath of await findHtmlFiles(distDirectory)) {
        const relativePath = path.relative(distDirectory, filePath);
        if (isAccountPage(relativePath)) {
            continue;
        }
        const html = await readFile(filePath, "utf8");
        for (const reference of apiReferences(html)) {
            offenders.push({ relativePath, reference });
        }
    }

    if (offenders.length > 0) {
        console.error("Marketing pages depending on /api/ in their HTML:");
        for (const { relativePath, reference } of offenders) {
            console.error(`- ${relativePath}: ${reference}`);
        }
        process.exit(1);
    }

    console.log("No /api/ references in marketing-page HTML.");
}
