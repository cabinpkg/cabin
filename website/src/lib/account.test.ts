// node:test suite for the account API client (`yarn test`). Runs on
// plain Node (>= 22.18 strips the type annotations natively), so the
// module under test stays dependency-free and fetch is mocked by hand.
import assert from "node:assert/strict";
import { test } from "node:test";
import {
    type ApiResult,
    asOutcome,
    createToken,
    type FetchLike,
    getUser,
    resolveAuth,
    revokeToken,
} from "./account.ts";

interface Call {
    input: string;
    init: RequestInit | undefined;
}

function jsonResponse(status: number, body: unknown): Response {
    return new Response(JSON.stringify(body), {
        status,
        headers: { "Content-Type": "application/json" },
    });
}

// A fetch double that serves the queued responses in order and records
// every call for assertions.
function mockFetch(...responses: Array<Response | Error>): {
    fetch: FetchLike;
    calls: Call[];
} {
    const calls: Call[] = [];
    const fetch: FetchLike = (input, init) => {
        calls.push({ input, init });
        const next = responses.shift();
        if (next === undefined) {
            throw new Error("mockFetch ran out of responses");
        }
        return next instanceof Error
            ? Promise.reject(next)
            : Promise.resolve(next);
    };
    return { fetch, calls };
}

const envelope = (detail: string) => ({ errors: [{ detail }] });

test("resolveAuth settles signed-in on a 200 user payload", async () => {
    const user = { github_id: 1, login: "octocat", plan: "free" };
    const { fetch, calls } = mockFetch(jsonResponse(200, user));

    assert.deepEqual(await resolveAuth(fetch), {
        state: "signed-in",
        user,
    });
    // The session cookie must ride along.
    assert.equal(calls[0]?.input, "/api/v1/user");
    assert.equal(calls[0]?.init?.credentials, "same-origin");
});

test("resolveAuth settles signed-out on 401", async () => {
    const { fetch } = mockFetch(
        jsonResponse(401, envelope("authentication required")),
    );

    assert.deepEqual(await resolveAuth(fetch), { state: "signed-out" });
});

test("resolveAuth settles error on a non-401 failure, reusing the detail", async () => {
    const { fetch } = mockFetch(jsonResponse(500, envelope("internal error")));

    assert.deepEqual(await resolveAuth(fetch), {
        state: "error",
        message: "internal error",
    });
});

test("resolveAuth settles error when the registry is unreachable", async () => {
    const { fetch } = mockFetch(new TypeError("fetch failed"));

    assert.deepEqual(await resolveAuth(fetch), {
        state: "error",
        message: "the registry could not be reached",
    });
});

test("a failure without the error envelope falls back to the status", async () => {
    const { fetch } = mockFetch(
        new Response("<html>bad gateway</html>", { status: 502 }),
    );

    const result = await getUser(fetch);
    assert.deepEqual(result, {
        ok: false,
        status: 502,
        message: "the registry answered 502",
    });
});

test("createToken sends the CSRF pair and returns the plaintext once", async () => {
    const created = {
        id: "abc",
        name: "ci",
        scopes: ["publish", "yank"],
        token: "cabin_secret",
    };
    const { fetch, calls } = mockFetch(jsonResponse(201, created));

    const result = await createToken(fetch, "ci", ["publish", "yank"]);
    assert.deepEqual(result, { ok: true, data: created });

    const init = calls[0]?.init;
    assert.equal(calls[0]?.input, "/api/v1/user/tokens");
    assert.equal(init?.method, "POST");
    assert.equal(init?.credentials, "same-origin");
    const headers = init?.headers as Record<string, string>;
    assert.equal(headers["Content-Type"], "application/json");
    assert.equal(headers["X-CSRF-Protection"], "1");
    assert.deepEqual(JSON.parse(String(init?.body)), {
        name: "ci",
        scopes: ["publish", "yank"],
    });
});

test("createToken surfaces the API's validation detail verbatim", async () => {
    const detail =
        'the body must be {"name": <1-64 chars>, "scopes": [..]} with scopes' +
        ' drawn from "publish", "yank", "verify"';
    const { fetch } = mockFetch(jsonResponse(400, envelope(detail)));

    const result = await createToken(fetch, "", []);
    assert.deepEqual(result, { ok: false, status: 400, message: detail });
});

test("quota refusals keep the API's detail and code-carrying envelope", async () => {
    const detail =
        "publish rate limit exceeded; retry after the token bucket refills";
    const { fetch } = mockFetch(
        jsonResponse(429, {
            errors: [{ detail, code: "rate_limited" }],
        }),
    );

    const result: ApiResult<unknown> = await getUser(fetch);
    assert.deepEqual(result, { ok: false, status: 429, message: detail });
});

test("revokeToken hits the id's revoke route with the CSRF pair", async () => {
    const { fetch, calls } = mockFetch(jsonResponse(200, { ok: true }));

    const result = await revokeToken(fetch, "a/b");
    assert.deepEqual(result, { ok: true, data: { ok: true } });
    // The id is URL-encoded, never spliced raw into the path.
    assert.equal(calls[0]?.input, "/api/v1/user/tokens/a%2Fb/revoke");
    const headers = calls[0]?.init?.headers as Record<string, string>;
    assert.equal(headers["X-CSRF-Protection"], "1");
    // The declared JSON content type stays truthful: a JSON body rides.
    assert.equal(String(calls[0]?.init?.body), "{}");
});

test("the create flow settles created, signed-out, or failed", async () => {
    // Success carries the plaintext through to the page...
    const created = { id: "a", name: "ci", scopes: [], token: "cabin_x" };
    let { fetch } = mockFetch(jsonResponse(201, created));
    assert.deepEqual(asOutcome(await createToken(fetch, "ci", [])), {
        state: "done",
        data: created,
    });

    // ...a 401 mid-operation sends the whole page signed-out...
    ({ fetch } = mockFetch(
        jsonResponse(401, envelope("authentication required")),
    ));
    assert.deepEqual(asOutcome(await createToken(fetch, "ci", [])), {
        state: "signed-out",
    });

    // ...and any other refusal surfaces the API's detail as the notice.
    ({ fetch } = mockFetch(jsonResponse(403, envelope("csrf detail"))));
    assert.deepEqual(asOutcome(await createToken(fetch, "ci", [])), {
        state: "failed",
        message: "csrf detail",
    });
});
