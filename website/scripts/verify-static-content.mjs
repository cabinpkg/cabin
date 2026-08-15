// Progressive enhancement, from the visitor's side: the key public
// pages must carry their meaningful content in static HTML, visible
// with JavaScript disabled or broken. The progressive-independence
// check keeps /api/ out of the markup; this one proves the markup that
// remains is worth rendering - a script-less homepage, docs page,
// search page, package page, and 404 each keep their core content, and
// JS-only sections ship `hidden` instead of as empty shells.
import { readdir, readFile, stat } from "node:fs/promises";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { parse } from "parse5";

// What a script-less visitor can read: the page's text and markup with
// the never-rendered subtrees removed - <head>, <script>, <style>,
// <template>, and anything under a `hidden` attribute. CSS-hidden
// content (Tailwind's `hidden` class and its responsive variants) is
// deliberately out of the model: `lg:block` and friends make its
// visibility viewport-dependent, which no lexical check can settle.
//
// parse5 keeps <template> children on `node.content`, which the walk
// never visits - the tag-set entry is what keeps templates excluded if
// a `.content` visit is ever added.
const INVISIBLE_TAGS = new Set(["head", "script", "style", "template"]);

function attribute(node, name) {
    return node.attrs?.find((entry) => entry.name === name)?.value;
}

// Parsed as a script-less browser parses it (scriptingEnabled: false),
// so <noscript> children are real elements: their text and links count
// as visible - they render exactly when scripts do not - and `hidden`
// works inside them. `mainText` is the <main> subtree only: the
// header/footer chrome alone is a few hundred characters on every
// page, so any floor over whole-page text would pass on an empty
// client-rendered shell.
export function visibleContent(html) {
    const headings = [];
    const links = [];
    const textParts = [];
    const mainParts = [];
    const walk = (node, intoHeading, intoLink, inMain) => {
        if (
            INVISIBLE_TAGS.has(node.tagName) ||
            attribute(node, "hidden") !== undefined
        ) {
            return;
        }
        if (node.nodeName === "#text") {
            textParts.push(node.value);
            if (inMain) {
                mainParts.push(node.value);
            }
            intoHeading?.push(node.value);
            intoLink?.push(node.value);
            return;
        }
        // Links carry their visible text and whether they sit in
        // <main>, so callers can require a usable content link - not a
        // bare `<a href>` shell, and not one the shared chrome happens
        // to provide.
        const href = node.tagName === "a" ? attribute(node, "href") : undefined;
        const linkText = href === undefined ? intoLink : [];
        const heading = node.tagName === "h1" ? [] : intoHeading;
        const within = inMain || node.tagName === "main";
        for (const child of node.childNodes ?? []) {
            walk(child, heading, linkText, within);
        }
        if (node.tagName === "h1") {
            headings.push({ text: heading.join(""), main: within });
        }
        if (href !== undefined) {
            links.push({ href, text: linkText.join(""), main: within });
        }
    };
    walk(parse(html, { scriptingEnabled: false }), undefined, undefined, false);
    const normalize = (raw) => raw.replace(/\s+/g, " ").trim();
    const normalized = (entry) => ({ ...entry, text: normalize(entry.text) });
    return {
        text: normalize(textParts.join(" ")),
        mainText: normalize(mainParts.join(" ")),
        headings: headings.map(normalized),
        links: links.map(normalized),
    };
}

// The homepage's registry stats band is registry data end to end, so
// it must ship `hidden` (home-stats.ts reveals it after a successful
// fetch): without JS - or with the registry down - the visitor gets
// the page without the band, never an empty strip. Every match must be
// hidden, so a second, unhidden band cannot mask the first.
export function statsBandShipsHidden(html) {
    let found = null;
    const walk = (node) => {
        if (attribute(node, "data-registry-stats") !== undefined) {
            const hidden = attribute(node, "hidden") !== undefined;
            found = found === null ? hidden : found && hidden;
        }
        for (const child of node.childNodes ?? []) {
            walk(child);
        }
    };
    walk(parse(html, { scriptingEnabled: false }));
    return found;
}

// The <main> content floor: enough visible prose to be worth
// rendering. Deliberately far below what the pages actually carry -
// the check catches a page collapsing to an empty client-rendered
// shell, not copy edits - and chrome never counts toward it.
const MIN_VISIBLE_TEXT = 200;

