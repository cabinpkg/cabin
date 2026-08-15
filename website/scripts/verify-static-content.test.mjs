// Fixtures for the static-content extractor: what a script-less visitor
// sees must count, and what never renders (scripts, templates, hidden
// subtrees) must not.
import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { test } from "node:test";
import {
    statsBandShipsHidden,
    visibleContent,
} from "./verify-static-content.mjs";

test("visible text keeps prose and noscript, drops never-rendered subtrees", () => {
    const { text } = visibleContent(
        "<head><title>Tab title</title><style>.a{}</style></head>" +
            "<body><p>Readable   prose.</p>" +
            "<noscript>Scriptless fallback.</noscript>" +
            "<script>const js = 'invisible';</script>" +
            "<template><p>Cloned by JS only.</p></template>" +
            "<div hidden><p>Revealed by JS only.</p></div>" +
            "<div data-x><span hidden>also hidden</span>kept</div></body>",
    );
    assert.equal(text, "Readable prose. Scriptless fallback. kept");
});

test("noscript children parse as real markup: links count, hidden works, tags do not become text", () => {
    const { text, links } = visibleContent(
        "<body><noscript>" +
            '<div class="chrome-soup"><a href="/packages/cabin-ports/zlib">zlib fallback</a></div>' +
            "<span hidden>js-only inside noscript</span>" +
            "</noscript></body>",
    );
    assert.deepEqual(links, [
        {
            href: "/packages/cabin-ports/zlib",
            text: "zlib fallback",
            main: false,
        },
    ]);
    // No tag or class soup leaks into text, and hidden still hides.
    assert.equal(text, "zlib fallback");
});

test("mainText and link placement track the main subtree, never the chrome", () => {
    const { text, mainText, links } = visibleContent(
        '<body><header>Site chrome <a href="/docs/">docs</a></header>' +
            '<main id="main-content"><p>The page\'s actual content.</p>' +
            '<a href="/packages/cabin-ports/zlib">zlib</a></main>' +
            "<footer>copyright chrome</footer></body>",
    );
    assert.equal(mainText, "The page's actual content. zlib");
    assert.equal(
        text,
        "Site chrome docs The page's actual content. zlib copyright chrome",
    );
    assert.deepEqual(links, [
        { href: "/docs/", text: "docs", main: false },
        { href: "/packages/cabin-ports/zlib", text: "zlib", main: true },
    ]);
});

test("headings collect visible h1 text and their main placement", () => {
    const { headings } = visibleContent(
        "<body><h1>The <em>chrome</em></h1>" +
            "<main><h1>The page</h1></main>" +
            "<div hidden><h1>JS-only</h1></div><h2>sub</h2></body>",
    );
    assert.deepEqual(headings, [
        { text: "The chrome", main: false },
        { text: "The page", main: true },
    ]);
});

test("links collect visible hrefs with their visible text", () => {
    const { links } = visibleContent(
        '<a href="/packages/cabin-ports/zlib">zlib<span hidden>-js</span></a>' +
            '<a href="/empty-shell"></a>' +
            '<div hidden><a href="/packages/js-only">x</a></div>' +
            '<template><a href="/packages/template-only">x</a></template>' +
            "<a>no href</a>",
    );
    // An empty anchor keeps its href but carries no text, so callers
    // can refuse bare link shells.
    assert.deepEqual(links, [
        { href: "/packages/cabin-ports/zlib", text: "zlib", main: false },
        { href: "/empty-shell", text: "", main: false },
    ]);
});

test("the stats band must ship hidden - every instance of it", () => {
    assert.equal(
        statsBandShipsHidden("<section data-registry-stats hidden></section>"),
        true,
    );
    assert.equal(
        statsBandShipsHidden("<section data-registry-stats></section>"),
        false,
    );
    // A later hidden band must not mask an earlier unhidden one (or
    // vice versa): one visible instance is a failure.
    assert.equal(
        statsBandShipsHidden(
            "<section data-registry-stats></section>" +
                "<section data-registry-stats hidden></section>",
        ),
        false,
    );
    assert.equal(
        statsBandShipsHidden(
            "<section data-registry-stats hidden></section>" +
                "<section data-registry-stats></section>",
        ),
        false,
    );
    // Absent counts as a failure at the call site (`=== true`): the
    // check pins the band's presence-and-hidden shape, not just the
    // attribute.
    assert.equal(statsBandShipsHidden("<section></section>"), null);
});

// The 404 page's HTTP status comes from the host, not the HTML:
// Workers Static Assets serves dist/404.html with a real 404 status
// only under `assets.not_found_handling`, so losing it would keep the
// page while breaking the status semantics. Comments are stripped
// (never after `:`, so URL values survive) and the remainder parsed
// as JSON - Biome formats the file without trailing commas - which
// scopes the assertion to the assets object and turns any decoy or
// surviving comment into a loud failure, never a false pass.
test("wrangler serves the built 404 page with a 404 status", async () => {
    const wrangler = await readFile(
        new URL("../wrangler.jsonc", import.meta.url),
        "utf8",
    );
    const parsed = JSON.parse(
        wrangler
            .replace(/\/\*[\s\S]*?\*\//g, "")
            .replace(/(^|[\s,{[])\/\/.*$/gm, "$1"),
    );
    assert.equal(parsed.assets.not_found_handling, "404-page");
});
