import { readdir, readFile } from "node:fs/promises";
import path from "node:path";

const distDirectory = path.resolve("dist");
const inlineScripts = [];

for (const filePath of await findHtmlFiles(distDirectory)) {
    const html = await readFile(filePath, "utf8");
    const scriptTags = html.matchAll(/<script\b([^>]*)>/gi);

    for (const match of scriptTags) {
        const attributes = match[1] ?? "";
        if (!/\ssrc\s*=/i.test(attributes)) {
            inlineScripts.push({
                filePath,
                tag: match[0],
            });
        }
    }
}

if (inlineScripts.length > 0) {
    console.error("Inline <script> tags found in built HTML:");
    for (const { filePath, tag } of inlineScripts) {
        console.error(`- ${path.relative(process.cwd(), filePath)}: ${tag}`);
    }
    process.exit(1);
}

console.log("No inline <script> tags found in dist/**/*.html.");

async function findHtmlFiles(directory) {
    const entries = await readdir(directory, { withFileTypes: true });
    const files = [];

    for (const entry of entries) {
        const entryPath = path.join(directory, entry.name);
        if (entry.isDirectory()) {
            files.push(...(await findHtmlFiles(entryPath)));
        } else if (entry.isFile() && entry.name.endsWith(".html")) {
            files.push(entryPath);
        }
    }

    return files;
}