if (process.argv[1] === fileURLToPath(import.meta.url)) {
    const distDirectory = path.resolve("dist");
    const failures = [];
    const page = async (relativePath) => {
        const html = await readFile(
            path.join(distDirectory, relativePath),
            "utf8",
        );
        const content = visibleContent(html);
        const expect = (condition, requirement) => {
            if (!condition) {
                failures.push(`${relativePath}: ${requirement}`);
            }
        };
        return { html, content, expect };
    };

    {
        const home = await page("index.html");
        home.expect(
            home.content.headings.some((h) => h.main && h.text.length > 0),
            "carries its own non-empty visible <h1> in <main>",
        );
        home.expect(
            home.content.mainText.length >= MIN_VISIBLE_TEXT,
            "carries meaningful visible text without JavaScript",
        );
        home.expect(
            statsBandShipsHidden(home.html) === true,
            "ships the registry stats band hidden (JS reveals it)",
        );
    }
    {
        const docs = await page("docs/index.html");
        docs.expect(
            docs.content.headings.some((h) => h.main && h.text.length > 0),
            "carries its own non-empty visible <h1> in <main>",
        );
        docs.expect(
            docs.content.mainText.length >= MIN_VISIBLE_TEXT,
            "carries meaningful visible text without JavaScript",
        );
    }
    {
        // The first page of packages is server-rendered; script-less
        // visitors still get browsable package links - in <main> and
        // with link text, so neither a chrome link nor a bare
        // `<a href>` shell counts.
        const search = await page("search/index.html");
        search.expect(
            search.content.links.some(
                (link) =>
                    link.main &&
                    link.href.startsWith("/packages/") &&
                    link.text.length > 0,
            ),
            "server-renders at least one usable /packages/ link",
        );
    }
    {
        // Expected package pages come from the ports tree - the same
        // source the build reads - never from what the build happened
        // to emit, so losing route generation (the latest pages or the
        // per-version ones) is a failure instead of a shrunken
        // expectation. Lowercasing and the manifest-only filter mirror
        // src/lib/ports.ts; each page must carry its own package's
        // install snippet, not just any snippet.
        const portsDirectory = path.resolve("..", "ports");
        const subdirectories = async (directory) =>
            (await readdir(directory, { withFileTypes: true }).catch(() => []))
                .filter((entry) => entry.isDirectory())
                .map((entry) => entry.name)
                .sort();
        const expectations = [];
        for (const port of await subdirectories(portsDirectory)) {
            const name = port.toLowerCase();
            const versions = [];
            for (const version of await subdirectories(
                path.join(portsDirectory, port),
            )) {
                const published = await stat(
                    path.join(portsDirectory, port, version, "cabin.toml"),
                ).then(
                    // isFile mirrors ports.ts: a directory named
                    // cabin.toml is not a publishable version.
                    (entry) => entry.isFile(),
                    () => false,
                );
                if (published) {
                    versions.push(version);
                }
            }
            if (versions.length === 0) {
                continue;
            }
            expectations.push({
                segments: ["packages", "cabin-ports", name, "index.html"],
                snippet: `"cabin-ports/${name}" = "=`,
            });
            for (const version of versions) {
                expectations.push({
                    segments: [
                        "packages",
                        "cabin-ports",
                        name,
                        version,
                        "index.html",
                    ],
                    snippet: `"cabin-ports/${name}" = "=${version}"`,
                });
            }
        }
        if (expectations.length === 0) {
            failures.push("ports: no publishable port versions found");
        }
        for (const { segments, snippet } of expectations) {
            const relativePath = path.join(...segments);
            let detail;
            try {
                detail = await page(relativePath);
            } catch {
                failures.push(`${relativePath}: page missing from the build`);
                continue;
            }
            detail.expect(
                detail.content.mainText.includes("[dependencies]") &&
                    detail.content.mainText.includes(snippet),
                "carries this package's install snippet in static text",
            );
        }
    }
    {
        // The source viewer is JS-only by design; the check pins its
        // noscript fallback staying reachable (deleting it, or moving
        // it inside the shell's hidden content slot, loses the link).
        // `main` keeps a chrome /search link from masking a deletion.
        const source = await page("dashboard/source/index.html");
        source.expect(
            source.content.links.some(
                (link) =>
                    link.main &&
                    link.href === "/search" &&
                    link.text.length > 0,
            ),
            "keeps a script-less fallback link to the package index",
        );
    }
    {
        const notFound = await page("404.html");
        notFound.expect(
            notFound.content.mainText.includes("404"),
            "names the 404 in visible text",
        );
        notFound.expect(
            notFound.content.headings.some((h) => h.main && h.text.length > 0),
            "carries its own non-empty visible <h1> in <main>",
        );
    }

    if (failures.length > 0) {
        console.error("Public pages losing their script-less content:");
        for (const failure of failures) {
            console.error(`- ${failure}`);
        }
        process.exit(1);
    }

    console.log("Public pages keep meaningful static content without JS.");
}
