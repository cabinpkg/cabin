import { readdir } from "node:fs/promises";
import path from "node:path";

// Recursively collect every *.html file under `directory`. Shared by the
// post-build verification scripts (CSP inline-script check, docs-link check).
export async function findHtmlFiles(directory) {
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
