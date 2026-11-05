// node:test suite for classifying the registry's catalog response
// (`npm test`); a fake fetch stands in for the registry.
import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { test } from "node:test";
import { fetchCatalog } from "./fetch-catalog.mjs";

const PRIVATE_ALPHA =
    '{"errors":[{"detail":"the hosted Cabin registry is in private alpha and restricted to its maintainer","code":"registry_private_alpha"}]}';

function respond(...responses) {
    const calls = [];
    const fetchImpl = async (url) => {
        calls.push(url);
        const next = responses.shift();
        if (next instanceof Error) {
            throw next;
        }
        return new Response(next.stream ?? next.body, { status: next.status });
    };
    return {
        calls,
        fetch: (options) =>
            fetchCatalog({ fetchImpl, retryDelayMs: 0, ...options }),
    };
}

test("a 200 is the available catalog, hashed as served", async () => {
    const body = '{"schema":1,"packages":[]}';
    const registry = respond({ status: 200, body });
    const fetched = await registry.fetch();
    assert.equal(fetched.state, "available");
    assert.equal(fetched.body.toString(), body);
    assert.equal(
        fetched.sha256,
        createHash("sha256").update(body).digest("hex"),
    );
});

test("the private-alpha 401 is the unavailable state", async () => {
    const registry = respond({ status: 401, body: PRIVATE_ALPHA });
    assert.deepEqual(await registry.fetch(), { state: "unavailable" });
});

test("any other refusal fails the build without a retry", async () => {
    for (const response of [
        { status: 401, body: '{"errors":[{"detail":"no"}]}' },
        { status: 403, body: PRIVATE_ALPHA },
        { status: 404, body: "" },
    ]) {
        const registry = respond(response);
        await assert.rejects(registry.fetch(), /answered/);
        assert.equal(registry.calls.length, 1);
    }
});

test("network failures and 5xx responses retry a bounded number of times", async () => {
    const recovered = respond(
        new TypeError("reset"),
        { status: 503, body: "" },
        {
            status: 401,
            body: PRIVATE_ALPHA,
        },
    );
    assert.deepEqual(await recovered.fetch(), { state: "unavailable" });
    assert.equal(recovered.calls.length, 3);

    const down = respond(
        { status: 503, body: "" },
        { status: 503, body: "" },
        { status: 503, body: "" },
        { status: 200, body: "{}" },
    );
    await assert.rejects(down.fetch(), /answered 503/);
    assert.equal(down.calls.length, 3);
});

test("a body cut off mid-transfer retries; an oversized one does not", async () => {
    const cutOff = new ReadableStream({
        pull(controller) {
            controller.error(new TypeError("terminated"));
        },
    });
    const recovered = respond(
        { status: 200, stream: cutOff },
        { status: 401, body: PRIVATE_ALPHA },
    );
    assert.deepEqual(await recovered.fetch(), { state: "unavailable" });
    assert.equal(recovered.calls.length, 2);

    let sent = 0;
    const endless = new ReadableStream({
        pull(controller) {
            sent += 1;
            controller.enqueue(new Uint8Array(1024 * 1024));
        },
    });
    const flood = respond({ status: 200, stream: endless });
    await assert.rejects(flood.fetch(), /more than/);
    assert.equal(flood.calls.length, 1);
    // The stream may prefetch a chunk past the cap; it never reads on.
    assert.ok(sent < 70, `read ${sent} MiB before refusing`);
});
