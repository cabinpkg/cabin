// Fixtures for the progressive-independence matcher: functional /api/
// references must be caught, and prose or look-alike attributes must not
// (the docs legitimately document the API in text).
import assert from "node:assert/strict";
import { test } from "node:test";
import { apiReferences } from "./verify-progressive-independence.mjs";

test("functional /api/ references are caught", () => {
    assert.deepEqual(apiReferences('<a href="/api/v1/user">x</a>'), [
        "href=/api/v1/user",
    ]);
    assert.deepEqual(apiReferences("<form action='/api/v1/user/tokens'>"), [
        "action=/api/v1/user/tokens",
    ]);
    // Legal unquoted attribute values count too.
    assert.deepEqual(apiReferences("<a href=/api/v1/user>x</a>"), [
        "href=/api/v1/user",
    ]);
    assert.deepEqual(apiReferences('<img SRC="/api/v1/blob">'), [
        "src=/api/v1/blob",
    ]);
    // A `>` inside a quoted attribute value must not end the tag scan
    // early and let the reference evade.
    assert.deepEqual(apiReferences('<a title="a > b" href="/api/user">'), [
        "href=/api/user",
    ]);
    assert.deepEqual(apiReferences('<a href="/api/user?q=a>b">'), [
        "href=/api/user?q=a>b",
    ]);
});

test("prose and look-alike attributes are exempt", () => {
    for (const html of [
        // Documentation prose (angle brackets in rendered prose arrive
        // HTML-escaped, so raw text never looks like a tag).
        "<p>the PUT /api/v1/packages route</p>",
        "<p>Use href=/api/user to link.</p>",
        "<code>&lt;a href=&quot;/api/v1/user&quot;&gt;</code>",
        // Non-target attributes, including ones embedding href= text.
        '<div data-href="/api/v1/user">x</div>',
        '<span title="href=/api/user">x</span>',
        // Ordinary asset references.
        '<script src="/_astro/account.js"></script>',
        // Commented-out markup.
        '<!-- <a href="/api/v1/user">x</a> -->',
    ]) {
        assert.deepEqual(apiReferences(html), [], html);
    }
});
