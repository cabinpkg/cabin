// node:test suite for the stats client (`npm test`). The homepage's
// stats band and a package page's downloads are optional enhancements:
// every failure mode must settle to null so the caller renders
// nothing, never partial numbers.
import assert from "node:assert/strict";
import { test } from "node:test";
import type { FetchLike } from "./account.ts";
import { getPackageDownloads, getRegistryStats } from "./stats.ts";

function statsFetch(response: Response | Error): {
    fetch: FetchLike;
    calls: string[];
} {
    const calls: string[] = [];
    const fetch: FetchLike = (input) => {
        calls.push(input);
        return response instanceof Error
            ? Promise.reject(response)
            : Promise.resolve(response);
    };
    return { fetch, calls };
}

const json = (status: number, body: string) =>
    new Response(body, {
        status,
        headers: { "Content-Type": "application/json" },
    });

test("a well-formed 200 yields the stats, from the public endpoint", async () => {
    const { fetch, calls } = statsFetch(
        json(200, '{"packages":3,"versions":7,"downloads":42}'),
    );
    assert.deepEqual(await getRegistryStats(fetch), {
        packages: 3,
        versions: 7,
        downloads: 42,
    });
    assert.deepEqual(calls, ["/api/v1/stats"]);
});

test("every failure mode settles to null", async () => {
    const failures: Array<[string, Response | Error]> = [
        ["registry unreachable", new TypeError("fetch failed")],
        // A well-shaped body on a failure status must still settle to
        // null, so this case exercises the status guard and nothing
        // else (an error-envelope body would pass via the shape guard
        // even without it).
        [
            "non-2xx status",
            json(503, '{"packages":3,"versions":7,"downloads":42}'),
        ],
        ["body is not JSON", json(200, "<!doctype html>")],
        ["body is not an object", json(200, '"ok"')],
        ["body is null", json(200, "null")],
        ["missing fields", json(200, '{"packages":3}')],
        [
            "non-number fields",
            json(200, '{"packages":"3","versions":7,"downloads":42}'),
        ],
    ];
    for (const [label, response] of failures) {
        const { fetch } = statsFetch(response);
        assert.equal(await getRegistryStats(fetch), null, label);
    }
});

test("a package's downloads come from its stats endpoint", async () => {
    const body =
        '{"downloads":12,"days":[{"day":"2026-11-18","downloads":0},{"day":"2026-11-19","downloads":3}]}';
    const { fetch, calls } = statsFetch(json(200, body));
    assert.deepEqual(await getPackageDownloads("fmtlib/fmt", fetch), {
        downloads: 12,
        days: [
            { day: "2026-11-18", downloads: 0 },
            { day: "2026-11-19", downloads: 3 },
        ],
    });
    assert.deepEqual(calls, ["/api/v1/stats/packages/fmtlib/fmt"]);
});

test("every failure of a package's downloads settles to null", async () => {
    const failures: Array<[string, Response | Error]> = [
        ["registry unreachable", new TypeError("fetch failed")],
        ["non-2xx status", json(404, '{"downloads":1,"days":[]}')],
        ["body is not JSON", json(200, "<!doctype html>")],
        ["body is null", json(200, "null")],
        ["no days", json(200, '{"downloads":1}')],
        ["days is not a list", json(200, '{"downloads":1,"days":{}}')],
        [
            "a day without a count",
            json(200, '{"downloads":1,"days":[{"day":"2026-11-19"}]}'),
        ],
        ["no total", json(200, '{"days":[]}')],
        ["a negative total", json(200, '{"downloads":-1,"days":[]}')],
        ["a total past any count", json(200, '{"downloads":1e999,"days":[]}')],
        [
            "a fractional count",
            json(
                200,
                '{"downloads":1,"days":[{"day":"2026-11-19","downloads":0.5}]}',
            ),
        ],
        [
            "a day that is not a date",
            json(
                200,
                '{"downloads":1,"days":[{"day":"yesterday","downloads":1}]}',
            ),
        ],
        [
            "a day that is a number",
            json(
                200,
                '{"downloads":1,"days":[{"day":20261119,"downloads":1}]}',
            ),
        ],
        ["a day that is null", json(200, '{"downloads":1,"days":[null]}')],
    ];
    for (const [label, response] of failures) {
        const { fetch } = statsFetch(response);
        assert.equal(
            await getPackageDownloads("fmtlib/fmt", fetch),
            null,
            label,
        );
    }
});
