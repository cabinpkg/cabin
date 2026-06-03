import { readFile } from "node:fs/promises";
import path from "node:path";
import { findHtmlFiles } from "./lib/find-html-files.mjs";

// Validates the built docs in two ways:
//  1. Every internal `/docs/...` link (sidebar, nav, rewritten cross-links)
//     resolves to a generated docs page.
//  2. No relative `*.md` link survives in a docs page — that would mean the
//     remark rewriter did not run, and the link would 404 in the browser.

const distDirectory = path.resolve("dist");
const docsDirectory = path.join(distDirectory, "docs");

const generatedPages = new Set();
for (const filePath of await findHtmlFiles(docsDirectory)) {
    if (path.basename(filePath) !== "index.html") {
        continue;
    }
    const relativeDir = path.relative(distDirectory, path.dirname(filePath));
    generatedPages.add(`/${relativeDir.split(path.sep).join("/")}/`);
}

const docsLink = /href="(\/docs\/[^"#]*)(?:#[^"]*)?"/gi;
// A relative Markdown target (not absolute, protocol-relative, root-relative,
// in-page, or mailto) — exactly what the rewriter is responsible for.
const unrewrittenMarkdownLink =
    /href="(?!https?:|\/\/|\/|#|mailto:)([^"]*\.md(?:#[^"]*)?)"/gi;

const problems = [];

for (const filePath of await findHtmlFiles(distDirectory)) {
    const html = await readFile(filePath, "utf8");
    const where = path.relative(process.cwd(), filePath);
    const inDocs = filePath.startsWith(`${docsDirectory}${path.sep}`);

    for (const match of html.matchAll(docsLink)) {
        const target = match[1].endsWith("/") ? match[1] : `${match[1]}/`;
        if (!generatedPages.has(target)) {
            problems.push(`${where}: unresolved docs link -> ${match[1]}`);
        }
    }

    if (inDocs) {
        for (const match of html.matchAll(unrewrittenMarkdownLink)) {
            problems.push(
                `${where}: un-rewritten relative Markdown link -> ${match[1]}`,
            );
        }
    }
}

if (problems.length > 0) {
    console.error("Docs link check failed:");
    for (const problem of problems) {
        console.error(`- ${problem}`);
    }
    process.exit(1);
}

console.log(
    `All internal docs links resolve (${generatedPages.size} docs pages).`,
);
