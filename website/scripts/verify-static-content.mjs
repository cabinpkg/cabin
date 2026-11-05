// Progressive enhancement, from the visitor's side: the key public
// pages must carry their meaningful content in static HTML, visible
// with JavaScript disabled or broken. The progressive-independence
// check keeps /api/ out of the markup; this one proves the markup that
// remains is worth rendering - a script-less homepage, docs page,
// search page, package page, and 404 each keep their core content, and
// JS-only sections ship `hidden` instead of as empty shells.
import { readFile, stat } from "node:fs/promises";
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
    const distDirectory = path.resolve(process.env.SITE_DIST ?? "dist");
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
        // Expectations come from the catalog the build read (.catalog/,
        // src/lib/catalog.ts), never from what the build happened to
        // emit, so losing route generation (the latest pages or the
        // per-version ones) is a failure instead of a shrunken
        // expectation.
        const catalogInput = (file) =>
            readFile(path.resolve(".catalog", file), "utf8");
        const provenance = JSON.parse(await catalogInput("provenance.json"));
        const emitted = await readFile(
            path.join(distDirectory, "catalog-provenance.json"),
            "utf8",
        ).catch(() => "");
        if (emitted !== JSON.stringify(provenance)) {
            failures.push(
                "catalog-provenance.json: does not record the catalog the build read",
            );
        }
        const search = await page("search/index.html");
        const packageLinks = search.content.links.filter(
            (link) =>
                link.main &&
                link.href.startsWith("/packages/") &&
                link.text.length > 0,
        );
        if (provenance.state === "available") {
            const { packages } = JSON.parse(await catalogInput("catalog.json"));
            // The first page of packages is server-rendered; script-less
            // visitors still get browsable package links - in <main> and
            // with link text, so neither a chrome link nor a bare
            // `<a href>` shell counts.
            search.expect(
                packages.length === 0 || packageLinks.length > 0,
                "server-renders at least one usable /packages/ link",
            );
            // scripts/search.ts re-paginates into this element, even
            // when the static render has a single page.
            search.expect(
                search.html.includes('id="pagination"'),
                "ships the #pagination element the search script fills",
            );
            // The search index lists each package at its latest version,
            // in the catalog's order.
            const listed = await readFile(
                path.join(distDirectory, "packages.json"),
                "utf8",
            ).catch(() => "[]");
            if (
                JSON.stringify(
                    JSON.parse(listed).map((item) => [item.name, item.version]),
                ) !==
                JSON.stringify(packages.map((pack) => [pack.name, pack.latest]))
            ) {
                failures.push(
                    "packages.json: does not list the catalog's packages at their latest versions",
                );
            }
            // Each page must carry its own package's install snippet,
            // not just any snippet.
            const expectations = packages.flatMap(
                ({ name, latest, versions }) => [
                    {
                        segments: ["packages", ...name.split("/")],
                        snippet: `"${name}" = "=${latest}"`,
                    },
                    ...versions.map(({ version }) => ({
                        segments: ["packages", ...name.split("/"), version],
                        snippet: `"${name}" = "=${version}"`,
                    })),
                ],
            );
            for (const { segments, snippet } of expectations) {
                const relativePath = path.join(...segments, "index.html");
                let detail;
                try {
                    detail = await page(relativePath);
                } catch {
                    failures.push(
                        `${relativePath}: page missing from the build`,
                    );
                    continue;
                }
                detail.expect(
                    detail.content.mainText.includes("[dependencies]") &&
                        detail.content.mainText.includes(snippet),
                    "carries this package's install snippet in static text",
                );
            }
        } else {
            // While the registry refuses anonymous catalog reads, the
            // site says so and lists nothing.
            search.expect(
                search.html.includes('id="catalog-unavailable"') &&
                    search.content.mainText.includes("private alpha") &&
                    packageLinks.length === 0,
                "says the catalog is unavailable and links no package",
            );
            const listed = await readFile(
                path.join(distDirectory, "packages.json"),
                "utf8",
            );
            if (listed !== "[]") {
                failures.push("packages.json: lists packages");
            }
            if (
                await stat(path.join(distDirectory, "packages")).then(
                    () => true,
                    () => false,
                )
            ) {
                failures.push("packages/: package pages were rendered");
            }
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
