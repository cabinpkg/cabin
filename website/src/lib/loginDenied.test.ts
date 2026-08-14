// node:test suite for the denied page's account-age param validation
// (`npm test`): only the registry's exact refusal shape yields a date.
import assert from "node:assert/strict";
import { test } from "node:test";

import { accountAgeEligibleDate } from "./loginDenied.ts";

const params = (query: string): URLSearchParams => new URLSearchParams(query);

test("returns the date for the registry's exact refusal shape", () => {
    assert.equal(
        accountAgeEligibleDate(
            params("?reason=account-age&eligible=2026-09-13"),
        ),
        "2026-09-13",
    );
});

test("ignores every other reason, including none", () => {
    assert.equal(accountAgeEligibleDate(params("")), null);
    assert.equal(accountAgeEligibleDate(params("?eligible=2026-09-13")), null);
    assert.equal(
        accountAgeEligibleDate(params("?reason=other&eligible=2026-09-13")),
        null,
    );
});

test("rejects dates not in the registry's yyyy-mm-dd shape", () => {
    for (const eligible of [
        "",
        "soon",
        "2026-9-13",
        "2026-09-13T00:00:00Z",
        "2026-09-13 or never",
        "<b>2026-09-13</b>",
        "13-09-2026",
    ]) {
        const query = new URLSearchParams({ reason: "account-age", eligible });
        assert.equal(accountAgeEligibleDate(query), null, eligible);
    }
    assert.equal(accountAgeEligibleDate(params("?reason=account-age")), null);
});
